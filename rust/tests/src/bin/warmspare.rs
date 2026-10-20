//! While the user is idle, Agent Screen makes the spare the next "+" or
//! create_display of the default size takes, so that one adds no display
//! and nothing on the Mac stops drawing. The test build counts the user as
//! idle at once (AGENTSCREEN_IDLE_SECS=0) and tidies every 15 s.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e warmspare
//!
//! Makes 2 displays: one in use, and the spare the app makes.

use std::process::ExitCode;
use std::thread;

use agent_screen_tests::{
    Control, check, instance, json, probe, run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en"), ("AGENTSCREEN_IDLE_SECS", "0")])?;
        let result = test();
        stop_test_app();
        result
    })
}

/// Display reconfigurations while `act` runs, counted for `seconds`.
fn reconfigurations<T>(seconds: u32, act: impl FnOnce() -> Result<T>) -> Result<(T, u64)> {
    let counting = thread::spawn(move || probe(&["reconfigurations", &seconds.to_string()]));
    // The probe needs a moment to start listening.
    sleep_ms(1500);
    let value = act()?;
    let count = counting
        .join()
        .map_err(|_| anyhow::anyhow!("probe panicked"))??
        .trim()
        .parse()
        .context("count")?;
    Ok((value, count))
}

fn create(control: &Control, key: &str) -> Result<u64> {
    control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": key, "width": 1280, "height": 800 }),
    )?["displayId"]
        .as_u64()
        .context("display id")
}

fn log() -> String {
    std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Library/Logs/Agent Screen/displays-e2e.log"),
    )
    .unwrap_or_default()
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let from = log().len();
    let first = create(&control, "first")?;
    // The next tidy after the display was used makes the spare.
    let mut spare = None;
    for _ in 0..40 {
        sleep_ms(1000);
        spare = log()[from..]
            .lines()
            .find(|l| l.contains("spare made while idle"))
            .map(String::from);
        if spare.is_some() {
            break;
        }
    }
    check(
        "an idle user gets a spare made while displays are in use",
        spare.is_some(),
        format!("{spare:?}"),
    );
    // Let it settle in place before counting.
    sleep_ms(3000);
    let (second, added) = reconfigurations(5, || create(&control, "second"))?;
    check(
        "the next display takes the spare: no display change",
        second != first && added == 0,
        format!("display {second}, {added} reconfigurations"),
    );
    Ok(())
}
