//! A window the user handed over goes back to them when the agent ends its
//! display (#161). An agent ended a display with WeCom's window on it as a
//! Handover; the window stayed on the ended display, and the same session's
//! next place_window for WeCom failed, saying other agent sessions held the
//! window, though there were none.
//!
//! A stage session starts Field on a user stand-in stage and gives it up to
//! the user. The agent session, over MCP, takes it with the user's words,
//! launches Login itself, and calls end_display. The script checks:
//!
//! - Field is back on the stage at once, while Login keeps the ended
//!   display in review;
//! - the same session's next display takes Field again with the user's
//!   words;
//! - place_window for Login, which waits on the ended display, says the
//!   display waits for the user's review and blames no other session.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e endhandover
//!
//! Makes 3 displays: the stage, the ended display and the next one. No
//! real pointer or keyboard input, and nothing on the user's screens moves.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, check, field_app, instance, json, kill_ours, login_app, open_user_stage,
    our_binary, out, probe, run, sh_lenient, sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let tag = format!("--endhandover-{}", std::process::id());
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        field_app()?;
        let login = login_app()?;
        let result = test(&tag, login.to_str().context("Login's path")?);
        end_fixtures(&tag);
        stop_test_app();
        result
    })
}

/// End the Field started with `tag` and the Login the agent launched.
fn end_fixtures(tag: &str) {
    sh_lenient("/usr/bin/pkill", &["-TERM", "-f", "--", tag]);
    kill_ours(our_binary("Login"));
    sleep_ms(500);
}

/// The stage's bounds in global points.
fn bounds_of(display: u64) -> Result<[f64; 4]> {
    let b: Vec<f64> = probe(&["bounds", &display.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    anyhow::ensure!(b.len() == 4, "no bounds for display {display}");
    Ok([b[0], b[1], b[2], b[3]])
}

/// Whether window `window` of `pid` has its center on display `display`.
fn on(pid: u32, window: u64, display: u64) -> bool {
    let Ok([x, y, w, h]) = bounds_of(display) else {
        return false;
    };
    windows_of(pid).into_iter().any(|f| {
        let (cx, cy) = (f.x + f.width / 2.0, f.y + f.height / 2.0);
        f.id == window && cx >= x && cy >= y && cx < x + w && cy < y + h
    })
}

fn test(tag: &str, login: &str) -> Result<()> {
    let stage = Control::connect()?;
    let stage_id = open_user_stage(&stage)?;
    let session = format!("endhandover-{}", std::process::id());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
    let first = client
        .call(
            "create_display",
            json!({ "title": "End handover", "width": 1280, "height": 800 }),
        )?
        .display_id()
        .context("create_display gave no display_id")?;

    // The user's Field window on the stage: the stage session launches it,
    // so AeroSpace leaves it there, then gives it to the user.
    let field = stage.call(
        "launchApp",
        json!({
            "displayId": stage_id, "app": out().join("Field.app"),
            "arguments": [format!("{tag}-user")],
            "environment": { "FIELD_DISPLAY": stage_id.to_string() }
        }),
    )?;
    let pid = field["pid"].as_u64().context("Field's pid")? as u32;
    let window = field["window"]["windowId"]
        .as_u64()
        .context("Field's window")?;
    sleep_ms(1500);
    stage.call(
        "debugUserWindow",
        json!({ "displayId": stage_id, "windowId": window }),
    )?;
    anyhow::ensure!(
        on(pid, window, stage_id),
        "the user's Field window isn't on the stage"
    );

    let placed = client.tool(
        "place_window",
        json!({
            "display_id": first, "app": "Field",
            "user_instruction": "Take my Field window.", "timeout_seconds": 5
        }),
    )?;
    sleep_ms(800);
    check(
        "the user's words hand Field over",
        !placed.is_error && on(pid, window, first),
        placed.text(),
    );
    let launched = client.tool("launch_app", json!({ "display_id": first, "app": login }))?;
    check(
        "the agent's own Login opens on the display",
        !launched.is_error,
        launched.text(),
    );

    client.call("end_display", json!({ "display_id": first }))?;
    let mut home = false;
    for _ in 0..30 {
        if on(pid, window, stage_id) {
            home = true;
            break;
        }
        sleep_ms(200);
    }
    check(
        "end_display gives the Handover back to the user stand-in stage",
        home,
        format!(
            "Field's window: {:?}",
            windows_of(pid).into_iter().find(|f| f.id == window)
        ),
    );

    let second = client
        .call(
            "create_display",
            json!({ "title": "End handover again", "width": 1280, "height": 800 }),
        )?
        .display_id()
        .context("the second create_display gave no display_id")?;
    let again = client.tool(
        "place_window",
        json!({
            "display_id": second, "app": "Field",
            "user_instruction": "Take my Field window again.", "timeout_seconds": 5
        }),
    )?;
    sleep_ms(800);
    check(
        "the same session's next display takes Field again",
        !again.is_error && on(pid, window, second),
        again.text(),
    );
    let review = client.tool(
        "place_window",
        json!({ "display_id": second, "app": "Login", "timeout_seconds": 5 }),
    )?;
    check(
        "Login on the ended display is in review, and no other session is blamed",
        review.is_error
            && review.text().contains("review")
            && !review.text().contains("other agent session"),
        review.text(),
    );

    end_fixtures(tag);
    let _ = client.call("end_display", json!({ "display_id": second }));
    client.close();
    let _ = stage.call("endDisplay", json!({ "displayId": stage_id }));
    Ok(())
}
