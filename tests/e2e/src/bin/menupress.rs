//! act_on_element press on an Electron app's menu item (issue #39). The
//! fixture's Go > Previous view has no shortcut and writes
//! `event.triggeredByAccelerator` from its click handler. AppKit runs an
//! accessibility press with no current event, and Electron reads anything
//! but a mouse event as a shortcut, so the press reaches the item as one;
//! apps that drop shortcuts while a text field has focus ignore it. The
//! press answers with a note that says so. Pressed with the page's field
//! unfocused, then focused after typing in it.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e menupress

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, check, fixture, instance, json, out, run, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "menupress";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    run(|| {
        allow_on_your_screen(
            "Electron",
            800,
            "Electron brings itself to the front as its first window shows and answers no accessibility call for about 0.5 s after, so nothing can move the window sooner (as in `electron`)",
        );
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let mut pid = None;
        let result = test(&electron, &mut pid);
        // Only the instance this script started: another script's may run.
        if let Some(pid) = pid {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.to_string())
                .status();
        }
        stop_test_app();
        result
    })
}

/// An element's number in a window state's tree.
fn number(tree: &str, pattern: &str) -> Result<i64> {
    let found = Regex::new(&format!(r"\[(\d+)\] {pattern}"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern} in\n{tree}"))?;
    Ok(found[1].parse()?)
}

fn test(electron: &std::path::Path, pid: &mut Option<i64>) -> Result<()> {
    let control = Control::connect()?;
    let log = out().join("menupress.log");
    let _ = std::fs::remove_file(&log);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Menu press", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].clone();
    let launched = control.call(
        "launchApp",
        json!({
            "sessionKey": KEY,
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_MENU_LOG": log.to_str() },
        }),
    )?;
    *pid = launched["pid"].as_i64();
    let window = launched["window"]["windowId"].clone();
    let at = |params: Value| {
        let mut params = params;
        params["sessionKey"] = json!(KEY);
        params["displayId"] = id.clone();
        params["windowId"] = window.clone();
        params
    };
    let state = || -> Result<String> {
        Ok(control.call("windowState", at(json!({})))?["tree"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    let press = |label: &str| -> Result<(String, String)> {
        let go = number(&state()?, r#"menubaritem "Go""#)?;
        let menu = control.call(
            "elementAction",
            at(json!({ "element": go, "action": "press" })),
        )?;
        let menu = menu["menu"].as_str().unwrap_or_default().to_string();
        let item = number(&menu, r#"menuitem "Previous view""#)?;
        let before = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count();
        let pressed = control.call(
            "elementAction",
            at(json!({ "element": item, "action": "press" })),
        )?;
        let note = pressed["note"].as_str().unwrap_or_default().to_string();
        for _ in 0..20 {
            let lines: Vec<String> = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(String::from)
                .collect();
            if lines.len() > before {
                return Ok((lines[before].clone(), note));
            }
            sleep_ms(100);
        }
        Ok((format!("no click reached the item ({label})"), note))
    };
    for focused in [false, true] {
        if focused {
            let field = number(&state()?, "textfield")?;
            control.call(
                "elementAction",
                at(json!({ "element": field, "action": "focus" })),
            )?;
            control.call(
                "type",
                json!({ "sessionKey": KEY, "displayId": id, "text": "abc" }),
            )?;
            sleep_ms(300);
        }
        let label = if focused {
            "with the field focused"
        } else {
            "with no field focused"
        };
        let (seen, note) = press(label)?;
        check(
            &format!("a press on the menu item reaches it, {label}"),
            seen.starts_with("previous accelerator="),
            &seen,
        );
        check(
            &format!("the press says Electron takes it for a shortcut, {label}"),
            note.contains("triggeredByAccelerator") && note.contains("text field"),
            format!("{seen}; note {note:?}"),
        );
    }
    let tree = state()?;
    check(
        "the field kept what was typed",
        tree.contains(r#"textfield value="abc""#),
        tree.lines()
            .find(|l| l.contains("textfield"))
            .unwrap_or_default(),
    );
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    Ok(())
}
