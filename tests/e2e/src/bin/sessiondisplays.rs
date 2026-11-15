//! One Agent Session with two Agent Displays (ADR 0016). A session used to
//! have at most one display; now `create_display` with `new: true` adds
//! another, and a call without it still returns the session's latest.
//!
//! The agent session, over MCP:
//!
//! - creates display A, and gets A again from a second create_display;
//! - creates display B with `new: true` and the same title, and gets a
//!   different handle and a different Display Label;
//! - gets B, its latest, from a create_display without `new`, with both
//!   handles listed;
//! - launches Field on A and moves its window to B with place_window, which
//!   asks the user nothing and keeps the window's origin;
//! - ends A with end_display, while B goes on;
//! - closes its connection, after which B has ended too. A second client
//!   of the same session, which joined by reusing B, sees it.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e sessiondisplays
//!
//! Makes exactly 2 displays. No real pointer or keyboard input, and
//! nothing on the user's screens moves.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, check, field_app, instance, json, out, pid_of, probe, run, sh_lenient,
    sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let tag = format!("--sessiondisplays-{}", std::process::id());
    run(|| {
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_DISCONNECT_GRACE_MS", "800"),
        ])?;
        field_app()?;
        let result = test(&tag);
        // Field goes before the app quits: a window left on a display the
        // quit removes would show on the user's screen until Field ends.
        sh_lenient("/usr/bin/pkill", &["-KILL", "-f", "--", &tag]);
        let field = format!("{}-field", tag.trim_start_matches('-'));
        for _ in 0..30 {
            if pid_of(&field).is_none() {
                break;
            }
            sleep_ms(100);
        }
        stop_test_app();
        result
    })
}

/// A display's bounds in global points.
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

