//! wait_for_change returns when a window changes, with the change in
//! act_on_element's + / ~ / - form, or times out (#163). The Watch fixture
//! changes itself seconds after a button press, which the script sends
//! through act_on_element as an agent would: no pointer, no keyboard.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e waitchange
//!
//! One display; about 30 s.

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    McpClient, ToolResult, check, instance, json, kill_ours, out, run, running, skip, sleep_ms,
    start_test_app, stop_test_app, watch_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        if running("Watch") {
            skip("a Watch app is already running");
        }
        let app = watch_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&app);
        kill_ours(out().join("Watch.app"));
        sleep_ms(300);
        stop_test_app();
        result
    })
}

/// The number of the element whose line matches `pattern` after its role.
fn numbered(state: &ToolResult, pattern: &str) -> Option<u64> {
    Regex::new(&format!(r"(?m)^\s*\[(\d+)\] {pattern}"))
        .ok()?
        .captures(state.first_text())?[1]
        .parse()
        .ok()
}

/// Whether a line of `text` starts with `sign`, then a number, and holds
/// `holds`.
fn has_line(text: &str, sign: char, holds: &str) -> bool {
    text.lines()
        .any(|l| l.starts_with(&format!("{sign} [")) && l.contains(holds))
}

fn test(app: &std::path::Path) -> Result<()> {
    let client = McpClient::start(&[])?;
    let id = client
        .call("create_display", json!({ "width": 1280, "height": 800 }))?
        .display_id()
        .context("display id")?;
    client.call(
        "launch_app",
        json!({ "display_id": id, "app": app.to_str() }),
    )?;
    sleep_ms(700);
    let window = client
        .call("list_windows", json!({ "display_id": id }))?
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.app == "Watch")
        .context("no Watch window")?
        .window_id;
    let at = |extra: Value| {
        let mut args = json!({ "display_id": id, "window_id": window });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };
    let wait = |extra: Value| -> Result<(String, f64)> {
        let started = Instant::now();
        let result = client.tool("wait_for_change", at(extra))?;
        let took = started.elapsed().as_secs_f64();
        let text = result.first_text().to_string();
        println!("  wait_for_change in {took:.1} s: {text}");
        Ok((text, took))
    };
    let state = client.tool("get_window_state", at(json!({})))?;
    let button = |name: &str| {
        numbered(&state, &format!("button \"{name}\"")).with_context(|| format!("no {name} button"))
    };
    let (send, tick, clear, close) = (
        button("Send")?,
        button("Tick")?,
        button("Clear")?,
        button("Close")?,
    );
    let messages = numbered(&state, "group \"Messages\"").context("no Messages group")?;
    let press = |element: u64, result: &str| {
        client.tool(
            "act_on_element",
            at(json!({ "element": element, "action": "press", "result": result })),
        )
    };

    // Nothing moves: the wait ends at its timeout and says nothing changed.
    let (text, took) = wait(json!({ "timeout_seconds": 2 }))?;
    check(
        "an idle window times out after timeout_seconds",
        text.starts_with("timed_out")
            && text.contains("Nothing changed")
            && (1.8..5.0).contains(&took),
        format!("{took:.1} s: {text}"),
    );

    // Send shows "Sending…" at once and "Delivered" 3 s later.
    let pressed = press(send, "changes")?;
    check(
        "act_on_element's changes show the immediate Sending…",
        pressed.first_text().contains("Sending"),
        pressed.first_text(),
    );
    let (text, took) = wait(json!({
        "appears": { "role": "statictext", "text": "delivered" },
        "timeout_seconds": 10
    }))?;
    check(
        "an element appearing seconds later fires appeared",
        text.starts_with("appeared after") && (0.5..6.0).contains(&took),
        format!("{took:.1} s: {text}"),
    );
    check(
        "the change lists + Delivered, - Sending… and ~ the status",
        has_line(&text, '+', "Delivered")
            && has_line(&text, '-', "Sending")
            && has_line(&text, '~', "Status: sent"),
        &text,
    );

    // Tick changes Count, outside Messages: a wait on the Messages subtree
    // sees nothing, and a wait on the whole window then fires at once,
    // since Count changed after the window's last tree.
    press(tick, "none")?;
    let (text, took) = wait(json!({ "root": messages, "timeout_seconds": 3 }))?;
    check(
        "a subtree wait ignores a change outside its root",
        text.starts_with("timed_out") && text.contains("since the call started") && took >= 2.8,
        format!("{took:.1} s: {text}"),
    );
    let (text, took) = wait(json!({ "timeout_seconds": 5 }))?;
    check(
        "a change that landed before the call fires at once against the last tree",
        text.starts_with("changed after")
            && text.contains("since its last tree you got")
            && has_line(&text, '~', "Count 1")
            && took < 2.0,
        format!("{took:.1} s: {text}"),
    );

    // Clear removes Delivered 1.5 s later.
    press(clear, "none")?;
    let (text, took) =
        wait(json!({ "disappears": { "text": "Delivered" }, "timeout_seconds": 10 }))?;
    check(
        "an element going away fires disappeared with its old line",
        text.starts_with("disappeared after")
            && has_line(&text, '-', "Delivered")
            && (0.8..5.0).contains(&took),
        format!("{took:.1} s: {text}"),
    );

    // A condition that never holds, with a short timeout.
    let (text, _) = wait(json!({ "appears": { "text": "never shown" }, "timeout_seconds": 1.5 }))?;
    check(
        "an unmet condition times out",
        text.starts_with("timed_out") && text.contains("no condition fired"),
        &text,
    );

    // Close closes the window 1.5 s later, during the wait.
    press(close, "none")?;
    let (text, took) =
        wait(json!({ "appears": { "text": "never shown" }, "timeout_seconds": 10 }))?;
    check(
        "the window closing ends the wait and says so",
        text.starts_with("window_closed") && took < 6.0,
        format!("{took:.1} s: {text}"),
    );
    client.call("end_display", json!({ "display_id": id }))?;
    Ok(())
}
