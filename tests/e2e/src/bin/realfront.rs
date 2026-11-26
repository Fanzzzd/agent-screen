//! A click with your real pointer in an agent's window of an app that takes
//! AXFrontmost and stays behind (#255), as Dia and Electron apps do. The
//! loan must bring the app forward another way, or the click fails with
//! "the app didn't come to the front". `diarealclick` runs the same in
//! your own Dia.
//!
//! The Electron harness in HARNESS_PAIR mode stands in for the browser:
//! its user window on your main screen, its agent window on the Agent
//! Display, handed over as a launch would. A realInput click on the
//! agent's press button, allowed as the card's Allow does, runs twice:
//! once while another app is in front, and once while the harness is in
//! front with its user window key. Each must reach the agent's page with
//! route real_input. Afterwards the app that was in front is back, the
//! user window keeps its frame, AeroSpace shows the workspaces it showed,
//! and with the harness in front its user window is key again. The Red
//! Glow is Live while the loan has the pointer, and off once the session
//! is done.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e realfront
//!
//! Puts a small window and the front on your main screen for about half a
//! minute, and moves your pointer for a second, twice: keep your hands off
//! the Mac. Makes 1 display.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, cut, fixture, fronts_during, instance, json,
    locked, out, own_input_now, probe, run, sh_lenient, skip, sleep_ms, start_test_app,
    stop_test_app,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "realfront";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    if locked() {
        skip("the screen is locked, and this test moves the real pointer");
    }
    // Before the sign shows, which may take the front.
    let Some(original) = front_pid() else {
        skip("no front app");
    };
    let Some(finder) = sh_lenient("/usr/bin/pgrep", &["-x", "Finder"])
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
    else {
        skip("Finder isn't running");
    };
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Real front", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let log = out().join("realfront-keys.log");
        let _ = std::fs::write(&log, "");
        let mut harness = None;
        let result = test(&control, &id, &electron, &log, finder, &mut harness);
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
        if let Some(pid) = harness {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.to_string())
                .status();
        }
        if front_pid() != Some(original) {
            let _ = probe(&["activate", &original.to_string()]);
            sleep_ms(500);
        }
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        stop_test_app();
        check(
            "the app you were in is in front again",
            front_pid() == Some(original),
            format!("front {:?}, yours {original}", front_pid()),
        );
        result
    })
}

