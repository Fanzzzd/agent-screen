//! place_window answers placed only for a window that is on the display,
//! and a window of the agent's that left its display is a stray it can
//! place again (#47, #19). UTM put a VM window back on the user's screen
//! after place_window had answered placed, and the agent could not place it
//! again. Field here moves its window to a user stand-in stage, as an app
//! resizing its window past a small display does: once when told, then
//! each time 300 ms after it lands on another screen.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e placeheld
//!
//! Makes 2 displays. Field moves between the target and the user stand-in stage.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, check, field_app, instance, json, open_user_stage, pid_of, run, sh_lenient,
    sign_step, sleep_ms, start_test_app, stop_test_app, windows_of,
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
        // Field moves to the user stand-in stage.
        let mut process = FieldProcess::default();
        let result = test(field.to_str().unwrap(), &mut process);
        drop(process);
        stop_test_app();
        result
    })
}

/// The one Field instance this run started. Never signal another checkout's
/// Field, or one left by an earlier script in this checkout.
#[derive(Default)]
struct FieldProcess {
    pid: Option<u32>,
}

impl FieldProcess {
    fn signal(&self, name: &str) {
        if let Some(pid) = self.pid {
            sh_lenient("/bin/kill", &[name, &pid.to_string()]);
        }
    }

    fn whereabouts(&self) -> String {
        self.pid
            .map(|pid| format!("{:?}", windows_of(pid)))
            .unwrap_or_default()
    }
}

impl Drop for FieldProcess {
    fn drop(&mut self) {
        self.signal("-TERM");
    }
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

fn launch(
    client: &McpClient,
    id: u64,
    stage: u64,
    field: &str,
    process: &mut FieldProcess,
) -> Result<u64> {
    // AppKit ignores this argument. It identifies only this launch even if
    // another script has Field running from the same checkout.
    let marker = format!("--placeheld-{}", std::process::id());
    let launched = client.call(
        "launch_app",
        json!({ "display_id": id, "app": field, "arguments": [marker], "environment": { "FIELD_DISPLAY": id.to_string(), "FIELD_MOVE_DISPLAY": stage.to_string(), "FIELD_LIFETIME_MS": "60000" } }),
    );
    let executable = format!("{field}/Contents/MacOS/Field");
    process.pid = pid_of(&format!("^{} {}( |$)", regex::escape(&executable), marker));
    process
        .pid
        .context("the Field instance this script started")?;
    Ok(launched?.window().context("Field's window")?.window_id)
}

fn test(field: &str, process: &mut FieldProcess) -> Result<()> {
    sign_step("opening the user stand-in stage / 打开模拟用户显示器");
    let stage = Control::connect()?;
    let stage_id = open_user_stage(&stage)?;
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;

    // Once, 5 s after launch, when the launch's watch has ended.
    let window = launch(&client, id, stage_id, field, process)?;
    sleep_ms(5000);
    sign_step("moving Field to the stage / 将 Field 移到模拟用户显示器");
    process.signal("-USR1");
    sleep_ms(1000);
    check(
        "the launched window that left the display is listed as a stray",
        !on_display(&client, id, window)? && stray(&client, id, window)?,
        format!("window {window} at {}", process.whereabouts()),
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

    sign_step("Field refuses placement / Field 持续移回模拟用户显示器");
    // Each time, 300 ms after it lands: place_window can't keep it there.
    process.signal("-USR1");
    process.signal("-USR2");
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
            process.whereabouts()
        ),
    );
    check(
        "the window is still a stray to place, not a request to the user",
        stray(&client, id, window)?,
        format!("window {window}"),
    );
    process.signal("-TERM");
    process.pid = None;
    sleep_ms(300);
    client.call("end_display", json!({ "display_id": id }))?;
    stage.call("endDisplay", json!({ "displayId": stage_id }))?;
    client.close();
    Ok(())
}
