//! The agents' own Chrome instance never takes the user's front app while
//! it starts (#241, ADR 0022). launch_app starts the instance cold
//! E2E_LAUNCHES times (20 by default) on one display, quitting it between
//! launches. Every front app change is recorded twice: by `probe fronts`,
//! notified and polled every 10 ms, and by the SETFRONT lines
//! launchservicesd writes to the unified log. No change may go to the
//! agents' instance.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e agentfront
//!
//! Needs Chrome not running: the instance is started on the test
//! instance's agents' profile, never yours. Makes 1 display.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use agent_screen_tests::{
    Control, check, instance, json, profile_pids, quit_profile, run, running_headed, sh_lenient,
    skip, sleep_ms, start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};

const KEY: &str = "agentfront";

fn agents_profile() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home)
        .join("Library/Application Support/Agent Screen")
        .join(format!(
            "Browser Profiles ({})",
            instance().unwrap_or_default()
        ))
        .join("com.google.Chrome")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// The instance's browser process: the one Agent Screen started with the
/// pipe, not its helpers.
fn main_pid(profile: &Path) -> Option<i32> {
    profile_pids(profile).into_iter().find(|&pid| {
        sh_lenient("/bin/ps", &["-o", "args=", "-p", &pid.to_string()])
            .contains("--remote-debugging-pipe")
    })
}

/// Quit the instance as its Quit command would, and wait until every
/// process on the profile has ended.
fn quit_instance(profile: &Path) {
    if let Some(pid) = main_pid(profile) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    for _ in 0..50 {
        if profile_pids(profile).is_empty() {
            return;
        }
        sleep_ms(100);
    }
    quit_profile(profile);
}

fn serve() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut line = String::new();
            let _ = BufReader::new(&stream).read_line(&mut line);
            let body = "<!doctype html><title>agentfront</title><h1>agentfront</h1>";
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    Ok(port)
}

/// Front app changes from `probe fronts`: epoch ms and pid.
type Fronts = Arc<Mutex<Vec<(u128, i32, String)>>>;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to start the agents' instance alone");
    }
    let launches: usize = std::env::var("E2E_LAUNCHES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let profile = agents_profile();
    run(|| {
        let port = serve()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        quit_instance(&profile);
        let result = test(launches, port, &profile);
        quit_instance(&profile);
        stop_test_app();
        result
    })
}

fn test(launches: usize, port: u16, profile: &Path) -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Agents' front", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let probe = tool("probe")?;
    // Generous: each launch takes about 4 s, and the quit after it 1 s.
    let seconds = (launches as u64 * 12 + 30).to_string();
    let mut child = Command::new(&probe)
        .args(["fronts", &seconds])
        .stdout(Stdio::piped())
        .spawn()?;
    let fronts: Fronts = Arc::default();
    {
        let fronts = fronts.clone();
        let stdout = child.stdout.take().context("probe stdout")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let mut parts = line.splitn(3, ' ');
                let (Some(at), Some(pid)) = (parts.next(), parts.next()) else {
                    continue;
                };
                let (Ok(at), Ok(pid)) = (at.parse(), pid.parse()) else {
                    continue;
                };
                let name = parts.next().unwrap_or("").to_string();
                fronts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((at, pid, name));
            }
        });
    }
    sleep_ms(500);
    let began = now_ms();
    let log_start = sh_lenient("/bin/date", &["+%Y-%m-%d %H:%M:%S"]);
    let user_front = fronts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .first()
        .map(|f| f.1);
    println!("front before: {user_front:?}");
    // Each launch: its index, the instance's pid, and when it ran.
    let mut runs: Vec<(usize, i32, u128, u128)> = Vec::new();
    let mut failures = Vec::new();
    for n in 0..launches {
        let from = now_ms();
        let started = Instant::now();
        let launched = control.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": "Google Chrome",
                    "open": format!("http://127.0.0.1:{port}/front?n={n}") }),
        );
        let answered = started.elapsed().as_millis();
        // An activation came about 100 ms after the window showed (#241):
        // give a late one time to show before the instance quits.
        sleep_ms(1500);
        let pid = main_pid(profile).unwrap_or(0);
        match &launched {
            Ok(v) if v["window"]["windowId"].as_u64().is_some() && v.get("shared").is_none() => {
                println!("launch {n}: pid {pid}, answered in {answered} ms");
            }
            other => {
                println!("launch {n}: no agents' window: {other:?}");
                failures.push(n);
            }
        }
        quit_instance(profile);
        runs.push((n, pid, from, now_ms()));
        sleep_ms(500);
    }
    sleep_ms(500);
    let _ = child.kill();
    let _ = child.wait();
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    let pids: Vec<i32> = runs.iter().map(|r| r.1).filter(|&p| p > 0).collect();
    let fronts = fronts.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let theirs: Vec<String> = fronts
        .iter()
        .filter(|f| f.0 >= began && pids.contains(&f.1))
        .map(|f| {
            let run = runs.iter().find(|r| r.1 == f.1);
            format!(
                "launch {} pid {} at {} ms into it",
                run.map_or(0, |r| r.0),
                f.1,
                run.map_or(0, |r| f.0.saturating_sub(r.2))
            )
        })
        .collect();
    let others: Vec<String> = fronts
        .iter()
        .filter(|f| f.0 >= began && !pids.contains(&f.1) && Some(f.1) != user_front)
        .map(|f| format!("{} {} at {}", f.1, f.2, f.0 - began))
        .collect();
    if !others.is_empty() {
        println!("other front changes, not counted: {others:?}");
    }
    // launchservicesd logs each front change with the pid it went to.
    let setfront = sh_lenient(
        "/usr/bin/log",
        &[
            "show",
            "--style",
            "compact",
            "--start",
            log_start.trim(),
            "--predicate",
            "subsystem == \"com.apple.processmanager\" AND eventMessage BEGINSWITH \"SETFRONT\"",
        ],
    );
    let logged: Vec<String> = setfront
        .lines()
        .filter(|l| {
            l.split_whitespace()
                .find_map(|w| w.strip_prefix("pid="))
                .and_then(|p| p.parse::<i32>().ok())
                .is_some_and(|p| pids.contains(&p))
        })
        .map(|l| l.chars().take(140).collect())
        .collect();
    println!("{} launches, instance pids {pids:?}", runs.len());
    check(
        "every launch answered with a window of the agents' instance",
        failures.is_empty(),
        format!("failed launches {failures:?}"),
    );
    check(
        "the agents' instance never came to the front, sampled every 10 ms and notified",
        theirs.is_empty(),
        format!("{} of {} launches: {theirs:?}", theirs.len(), runs.len()),
    );
    check(
        "launchservicesd logged no front change to the agents' instance",
        logged.is_empty(),
        format!("{logged:?}"),
    );
    Ok(())
}
