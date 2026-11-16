//! Per-display MP4 video recorder (`ffmpeg` rawvideo pipe) and version-1
//! `.timeline.jsonl` sidecar writer (`agent_screen_app_core::timeline::TimelineWriter`).

use agent_screen_app_core::ports::Display;
use agent_screen_app_core::timeline::TimelineWriter;
use agent_screen_model::model::DisplayId;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RECORD_FPS: u32 = 12;
const MAX_DURATION: Duration = Duration::from_secs(30 * 60);

pub struct DoneRecording {
    pub codec: &'static str,
    pub video: PathBuf,
    pub timeline: PathBuf,
    pub seconds: f64,
    pub actions: u64,
    pub problem: Option<String>,
}

struct ActiveRecording {
    video: PathBuf,
    codec: &'static str,
    logical_size: (f64, f64),
    started_at: Instant,
    stop_flag: Arc<AtomicBool>,
    pump_done: Arc<tokio::sync::Notify>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Child,
    timeline: TimelineWriter,
}

#[derive(Default)]
pub struct RecordingManager {
    active: Mutex<HashMap<DisplayId, ActiveRecording>>,
}

impl RecordingManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn is_recording(&self, id: DisplayId) -> bool {
        self.active.lock().unwrap().contains_key(&id)
    }

    pub fn start(
        self: &Arc<Self>,
        display: Arc<dyn Display>,
        label: &str,
    ) -> Result<(PathBuf, PathBuf, &'static str), String> {
        let id = display.id();
        if self.is_recording(id) {
            return Err(format!("display {id} is already being recorded"));
        }
        let metrics = display.metrics();
        // Ensure even dimensions for YUV420p H.264 compatibility.
        let width = (metrics.width & !1).max(2);
        let height = (metrics.height & !1).max(2);

        let folder = recordings_dir();
        std::fs::create_dir_all(&folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;

        let stem = unique_recording_stem(&folder, label);
        let video_path = folder.join(format!("{stem}.mp4"));
        let timeline_path = folder.join(format!("{stem}.timeline.jsonl"));
        let codec = "avc1";

        let mut child = spawn_ffmpeg_encoder(width, height, &video_path)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg did not open stdin pipe".to_string())?;

        let logical = (metrics.width as f64, metrics.height as f64);
        let timeline = TimelineWriter::create(&timeline_path, logical, (width, height), codec)?;

        let stop_flag = Arc::new(AtomicBool::new(false));
        let pump_done = Arc::new(tokio::sync::Notify::new());
        let stdin_shared = Arc::new(Mutex::new(Some(stdin)));
        let started_at = Instant::now();

        let rec = ActiveRecording {
            video: video_path.clone(),
            codec,
            logical_size: logical,
            started_at,
            stop_flag: stop_flag.clone(),
            pump_done: pump_done.clone(),
            stdin: stdin_shared.clone(),
            child,
            timeline,
        };
        self.active.lock().unwrap().insert(id, rec);

        // Background frame pump feeding RGBA frames to ffmpeg at RECORD_FPS.
        let disp_pump = display;
        tokio::spawn(async move {
            let interval = Duration::from_millis(1000 / RECORD_FPS as u64);
            let target_bytes = (width as usize) * (height as usize) * 4;
            let mut last_buf = vec![0u8; target_bytes];

            while !stop_flag.load(Ordering::Acquire) {
                let tick_start = Instant::now();
                if let Ok(raw) = disp_pump.capture_raw().await {
                    if raw.width == width && raw.height == height && raw.rgba.len() >= target_bytes
                    {
                        last_buf.copy_from_slice(&raw.rgba[..target_bytes]);
                    } else {
                        crop_or_pad_rgba(
                            &raw.rgba,
                            raw.width,
                            raw.height,
                            &mut last_buf,
                            width,
                            height,
                        );
                    }
                }
                let frame_copy = last_buf.clone();
                let stdin_ref = stdin_shared.clone();
                let write_ok = tokio::task::spawn_blocking(move || {
                    let mut guard = stdin_ref.lock().unwrap();
                    let Some(pipe) = guard.as_mut() else {
                        return false;
                    };
                    pipe.write_all(&frame_copy).is_ok()
                })
                .await
                .unwrap_or(false);

                if !write_ok {
                    break;
                }
                let elapsed = tick_start.elapsed();
                if elapsed < interval {
                    tokio::time::sleep(interval - elapsed).await;
                }
            }
            pump_done.notify_waiters();
        });

        // Auto-stop after 30 minutes so a forgotten recording never fills disk.
        let mgr_timeout = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(MAX_DURATION).await;
            let still_same = mgr_timeout
                .active
                .lock()
                .unwrap()
                .get(&id)
                .is_some_and(|r| r.started_at == started_at);
            if still_same {
                let _ = mgr_timeout.stop(id).await;
            }
        });

        Ok((video_path, timeline_path, codec))
    }

    pub fn note_action(&self, id: DisplayId, event: Value) {
        let mut map = self.active.lock().unwrap();
        let Some(rec) = map.get_mut(&id) else {
            return;
        };
        let secs = rec.started_at.elapsed().as_secs_f64();
        rec.timeline.record_action(secs, event);
    }

    pub fn note_pointer(&self, id: DisplayId, point: (f64, f64), click: bool, active_agent: bool) {
        let mut map = self.active.lock().unwrap();
        let Some(rec) = map.get_mut(&id) else {
            return;
        };
        let secs = rec.started_at.elapsed().as_secs_f64();
        let logical = rec.logical_size;
        rec.timeline
            .record_agent_pointer(secs, point, logical, click, active_agent);
    }

    pub async fn stop(&self, id: DisplayId) -> Option<DoneRecording> {
        let rec = self.active.lock().unwrap().remove(&id)?;
        let seconds = rec.started_at.elapsed().as_secs_f64();
        rec.stop_flag.store(true, Ordering::Release);

        let notified = rec.pump_done.notified();
        let _ = tokio::time::timeout(Duration::from_millis(500), notified).await;

        // Close ffmpeg stdin so it writes the MP4 trailer (`moov` atom) and exits.
        let _ = rec.stdin.lock().unwrap().take();

        let mut child = rec.child;
        let wait_res = tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        return if status.success() {
                            None
                        } else {
                            Some(format!("ffmpeg exited with {status}"))
                        };
                    }
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Some("ffmpeg took longer than 5 s to finalize MP4".into());
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(e) => return Some(format!("ffmpeg wait error: {e}")),
                }
            }
        })
        .await
        .unwrap_or_else(|e| Some(e.to_string()));

        let finished_tl = rec.timeline.finish(seconds, wait_res.is_none());
        Some(DoneRecording {
            codec: rec.codec,
            video: rec.video,
            timeline: finished_tl.path,
            seconds: finished_tl.seconds,
            actions: finished_tl.actions,
            problem: wait_res,
        })
    }

    pub async fn stop_all(&self) {
        let ids: Vec<DisplayId> = self.active.lock().unwrap().keys().copied().collect();
        for id in ids {
            let _ = self.stop(id).await;
        }
    }
}

