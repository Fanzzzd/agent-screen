//! A drag in a web page holds the button down (#37): the Electron harness's
//! splitter, which captures the pointer and follows pointermove only while
//! `buttons` has the left button, as react-resizable-panels does, follows
//! a whole drag and a drag sent as press, moves and release.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e webdrag
//!
//! Makes 1 display. Nothing shows on your screen.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, check, fixture, instance, json, out, run, sh_lenient, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&electron);
        quit_harness();
        stop_test_app();
        result
    })
}

/// This checkout's harness only: another worktree may run its own.
fn quit_harness() {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &["-f", fixture("electron").to_str().unwrap()],
    );
    sleep_ms(1000);
}

/// The harness window, title included.
fn harness(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["kind"] == "window")
            .cloned()
    }))
}

/// A number the page put in its title, as in "held=12"; 0 when absent.
fn seen(window: &Value, key: &str) -> i64 {
    window["title"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Start the harness; the middle of its splitter, in screenshot pixels.
fn start(control: &Control, id: &Value, electron: &std::path::Path) -> Result<(i64, i64)> {
    control.call(
        "launchApp",
        json!({ "displayId": id, "app": electron.to_str(),
                "arguments": [fixture("electron").to_str()] }),
    )?;
    let mut window = None;
    for _ in 0..50 {
        window =
            harness(control, id)?.filter(|w| w["title"].as_str().is_some_and(|t| t.contains("w=")));
        if window.is_some() {
            break;
        }
        sleep_ms(200);
    }
    let window = window.context("the harness window didn't show")?;
    let (x, y) = (
        window["x"].as_i64().context("x")?,
        window["y"].as_i64().context("y")?,
    );
    // The splitter: 340 pt from the page's left, 200 to 360 pt down, below
    // a 28 pt title bar. The display is within the image budget, so
    // screenshot pixels are points.
    Ok((x + 346, y + 28 + 280))
}

fn test(electron: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let key = "webdrag";
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Web drag", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].clone();
    let result = (|| -> Result<()> {
        // left_click_drag.
        let (sx, sy) = start(&control, &id, electron)?;
        control.call(
            "drag",
            json!({ "sessionKey": key, "displayId": id,
                    "fromX": sx, "fromY": sy, "toX": sx + 80, "toY": sy }),
        )?;
        sleep_ms(500);
        let now = harness(&control, &id)?.context("window")?;
        check(
            "a drag's pointermoves have the left button held, and the splitter follows it",
            seen(&now, "free") == 0 && seen(&now, "held") > 0 && seen(&now, "split") >= 70,
            now["title"].as_str().unwrap_or_default(),
        );
        quit_harness();

        // left_mouse_down, two mouse_move, left_mouse_up.
        let (sx, sy) = start(&control, &id, electron)?;
        let at = |x: i64| json!({ "sessionKey": key, "displayId": id, "x": x, "y": sy });
        control.call("mouseDown", at(sx))?;
        control.call("move", at(sx + 30))?;
        control.call("move", at(sx + 60))?;
        control.call("mouseUp", at(sx + 60))?;
        sleep_ms(500);
        let now = harness(&control, &id)?.context("window")?;
        check(
            "moves between mouse_down and mouse_up have the left button held, and the splitter follows",
            seen(&now, "free") == 0 && seen(&now, "held") > 0 && seen(&now, "split") >= 50,
            now["title"].as_str().unwrap_or_default(),
        );
        Ok(())
    })();
    let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
    result
}
