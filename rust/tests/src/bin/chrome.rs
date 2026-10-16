//! Chrome on an Agent Display, on a throwaway profile, while another app
//! starts on another display: a click on a page's text field focuses it,
//! typed text lands in it, and a click on a button runs the page's script.
//! An app starting makes the system's front app change for a moment, which
//! takes key status from Chrome's window; unless Sidescreen makes it key
//! again, Chrome ignores the clicks in its page.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e chrome
//!
//! Skips when Chrome or Firefox is running with windows. Quits the copies it
//! started, on throwaway profiles.

use std::process::ExitCode;

use anyhow::{Context, Result};
use regex::Regex;
use sidescreen_tests::{
    Control, check, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};

const KEY: &str = "chrome";

fn main() -> ExitCode {
    if running_headed("Google Chrome") || running_headed("Firefox") {
        skip("Chrome or Firefox is running; quit it to test");
    }
    run(|| {
        start_test_app(&[])?;
        let result = test();
        quit_profile(&out().join("chrome-profile"));
        quit_profile(&out().join("chrome-firefox-profile"));
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

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("chrome-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/signup.html").display());
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Chrome", "width": 1100, "height": 700 }),
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

    // Another agent starts Firefox on its own display, over its own
    // connection, as each agent's session is its connection's.
    let other = Control::connect()?;
    let elsewhere = other.call(
        "createDisplay",
        json!({ "sessionKey": "other", "title": "Other", "width": 1100, "height": 700 }),
    )?;
    let firefox = out().join("chrome-firefox-profile");
    let _ = std::fs::remove_dir_all(&firefox);
    std::fs::create_dir_all(&firefox)?;
    other.call(
        "launchApp",
        json!({ "sessionKey": "other", "displayId": elsewhere["displayId"], "app": "Firefox", "arguments": [
            "-no-remote", "-profile", firefox.display().to_string(), "about:blank",
        ] }),
    )?;
    sleep_ms(1500);

    let state = || -> Result<String> {
        let state = control.call(
            "windowState",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
        )?;
        Ok(state["tree"].as_str().unwrap_or_default().to_string())
    };
    let tree = state()?;
    let name = center(&tree, r#"textfield "Name""#)?;
    let submit = center(&tree, r#"button "Create account""#)?;
    let act = |method: &str, mut params: serde_json::Value| -> Result<()> {
        params["sessionKey"] = json!(KEY);
        params["displayId"] = json!(id);
        control.call(method, params)?;
        Ok(())
    };
    act("click", json!({ "x": name.0, "y": name.1 }))?;
    act("type", json!({ "text": "Ada" }))?;
    sleep_ms(500);
    let typed = state()?
        .lines()
        .find(|l| l.contains(r#"textfield "Name""#))
        .unwrap_or_default()
        .to_string();
    check(
        "typed text lands in the field",
        typed.contains("Ada"),
        &typed,
    );
    act("click", json!({ "x": submit.0, "y": submit.1 }))?;
    sleep_ms(500);
    check(
        "a click on the button runs the page's script",
        state()?.contains("Enter a valid email address."),
        "",
    );
    Ok(())
}
