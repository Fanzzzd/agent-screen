//! An agent's screenshot never shows the user's menu bar: right after the
//! Electron harness on the display quits, a burst of screenshots keeps the
//! top strip one flat color (#38). Control Centre moves the same status
//! item windows onto the display then, and a listing of shareable windows
//! kept from before still had them on the user's screen, so the filter let
//! them in.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e barleak
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//! Makes 1 display. Nothing shows on your screen. BARLEAK_ROUNDS sets how
//! many times the harness starts and quits (default 6).

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, check, fixture, instance, json, out, probe, run, sh_lenient, skip, sleep_ms,
    start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};
use base64::Engine;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&electron);
        quit_harness();
        stop_test_app();
        result
    })
}

/// This checkout's harness only: another worktree may run its own.
fn quit_harness() {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &["-f", fixture("electron").to_str().unwrap()],
    );
}

/// Pixels in the top 24 rows of a screenshot unlike its corner.
fn leaked(control: &Control, id: &Value, n: usize) -> Result<usize> {
    let shot = control.call("screenshot", json!({ "displayId": id, "format": "png" }))?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().context("image data")?)?;
    let path = out().join(format!("barleak-{n}.png"));
    std::fs::write(&path, data)?;
    Ok(probe(&["strip", path.to_str().unwrap(), "24"])?
        .trim()
        .parse()?)
}

fn test(electron: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let rounds: usize = std::env::var("BARLEAK_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let key = "barleak";
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Bar leak", "width": 1440, "height": 900 }),
    )?;
    let id = made["displayId"].clone();
    let result = (|| -> Result<()> {
        let mut shots = 0;
        let mut leaks = Vec::new();
        for round in 0..rounds {
            control.call(
                "launchApp",
                json!({ "displayId": id, "app": electron.to_str(),
                        "arguments": [fixture("electron").to_str()] }),
            )?;
            sleep_ms(1200);
            let up = leaked(&control, &id, shots)?;
            shots += 1;
            if up > 0 {
                leaks.push(format!("round {round} with the harness up: {up} px"));
            }
            control.call(
                "key",
                json!({ "sessionKey": key, "displayId": id, "keys": "cmd+q" }),
            )?;
            // Screenshots as fast as an agent's next calls come.
            for i in 0..12 {
                let n = leaked(&control, &id, shots)?;
                shots += 1;
                if n > 0 {
                    // What sits in the strip then, by the window server.
                    let windows = sh_lenient(
                        &tool("onscreen")?,
                        &[&id.as_u64().unwrap_or_default().to_string()],
                    );
                    let bar: Vec<&str> = windows
                        .lines()
                        .filter(|l| l.contains("\"layer\":25") || l.contains("\"layer\":24"))
                        .collect();
                    println!(
                        "round {round} shot {i}: {n} px in the strip\n  {}",
                        bar.join("\n  ")
                    );
                    leaks.push(format!("round {round} shot {i}: {n} px"));
                }
            }
            quit_harness();
            sleep_ms(500);
        }
        check(
            &format!("the top strip stays flat in {shots} screenshots around {rounds} quits"),
            leaks.is_empty(),
            leaks.join("; "),
        );
        Ok(())
    })();
    let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
    result
}
