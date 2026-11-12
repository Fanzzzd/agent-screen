//! A display's stream that starts showing another screen after a display
//! change starts again by itself. After a screen lock the user's Peek tile
//! showed another screen until they opened the Viewer: ScreenCaptureKit
//! can mix displays up after a display change, the repair ran only on the
//! change notification, and the wrong frames came after it. Peek and the
//! Viewer draw the same stream.
//!
//! AGENTSCREEN_TEST_MISROUTE makes the debug app take every frame of a
//! stream started before the latest display change for another screen's,
//! as macOS's streams do then; a stream started since passes. The display
//! change is real: `tools/mode.swift` switches the display to 800×600, and
//! Agent Screen switches it back. The stream must show the display again
//! within 5 s, with no Viewer.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e peekmisroute
//!
//! Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, run, sh_lenient, sleep_ms, start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};
use serde_json::Value;

const KEY: &str = "peekmisroute";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_TEST_MISROUTE", "1"),
        ])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn stream(control: &Control, id: u64) -> Result<Value> {
    let streams = control.call("debugStreams", json!({}))?;
    streams["streams"]
        .as_array()
        .and_then(|s| {
            s.iter()
                .find(|s| s["displayId"].as_u64() == Some(id))
                .cloned()
        })
        .context("no stream for the display")
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Peek misroute", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let mut before = stream(&control, id)?;
    for _ in 0..30 {
        if before["delivered"].as_u64().unwrap_or(0) > 0 && before["running"] == true {
            break;
        }
        sleep_ms(100);
        before = stream(&control, id)?;
    }
    check(
        "the new display's stream runs and delivers frames",
        before["running"] == true && before["delivered"].as_u64().unwrap_or(0) > 0,
        before.to_string(),
    );
    let delivered = before["delivered"].as_u64().unwrap_or(0);
    let switched = sh_lenient(tool("mode")?, &[&id.to_string(), "800", "600"]);
    println!("  switched to 800×600: {}", switched.trim());
    // The stream may misroute once per change: the switch and the switch
    // back are two. It shows the display again once a frame gets through
    // and it says it's fine.
    let mut now = stream(&control, id)?;
    let mut fine = false;
    for _ in 0..50 {
        sleep_ms(100);
        now = stream(&control, id)?;
        fine = now["running"] == true
            && now["broken"] == false
            && now["hasFrame"] == true
            && now["delivered"].as_u64().unwrap_or(0) > delivered;
        if fine && now["repairs"].as_u64().unwrap_or(0) > 0 {
            break;
        }
    }
    check(
        "after a display change, a stream that showed another screen starts again and shows the display",
        fine && now["repairs"].as_u64().unwrap_or(0) > 0,
        now.to_string(),
    );
    let log = std::path::PathBuf::from(std::env::var("HOME")?).join(format!(
        "Library/Logs/Agent Screen/displays-{}.log",
        instance().unwrap_or_default()
    ));
    let noted = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .any(|l| l.contains(&format!("misroute display={id} ")));
    check(
        "the display log notes the misroute",
        noted,
        log.display().to_string(),
    );
    control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }))?;
    Ok(())
}
