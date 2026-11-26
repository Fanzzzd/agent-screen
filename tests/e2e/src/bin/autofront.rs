//! Auto starts what the user's front app blocks by itself while their
//! hands rest (ADR 0025), with `debugActivity` playing their idle time:
//!
//! - A Keyboard Borrow in Auto waits for the hands to rest, and for an open
//!   composition to end, then runs: Apple's pinyin composes 你好 in Field on
//!   the display.
//! - A front request: with the Electron harness in front, its user window
//!   key, a plain key for its agent window waits; at rest Agent Screen
//!   starts the front hold itself, the key reaches the agent's page, and
//!   the harness is in front again. With `composing` set it waits.
//!
//! The second part needs an Electron.app (`target/e2e/Electron.app`, or
//! E2E_ELECTRON) and is skipped without one, saying so.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e autofront
//!
//! Makes 1 display. Takes your keyboard for a moment and puts a window and
//! the front on your screen: keep your hands off the Mac. Writes only the
//! e2e instance's `autonomy-e2e.json`, and puts it back.

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, field_app, fixture, instance, json, kill_ours,
    locked, our_binary, out, probe, put_back_autonomy, run, set_autonomy_file, sh_lenient, skip,
    sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result, ensure};

const KEY: &str = "autofront";
const PINYIN: &str = "com.apple.inputmethod.SCIM.ITABC";

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if locked() {
        skip("the screen is locked, and this test uses the real keyboard");
    }
    run(|| {
        allow_on_your_screen(
            "Electron",
            120_000,
            "the harness's user window stands in for your own browser window, on your screen on purpose",
        );
        stop_test_app();
        let before = set_autonomy_file(&json!({ "mode": "ask" }))?;
        let front = probe(&["frontpid"])?;
        let source = probe(&["source"])?;
        let result = start_test_app(&[("AGENTSCREEN_LANG", "en")]).and_then(|()| {
            let control = Control::connect()?;
            let made = control.call(
                "createDisplay",
                json!({ "sessionKey": KEY, "title": "Auto front", "width": 1280, "height": 800 }),
            )?;
            let id = made["displayId"].clone();
            agent_screen_tests::countdown();
            let result = borrow(&control, &id).and_then(|()| front_request(&control, &id));
            let _ = control.call("debugActivity", json!({ "clear": true }));
            let _ = control.call("debugShow", json!({ "surface": "autonomy-ask" }));
            let _ = control.call(
                "inputQueue",
                json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
            );
            kill_ours(our_binary("Field"));
            quit_harness();
            let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
            result
        });
        if probe(&["source"]).unwrap_or_default() != source {
            let _ = probe(&["source", source.trim()]);
        }
        if probe(&["frontpid"]).unwrap_or_default() != front {
            let _ = probe(&["activate", front.trim()]);
        }
        stop_test_app();
        put_back_autonomy(before);
        result
    })
}

/// A Keyboard Borrow in Auto waits for rest and for the composition to end.
fn borrow(control: &Control, id: &Value) -> Result<()> {
    let saved = out().join("autofront-field.txt");
    let _ = std::fs::remove_file(&saved);
    let read = || std::fs::read_to_string(&saved).unwrap_or_default();
    control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": field_app()?.to_str(),
                "environment": { "FIELD_OUT": saved.to_str() } }),
    )?;
    sleep_ms(800);
    let windows = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
    let window = windows["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["title"] == "Field"))
        .cloned()
        .context("no Field window")?;
    let (x, y, w, h) = (
        window["x"].as_f64().unwrap_or(0.0),
        window["y"].as_f64().unwrap_or(0.0),
        window["width"].as_f64().unwrap_or(0.0),
        window["height"].as_f64().unwrap_or(0.0),
    );
    control.call(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": x + w / 2.0, "y": y + h / 2.0 }),
    )?;
    control.call("debugShow", json!({ "surface": "autonomy-auto" }))?;
    // Types `text` while `release`, sent 1.5 s in, lets it go on.
    let timed = |text: &str, release: Value| -> (Result<Value>, u128) {
        std::thread::scope(|scope| {
            let started = Instant::now();
            let releaser = scope.spawn(|| {
                sleep_ms(1500);
                let _ = Control::connect().and_then(|c| c.call("debugActivity", release.clone()));
            });
            let typed = control.call(
                "type",
                json!({ "sessionKey": KEY, "displayId": id, "text": text,
                        "viaInputMethod": true, "inputSource": PINYIN }),
            );
            let _ = releaser.join();
            (typed, started.elapsed().as_millis())
        })
    };

    control.call("debugActivity", json!({ "idleMs": 0 }))?;
    let (typed, ms) = timed("nihao ", json!({ "idleMs": 9000 }));
    if let Err(e) = &typed
        && e.to_string().contains("secure input")
    {
        skip(
            "a password field on the user's screen has the keyboard; Agent Screen refused, as it should",
        );
    }
    sleep_ms(300);
    check(
        "an Auto Keyboard Borrow waits for the hands to rest, then runs",
        typed.is_ok() && ms >= 1400 && read().starts_with("你好"),
        format!("{ms} ms {typed:?} field {:?}", read()),
    );

    control.call(
        "debugActivity",
        json!({ "idleMs": 9000, "composing": true }),
    )?;
    let (typed, ms) = timed("ma ", json!({ "composing": false }));
    sleep_ms(300);
    check(
        "it waits for an open composition to end",
        typed.is_ok()
            && ms >= 1400
            && read()
                .split('\t')
                .next()
                .is_some_and(|t| t.starts_with("你好") && t.chars().count() == 3),
        format!("{ms} ms {typed:?} field {:?}", read()),
    );
    control.call("debugActivity", json!({ "clear": true }))?;
    kill_ours(our_binary("Field"));
    Ok(())
}

