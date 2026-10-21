//! Agent Screen's claim file tells AeroSpace which displays and windows are
//! the agents' (the fork's SIDESCREEN.md, Claims), so AeroSpace leaves a
//! window on an Agent Display alone when its app activates itself, and
//! leaves restored windows on a display made again after a restart. The
//! file lists a display from its creation, a window once it is placed
//! there, and drops the window when the user takes it back. It is gone
//! once the app quits.
//!
//! Needs Field.app, which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e claims
//!
//! Makes 1 display. Field's window comes to your screen when taken back.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, Value, check, instance, json, out, pid_of, run, sh_lenient, sleep_ms,
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
        let own = pid_of(DEBUG_BINARY).context("no test app")?;
        let result = test(own);
        let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
        stop_test_app();
        sleep_ms(500);
        check(
            "the claim file is gone once the app quits",
            !claim_path(own).exists(),
            claim_path(own).display().to_string(),
        );
        result
    })
}

fn claim_path(pid: u32) -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/AeroSpace/claims")
        .join(format!("{pid}.json"))
}

/// The claimed display and window ids.
fn claims(pid: u32) -> (Vec<u64>, Vec<u64>) {
    let text = std::fs::read_to_string(claim_path(pid)).unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or_default();
    let ids = |key: &str| {
        value[key]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default()
    };
    (ids("displays"), ids("windows"))
}

fn test(own: u32) -> Result<()> {
    let control = Control::connect()?;
    let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
    let key = "claims";
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Claims", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    check(
        "the new display is claimed",
        claims(own).0.contains(&id),
        format!("{:?}", claims(own)),
    );
    control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": out().join("Field.app").to_str() }),
    )?;
    let mut window = None;
    for _ in 0..50 {
        let listed = control.call("listWindows", json!({ "displayId": id }))?;
        window = listed["windows"].as_array().and_then(|w| {
            w.iter()
                .find(|w| w["app"] == "Field")
                .and_then(|w| w["windowId"].as_u64())
        });
        if window.is_some() {
            break;
        }
        sleep_ms(100);
    }
    let window = window.context("Field's window never reached the display")?;
    check(
        "Field's window on the display is claimed",
        claims(own).1.contains(&window),
        format!("window {window}, {:?}", claims(own)),
    );
    control.call(
        "debugTakeBack",
        json!({ "sessionKey": key, "displayId": id, "windowId": window }),
    )?;
    check(
        "a window taken back is no longer claimed",
        !claims(own).1.contains(&window),
        format!("window {window}, {:?}", claims(own)),
    );
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}
