//! An interactive probe of Dia's own instance on one display (#236), for
//! verifying a browser before it goes on `agent_browser::VERIFIED`. It
//! holds one control connection and one display, and runs each line
//! written to the FIFO `$E2E_PROBE/in` as `<method> <json>`, with `$ID`
//! replaced by the display id. `shot <name>` captures the display, `quit`
//! ends it. Meanwhile it prints every window of a Dia process it didn't
//! find running that shows off the display, and every front app change.
//!
//!     E2E_PROBE=/tmp/dia-probe cargo e2e diaprobe

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use agent_screen_tests::{
    Control, capture, instance, json, probe, sh_lenient, sleep_ms, start_test_app, stop_test_app,
    windows_of,
};
use anyhow::{Context, Result};

const KEY: &str = "diaprobe";

fn pids() -> Vec<u32> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Dia"])
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn front_pid() -> Option<u32> {
    probe(&["frontpid"]).ok()?.trim().parse().ok()
}

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            println!("probe failed: {e:#}");
            stop_test_app();
            ExitCode::FAILURE
        }
    }
}

fn serve() -> Result<()> {
    let dir = std::env::var("E2E_PROBE").context("set E2E_PROBE")?;
    let fifo = format!("{dir}/in");
    start_test_app(&[
        ("AGENTSCREEN_LANG", "en"),
        ("AGENTSCREEN_NO_SHARED_FALLBACK", "1"),
        ("AGENTSCREEN_VERIFY_BROWSER", "company.thebrowser.dia"),
    ])?;
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Dia probe", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let said = probe(&["bounds", &id.to_string()])?;
    let v: Vec<f64> = said
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    let [x, y, w, h]: [f64; 4] = v.try_into().map_err(|_| anyhow::anyhow!("bounds {said}"))?;
    let theirs = pids();
    let start = Instant::now();
    println!("display {id} at {x},{y} {w}x{h}; your Dia {theirs:?}; front {:?}", front_pid());
    let stop = Arc::new(AtomicBool::new(false));
    let watch = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut front = front_pid();
            let mut seen = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                for pid in pids().into_iter().filter(|p| !theirs.contains(p)) {
                    for f in windows_of(pid) {
                        let inside = f.x >= x - 1.0
                            && f.y >= y - 1.0
                            && f.x + f.width <= x + w + 1.0
                            && f.y + f.height <= y + h + 1.0;
                        let line = format!(
                            "pid {pid} window {} layer {} at {},{} {}x{}",
                            f.id, f.layer, f.x, f.y, f.width, f.height
                        );
                        if !inside && !seen.contains(&line) {
                            println!("{:>7} ms OFF DISPLAY {line}", start.elapsed().as_millis());
                            seen.push(line);
                        }
                    }
                }
                let now = front_pid();
                if now != front {
                    println!("{:>7} ms FRONT {front:?} -> {now:?}", start.elapsed().as_millis());
                    front = now;
                }
                sleep_ms(30);
            }
        })
    };
    loop {
        // Opening the FIFO blocks until a writer comes.
        let Ok(file) = std::fs::File::open(&fifo) else {
            break;
        };
        let mut quit = false;
        for line in std::io::BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            let line = line.trim().replace("$ID", &id.to_string());
            if line.is_empty() {
                continue;
            }
            let at = start.elapsed().as_millis();
            let (method, rest) = line.split_once(' ').unwrap_or((&line, "{}"));
            match method {
                "quit" => {
                    quit = true;
                    break;
                }
                "shot" => match capture(rest, [x, y, w, h]) {
                    Ok(file) => println!("{at:>7} ms shot {}", file.display()),
                    Err(e) => println!("{at:>7} ms shot failed: {e:#}"),
                },
                "windows" => {
                    for pid in pids() {
                        println!("{at:>7} ms pid {pid}: {:?}", windows_of(pid));
                    }
                }
                _ => {
                    let params = serde_json::from_str(rest).unwrap_or(json!({}));
                    let answer = control.call(method, params);
                    let text = match answer {
                        Ok(v) => v.to_string(),
                        Err(e) => format!("error: {e:#}"),
                    };
                    let cut: String = text.chars().take(6000).collect();
                    println!("{at:>7} ms {method} -> {cut}");
                }
            }
            let _ = std::io::stdout().flush();
        }
        if quit {
            break;
        }
    }
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    sleep_ms(1500);
    stop_test_app();
    sleep_ms(3000);
    stop.store(true, Ordering::SeqCst);
    let _ = watch.join();
    println!("probe ended");
    Ok(())
}
