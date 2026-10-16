//! What agent displays cost the Mac: CPU of WindowServer, replayd and
//! Sidescreen, and the whole Mac's load, with none and then with several
//! displays, each showing a Chrome page with a blinking caret.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e load
//!
//! LOAD_DISPLAYS sets how many (12), LOAD_SECONDS how long each sample runs
//! (20). SIDESCREEN_DISPLAY_HZ and SIDESCREEN_DISPLAY_SCALE pass through to
//! the test build, to compare refresh rates and 1x with 2x. Skips when
//! Chrome is running with windows; quits the copies it started.

use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use sidescreen_tests::{
    Control, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms, start_test_app,
    stop_test_app,
};

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to measure");
    }
    let count: usize = env("LOAD_DISPLAYS", 12);
    let seconds: u64 = env("LOAD_SECONDS", 20);
    let pass: Vec<(String, String)> = ["SIDESCREEN_DISPLAY_HZ", "SIDESCREEN_DISPLAY_SCALE"]
        .iter()
        .filter_map(|n| std::env::var(n).ok().map(|v| (n.to_string(), v)))
        .collect();
    run(|| {
        let mut app_env: Vec<(&str, &str)> =
            vec![("SIDESCREEN_LANG", "en"), ("SIDESCREEN_NEW_DISPLAYS", "32")];
        app_env.extend(pass.iter().map(|(n, v)| (n.as_str(), v.as_str())));
        start_test_app(&app_env)?;
        let result = measure(count, seconds);
        for n in 0..count {
            quit_profile(&out().join(format!("load-{n}-profile")));
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
        "top -l 2 -s {seconds} -stats cpu,command -n 400 | awk '/Processes:/{{n++}} n==2 && $1+0==$1 {{all+=$1; c[$2]+=$1}} END {{printf \"all %.0f%%, WindowServer %.0f%%, replayd %.0f%%, Sidescreen %.0f%%, kernel_task %.0f%%\", all, c[\"WindowServer\"], c[\"replayd\"], c[\"Sidescreen\"], c[\"kernel_task\"]}}'; printf '; load '; sysctl -n vm.loadavg"
    );
    let output = Command::new("/bin/sh").args(["-c", &script]).output()?;
    println!(
        "{label}: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(())
}

fn measure(count: usize, seconds: u64) -> Result<()> {
    sample("no displays", seconds)?;
    let page = format!("file://{}", fixture("web/signup.html").display());
    let mut agents = vec![];
    for n in 0..count {
        let control = Control::connect()?;
        let key = format!("load-{n}");
        let display = control.call(
            "createDisplay",
            json!({ "sessionKey": key, "client": "load", "title": format!("Load {n}"), "width": 1100, "height": 700 }),
        )?;
        let id = display["displayId"].as_u64().context("display id")?;
        let profile = out().join(format!("load-{n}-profile"));
        let _ = std::fs::remove_dir_all(&profile);
        control.call(
            "launchApp",
            json!({ "sessionKey": key, "displayId": id, "app": "Google Chrome", "arguments": [
                format!("--user-data-dir={}", profile.display()),
                "--no-first-run", "--no-default-browser-check", "--new-window", page,
            ] }),
        )?;
        agents.push((control, key, id));
    }
    // A focused field, so each page blinks its caret as an agent's would.
    sleep_ms(3000);
    for (control, key, id) in &agents {
        let _ = control.call(
            "click",
            json!({ "sessionKey": key, "displayId": id, "x": 550, "y": 330 }),
        );
    }
    sleep_ms(3000);
    sample(&format!("{count} displays, idle pages"), seconds)?;
    Ok(())
}
