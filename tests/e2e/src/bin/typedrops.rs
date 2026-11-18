//! Typing into address fields in Chrome, on a throwaway profile (issue
//! #50): a page busy with suggestions for 40 ms at every key, and a
//! controlled field that writes its value back, both get every character,
//! and the call waits for the busy one to show them. A page that drops
//! keys coming faster than it handles them makes the type action fail and
//! say what the field holds, and typing a character per call fills it.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e typedrops
//!
//! Skips when Chrome is running with windows. Quits the copy it started.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "typedrops";
const STREET: &str = "1234 Wonderland Avenue Apartment 56 North";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_profile(&out().join("typedrops-profile"));
        stop_test_app();
        result
    })
}

/// A field's line in a window state's tree.
fn line<'a>(tree: &'a str, label: &str) -> &'a str {
    tree.lines()
        .find(|l| l.contains(&format!(r#"textfield "{label}""#)))
        .unwrap_or_default()
}

fn center(line: &str) -> Result<(f64, f64)> {
    let found = Regex::new(r" at \((\d+),(\d+)\)")?
        .captures(line)
        .with_context(|| format!("no position in {line}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("typedrops-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/autocomplete.html").display());
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Type drops", "width": 1100, "height": 700 }),
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
    // The page's scripts, focus handling included, run once it has loaded.
    sleep_ms(1500);
    let call = |method: &str, mut params: Value| {
        params["sessionKey"] = json!(KEY);
        params["displayId"] = json!(id);
        control.call(method, params)
    };
    let tree = || -> Result<String> {
        Ok(call("windowState", json!({ "windowId": window }))?["tree"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    // A busy page takes a while to show the last keys: up to 3 s.
    let shown = |label: &str| -> Result<String> {
        let mut field = String::new();
        for _ in 0..10 {
            field = line(&tree()?, label).to_string();
            if field.contains(&format!(r#"value="{STREET}""#)) {
                break;
            }
            sleep_ms(300);
        }
        Ok(field)
    };
    let first = tree()?;
    for label in ["Street busy", "Street controlled"] {
        let at = center(line(&first, label))?;
        call("click", json!({ "x": at.0, "y": at.1 }))?;
        let typed = call("type", json!({ "text": STREET }));
        let field = shown(label)?;
        check(
            &format!("{label}: every character lands and the call succeeds"),
            typed.is_ok() && field.contains(&format!(r#"value="{STREET}""#)),
            format!("{typed:?}; {field}"),
        );
    }
    let at = center(line(&first, "Street lossy"))?;
    call("click", json!({ "x": at.0, "y": at.1 }))?;
    let typed = call("type", json!({ "text": STREET }));
    let error = typed
        .as_ref()
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    let field = shown("Street lossy")?;
    check(
        "keys the page dropped make the call fail and say what the field holds",
        error.contains("without some of them"),
        format!("{typed:?}; {field}"),
    );
    call("click", json!({ "x": at.0, "y": at.1 }))?;
    call("key", json!({ "keys": "cmd+a" }))?;
    call("key", json!({ "keys": "Delete" }))?;
    for c in STREET.chars() {
        call("type", json!({ "text": c.to_string() }))?;
    }
    let field = shown("Street lossy")?;
    check(
        "a character per call fills it",
        field.contains(&format!(r#"value="{STREET}""#)),
        field,
    );
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    Ok(())
}
