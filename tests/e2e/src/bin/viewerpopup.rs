//! A real click in the Viewer on an item of a pop-up list picks that item.
//! The user clicked a contact in WeCom's search results through the
//! Viewer and got the chat row under the list: WeCom's Qt closes its
//! list on any press in another app, and the press in the Viewer was one,
//! so the list was gone by the time Agent Screen hit-tested the click.
//! tools/dropdown.swift closes its list the same way. A
//! `debugViewerInput` click reaches no other app, so this one is real:
//! the pointer goes to the Viewer on the stage display and comes back.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerpopup
//!
//! Makes 2 displays: the stage and the list's. A sign on your screen
//! counts down from 3 and says when you have the machine back.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, dropdown_app, hid, instance, json, kill_ours, open_stage, our_binary, out, run,
    sign_step, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use serde_json::Value;

const KEY: &str = "viewerpopup";
/// The fixture's row height and list inset, in points.
const ROW: f64 = 50.0;
const INSET: f64 = 40.0;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        // AGENTSCREEN_NO_PRIVATE_PRESS=1 leaves the tap out, to see this fail.
        let off = std::env::var("AGENTSCREEN_NO_PRIVATE_PRESS").unwrap_or_default();
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_NO_PRIVATE_PRESS", &off),
        ])?;
        let result = test();
        kill_ours(our_binary("Dropdown"));
        stop_test_app();
        result
    })
}

fn window(control: &Control, id: u64) -> Result<Value> {
    let listed = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
    listed["windows"]
        .as_array()
        .and_then(|w| {
            w.iter()
                .find(|w| {
                    w["title"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("dropdown"))
                })
                .cloned()
        })
        .context("no dropdown window on the display")
}

fn title(control: &Control, id: u64) -> String {
    window(control, id)
        .ok()
        .and_then(|w| w["title"].as_str().map(String::from))
        .unwrap_or_default()
}

fn wait_title(control: &Control, id: u64, part: &str) -> String {
    let mut now = title(control, id);
    for _ in 0..20 {
        if now.contains(part) {
            break;
        }
        sleep_ms(100);
        now = title(control, id);
    }
    now
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Viewer pop-up", "width": 1280, "height": 600 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let (dw, dh) = (
        display["width"].as_f64().unwrap_or(1280.0),
        display["height"].as_f64().unwrap_or(600.0),
    );
    let app = dropdown_app()?;
    // Every press the fixture takes and why its list closed, for a failure.
    let log = out().join("dropdown.log");
    let _ = std::fs::remove_file(&log);
    control.call(
        "launchApp",
        json!({
            "sessionKey": KEY,
            "displayId": id,
            "app": app.to_str(),
            "environment": { "DROPDOWN_LOG": log.to_str() },
        }),
    )?;
    sleep_ms(800);
    let w = window(&control, id)?;
    let (wx, wy, ww, wh) = (
        w["x"].as_f64().context("x")?,
        w["y"].as_f64().context("y")?,
        w["width"].as_f64().context("width")?,
        w["height"].as_f64().context("height")?,
    );
    // Six rows of content under the title bar.
    let top = wy + wh - 6.0 * ROW;
    let open = |control: &Control| -> Result<String> {
        control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": wx + 100.0, "y": top + ROW / 2.0 }),
        )?;
        Ok(wait_title(control, id, "open=1"))
    };
    let opened = open(&control)?;
    check(
        "the agent's click opens the list",
        opened.contains("open=1"),
        &opened,
    );

    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    // Opening the Viewer gets the keys ready in the list's app. A click to
    // wake the list, where a title bar would be, picked its first item.
    let shown = title(&control, id);
    check(
        "opening the Viewer leaves the list open, with nothing picked",
        shown.contains("open=1 got=none"),
        &shown,
    );
    let viewer = control.call("debugViewer", json!({}))?;
    check(
        "the tap that keeps Viewer presses from other apps is on",
        viewer["privatePress"] == true,
        viewer["privatePress"].to_string(),
    );
    let image: Vec<f64> = viewer["image"]
        .as_array()
        .context("image")?
        .iter()
        .filter_map(|v| v.as_f64())
        .collect();
    // item2 covers row2 of the main window: the list starts one row down.
    let (px, py) = (wx + INSET + (ww - 2.0 * INSET) / 2.0, top + 2.5 * ROW);
    let at = (image[0] + px / dw * image[2], image[1] + py / dh * image[3]);

    sign_step("one click on item2 of the list / 点列表的 item2");
    hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
    let got = wait_title(&control, id, "got=item");
    check(
        "a real click in the Viewer on a list item picks the item, not the row under the list",
        got.contains("got=item2"),
        &got,
    );

    // Once more, after the agent opens the list again: the first click
    // must not have left anything that makes the second land elsewhere.
    // The click took the display over; Let Continue hands it back.
    control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    sleep_ms(300);
    let opened = open(&control)?;
    if opened.contains("open=1") {
        hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
        sleep_ms(300);
        let again = wait_title(&control, id, "open=0");
        check(
            "a second real click on the reopened list picks the item too",
            again.contains("open=0 got=item2"),
            &again,
        );
    }
    println!(
        "the fixture's presses:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }))?;
    Ok(())
}
