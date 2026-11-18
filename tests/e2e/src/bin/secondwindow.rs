//! A second window an app on an Agent Display opens leaves the user's front
//! app in front (issue #67). The Electron fixture starts on the display,
//! then a click there runs the page's `window.open`, whose window Electron
//! shows by activating the app. `probe fronts` lists every app that comes
//! to the front meanwhile, as NSWorkspace reports it. macOS reports the
//! activation only once it happened, so the app may hold the front for a
//! few ms; the checks allow 300.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON. The
//! app may show in front of yours for a moment while the bug is there.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e secondwindow

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, check, fixture, instance, json, now_ms, out, run, skip,
    sleep_ms, start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};
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
        let result = test(&electron);
        stop_test_app();
        result
    })
}

fn harness_windows(control: &Control, id: &Value) -> Result<Vec<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"]
        .as_array()
        .map(|w| {
            w.iter()
                .filter(|w| w["app"].as_str() == Some("Electron") && w["kind"] == "window")
                .cloned()
                .collect()
        })
        .unwrap_or_default())
}

/// The apps that came to the front: (epoch ms, pid, name).
fn fronts(output: &str) -> Vec<(u128, i64, String)> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            Some((
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next().unwrap_or_default().to_string(),
            ))
        })
        .collect()
}

fn test(electron: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "secondwindow", "title": "Second window", "width": 1440, "height": 900 }),
    )?;
    let id = display["displayId"].clone();
    let watch = std::process::Command::new(tool("probe")?)
        .args(["fronts", "30"])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    sleep_ms(300);
    let launched_at = now_ms();
    let launched = control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
        }),
    );
    let mut first = None;
    for _ in 0..50 {
        first = harness_windows(&control, &id)?.into_iter().next();
        if first.is_some() {
            break;
        }
        sleep_ms(200);
    }
    let first = first.with_context(|| format!("no harness window: {launched:?}"))?;
    let pid = launched.as_ref().ok().and_then(|l| l["pid"].as_i64());
    // Let the launch settle, so what follows is the second window's alone.
    sleep_ms(3000);
    let (x, y, width) = (
        first["x"].as_i64().context("x")?,
        first["y"].as_i64().context("y")?,
        first["width"].as_i64().context("width")?,
    );
    let scale = width as f64 / 640.0;
    // The button that opens a window sits in the page's top right corner,
    // below the 28 pt title bar.
    let (bx, by) = (
        x + width - (68.0 * scale) as i64,
        y + ((28.0 + 23.0) * scale) as i64,
    );
    let clicked_at = now_ms();
    control.call("click", json!({ "displayId": id, "x": bx, "y": by }))?;
    let mut count = 0;
    for _ in 0..40 {
        count = harness_windows(&control, &id)?.len();
        if count >= 2 {
            break;
        }
        sleep_ms(200);
    }
    sleep_ms(4000);
    let ended_at = now_ms();
    let _ = std::process::Command::new("/bin/kill")
        .arg(watch.id().to_string())
        .status();
    let output = String::from_utf8_lossy(&watch.wait_with_output()?.stdout).to_string();
    let seen = fronts(&output);
    for (at, pid, name) in &seen {
        println!(
            "  front at {:+} ms after the click: {name} ({pid})",
            *at as i128 - clicked_at as i128
        );
    }
    let user = seen.first().map(|f| f.2.clone()).unwrap_or_default();
    // The longest Electron stayed the front app, in ms, from a change
    // between `from` and `to` to the next change.
    let held = |from: u128, to: u128| {
        seen.iter()
            .enumerate()
            .filter(|(_, (at, _, name))| *at >= from && *at < to && name == "Electron")
            .map(|(i, (at, _, _))| seen.get(i + 1).map_or(ended_at, |next| next.0) - at)
            .max()
            .unwrap_or(0)
    };
    // macOS reports the activation after the app made it, so the user's app
    // comes back a moment later: allow that, not seconds.
    const LONGEST_MS: u128 = 300;
    check(
        "the second window opens on the display",
        count >= 2,
        format!("{count} harness windows on the display"),
    );
    let second = held(clicked_at, ended_at);
    check(
        "the second window leaves the user's front app in front",
        second < LONGEST_MS && seen.last().is_some_and(|f| f.2 == user),
        format!("front before: {user}; Electron held the front for {second} ms"),
    );
    let launch = held(launched_at, clicked_at);
    check(
        "the launch leaves the user's front app in front",
        launch < LONGEST_MS,
        format!("Electron held the front for {launch} ms"),
    );
    quit_harness(pid);
    Ok(())
}

/// Quit this run's harness only: another checkout's may be running.
fn quit_harness(pid: Option<i64>) {
    if let Some(pid) = pid {
        let _ = std::process::Command::new("/bin/kill")
            .arg(pid.to_string())
            .status();
        sleep_ms(1000);
    }
}
