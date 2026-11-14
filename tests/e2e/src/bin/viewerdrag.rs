//! The user drags a window by its title bar in the Viewer: the window moves
//! on the Agent Display, as far as the drag went, and stays there for a
//! native window, a custom-title-bar window that moves asynchronously after
//! mouseUp, a window taller than the display dragged past the Viewer image
//! edge, and a window with an attached modal sheet. Also checks Takeover
//! Viewer clicks through an empty host window into an `.appex` pop-up menu and
//! click-outside menu dismissal (#205, #207). The Viewer opens on a stage
//! display and input goes through its own handler, so the user's pointer and
//! screen are untouched.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerdrag

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Window, check, instance, json, kill_ours, menu_choice_app, open_stage, our_binary,
    out, quit_app, run, running, sh, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running, and may be the user's");
    }
    run(|| {
        let fixture = menu_choice_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&fixture);
        let _ = std::process::Command::new("/usr/bin/pkill")
            .args(["-f", "MenuPane.appex/Contents/MacOS/MenuPane"])
            .status();
        kill_ours(our_binary("MenuChoice"));
        quit_app("Calculator");
        stop_test_app();
        result
    })
}

fn test(fixture: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let windows_on = |display_id: u64| -> Result<Vec<Window>> {
        let listed = control.call("listWindows", json!({ "displayId": display_id }))?;
        Ok(serde_json::from_value(listed["windows"].clone())?)
    };
    let calculator = |display_id: u64| -> Result<Option<Window>> {
        Ok(windows_on(display_id)?
            .into_iter()
            .find(|w| w.app == "Calculator"))
    };
    let titled = |display_id: u64, name: &str| -> Result<Option<Window>> {
        Ok(windows_on(display_id)?
            .into_iter()
            .find(|w| w.title == name))
    };

    let stage_id = open_stage(&control)?;
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
    control.call(
        "launchApp",
        json!({ "sessionKey": "viewerdrag", "displayId": id, "app": "Calculator" }),
    )?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    let image = control.call("debugViewer", json!({}))?["image"].clone();
    let before = calculator(id)?;
    check("Calculator opened on the display", before.is_some(), "");
    if let Some(before) = before {
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
    }
    quit_app("Calculator");
    let _ = control.call(
        "debugTakeover",
        json!({ "sessionKey": "viewerdrag", "displayId": id, "active": false }),
    );
    sleep_ms(300);

    let pane_state = out().join("viewerdrag-pane-state");
    let _ = std::fs::remove_file(&pane_state);
    control.call(
        "launchApp",
        json!({
            "sessionKey": "viewerdrag",
            "displayId": id,
            "app": fixture,
            "environment": {
                "MENU_CUSTOM_DRAG": "1",
                "MENU_DISPLAY_ID": id.to_string(),
                "MENU_STAGE_ID": stage_id.to_string(),
                "MENU_PANE_STATE": pane_state,
            }
        }),
    )?;
    for _ in 0..40 {
        if titled(id, "CustomDrag")?.is_some_and(|w| w.x < 100.0)
            && titled(id, "TallDrag")?.is_some_and(|w| w.x > 250.0)
            && titled(id, "SheetDrag")?.is_some_and(|w| w.x > 500.0)
            && pane_state.exists()
        {
            break;
        }
        sleep_ms(50);
    }

    // 1. Custom-title-bar window that yanks off the display 80 ms after mouseUp.
    let before_custom = titled(id, "CustomDrag")?.context("CustomDrag on display")?;
    let c_from = ((before_custom.x + 60.0) / w, (before_custom.y + 30.0) / h);
    let c_by = (0.10, 0.14);
    control.call(
        "debugViewerInput",
        json!({
            "kind": "drag",
            "x": c_from.0,
            "y": c_from.1,
            "toX": c_from.0 + c_by.0,
            "toY": c_from.1 + c_by.1,
        }),
    )?;
    sleep_ms(1000);
    let after_custom = titled(id, "CustomDrag")?;
    let custom_ok = after_custom.as_ref().is_some_and(|after| {
        let moved = (
            (after.x - before_custom.x) / w,
            (after.y - before_custom.y) / h,
        );
        (moved.0 - c_by.0).abs() < 0.03 && (moved.1 - c_by.1).abs() < 0.03
    });
    check(
        "a custom-title-bar window yanked off the display after mouseUp returns at its dragged position",
        custom_ok,
        format!("{} -> {}", json!(before_custom), json!(after_custom)),
    );

    // 2. Window taller than the display (`mid.y > h`), dragged past the Viewer image (`toX = 1.2`).
    let before_tall = titled(id, "TallDrag")?.context("TallDrag on display")?;
    let t_from = ((before_tall.x + 110.0) / w, (before_tall.y + 20.0) / h);
    let t_dy = 0.12;
    control.call(
        "debugViewerInput",
        json!({
            "kind": "drag",
            "x": t_from.0,
            "y": t_from.1,
            "toX": 1.2,
            "toY": t_from.1 + t_dy,
        }),
    )?;
    sleep_ms(900);
    let after_tall = titled(id, "TallDrag")?;
    let tall_ok = after_tall.as_ref().is_some_and(|after| {
        (after.height - before_tall.height).abs() < 2.0
            && (after.x - (w - before_tall.width)).abs() < 4.0
            && ((after.y - before_tall.y) / h - t_dy).abs() < 0.03
    });
    check(
        "a window taller than the display dragged outside the Viewer image clamps at the edge and keeps its height",
        tall_ok,
        format!("{} -> {}", json!(before_tall), json!(after_tall)),
    );

    // 3. Window with an attached modal sheet dragged by its title bar stays on the display.
    let before_sheet = titled(id, "SheetDrag")?.context("SheetDrag on display")?;
    let s_from = ((before_sheet.x + 110.0) / w, (before_sheet.y + 12.0) / h);
    let s_by = (-0.08, 0.10);
    control.call(
        "debugViewerInput",
        json!({
            "kind": "drag",
            "x": s_from.0,
            "y": s_from.1,
            "toX": s_from.0 + s_by.0,
            "toY": s_from.1 + s_by.1,
        }),
    )?;
    sleep_ms(800);
    let after_sheet = titled(id, "SheetDrag")?;
    let sheet_ok = after_sheet.as_ref().is_some_and(|after| {
        let moved = (
            (after.x - before_sheet.x) / w,
            (after.y - before_sheet.y) / h,
        );
        (moved.0 - s_by.0).abs() < 0.03 && (moved.1 - s_by.1).abs() < 0.03
    });
    check(
        "a window with an attached modal sheet drags cleanly and stays on the Agent Display",
        sheet_ok,
        format!("{} -> {}", json!(before_sheet), json!(after_sheet)),
    );

    // 4. Real Takeover Viewer clicks on an `.appex` pop-up menu behind a host window (#205, #207).
    control.call(
        "debugTakeover",
        json!({ "sessionKey": "viewerdrag", "displayId": id, "active": true }),
    )?;
    let pop_pt = ((610.0 + 115.0) / w, (60.0 + 74.0) / h);
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": pop_pt.0, "y": pop_pt.1, "key": 1 }),
    )?;
    let mut state = String::new();
    for _ in 0..25 {
        state = std::fs::read_to_string(&pane_state).unwrap_or_default();
        if state.contains("paneMenu=1") {
            break;
        }
        sleep_ms(50);
    }
    let menu_bounds = || -> Option<(f64, f64, f64, f64)> {
        for _ in 0..15 {
            let out = sh(
                "/usr/bin/swift",
                &[
                    "-e",
                    &format!(
                        "import CoreGraphics
let disp = CGDisplayBounds({id})
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list {{
  guard let layer = w[kCGWindowLayer as String] as? Int, layer == 101,
        let b = w[kCGWindowBounds as String] as? [String: Double],
        let x = b[\"X\"], let y = b[\"Y\"], let width = b[\"Width\"], let height = b[\"Height\"] else {{ continue }}
  let r = CGRect(x: x, y: y, width: width, height: height)
  if disp.intersects(r) {{ print(x - disp.origin.x, y - disp.origin.y, width, height); break }}
}}"
                    ),
                ],
            )
            .ok()?;
            let nums: Vec<f64> = out
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if let [mx, my, mw, mh] = nums[..] {
                return Some((mx, my, mw, mh));
            }
            sleep_ms(50);
        }
        None
    };

    if let Some((mx, my, mw, mh)) = menu_bounds() {
        control.call(
            "debugViewerInput",
            json!({
                "kind": "click",
                "x": (mx + mw * 0.5) / w,
                "y": (my + mh * 0.82) / h,
            }),
        )?;
        for _ in 0..25 {
            state = std::fs::read_to_string(&pane_state).unwrap_or_default();
            if state.contains("pane=Manually") && state.contains("paneMenu=0") {
                break;
            }
            sleep_ms(50);
        }
    }
    check(
        "Takeover Viewer click through a host window into an .appex NSMenu item picks Manually (#205, #207)",
        state.contains("pane=Manually") && state.contains("paneMenu=0"),
        state.clone(),
    );

    // Reopen the `.appex` NSMenu and click empty space in the Viewer to dismiss it (#207).
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": pop_pt.0, "y": pop_pt.1 }),
    )?;
    for _ in 0..25 {
        state = std::fs::read_to_string(&pane_state).unwrap_or_default();
        if state.contains("paneMenu=1") {
            break;
        }
        sleep_ms(50);
    }
    let reopened = state.contains("paneMenu=1");
    if reopened {
        control.call(
            "debugViewerInput",
            json!({ "kind": "click", "x": 0.92, "y": 0.88 }),
        )?;
        for _ in 0..25 {
            state = std::fs::read_to_string(&pane_state).unwrap_or_default();
            if state.contains("paneMenu=0") {
                break;
            }
            sleep_ms(50);
        }
    }
    check(
        "Takeover Viewer click on empty space dismisses an open .appex NSMenu (#207)",
        reopened && state.contains("pane=Manually") && state.contains("paneMenu=0"),
        state,
    );

    let _ = control.call(
        "debugTakeover",
        json!({ "sessionKey": "viewerdrag", "displayId": id, "active": false }),
    );
    let _ = std::process::Command::new("/usr/bin/pkill")
        .args(["-f", "MenuPane.appex/Contents/MacOS/MenuPane"])
        .status();
    kill_ours(our_binary("MenuChoice"));
    sleep_ms(300);
    let _ = control.call(
        "endDisplay",
        json!({ "sessionKey": "viewerdrag", "displayId": id }),
    );
    Ok(())
}
