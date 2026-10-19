//! What agents and the user wait for, in numbers a change can be judged by.
//! ROUNDS times (3): a new display of a size no spare has, a reused spare,
//! Calculator launched on it, 10 clicks, 5 screenshots and 3 window states,
//! then the display ends. Meanwhile `lag`, a plain AppKit app, records how
//! late its main thread runs, as the user's apps would, and the test app's
//! own stall lines come from its displays log.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e perf
//!
//! Prints one line per measure and writes them all as JSON to
//! target/e2e/perf/PERF_LABEL-TIME.json (PERF_LABEL defaults to the commit).
//! Skips when Calculator is running with windows; quits the one it started.

use std::process::ExitCode;
use std::thread;
use std::time::Instant;

use agent_screen_tests::{
    Control, env_count, instance, json, now_ms, out, probe, run, running_headed, sh_lenient, skip,
    sleep_ms, start_test_app, stop_test_app, tool, try_osa,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running_headed("Calculator") {
        skip("Calculator is running; quit it to measure");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = measure();
        drop(try_osa("tell application \"Calculator\" to quit"));
        stop_test_app();
        result
    })
}

fn ms(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 10000.0).round() / 10.0
}

/// Median and p90 of `values`, and every value.
fn spread(values: &[f64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| {
        let i = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len().max(1));
        sorted.get(i - 1).copied().unwrap_or(0.0)
    };
    json!({ "median": at(0.5), "p90": at(0.9), "max": at(1.0), "all": values })
}

fn log_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!(
        "Library/Logs/Agent Screen/displays-{}.log",
        instance().unwrap_or_default()
    ))
}

fn measure() -> Result<()> {
    let rounds = env_count("ROUNDS", 3.0) as usize;
    // Room for every round, at about 20 s a round.
    let seconds = 15 + rounds * 25;
    let lag_tool = tool("lag")?;
    let lagging = thread::spawn(move || sh_lenient(&lag_tool, &[&seconds.to_string()]));
    let counting = {
        let seconds = seconds.to_string();
        thread::spawn(move || probe(&["reconfigurations", &seconds]))
    };
    let log_start = std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0);
    let online_before = probe(&["units"])?.lines().count();
    let started = Instant::now();
    sleep_ms(1500);

    let control = Control::connect()?;
    let (mut fresh, mut reused, mut cold, mut warm) = (vec![], vec![], vec![], vec![]);
    let (mut clicks, mut shots, mut states, mut ends) = (vec![], vec![], vec![], vec![]);
    for round in 0..rounds {
        let key = format!("perf-{round}");
        // A size no spare has, so the display is new.
        let width = 1100 + 8 * ((now_ms() / 1000 % 50) as u64) + round as u64;
        let at = Instant::now();
        let id = create(&control, &key, width)?;
        fresh.push(ms(at));
        sleep_ms(2500);

        let at = Instant::now();
        control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
        ends.push(ms(at));
        sleep_ms(1000);
        let at = Instant::now();
        let again = create(&control, &key, width)?;
        reused.push(ms(at));

        let at = Instant::now();
        let launched = control.call(
            "launchApp",
            json!({ "sessionKey": key, "displayId": again, "app": "Calculator" }),
        )?;
        if round == 0 { &mut cold } else { &mut warm }.push(ms(at));
        let window = &launched["window"];
        let window_id = window["windowId"].as_u64().context("Calculator window")?;
        let num = |k: &str| window[k].as_f64().unwrap_or(0.0);
        let (x, y) = (
            num("x") + num("width") / 2.0,
            num("y") + num("height") * 0.7,
        );
        sleep_ms(800);

        for _ in 0..10 {
            let at = Instant::now();
            control.call(
                "click",
                json!({ "sessionKey": key, "displayId": again, "x": x, "y": y }),
            )?;
            clicks.push(ms(at));
            sleep_ms(150);
        }
        for _ in 0..5 {
            let at = Instant::now();
            control.call(
                "screenshot",
                json!({ "sessionKey": key, "displayId": again }),
            )?;
            shots.push(ms(at));
        }
        for _ in 0..3 {
            let at = Instant::now();
            control.call(
                "windowState",
                json!({ "sessionKey": key, "displayId": again, "windowId": window_id }),
            )?;
            states.push(ms(at));
        }
        drop(try_osa("tell application \"Calculator\" to quit"));
        sleep_ms(1000);
        let at = Instant::now();
        control.call(
            "endDisplay",
            json!({ "sessionKey": key, "displayId": again }),
        )?;
        ends.push(ms(at));
        sleep_ms(3000);
    }
    drop(control);
    let busy = started.elapsed().as_secs();

    let lag: serde_json::Value = serde_json::from_str(
        lagging
            .join()
            .map_err(|_| anyhow::anyhow!("lag panicked"))?
            .trim(),
    )
    .context("lag output")?;
    let reconfigurations: u64 = counting
        .join()
        .map_err(|_| anyhow::anyhow!("probe panicked"))??
        .trim()
        .parse()
        .context("count")?;
    let log = std::fs::read(log_path()).unwrap_or_default();
    let tail = String::from_utf8_lossy(&log[(log_start as usize).min(log.len())..]).to_string();
    let main_busy: Vec<f64> = tail
        .lines()
        .filter_map(|l| l.split("main thread busy ").nth(1))
        .filter_map(|r| r.split(' ').next()?.parse().ok())
        .collect();

    let result = json!({
        "label": std::env::var("PERF_LABEL").unwrap_or_else(|_| sh_lenient("/usr/bin/git", &["rev-parse", "--short", "HEAD"])),
        "rounds": rounds,
        "seconds": busy,
        "displaysOnlineAtStart": online_before,
        "loadavg": sh_lenient("/usr/sbin/sysctl", &["-n", "vm.loadavg"]),
        "reconfigurations": reconfigurations,
        "createNew": spread(&fresh),
        "createReused": spread(&reused),
        "endDisplay": spread(&ends),
        "launchCold": spread(&cold),
        "launchRunning": spread(&warm),
        "click": spread(&clicks),
        "screenshot": spread(&shots),
        "windowState": spread(&states),
        "appMainBusy": { "count": main_busy.len(), "sum": main_busy.iter().sum::<f64>(), "max": main_busy.iter().cloned().fold(0.0, f64::max), "all": main_busy },
        "otherAppLag": { "max": lag["max"], "over50": lag["over50"], "over100": lag["over100"], "stalled": lag["stalled"] },
    });
    for (k, v) in result.as_object().unwrap() {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("all");
        }
        println!("{k}: {v}");
    }
    let dir = out().join("perf");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!(
        "{}-{}.json",
        result["label"].as_str().unwrap_or("run").trim(),
        now_ms() / 1000
    ));
    let mut full = result.clone();
    full["otherAppLag"] = lag;
    std::fs::write(&file, serde_json::to_string_pretty(&full)?)?;
    println!("wrote {}", file.display());
    Ok(())
}

fn create(control: &Control, key: &str, width: u64) -> Result<u64> {
    control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Perf", "width": width, "height": 760 }),
    )?["displayId"]
        .as_u64()
        .context("display id")
}
