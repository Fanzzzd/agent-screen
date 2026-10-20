//! While the user is idle, Agent Screen makes the two spares the next two
//! "+" or create_display of the default size take, so those add no display
//! and nothing on the Mac stops drawing. The test build counts the user as
//! idle at once (AGENTSCREEN_IDLE_SECS=0) and tidies every 15 s.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e warmspare
//!
//! Makes 3 displays: one in use, and the two spares the app makes.

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
    // The tidies after the display was used make the spares, one each.
    let mut spares = Vec::new();
    for _ in 0..60 {
        sleep_ms(1000);
        spares = log()[from..]
            .lines()
            .filter(|l| l.contains("spare made while idle"))
            .map(String::from)
            .collect::<Vec<_>>();
        if spares.len() >= 2 {
            break;
        }
    }
    check(
        "an idle user gets two spares made while displays are in use",
        spares.len() == 2,
        format!("{spares:#?}"),
    );
    // Let them settle in place before counting.
    sleep_ms(3000);
    for key in ["second", "third"] {
        let (id, added) = reconfigurations(5, || create(&control, key))?;
        check(
            &format!("the {key} display takes a spare: no display change"),
            id != first && added == 0,
            format!("display {id}, {added} reconfigurations"),
        );
    }
    Ok(())
}
