//! How long the Mac stops drawing when a display is added and when one is
//! removed: a window of the `frames` tool animates two squares, one by Core
//! Animation and one by its main thread, and every stretch where they stood
//! still counts. The test build makes one display of a size no spare has,
//! ends it, and quits, which removes it. Set the test build's knobs in the
//! environment to compare: AGENTSCREEN_DISPLAY_SCALE, AGENTSCREEN_DISPLAY_HZ,
//! AGENTSCREEN_NO_PARK, AGENTSCREEN_MAX_PIXELS.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e changecost
//!
//! Makes 1 display.

use std::io::{BufRead, BufReader};
use std::process::{Command, ExitCode, Stdio};

use agent_screen_tests::{
    Control, instance, json, now_ms, run, sleep_ms, start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(test)
}

fn test() -> Result<()> {
    let mut frames = Command::new(tool("frames")?)
        .arg("40")
        .stdout(Stdio::piped())
        .spawn()?;
    let mut lines = BufReader::new(frames.stdout.take().context("frames output")?).lines();
    // Up and capturing before anything changes.
    let first = lines.next().context("frames ended")??;
    anyhow::ensure!(first == "capturing", "frames: {first}");
    // No tidying while measuring: displays change only when this script says.
    start_test_app(&[
        ("AGENTSCREEN_LANG", "en"),
        ("AGENTSCREEN_IDLE_SECS", "1000000"),
    ])?;
    sleep_ms(4000);
    let control = Control::connect()?;
    let width = 1000 + 8 * (now_ms() / 1000 % 50);
    let added = now_ms() as u64;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "cost", "title": "Cost", "width": width, "height": 700 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    sleep_ms(6000);
    control.call(
        "endDisplay",
        json!({ "sessionKey": "cost", "displayId": id }),
    )?;
    sleep_ms(2000);
    let removed = now_ms() as u64;
    stop_test_app();
    sleep_ms(6000);
    let _ = frames.kill();
    let gaps: Vec<(String, u64, u64)> = lines
        .map_while(|l| l.ok())
        .filter_map(|l| {
            let mut parts = l.split(' ').skip(1);
            Some((
                parts.next()?.to_string(),
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ))
        })
        .collect();
    let sum = |from: u64, to: u64, kind: &str| {
        let within: Vec<u64> = gaps
            .iter()
            .filter(|(k, start, _)| k == kind && *start >= from && *start < to)
            .map(|(_, _, ms)| *ms)
            .collect();
        (
            within.iter().sum::<u64>(),
            within.iter().copied().max().unwrap_or(0),
            within.len(),
        )
    };
    for (what, from, to) in [
        ("add", added, added + 6000),
        ("remove", removed, removed + 6000),
    ] {
        let (ca, ca_max, ca_n) = sum(from, to, "ca");
        let (main, main_max, main_n) = sum(from, to, "main");
        println!(
            "{what}: Core Animation stood still {ca} ms in {ca_n} stops (longest {ca_max}), main-thread animation {main} ms in {main_n} (longest {main_max})"
        );
    }
    Ok(())
}
