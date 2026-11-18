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
    Control, Rect, check, debug_pid, dropdown_app, hid, instance, json, kill_ours, let_continue,
    on_screen, open_user_stage, our_binary, out, probe, run, sign_step, sleep_ms, start_test_app,
    stop_test_app, window_titled, windows_of,
};
use anyhow::{Context, Result};

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

fn title(control: &Control, id: u64) -> String {
    window_titled(control, KEY, id, "dropdown")
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
    open_user_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Viewer pop-up", "width": 1280, "height": 600 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let size = (
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
    let w = window_titled(&control, KEY, id, "dropdown")?;
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
    let image: Rect = serde_json::from_value(viewer["image"].clone()).context("image")?;
    // item2 covers row2 of the main window: the list starts one row down.
    let at = on_screen(
        image,
        size,
        wx + INSET + (ww - 2.0 * INSET) / 2.0,
        top + 2.5 * ROW,
    );

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
    let_continue(&control, id)?;
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

    // A press closes an open menu, here the Viewer's Give menu on the
    // stage, which stands in for the user's screen, and the menu takes it:
    // it never reaches the display. The list is closed, so a press that
    // went through would pick row2 under it. The menu opens at the
    // pointer unless told where; here the picture's lower left. AppKit
    // runs the menu's loop meanwhile and the control socket answers only
    // once it closes, so the menu is asked for from another thread.
    let pid = debug_pid().context("the test app's pid")?;
    let menu = || windows_of(pid).into_iter().find(|w| w.layer == 101);
    let rest = (image[0] + 40.0, image[1] + image[3] - 40.0);
    let asked = std::thread::spawn(move || -> Result<()> {
        Control::connect()?.call(
            "debugShow",
            json!({ "surface": "give", "displayId": id, "x": rest.0, "y": rest.1 - 160.0 }),
        )?;
        Ok(())
    });
    let mut open = None;
    for _ in 0..30 {
        open = menu();
        if open.is_some() {
            break;
        }
        sleep_ms(100);
    }
    // The menu must be on the stage, beside the Viewer, not on the user's
    // screen.
    let on_stage = open.as_ref().is_some_and(|m| {
        let frame = &viewer["frame"];
        let v = |i: usize| frame[i].as_f64().unwrap_or(0.0);
        m.x < v(0) + v(2) && m.x + m.width > v(0) && m.y < v(1) + v(3) && m.y + m.height > v(1)
    });
    let covers = open.as_ref().is_some_and(|m| {
        at.0 >= m.x && at.0 <= m.x + m.width && at.1 >= m.y && at.1 <= m.y + m.height
    });
    if check(
        "the Viewer's Give menu opens on the stage, beside the list item",
        open.is_some() && !covers && on_stage,
        format!("{open:?}"),
    ) {
        hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
    }
    let mut left = menu();
    for _ in 0..20 {
        if left.is_none() {
            break;
        }
        sleep_ms(100);
        left = menu();
    }
    if let Some(m) = &left {
        // Never leave a menu open on the screen.
        let _ = probe(&[
            "cancelmenu",
            &(m.x + 10.0).to_string(),
            &(m.y + 10.0).to_string(),
        ]);
    }
    let _ = asked.join();
    sleep_ms(300);
    let now = title(&control, id);
    check(
        "a click in the Viewer while a menu is open closes the menu and reaches nothing under it",
        left.is_none() && now.contains("got=item2"),
        format!("menu left open: {left:?}; {now}"),
    );
    println!(
        "the fixture's presses:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }))?;
    Ok(())
}
