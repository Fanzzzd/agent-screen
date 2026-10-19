//! The user drags a window by its title bar in the Viewer: the window moves
//! on the Agent Display, as far as the drag went, and stays there. Posted
//! events can't do that: WindowServer moves a window being dragged to the
//! real pointer, on the user's screen. The Viewer opens on a stage display
//! and the drag is made through its own handler, so the user's pointer is
//! untouched and this runs at any time.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e viewerdrag

use std::process::ExitCode;

use anyhow::{Context, Result};
use agent_screen_tests::{
    Control, Window, check, instance, json, open_stage, quit_app, run, running, skip, sleep_ms,
    start_test_app, stop_test_app,
};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running, and may be the user's");
    }
    run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let result = test();
        quit_app("Calculator");
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let calculator = |display_id: u64| -> Result<Option<Window>> {
        let listed = control.call("listWindows", json!({ "displayId": display_id }))?;
        let windows: Vec<Window> = serde_json::from_value(listed["windows"].clone())?;
        Ok(windows.into_iter().find(|w| w.app == "Calculator"))
    };

    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({
            "sessionKey": "viewerdrag",
            "client": "Claude Code",
            "title": "Viewer drag",
            "width": 900,
            "height": 600,
        }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let (w, h) = (
        display["width"].as_f64().unwrap_or(f64::NAN),
        display["height"].as_f64().unwrap_or(f64::NAN),
    );
    control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    let image = control.call("debugViewer", json!({}))?["image"].clone();
    let before = calculator(id)?;
    check("Calculator opened on the display", before.is_some(), "");
    let Some(before) = before else { return Ok(()) };

    // Left of the toolbar's buttons: SwiftUI's background there, not the
    // window itself, is what Accessibility finds.
    let from = (
        (before.x + before.width * 0.17) / w,
        (before.y + before.height * 0.03) / h,
    );
    let by = (0.15, 0.1);
    control.call(
        "debugViewerInput",
        json!({
            "kind": "drag",
            "x": from.0,
            "y": from.1,
            "toX": from.0 + by.0,
            "toY": from.1 + by.1,
        }),
    )?;
    sleep_ms(1500);
    let after = calculator(id)?;
    check(
        "the window stays on the Agent Display",
        after.is_some(),
        json!(after).to_string(),
    );
    if let Some(after) = after {
        let moved = ((after.x - before.x) / w, (after.y - before.y) / h);
        check(
            "it moved as far as the drag went",
            (moved.0 - by.0).abs() < 0.02 && (moved.1 - by.1).abs() < 0.02,
            format!("{} -> {}, image {image}", json!(before), json!(after)),
        );
    }
    Ok(())
}
