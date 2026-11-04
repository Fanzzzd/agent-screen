//! One browser, two windows (#110): the user works in one window of an
//! app while the agent works in another of its windows on its Agent
//! Display. The app is the user's front app, so posted keys reach its key
//! window, the user's, and a posted click would make the agent's window
//! key and take the user's typing.
//!
//! The Electron harness in HARNESS_PAIR mode stands in for the browser. It
//! starts with `open`, as an app the user started, so it has no DevTools
//! pipe. Its agent window opens on the display; its user window opens on
//! your main screen, and the script brings the app to the front, where it
//! makes that window key. Both pages log every key, input, focus and click
//! to a file.
//!
//! In that state: a click on the agent's field, `type`, Return, a click on
//! the agent's button, and Tab. Each either reaches the agent's page or
//! fails and names the user's window, and the user's window never loses
//! key focus or sees an event. Then, with your front app back, the same
//! harness takes posted keys as before.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//! Puts a window and the front on your main screen for about half a
//! minute; keep your hands off the keyboard, which would type into it.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e sharedbrowser
//!
//! Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, cut, fixture, instance, json, out, restore, run,
    sh_lenient, skip, sleep_ms, snapshot, start_test_app, stop_test_app, tool, wait_for_idle,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "sharedbrowser";

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
    if !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| {
        allow_on_your_screen(
            "Electron",
            120_000,
            "the harness's user window stands in for your own browser window, on your screen on purpose",
        );
        let before = snapshot();
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Shared browser", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let log = out().join("sharedbrowser-keys.log");
        let _ = std::fs::remove_file(&log);
        let result = test(&control, &id, &electron, &log, &before.front);
        quit_harness();
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        restore(&before);
        stop_test_app();
        result
    })
}

/// This checkout's harness only: another worktree may run its own.
fn quit_harness() {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &["-f", fixture("electron").to_str().unwrap_or_default()],
    );
    sleep_ms(1000);
}

/// The harness's log since `from` lines in.
fn lines(log: &std::path::Path, from: usize) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .skip(from)
        // Each line starts with the time in ms.
        .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_string())
        .collect()
}

fn has(lines: &[String], wanted: &str) -> bool {
    lines.iter().any(|l| l.starts_with(wanted))
}

/// The user's window lost key focus, or its page saw input.
fn user_touched(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| {
            l.starts_with("win user blur")
                || l.starts_with("win agent focus")
                || [
                    "keydown",
                    "keydown-page",
                    "input",
                    "submit",
                    "click",
                    "blur",
                    "page-blur",
                    "escape",
                ]
                .iter()
                .any(|e| {
                    l.starts_with(&format!("pair user {e} ")) || **l == format!("pair user {e}")
                })
        })
        .cloned()
        .collect()
}

fn agent_window(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["title"] == "pair agent")
            .cloned()
    }))
}

/// The line of a labelled element in a window state's tree, and its
/// middle.
fn element(tree: &str, label: &str) -> Result<(String, (i64, i64))> {
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
    Ok((line.to_string(), (x.trim().parse()?, y.trim().parse()?)))
}

fn tree(control: &Control, id: &Value, window: u64) -> Result<String> {
    let state = control.call(
        "windowState",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
    )?;
    Ok(state["tree"].as_str().unwrap_or_default().to_string())
}

fn front_app() -> String {
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            "tell application \"System Events\" to get name of first process whose frontmost is true",
        ],
    )
    .trim()
    .to_string()
}

/// An answer, or the error's text.
fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// Failed, naming the user's window.
fn names_users_window(reply: &Result<Value>) -> bool {
    reply
        .as_ref()
        .is_err_and(|e| e.to_string().contains("\"pair user\""))
}

