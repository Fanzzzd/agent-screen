//! How far one scroll call moves a long virtualized list in Chrome (#104):
//! the agent asked for 60, 200, 400 and 800 wheel clicks and still hadn't
//! reached the end of 79 rows. Each amount scrolls down from the top of
//! tests/e2e/fixtures/web/longlist.html, 10,000 rows of 30 px, whose title
//! says how far it went. A wheel click moves Chrome's page 40 px.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e scrolljump
//!
//! Chrome on a throwaway profile; skips when Chrome runs with windows.
//! 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

const KEY: &str = "scrolljump";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[])?;
        let result = test();
        quit_profile(&out().join("scrolljump-profile"));
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("scrolljump-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Scroll", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let page = format!("file://{}", fixture("web/longlist.html").display());
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Google Chrome", "arguments": [
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run", "--no-default-browser-check", page,
        ] }),
    )?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .context("no window")?;
    let title = || -> Result<String> {
        let listed = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
        Ok(listed["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"] == window))
            .and_then(|w| w["title"].as_str())
            .unwrap_or_default()
            .to_string())
    };
    let top = |t: &str| -> f64 {
        t.split("top=")
            .nth(1)
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(f64::NAN)
    };
    sleep_ms(2000);
    let mut moved = Vec::new();
    for amount in [3.0, 60.0, 200.0, 800.0] {
        // Back to the top: reload the page.
        control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": "cmd+r" }),
        )?;
        sleep_ms(1500);
        let from = top(&title()?);
        control.call(
            "scroll",
            json!({ "sessionKey": KEY, "displayId": id, "x": 550, "y": 400, "deltaX": 0, "deltaY": -amount }),
        )?;
        sleep_ms(1200);
        let to = top(&title()?);
        println!("     {amount} clicks: top {from} -> {to}");
        moved.push((amount, to - from));
    }
    for (amount, by) in &moved {
        check(
            &format!(
                "{amount} wheel clicks move the list at least {} px",
                amount * 30.0
            ),
            *by >= amount * 30.0,
            format!("moved {by} px"),
        );
    }
    Ok(())
}
