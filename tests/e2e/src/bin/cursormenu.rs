//! A menu item pressed through accessibility in Cursor, a VS Code fork
//! (#39). VS Code runs a menu item Electron reports as a shortcut as its
//! key binding in whatever has focus, not as the item's command: with the
//! terminal focused, Edit > Find opens the terminal's find box, not the
//! editor's. Agent Screen can't change that (AppKit runs the press with no
//! current event), so the press answers with a note that says so and
//! points to the Command Palette. Cursor starts on a
//! throwaway profile and extensions folder with
//! tests/e2e/fixtures/workspace/notes.txt open.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e cursormenu
//!
//! Skips when Cursor runs. 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, check, fixture, json, out, quit_profile, run, running, skip,
    sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "cursormenu";

fn main() -> ExitCode {
    if running("Cursor") {
        skip("Cursor is running; quit it to test");
    }
    run(|| {
        allow_on_your_screen(
            "Cursor",
            800,
            "Electron brings itself to the front as its first window shows and answers no accessibility call for about 0.5 s after (as in `electron`)",
        );
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_profile(&out().join("cursor-profile"));
        stop_test_app();
        result
    })
}

/// An element's number in a tree.
fn number(tree: &str, pattern: &str) -> Result<i64> {
    let found = Regex::new(&format!(r"\[(\d+)\] {pattern}"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern} in\n{tree}"))?;
    Ok(found[1].parse()?)
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("cursor-profile");
    let extensions = out().join("cursor-extensions");
    let _ = std::fs::remove_dir_all(&profile);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Cursor", "width": 1440, "height": 900 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Cursor", "arguments": [
            format!("--user-data-dir={}", profile.display()),
            format!("--extensions-dir={}", extensions.display()),
            "--skip-welcome", "--disable-workspace-trust",
            fixture("workspace").to_str(),
            fixture("workspace/notes.txt").to_str(),
        ] }),
    )?;
    let window = launched["window"]["windowId"].clone();
    sleep_ms(8000);
    let with = |mut v: Value| {
        v["sessionKey"] = json!(KEY);
        v["displayId"] = json!(id);
        v["windowId"] = window.clone();
        v
    };
    let tree = || -> Result<String> {
        Ok(control.call("windowState", with(json!({})))?["tree"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    // Menu > item, both through accessibility, as act_on_element does.
    let choose = |menu: &str, item: &str| -> Result<String> {
        let bar = number(&tree()?, &format!(r#"menubaritem "{menu}""#))?;
        let listed = control.call(
            "elementAction",
            with(json!({ "element": bar, "action": "press" })),
        )?;
        let listed = listed["menu"].as_str().unwrap_or_default().to_string();
        let n = number(&listed, &format!(r#"menuitem "{item}"#))?;
        let pressed = control.call(
            "elementAction",
            with(json!({ "element": n, "action": "press" })),
        )?;
        Ok(pressed["note"].as_str().unwrap_or_default().to_string())
    };
    choose("Terminal", "New Terminal")?;
    sleep_ms(3000);
    let before = tree()?;
    std::fs::write(out().join("cursormenu-terminal.txt"), &before)?;
    let note = choose("Edit", "Find")?;
    sleep_ms(1500);
    let after = tree()?;
    std::fs::write(out().join("cursormenu-find.txt"), &after)?;
    // Find boxes that came with the press, by height: the editor's is in
    // the top half of the window, the terminal panel's in the bottom.
    let field = Regex::new(r#"textfield "Find"[^\n]* at \((\d+),(\d+)\)"#)?;
    let new: Vec<(i64, i64)> = field
        .captures_iter(&after)
        .map(|c| (c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0)))
        .filter(|p| !before.contains(&format!("at ({},{})", p.0, p.1)))
        .collect();
    println!("     find boxes that appeared: {new:?}");
    let height = launched["window"]["height"].as_f64().unwrap_or(754.0);
    let top = launched["window"]["y"].as_f64().unwrap_or(0.0);
    check(
        "Edit > Find pressed with the terminal focused runs as ⌘F there: the terminal's find box opens",
        new.iter().any(|&(_, y)| (y as f64) >= top + height / 2.0),
        format!("{new:?}; window from y {top}, {height} high"),
    );
    check(
        "the press says the app ran the item as its shortcut and names the Command Palette",
        note.contains("built on VS Code") && note.contains("Command Palette"),
        &note,
    );
    Ok(())
}
