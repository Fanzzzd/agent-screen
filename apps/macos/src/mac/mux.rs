//! Putting a recording's sound into its video: AVFoundation copies the
//! video track and the audio track, shifted by when each began, into one
//! MP4, without encoding either again.

use block2::RcBlock;
use objc2::encode::{Encode, Encoding};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSString, NSURL};
use std::path::Path;
use std::time::Duration;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTimeRange {
    start: CMTime,
    duration: CMTime,
}

unsafe impl Encode for CMTimeRange {
    const ENCODING: Encoding = Encoding::Struct("?", &[CMTime::ENCODING, CMTime::ENCODING]);
}

const VALID: u32 = 1;

fn seconds(s: f64) -> CMTime {
    CMTime {
        value: (s * 600.0).round() as i64,
        timescale: 600,
        flags: VALID,
        epoch: 0,
    }
}

fn as_seconds(t: CMTime) -> f64 {
    if t.timescale == 0 {
        0.0
    } else {
        t.value as f64 / t.timescale as f64
    }
}

fn class(name: &str) -> Result<&'static AnyClass, String> {
    AnyClass::get(&std::ffi::CString::new(name).unwrap()).ok_or(format!("no class {name}"))
}

fn asset(path: &Path) -> Result<Retained<AnyObject>, String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let none: Option<&AnyObject> = None;
    let made: Option<Retained<AnyObject>> =
        unsafe { msg_send![class("AVURLAsset")?, URLAssetWithURL: &*url, options: none] };
    made.ok_or(format!("can't open {}", path.display()))
}

fn first_track(asset: &AnyObject, kind: &str) -> Option<Retained<AnyObject>> {
    // The synchronous form: these are local files, already written.
    let tracks: Retained<NSArray<AnyObject>> =
        unsafe { msg_send![asset, tracksWithMediaType: &*NSString::from_str(kind)] };
    tracks.firstObject()
}

/// Write `video` with `audio`'s sound into `out`. `offset` is how many
/// seconds after the video the audio began; the audio is cut to the
/// video's length. False when the audio file holds no sound.
pub fn merge(video: &Path, audio: &Path, offset: f64, out: &Path) -> Result<bool, String> {
    let video_asset = asset(video)?;
    let audio_asset = asset(audio)?;
    let video_track = first_track(&video_asset, "vide").ok_or("the video has no picture")?;
    let Some(audio_track) = first_track(&audio_asset, "soun") else {
        return Ok(false);
    };
    let video_length: CMTime = unsafe { msg_send![&video_asset, duration] };
    let audio_length: CMTime = unsafe { msg_send![&audio_asset, duration] };
    let length = as_seconds(video_length);

    let composition: Retained<AnyObject> =
        unsafe { msg_send![class("AVMutableComposition")?, composition] };
    let add = |kind: &str| -> Option<Retained<AnyObject>> {
        unsafe {
            msg_send![&composition, addMutableTrackWithMediaType: &*NSString::from_str(kind), preferredTrackID: 0i32]
        }
    };
    let picture = add("vide").ok_or("can't add a video track")?;
    let sound = add("soun").ok_or("can't add an audio track")?;
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let whole = CMTimeRange {
        start: seconds(0.0),
        duration: video_length,
    };
    let ok: bool = unsafe {
        msg_send![&picture, insertTimeRange: whole, ofTrack: &*video_track, atTime: seconds(0.0), error: &mut error]
    };
    if !ok {
        return Err("can't copy the video track".into());
    }
    // Audio that began before the video loses its head; audio that began
    // after it starts that much in.
    let skip = (-offset).max(0.0);
    let at = offset.max(0.0);
    let take = (as_seconds(audio_length) - skip).min(length - at);
    if take > 0.05 {
        let range = CMTimeRange {
            start: seconds(skip),
            duration: seconds(take),
        };
        let ok: bool = unsafe {
            msg_send![&sound, insertTimeRange: range, ofTrack: &*audio_track, atTime: seconds(at), error: &mut error]
        };
        if !ok {
            return Err("can't copy the audio track".into());
        }
    }

    let _ = std::fs::remove_file(out);
    let session: Option<Retained<AnyObject>> = unsafe {
        msg_send![msg_send![class("AVAssetExportSession")?, alloc], initWithAsset: &*composition, presetName: &*NSString::from_str("AVAssetExportPresetPassthrough")]
    };
    let session = session.ok_or("can't make an export session")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&out.to_string_lossy()));
    unsafe {
        let _: () = msg_send![&session, setOutputURL: &*url];
        let _: () = msg_send![&session, setOutputFileType: &*NSString::from_str("public.mpeg-4")];
    }
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let block = RcBlock::new(move || {
        let _ = done.send(());
    });
    unsafe {
        let _: () = msg_send![&session, exportAsynchronouslyWithCompletionHandler: &*block];
    }
    if wait.recv_timeout(Duration::from_secs(60)).is_err() {
        unsafe {
            let _: () = msg_send![&session, cancelExport];
        }
        return Err("joining the sound took over 60 s".into());
    }
    // AVAssetExportSessionStatusCompleted.
    let status: isize = unsafe { msg_send![&session, status] };
    if status != 3 {
        let error: Option<Retained<AnyObject>> = unsafe { msg_send![&session, error] };
        let text = error
            .map(|e| {
                let d: Retained<NSString> = unsafe { msg_send![&e, localizedDescription] };
                d.to_string()
            })
            .unwrap_or_else(|| format!("status {status}"));
        return Err(format!("joining the sound failed: {text}"));
    }
    Ok(true)
}
