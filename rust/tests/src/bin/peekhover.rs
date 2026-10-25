//! Peek opened by hovering the menu bar icon closes once the pointer has
//! left the icon and Peek, however it left, or at the first click on the
//! icon. Hover is reported to the app
//! the way AppKit and GPUI report it, so the user's pointer is untouched
//! and this runs at any time. Peek opens on a stage display.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e peekhover

use std::collections::HashSet;
use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, check, instance, json, open_stage, pid_of, run, start_test_app,
    stop_test_app, windows_of,
};
use anyhow::{Result, bail};

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
    let Some(pid) = pid_of(DEBUG_BINARY) else {
        bail!("no test app");
    };
    // Each call waits 900 ms in the app, longer than Peek's hover delays.
    let show = |surface: &str, display_id: u64| {
        control.call(
            "debugShow",
            json!({ "surface": surface, "displayId": display_id }),
        )
    };

    open_stage(&control)?;
    let before: HashSet<u64> = windows_of(pid).iter().map(|w| w.id).collect();
    let peek_shown = || windows_of(pid).iter().any(|w| !before.contains(&w.id));

    show("icon-hover", 1)?;
    check("hovering the icon opens Peek", peek_shown(), "");
    show("icon-hover", 0)?;
    check("leaving the icon closes it", !peek_shown(), "");

    // The pointer goes onto Peek, and Escape closes it there. The pointer
    // then leaves Peek's spot, which is gone, so nothing reports that.
    show("icon-hover", 1)?;
    show("icon-hover", 0)?;
    show("card-hover", 1)?;
    show("hide", 0)?;
    check("Escape closes Peek", !peek_shown(), "");
    show("icon-hover", 1)?;
    check("hovering the icon opens it again", peek_shown(), "");
    show("icon-hover", 0)?;
    check("leaving the icon closes it again", !peek_shown(), "");

    // Opened by hover, the first click on the icon closes it, and hover
    // doesn't bring it back until the pointer leaves the icon.
    show("icon-hover", 1)?;
    show("click", 0)?;
    check("a click closes Peek opened by hover", !peek_shown(), "");
    show("card-hover", 0)?;
    check("it stays closed under the pointer", !peek_shown(), "");
    show("icon-hover", 0)?;
    show("icon-hover", 1)?;
    check("hovering after leaving opens it", peek_shown(), "");
    show("click", 0)?;
    show("icon-hover", 0)?;
    show("click", 0)?;
    check("a click on the closed icon opens it", peek_shown(), "");
    show("click", 0)?;
    check("the next click closes it", !peek_shown(), "");
    Ok(())
}
