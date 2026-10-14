//! Recordings of an Agent Display: a video through ScreenCaptureKit's
//! recording output, and beside it a timeline, one JSON line per agent
//! action with its offset into the video.
//!
//! A recording has a stream of its own at the display's full size, so the
//! Viewer's and thumbnails' rates and sizes don't change the video. It ends
//! when the agent stops it, when the display ends, or after `LONGEST`.

use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    RecordingCallbacks, SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};
use serde_json::{Value, json};
use sidescreen_model::model::DisplayId;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A recording stops by itself after this long, so a forgotten one doesn't
/// fill the disk.
pub const LONGEST: Duration = Duration::from_secs(30 * 60);

struct Recording {
    stream: SCStream,
    output: SCRecordingOutput,
    video: PathBuf,
    timeline: PathBuf,
    /// When the first frame went into the video; offsets count from it.
    began: Arc<Mutex<Option<Instant>>>,
    asked: Instant,
    finished: mpsc::Receiver<Result<(), String>>,
    actions: usize,
}

static RECORDINGS: Mutex<Option<HashMap<DisplayId, Recording>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut HashMap<DisplayId, Recording>) -> T) -> T {
    let mut all = RECORDINGS.lock().unwrap_or_else(|e| e.into_inner());
    f(all.get_or_insert_with(HashMap::new))
}

/// What a finished recording left.
pub struct Done {
    pub video: PathBuf,
    pub timeline: PathBuf,
    pub seconds: f64,
    pub actions: usize,
    /// Why the video may be unusable, if ScreenCaptureKit said so.
    pub problem: Option<String>,
}

/// The folder recordings go to: ~/Movies/Sidescreen.
pub fn folder() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join("Movies").join("Sidescreen")
}

pub fn is_recording(id: DisplayId) -> bool {
    with(|all| all.contains_key(&id))
}

/// Start recording display `id`, whose label names the files. Blocking.
pub fn start(id: DisplayId, label: &str) -> Result<(PathBuf, PathBuf), String> {
    if is_recording(id) {
        return Err("display is already being recorded; stop that recording first".into());
    }
    if !SCRecordingOutput::is_available() {
        return Err("recording needs macOS 15 or later".into());
    }
    let folder = folder();
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("can't make {}: {e}", folder.display()))?;
    let stamp = chrono_like_stamp();
    let name: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(60)
        .collect();
    let base = format!("{name} {stamp}");
    let video = folder.join(format!("{base}.mp4"));
    let timeline = folder.join(format!("{base}.timeline.jsonl"));

    let bounds = super::cg::display_bounds(id);
    let pixels = super::cg::pixel_size(id).unwrap_or((bounds.width as u32, bounds.height as u32));
    let filter = super::screenshot::filter(id, true, false)?;
    let configuration = SCStreamConfiguration::new()
        .with_width(pixels.0)
        .with_height(pixels.1)
        .with_pixel_format(PixelFormat::BGRA)
        .with_minimum_frame_interval(&CMTime::new(1, 30))
        .with_shows_cursor(false)
        .with_queue_depth(6);
    let stream = SCStream::new(&filter, &configuration).map_err(|e| e.to_string())?;
    let output_configuration = SCRecordingOutputConfiguration::new()
        .map_err(|e| e.to_string())?
        .with_output_url(&video)
        .map_err(|e| e.to_string())?
        .with_video_codec(SCRecordingOutputCodec::H264)
        .with_output_file_type(SCRecordingOutputFileType::MP4);
    let began = Arc::new(Mutex::new(None));
    let (done, finished) = mpsc::channel();
    let failed = done.clone();
    let start_mark = began.clone();
    let callbacks = RecordingCallbacks::new()
        .on_start(move || {
            start_mark.lock().unwrap().get_or_insert_with(Instant::now);
        })
        .on_fail(move |error| {
            log::warn!("recording of display {id} failed: {error}");
            let _ = failed.send(Err(error));
        })
        .on_finish(move || {
            let _ = done.send(Ok(()));
        });
    let output = SCRecordingOutput::new_with_delegate(&output_configuration, callbacks)
        .ok_or("ScreenCaptureKit wouldn't make a recording output")?;
    stream
        .add_recording_output(&output)
        .map_err(|e| e.to_string())?;
    stream.start_capture().map_err(|e| e.to_string())?;
    std::fs::write(&timeline, "")
        .map_err(|e| format!("can't write {}: {e}", timeline.display()))?;
    log::info!("recording display {id} to {}", video.display());
    let asked = Instant::now();
    with(|all| {
        all.insert(
            id,
            Recording {
                stream,
                output,
                video: video.clone(),
                timeline: timeline.clone(),
                began,
                asked,
                finished,
                actions: 0,
            },
        )
    });
    // Stop a forgotten recording.
    std::thread::spawn(move || {
        std::thread::sleep(LONGEST);
        if with(|all| all.get(&id).is_some_and(|r| r.asked == asked)) {
            log::info!(
                "recording of display {id} reached {} min; stopping it",
                LONGEST.as_secs() / 60
            );
            stop(id);
        }
    });
    Ok((video, timeline))
}

/// Add a line to display `id`'s timeline, if it is being recorded:
/// `event` with `t`, the seconds into the video.
pub fn note(id: DisplayId, mut event: Value) {
    with(|all| {
        let Some(recording) = all.get_mut(&id) else {
            return;
        };
        let began = *recording.began.lock().unwrap();
        let t = began.map_or(0.0, |b| b.elapsed().as_secs_f64());
        if let Value::Object(map) = &mut event {
            map.insert("t".into(), json!((t * 1000.0).round() / 1000.0));
        }
        let line = event.to_string();
        match std::fs::OpenOptions::new()
            .append(true)
            .open(&recording.timeline)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{line}");
                recording.actions += 1;
            }
            Err(error) => log::warn!("timeline of display {id}: {error}"),
        }
    });
}

/// Stop recording display `id` and wait, at most 5 s, for the video to be
/// written. None if it wasn't being recorded. Blocking.
pub fn stop(id: DisplayId) -> Option<Done> {
    let recording = with(|all| all.remove(&id))?;
    let seconds = recording
        .began
        .lock()
        .unwrap()
        .map_or(0.0, |b| b.elapsed().as_secs_f64());
    let _ = recording.stream.remove_recording_output(&recording.output);
    let _ = recording.stream.stop_capture();
    let problem = match recording.finished.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some("the video took more than 5 s to finish writing".into()),
    };
    log::info!(
        "recorded display {id}: {:.1} s, {} actions, {}",
        seconds,
        recording.actions,
        recording.video.display()
    );
    Some(Done {
        video: recording.video,
        timeline: recording.timeline,
        seconds,
        actions: recording.actions,
        problem,
    })
}

/// Local time as 2026-10-15 02-30-05, for file names.
fn chrono_like_stamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
