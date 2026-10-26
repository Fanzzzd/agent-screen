//! The user clicks the Viewer while another window has the keyboard. The
//! click makes the Viewer key, and AppKit draws it at once, inside the
//! click's handling: the handler held the Viewer's input state while the
//! Viewer's paint wrote its layout there, and the app aborted. Clicks go
//! through the Viewer's own handler, so the user's pointer is untouched.
//! Then, in the Takeover the clicks started, Command-V after each click
//! (issue #40).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerkey
//!
//! Makes 2 displays: the stage and the one the Viewer shows.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, debug_pid, instance, json, open_stage, run, sleep_ms, start_test_app,
    stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let pid = debug_pid().context("no test app")?;
    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "viewerkey", "title": "Viewer key", "width": 900, "height": 600 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    // The first click makes the Viewer key for the first time; AppKit
    // draws synchronously only when it becomes key again.
    for round in 1..=3 {
        let clicked = control.call(
            "debugViewerInput",
            json!({ "kind": "click", "x": 0.5, "y": 0.6, "key": 1 }),
        );
        sleep_ms(400);
        let alive = debug_pid() == Some(pid);
        check(
            &format!("the app is still running after click {round} takes the keyboard"),
            alive && clicked.is_ok(),
            format!("{clicked:?}"),
        );
        if !alive {
            return Ok(());
        }
    }
    // The clicks took the display over. The user pastes into it: Command-V
    // straight after a click that took the keyboard back (issue #40).
    let typed = control.call("type", json!({ "displayId": id, "text": "x" }));
    check(
        "the clicks took the display over",
        typed.is_err(),
        format!("{typed:?}"),
    );
    for round in 1..=3 {
        control.call(
            "debugViewerInput",
            json!({ "kind": "click", "x": 0.5, "y": 0.6, "key": 1 }),
        )?;
        let pasted = control.call(
            "debugViewerInput",
            json!({ "kind": "key", "code": 9, "command": 1 }),
        );
        sleep_ms(400);
        let alive = debug_pid() == Some(pid);
        check(
            &format!("the app is still running after paste {round} in the Takeover"),
            alive && pasted.is_ok(),
            format!("{pasted:?}"),
        );
        if !alive {
            return Ok(());
        }
    }
    let key = control.call("debugViewer", json!({}))?["key"].as_bool();
    check(
        "the Viewer is the key window",
        key == Some(true),
        format!("{key:?}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "viewerkey", "displayId": id }),
    )?;
    Ok(())
}
