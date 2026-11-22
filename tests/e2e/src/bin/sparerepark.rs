//! A spare that macOS reattaches next to the user's screen is parked out
//! of the way again (#286). With no Agent Display live, Agent Screen used
//! to check only live displays after a display change, so a spare could
//! stay beside the user's screen for up to an hour and take their pointer.
//! The script makes a display, ends it so it stays as a spare, moves the
//! spare against the main display's right edge with `tools/origin.swift`,
//! and waits for Agent Screen to move it off that edge.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e sparerepark
//!
//! Makes 1 display. The spare touches your main screen's right edge for
//! up to 5 s, and your pointer could slide onto it meanwhile.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, check, instance, json, run, sh, sleep_ms, start_test_app, stop_test_app, tool,
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
        // Quitting releases the spare, wherever it is.
        stop_test_app();
        result
    })
}

/// A display's bounds: x, y, width, height in points.
fn bounds(display: &str) -> Result<[i64; 4]> {
    let text = sh(tool("origin")?, &[display])?;
    let n: Vec<i64> = text
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    Ok([
        *n.first().context("x")?,
        *n.get(1).context("y")?,
        *n.get(2).context("w")?,
        *n.get(3).context("h")?,
    ])
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "sparerepark", "title": "Spare repark" }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    // Empty, it stays connected as a spare under the same id.
    control.call(
        "endDisplay",
        json!({ "sessionKey": "sparerepark", "displayId": id }),
    )?;
    sleep_ms(1000);
    let id = id.to_string();
    let parked = bounds(&id)?;
    check(
        "the ended display stays online as a spare",
        parked[2] > 0,
        format!("{parked:?}"),
    );
    let main = bounds("main")?;
    let edge = (main[0] + main[2], main[1]);
    let moved = sh(
        tool("origin")?,
        &[&id, &edge.0.to_string(), &edge.1.to_string()],
    )?;
    println!("  moved the spare to {edge:?}: {}", moved.trim());
    let beside = |b: [i64; 4]| b[0] == edge.0 && b[1] < main[1] + main[3] && b[1] + b[3] > main[1];
    let until = Instant::now() + Duration::from_secs(5);
    let mut now = bounds(&id)?;
    while beside(now) && Instant::now() < until {
        sleep_ms(100);
        now = bounds(&id)?;
    }
    check(
        "macOS took the move of the spare against the main screen's right edge",
        moved.trim() == "0",
        moved.trim(),
    );
    check(
        "Agent Screen parks the spare off the user's screen again within 5 s",
        !beside(now),
        format!("main {main:?}, spare {now:?}"),
    );
    Ok(())
}
