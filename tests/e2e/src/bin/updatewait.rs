//! Install and Relaunch in Sparkle's own window asks the user first while
//! an Agent Display is open (ADR 0012). Sparkle's delegate reads the count
//! of open displays when the user clicks, so the count must follow every
//! display made and ended, not only while a downloaded update waits (#284).
//! `debugUpdateState` reports the count the delegate reads.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e updatewait
//!
//! Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, run, sleep_ms, start_test_app, stop_test_app,
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

/// The count of open displays Install and Relaunch reads, once it is
/// `want` or after 3 s.
fn open_displays(control: &Control, want: u64) -> Result<u64> {
    let mut open = u64::MAX;
    for _ in 0..30 {
        open = control.call("debugUpdateState", json!({}))?["openDisplays"]
            .as_u64()
            .context("openDisplays")?;
        if open == want {
            break;
        }
        sleep_ms(100);
    }
    Ok(open)
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let state = control.call("debugUpdateState", json!({}))?;
    check(
        "no update waits, so the update watch never ran",
        state["pending"].is_null(),
        state.to_string(),
    );
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "updatewait", "title": "Update wait" }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let open = open_displays(&control, 1)?;
    check(
        "with a display open, Install and Relaunch would ask first",
        open == 1,
        format!("open displays it reads: {open}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "updatewait", "displayId": id }),
    )?;
    let open = open_displays(&control, 0)?;
    check(
        "once the display ended, Install and Relaunch restarts at once",
        open == 0,
        format!("open displays it reads: {open}"),
    );
    Ok(())
}
