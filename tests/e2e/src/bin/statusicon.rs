//! The menu bar icon follows the app's state: two outlined screens while no
//! display is in use, the front one filled while one is, and outlined again
//! once it has ended. debugIcon says which picture shows; the pictures are
//! assets/status/*.svg.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e statusicon
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

fn state(control: &Control) -> Result<String> {
    Ok(control.call("debugIcon", json!({}))?["state"]
        .as_str()
        .context("icon state")?
        .to_string())
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let idle = state(&control)?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "icon", "title": "Icon", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    sleep_ms(500);
    let working = state(&control)?;
    check(
        "the icon's front screen fills while a display is in use",
        idle == "idle" && working == "working",
        format!("before {idle}, with a display {working}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "icon", "displayId": id }),
    )?;
    sleep_ms(500);
    let after = state(&control)?;
    check(
        "the icon is outlined again once the display has ended",
        after == "idle",
        after,
    );
    Ok(())
}
