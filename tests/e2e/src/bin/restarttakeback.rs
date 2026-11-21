//! An app the user took back stays theirs when Agent Screen restarts
//! (#285, ADR 0016). The agent launches Field on its display, the user
//! takes Field's window back, and the app aborts. After the restart, the
//! session's next display must not adopt Field: its window is no stray
//! there, so place_window can't take it without a Window Request.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e restarttakeback
//!
//! Makes 2 displays. Field's window shows on your screen from the Take
//! Back to the end, a few seconds.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, allow_on_your_screen, check, debug_pid, field_app, instance, json,
    kill_ours, listening, our_binary, run, sleep_ms, start_test_app, stop_test_app,
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
        let result = test(&field);
        kill_ours(our_binary("Field"));
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

    allow_on_your_screen(
        "Field",
        u64::MAX,
        "the user takes Field's window back, then the app aborts on purpose",
    );
    let taken = Control::connect()?.call(
        "debugTakeBack",
        json!({ "displayId": old, "windowId": window.window_id }),
    )?;
    check(
        "Take Back moves Field's window to the user's screen",
        taken["left"].as_bool() == Some(true),
        taken.to_string(),
    );

    // The abort: no chance to end anything.
    debug_pid().context("no test app")?;
    Control::connect()?.send("debugPanic", json!({}))?;
    for _ in 0..50 {
        if !listening() {
            break;
        }
        sleep_ms(100);
    }
    check("the panic stops the app", !listening(), "");
    sleep_ms(1000);
    start_test_app(&[("AGENTSCREEN_LANG", "en")])?;

    let second = client.call("create_display", json!({ "width": 900, "height": 600 }))?;
    let id = second.display_id().context("display id")?;
    let strays = client.call("list_windows", json!({ "display_id": id, "strays": true }))?;
    let listed = strays
        .windows()
        .unwrap_or_default()
        .iter()
        .any(|w| w.window_id == window.window_id);
    check(
        "after the restart, the window the user took back is not the new display's stray",
        !listed,
        strays.text(),
    );
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
