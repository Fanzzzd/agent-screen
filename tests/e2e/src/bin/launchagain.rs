//! launch_app on an app that already runs on the display, with the same
//! arguments, as an agent that lost track of its window does (issue #29).
//! The Electron fixture allows one instance, as packaged Electron apps do:
//! the second one quits at once. The call must not answer "did not start".
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e launchagain

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, check, fixture, instance, json, out, run, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::Result;
use serde_json::Value;

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
        allow_on_your_screen(
            "Electron",
            800,
            "Electron brings itself to the front as its first window shows, which shows the app Agent Screen started hidden, and answers no accessibility call for about 0.5 s after (AXRole on it took 508 ms), so nothing can move the window sooner",
        );
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let mut pids = Vec::new();
        let result = test(&electron, &mut pids);
        for pid in pids {
            kill(pid);
        }
        stop_test_app();
        result
    })
}

/// Only the instances this script started: another script's may run.
fn kill(pid: i64) {
    let _ = std::process::Command::new("/bin/kill")
        .arg(pid.to_string())
        .status();
}

fn launch(control: &Control, id: &Value, electron: &std::path::Path) -> Result<Value> {
    control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_SINGLE": "1" },
        }),
    )
}

fn test(electron: &std::path::Path, pids: &mut Vec<i64>) -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "launchagain", "title": "Launch again", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].clone();
    let first = launch(&control, &id, electron);
    let first_pid = first.as_ref().ok().and_then(|f| f["pid"].as_i64());
    pids.extend(first_pid);
    check(
        "the first launch starts the app",
        first_pid.is_some(),
        format!("{first:?}"),
    );
    sleep_ms(1000);
    let started = std::time::Instant::now();
    let again = launch(&control, &id, electron);
    let took = started.elapsed().as_millis();
    if let Some(pid) = again.as_ref().ok().and_then(|a| a["pid"].as_i64()) {
        pids.push(pid);
    }
    println!("second launch in {took} ms: {again:?}");
    let error = again
        .as_ref()
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    check(
        "launching it again doesn't say it did not start",
        !error.contains("did not start"),
        format!("{again:?}"),
    );
    check(
        "launching it again answers with the running app's window, or says it is already running",
        again.as_ref().is_ok_and(|a| !a["window"].is_null()) || error.contains("already running"),
        format!("{again:?}"),
    );
    let _ = control.call(
        "endDisplay",
        json!({ "sessionKey": "launchagain", "displayId": id }),
    );
    Ok(())
}
