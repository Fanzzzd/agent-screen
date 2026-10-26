//! act_on_element set_value on a page's select in Chrome, on a throwaway
//! profile (issues #48, #79): one call opens its menu, presses the item it
//! names and leaves no menu open, so the user's clicks and keys between
//! the agent's calls can't close it or go into it. A name that matches no
//! item, or several, fails, lists the items and changes nothing.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e selectvalue
//!
//! Skips when Chrome is running with windows. Quits the copy it started.

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
    let ambiguous = call(
        "elementAction",
        json!({ "element": select()?, "action": "set_value", "value": "Ma" }),
    );
    let error = ambiguous
        .as_ref()
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    sleep_ms(500);
    let title = shows("May")?;
    check(
        "a name two items match fails, lists the items and changes nothing",
        error.contains("\"March\"") && title.contains("native=May") && menus() == 0,
        format!("{error}; {title}; menus open {}", menus()),
    );
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    Ok(())
}
