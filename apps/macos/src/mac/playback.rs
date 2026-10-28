//! Local file library and AVPlayer bridge. No display or input dependencies.

use agent_screen_model::recording_playback::{
    MAX_BYTES, Support, Timeline, recording_name, seek_time, sidecar_name,
};
use core_foundation::base::TCFType;
use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
use std::ffi::{CString, c_void};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

unsafe extern "C" {
    fn ASPlaybackCreate(path: *const std::ffi::c_char) -> *mut c_void;
    fn ASPlaybackDestroy(player: *mut c_void);
    fn ASPlaybackSetPlaying(player: *mut c_void, playing: bool);
    fn ASPlaybackSeek(player: *mut c_void, seconds: f64);
    fn ASPlaybackSnapshot(
        player: *mut c_void,
        seconds: *mut f64,
        duration: *mut f64,
        status: *mut i32,
        playing: *mut bool,
        seeking: *mut bool,
    ) -> CVPixelBufferRef;
}

pub struct Player(NonNull<c_void>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready,
    Failed,
}

pub struct Snapshot {
    pub seconds: f64,
    pub duration: f64,
    pub status: Status,
    pub playing: bool,
    pub seeking: bool,
    pub frame: Option<CVPixelBuffer>,
}

impl Player {
    pub fn open(path: &Path) -> Result<Self, String> {
        if sidecar_name(
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default(),
        )
        .is_none()
        {
            return Err("only local MP4 files can be played".into());
        }
        let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        NonNull::new(unsafe { ASPlaybackCreate(path.as_ptr()) })
            .map(Self)
            .ok_or("AVPlayer could not open the file".into())
    }

    pub fn set_playing(&self, playing: bool) {
        unsafe { ASPlaybackSetPlaying(self.0.as_ptr(), playing) }
    }

    pub fn seek(&self, seconds: f64, duration: f64) {
        unsafe { ASPlaybackSeek(self.0.as_ptr(), seek_time(seconds, duration)) }
    }

    pub fn snapshot(&self) -> Snapshot {
        let (mut seconds, mut duration, mut status, mut playing, mut seeking) =
            (0.0, 0.0, 0, false, false);
        let raw = unsafe {
            ASPlaybackSnapshot(
                self.0.as_ptr(),
                &mut seconds,
                &mut duration,
                &mut status,
                &mut playing,
                &mut seeking,
            )
        };
        let frame = (!raw.is_null()).then(|| unsafe { CVPixelBuffer::wrap_under_create_rule(raw) });
        Snapshot {
            seconds,
            duration,
            status: match status {
                1 => Status::Ready,
                2 => Status::Failed,
                _ => Status::Loading,
            },
            playing,
            seeking,
            frame,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe { ASPlaybackDestroy(self.0.as_ptr()) }
    }
}

#[derive(Clone)]
pub struct Video {
    pub path: PathBuf,
    pub name: String,
    pub label: String,
    pub started: Option<String>,
}

pub fn library(folder: &Path) -> std::io::Result<Vec<Video>> {
    if !folder.exists() {
        return Ok(Vec::new());
    }
    let mut files = std::fs::read_dir(folder)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            sidecar_name(&name)?;
            let parsed = recording_name(&name);
            Some(Video {
                path: entry.path(),
                name,
                label: parsed.label,
                started: parsed.started,
            })
        })
        .take(2000)
        .collect::<Vec<_>>();
    sort_newest_first(&mut files);
    Ok(files)
}

/// Newest start time first; files without one go last, by name.
fn sort_newest_first(files: &mut [Video]) {
    files.sort_by(|a, b| {
        b.started
            .is_some()
            .cmp(&a.started.is_some())
            .then_with(|| b.started.cmp(&a.started))
            .then_with(|| a.name.cmp(&b.name))
    });
}

