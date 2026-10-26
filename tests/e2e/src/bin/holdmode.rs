//! A display keeps the size it was made with (issue #33). Under load,
//! WindowServer switched a 1600×1000 display to 800×600 by itself; `tools/
//! mode.swift` makes the same switch, and the display is back at 1600×1000
//! at 2x within 3 s, with a line in the display log that says so.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e holdmode
//!
//! Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, run, sh, sh_lenient, sleep_ms, start_test_app, stop_test_app,
    tool,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn mode(id: u64) -> Result<String> {
    Ok(sh(tool("mode")?, &[&id.to_string()])?.trim().to_string())
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "holdmode", "title": "Hold mode", "width": 1600, "height": 1000 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    sleep_ms(1000);
    let wanted = "1600 1000 3200";
    let before = mode(id)?;
    check(
        "the display runs at 1600×1000 at 2x",
        before == wanted,
        &before,
    );
    let log = std::path::PathBuf::from(std::env::var("HOME")?).join(format!(
        "Library/Logs/Agent Screen/displays-{}.log",
        instance().unwrap_or_default()
    ));
    let logged = || std::fs::read_to_string(&log).unwrap_or_default();
    let before_log = logged().len();
    let switched = sh_lenient(tool("mode")?, &[&id.to_string(), "800", "600"]);
    let dropped = mode(id)?;
    println!("  switched to 800×600: {} (now {dropped})", switched.trim());
    // Agent Screen may switch it back before the first look above, and
    // notes it in the display log once done.
    let noted = || {
        logged()
            .get(before_log..)
            .unwrap_or_default()
            .lines()
            .find(|l| l.contains(&format!("mode display={id} ")) && l.contains("ran at 800x600"))
            .map(str::to_string)
    };
    let mut now = dropped.clone();
    let mut note = None;
    for _ in 0..30 {
        sleep_ms(100);
        now = mode(id)?;
        note = noted();
        if now == wanted && note.is_some() {
            break;
        }
    }
    check(
        "after WindowServer drops its mode, the display is back at 1600×1000 at 2x",
        switched.trim() == "0" && now == wanted && note.is_some(),
        format!("right after the switch {dropped}, then {now}; display log: {note:?}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "holdmode", "displayId": id }),
    )?;
    Ok(())
}