fn test(tag: &str) -> Result<()> {
    let session = format!("sessiondisplays-{}", std::process::id());
    let env = [("CLAUDE_CODE_SESSION_ID", session.as_str())];
    let client = McpClient::start(&env)?;
    let create = |args: serde_json::Value| client.call("create_display", args);

    let a = create(json!({ "title": "Two screens" }))?
        .display_id()
        .context("create_display gave no display_id")?;
    let again = create(json!({ "title": "Two screens" }))?;
    check(
        "a second create_display without new returns the same display",
        again.display_id() == Some(a) && again.get("reused") == &json!(true),
        again.text(),
    );

    let made = create(json!({ "title": "Two screens", "new": true }))?;
    let b = made
        .display_id()
        .context("create_display with new gave no display_id")?;
    check(
        "new: true makes a second display",
        b != a && made.get("reused") == &json!(false),
        made.text(),
    );

    let latest = create(json!({ "title": "Two screens" }))?;
    let listed: Vec<(u64, String)> = latest
        .get("displays")
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|d| Some((d["display_id"].as_u64()?, d["label"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    check(
        "without new, the session gets its latest display and both handles",
        latest.display_id() == Some(b)
            && listed.iter().map(|(id, _)| *id).collect::<Vec<_>>() == vec![a, b],
        latest.text(),
    );
    check(
        "the two displays have different labels",
        listed.len() == 2 && listed[0].1 != listed[1].1,
        format!("{listed:?}"),
    );

    let launched = client.call(
        "launch_app",
        json!({
            "display_id": a, "app": out().join("Field.app"),
            "arguments": [format!("{tag}-field")]
        }),
    )?;
    let window = launched
        .window()
        .context("launch_app gave no window")?
        .window_id;
    // pgrep reads a pattern that starts with "--" as an option.
    let pid = pid_of(&format!("{}-field", tag.trim_start_matches('-'))).context("Field's pid")?;
    sleep_ms(800);
    check(
        "Field opens on display A",
        on(pid, window, a),
        launched.text(),
    );

    let moved = client.call(
        "place_window",
        json!({ "display_id": b, "window_id": window, "timeout_seconds": 5 }),
    )?;
    sleep_ms(800);
    check(
        "place_window moves the window from A to B without a Window Request",
        !moved.is_error
            && moved.get("moved_from") == &json!(a)
            && moved.get("handedOver").is_null()
            && on(pid, window, b),
        moved.text(),
    );
    let listed = client.call("list_windows", json!({ "display_id": b }))?;
    let origin = listed.get("windows").as_array().and_then(|list| {
        list.iter()
            .find(|w| w["windowId"].as_u64() == Some(window))
            .map(|w| w["origin"].clone())
    });
    check(
        "the window keeps its origin on B",
        origin == Some(json!("launched")),
        listed.text(),
    );
    let left = client.call("list_windows", json!({ "display_id": a }))?;
    check(
        "A no longer has the window",
        !left
            .windows()
            .unwrap_or_default()
            .iter()
            .any(|w| w.window_id == window)
            && !on(pid, window, a),
        left.text(),
    );

    client.call("end_display", json!({ "display_id": a }))?;
    let after = client.tool("list_windows", json!({ "display_id": a }))?;
    check(
        "end_display ends A",
        after.is_error && after.text().contains("ended"),
        after.text(),
    );
    let still = client.tool("list_windows", json!({ "display_id": b }))?;
    check(
        "B goes on after A ended",
        !still.is_error && on(pid, window, b),
        still.text(),
    );

    // Wait past `schedule_sweep` (600 ms) so empty display A returns to the
    // spare pool, then another session reuses A's parked spare (making no
    // virtual display) and gets a holder-aware error within 1 s (#189).
    let control = Control::connect()?;
    let _ = control.call("debugShow", json!({ "surface": "dismiss", "displayId": a }));
    sleep_ms(750);
    let other_session = format!("{session}-other");
    let other = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", other_session.as_str())])?;
    let c_reply = other.call("create_display", json!({ "title": "Other session" }))?;
    let c = c_reply
        .display_id()
        .context("other session create_display gave no display_id")?;
    let held_err = other.tool(
        "place_window",
        json!({ "display_id": c, "app": "Field", "timeout_seconds": 1 }),
    )?;
    check(
        "cross-session request error names the live holder session and connection state",
        held_err.is_error
            && held_err.text().contains("Two screens")
            && held_err.text().contains("[session ")
            && held_err.text().contains("connection open]"),
        held_err.text(),
    );
    other.call("end_display", json!({ "display_id": c }))?;
    other.close();

    // Closing the connection starts the disconnect grace period (#189); a
    // reconnecting client of the same session rebinds B on any tool call
    // before grace expires, and regains InputQueue ownership (H1).
    client.close();
    sleep_ms(150);
    let reconnected = McpClient::start(&env)?;
    let rebound_queue = reconnected.tool(
        "input_queue",
        json!({ "display_id": b, "action": "status" }),
    )?;
    check(
        "a reconnecting client within grace rebinds B on input_queue and owns its queue",
        !rebound_queue.is_error,
        rebound_queue.text(),
    );
    let rebound = reconnected.call("create_display", json!({ "title": "Two screens" }))?;
    check(
        "create_display after rebind returns B without creating a new display",
        rebound.display_id() == Some(b) && rebound.get("reused") == &json!(true),
        rebound.text(),
    );
    sleep_ms(1000);
    let kept = reconnected.tool("list_windows", json!({ "display_id": b }))?;
    check(
        "rebound display B stays live past the disconnect grace window",
        !kept.is_error && on(pid, window, b),
        kept.text(),
    );

    // An overlapping client of the same session keeps B alive and inherits its
    // InputQueue ownership when the earlier connection closes; B ends only
    // after the last connection closes and the grace window expires.
    let watcher = McpClient::start(&env)?;
    let joined = watcher.call("create_display", json!({ "title": "Two screens" }))?;
    check(
        "an overlapping client of the same session reuses B, making no display",
        joined.display_id() == Some(b) && joined.get("reused") == &json!(true),
        joined.text(),
    );
    reconnected.close();
    sleep_ms(1000);
    let fallback_queue = watcher.tool(
        "input_queue",
        json!({ "display_id": b, "action": "status" }),
    )?;
    check(
        "closing one overlapping connection migrates B and its InputQueue to the remaining connection",
        !fallback_queue.is_error && on(pid, window, b),
        fallback_queue.text(),
    );

    watcher.close();
    // Wait past the 800 ms disconnect grace, then query from a client of the
    // same session (`rebind_session` touches only displays still in grace, so
    // it cannot revive B and sees `"ended"`, `M2`).
    sleep_ms(1100);
    let verifier = McpClient::start(&env)?;
    let ended = verifier.tool("list_windows", json!({ "display_id": b }))?;
    verifier.close();
    check(
        "closing the session's last connection ends B after the grace period",
        ended.is_error && ended.text().contains("ended"),
        ended.text(),
    );

    // Once Field exits so B has no remaining windows in review, `sweep_ended`
    // sweeps B out of the active display list into `spares`.
    sh_lenient("/usr/bin/pkill", &["-KILL", "-f", "--", tag]);
    let field = format!("{}-field", tag.trim_start_matches('-'));
    for _ in 0..20 {
        if pid_of(&field).is_none() {
            break;
        }
        sleep_ms(50);
    }
    let mut swept = false;
    let mut sweep_msg = String::new();
    for _ in 0..20 {
        if let Err(err) = control.call("listWindows", json!({ "displayId": b })) {
            sweep_msg = err.to_string();
            if sweep_msg.contains("ended") && !sweep_msg.contains("belongs to another session") {
                swept = true;
                break;
            }
        }
        sleep_ms(150);
    }
    check(
        "ended display B is swept once its last window closes",
        swept,
        sweep_msg,
    );
    Ok(())
}
