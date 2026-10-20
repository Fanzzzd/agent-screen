//! Agent Screen's own windows keep drawing while a display comes and goes.
//! WindowServer holds on to drawables then; a window that waited for one on
//! the main thread froze the whole UI for up to a second. The Viewer shows
//! an agent's display, Calculator counting on it, on a stage display while
//! two more displays are made and ended; the test app's log must show no
//! main-thread stall waiting for a drawable, and the Viewer must still draw.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e displaychange
//!
//! Makes 4 displays: the stage, the agent's, and two of sizes no spare has.
//! Skips when Calculator is running with windows; quits the one it started.

use std::path::PathBuf;
use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, capture, check, instance, json, now_ms, open_stage, pid_of, pixel, run,
    running_headed, skip, sleep_ms, start_test_app, stop_test_app, try_osa, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running_headed("Calculator") {
        skip("Calculator is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        drop(try_osa("tell application \"Calculator\" to quit"));
        stop_test_app();
        result
    })
}

fn log_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!(
        "Library/Logs/Agent Screen/displays-{}.log",
        instance().unwrap_or_default()
    ))
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let pid = pid_of(DEBUG_BINARY).context("no test app")?;
    open_stage(&control)?;
    let agent = control.call(
        "createDisplay",
        json!({ "sessionKey": "agent", "title": "Agent", "width": 1200, "height": 760 }),
    )?["displayId"]
        .as_u64()
        .context("display id")?;
    control.call(
        "launchApp",
        json!({ "sessionKey": "agent", "displayId": agent, "app": "Calculator" }),
    )?;
    let before: Vec<u64> = windows_of(pid).iter().map(|w| w.id).collect();
    control.call(
        "debugShow",
        json!({ "surface": "viewer", "displayId": agent }),
    )?;
    sleep_ms(1000);
    let viewer = windows_of(pid)
        .into_iter()
        .filter(|w| !before.contains(&w.id))
        .max_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height)))
        .context("the Viewer didn't open")?;
    let from = std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0);
    // Another agent makes and ends two displays, on its own connection.
    let changing = std::thread::spawn(|| -> Result<()> {
        let control = Control::connect()?;
        for round in 0..2u64 {
            let key = format!("displaychange-{round}");
            // A size no spare has, so the display is new.
            let width = 1000 + 8 * ((now_ms() / 1000 % 50) as u64) + round;
            let made = control.call(
                "createDisplay",
                json!({ "sessionKey": key, "title": "Change", "width": width, "height": 700 }),
            )?;
            let id = made["displayId"].as_u64().context("display id")?;
            sleep_ms(2500);
            control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
            sleep_ms(1500);
        }
        Ok(())
    });
    // Meanwhile Calculator's number changes all along, so the Viewer draws
    // all along.
    while !changing.is_finished() {
        control.call(
            "type",
            json!({ "sessionKey": "agent", "displayId": agent, "text": "7" }),
        )?;
        sleep_ms(150);
    }
    changing
        .join()
        .map_err(|_| anyhow::anyhow!("display changes panicked"))??;

    let log = std::fs::read(log_path()).unwrap_or_default();
    let tail = String::from_utf8_lossy(&log[(from as usize).min(log.len())..]).to_string();
    let stalls: Vec<&str> = tail
        .lines()
        .filter(|l| l.contains("main thread busy"))
        .collect();
    let waited: Vec<&&str> = stalls
        .iter()
        .filter(|l| l.contains("Drawable") || l.contains("semaphore"))
        .collect();
    check(
        "the main thread never waits for a drawable while displays change",
        waited.is_empty(),
        format!("{} stalls: {stalls:#?}", stalls.len()),
    );

    // The Viewer still draws: its middle isn't the empty window's clear color.
    let shot = capture(
        "displaychange-viewer",
        [viewer.x, viewer.y, viewer.width, viewer.height],
    )?;
    let middle = pixel(&shot, viewer.width / 2.0, viewer.height / 2.0);
    check(
        "the Viewer still draws after the displays came and went",
        middle.iter().take(3).any(|&c| c > 8.0),
        format!("pixel {middle:?} in {}", shot.display()),
    );
    control.call(
        "debugShow",
        json!({ "surface": "collapse", "displayId": 0 }),
    )?;
    Ok(())
}
