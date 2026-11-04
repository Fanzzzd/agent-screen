//! Chrome's first window when launch_app starts it (issue #123): RUNS times
//! (default 20) on one display, each on a new throwaway profile, so Chrome
//! has no saved window place. Chrome creates the window on the main screen,
//! clear, and draws it about 250 ms later; it must be on the display by
//! then. The run's screen watch fails on any frame of it on your screen.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e coldflash
//!     RUNS=5 ...
//!
//! Skips when Chrome is running with windows. Quits each copy it started.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, env_count, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

const KEY: &str = "coldflash";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_profile(&profile());
        stop_test_app();
        result
    })
}

fn profile() -> std::path::PathBuf {
    out().join("coldflash-profile")
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Cold flash", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let runs = env_count("RUNS", 20.0) as usize;
    let mut placed = 0;
    for run in 1..=runs {
        let _ = std::fs::remove_dir_all(profile());
        let launched = control.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": "Google Chrome", "arguments": [
                format!("--user-data-dir={}", profile().display()),
                "--no-first-run", "--no-default-browser-check", "about:blank",
            ] }),
        );
        match launched {
            Ok(l) if l["window"]["windowId"].is_u64() => placed += 1,
            other => println!("run {run}: no window: {other:?}"),
        }
        quit_profile(&profile());
        sleep_ms(500);
    }
    check(
        "every launch answers with its window",
        placed == runs,
        format!("{placed}/{runs}"),
    );
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    Ok(())
}
