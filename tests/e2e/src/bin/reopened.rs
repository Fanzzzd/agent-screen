//! A window an app the agent started makes on its own, while all its other
//! windows are on the agent's display, opens there too (#46). UTM closed a
//! VM window and opened a new one each time the guest rebooted, minutes
//! after the agent's last action, and each opened on the user's screen.
//! Field here does the same 33 s after it starts, past the 30 s a cold
//! start counts as the agent's.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e reopened
//!
//! Makes 1 display and takes 40 s. Before the fix, Field's new window shows
//! on your screen.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, check, field_app, instance, json, run, sh_lenient, sleep_ms, start_test_app,
    stop_test_app,
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
        // Field's new window may show on the user's screen.
        let result = test(field.to_str().unwrap());
        let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
        stop_test_app();
        result
    })
}

fn test(field: &str) -> Result<()> {
    let client = McpClient::start(&[])?;
    let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    let launched = client.call(
        "launch_app",
        json!({ "display_id": id, "app": field, "environment": { "FIELD_REOPEN": "33000" } }),
    )?;
    let first = launched.window().context("Field's window")?.window_id;
    // No action meanwhile: the new window comes long after the last one.
    sleep_ms(36_000);
    let listed = client.call("list_windows", json!({ "display_id": id }))?;
    let windows = listed.windows().unwrap_or_default();
    let again = windows.iter().find(|w| w.title == "Field again");
    check(
        "the window Field opened by itself is on the display",
        again.is_some_and(|w| w.window_id != first),
        listed.text(),
    );
    let strays = client.call("list_windows", json!({ "display_id": id, "strays": true }))?;
    check(
        "and isn't a stray on the user's screen",
        !strays
            .windows()
            .unwrap_or_default()
            .iter()
            .any(|w| w.title == "Field again"),
        strays.text(),
    );
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
