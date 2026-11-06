//! Native AX subtree reads reach a message pane after a long sibling list (#134).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e subtreestate
//!
//! One display and the owned Electron fixture; no private apps or real input.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, ToolResult, allow_on_your_screen, check, fixture, instance, json, kill_ours,
    listening, out, run, running, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no owned Electron.app fixture");
    }
    run(|| {
        if listening() {
            skip("the test instance is already running");
        }
        if running("Electron") {
            skip("Electron is already running and may belong to another task");
        }
        allow_on_your_screen(
            "Electron",
            800,
            "the cold-start fixture may show before its first accessibility response",
        );
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&electron);
        kill_ours(fixture("electron"));
        sleep_ms(500);
        stop_test_app();
        result
    })
}

fn numbered(state: &ToolResult, label: &str) -> Option<u64> {
    Regex::new(&format!(
        r#"(?m)^\s*\[(\d+)\] \S+ \"{}\"(?: |$)"#,
        regex::escape(label)
    ))
    .ok()?
    .captures(state.first_text())?[1]
        .parse()
        .ok()
}

fn test(electron: &std::path::Path) -> Result<()> {
    let client = McpClient::start(&[])?;
    let id = client
        .call("create_display", json!({ "width": 1280, "height": 800 }))?
        .display_id()
        .context("display id")?;
    client.call(
        "launch_app",
        json!({
            "display_id": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": {
                "HARNESS_SUBTREE": "1", "HARNESS_BACKGROUND": "1",
                "HARNESS_DISPLAY": id.to_string(), "HARNESS_SIZE": "900x600"
            }
        }),
    )?;
    sleep_ms(700);
    let window = client
        .call("list_windows", json!({ "display_id": id }))?
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.app == "Electron")
        .context("no fixture window")?
        .window_id;
    let at = |extra: Value| {
        let mut args = json!({ "display_id": id, "window_id": window });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };
    // The first AX read requests Chromium's full accessibility tree.
    client.tool("get_window_state", at(json!({})))?;
    sleep_ms(300);
    let full = client.tool("get_window_state", at(json!({})))?;
    if let Some(image) = full.image() {
        std::fs::write(out().join("subtree-full.png"), image)?;
    }
    let first = numbered(&full, "Conversation 0").context("first conversation element")?;
    check(
        "the native default read exhausts its budget before the later pane",
        full.first_text().contains("truncated") && numbered(&full, "Message pane").is_none(),
        format!(
            "{} bytes, pane {:?}",
            full.first_text().len(),
            numbered(&full, "Message pane")
        ),
    );
    let mut found = None;
    for depth in 0..=8 {
        let shallow = client.tool("get_window_state", at(json!({ "max_depth": depth })))?;
        if let Some(root) = numbered(&shallow, "Message pane") {
            found = Some((root, depth));
            break;
        }
    }
    check(
        "a shallow native read discovers the later message pane",
        found.is_some(),
        format!("root and depth {found:?}"),
    );
    let (root, depth) = found.context("message pane was not discovered")?;
    let pane = client.tool("get_window_state", at(json!({ "root": root })))?;
    let card = numbered(&pane, "Forwarded record").context("record button in subtree")?;
    check(
        "a rooted native read reaches the card without reading the conversation list",
        !pane.is_error && !pane.first_text().contains("Conversation 0"),
        pane.first_text(),
    );
    let pressed = client.tool(
        "act_on_element",
        at(json!({ "element": card, "action": "press", "root": root, "result": "changes" })),
    )?;
    check(
        "a scoped action reports the card's changed state in the same pane",
        !pressed.is_error
            && pressed.first_text().contains("Opened local record")
            && !pressed.first_text().contains("Conversation 0"),
        pressed.first_text(),
    );
    let outside = client.tool(
        "act_on_element",
        at(json!({ "element": first, "action": "press", "result": "none" })),
    )?;
    check(
        "a partial read preserves the previously numbered element outside its subtree",
        !outside.is_error && outside.first_text() == "Done.",
        outside.first_text(),
    );
    let again = client.tool("get_window_state", at(json!({ "max_depth": depth })))?;
    check(
        "the discovered pane keeps its number across shallow and subtree reads",
        numbered(&again, "Message pane") == Some(root),
        format!(
            "before {root}, after {:?}",
            numbered(&again, "Message pane")
        ),
    );
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
