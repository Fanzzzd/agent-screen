//! Record the clips of the promo video (promo/), out of the user's sight:
//! an agent works Calculator on its own display, seen in the Viewer, with
//! quick clicks, then asks for help.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e promo
//!
//! The display wears promo/public/wallpaper.png, through the wallpaper
//! setting, which it puts back after. Skips when Calculator is running, and
//! quits it after. Writes promo/public/agent.mp4. Needs ffmpeg.

use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use regex::Regex;
use sidescreen_tests::{
    Control, Rect, json, open_stage, out, quit_app, root, run, running, skip, sleep_ms,
    start_test_app, stop_test_app,
};

fn main() -> ExitCode {
    if running("Calculator") {
        skip("Calculator is running; quit it to record");
    }
    let saved = wear_promo_wallpaper();
    let code = run(|| {
        start_test_app(&[])?;
        let control = Control::connect()?;
        let result = record(&control);
        quit_app("Calculator");
        stop_test_app();
        result
    });
    put_back_wallpaper(saved);
    code
}

fn support() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/Sidescreen")
}

/// The user's chosen wallpaper, if any, moved aside, and the promo's put in
/// its place, as the app's wallpaper setting keeps it.
fn wear_promo_wallpaper() -> Option<(std::path::PathBuf, Vec<u8>)> {
    let saved = std::fs::read_dir(support()).ok().and_then(|entries| {
        entries.flatten().map(|e| e.path()).find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("wallpaper-"))
        })
    });
    let saved = saved.and_then(|path| {
        let bytes = std::fs::read(&path).ok()?;
        std::fs::remove_file(&path).ok()?;
        Some((path, bytes))
    });
    let promo = root().parent().unwrap().join("promo/public/wallpaper.png");
    let _ = std::fs::copy(promo, support().join("wallpaper-promo.png"));
    saved
}

fn put_back_wallpaper(saved: Option<(std::path::PathBuf, Vec<u8>)>) {
    let _ = std::fs::remove_file(support().join("wallpaper-promo.png"));
    if let Some((path, bytes)) = saved {
        let _ = std::fs::write(path, bytes);
    }
}

/// The center of the button labeled `label` in a window state's tree.
fn center(tree: &str, label: &str) -> Result<(f64, f64)> {
    let pattern = format!(
        r#"button "{}"[^\n]* at \((\d+),(\d+)\)"#,
        regex::escape(label)
    );
    let found = Regex::new(&pattern)?
        .captures(tree)
        .with_context(|| format!("no button {label}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

fn record(control: &Control) -> Result<()> {
    open_stage(control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "promo", "client": "Claude Code", "title": "Check the invoice total", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let launched = control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .context("no Calculator window")?;
    // Calculator sits in the middle of the display.
    let _ = control.send(
        "placeWindow",
        json!({ "displayId": id, "windowId": window, "x": 380, "y": 140, "width": 340, "height": 420 }),
    )?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1500);
    let state = control.call(
        "windowState",
        json!({ "displayId": id, "windowId": window }),
    )?;
    let tree = state["tree"].as_str().context("no tree")?.to_string();
    let viewer = control.call("debugViewer", json!({}))?;
    let frame: Rect = serde_json::from_value(viewer["frame"].clone())?;

    let movie = out().join("promo-agent.mov");
    let _ = std::fs::remove_file(&movie);
    let region = frame.map(|v| v.round().to_string()).join(",");
    let mut recorder = Command::new("/usr/sbin/screencapture")
        .args(["-v", "-x", "-V", "20", "-R", &region])
        .arg(&movie)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    sleep_ms(1500);

    let click = |label: &str, wait: u64| -> Result<()> {
        let (x, y) = center(&tree, label)?;
        control.call("click", json!({ "displayId": id, "x": x, "y": y }))?;
        sleep_ms(wait);
        Ok(())
    };
    click("All Clear", 500)?;
    for label in ["1", "2", "8", "0", "Multiply", "1", "2", "Equals"] {
        click(label, 260)?;
    }
    sleep_ms(900);
    // Then faster, as an agent sure of its steps.
    for label in ["Subtract", "9", "6", "0", "Equals"] {
        click(label, 150)?;
    }
    sleep_ms(1200);
    control.call(
        "requestHelp",
        json!({ "displayId": id, "reason": "The total is 14,400. Can you confirm it before I send the invoice?" }),
    )?;
    sleep_ms(3500);
    control.call("withdrawHelp", json!({ "displayId": id }))?;
    sleep_ms(1000);

    drop(recorder.stdin.take());
    let _ = recorder.wait();
    if !movie.exists() {
        bail!("screencapture recorded nothing");
    }
    let public = root().parent().unwrap().join("promo/public");
    std::fs::create_dir_all(&public)?;
    let clip = public.join("agent.mp4");
    let ffmpeg = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(&movie)
        .args([
            "-vf",
            "fps=30,scale=1920:-2:flags=lanczos",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "18",
        ])
        .arg(&clip)
        .output()
        .context("ffmpeg can't start")?;
    if !ffmpeg.status.success() {
        bail!("{}", String::from_utf8_lossy(&ffmpeg.stderr));
    }
    println!("{}", clip.display());
    Ok(())
}
