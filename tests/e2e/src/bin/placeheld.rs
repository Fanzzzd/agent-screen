//! place_window answers placed only for a window that is on the display,
//! and a window of the agent's that left its display is a stray it can
//! place again (#47, #19). UTM put a VM window back on the user's screen
//! after place_window had answered placed, and the agent could not place it
//! again. Field here moves its window to the main screen, as an app
//! resizing its window past a small display does: once when told, then
//! each time 300 ms after it lands on another screen.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e placeheld
//!
//! Makes 1 display. Field's window shows on your screen for moments.

use std::process::ExitCode;

use agent_screen_tests::{
    HandsOff, McpClient, check, field_app, instance, json, pid_of, run, sh_lenient, sleep_ms,
    start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let field = field_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        // Field's window moves to the user's screen.
        let _sign = HandsOff::start("placeheld")?;
        let result = test(field.to_str().unwrap());
        quit_field();
        stop_test_app();
        result
    })
}

fn quit_field() {
    let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
    sleep_ms(500);
}

/// Where Field's windows are, for the report.
fn whereabouts() -> String {
    pid_of("Field.app/Contents/MacOS/Field")
        .map(|pid| format!("{:?}", windows_of(pid)))
        .unwrap_or_default()
}

fn on_display(client: &McpClient, id: u64, window: u64) -> Result<bool> {
    Ok(client
        .call("list_windows", json!({ "display_id": id }))?
        .windows()
        .unwrap_or_default()
        .iter()
        .any(|w| w.window_id == window))
}

fn stray(client: &McpClient, id: u64, window: u64) -> Result<bool> {
    Ok(client
        .call("list_windows", json!({ "display_id": id, "strays": true }))?
        .windows()
        .unwrap_or_default()
        .iter()
        .any(|w| w.window_id == window))
}

fn signal(name: &str) {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &[name, "-f", "Field.app/Contents/MacOS/Field"],
    );
}

fn launch(client: &McpClient, id: u64, field: &str) -> Result<u64> {
    quit_field();
    let launched = client.call("launch_app", json!({ "display_id": id, "app": field }))?;
    Ok(launched.window().context("Field's window")?.window_id)
}

fn test(field: &str) -> Result<()> {
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;

    // Once, 5 s after launch, when the launch's watch has ended.
    let window = launch(&client, id, field)?;
    sleep_ms(5000);
    signal("-USR1");
    sleep_ms(1000);
    check(
        "the launched window that left the display is listed as a stray",
        !on_display(&client, id, window)? && stray(&client, id, window)?,
        format!("window {window} at {}", whereabouts()),
    );
    let placed = client.tool(
        "place_window",
        json!({ "display_id": id, "window_id": window, "timeout_seconds": 1 }),
    )?;
    sleep_ms(1000);
    check(
        "place_window places it again, and it is there",
        !placed.is_error && on_display(&client, id, window)?,
        placed.text(),
    );

    // Each time, 300 ms after it lands: place_window can't keep it there.
    signal("-USR1");
    signal("-USR2");
    sleep_ms(1000);
    let placed = client.tool(
        "place_window",
        json!({ "display_id": id, "window_id": window, "timeout_seconds": 1 }),
    )?;
    sleep_ms(500);
    let there = on_display(&client, id, window)?;
    check(
        "place_window says so when the window doesn't stay on the display",
        placed.is_error && placed.text().contains("could not") && !there,
        format!(
            "{} (on display: {there}) at {}",
            placed.text(),
            whereabouts()
        ),
    );
    check(
        "the window is still a stray to place, not a request to the user",
        stray(&client, id, window)?,
        format!("window {window}"),
    );
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
