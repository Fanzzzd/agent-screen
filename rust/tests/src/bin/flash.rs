//! How long a running app's new window shows on the user's screen before it
//! moves: TextEdit launched again RUNS times (default 15), with the
//! distribution. TextEdit must not be in use.
//!
//!     SIDESCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e flash
//!     FILES=1 SIDESCREEN_RUNNING_TURN_MS=0 ...   a file window that comes late

use std::process::ExitCode;

use anyhow::{Context, Result};
use agent_screen_tests::{
    McpClient, env_count, env_is, in_test_app, instance, json, now_ms, quit_discarding, run,
    running, sh, skip, sleep_ms, tool, wait_for_idle, watch_flash,
};

fn main() -> ExitCode {
    if running("TextEdit") {
        skip("TextEdit is the user's");
    }
    if instance().is_some() && !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| {
        let mut times = Vec::new();
        in_test_app(|| test(&mut times))?;
        let mut sorted = times.clone();
        sorted.sort_unstable();
        let all: Vec<String> = times.iter().map(i64::to_string).collect();
        println!(
            "flash ms: median {}, max {}, over 34: {}/{} ({})",
            json!(sorted.get(sorted.len() / 2)),
            json!(sorted.last()),
            times.iter().filter(|&&t| t > 34).count(),
            times.len(),
            all.join(" ")
        );
        Ok(())
    })
}

fn test(times: &mut Vec<i64>) -> Result<()> {
    tool("flash")?;
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &format!("flash-{}", now_ms()))])?;
    let display_id = client
        .tool("create_display", json!({ "title": "Flash" }))?
        .display_id()
        .context("create_display gave no display")?;

    let launch = |file: Option<&str>| {
        let mut args = json!({ "display_id": display_id, "app": "TextEdit" });
        if let Some(file) = file {
            args["open"] = json!(file);
        }
        client.tool("launch_app", args)
    };
    let result = (|| -> Result<()> {
        launch(None)?;
        let runs = env_count("RUNS", 15.0);
        let mut i = 0;
        while f64::from(i) < runs {
            // FILES=1: open a new file each time. With SIDESCREEN_RUNNING_TURN_MS=0
            // each one takes the path of a file that opens after the 4 s turn.
            let file =
                env_is("FILES", "1").then(|| format!("/tmp/sidescreen flash {}.txt", now_ms()));
            if let Some(file) = &file {
                std::fs::write(file, "flash\n")?;
            }
            let (seen, ()) = watch_flash("TextEdit", || {
                let reply = launch(file.as_deref())?;
                // A late file window moves after the reply: give it the time.
                if file.is_some() && reply.get("window").is_null() {
                    sleep_ms(1500);
                }
                Ok(())
            })?;
            times.push(seen.ms);
            if let Some(file) = &file {
                sh("/bin/rm", &["-f", file])?;
            }
            i += 1;
        }
        Ok(())
    })();

    let _ = client.tool("end_display", json!({ "display_id": display_id }));
    // Fails when TextEdit has quit already.
    quit_discarding("TextEdit");
    client.close();
    result
}
