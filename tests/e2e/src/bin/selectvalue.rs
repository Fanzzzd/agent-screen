//! A closed HTML select in a Chromium browser chosen with act_on_element
//! set_value (#79). The browser launch_app starts has a private DevTools
//! pipe (ADR 0010), so each option is chosen in one step: one input and one
//! change event, with no option passed on the way and no menu opened.
//! Titles with a space, a repeated key and CJK are chosen; the title the
//! select shows already sends nothing; a title no option has fails and says
//! so. Selects in a frame from another site, out of the pipe's reach, are
//! refused before any key, as typing into a web select could open its
//! menu. The page writes what it holds and its event counts into its title:
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
        // Counting one also closes it at once through accessibility, as
        // the user's click would: an open menu holds the user's keyboard,
        // and a regression must not leave it so while the script fails.
        let menus = || {
            let open: Vec<Value> = probe(&["windows", &pid.to_string()])
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(|w| w["layer"] == 101)
                .collect();
            for w in &open {
                let n = |k: &str| w[k].as_f64().unwrap_or_default();
                let (x, y) = (n("x") + n("width") / 2.0, n("y") + n("height") / 2.0);
                let _ = probe(&["cancelmenu", &x.to_string(), &y.to_string()]);
            }
            open.len()
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
                if shown.contains(&format!("city={city} |")) {
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
                    && shown.contains(&format!("i={changes} c={changes} city={city} "))
                    && open == 0,
                format!("{result:?} in {took} ms; {shown}; menus open {open}"),
            );
        }
        let again = set("Newark")?;
        let shown = title()?;
        check(
            "the title the select shows already sends nothing",
            again.is_ok() && shown.contains(&format!("c={changes} city=Newark ")),
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
                && shown.contains(&format!("c={changes} city=Newark "))
                && menus() == 0,
            format!("{missing:?}; {shown}"),
        );
        // The selects in the other site's frame are out of the pipe's
        // reach, and typing into a web select could open its menu: each is
        // refused before any key, and the zip field the country's change
        // handler would focus stays empty.
        let framed = |name: &str| -> Result<i64> {
            let tree = call("windowState", json!({}))?["tree"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let found = Regex::new(&format!(r#"\[(\d+)\] popupbutton "{name}""#))?
                .captures(&tree)
                .with_context(|| format!("no {name} select in\n{tree}"))?;
            Ok(found[1].parse()?)
        };
        for (name, value) in [
            ("Town", "United States"),
            ("Town", "11"),
            ("Country", "Blueberry"),
        ] {
            let before = title()?;
            let refused = call(
                "elementAction",
                json!({ "element": framed(name)?, "action": "set_value", "value": value }),
            );
            sleep_ms(300);
            let after = title()?;
            let open = menus();
            check(
                &format!("{name} {value:?} in another site's frame is refused before any key"),
                refused.as_ref().err().is_some_and(|e| {
                    let e = format!("{e:#}");
                    e.contains("out of its reach") && e.contains("nothing was sent")
                }) && after == before
                    // The browser trims the title's trailing space.
                    && after.trim_end().ends_with(" zip=")
                    && open == 0,
                format!("{refused:?}; {before} -> {after}; menus open {open}"),
            );
        }
        Ok(())
    })();
    // Close the browser before its display ends.
    quit_profile(&profile);
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}
