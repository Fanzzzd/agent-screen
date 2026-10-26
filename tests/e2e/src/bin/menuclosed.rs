//! A page's select menu in Chrome, on a throwaway profile (issues #48,
//! #79). macOS closes an app's open menu at the user's next click or key
//! anywhere, so a menu the agent opened can be gone by its next call. Typing
//! into an open menu chooses its item; once the menu has closed, the next
//! click or key fails and says so, instead of landing on the page under it.
//! The script closes the menu through accessibility, as the user's click
//! would, without touching the pointer or keyboard.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e menuclosed
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

const KEY: &str = "menuclosed";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_profile(&out().join("menuclosed-profile"));
        stop_test_app();
        result
    })
}

fn center(tree: &str, pattern: &str) -> Result<(f64, f64)> {
    let found = Regex::new(&format!(r"{pattern}[^\n]* at \((\d+),(\d+)\)"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

/// The center of Chrome's open menu, in global points.
fn open_menu(pid: i64) -> Option<(f64, f64)> {
    probe(&["windows", &pid.to_string()])
        .ok()?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|w| w["layer"] == 101)
        .map(|w| {
            let n = |k: &str| w[k].as_f64().unwrap_or_default();
            (n("x") + n("width") / 2.0, n("y") + n("height") / 2.0)
        })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("menuclosed-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/dropdowns.html").display());
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Menu closed", "width": 1100, "height": 700 }),
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
    let call = |method: &str, mut params: Value| {
        params["sessionKey"] = json!(KEY);
        params["displayId"] = json!(id);
        control.call(method, params)
    };
    let title = || -> Result<String> {
        let list = call("listWindows", json!({}))?;
        Ok(list["windows"][0]["title"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    let tree = call("windowState", json!({ "windowId": window }))?["tree"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let select = center(&tree, r#"popupbutton "Native month""#)?;
    // Open the menu and look at it, as an agent does.
    let open = || -> Result<Option<(f64, f64)>> {
        call("click", json!({ "x": select.0, "y": select.1 }))?;
        let mut menu = None;
        for _ in 0..20 {
            menu = open_menu(pid);
            if menu.is_some() {
                break;
            }
            sleep_ms(100);
        }
        call("screenshot", json!({ "format": "png" }))?;
        Ok(menu)
    };

    let menu = open()?;
    check("a click on the select opens its menu", menu.is_some(), "");
    call("type", json!({ "text": "Ap" }))?;
    call("key", json!({ "keys": "Return" }))?;
    // Chrome names its window after the page's title a moment later.
    let mut chosen = String::new();
    for _ in 0..30 {
        chosen = title()?;
        if chosen.contains("native=April") {
            break;
        }
        sleep_ms(100);
    }
    check(
        "typing into the open menu chooses its item",
        chosen.contains("native=April"),
        &chosen,
    );

    let menu = open()?.context("the menu didn't open again")?;
    let closed = probe(&["cancelmenu", &menu.0.to_string(), &menu.1.to_string()])?;
    sleep_ms(300);
    check(
        "the menu closes as at a click elsewhere",
        open_menu(pid).is_none(),
        &closed,
    );
    let typed = call("type", json!({ "text": "Ma" }));
    let error = typed
        .as_ref()
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    check(
        "typing meant for the closed menu fails and says the menu closed",
        error.contains("closed before"),
        format!("{typed:?}"),
    );
    sleep_ms(1500);
    let kept = title()?;
    check(
        "the select keeps its value",
        kept.contains("native=April"),
        &kept,
    );
    call("screenshot", json!({ "format": "png" }))?;
    let after = call("key", json!({ "keys": "Escape" }));
    check(
        "after a screenshot shows it closed, keys go through again",
        after.is_ok(),
        format!("{after:?}"),
    );
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    Ok(())
}
