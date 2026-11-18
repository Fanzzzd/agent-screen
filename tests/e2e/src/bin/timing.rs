//! How long createDisplay takes to answer, median and p90 over 10 displays,
//! and launchApp for TextEdit, cold and already running, over 5 launches.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e timing    beside the user's app
//!     cargo e2e timing                            in its place, once the user is idle

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    Control, env_count, instance, json, quit_discarding, reopen_installed, restore, run, running,
    sleep_ms, snapshot, start_debug_app, start_test_app, stop_debug_app, stop_test_app,
    wait_for_quiet, wait_for_unlocked,
};
use anyhow::{Context, Result};

fn report(what: &str, values: &[i64]) {
    let mut sorted = values.to_vec();
    sorted.sort();
    let at = |q: f64| {
        let i = ((q * sorted.len() as f64).ceil() as usize).min(sorted.len());
        json!(i.checked_sub(1).and_then(|i| sorted.get(i)))
    };
    let all: Vec<String> = values.iter().map(i64::to_string).collect();
    println!(
        "{what}: median {}, p90 {} ({})",
        at(0.5),
        at(0.9),
        all.join(" ")
    );
}

fn elapsed_ms(start: Instant) -> i64 {
    (start.elapsed().as_secs_f64() * 1000.0).round() as i64
}

fn create(control: &Control, session: &str) -> Result<u64> {
    control.call(
        "createDisplay",
        json!({ "sessionKey": session, "title": "Timing" }),
    )?["displayId"]
        .as_u64()
        .context("display id")
}

fn main() -> ExitCode {
    let ready = if instance().is_some() {
        wait_for_unlocked()
    } else {
        wait_for_quiet()
    };
    if !ready {
        return ExitCode::FAILURE;
    }
    let before = instance().is_none().then(snapshot);
    run(|| {
        if instance().is_some() {
            start_test_app(&[])?;
        } else {
            start_debug_app(&[])?;
        }
        let control = Control::connect()?;
        let result = measure(&control);
        drop(control);
        match &before {
            None => stop_test_app(),
            Some(before) => {
                sleep_ms(800);
                stop_debug_app();
                reopen_installed();
                restore(before);
            }
        }
        result
    })
}

fn measure(control: &Control) -> Result<()> {
    let mut times = Vec::new();
    let runs = env_count("RUNS", 10.0);
    let mut i = 0;
    while (i as f64) < runs {
        let start = Instant::now();
        let id = create(control, &format!("timing-{i}"))?;
        times.push(elapsed_ms(start));
        control.call("endDisplay", json!({ "displayId": id }))?;
        sleep_ms(1500);
        i += 1;
    }
    let aerospace = if running("AeroSpace") {
        "with AeroSpace"
    } else {
        "without AeroSpace"
    };
    report(&format!("createDisplay ms {aerospace}"), &times);

    if running("TextEdit") {
        println!("skip launchApp: TextEdit is the user's");
        return Ok(());
    }
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    let launches = env_count("LAUNCHES", 5.0);
    let mut i = 0;
    while (i as f64) < launches {
        let id = create(control, &format!("launch-{i}"))?;
        for into in [&mut cold, &mut warm] {
            let start = Instant::now();
            control.call("launchApp", json!({ "displayId": id, "app": "TextEdit" }))?;
            into.push(elapsed_ms(start));
        }
        quit_discarding("TextEdit");
        sleep_ms(1000);
        control.call("endDisplay", json!({ "displayId": id }))?;
        sleep_ms(1500);
        i += 1;
    }
    report(&format!("launchApp TextEdit cold ms {aerospace}"), &cold);
    report(&format!("launchApp TextEdit running ms {aerospace}"), &warm);
    Ok(())
}