pub fn timeline(video: &Path) -> Timeline {
    let Some(name) = video
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(sidecar_name)
    else {
        return Timeline::unavailable(Support::Missing);
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(video.with_file_name(name))
    {
        Ok(file) => file,
        Err(error) => {
            return Timeline::unavailable(if error.kind() == std::io::ErrorKind::NotFound {
                Support::Missing
            } else {
                Support::Invalid
            });
        }
    };
    // A sidecar is data, not a pipe/device or a link to another resource.
    // Inspect the opened descriptor so a path swap cannot bypass this gate.
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Timeline::unavailable(Support::Invalid);
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Timeline::unavailable(Support::Invalid);
    }
    if bytes.len() > MAX_BYTES {
        return Timeline::unavailable(Support::TooLarge);
    }
    match std::str::from_utf8(&bytes) {
        Ok(source) => Timeline::parse(Some(source)),
        Err(_) => Timeline::unavailable(Support::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damaged_sidecar_does_not_hide_its_video_and_discovery_stays_local() {
        let folder = std::env::temp_dir().join(format!(
            "agent-screen-playback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        let video = folder.join("Fixture.mp4");
        std::fs::write(&video, b"synthetic placeholder, never opened by AVPlayer").unwrap();
        std::fs::write(folder.join("Fixture.timeline.jsonl"), b"invalid sidecar").unwrap();
        std::fs::write(folder.join(".sound.mov"), b"").unwrap();
        std::fs::create_dir(folder.join("directory.mp4")).unwrap();
        let files = library(&folder).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, video);
        assert_eq!(timeline(&video).support, Support::Invalid);
        std::fs::remove_file(folder.join("Fixture.timeline.jsonl")).unwrap();
        assert_eq!(timeline(&video).support, Support::Missing);
        std::fs::write(folder.join("Fixture.timeline.jsonl"), [0xff, 0xfe]).unwrap();
        assert_eq!(timeline(&video).support, Support::Invalid);
        std::fs::write(
            folder.join("Fixture.timeline.jsonl"),
            vec![b' '; MAX_BYTES + 1],
        )
        .unwrap();
        assert_eq!(timeline(&video).support, Support::TooLarge);
        assert_eq!(library(&folder).unwrap().len(), 1);
        std::os::unix::fs::symlink(&video, folder.join("Alias.mp4")).unwrap();
        assert_eq!(library(&folder).unwrap().len(), 1);
        std::fs::write(folder.join("Uppercase.MP4"), b"synthetic").unwrap();
        assert_eq!(library(&folder).unwrap().len(), 2);
        let companion = folder.join("Fixture.timeline.jsonl");
        std::fs::remove_file(&companion).unwrap();
        let linked_data = folder.join("linked-data.jsonl");
        let size = agent_screen_model::recording_playback::Size {
            width: 1280.0,
            height: 800.0,
        };
        let valid = format!(
            "{}\n{}",
            agent_screen_model::recording_playback::metadata(size, size, 1.0, "hvc1"),
            agent_screen_model::recording_playback::timeline_end(0.0, true)
        );
        assert_eq!(Timeline::parse(Some(&valid)).support, Support::Ready);
        std::fs::write(&linked_data, valid).unwrap();
        std::os::unix::fs::symlink(&linked_data, &companion).unwrap();
        assert_eq!(timeline(&video).support, Support::Invalid);
        std::fs::remove_file(&companion).unwrap();
        let fifo = CString::new(companion.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert_eq!(timeline(&video).support, Support::Invalid);
        std::fs::remove_file(&companion).unwrap();
        std::fs::create_dir(&companion).unwrap();
        assert_eq!(timeline(&video).support, Support::Invalid);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn library_lists_newest_recording_first_regardless_of_label() {
        let folder = std::env::temp_dir().join(format!(
            "agent-screen-playback-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        for name in [
            "Zebra 2026-10-01 09-00-00.mp4",
            "Alpha 2026-10-27 08-03-09.mp4",
            "Middle 2026-10-15 12-00-00-3-17930.mp4",
            "untitled.mp4",
        ] {
            std::fs::write(folder.join(name), b"synthetic").unwrap();
        }
        let files = library(&folder).unwrap();
        let labels: Vec<&str> = files.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, ["Alpha", "Middle", "Zebra", "untitled"]);
        assert_eq!(files[0].started.as_deref(), Some("2026-10-27 08:03:09"));
        assert_eq!(files[3].started, None);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
