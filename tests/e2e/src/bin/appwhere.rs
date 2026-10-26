//! An app the user brings to the front whose windows are all on an Agent
//! Display gets a notification naming that display (#51). Field opens on
//! the user's screen, as their own app: brought to the front, nothing to
//! say. The agent asks for its window and the test hands it over; brought
//! to the front again, the reminder names the display, and once more right
//! after, it stays quiet. A Field the agent started brings itself to the
//! front with nobody to tell. `debugActivated` stands in for the user's
//! click in the Dock.
//!
//! Needs Field.app, which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e appwhere
//!
//! Makes 1 display. Field's window shows on your screen until handed over,
//! and a notification shows there.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, Value, check, instance, json, out, pid_of, run, sh_lenient, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if !out().join("Field.app").exists() {
        eprintln!("no Field.app; run `cargo e2e ime` once to make it");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        quit_field();
        let result = test();
        quit_field();
        stop_test_app();
        result
    })
}

fn field_binary() -> String {
    out()
        .join("Field.app/Contents/MacOS/Field")
        .to_string_lossy()
        .into_owned()
}

/// This checkout's Field only: another worktree may run its own.
fn quit_field() {
    let _ = sh_lenient("/usr/bin/pkill", &["-f", &field_binary()]);
    sleep_ms(500);
}

/// The displays the reminder named when `pid` came to the front.
fn activated(control: &Control, pid: u32) -> Result<Vec<u64>> {
    let reply = control.call("debugActivated", json!({ "pid": pid }))?;
    Ok(reply["reminded"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default())
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    let result = (|| -> Result<()> {
        // The user's own Field, on their screen.
        sh_lenient(
            "/usr/bin/open",
            &["-g", "-n", out().join("Field.app").to_str().unwrap()],
        );
        sleep_ms(2500);
        let pid = pid_of(&field_binary()).context("Field didn't start")?;
        let named = activated(&control, pid)?;
        check(
            "an app with its window on the user's screen gets no reminder",
            named.is_empty(),
            format!("{named:?}"),
        );

        // Handed over to the agent's display.
        let answer = std::thread::spawn(move || {
            sleep_ms(800);
            Control::connect()?.call(
                "debugAnswerWindow",
                json!({ "displayId": id, "give": true }),
            )
        });
        let placed = client.tool("place_window", json!({ "display_id": id, "app": "Field" }))?;
        let _ = answer.join();
        check("the window is handed over", !placed.is_error, placed.text());
        sleep_ms(800);
        let named = activated(&control, pid)?;
        check(
            "brought to the front, the reminder names the display its window is on",
            named == vec![id],
            format!("{named:?}, display {id}"),
        );
        let again = activated(&control, pid)?;
        check(
            "right after, it stays quiet",
            again.is_empty(),
            format!("{again:?}"),
        );

        // A Field the agent started, bringing itself to the front.
        let launched = client.call(
            "launch_app",
            // An environment starts a new instance beside the user's.
            json!({ "display_id": id, "app": out().join("Field.app").to_str(),
                    "environment": { "FIELD_MIN_SIZE": "300,200" } }),
        )?;
        sleep_ms(1000);
        let started = launched
            .get("pid")
            .as_u64()
            .map(|p| p as u32)
            .or_else(|| {
                sh_lenient("/usr/bin/pgrep", &["-f", &field_binary()])
                    .lines()
                    .filter_map(|l| l.parse().ok())
                    .find(|&p| p != pid)
            })
            .context("the agent's Field didn't start")?;
        let named = activated(&control, started)?;
        check(
            "an app the agent started gets no reminder when it brings itself to the front",
            named.is_empty(),
            format!("{named:?}"),
        );
        Ok(())
    })();
    let _ = client.call("end_display", json!({ "display_id": id }));
    client.close();
    result
}
