//! An MCP process from before the pipe keeps working against this app until
//! its client restarts (ADR 0020). `AgentScreen mcp --v1` serves the tools
//! in its own process and calls the app with control protocol v1, as every
//! `AgentScreen mcp` up to 0.1.38 did. It lists the tools, makes a display,
//! takes a screenshot of it and ends it, while a pipe's session works
//! beside it and can't reach that display.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e mcpv1
//!
//! Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{McpClient, check, instance, json, run, start_test_app, stop_test_app};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let old = McpClient::start_v1(&[("CLAUDE_CODE_SESSION_ID", "e2e-mcpv1-old")])?;
    let tools = old.request("tools/list", json!({}))?;
    let names: Vec<String> = tools
        .result
        .as_ref()
        .and_then(|r| r["tools"].as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    check(
        "the v1 process lists the tools from its own build",
        names.iter().any(|n| n == "create_display"),
        format!("{names:?}"),
    );
    let report = old.call("get_diagnostics", json!({}))?;
    check(
        "the v1 process reaches the app and serves the tools itself",
        report.get("app")["status"] == "available" && report.get("pipe")["status"] == "none",
        format!("app {} pipe {}", report.get("app"), report.get("pipe")),
    );

    let new = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", "e2e-mcpv1-new")])?;
    let report = new.call("get_diagnostics", json!({}))?;
    check(
        "a pipe beside it says protocol 2",
        report.get("pipe")["status"] == "collected"
            && report.get("pipe")["control_protocol_version"] == 2,
        report.get("pipe").to_string(),
    );

    let created = old.call("create_display", json!({ "width": 900, "height": 600 }))?;
    let id = created.display_id().context("display id")?;
    let shot = old.tool(
        "computer",
        json!({ "display_id": id, "action": "screenshot" }),
    )?;
    check(
        "the v1 process takes a screenshot of its display",
        !shot.is_error && shot.image().is_some_and(|png| png.len() > 1000),
        shot.text(),
    );
    let other = new.tool(
        "computer",
        json!({ "display_id": id, "action": "screenshot" }),
    )?;
    check(
        "the pipe's session can't reach the v1 session's display",
        other.is_error,
        other.text(),
    );
    let ended = old.call("end_display", json!({ "display_id": id }))?;
    check(
        "the v1 process ends its display",
        !ended.is_error,
        ended.text(),
    );
    old.close();
    new.close();
    Ok(())
}
