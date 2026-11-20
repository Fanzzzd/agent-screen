//! launch_app into a running app whose new window Accessibility can't
//! describe as it shows (#270). Dia, building a window, keeps its main
//! thread busy: the catch's look at the window times out, and the launch
//! used to give up, saying no window came, while the window stayed on the
//! user's screen.
//!
//! The Electron harness stands in, started with `open` as an app the user
//! runs, with no window (HARNESS_IDLE). Its File > New Window opens a
//! window without activating the app, then keeps the main thread busy for
//! 1.5 s (HARNESS_SLOW_WINDOW). launch_app on it must press that item,
//! answer with the new window, and place it on the display; afterwards no
//! window of the harness is left on your screen.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e slowwindow
//!
//! The new window shows on your screen while the harness is busy, up to
//! about 2 s, as nothing can move it sooner. Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, cut, fixture, instance, json, out, probe, run,
    sh_lenient, skip, sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

const KEY: &str = "slowwindow";
/// How long the harness's main thread stays busy after its window shows.
const BUSY_MS: u64 = 1500;

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
            BUSY_MS + 700,
            "the harness's new window shows on your screen while its main thread is busy, so nothing can move it until then",
        );
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Slow window", "width": 1100, "height": 700 }),
        )?;
        let id = made["displayId"].clone();
        let log = out().join("slowwindow.log");
        let _ = std::fs::remove_file(&log);
        let mut pid = None;
        let result = test(&control, &id, &electron, &log, &mut pid);
        // Only the instance this script started.
        if let Some(pid) = pid {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.to_string())
                .status();
        }
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        stop_test_app();
        result
    })
}

fn test(
    control: &Control,
    id: &Value,
    electron: &std::path::Path,
    log: &std::path::Path,
    pid: &mut Option<u32>,
) -> Result<()> {
    let before = harness_pids();
    sh_lenient(
        "/usr/bin/open",
        &[
            "-n",
            "-g",
            "--env",
            "HARNESS_IDLE=1",
            "--env",
            &format!("HARNESS_MENU_LOG={}", log.display()),
            "--env",
            &format!("HARNESS_SLOW_WINDOW={BUSY_MS}"),
            "-a",
            electron.to_str().unwrap_or_default(),
            "--args",
            fixture("electron").to_str().unwrap_or_default(),
        ],
    );
    for _ in 0..40 {
        *pid = harness_pids().into_iter().find(|p| !before.contains(p));
        if pid.is_some() {
            break;
        }
        sleep_ms(250);
    }
    let pid = pid.context("the harness didn't start")?;
    // Its menu bar is up once Electron is ready.
    sleep_ms(2000);
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": electron.to_str() }),
    );
    let said = match &launched {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    };
    let window = launched
        .as_ref()
        .ok()
        .and_then(|v| v["window"]["windowId"].as_u64());
    let pressed = std::fs::read_to_string(log)
        .unwrap_or_default()
        .contains(&format!("new window {pid}"));
    check(
        "launch_app presses New Window and answers with the window the busy app made",
        window.is_some() && pressed,
        format!("pressed {pressed} | {}", cut(&said, 400)),
    );
    sleep_ms(1500);
    let on_display = control
        .call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))
        .map(|v| v.to_string())
        .unwrap_or_default();
    check(
        "the window is on the display",
        window.is_some_and(|w| on_display.contains(&format!("\"windowId\":{w}"))),
        cut(&on_display, 400),
    );
    let display_bounds = probe(&["bounds", &id.to_string()]).unwrap_or_default();
    let shown: Vec<_> = windows_of(pid)
        .into_iter()
        .filter(|w| w.layer == 0)
        .collect();
    check(
        "no window of the harness is left on your screen",
        !shown.is_empty() && shown.iter().all(|w| within(w, &display_bounds)),
        format!("windows {shown:?}, display {}", display_bounds.trim()),
    );
    Ok(())
}

/// Electron harness processes now: the main process of each instance.
fn harness_pids() -> Vec<u32> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Electron"])
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Whether window `w`'s middle is inside the display, given as `probe
/// bounds` prints it: x y width height.
fn within(w: &agent_screen_tests::Frame, bounds: &str) -> bool {
    let b: Vec<f64> = bounds
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter_map(|n| n.parse().ok())
        .collect();
    let (mx, my) = (w.x + w.width / 2.0, w.y + w.height / 2.0);
    b.len() >= 4 && mx >= b[0] && mx <= b[0] + b[2] && my >= b[1] && my <= b[1] + b[3]
}
