//! launch_app with arguments takes only the process it started (issue #35).
//! Every dev checkout runs the same Electron.app, so another tool may start
//! the bundle's binary from a shell while launch_app waits for its own
//! instance. That copy's window stays where macOS puts it.
//!
//! The test app holds each `open` for 1.5 s (AGENTSCREEN_OPEN_DELAY_MS), so
//! the copy always starts inside that wait.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e samebundle
//!
//! Makes 1 display. The copy shows Electron's "Error launching app" box on
//! your screen for a few seconds.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use agent_screen_tests::{
    Control, HandsOff, check, fixture, instance, json, listening, out, run, skip, sleep_ms,
    start_test_app, stop_test_app, windows_of,
};
use anyhow::{Result, bail};

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
        if listening() {
            bail!("quit the e2e app first: this test starts it with a delayed open");
        }
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_OPEN_DELAY_MS", "1500"),
        ])?;
        // The copy's error box shows on the user's screen.
        let _sign = HandsOff::start("samebundle")?;
        let result = test(&electron);
        quit_all(&electron);
        stop_test_app();
        result
    })
}

fn quit_all(electron: &Path) {
    let binary = electron.join("Contents/MacOS/Electron");
    let _ = Command::new("/usr/bin/pkill")
        .arg("-f")
        .arg(binary)
        .status();
    sleep_ms(1000);
}

fn test(electron: &Path) -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "samebundle", "title": "Same bundle", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].clone();
    let app = electron.to_str().unwrap_or_default();
    let (launched, copy) = std::thread::scope(|scope| -> Result<_> {
        let launching = scope.spawn(|| {
            control.call(
                "launchApp",
                json!({
                    "displayId": id,
                    "app": app,
                    "arguments": [fixture("electron").to_str()],
                }),
            )
        });
        sleep_ms(100);
        // Another tool's Electron, from a checkout with nothing built.
        let copy = Command::new(electron.join("Contents/MacOS/Electron"))
            .arg("/tmp/agent-screen-e2e-no-such-checkout")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let launched = launching
            .join()
            .map_err(|_| anyhow::anyhow!("launch thread panicked"))?;
        Ok((launched, copy))
    })?;
    sleep_ms(2000);
    let theirs: Vec<u64> = windows_of(copy.id()).iter().map(|w| w.id).collect();
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    let on: Vec<u64> = listed["windows"]
        .as_array()
        .map(|w| w.iter().filter_map(|w| w["windowId"].as_u64()).collect())
        .unwrap_or_default();
    let window = launched
        .as_ref()
        .ok()
        .and_then(|l| l["window"]["windowId"].as_u64());
    check(
        "the copy's window stays off the display",
        !theirs.is_empty() && !theirs.iter().any(|w| on.contains(w)),
        format!(
            "copy pid {} windows {theirs:?}, on display {on:?}",
            copy.id()
        ),
    );
    check(
        "launch_app answers with its own instance's window, on the display",
        window.is_some_and(|w| on.contains(&w) && !theirs.contains(&w)),
        format!("launched {launched:?}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "samebundle", "displayId": id }),
    )?;
    Ok(())
}
