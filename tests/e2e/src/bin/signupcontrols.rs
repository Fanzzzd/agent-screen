//! A Google-style signup page's controls in Chrome (#79): a custom
//! combobox whose listbox options take a click, and radio buttons. Each is
//! operated as the report did: act_on_element press, a click at the
//! option's point, and focus then the space key. The page writes what it
//! holds into its title: tests/e2e/fixtures/web/dropdowns.html.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e signupcontrols
//!
//! Chrome on a throwaway profile; skips when Chrome runs with windows.
//! 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Value, json as j};

const KEY: &str = "signupcontrols";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_profile(&out().join("signupcontrols-profile"));
        stop_test_app();
        result
    })
}

/// An element's number and center in a window state's tree.
fn find(tree: &str, pattern: &str) -> Result<(i64, (f64, f64))> {
    let found = Regex::new(&format!(r"\[(\d+)\] {pattern}[^\n]* at \((\d+),(\d+)\)"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern} in\n{tree}"))?;
    Ok((found[1].parse()?, (found[2].parse()?, found[3].parse()?)))
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("signupcontrols-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Signup", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let page = format!("file://{}", fixture("web/dropdowns.html").display());
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
    sleep_ms(2000);
    let with = |mut v: Value| {
        v["sessionKey"] = j!(KEY);
        v["displayId"] = j!(id);
        v
    };
    let tree = || -> Result<String> {
        Ok(
            control.call("windowState", with(j!({ "windowId": window })))?["tree"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    };
    let title = || -> Result<String> {
        let listed = control.call("listWindows", with(j!({})))?;
        Ok(listed["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"] == window))
            .and_then(|w| w["title"].as_str())
            .unwrap_or_default()
            .to_string())
    };
    let press = |pattern: &str| -> Result<()> {
        let (n, _) = find(&tree()?, pattern)?;
        control.call(
            "elementAction",
            with(j!({ "windowId": window, "element": n, "action": "press" })),
        )?;
        sleep_ms(500);
        Ok(())
    };
    let click = |pattern: &str| -> Result<()> {
        let (_, (x, y)) = find(&tree()?, pattern)?;
        control.call("click", with(j!({ "x": x, "y": y })))?;
        sleep_ms(500);
        Ok(())
    };
    let open_combo = || -> Result<()> {
        if !tree()?.contains(r#"list "Months""#) {
            press("combobox")?;
        }
        Ok(())
    };

    // The custom month, three ways.
    open_combo()?;
    press(r#"statictext value="April""#)?;
    let t = title()?;
    check(
        "act_on_element press on an option chooses it",
        t.contains("custom=April"),
        &t,
    );
    open_combo()?;
    click(r#"statictext value="March""#)?;
    let t = title()?;
    check(
        "a click on an option chooses it",
        t.contains("custom=March"),
        &t,
    );
    open_combo()?;
    control.call("key", with(j!({ "keys": "Down" })))?;
    control.call("key", with(j!({ "keys": "Down" })))?;
    control.call("key", with(j!({ "keys": "space" })))?;
    sleep_ms(500);
    let t = title()?;
    check(
        "down, down, space in the open list chooses an option",
        !t.contains("custom=March"),
        &t,
    );

    // The radios, two ways.
    press(r#"radiobutton "Create your own""#)?;
    let t = title()?;
    check(
        "act_on_element press on a radio selects it",
        t.contains("radio=own"),
        &t,
    );
    click(r#"radiobutton "Suggested address""#)?;
    let t = title()?;
    check(
        "a click on a radio selects it",
        t.contains("radio=suggested"),
        &t,
    );
    Ok(())
}