fn test(
    control: &Control,
    id: &Value,
    electron: &std::path::Path,
    log: &std::path::Path,
    your_front: &str,
) -> Result<()> {
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
        if has(&lines(log, 0), "ready") {
            window = agent_window(control, id)?;
            if window.is_some() {
                break;
            }
        }
        sleep_ms(250);
    }
    let window = window.with_context(|| {
        format!(
            "the harness's agent window didn't show on the display; log: {:?}",
            lines(log, 0)
        )
    })?;
    let window_id = window["windowId"].as_u64().context("windowId")?;
    // Given to the display as the user would hand over a window of theirs,
    // so Agent Screen claims it and AeroSpace leaves it there once the app
    // comes to the front. The agent's windows of a running browser come
    // this way, or by launch_app.
    control.call(
        "debugHandOver",
        json!({ "displayId": id, "windowId": window_id }),
    )?;
    sleep_ms(800);
    // The user working in the harness: it comes to the front, and its
    // user window becomes key.
    let pid = lines(log, 0)
        .iter()
        .find_map(|l| l.strip_prefix("ready ").map(str::to_string))
        .context("the harness's log names no pid")?;
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!(
                "tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true"
            ),
        ],
    );
    sleep_ms(1000);
    if agent_window(control, id)?.is_none() {
        let electron_on_main: Vec<String> = sh_lenient(tool("onscreen")?, &["main"])
            .lines()
            .filter(|l| l.contains("\"Electron\""))
            .map(String::from)
            .collect();
        let aerospace = sh_lenient(
            "aerospace",
            &[
                "list-windows",
                "--all",
                "--format",
                "%{window-id}|%{app-name}|%{window-title}|%{workspace}|%{monitor-name}",
            ],
        );
        bail!(
            "the agent window left the display when the harness came to the front: on your main screen {electron_on_main:?}; AeroSpace: {:?}; log {:?}",
            aerospace
                .lines()
                .filter(|l| l.contains("Electron"))
                .collect::<Vec<_>>(),
            lines(log, 0)
        );
    }
    let start = lines(log, 0);
    let user_key = start
        .iter()
        .rev()
        .find(|l| l.starts_with("win user"))
        .is_some_and(|l| l == "win user focus");
    if !user_key || front_app() != "Electron" {
        bail!(
            "the harness isn't in front with its user window key, so the test can't begin: front {}, log {start:?}",
            front_app()
        );
    }
    let from = start.len();
    let tree0 = tree(control, id, window_id)?;
    let (_, field) = element(&tree0, "agent field")?;
    let (_, press) = element(&tree0, "agent press")?;
    let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });

    // The user's app is in front with their window key.
    let clicked = control.call("click", at(field));
    sleep_ms(300);
    println!(
        "     click on the agent's field: {}",
        cut(&said(&clicked), 400)
    );
    let typed = control.call(
        "type",
        json!({ "sessionKey": KEY, "displayId": id, "text": "hello" }),
    );
    sleep_ms(500);
    let (line, _) = element(&tree(control, id, window_id)?, "agent field")?;
    let seen = lines(log, from);
    check(
        "type puts the text in the agent's field",
        typed.is_ok() && line.contains("hello"),
        format!("{} | field: {line}", cut(&said(&typed), 400)),
    );
    check(
        "the reply says it went through accessibility, without key events",
        typed
            .as_ref()
            .is_ok_and(|v| v["route"] == "accessibility" && !has(&seen, "pair agent keydown")),
        cut(&said(&typed), 400),
    );

    let returned = control.call(
        "key",
        json!({ "sessionKey": KEY, "displayId": id, "keys": "Return" }),
    );
    sleep_ms(500);
    let seen = lines(log, from);
    check(
        "Return submits the agent's form, or fails naming the user's window",
        (returned.is_ok() && has(&seen, "pair agent submit \"hello\""))
            || (returned.is_err() && names_users_window(&returned)),
        cut(&said(&returned), 400),
    );

    let pressed = control.call("click", at(press));
    sleep_ms(500);
    let seen = lines(log, from);
    check(
        "a click on the agent's button presses it, or fails naming the user's window",
        (pressed.is_ok() && has(&seen, "pair agent click 1")) || names_users_window(&pressed),
        format!("{} | log {seen:?}", cut(&said(&pressed), 400)),
    );

    let tabbed = control.call(
        "key",
        json!({ "sessionKey": KEY, "displayId": id, "keys": "Tab" }),
    );
    sleep_ms(300);
    check(
        "Tab fails and names the user's window",
        names_users_window(&tabbed),
        cut(&said(&tabbed), 400),
    );

    let seen = lines(log, from);
    let touched = user_touched(&seen);
    check(
        "the user's window kept key focus and saw no event",
        touched.is_empty(),
        format!("{touched:?}"),
    );
    check(
        "the harness is still the front app",
        front_app() == "Electron",
        front_app(),
    );
    println!("     log meanwhile: {seen:?}");

    // Your front app back: the harness is in the background, and posted
    // keys reach the agent's window as before.
    if your_front.is_empty() || your_front == "Electron" {
        println!("     no front app of yours to bring back; the background case is left out");
        return Ok(());
    }
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!("tell application \"{your_front}\" to activate"),
        ],
    );
    sleep_ms(800);
    let from = lines(log, 0).len();
    let clicked = control.call("click", at(field));
    sleep_ms(300);
    let typed = control.call(
        "type",
        json!({ "sessionKey": KEY, "displayId": id, "text": "w" }),
    );
    sleep_ms(500);
    let seen = lines(log, from);
    check(
        "in the background, the harness takes posted keys in the agent's field",
        clicked.is_ok() && typed.is_ok() && has(&seen, "pair agent keydown \"w\""),
        format!("{} {} | log {seen:?}", said(&clicked), said(&typed)),
    );
    let touched: Vec<_> = user_touched(&seen)
        .into_iter()
        // In the background the agent's window may be key, which the
        // user's window isn't once your app is in front.
        .filter(|l| {
            ![
                "win user blur",
                "win agent focus",
                "pair user page-blur false",
            ]
            .contains(&l.as_str())
        })
        .collect();
    check(
        "and nothing reaches its user window",
        touched.is_empty(),
        format!("{touched:?}"),
    );
    Ok(())
}
