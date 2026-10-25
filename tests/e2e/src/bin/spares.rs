//! An ended display stays connected as a spare, and the next display of its
//! size is that one: no display is added or removed, so no app on the Mac
//! reconfigures its screens. Each reconfiguration with a virtual display
//! connected leaves ColorSync answering more requests for minutes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e spares

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
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
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

fn test() -> Result<()> {
    let control = Control::connect()?;
    let (first, made) = reconfigurations(6, || create(&control, "first"))?;
    println!("     making display {first}: {made} reconfigurations");
    let ((), ended) = reconfigurations(5, || {
        control.call(
            "endDisplay",
            json!({ "sessionKey": "first", "displayId": first }),
        )?;
        Ok(())
    })?;
    check(
        "an ended, empty display stays connected",
        ended == 0,
        format!("{ended} reconfigurations"),
    );
    let (second, reused) = reconfigurations(5, || create(&control, "second"))?;
    check(
        "the next display of its size is the spare",
        second == first && reused == 0,
        format!("display {second}, {reused} reconfigurations"),
    );
    let shot = control.call("screenshot", json!({ "displayId": second }))?;
    check(
        "the reused display takes screenshots",
        shot["width"].as_u64().unwrap_or(0) > 0,
        format!("{}×{}", shot["width"], shot["height"]),
    );
    let log = std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("HOME")?)
            .join("Library/Logs/Agent Screen/displays-e2e.log"),
    )?;
    let mentions = |event: &str| log.contains(&format!("{event} display={first} "));
    check(
        "the display log records it",
        mentions("created") && mentions("kept") && mentions("reused"),
        format!(
            "created {}, kept {}, reused {}",
            mentions("created"),
            mentions("kept"),
            mentions("reused")
        ),
    );
    Ok(())
}
