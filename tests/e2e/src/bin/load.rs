//! What agent displays cost the Mac: CPU of WindowServer, replayd and
//! Agent Screen, and the whole Mac's load, with none and then with several
//! displays, each showing a Chrome page with a blinking caret.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e load
//!
//! LOAD_DISPLAYS sets how many (12), LOAD_SECONDS how long each sample runs
//! (20). Then every agent types and takes the screenshot after, all at
//! once, while a plain AppKit app's main thread is timed: the stutter the
//! user's apps see. Uses the Electron fixture when Chrome is the user's.
//! AGENTSCREEN_DISPLAY_HZ and AGENTSCREEN_DISPLAY_SCALE pass through to
//! the test build, to compare refresh rates and 1x with 2x. Skips when
//! Chrome is running with windows; quits the copies it started.

use std::process::{Command, ExitCode};

use agent_screen_tests::{
    Control, fixture, json, out, quit_profile, run, running_headed, sh_lenient, skip, sleep_ms,
    start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    // Chrome when it is free; else the Electron fixture, a Chromium page too.
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    let use_electron = running_headed("Google Chrome");
    if use_electron && !electron.exists() {
        skip("Chrome is running and there is no Electron.app; quit Chrome to measure");
    }
    let count: usize = env("LOAD_DISPLAYS", 12);
    let seconds: u64 = env("LOAD_SECONDS", 20);
    let pass: Vec<(String, String)> = [
        "AGENTSCREEN_DISPLAY_HZ",
        "AGENTSCREEN_DISPLAY_SCALE",
        "AGENTSCREEN_RECORD_SCALE",
    ]
    .iter()
    .filter_map(|n| std::env::var(n).ok().map(|v| (n.to_string(), v)))
    .collect();
    run(|| {
        let mut app_env: Vec<(&str, &str)> = vec![
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_NEW_DISPLAYS", "32"),
        ];
        app_env.extend(pass.iter().map(|(n, v)| (n.as_str(), v.as_str())));
        start_test_app(&app_env)?;
        let result = measure(count, seconds, use_electron.then_some(electron.as_path()));
        for n in 0..count {
            quit_profile(&out().join(format!("load-{n}-profile")));
        }
        if use_electron {
            sh_lenient("/usr/bin/pkill", &["-f", "fixtures/electron"]);
        }
        stop_test_app();
        result
    })
}

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// CPU % of the processes that draw and capture displays, and of the
/// whole Mac, over `seconds`, from top's second sample.
fn sample(label: &str, seconds: u64) -> Result<()> {
    let script = format!(
        "top -l 2 -s {seconds} -stats cpu,command -n 400 | awk '/Processes:/{{n++}} n==2 && $1+0==$1 {{all+=$1; c[$2]+=$1}} END {{printf \"all %.0f%%, WindowServer %.0f%%, replayd %.0f%%, Agent Screen %.0f%%, kernel_task %.0f%%\", all, c[\"WindowServer\"], c[\"replayd\"], c[\"AgentScreen\"], c[\"kernel_task\"]}}'; printf '; load '; sysctl -n vm.loadavg"
    );
    // How late a plain AppKit app's main thread runs meanwhile, as the
    // user's apps would: the stutter they see.
    let lag_tool = tool("lag")?;
    let lagging = std::thread::spawn(move || sh_lenient(&lag_tool, &[&seconds.to_string()]));
    let output = Command::new("/bin/sh").args(["-c", &script]).output()?;
    let lag: serde_json::Value =
        serde_json::from_str(lagging.join().unwrap_or_default().trim()).unwrap_or_default();
    println!(
        "{label}: {}; other app late: max {} ms, {} times over 50 ms, {} ms in all",
        String::from_utf8_lossy(&output.stdout).trim(),
        lag["max"],
        lag["over50"],
        lag["stalled"]
    );
    Ok(())
}

