//! Closing a display whose app asks whether to save. The question stays on
//! the display, in the Viewer, instead of landing on the user's screen: the
//! display waits while the sheet is up and goes once the window has closed.
//! After Cancel, the window stays and so does the display.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e asksave
//!
//! Uses tools/asksave.swift as an app; quits it after.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, out, pid_of, probe, run, sh, sh_lenient, sleep_ms,
    start_test_app, stop_test_app, tool, try_osa,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let app = ask_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&app, "Don't Save").and_then(|()| test(&app, "Cancel"));
        let _ = sh_lenient(
            "/usr/bin/pkill",
            &["-f", "AskSave.app/Contents/MacOS/AskSave"],
        );
        stop_test_app();
        result
    })
}

/// tools/asksave.swift as an app, so launch_app takes it.
fn ask_app() -> Result<PathBuf> {
    let app = out().join("AskSave.app");
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    std::fs::copy(tool("asksave")?, macos.join("AskSave"))?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.agentscreen.e2e.asksave</string>
<key>CFBundleExecutable</key><string>AskSave</string>
<key>CFBundleName</key><string>AskSave</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#,
    )?;
    sh(
        "/usr/bin/codesign",
        &["--force", "--sign", "-", app.to_str().unwrap()],
    )?;
    Ok(app)
}

fn log_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Logs/Agent Screen/displays-e2e.log")
}

/// Whether the display log says display `id` went, as a spare or for good,
/// in its lines past `from`.
fn display_gone(id: u64, from: usize) -> bool {
    let log = std::fs::read(log_path()).unwrap_or_default();
    let tail = String::from_utf8_lossy(&log[from.min(log.len())..]).to_string();
    tail.contains(&format!("kept display={id} "))
        || tail.contains(&format!("released display={id} "))
}

fn test(app: &Path, answer: &str) -> Result<()> {
    println!("-- answering {answer}");
    let control = Control::connect()?;
    let key = format!("asksave-{}", answer.len());
    let said = out().join("asksave.txt");
    let _ = std::fs::remove_file(&said);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "AskSave", "width": 1200, "height": 760 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": app.to_str().unwrap(),
                "environment": { "ASK_OUT": said.to_str().unwrap() } }),
    )?;
    sleep_ms(1000);
    let pid = pid_of("AskSave.app/Contents/MacOS/AskSave").context("AskSave running")?;
    let from = std::fs::metadata(log_path())
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    // The user closes the display, keeping the plan: the agent's window closes.
    control.call(
        "debugShow",
        json!({ "surface": "dismiss-confirm", "displayId": id }),
    )?;
    sleep_ms(3000);
    let asked = std::fs::read_to_string(&said).unwrap_or_default();
    let area: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let windows = probe(&["windows", &pid.to_string()])?;
    let on_display = windows.lines().any(|line| {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
        let (x, y) = (
            v["x"].as_f64().unwrap_or(-1e9),
            v["y"].as_f64().unwrap_or(-1e9),
        );
        area.len() == 4
            && x >= area[0] - 1.0
            && x < area[0] + area[2]
            && y >= area[1] - 1.0
            && y < area[1] + area[3]
    });
    check(
        "the app asks whether to save, on the agent's display",
        asked.contains("asked") && on_display && !display_gone(id, from),
        format!("app said {asked:?}; windows {windows:?}; display at {area:?}"),
    );
    // The user answers in the Viewer.
    let clicked = try_osa(&format!(
        "tell application \"System Events\" to tell process \"AskSave\" to click button \"{answer}\" of sheet 1 of window 1"
    ));
    check(
        "the sheet's button is pressed",
        clicked.is_ok(),
        format!("{clicked:?}"),
    );
    sleep_ms(4000);
    let gone = display_gone(id, from);
    let after = std::fs::read_to_string(&said).unwrap_or_default();
    if answer == "Cancel" {
        check(
            "after Cancel, the window and the display stay",
            after.contains("cancelled")
                && !gone
                && pid_of("AskSave.app/Contents/MacOS/AskSave").is_some(),
            format!("app said {after:?}, display gone {gone}"),
        );
        let _ = sh_lenient(
            "/usr/bin/pkill",
            &["-f", "AskSave.app/Contents/MacOS/AskSave"],
        );
        sleep_ms(1000);
        let _ = control.call(
            "debugShow",
            json!({ "surface": "dismiss-confirm", "displayId": id }),
        );
        sleep_ms(1500);
    } else {
        check(
            "once the window has closed, the display goes",
            after.contains("discarded") && gone,
            format!("app said {after:?}, display gone {gone}"),
        );
    }
    Ok(())
}
