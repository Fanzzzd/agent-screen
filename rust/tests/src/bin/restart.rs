//! The app aborts on a panic while an agent works (issue #43, after the
//! abort of #40): the log keeps the panic's message, its displays go with
//! it and macOS moves their windows to the user's screen. The agent's next
//! call naming the old display says the app quit, and after create_display
//! the windows of apps it started there are strays it can place_window back.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e restart
//!
//! Makes 2 displays. Field's window shows on your screen between the quit
//! and the place_window, a few seconds.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, McpClient, check, cut, instance, json, listening, out, pid_of, run,
    running, sh_lenient, sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let field = out().join("Field.app");
    if !field.exists() {
        eprintln!("no Field.app; run `cargo e2e ime` once to make it");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&field);
        let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
        stop_test_app();
        result
    })
}

fn test(field: &std::path::Path) -> Result<()> {
    let client = McpClient::start(&[])?;
    let first = client.call("create_display", json!({ "width": 900, "height": 600 }))?;
    let old = first.display_id().context("display id")?;
    let launched = client.call(
        "launch_app",
        json!({ "display_id": old, "app": field.to_str().unwrap() }),
    )?;
    let window = launched.window().context("Field's window")?;

    // The abort: no chance to end anything.
    let log = log_text();
    pid_of(DEBUG_BINARY).context("no test app")?;
    Control::connect()?.send("debugPanic", json!({}))?;
    // The process lingers while macOS writes its crash report; its
    // socket closes at once.
    for _ in 0..50 {
        if !listening() {
            break;
        }
        sleep_ms(100);
    }
    check("the panic stops the app", !listening(), "");
    let after = log_text();
    let added = after.get(log.len()..).unwrap_or(&after);
    check(
        "the log keeps the panic's message and place",
        added.contains("panic: ") && added.contains("debugPanic") && added.contains("agent.rs"),
        cut(added, 300),
    );
    sleep_ms(1000);
    if let Some(field) = pid_of("Field.app/Contents/MacOS/Field") {
        println!("  Field's windows: {:?}", windows_of(field));
    }
    start_test_app(&[("AGENTSCREEN_LANG", "en")])?;

    let stale = client.tool(
        "computer",
        json!({ "display_id": old, "action": "screenshot" }),
    )?;
    check(
        "a call naming the old display says the app quit",
        stale.is_error && stale.text().contains("quit"),
        stale.text(),
    );

    // An agent calls again seconds later, when AeroSpace has filed the
    // window in the user's workspace: on the next display it lays the
    // window out there again, wherever macOS puts it.
    let filed = || {
        sh_lenient(
            "/usr/bin/env",
            &[
                "aerospace",
                "list-windows",
                "--all",
                "--format",
                "%{window-id}",
            ],
        )
        .lines()
        .any(|l| l.trim() == window.window_id.to_string())
    };
    if running("AeroSpace") {
        for _ in 0..50 {
            if filed() {
                break;
            }
            sleep_ms(100);
        }
        println!("     AeroSpace files Field's window: {}", filed());
    }
    let second = client.call("create_display", json!({ "width": 900, "height": 600 }))?;
    let id = second.display_id().context("display id")?;
    let on_display = || -> Result<bool> {
        let on = client.call("list_windows", json!({ "display_id": id }))?;
        Ok(on
            .windows()
            .unwrap_or_default()
            .iter()
            .any(|w| w.window_id == window.window_id))
    };
    // macOS puts a window back by itself when the new display takes the
    // old one's serial and place, as it often does here; otherwise it is
    // a stray the agent places.
    let strays = client.call("list_windows", json!({ "display_id": id, "strays": true }))?;
    let listed = strays
        .windows()
        .unwrap_or_default()
        .iter()
        .any(|w| w.window_id == window.window_id);
    println!("     stray: {listed}, {}", strays.text());
    if listed {
        let placed = client.call(
            "place_window",
            json!({ "display_id": id, "window_id": window.window_id }),
        )?;
        println!("     place_window: {}", placed.text());
    }
    sleep_ms(1500);
    check(
        "Field's window is back on the new display, as a stray placed or put back by macOS",
        on_display()?,
        format!("stray {listed}"),
    );
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}

fn log_text() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let name = format!("displays-{}.log", instance().unwrap_or_default());
    std::fs::read_to_string(format!("{home}/Library/Logs/Agent Screen/{name}")).unwrap_or_default()
}
