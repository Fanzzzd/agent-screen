//! A closed HTML select in a Chromium browser chosen with act_on_element
//! set_value (#79). The browser launch_app starts has a private DevTools
//! pipe (ADR 0010), so each option is chosen in one step: one input and one
//! change event, with no option passed on the way and no menu opened.
//! Titles with a space, a repeated key and CJK are chosen; the title the
//! select shows already sends nothing; a title no option has fails and says
//! so. The page writes what it holds and its event counts into its title:
//! tests/e2e/fixtures/web/selects.html.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e selectvalue
//!
//! Chrome on a throwaway profile, or Brave when any Chrome runs, headless
//! ones included, since launch_app then takes Chrome for the running copy.
//! Skips when both run. 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, probe, quit_profile, run, running, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "selectvalue";

fn main() -> ExitCode {
    let browser = if !running("Google Chrome") {
        "Google Chrome"
    } else if !running("Brave Browser") {
        "Brave Browser"
    } else {
        skip("Chrome and Brave are running; quit one to test");
    };
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(browser);
        quit_profile(&out().join("selectvalue-profile"));
        stop_test_app();
        result
    })
}

fn test(browser: &str) -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("selectvalue-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/selects.html").display());
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Select value", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let result = (|| -> Result<()> {
        let launched = control.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": browser, "arguments": [
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
        // A browser's open menus are windows at the pop-up menu level.
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
            let found = Regex::new(r#"\[(\d+)\] popupbutton "City""#)?
                .captures(&tree)
                .with_context(|| format!("no select in\n{tree}"))?;
            Ok(found[1].parse()?)
        };
        let title = || -> Result<String> {
            Ok(call("listWindows", json!({}))?["windows"]
                .as_array()
                .and_then(|a| a.iter().find(|w| w["windowId"] == window))
                .and_then(|w| w["title"].as_str())
                .unwrap_or_default()
                .to_string())
        };
        // The page's title once it shows `city`.
        let shows = |city: &str| -> Result<String> {
            let mut shown = String::new();
            for _ in 0..20 {
                shown = title()?;
                if shown.contains(&format!("city={city} ")) {
                    break;
                }
                sleep_ms(100);
            }
            Ok(shown)
        };
        let set = |value: &str| -> Result<Result<Value>> {
            Ok(call(
                "elementAction",
                json!({ "element": select()?, "action": "set_value", "value": value }),
            ))
        };
        let mut changes = 0;
        for city in ["New York", "11", "東京", "1", "Newark"] {
            let started = std::time::Instant::now();
            let result = set(city)?;
            let took = started.elapsed().as_millis();
            changes += 1;
            let shown = shows(city)?;
            let open = menus();
            check(
                &format!("set_value {city:?} chooses it with one change event and no menu"),
                result.is_ok()
                    && shown.contains(&format!("city={city} "))
                    && shown.contains(&format!("inputs={changes} changes={changes}"))
                    && open == 0,
                format!("{result:?} in {took} ms; {shown}; menus open {open}"),
            );
        }
        let again = set("Newark")?;
        let shown = title()?;
        check(
            "the title the select shows already sends nothing",
            again.is_ok() && shown.contains(&format!("changes={changes}")),
            format!("{again:?}; {shown}"),
        );
        let missing = set("new york")?;
        let error = missing
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default();
        let shown = title()?;
        check(
            "a title no option has fails, names the near one and changes nothing",
            error.contains("differs in case only")
                && shown.contains("city=Newark ")
                && shown.contains(&format!("changes={changes}"))
                && menus() == 0,
            format!("{missing:?}; {shown}"),
        );
        // The framed select gets type-ahead: a space mid-title must not
        // open its menu, and a repeated key is refused before any key.
        let town = || -> Result<i64> {
            let tree = call("windowState", json!({}))?["tree"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let found = Regex::new(r#"\[(\d+)\] popupbutton "Town""#)?
                .captures(&tree)
                .with_context(|| format!("no framed select in\n{tree}"))?;
            Ok(found[1].parse()?)
        };
        let typed = call(
            "elementAction",
            json!({ "element": town()?, "action": "set_value", "value": "New York" }),
        );
        let mut shown = String::new();
        for _ in 0..20 {
            shown = title()?;
            if shown.contains("town=New York ") {
                break;
            }
            sleep_ms(100);
        }
        let open = menus();
        check(
            "type-ahead with a space mid-title chooses the framed option with no menu",
            typed.is_ok() && shown.contains("town=New York ") && open == 0,
            format!("{typed:?}; {shown}; menus open {open}"),
        );
        let repeated = call(
            "elementAction",
            json!({ "element": town()?, "action": "set_value", "value": "11" }),
        );
        let after = title()?;
        check(
            "a repeated key is refused for a typed web select, and nothing changes",
            repeated
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("can't reach \"11\""))
                && after.contains("town=New York ")
                && menus() == 0,
            format!("{repeated:?}; {after}"),
        );
        Ok(())
    })();
    // Close the browser before its display ends.
    quit_profile(&profile);
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}