fn measure(count: usize, seconds: u64, electron: Option<&std::path::Path>) -> Result<()> {
    sample("no displays", seconds)?;
    let page = format!("file://{}", fixture("web/signup.html").display());
    let mut agents = vec![];
    let mut fields = vec![];
    for n in 0..count {
        let control = Control::connect()?;
        let key = format!("load-{n}");
        let display = control.call(
            "createDisplay",
            json!({ "sessionKey": key, "client": "load", "title": format!("Load {n}"), "width": 1100, "height": 700 }),
        )?;
        let id = display["displayId"].as_u64().context("display id")?;
        let launched = match electron {
            Some(electron) => control.call(
                "launchApp",
                json!({ "sessionKey": key, "displayId": id, "app": electron.to_str(),
                        "arguments": [fixture("electron").to_str()] }),
            )?,
            None => {
                let profile = out().join(format!("load-{n}-profile"));
                let _ = std::fs::remove_dir_all(&profile);
                control.call(
                    "launchApp",
                    json!({ "sessionKey": key, "displayId": id, "app": "Google Chrome", "arguments": [
                        format!("--user-data-dir={}", profile.display()),
                        "--no-first-run", "--no-default-browser-check", "--new-window", page,
                    ] }),
                )?
            }
        };
        // The page's text field: the sign-up form's in Chrome, the
        // harness's, near the bottom left, in Electron.
        let w = &launched["window"];
        let field = match electron {
            Some(_) => (
                w["x"].as_f64().unwrap_or(0.0) + 60.0,
                w["y"].as_f64().unwrap_or(0.0) + w["height"].as_f64().unwrap_or(480.0) - 23.0,
            ),
            None => (550.0, 330.0),
        };
        fields.push(field);
        agents.push((control, key, id));
    }
    // A focused field, so each page blinks its caret as an agent's would.
    sleep_ms(3000);
    for ((control, key, id), (x, y)) in agents.iter().zip(&fields) {
        let _ = control.call(
            "click",
            json!({ "sessionKey": key, "displayId": id, "x": x, "y": y }),
        );
    }
    sleep_ms(3000);
    sample(&format!("{count} displays, idle pages"), seconds)?;
    // How long an agent waits to read its window, alone, then while every
    // display records, as the fleet's do.
    let time_state = |label: &str| -> Result<()> {
        let mut times = vec![];
        for (control, key, id) in agents.iter().take(3) {
            let list =
                control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
            let window = list["windows"][0]["windowId"].as_u64().unwrap_or(0);
            let started = std::time::Instant::now();
            control.call(
                "windowState",
                json!({ "sessionKey": key, "displayId": id, "windowId": window }),
            )?;
            times.push(started.elapsed().as_millis());
        }
        println!("windowState {label}: {times:?} ms");
        Ok(())
    };
    time_state("with no recording")?;
    for (control, key, id) in &agents {
        control.call("recordStart", json!({ "sessionKey": key, "displayId": id }))?;
    }
    sleep_ms(2000);
    time_state("while every display records")?;
    for (control, key, id) in &agents {
        let _ = control.call("recordStop", json!({ "sessionKey": key, "displayId": id }));
    }
    // Every agent at work at once: a key, then the screenshot once its
    // display is still, over and over.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let workers: Vec<_> = agents
        .into_iter()
        .map(|(control, key, id)| {
            let stop = stop.clone();
            std::thread::spawn(move || -> Vec<u128> {
                let mut steps = vec![];
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let at = std::time::Instant::now();
                    let typed = control.call(
                        "type",
                        json!({ "sessionKey": key, "displayId": id, "text": "a" }),
                    );
                    let shot = control.call(
                        "screenshot",
                        json!({ "sessionKey": key, "displayId": id, "settle": true }),
                    );
                    if typed.is_err() || shot.is_err() {
                        break;
                    }
                    steps.push(at.elapsed().as_millis());
                }
                steps
            })
        })
        .collect();
    sleep_ms(1000);
    sample(&format!("{count} displays, agents working"), seconds)?;
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut steps: Vec<u128> = workers
        .into_iter()
        .flat_map(|w| w.join().unwrap_or_default())
        .collect();
    steps.sort();
    let at = |q: f64| {
        steps
            .get(((steps.len() as f64 * q) as usize).min(steps.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    println!(
        "agent steps while all work: {} steps, median {} ms, p90 {} ms, max {} ms",
        steps.len(),
        at(0.5),
        at(0.9),
        at(1.0)
    );
    Ok(())
}