/// This checkout's harness only: another worktree may run its own.
fn quit_harness() {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &["-f", fixture("electron").to_str().unwrap_or_default()],
    );
    sleep_ms(800);
}

fn lines(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_string())
        .collect()
}

fn agent_key(lines: &[String], k: &str) -> bool {
    lines
        .iter()
        .any(|l| l.starts_with("pair agent keydown") && l.ends_with(&format!("\"{k}\"")))
}

/// A front request in Auto: Agent Screen starts the front hold itself.
fn front_request(control: &Control, id: &Value) -> Result<()> {
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        println!(
            "skip the front request: no Electron.app; copy one to target/e2e or set E2E_ELECTRON"
        );
        return Ok(());
    }
    let log = out().join("autofront-keys.log");
    let _ = std::fs::remove_file(&log);
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
        if lines(&log).iter().any(|l| l.starts_with("ready")) {
            let list = control.call("listWindows", json!({ "displayId": id }))?;
            window = list["windows"].as_array().and_then(|w| {
                w.iter()
                    .find(|w| w["app"] == "Electron" && w["title"] == "pair agent")
                    .cloned()
            });
            if window.is_some() {
                break;
            }
        }
        sleep_ms(250);
    }
    let window = window.context("the harness's agent window didn't show on the display")?;
    let window_id = window["windowId"].as_u64().context("windowId")?;
    control.call(
        "debugHandOver",
        json!({ "displayId": id, "windowId": window_id }),
    )?;
    sleep_ms(800);
    let pid = lines(&log)
        .iter()
        .find_map(|l| l.strip_prefix("ready ").map(str::to_string))
        .context("the harness's log names no pid")?;
    let to_front = || {
        sh_lenient(
            "/usr/bin/osascript",
            &[
                "-e",
                &format!(
                    "tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true"
                ),
            ],
        );
        sleep_ms(1500);
    };
    to_front();
    ensure!(
        probe(&["frontpid"])?.trim() == pid,
        "setup: the harness isn't in front"
    );
    let key = |k: &str| {
        control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": k }),
        )
    };

    control.call("debugActivity", json!({ "idleMs": 9000 }))?;
    sleep_ms(300);
    let reply = key("x");
    sleep_ms(800);
    check(
        "at rest, Auto starts the front hold and the key reaches the agent's page",
        reply.is_ok() && agent_key(&lines(&log), "x"),
        format!("{reply:?} log {:?}", lines(&log)),
    );
    check(
        "the harness is in front again after",
        probe(&["frontpid"])?.trim() == pid,
        "",
    );

    control.call(
        "debugActivity",
        json!({ "idleMs": 9000, "composing": true }),
    )?;
    let started = Instant::now();
    let reply = std::thread::scope(|scope| {
        let releaser = scope.spawn(|| {
            sleep_ms(2500);
            let ran = agent_key(&lines(&log), "y");
            let _ = Control::connect()
                .and_then(|c| c.call("debugActivity", json!({ "composing": false })));
            ran
        });
        let reply = key("y");
        (reply, releaser.join().unwrap_or(true))
    });
    let (reply, ran_early) = reply;
    sleep_ms(800);
    check(
        "with a composition open it waits, and runs once it ends",
        !ran_early
            && reply.is_ok()
            && agent_key(&lines(&log), "y")
            && started.elapsed().as_millis() >= 2400,
        format!("{reply:?} early {ran_early}"),
    );
    Ok(())
}