pub fn recordings_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENTSCREEN_PLAYBACK_DIR")
        .or_else(|_| std::env::var("AGENTSCREEN_RECORDING_DIR"))
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    if let Ok(vids) = std::env::var("XDG_VIDEOS_DIR")
        && !vids.trim().is_empty()
    {
        return PathBuf::from(vids).join("Agent Screen");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join("Videos").join("Agent Screen")
}

fn spawn_ffmpeg_encoder(width: u32, height: u32, out_path: &Path) -> Result<Child, String> {
    let size_arg = format!("{width}x{height}");
    let fps_arg = RECORD_FPS.to_string();
    let has_x264 = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"));

    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        &size_arg,
        "-r",
        &fps_arg,
        "-i",
        "pipe:0",
    ]);
    if has_x264 {
        cmd.args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ]);
    } else {
        cmd.args(["-c:v", "mpeg4", "-pix_fmt", "yuv420p"]);
    }
    cmd.arg(out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to launch ffmpeg for display recording: {e}"))
}

fn crop_or_pad_rgba(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
    dst.fill(0);
    let copy_w = sw.min(dw) as usize * 4;
    let rows = sh.min(dh) as usize;
    for y in 0..rows {
        let s_off = y * (sw as usize * 4);
        let d_off = y * (dw as usize * 4);
        if s_off + copy_w <= src.len() && d_off + copy_w <= dst.len() {
            dst[d_off..d_off + copy_w].copy_from_slice(&src[s_off..s_off + copy_w]);
        }
    }
}

fn unique_recording_stem(folder: &Path, label: &str) -> String {
    let clean_label: String = label
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    let prefix = if clean_label.is_empty() {
        "Agent Display".to_string()
    } else {
        clean_label
    };
    let stamp = format_utc_stamp();
    let base = format!("{prefix} {stamp}");
    if !folder.join(format!("{base}.mp4")).exists() {
        return base;
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !folder.join(format!("{candidate}.mp4")).exists() {
            return candidate;
        }
    }
    format!("{base}-{}", std::process::id())
}

fn format_utc_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
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
