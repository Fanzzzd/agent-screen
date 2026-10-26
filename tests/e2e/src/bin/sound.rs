//! A recording has the sound of the apps on its display and nothing else:
//! an Electron window on the display plays 440 Hz while a windowless
//! process plays 2000 Hz, and the video has the first and not the second.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e sound
//!
//! Mutes the Mac's output while it runs and puts it back. Needs ffmpeg,
//! and an Electron.app as `cargo e2e electron` does.

use std::process::{Command, ExitCode};

use agent_screen_tests::{
    Control, check, fixture, json, kill_ours, out, run, skip, sleep_ms, start_test_app,
    stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    let muted = osascript("output muted of (get volume settings)") == "true";
    osascript("set volume output muted true");
    let result = run(|| {
        start_test_app(&[])?;
        let control = Control::connect()?;
        let result = test(&control, &electron);
        stop_test_app();
        result
    });
    if !muted {
        osascript("set volume output muted false");
    }
    result
}

fn osascript(script: &str) -> String {
    Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// A 2000 Hz sine, 6 s, as a WAV file.
fn wav(path: &std::path::Path) -> Result<()> {
    let rate = 44_100u32;
    let samples: Vec<i16> = (0..rate * 6)
        .map(|i| {
            ((i as f64 * 2000.0 * std::f64::consts::TAU / rate as f64).sin() * 12_000.0) as i16
        })
        .collect();
    let mut bytes = Vec::new();
    let data = (samples.len() * 2) as u32;
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for s in samples {
        bytes.extend(s.to_le_bytes());
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// The video's sound as mono samples at 48 kHz.
fn samples(video: &std::path::Path) -> Result<Vec<f32>> {
    let output = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(video)
        .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "-"])
        .output()
        .context("ffmpeg can't start")?;
    let bytes = output.stdout;
    if bytes.is_empty() {
        anyhow::bail!(
            "the video has no sound: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}

/// The amplitude of `hz` in `samples`, in dB of full scale (Goertzel).
fn level(samples: &[f32], hz: f64) -> f64 {
    let w = std::f64::consts::TAU * hz / 48_000.0;
    let k = 2.0 * w.cos();
    let (mut a, mut b) = (0.0f64, 0.0f64);
    for &x in samples {
        let c = x as f64 + k * a - b;
        b = a;
        a = c;
    }
    let power = a * a + b * b - k * a * b;
    let amplitude = 2.0 * power.sqrt() / samples.len().max(1) as f64;
    20.0 * amplitude.max(1e-12).log10()
}

fn test(control: &Control, electron: &std::path::Path) -> Result<()> {
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "sound", "title": "Sound", "width": 1000, "height": 700 }),
    )?;
    let id = display["displayId"].clone();
    control.call("recordStart", json!({ "displayId": id }))?;
    control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_TONE": "440" },
        }),
    )?;
    // A process with no window: the recording must not hear it.
    let other = out().join("sound-2000.wav");
    wav(&other)?;
    let mut afplay = Command::new("/usr/bin/afplay").arg(&other).spawn()?;
    sleep_ms(5000);
    let done = control.call("recordStop", json!({ "displayId": id }))?;
    let _ = afplay.kill();
    // Only this checkout's harness: quitting Electron.app by name would end
    // another worktree's run of it too.
    kill_ours(fixture("electron"));
    control.call("endDisplay", json!({ "displayId": id }))?;
    println!("sound: {}", done["sound"]);
    let video = std::path::PathBuf::from(done["video"].as_str().context("no video")?);
    let heard = done["sound"]["apps"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    check(
        "the app on the display is heard",
        heard
            .iter()
            .any(|a| a.as_str().is_some_and(|n| n.contains("Electron"))),
        done["sound"]["apps"].to_string(),
    );
    let sound = samples(&video)?;
    let (wanted, unwanted) = (level(&sound, 440.0), level(&sound, 2000.0));
    let heard = check(
        "its 440 Hz is in the video",
        wanted > -30.0,
        format!("{wanted:.1} dB"),
    );
    let quiet = check(
        "a process off the display, at 2000 Hz, is not",
        unwanted < wanted - 60.0,
        format!("{unwanted:.1} dB"),
    );
    if heard && quiet {
        let _ = std::fs::remove_file(&video);
        let _ = std::fs::remove_file(video.with_extension("timeline.jsonl"));
    } else {
        println!("kept {}", video.display());
    }
    Ok(())
}