fn test(
    control: &Control,
    id: &Value,
    electron: &std::path::Path,
    log: &std::path::Path,
    other: i32,
    harness: &mut Option<i32>,
) -> Result<()> {
    allow_on_your_screen(
        "Electron",
        60_000,
        "the harness's user window stands in for yours",
    );
    sh_lenient(
        "/usr/bin/open",
        &[
            "-n",
            "-g",
            "--env",
            "HARNESS_PAIR=1",
            "--env",
            &format!("HARNESS_DISPLAY={id}"),
            "--env",
            &format!("HARNESS_KEY_LOG={}", log.display()),
            "-a",
            electron.to_str().unwrap_or_default(),
            "--args",
            fixture("electron").to_str().unwrap_or_default(),
        ],
    );
    let mut window = None;
    for _ in 0..60 {
        if lines(log).iter().any(|l| l.starts_with("ready ")) {
            window = agent_window(control, id)?;
            if window.is_some() {
                break;
            }
        }
        sleep_ms(250);
    }
    let pid: i32 = lines(log)
        .iter()
        .find_map(|l| l.strip_prefix("ready ").and_then(|p| p.parse().ok()))
        .context("the harness's log names no pid")?;
    *harness = Some(pid);
    let window = window.context("the harness's agent window didn't show on the display")?;
    let window_id = window["windowId"].as_u64().context("windowId")?;
    control.call(
        "debugHandOver",
        json!({ "displayId": id, "windowId": window_id }),
    )?;
    sleep_ms(800);
    let state = control.call(
        "windowState",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window_id }),
    )?;
    let tree = state["tree"].as_str().unwrap_or_default();
    // Left of the label: there Accessibility names the button itself, and
    // on the label its text, which the loan's check would call a change.
    let (x, y) = element(tree, "agent press")?;
    let point = (x - 60, y);
    let user_frame = || {
        windows(pid)
            .into_iter()
            .filter(|(w, _)| *w != window_id)
            .collect::<Vec<_>>()
    };

    for round in ["another app in front", "the harness in front"] {
        let front = if round.starts_with("the harness") {
            pid
        } else {
            other
        };
        // Brought forward as the user switching would; the harness's user
        // window is key the first time it comes forward.
        sh_lenient(
            "/usr/bin/osascript",
            &[
                "-e",
                &format!(
                    "tell application \"System Events\" to set frontmost of (first process whose unix id is {front}) to true"
                ),
            ],
        );
        let started = Instant::now();
        while front_pid() != Some(front) && started.elapsed() < Duration::from_secs(3) {
            sleep_ms(100);
        }
        sleep_ms(800);
        if front_pid() != Some(front) {
            bail!(
                "{round}: pid {front} didn't come forward: front {:?}",
                front_pid()
            );
        }
        let key_before = last_key(log);
        let frames = user_frame();
        let spaces = aerospace_visible();
        let from = lines(log).len();
        let call = control.begin(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": point.0, "y": point.1,
                    "button": "left", "count": 1, "realInput": true }),
        )?;
        sleep_ms(800);
        let glows = std::sync::Mutex::new(Vec::<String>::new());
        let (reply, fronts) = fronts_during(6, None, || {
            // The loan moves and clicks your pointer: that's the script's
            // input, not yours.
            let loaning = std::sync::atomic::AtomicBool::new(true);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    while loaning.load(std::sync::atomic::Ordering::Relaxed) {
                        own_input_now();
                        sleep_ms(100);
                    }
                });
                // The glow's state through the loan, on a second connection.
                scope.spawn(|| {
                    let Ok(glow) = Control::connect() else {
                        return;
                    };
                    while loaning.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Ok(g) = glow.call("debugGlow", json!({ "alpha": false }))
                            && let Some(state) = g["state"].as_str()
                        {
                            let mut seen = glows.lock().unwrap();
                            if seen.last().map(String::as_str) != Some(state) {
                                seen.push(state.to_string());
                            }
                        }
                        sleep_ms(30);
                    }
                });
                let _ = control.call(
                    "debugShow",
                    json!({ "sessionKey": KEY, "surface": "input-queue-allow", "displayId": id }),
                );
                let reply = call.wait();
                // The pointer and front app come back after the reply.
                sleep_ms(1500);
                loaning.store(false, std::sync::atomic::Ordering::Relaxed);
                reply
            })
        })?;
        let page: Vec<String> = lines(log).into_iter().skip(from).collect();
        check(
            &format!("{round}: the real click reaches the agent's page with route real_input"),
            reply.as_ref().is_ok_and(|r| r["route"] == "real_input")
                && page.iter().any(|l| l.starts_with("pair agent click")),
            format!("{} | page {page:?}", cut(&said(&reply), 400)),
        );
        println!("     {round}: fronts {fronts:?}");
        check(
            &format!("{round}: the app that was in front is in front again"),
            front_pid() == Some(front),
            format!("front {:?}, was {front}", front_pid()),
        );
        let frames_now = user_frame();
        check(
            &format!("{round}: the harness's user window keeps its frame"),
            frames_now == frames,
            format!("{frames:?} -> {frames_now:?}"),
        );
        let spaces_now = aerospace_visible();
        check(
            &format!("{round}: AeroSpace shows the workspaces it showed"),
            spaces_now == spaces,
            format!("{spaces:?} -> {spaces_now:?}"),
        );
        if front == pid {
            let key_now = last_key(log);
            check(
                &format!("{round}: the harness's user window is key again"),
                key_before.as_deref() == Some("win user focus")
                    && key_now.as_deref() == Some("win user focus"),
                format!("{key_before:?} -> {key_now:?} | {page:?}"),
            );
        }
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
        sleep_ms(1000);
        let after = control
            .call("debugGlow", json!({ "alpha": false }))
            .map(|g| g["state"].as_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        let seen = glows.into_inner().unwrap_or_default();
        check(
            &format!(
                "{round}: the Red Glow is Live while the loan runs, and off once the session is done"
            ),
            seen.iter().any(|s| s == "live") && after == "off",
            format!("seen {seen:?}, after done {after}"),
        );
    }
    Ok(())
}

fn front_pid() -> Option<i32> {
    probe(&["frontpid"]).ok()?.trim().parse().ok()
}

/// The harness's log, each line without its time.
fn lines(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_string())
        .collect()
}

/// The last focus change of either window, as "win user focus".
fn last_key(log: &std::path::Path) -> Option<String> {
    lines(log)
        .into_iter()
        .rev()
        .find(|l| l.starts_with("win user") || l.starts_with("win agent"))
}

fn agent_window(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["title"] == "pair agent")
            .cloned()
    }))
}

/// The middle of a labelled element in a window state's tree.
fn element(tree: &str, label: &str) -> Result<(i64, i64)> {
    let line = tree
        .lines()
        .find(|l| l.contains(&format!("\"{label}\"")))
        .with_context(|| format!("no {label} in the tree:\n{}", cut(tree, 2000)))?;
    let at = line
        .split(" at (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .with_context(|| format!("no position in {line}"))?;
    let (x, y) = at.split_once(',').context("x,y")?;
    Ok((x.trim().parse()?, y.trim().parse()?))
}

/// AeroSpace's visible workspaces and its focused one, empty without it.
fn aerospace_visible() -> String {
    let visible = sh_lenient(
        "/opt/homebrew/bin/aerospace",
        &["list-workspaces", "--monitor", "all", "--visible"],
    );
    let focused = sh_lenient(
        "/opt/homebrew/bin/aerospace",
        &["list-workspaces", "--focused"],
    );
    format!(
        "visible {} focused {}",
        visible.split_whitespace().collect::<Vec<_>>().join(","),
        focused.trim()
    )
}

/// The app's normal windows on screen, each with its frame.
fn windows(pid: i32) -> Vec<(u64, String)> {
    probe(&["windows", &pid.to_string()])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|w| w["layer"] == 0)
        .filter_map(|w| {
            let frame = format!("{},{},{},{}", w["x"], w["y"], w["width"], w["height"]);
            Some((w["id"].as_u64()?, frame))
        })
        .collect()
}

/// An answer, or the error's text.
fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    }
}
