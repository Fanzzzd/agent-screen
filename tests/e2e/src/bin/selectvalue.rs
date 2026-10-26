//! A closed Chromium select chooses by type-ahead and verifies its value.
//! AGENTSCREEN_INSTANCE=e2e cargo e2e selectvalue
//! Skips when Chrome or the e2e instance is already running.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, listening, out, probe, run, running_headed, skip, sleep_ms,
    start_test_app, stop_started_pid, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "selectvalue";

fn main() -> ExitCode {
    if listening() {
        skip("the e2e socket is in use; leave that instance alone");
    }
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
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
    let profile = out().join("selectvalue-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/dropdowns.html").display());
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Select value", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Google Chrome", "arguments": [
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run", "--no-default-browser-check", page,
        ] }),
    )?;
    let result = (|| -> Result<()> {
        let window = launched["window"]["windowId"]
            .as_u64()
            .context("no window")?;
        let pid = launched["pid"].as_i64().context("no pid")?;
        sleep_ms(1500);
        let call = |method: &str, mut params: Value| {
            params["sessionKey"] = json!(KEY);
            params["displayId"] = json!(id);
            params["windowId"] = json!(window);
            control.call(method, params)
        };
        let menus = || {
            probe(&["windows", &pid.to_string()])
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains(r#""layer":101"#))
                .count()
        };
        let select = || -> Result<i64> {
            let tree = call("windowState", json!({}))?["tree"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let found = Regex::new(r#"\[(\d+)\] popupbutton "Native month""#)?
                .captures(&tree)
                .context("no select in the tree")?;
            Ok(found[1].parse()?)
        };
        // The page names its window after what the select shows, a moment later.
        let shows = |month: &str| -> Result<String> {
            let mut title = String::new();
            for _ in 0..20 {
                title = call("listWindows", json!({}))?["windows"][0]["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if title.contains(&format!("native={month}")) {
                    break;
                }
                sleep_ms(100);
            }
            Ok(title)
        };
        for month in ["April", "February", "may"] {
            let started = std::time::Instant::now();
            let set = call(
                "elementAction",
                json!({ "element": select()?, "action": "set_value", "value": month }),
            );
            let took = started.elapsed().as_millis();
            let wanted = if month == "may" { "May" } else { month };
            let title = shows(wanted)?;
            check(
                &format!("set_value {month} chooses it in one call, with no menu left open"),
                set.is_ok() && title.contains(&format!("native={wanted}")) && menus() == 0,
                format!("{set:?} in {took} ms; {title}; menus open {}", menus()),
            );
        }
        let prefix = call(
            "elementAction",
            json!({"element":select()?,"action":"set_value","value":"Apr"}),
        );
        let error = prefix
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default();
        let title = shows("April")?;
        check(
            "a prefix that changes the choice reports the actual value instead of success",
            error.contains("shows \"April\"") && title.contains("native=April") && menus() == 0,
            format!("{error}; {title}"),
        );
        Ok(())
    })();
    // Close our browser before returning its display to review.
    if let Some(pid) = launched["pid"].as_u64() {
        stop_started_pid(pid as u32)?;
    }
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}
