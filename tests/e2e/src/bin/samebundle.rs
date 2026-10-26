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
//! Makes 1 display.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use agent_screen_tests::{
    Control, allow_on_your_screen, check, fixture, instance, json, listening, out, run, skip,
    sleep_ms, start_test_app, stop_test_app,
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
        allow_on_your_screen(
            "Electron",
            800,
            "Electron brings itself to the front as its first window shows, which shows the app Agent Screen started hidden, and answers no accessibility call for about 0.5 s after (AXRole on it took 508 ms), so nothing can move the window sooner",
        );
        if listening() {
            bail!("quit the e2e app first: this test starts it with a delayed open");
        }
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_OPEN_DELAY_MS", "1500"),
        ])?;
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

fn copy_running(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
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
        // Another tool's Electron, started from a shell with other
        // arguments. It opens no window: one would show on the user's screen.
        let copy = Command::new(electron.join("Contents/MacOS/Electron"))
            .arg(fixture("electron"))
            .arg("--another-tool")
            .env("HARNESS_IDLE", "1")
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
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    let on: Vec<u64> = listed["windows"]
        .as_array()
        .map(|w| w.iter().filter_map(|w| w["windowId"].as_u64()).collect())
        .unwrap_or_default();
    let window = launched
        .as_ref()
        .ok()
        .and_then(|l| l["window"]["windowId"].as_u64());
    let pid = launched.as_ref().ok().and_then(|l| l["pid"].as_u64());
    let copy_runs = copy_running(copy.id());
    check(
        "the copy still runs, and launch_app didn't take it",
        copy_runs && pid != Some(u64::from(copy.id())),
        format!(
            "copy pid {} running {copy_runs}, launched pid {pid:?}",
            copy.id()
        ),
    );
    check(
        "launch_app answers with its own instance's window, on the display",
        window.is_some_and(|w| on.contains(&w)),
        format!("launched {launched:?}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "samebundle", "displayId": id }),
    )?;
    Ok(())
}
