//! An agent that names a display that ended is told why (#326), and the
//! end leaves one line in displays.log and an entry in get_diagnostics.
//!
//! A client that sends no session ID (not Claude Code, no Codex `_meta`)
//! is one session per `AgentScreen mcp` process:
//!
//! - it creates a display and ends it with end_display; the next call on
//!   it, and one after the sweep removed the empty display, says this
//!   session ended it with end_display;
//! - it creates a display again, which reuses the spare, and closes its
//!   connection, as a client that restarts its MCP server does. Past the
//!   Disconnect Grace a new process of the same client names the display
//!   and hears that the session that made it disconnected, and that a
//!   restarted MCP server was its own earlier session;
//! - displays.log has an `ended display=N reason=…` line for each end, and
//!   get_diagnostics lists both ends.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e endedreason
//!
//! Makes 1 display; the second create_display reuses it as a spare. No
//! windows, no real pointer or keyboard input, nothing on the user's
//! screens moves.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, check, instance, json, run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_DISCONNECT_GRACE_MS", "800"),
        ])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn log() -> String {
    std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Library/Logs/Agent Screen/displays-e2e.log"),
    )
    .unwrap_or_default()
}

/// A client with no session hint, as omp and most clients are.
fn client() -> Result<McpClient> {
    McpClient::start(&[("E2E_CLIENT_NAME", "endedreason")])
}

fn test() -> Result<()> {
    let from = log().len();
    let first = client()?;
    let a = first
        .call("create_display", json!({ "title": "Ended reason" }))?
        .display_id()
        .context("create_display gave no display_id")?;
    first.call("end_display", json!({ "display_id": a }))?;
    let own = first.tool("list_windows", json!({ "display_id": a }))?;
    check(
        "after end_display the session hears it ended the display itself",
        own.is_error
            && own
                .text()
                .contains("this session ended it with end_display"),
        own.text(),
    );
    // Past `schedule_sweep` (600 ms): the empty display is gone.
    sleep_ms(1000);
    let swept = first.tool("list_windows", json!({ "display_id": a }))?;
    check(
        "the reason stays once the empty display was swept",
        swept.is_error
            && swept
                .text()
                .contains("this session ended it with end_display")
            && !swept.text().contains("does not exist"),
        swept.text(),
    );

    let b = first
        .call("create_display", json!({ "title": "Ended reason" }))?
        .display_id()
        .context("the second create_display gave no display_id")?;
    // The client restarts its MCP server: the old process goes, and the
    // grace (800 ms here) and the sweep pass before the new one calls.
    first.close();
    sleep_ms(2000);
    let second = client()?;
    let restarted = second.tool("list_windows", json!({ "display_id": b }))?;
    check(
        "a restarted MCP server hears its earlier session disconnected",
        restarted.is_error
            && restarted
                .text()
                .contains("the connection of the session that made it closed")
            && restarted
                .text()
                .contains("If your client restarted its MCP server"),
        restarted.text(),
    );

    let lines = log().get(from..).unwrap_or_default().to_string();
    let ended: Vec<&str> = lines
        .lines()
        .filter(|l| l.contains(" ended display="))
        .collect();
    check(
        "displays.log has one line per end with its reason and session",
        ended.iter().any(|l| {
            l.contains(&format!(
                "ended display={a} reason=end_display session=pipe:"
            ))
        }) && ended.iter().any(|l| {
            l.contains(&format!(
                "ended display={b} reason=disconnected session=pipe:"
            ))
        }) && ended.len() == 2,
        ended.join("\n"),
    );

    let diagnostics = second.call("get_diagnostics", json!({}))?;
    let ends = diagnostics.get("app")["ended_displays"].clone();
    let listed = |id: u64, reason: &str| {
        ends.as_array().is_some_and(|list| {
            list.iter()
                .any(|e| e["display"].as_u64() == Some(id) && e["reason"] == reason)
        })
    };
    check(
        "get_diagnostics lists both ends with their reasons",
        listed(a, "end_display") && listed(b, "disconnected"),
        ends.to_string(),
    );
    second.close();
    Ok(())
}
