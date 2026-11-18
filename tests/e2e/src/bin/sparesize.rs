//! A spare that grows taller is parked again before adoption (#132).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e sparesize
//!
//! One empty display, reused at two sizes. No app launches or real input.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, listening, probe, run, skip, sleep_ms, start_test_app,
    stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        if listening() {
            skip("the test instance is already running; this test needs an empty spare list");
        }
        start_test_app(&[])?;
        let result = test();
        stop_test_app();
        result
    })
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let first = control.call(
        "createDisplay",
        json!({ "sessionKey": "small", "width": 1280, "height": 800 }),
    )?;
    let id = first["displayId"].as_u64().context("display id")?;
    control.call(
        "endDisplay",
        json!({ "sessionKey": "small", "displayId": id }),
    )?;
    // The sweep checks emptiness off the main thread before retaining it.
    sleep_ms(3000);
    let second = control.call(
        "createDisplay",
        json!({ "sessionKey": "tall", "width": 1280, "height": 1600 }),
    );
    check(
        "a taller request reuses the spare instead of rejecting or replacing it",
        second
            .as_ref()
            .is_ok_and(|r| r["displayId"].as_u64() == Some(id)),
        format!("first {id}, second {second:?}"),
    );
    let second = second?;
    let bounds = probe(&["bounds", &id.to_string()])?;
    let values: Vec<f64> = bounds
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    check(
        "the reused display runs at the requested larger size",
        values.len() == 4
            && values[2..] == [1280.0, 1600.0]
            && second["height"].as_u64() == Some(1600),
        bounds,
    );
    Ok(())
}
