//! A system alert that lands on an Agent Display goes to the main display
//! (issue #22). The script shows two alerts in turn and moves each onto the
//! display; Agent Screen moves it back within 2 s. The first is its own
//! alert, drawn by UserNotificationCenter as macOS's prompts are. The second
//! stands in for the Apple Account sign-in alert, which FollowUpUI draws:
//! `tools/followup.swift` run as a binary named FollowUpUI. Each alert shows
//! on the user's screen for a moment at the start and the end, after a
//! countdown there.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e alerts

use std::process::{Child, ExitCode};

use agent_screen_tests::{
    Control, HandsOff, check, command, instance, json, out, pid_of, probe, run, sleep_ms,
    start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let mut sign = HandsOff::start("system alerts show on your screen")?;
        let result = test(&mut sign);
        drop(sign);
        stop_test_app();
        result
    })
}

/// An alert's window: (pid, id, x, y, width, height).
type Alert = (u32, u64, f64, f64, f64, f64);

/// The alert window of `pid`, above normal windows.
fn alert_window(pid: u32) -> Option<Alert> {
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

/// The stand-in for FollowUpUI: tools/followup.swift under that name.
fn follow_up() -> Result<Child> {
    let built = tool("followup")?;
    let named = out().join("FollowUpUI");
    std::fs::copy(&built, &named)?;
    Ok(command(&named).arg("20").spawn()?)
}

fn test(sign: &mut HandsOff) -> Result<()> {
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

    sign.step("a UserNotificationCenter alert");
    let mut alert = command(tool("alert")?).arg("20").spawn()?;
    let result = (|| {
        let mut pid = None;
        for _ in 0..30 {
            pid = pid_of("UserNotificationCenter").filter(|&p| alert_window(p).is_some());
            if pid.is_some() {
                break;
            }
            sleep_ms(100);
        }
        sent_home(
            &control,
            &bounds,
            "UserNotificationCenter",
            pid.context("the test alert didn't show")?,
        )
    })();
    let _ = alert.kill();
    result?;

    sign.step("an Apple Account sign-in alert from FollowUpUI");
    let mut follow = follow_up()?;
    let pid = follow.id();
    for _ in 0..30 {
        if alert_window(pid).is_some() {
            break;
        }
        sleep_ms(100);
    }
    let result = sent_home(&control, &bounds, "FollowUpUI", pid);
    let _ = follow.kill();
    let _ = control.call(
        "endDisplay",
        json!({ "sessionKey": "alerts", "displayId": id }),
    );
    result
}

/// Put `pid`'s alert on the display at `bounds`; Agent Screen moves it to
/// the main display.
fn sent_home(control: &Control, bounds: &[f64], owner: &str, pid: u32) -> Result<()> {
    let (_, window_id, ..) =
        alert_window(pid).with_context(|| format!("the {owner} alert didn't show"))?;
    let moved = control.call(
        "debugMoveWindow",
        json!({ "pid": pid, "windowId": window_id, "x": bounds[0] + 400.0, "y": bounds[1] + 200.0 }),
    )?;
    let on_display = |w: Alert| {
        let (x, y) = (w.2 + w.4 / 2.0, w.3 + w.5 / 2.0);
        x >= bounds[0] && x < bounds[0] + bounds[2] && y >= bounds[1] && y < bounds[1] + bounds[3]
    };
    sleep_ms(150);
    check(
        &format!("the test put the {owner} alert on the agent display"),
        moved["moved"] == true && alert_window(pid).is_some_and(on_display),
        format!("{:?}", alert_window(pid)),
    );
    sleep_ms(2000);
    let main: Vec<f64> = probe(&["main"])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let after = alert_window(pid);
    check(
        &format!("Agent Screen moved the {owner} alert to the main display"),
        after.is_some_and(|w| {
            let (x, y) = (w.2 + w.4 / 2.0, w.3 + w.5 / 2.0);
            x >= 0.0 && x < main[0] && y >= 0.0 && y < main[1]
        }),
        format!("{after:?}"),
    );
    Ok(())
}
