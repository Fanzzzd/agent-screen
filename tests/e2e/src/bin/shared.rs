//! An app the user and an agent both use: keys go to the app's key window,
//! so the agent's typing must reach only its window, and none of it may
//! ever reach the user's. Opens one TextEdit window on the user's screen,
//! behind their front app, and closes it at the end.
//!
//! The user's window comes after a key press of theirs to TextEdit
//! (`debugUserKey`), as their cmd+n would: with nobody acting, a window
//! the agent's app makes is the agent's (ADR 0007, #46) and goes to the
//! display, and the agent's keys follow it there (#120).
//!
//! Before keys go out, the app's focused window must be the agent's, or
//! at least on an Agent Display. The focus record alone doesn't take key
//! focus from the user's window; a click on the agent's title bar or
//! AXMain does, with TextEdit still in the background. When neither can,
//! the call fails and sends nothing, which this script counts as a
//! failure too: on this Mac delivery can be made safe.
//!
//! Not covered: a window a shortcut opens. TextEdit in the background
//! keeps its first window key whatever focus record it gets, so keys after
//! cmd+n reach the earlier window until the agent clicks the new one; the
//! computer tool's description says so.
//!
//! Not covered either: handing key focus back to the user's window. The
//! focus record doesn't make it key again in the background, and a real
//! click would bring TextEdit to the front, taking the user's focus.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e shared

use std::process::{Command, ExitCode};

use agent_screen_tests::{
    Control, McpClient, allow_on_your_screen, check, cut, in_test_app, instance, json, now_ms,
    pid_of, quit_discarding, quoted, run, running, skip, sleep_ms, try_osa, wait_for_idle,
};
use anyhow::{Context, Result};

fn text_of(name: &str) -> Result<String> {
    try_osa(&format!(
        "tell application \"System Events\" to tell process \"TextEdit\" to get value of text area 1 of scroll area 1 of window \"{name}\""
    ))
}

fn main() -> ExitCode {
    if running("TextEdit") {
        skip("TextEdit is the user's");
    }
    if instance().is_some() && !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| in_test_app(test))
}

fn test() -> Result<()> {
    let session = format!("shared-{}", now_ms());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
    let display_id = client
        .tool("create_display", json!({ "title": "Shared" }))?
        .display_id()
        .context("create_display gave no display_id")?;
    let result = typing(&client, display_id);
    let _ = client.tool("end_display", json!({ "display_id": display_id }));
    if !quit_discarding("TextEdit") {
        // TextEdit started for this test only.
        let _ = Command::new("/usr/bin/pkill")
            .args(["-x", "TextEdit"])
            .status();
    }
    client.close();
    result
}

fn typing(client: &McpClient, display_id: u64) -> Result<()> {
    let type_text = |text: &str| {
        client.tool(
            "computer",
            json!({ "display_id": display_id, "action": "type", "text": text }),
        )
    };
    let launched = client.tool(
        "launch_app",
        json!({ "display_id": display_id, "app": "TextEdit" }),
    )?;
    let agent = launched.get("window")["title"]
        .as_str()
        .unwrap_or("")
        .to_string();
    check(
        "the agent has a TextEdit window",
        !agent.is_empty(),
        launched.structured_content.to_string(),
    );
    type_text("first ")?;
    // Long after the agent's last action, right after a key press of the
    // user's to TextEdit: the window stays theirs, on their screen.
    sleep_ms(3500);
    let pid = pid_of("TextEdit.app/Contents/MacOS/TextEdit").context("TextEdit's pid")?;
    Control::connect()?.call("debugUserKey", json!({ "pid": pid }))?;
    allow_on_your_screen("TextEdit", 120_000, "the user's own TextEdit window");
    try_osa("tell application \"TextEdit\" to make new document")?;
    sleep_ms(800);
    let user = try_osa(
        "tell application \"System Events\" to tell process \"TextEdit\" to get name of every window",
    )?
    .split(", ")
    .find(|n| *n != agent)
    .context("no window of the user's")?
    .to_string();

    let listed = client.tool("list_windows", json!({ "display_id": display_id }))?;
    check(
        "the user's window stays on their screen",
        !listed
            .windows()
            .unwrap_or_default()
            .iter()
            .any(|w| w.title == user),
        quoted(&user),
    );

    let typed = type_text("second")?;
    check(
        "the agent's typing succeeds",
        !typed.is_error,
        cut(typed.text(), 300),
    );
    let in_agent = text_of(&agent)?;
    check(
        "it lands in the agent's window",
        in_agent.to_lowercase() == "first second",
        quoted(&in_agent),
    );
    let in_user = text_of(&user)?;
    check(
        "none of it lands in the user's window",
        in_user.is_empty(),
        quoted(&in_user),
    );

    let pressed = client.tool(
        "computer",
        json!({ "display_id": display_id, "action": "key", "text": "return" }),
    )?;
    let typed = type_text("third")?;
    check(
        "the next key and typing succeed",
        !pressed.is_error && !typed.is_error,
        cut(&format!("{} {}", pressed.text(), typed.text()), 300),
    );
    let in_agent = text_of(&agent)?;
    check(
        "the next call reaches the agent's window again",
        in_agent.to_lowercase() == "first second\nthird",
        quoted(&in_agent),
    );
    let in_user = text_of(&user)?;
    check(
        "still nothing in the user's window",
        in_user.is_empty(),
        quoted(&in_user),
    );
    Ok(())
}
