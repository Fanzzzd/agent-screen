//! The only app on a display quits, and the display stays the agent's
//! (issue #43, ADR 0004): it ends in review, not when its last app quits
//! while the session lives. The next launch_app on it works, and a
//! screenshot of it too.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e lastapp
//!
//! Makes 1 display. Needs Field.app from `ime`.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, check, cut, instance, json, out, pid_of, run, sh_lenient, sleep_ms, start_test_app,
    stop_test_app,
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
        // This checkout's Field only: another's may be running.
        let _ = sh_lenient("/usr/bin/pkill", &["-f", &binary()]);
        stop_test_app();
        result
    })
}

fn binary() -> String {
    out()
        .join("Field.app/Contents/MacOS/Field")
        .to_string_lossy()
        .into_owned()
}

fn test(field: &std::path::Path) -> Result<()> {
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 900, "height": 600 }))?;
    let id = made.display_id().context("display id")?;
    let launched = client.call(
        "launch_app",
        json!({ "display_id": id, "app": field.to_str().unwrap() }),
    )?;
    check(
        "launch_app puts Field on the display",
        launched.window().is_some(),
        cut(launched.text(), 200),
    );
    let pid = pid_of(&binary()).context("Field's pid")?;
    // Its only app quits, as UTM did when the agent quit it from a shell.
    sh_lenient("/bin/kill", &[&pid.to_string()]);
    for _ in 0..30 {
        if pid_of(&binary()).is_none() {
            break;
        }
        sleep_ms(100);
    }
    // The sweep of empty ended displays runs 600 ms after a change.
    sleep_ms(3000);
    let shot = client.tool(
        "computer",
        json!({ "display_id": id, "action": "screenshot" }),
    )?;
    check(
        "after its only app quit, a screenshot of the display works",
        shot.image().is_some(),
        cut(shot.text(), 200),
    );
    let again = client.tool(
        "launch_app",
        json!({ "display_id": id, "app": field.to_str().unwrap() }),
    )?;
    check(
        "after its only app quit, launch_app on the display works",
        again.window().is_some(),
        cut(again.text(), 200),
    );
    let _ = client.tool("end_display", json!({ "display_id": id }));
    Ok(())
}
