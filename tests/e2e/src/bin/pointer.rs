//! Record the Agent Pointer in the Viewer, out of the user's sight, to judge
//! its look and motion frame by frame: long and short moves, a run of quick
//! clicks, and the rest after them.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e pointer
//!
//! The Viewer opens on a stage display. Writes target/e2e/pointer.mov
//! and, with ffmpeg, its frames at 60 fps to target/e2e/pointer/.

use std::process::{Command, ExitCode, Stdio};

use agent_screen_tests::{
    Control, Rect, json, open_stage, out, run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result, bail};

fn main() -> ExitCode {
    run(|| {
        start_test_app(&[])?;
        let control = Control::connect()?;
        let result = record(&control);
        stop_test_app();
        result
    })
}

fn record(control: &Control) -> Result<()> {
    open_stage(control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "pointer", "client": "Claude Code", "title": "Pointer", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1500);
    let viewer = control.call("debugViewer", json!({}))?;
    let frame: Rect = serde_json::from_value(viewer["frame"].clone())?;

    let movie = out().join("pointer.mov");
    let _ = std::fs::remove_file(&movie);
    let region = frame.map(|v| v.round().to_string()).join(",");
    let mut recorder = Command::new("/usr/sbin/screencapture")
        .args(["-v", "-x", "-V", "9", "-R", &region])
        .arg(&movie)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    sleep_ms(1200);

    let at = |method: &str, x: f64, y: f64| {
        // A click on the empty display finds no window; the pointer shows anyway.
        let _ = control.send(method, json!({ "displayId": id, "x": x, "y": y }));
    };
    // A long diagonal, a short hop, a long horizontal, then up.
    at("move", 200.0, 150.0);
    sleep_ms(700);
    at("move", 1050.0, 620.0);
    sleep_ms(700);
    at("click", 1090.0, 640.0);
    sleep_ms(700);
    at("move", 150.0, 640.0);
    sleep_ms(700);
    at("click", 640.0, 120.0);
    sleep_ms(500);
    // Quick clicks, as an agent filling a form: the pointer must keep up.
    for (x, y) in [
        (300.0, 300.0),
        (520.0, 300.0),
        (740.0, 300.0),
        (960.0, 300.0),
    ] {
        at("click", x, y);
        sleep_ms(160);
    }
    sleep_ms(1800);

    drop(recorder.stdin.take());
    let _ = recorder.wait();
    if !movie.exists() {
        bail!("screencapture recorded nothing");
    }
    let frames = out().join("pointer");
    let _ = std::fs::remove_dir_all(&frames);
    std::fs::create_dir_all(&frames)?;
    let ffmpeg = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(&movie)
        .args(["-vf", "fps=60"])
        .arg(frames.join("%04d.png"))
        .output()
        .context("ffmpeg can't start")?;
    if !ffmpeg.status.success() {
        bail!("{}", String::from_utf8_lossy(&ffmpeg.stderr));
    }
    println!("{}", movie.display());
    Ok(())
}
