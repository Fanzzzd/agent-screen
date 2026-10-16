//! Recordings of an Agent Display: a video through ScreenCaptureKit's
//! recording output, and beside it a timeline, one JSON line per agent
//! action with its offset into the video.
//!
//! A recording has a stream of its own at the display's full size, so the
//! Viewer's and thumbnails' rates and sizes don't change the video. It ends
//! when the agent stops it, when the display ends, or after `LONGEST`.
//!
//! Its sound comes from a second stream that hears only the apps whose
//! every window is on the display (`model::heard_apps`): macOS separates
//! audio by app, not by window, so an app the user also has open stays
//! silent. The stream follows apps as they come and go; at the end the
//! sound goes into the video (`mux`).

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

/// The sound side of a recording.
struct Sound {
    stream: SCStream,
    output: SCRecordingOutput,
    file: PathBuf,
    began: Arc<Mutex<Option<Instant>>>,
    finished: mpsc::Receiver<Result<(), String>>,
    /// Names of the apps heard at any point, and of those left out.
    heard: Arc<Mutex<std::collections::BTreeSet<String>>>,
    left_out: Arc<Mutex<std::collections::BTreeSet<String>>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

struct Recording {
    sound: Option<Sound>,
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
    /// The apps whose sound is in the video, and those left out because
    /// they also have windows elsewhere.
    pub heard: Vec<String>,
    pub left_out: Vec<String>,
    /// Why the video has no sound, when it has none for a reason.
    pub sound_problem: Option<String>,
}

/// The apps display `id` lets a recording hear, as ScreenCaptureKit
/// knows them, with the names of those heard and left out.
fn heard_apps(
    id: DisplayId,
    content: &SCShareableContent,
) -> (Vec<SCRunningApplication>, Vec<String>, Vec<String>) {
    let shown = super::cg::on_screen_ids();
    let windows: Vec<(i32, sidescreen_model::model::Rect, bool)> = super::cg::all_windows()
        .into_iter()
        .map(|w| (w.target.pid, w.bounds, shown.contains(&w.target.window_id)))
        .collect();
    let area = super::cg::display_bounds(id);
    let (heard, left_out) = sidescreen_model::model::heard_apps(&windows, &area);
    log::debug!("display {id} at {area:?}: hears pids {heard:?}, leaves out {left_out:?}");
    let apps = content.applications();
    let name = |pid: &i32| {
        apps.iter()
            .find(|a| a.process_id() == *pid)
            .map(|a| a.application_name())
            .unwrap_or_else(|| format!("pid {pid}"))
    };
    let names = heard.iter().map(name).collect();
    let left = left_out.iter().map(name).collect();
    let chosen = apps
        .into_iter()
        .filter(|a| heard.contains(&a.process_id()))
        .collect();
    (chosen, names, left)
}

fn sound_filter(
    display: &SCDisplay,
    apps: &[SCRunningApplication],
) -> Result<SCContentFilter, String> {
    let refs: Vec<&SCRunningApplication> = apps.iter().collect();
    SCContentFilter::create()
        .with_display(display)
        .with_including_windows(&[])
        .with_including_applications(&refs, &[])
        .build()
        .map_err(|e| e.to_string())
}

/// Start the sound stream for display `id`, writing to `file`.
fn start_sound(id: DisplayId, file: &Path) -> Result<Sound, String> {
    let content = SCShareableContent::get().map_err(|e| e.to_string())?;
    let display = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == id)
        .ok_or("the display isn't listed for capture")?;
    let (apps, names, left) = heard_apps(id, &content);
    let filter = sound_filter(&display, &apps)?;
    // The picture is a token: SCRecordingOutput needs one.
    let configuration = SCStreamConfiguration::new()
        .with_width(64)
        .with_height(64)
        .with_pixel_format(PixelFormat::BGRA)
        .with_minimum_frame_interval(&CMTime::new(1, 2))
        .with_shows_cursor(false)
        .with_captures_audio(true)
        .with_excludes_current_process_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2);
    let stream = SCStream::new(&filter, &configuration).map_err(|e| e.to_string())?;
    let output_configuration = SCRecordingOutputConfiguration::new()
        .map_err(|e| e.to_string())?
        .with_output_url(file)
        .map_err(|e| e.to_string())?
        .with_video_codec(SCRecordingOutputCodec::H264)
        .with_output_file_type(SCRecordingOutputFileType::MOV);
    let began = Arc::new(Mutex::new(None));
    let (done, finished) = mpsc::channel();
    let failed = done.clone();
    let mark = began.clone();
    let callbacks = RecordingCallbacks::new()
        .on_start(move || {
            mark.lock().unwrap().get_or_insert_with(Instant::now);
        })
        .on_fail(move |error| {
            let _ = failed.send(Err(error));
        })
        .on_finish(move || {
            let _ = done.send(Ok(()));
        });
    let output = SCRecordingOutput::new_with_delegate(&output_configuration, callbacks)
        .ok_or("ScreenCaptureKit wouldn't make a recording output for the sound")?;
    stream
        .add_recording_output(&output)
        .map_err(|e| e.to_string())?;
    stream.start_capture().map_err(|e| e.to_string())?;
    let heard = Arc::new(Mutex::new(
        names.into_iter().collect::<std::collections::BTreeSet<_>>(),
    ));
    let left_out = Arc::new(Mutex::new(
        left.into_iter().collect::<std::collections::BTreeSet<_>>(),
    ));
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    // Follow apps as the agent opens and closes them, and as the user
    // opens windows of them elsewhere.
    {
        let stream = stream.clone();
        let (heard, left_out, running) = (heard.clone(), left_out.clone(), running.clone());
        let mut pids: Vec<i32> = apps.iter().map(|a| a.process_id()).collect();
        std::thread::spawn(move || {
            while running.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                let Ok(content) = SCShareableContent::get() else {
                    continue;
                };
                let (apps, names, left) = heard_apps(id, &content);
                let now: Vec<i32> = apps.iter().map(|a| a.process_id()).collect();
                if now == pids {
                    continue;
                }
                let Some(display) = content
                    .displays()
                    .into_iter()
                    .find(|d| d.display_id() == id)
                else {
                    continue;
                };
                match sound_filter(&display, &apps)
                    .and_then(|f| stream.update_content_filter(&f).map_err(|e| e.to_string()))
                {
                    Ok(()) => {
                        log::info!(
                            "recording of display {id} hears {names:?}, leaves out {left:?}"
                        );
                        pids = now;
                        heard.lock().unwrap().extend(names);
                        left_out.lock().unwrap().extend(left);
                    }
                    Err(error) => log::warn!("recording of display {id}: sound filter: {error}"),
                }
            }
        });
    }
    Ok(Sound {
        stream,
        output,
        file: file.to_path_buf(),
        began,
        finished,
        heard,
        left_out,
        running,
    })
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
    let sound_file = folder.join(format!(".{base}.sound.mov"));
    let timeline = folder.join(format!("{base}.timeline.jsonl"));

    let bounds = super::cg::display_bounds(id);
    let mut pixels =
        super::cg::pixel_size(id).unwrap_or((bounds.width as u32, bounds.height as u32));
    // Test builds may record at one pixel per point, to compare the cost.
    if cfg!(debug_assertions) && std::env::var("SIDESCREEN_RECORD_SCALE").as_deref() == Ok("1") {
        pixels = (bounds.width as u32, bounds.height as u32);
    }
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
    let sound = match start_sound(id, &sound_file) {
        Ok(sound) => Some(sound),
        Err(error) => {
            log::warn!("recording of display {id} has no sound: {error}");
            None
        }
    };
    log::info!("recording display {id} to {}", video.display());
    let asked = Instant::now();
    with(|all| {
        all.insert(
            id,
            Recording {
                sound,
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
    let video_began = *recording.began.lock().unwrap();
    let (heard, left_out, sound_problem) = match recording.sound {
        Some(sound) => finish_sound(sound, &recording.video, video_began, problem.is_none()),
        None => (
            Vec::new(),
            Vec::new(),
            Some("the sound stream didn't start".into()),
        ),
    };
    log::info!(
        "recorded display {id}: {:.1} s, {} actions, sound of {heard:?}, {}",
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
        heard,
        left_out,
        sound_problem,
    })
}

/// Stop the sound stream and put its sound into `video`.
fn finish_sound(
    sound: Sound,
    video: &Path,
    video_began: Option<Instant>,
    video_ok: bool,
) -> (Vec<String>, Vec<String>, Option<String>) {
    sound
        .running
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let _ = sound.stream.remove_recording_output(&sound.output);
    let _ = sound.stream.stop_capture();
    let heard: Vec<String> = sound.heard.lock().unwrap().iter().cloned().collect();
    let left_out: Vec<String> = sound.left_out.lock().unwrap().iter().cloned().collect();
    let written = sound.finished.recv_timeout(Duration::from_secs(5));
    let sound_began = *sound.began.lock().unwrap();
    let problem = match (written, video_began, sound_began) {
        _ if !video_ok => Some("the video itself failed".to_string()),
        (Ok(Ok(())), Some(v), Some(a)) => {
            let offset = if a >= v {
                (a - v).as_secs_f64()
            } else {
                -(v - a).as_secs_f64()
            };
            let joined = video.with_extension("joined.mp4");
            match super::mux::merge(video, &sound.file, offset, &joined) {
                Ok(true) => {
                    // AVFoundation writes the file for its owner alone.
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(video)
                        .map(|m| m.permissions().mode())
                        .unwrap_or(0o644);
                    let _ =
                        std::fs::set_permissions(&joined, std::fs::Permissions::from_mode(mode));
                    match std::fs::rename(&joined, video) {
                        Ok(()) => None,
                        Err(e) => Some(e.to_string()),
                    }
                }
                Ok(false) => Some("no app on the display made a sound".into()),
                Err(error) => Some(error),
            }
        }
        (Ok(Err(error)), _, _) => Some(error),
        (Err(_), _, _) => Some("the sound took more than 5 s to finish writing".into()),
        _ => Some("the sound stream never started".into()),
    };
    let _ = std::fs::remove_file(&sound.file);
    if let Some(problem) = &problem {
        log::info!("recording sound: {problem}");
    }
    (heard, left_out, problem)
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
