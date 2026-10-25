//! The `key` action in Chrome on an Agent Display, on a throwaway profile:
//! a page's keydown listener sees `]`, `}`, `cmd+[` and `cmd+]` as those
//! characters, whatever layout the Mac types with, and the bracket lands in
//! the focused text field.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e keys
//!
//! Skips when Chrome is running with windows. Quits the copy it started.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;

const KEY: &str = "keys";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[])?;
        let result = test();
        quit_profile(&out().join("keys-profile"));
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("keys-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/keys.html").display());
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Keys", "width": 900, "height": 600 }),
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
    sleep_ms(1500);

    let tree = || -> Result<String> {
        let state = control.call(
            "windowState",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
        )?;
        Ok(state["tree"].as_str().unwrap_or_default().to_string())
    };
    let field = Regex::new(r#"textfield "Field"[^\n]* at \((\d+),(\d+)\)"#)?
        .captures(&tree()?)
        .context("no text field")?
        .iter()
        .skip(1)
        .map(|m| m.map_or(0.0, |m| m.as_str().parse().unwrap_or(0.0)))
        .collect::<Vec<f64>>();
    control.call(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": field[0], "y": field[1] }),
    )?;
    for chord in ["]", "shift+]", "cmd+[", "cmd+]", "bracketleft"] {
        control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": chord }),
        )?;
        sleep_ms(150);
    }
    sleep_ms(500);

    let tree = tree()?;
    let seen = tree
        .lines()
        .find(|l| l.contains("Seen:"))
        .unwrap_or_default()
        .to_string();
    check(
        "the page sees each chord's character",
        seen.contains("Seen: ] } cmd+[ cmd+] ["),
        &seen,
    );
    let typed = tree
        .lines()
        .find(|l| l.contains(r#"textfield "Field""#))
        .unwrap_or_default()
        .to_string();
    check(
        "the brackets land in the field",
        typed.contains("]}["),
        &typed,
    );
    Ok(())
}
