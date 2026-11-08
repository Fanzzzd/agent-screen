//! A closed HTML select in Chrome chosen with act_on_element set_value
//! (#79): each month in one call, by type-ahead sent to Chrome's process
//! only, with no menu opened. A prefix of a title fails and names what the
//! select shows. The page writes what it holds into its title:
//! tests/e2e/fixtures/web/dropdowns.html.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e selectvalue
//!
//! Chrome on a throwaway profile; skips when Chrome runs with windows.
//! 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, probe, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "selectvalue";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_profile(&out().join("selectvalue-profile"));
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
    let result = (|| -> Result<()> {
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
        let pid = launched["pid"].as_i64().context("no pid")?;
        sleep_ms(1500);
        let call = |method: &str, mut params: Value| {
            params["sessionKey"] = json!(KEY);
            params["displayId"] = json!(id);
            params["windowId"] = json!(window);
            control.call(method, params)
        };
        // Chrome's open menus are windows at the pop-up menu level.
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
                .with_context(|| format!("no select in\n{tree}"))?;
            Ok(found[1].parse()?)
        };
        // The page names its window after what the select shows.
        let shows = |month: &str| -> Result<String> {
            let mut title = String::new();
            for _ in 0..20 {
                title = call("listWindows", json!({}))?["windows"]
                    .as_array()
                    .and_then(|a| a.iter().find(|w| w["windowId"] == window))
                    .and_then(|w| w["title"].as_str())
                    .unwrap_or_default()
                    .to_string();
                if title.contains(&format!("native={month}")) {
                    break;
                }
                sleep_ms(100);
            }
            Ok(title)
        };
        for month in ["April", "February", "May"] {
            let started = std::time::Instant::now();
            let set = call(
                "elementAction",
                json!({ "element": select()?, "action": "set_value", "value": month }),
            );
            let took = started.elapsed().as_millis();
            let title = shows(month)?;
            let open = menus();
            check(
                &format!("set_value {month} chooses it in one call, with no menu open"),
                set.is_ok() && title.contains(&format!("native={month}")) && open == 0,
                format!("{set:?} in {took} ms; {title}; menus open {open}"),
            );
        }
        call(
            "elementAction",
            json!({ "element": select()?, "action": "set_value", "value": "April" }),
        )?;
        let prefix = call(
            "elementAction",
            json!({ "element": select()?, "action": "set_value", "value": "Apr" }),
        );
        let error = prefix
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default();
        let title = shows("April")?;
        check(
            "a title's prefix fails and names what the select shows",
            error.contains("shows \"April\"") && title.contains("native=April") && menus() == 0,
            format!("{prefix:?}; {title}"),
        );
        Ok(())
    })();
    // Close the browser before its display ends.
    quit_profile(&profile);
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}
