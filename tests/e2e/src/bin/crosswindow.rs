//! One agent session asks for a window another session holds (ADR 0017).
//! place_window used to refuse it, and an agent that needed WeChat from
//! another session's display could only message that session to let go.
//! Now the user decides on a Window Request that names the holder.
//!
//! Two sessions over MCP, each with one display:
//!
//! - session A launches Field on its display;
//! - session B calls place_window for Field's window. The script answers
//!   the card's Hand Over through `debugAnswerWindow`, as its button does.
//!   The window moves to B's display as a Placement, the reply names A's
//!   display, and A's next list_windows and a call naming the window say
//!   the user gave Field to another agent session;
//! - session A asks for the window back, and the script answers Keep: A's
//!   call fails naming B, and the window stays on B's display.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e crosswindow
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
    let tag = format!("--crosswindow-{}", std::process::id());
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
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

/// Answer display `display`'s Window Request once it shows, as the card's
/// buttons do.
fn answer_soon(display: u64, give: bool) -> std::thread::JoinHandle<Result<serde_json::Value>> {
    std::thread::spawn(move || {
        sleep_ms(800);
        Control::connect()?.call(
            "debugAnswerWindow",
            json!({ "displayId": display, "give": give }),
        )
    })
}

fn test(tag: &str) -> Result<()> {
    let run = std::process::id();
    let (key_a, key_b) = (
        format!("crosswindow-a-{run}"),
        format!("crosswindow-b-{run}"),
    );
    let holder = McpClient::start(&[
        ("CLAUDE_CODE_SESSION_ID", key_a.as_str()),
        ("E2E_CLIENT_NAME", "Claude Code"),
    ])?;
    let asker = McpClient::start(&[
        ("CLAUDE_CODE_SESSION_ID", key_b.as_str()),
        ("E2E_CLIENT_NAME", "Codex"),
    ])?;
    let a = holder
        .call("create_display", json!({ "title": "Holder" }))?
        .display_id()
        .context("A's display")?;
    let b = asker
        .call("create_display", json!({ "title": "Asker" }))?
        .display_id()
        .context("B's display")?;
    check(
        "the two sessions have two displays",
        a != b,
        format!("{a} {b}"),
    );

    let launched = holder.call(
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
        "Field opens on A's display",
        on(pid, window, a),
        launched.text(),
    );

    // Hand Over: the window moves to B as a Placement.
    let answer = answer_soon(b, true);
    let placed = asker.tool(
        "place_window",
        json!({ "display_id": b, "window_id": window, "timeout_seconds": 20 }),
    )?;
    let answered = answer.join().expect("answer thread");
    println!("     answer: {answered:?}");
    check(
        "the user's Hand Over answers B's request for A's window",
        answered
            .as_ref()
            .is_ok_and(|a| a["answered"] == json!(true)),
        format!("{answered:?}"),
    );
    sleep_ms(800);
    check(
        "the window moves to B's display, and the reply names A's display",
        !placed.is_error
            && placed
                .get("taken_from")
                .as_str()
                .is_some_and(|l| l.contains("Holder"))
            && on(pid, window, b),
        placed.text(),
    );
    let listed = asker.call("list_windows", json!({ "display_id": b }))?;
    let origin = listed.get("windows").as_array().and_then(|list| {
        list.iter()
            .find(|w| w["windowId"].as_u64() == Some(window))
            .map(|w| w["origin"].clone())
    });
    check(
        "on B it is a Placement",
        origin == Some(json!("placed")),
        listed.text(),
    );

    // A is told: once in its next list_windows, and whenever it names the
    // window.
    let told = holder.call("list_windows", json!({ "display_id": a }))?;
    check(
        "A's next list_windows says the user gave Field to another session",
        told.text()
            .contains("The user gave Field to another agent session")
            && !told
                .windows()
                .unwrap_or_default()
                .iter()
                .any(|w| w.window_id == window),
        told.text(),
    );
    let named = holder.tool(
        "get_window_state",
        json!({ "display_id": a, "window_id": window }),
    )?;
    check(
        "a call naming the window tells A the user gave it away",
        named.is_error
            && named
                .text()
                .contains("the user gave Field to another agent session"),
        named.text(),
    );

    // Keep: A asks for it back, and the user keeps it with B.
    let answer = answer_soon(a, false);
    let back = holder.tool(
        "place_window",
        json!({ "display_id": a, "window_id": window, "timeout_seconds": 20 }),
    )?;
    let answered = answer.join().expect("answer thread");
    println!("     answer: {answered:?}");
    check(
        "Keep declines A's request and names B",
        back.is_error && back.text().contains("kept") && back.text().contains("Asker"),
        back.text(),
    );
    sleep_ms(500);
    check(
        "the window stays on B's display",
        on(pid, window, b) && !on(pid, window, a),
        format!("window {window}"),
    );

    sh_lenient("/usr/bin/pkill", &["-KILL", "-f", "--", tag]);
    sleep_ms(300);
    asker.call("end_display", json!({ "display_id": b }))?;
    holder.call("end_display", json!({ "display_id": a }))?;
    asker.close();
    holder.close();
    Ok(())
}
