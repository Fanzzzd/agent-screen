//! The Settings window opens, fits its content, and closes with Done.
//! Captures it under target/e2e. It shows on the user's screen and takes
//! the keyboard for a few seconds.
//!
//!     AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e settings

use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, Frame, capture, check, instance, json, pid_of, run, sleep_ms,
    start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() || std::env::var_os("E2E_NOW").is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e E2E_NOW=1: the window shows on your screen");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let pid = pid_of(DEBUG_BINARY).context("no test app")?;
    // The window is 520 pt wide.
    let window = || -> Option<Frame> { windows_of(pid).into_iter().find(|w| w.width == 520.0) };

    control.call("debugShow", json!({ "surface": "settings" }))?;
    sleep_ms(1200);
    let shown = window();
    check("Settings opens", shown.is_some(), "");
    if let Some(w) = shown {
        check(
            "it fits on a 13-inch screen",
            w.height <= 800.0,
            format!("{} pt high", w.height),
        );
        let file = capture("settings", [w.x, w.y, w.width, w.height])?;
        println!("     window: {}", file.display());
    }
    control.call("debugShow", json!({ "surface": "setup-close" }))?;
    sleep_ms(400);
    check("Done closes it", window().is_none(), "");
    Ok(())
}
