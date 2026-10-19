//! A system alert that lands on an Agent Display goes to the main display
//! (issue #22). The script shows its own alert, drawn by
//! UserNotificationCenter as macOS's prompts are, and moves it onto the
//! display; Sidescreen moves it back within 2 s. The alert shows on the
//! user's screen for a moment at the start and the end.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e alerts

use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::Value;
use agent_screen_tests::{
    Control, check, instance, json, pid_of, probe, run, sleep_ms, start_test_app, stop_test_app,
    tool,
};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let mut alert = std::process::Command::new(tool("alert")?)
            .arg("20")
            .spawn()?;
        let result = test();
        let _ = alert.kill();
        stop_test_app();
        result
    })
}

/// The test alert's window: (pid, id, x, y, width, height).
fn alert_window() -> Option<(u32, u64, f64, f64, f64, f64)> {
    let pid = pid_of("UserNotificationCenter")?;
    probe(&["windows", &pid.to_string()])
        .ok()?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|w| w["layer"].as_i64().unwrap_or(0) > 0)
        .map(|w| {
            let n = |k: &str| w[k].as_f64().unwrap_or(0.0);
            (
                pid,
                w["id"].as_u64().unwrap_or(0),
                n("x"),
                n("y"),
                n("width"),
                n("height"),
            )
        })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "alerts", "title": "Alerts", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].as_u64().context("displayId")?;
    let bounds: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let mut window = None;
    for _ in 0..30 {
        window = alert_window();
        if window.is_some() {
            break;
        }
        sleep_ms(100);
    }
    let (pid, window_id, ..) = window.context("the test alert didn't show")?;
    let moved = control.call(
        "debugMoveWindow",
        json!({ "pid": pid, "windowId": window_id, "x": bounds[0] + 400.0, "y": bounds[1] + 200.0 }),
    )?;
    let on_display = |w: (u32, u64, f64, f64, f64, f64)| {
        let (x, y) = (w.2 + w.4 / 2.0, w.3 + w.5 / 2.0);
        x >= bounds[0] && x < bounds[0] + bounds[2] && y >= bounds[1] && y < bounds[1] + bounds[3]
    };
    sleep_ms(150);
    check(
        "the test put the alert on the agent display",
        moved["moved"] == true && alert_window().is_some_and(on_display),
        format!("{:?}", alert_window()),
    );
    sleep_ms(2000);
    let main: Vec<f64> = probe(&["main"])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let after = alert_window();
    check(
        "Sidescreen moved the alert to the main display",
        after.is_some_and(|w| {
            let (x, y) = (w.2 + w.4 / 2.0, w.3 + w.5 / 2.0);
            x >= 0.0 && x < main[0] && y >= 0.0 && y < main[1]
        }),
        format!("{after:?}"),
    );
    Ok(())
}
