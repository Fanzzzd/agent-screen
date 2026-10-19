//! A file Open panel on an Agent Display. AppKit draws the panel in a
//! separate process (the Open and Save Panel Service); the app owns only an
//! empty window in front of it. The agent's keys, clicks and window state
//! must reach the panel. Opens Preview on the agent's display and quits it.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e openpanel

use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use agent_screen_tests::{
    McpClient, Value, Window, check, cut, in_test_app, instance, json, now_ms, quit_app, run,
    running, sh, skip, sleep_ms, wait_for_idle,
};

fn main() -> ExitCode {
    if running("Preview") {
        skip("Preview is the user's");
    }
    if instance().is_some() && !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| in_test_app(test))
}

fn test() -> Result<()> {
    let session = format!("openpanel-{}", now_ms());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
    sh(
        "/bin/cp",
        &["/tmp/openpanel.png", "/tmp/openpanel-pick.png"],
    )?;
    let display_id = client
        .tool("create_display", json!({ "title": "Open panel" }))?
        .display_id()
        .context("create_display gave no display_id")?;
    let result = panel(&client, display_id);
    let _ = client.tool("end_display", json!({ "display_id": display_id }));
    if !quit_app("Preview") {
        let _ = Command::new("/usr/bin/pkill")
            .args(["-x", "Preview"])
            .status();
    }
    client.close();
    result
}

fn panel(client: &McpClient, display_id: u64) -> Result<()> {
    let computer = |mut args: Value| {
        args["display_id"] = json!(display_id);
        client.tool("computer", args)
    };
    let windows = || -> Result<Vec<Window>> {
        Ok(client
            .tool("list_windows", json!({ "display_id": display_id }))?
            .windows()
            .unwrap_or_default())
    };
    let panel =
        || -> Result<Option<Window>> { Ok(windows()?.into_iter().find(|w| w.title == "Open")) };
    let wait_for = |want: bool| -> Result<bool> {
        for _ in 0..30 {
            if panel()?.is_some() == want {
                return Ok(true);
            }
            sleep_ms(200);
        }
        Ok(false)
    };

    let launched = client.tool(
        "launch_app",
        json!({ "display_id": display_id, "app": "Preview", "open": "/tmp/openpanel.png" }),
    )?;
    check(
        "Preview opens a window",
        !launched.get("window").is_null(),
        cut(&launched.structured_content.to_string(), 200),
    );
    computer(json!({ "action": "key", "text": "cmd+o" }))?;
    check("cmd+o shows the Open panel", wait_for(true)?, "");
    let open = panel()?;

    // With no panel, no window_id at all.
    let mut args = json!({ "display_id": display_id });
    if let Some(open) = &open {
        args["window_id"] = json!(open.window_id);
    }
    let state = client.tool("get_window_state", args)?;
    check(
        "its window state reads",
        !state.is_error,
        cut(state.text(), 200),
    );
    check(
        "and shows its buttons",
        serde_json::to_string(&state)?.contains("Cancel"),
        cut(state.text(), 200),
    );

    computer(json!({ "action": "key", "text": "escape" }))?;
    check("escape closes it", wait_for(false)?, "");

    computer(json!({ "action": "key", "text": "cmd+o" }))?;
    check("cmd+o shows it again", wait_for(true)?, "");
    let again = panel()?.context("no Open panel")?;
    // Cancel sits at the bottom right of the panel.
    let shot = computer(json!({ "action": "screenshot" }))?;
    let scale = shot.get("scale").as_f64().unwrap_or(1.0);
    let at = [
        ((again.x + again.width - 140.0) * scale).round() as i64,
        ((again.y + again.height - 25.0) * scale).round() as i64,
    ];
    let clicked = computer(json!({ "action": "left_click", "coordinate": at }))?;
    check(
        "a click on Cancel succeeds",
        !clicked.is_error,
        cut(clicked.text(), 160),
    );
    check("and closes it", wait_for(false)?, "");

    // Pick a file by path, as an agent uploading one would.
    computer(json!({ "action": "key", "text": "cmd+o" }))?;
    check("cmd+o shows it once more", wait_for(true)?, "");
    let snap = |name: &str| -> Result<()> {
        let image = computer(json!({ "action": "screenshot" }))?.image();
        let wanted = std::env::var("SNAP").is_ok_and(|v| !v.is_empty());
        if let Some(image) = image.filter(|i| wanted && !i.is_empty()) {
            std::fs::write(format!("/tmp/op-{name}.png"), image)?;
        }
        Ok(())
    };
    computer(json!({ "action": "key", "text": "cmd+shift+g" }))?;
    sleep_ms(600);
    snap("1-goto")?;
    computer(json!({ "action": "type", "text": "/tmp/openpanel-pick.png" }))?;
    sleep_ms(400);
    snap("2-typed")?;
    computer(json!({ "action": "key", "text": "return" }))?;
    sleep_ms(600);
    snap("3-return")?;
    computer(json!({ "action": "key", "text": "return" }))?;
    let mut picked = false;
    for _ in 0..30 {
        picked = windows()?
            .iter()
            .any(|w| w.title.starts_with("openpanel-pick"));
        if picked {
            break;
        }
        sleep_ms(200);
    }
    check("go to folder, a path and return open the file", picked, "");
    Ok(())
}
