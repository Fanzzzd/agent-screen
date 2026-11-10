//! The user's front app, whose focused window is a Handover on the agent's
//! display, takes the agent's input (#162). WeCom's only window was handed
//! over and WeCom was the user's front app: every click and key was refused
//! as reaching "the user's window", which was that Handover.
//!
//! A stage session starts Login on a user stand-in stage and gives both its
//! windows, "login" and "other", to the user. The agent session, over MCP,
//! takes "login" with the user's words. A click of the stage session's
//! makes "other" Login's focused window, on the stage, as the user's click
//! would. Login is then the user's front app (`debugFront`, as when they had
//! just used it). The script checks:
//!
//! - with "other" focused on the stage, a double click on Login's user
//!   field is refused and names "other", as before (#110);
//! - once "other" is handed over too, the same double click and a `type`
//!   reach the login window: its title shows the user field focused and
//!   the text.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e handoverfront
//!
//! Makes 2 displays: the stage and the agent's. No real pointer or keyboard
//! input; the front app is only noted, not changed.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, check, instance, json, kill_ours, login_app, open_user_stage, our_binary,
    run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let login = login_app()?;
        let result = test(login.to_str().context("Login's path")?);
        let _ = Control::connect().and_then(|c| c.call("debugFront", json!({})));
        kill_ours(our_binary("Login"));
        sleep_ms(500);
        stop_test_app();
        result
    })
}

/// Login's window whose title starts with `title` on the stage, by id.
fn stage_window(stage: &Control, stage_id: u64, title: &str) -> Result<u64> {
    let listed = stage.call("listWindows", json!({ "displayId": stage_id }))?;
    listed["windows"]
        .as_array()
        .and_then(|ws| {
            ws.iter()
                .find(|w| w["title"].as_str().is_some_and(|t| t.starts_with(title)))
        })
        .and_then(|w| w["windowId"].as_u64())
        .with_context(|| format!("no {title:?} window on the stage: {listed}"))
}

/// The window on the agent's display whose title starts with `title`.
fn window(client: &McpClient, id: u64, title: &str) -> Result<agent_screen_tests::Window> {
    client
        .call("list_windows", json!({ "display_id": id }))?
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.title.starts_with(title))
        .with_context(|| format!("no {title:?} window on the display"))
}

fn test(login: &str) -> Result<()> {
    let stage = Control::connect()?;
    let stage_id = open_user_stage(&stage)?;
    let session = format!("handoverfront-{}", std::process::id());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
    let id = client
        .call(
            "create_display",
            json!({ "title": "Handover front", "width": 1280, "height": 800 }),
        )?
        .display_id()
        .context("create_display gave no display_id")?;

    let launched = stage.call("launchApp", json!({ "displayId": stage_id, "app": login }))?;
    let pid = launched["pid"].as_u64().context("Login's pid")?;
    sleep_ms(1500);
    let login_id = stage_window(&stage, stage_id, "login")?;
    let other_id = stage_window(&stage, stage_id, "other")?;
    for w in [login_id, other_id] {
        stage.call(
            "debugUserWindow",
            json!({ "displayId": stage_id, "windowId": w }),
        )?;
    }
    let take = |window: u64, words: &str| -> Result<agent_screen_tests::ToolResult> {
        client.tool(
            "place_window",
            json!({
                "display_id": id, "window_id": window,
                "user_instruction": words, "timeout_seconds": 5
            }),
        )
    };
    let placed = take(login_id, "Take my Login window.")?;
    anyhow::ensure!(!placed.is_error, "handing over login: {}", placed.text());
    sleep_ms(800);
    // The user clicks "other" on their screen: it becomes Login's focused
    // window there.
    let listed = stage.call("listWindows", json!({ "displayId": stage_id }))?;
    let other = listed["windows"]
        .as_array()
        .and_then(|ws| ws.iter().find(|w| w["windowId"].as_u64() == Some(other_id)))
        .cloned()
        .context("other isn't on the stage")?;
    let f = |k: &str| other[k].as_f64().unwrap_or(0.0);
    stage.call(
        "click",
        json!({
            "displayId": stage_id,
            "x": f("x") + f("width") / 2.0, "y": f("y") + f("height") / 2.0
        }),
    )?;
    sleep_ms(500);
    stage.call("debugFront", json!({ "pid": pid }))?;

    let login_window = window(&client, id, "login")?;
    let field = [login_window.x + 180.0, login_window.y + 62.0];
    let double = || -> Result<agent_screen_tests::ToolResult> {
        client.tool(
            "computer",
            json!({ "display_id": id, "action": "double_click", "coordinate": field }),
        )
    };
    let refused = double()?;
    check(
        "with the user's window focused on their screen, the click is refused (#110)",
        refused.is_error
            && refused.text().contains("user's front app")
            && refused.text().contains("other"),
        refused.text(),
    );

    let again = take(other_id, "Take the other Login window too.")?;
    anyhow::ensure!(!again.is_error, "taking other: {}", again.text());
    sleep_ms(800);
    // Clear of the login window, so the click lands on login, not on it.
    let moved = client.tool(
        "place_window",
        json!({ "display_id": id, "window_id": other_id, "x": 900, "y": 100 }),
    )?;
    anyhow::ensure!(!moved.is_error, "moving other: {}", moved.text());
    sleep_ms(500);
    stage.call("debugFront", json!({ "pid": pid }))?;
    let clicked = double()?;
    sleep_ms(500);
    let title = window(&client, id, "login")?.title;
    check(
        "with the focused window a Handover on the display, the double click goes in",
        !clicked.is_error && title.contains("focus=user"),
        format!("{} / {title}", clicked.text()),
    );
    let typed = client.tool(
        "computer",
        json!({ "display_id": id, "action": "type", "text": "abc" }),
    )?;
    sleep_ms(600);
    let title = window(&client, id, "login")?.title;
    check(
        "and typing reaches the login window's user field",
        !typed.is_error && title.contains("text=abc|"),
        format!("{} / {title}", typed.text()),
    );

    stage.call("debugFront", json!({}))?;
    kill_ours(our_binary("Login"));
    sleep_ms(500);
    let _ = client.call("end_display", json!({ "display_id": id }));
    client.close();
    let _ = stage.call("endDisplay", json!({ "displayId": stage_id }));
    Ok(())
}
