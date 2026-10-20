//! Three "+" in quick succession: each tile grows in while the last is
//! still growing and displays arrive meanwhile. Once all have come, Peek is
//! one card the size of its window, showing every display once.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e peekrapid
//!
//! Makes 4 displays: the stage and three new ones.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, capture, check, instance, json, open_stage, pid_of, pixel, run,
    sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        // No spares made meanwhile: every "+" makes a display.
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_IDLE_SECS", "1000000"),
        ])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let pid = pid_of(DEBUG_BINARY).context("no test app")?;
    open_stage(&control)?;
    let before: Vec<u64> = windows_of(pid).iter().map(|w| w.id).collect();
    // Opened by a click, as the user does: Peek takes the keyboard.
    control.call("debugShow", json!({ "surface": "click" }))?;
    sleep_ms(600);
    let peek = windows_of(pid)
        .into_iter()
        .find(|w| !before.contains(&w.id))
        .context("Peek didn't open")?;
    for _ in 0..3 {
        control.call("debugShow", json!({ "surface": "create" }))?;
        sleep_ms(
            std::env::var("GAP_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1500),
        );
    }
    sleep_ms(5000);
    let after = windows_of(pid)
        .into_iter()
        .find(|w| w.id == peek.id)
        .context("Peek closed")?;
    let shot = capture("peekrapid", [after.x, after.y, after.width, after.height])?;
    // Four display tiles and the "+": the card's edges are the window's.
    let (wide, _, _) = (after.width, 0, 0);
    let left = pixel(&shot, 3.0, after.height / 2.0);
    let right = pixel(&shot, after.width - 3.0, after.height / 2.0);
    let gray = |p: &[f64]| p.len() >= 3 && p[0] > 25.0 && (p[2] - p[0]).abs() < 12.0;
    check(
        "Peek is one card filling its window after three quick +",
        gray(&left) && gray(&right),
        format!(
            "window {wide} wide; left {left:?}, right {right:?} in {}",
            shot.display()
        ),
    );
    control.call("debugShow", json!({ "surface": "hide" }))?;
    Ok(())
}
