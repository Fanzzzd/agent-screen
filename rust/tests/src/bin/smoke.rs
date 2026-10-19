//! End-to-end smoke test: acts as an MCP client over stdio against the real
//! menu bar app. Run with the app open:
//!
//!     cargo e2e smoke [app]
//!
//! Saves each screenshot to /tmp/sidescreen-smoke-N.jpg.

use std::process::ExitCode;

use anyhow::{Context, Result};
use regex::Regex;
use agent_screen_tests::{McpClient, ToolResult, fail, json, now_ms, run, sleep_ms};

/// "ok" or "FAIL"; a FAIL counts toward the exit code.
fn verdict(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        fail();
        "FAIL"
    }
}

/// The number of the button with this label.
fn element(state: &ToolResult, label: &str) -> Option<u64> {
    Regex::new(&format!(r#"\[(\d+)\] button "{label}""#))
        .ok()?
        .captures(state.first_text())?[1]
        .parse()
        .ok()
}

/// The number shown in Calculator's edit field.
fn shown(state: &ToolResult) -> Option<String> {
    Regex::new(r#""Edit field".*\n.*value="\W*(\d+)""#)
        .ok()?
        .captures(state.first_text())
        .map(|c| c[1].to_string())
}

fn main() -> ExitCode {
    let app = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Calculator".into());
    run(|| test(&app))
}

fn test(app: &str) -> Result<()> {
    let session = format!("smoke-{}", now_ms());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;

    let tools = client.request("tools/list", json!({}))?;
    let names: Vec<&str> = tools
        .result
        .as_ref()
        .and_then(|r| r["tools"].as_array())
        .context("tools/list gave no tools")?
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or(""))
        .collect();
    println!("tools: {}", names.join(", "));

    let created = client.call("create_display", json!({ "title": "Smoke test" }))?;
    let display_id = created.get("display_id").clone();
    client.call("create_display", json!({}))?; // must reuse
    sleep_ms(1500);
    client.call(
        "launch_app",
        json!({ "display_id": display_id, "app": app }),
    )?;
    client.call(
        "computer",
        json!({ "display_id": display_id, "action": "type", "text": "7+8=" }),
    )?;

    // Per-window tools: 2 × 3 through accessibility presses.
    let launched = client
        .call("list_windows", json!({ "display_id": display_id }))?
        .windows()
        .unwrap_or_default()
        .into_iter()
        .next()
        .context("list_windows gave no window")?;
    let window_id = launched.window_id;
    // An element keeps its number while it stays in the window.
    let mut state = client.call(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": window_id }),
    )?;
    let press = |state: &ToolResult, label: &str| {
        client.call(
            "act_on_element",
            json!({
                "display_id": display_id,
                "window_id": window_id,
                "element": element(state, label),
                "action": "press",
            }),
        )
    };
    // Calculator remembers its sidebar; close it if an earlier run left it open.
    if launched.width > 300.0 {
        state = press(&state, "Hide Sidebar")?;
    }
    let before = state.clone();
    for label in ["All Clear", "2", "Multiply", "3", "Equals"] {
        state = press(&state, label)?;
    }
    let result = shown(&state);
    println!(
        "act_on_element 2 × 3 → {} {}",
        result.as_deref().unwrap_or("none"),
        verdict(result.as_deref() == Some("6")),
    );
    let two = element(&before, "2");
    let now = element(&state, "2");
    println!(
        "button 2 keeps its number: {} → {} {}",
        json!(two),
        json!(now),
        verdict(two.is_some() && two == now),
    );

    // The tree's centers are where the computer tool clicks.
    let center = Regex::new(r#"button "5"[^\n]* at \((\d+),(\d+)\)"#)?
        .captures(state.first_text())
        .and_then(|c| Some([c[1].parse::<u64>().ok()?, c[2].parse::<u64>().ok()?]));
    press(&state, "All Clear")?;
    let center = center.context("no center for button 5")?;
    client.call(
        "computer",
        json!({ "display_id": display_id, "action": "left_click", "coordinate": center }),
    )?;
    let clicked = shown(&client.call(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": window_id }),
    )?);
    println!(
        "clicking the tree's center of 5 → {} {}",
        clicked.as_deref().unwrap_or("none"),
        verdict(clicked.as_deref() == Some("5")),
    );
    client.call(
        "set_disposition",
        json!({ "display_id": display_id, "window_id": window_id, "disposition": "keep" }),
    )?;
    client.call("list_windows", json!({ "display_id": display_id }))?;

    client.call(
        "computer",
        json!({ "display_id": display_id, "action": "key", "text": "cmd+nope" }),
    )?;
    client.call(
        "computer",
        json!({ "display_id": display_id, "action": "left_click", "coordinate": [5000, 10] }),
    )?;
    client.call("end_display", json!({ "display_id": display_id }))?;
    client.call(
        "computer",
        json!({ "display_id": display_id, "action": "screenshot" }),
    )?;

    client.close();
    Ok(())
}
