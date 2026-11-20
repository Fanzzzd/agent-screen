//! A queued key in real Dia, shared with the user (#219): once it runs,
//! switching back to Dia lands in the user's window, not the agent's.
//! `sharedbrowser` checks the same on an owned Electron fixture; this is
//! the run in the browser the issue names.
//!
//! Dia is your own browser, so the script skips unless E2E_DIA=1. It never
//! closes, moves or navigates a window of yours, and you don't need to
//! touch the Mac. Started from Dia, it uses Finder as the app you leave
//! Dia for, and gives Dia the front back at the end. It brings Dia to the
//! front, where your window has focus, and opens its agent page in a new
//! Dia window on the Agent Display with `launch_app`. A plain key there waits in the Input Queue, since it
//! would reach your window. The script then brings back the app you had in
//! front, as you leaving Dia, and the same call again runs the key in the
//! agent's page. Meanwhile your app stays in front and Dia's windows stay
//! below it, sampled every 10 ms. Then the script brings Dia forward again,
//! without raising a window, and Dia's focused window must be yours. At the
//! end it gives the front back to your app and closes only the window it
//! opened, on the display.
//!
//!     E2E_DIA=1 AGENTSCREEN_INSTANCE=e2e cargo e2e diaqueue
//!
//! Brings Dia forward twice, a few seconds each. Makes 1 display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, check, cut, fronts_during, instance, json, probe, raised, run, sh_lenient,
    skip, sleep_ms, start_test_app, stop_test_app, taken_back,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "diaqueue";

/// What the agent's page reported, in order.
type Log = Arc<Mutex<Vec<String>>>;

const AGENT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diaqueue agent</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>diaqueue agent</h1>
<p><input id="name" aria-label="agent name" placeholder="agent name" autofocus></p>
<script>
const log = e => fetch('/log?e=' + encodeURIComponent(e)).catch(() => {});
document.addEventListener('keydown', ev => log('keydown ' + ev.key), true);
log('loaded');
</script></body></html>"#;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if std::env::var("E2E_DIA").as_deref() != Ok("1") {
        skip("Dia is your own browser: set E2E_DIA=1 to let the script bring it forward");
    }
    let Some(dia) = dia_pid() else {
        skip("Dia isn't running");
    };
    let log: Log = Arc::default();
    let port = match serve(log.clone()) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("no loopback server: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    run(|| {
        // The app you were in comes back at the end. With Dia in front,
        // Finder stands in for the app you'd leave Dia for.
        let original = front_pid().context("no front app")?;
        let yours = if original == dia {
            finder_pid().context("Dia is in front and Finder isn't running")?
        } else {
            original
        };
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        control.call("debugFrontWait", json!({ "ms": 1500 }))?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Dia queue", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let mut opened = None;
        let result = test(&control, &id, dia, yours, port, &log, &mut opened);
        // The app you were in back in front, then only the script's window
        // closes, on the display.
        if front_pid() != Some(original) {
            let _ = probe(&["activate", &original.to_string()]);
            sleep_ms(500);
        }
        if let Some(window) = opened {
            let said = probe(&["close", &dia.to_string(), &window.to_string()]).unwrap_or_default();
            sleep_ms(1000);
            check(
                "the script closes the Dia window it opened, on the display",
                said.trim() == "closed",
                said.trim(),
            );
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
    dia: i32,
    yours: i32,
    port: u16,
    log: &Log,
    opened: &mut Option<u64>,
) -> Result<()> {
    // Dia forward: the window you used last has its focus.
    probe(&["activate", &dia.to_string()])?;
    let started = Instant::now();
    while front_pid() != Some(dia) && started.elapsed() < Duration::from_secs(2) {
        sleep_ms(100);
    }
    let user_window = focused_window(dia);
    if front_pid() != Some(dia) || user_window == 0 {
        bail!(
            "Dia didn't come forward with a focused window: front {:?}, focused {user_window}",
            front_pid()
        );
    }
    let before = windows(dia);
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Dia",
                "open": format!("http://127.0.0.1:{port}/agent") }),
    );
    let window = launched
        .as_ref()
        .ok()
        .and_then(|v| v["window"]["windowId"].as_u64())
        .filter(|w| !before.contains(w));
    // A launch that gave up may still have made the window (#270): it is
    // the script's all the same.
    *opened = window.or_else(|| {
        sleep_ms(1000);
        windows(dia).into_iter().find(|w| !before.contains(w))
    });
    check(
        "launch_app opens a new Dia window on the display",
        window.is_some() && seen(log, 0, "loaded", 5000),
        cut(&said(&launched), 300),
    );
    if window.is_none() {
        bail!("no new Dia window");
    }
    let started = Instant::now();
    while focused_window(dia) != user_window && started.elapsed() < Duration::from_secs(3) {
        sleep_ms(100);
    }
    if front_pid() != Some(dia) || focused_window(dia) != user_window {
        bail!(
            "Dia isn't in front with your window focused after the launch: front {:?}, focused {} (yours {user_window})",
            front_pid(),
            focused_window(dia)
        );
    }

    let key = |keys: &str| {
        control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": keys }),
        )
    };
    let from = mark(log);
    let pending = key("x");
    check(
        "a plain key in the agent's Dia window, beside yours, answers input_pending",
        said(&pending).contains("input_pending"),
        cut(&said(&pending), 300),
    );
    control.call("debugFrontWait", json!({ "ms": 0 }))?;
    let dia_text = dia.to_string();
    let (ran, fronts) = fronts_during(6, Some(&dia_text), || {
        let leave = std::thread::spawn(move || {
            sleep_ms(1000);
            let _ = probe(&["activate", &yours.to_string()]);
        });
        let ran = key("x");
        let _ = leave.join();
        ran
    })?;
    let page = since(log, from);
    check(
        "once your app is in front, the same call runs the key in the agent's page",
        ran.is_ok() && page.iter().any(|e| e == "keydown x"),
        format!("{} | page {page:?}", cut(&said(&ran), 300)),
    );
    let taken = taken_back(&fronts, &dia_text);
    check(
        "Dia never comes back to the front while the key runs",
        taken.is_empty(),
        format!("{taken:?} | {fronts:?}"),
    );
    let rose = raised(&fronts);
    check(
        "your Dia windows never rise above your app",
        rose.is_empty(),
        format!("{rose:?} | {fronts:?}"),
    );

    // You switch back to Dia.
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!(
                "tell application \"System Events\" to set frontmost of (first process whose unix id is {dia}) to true"
            ),
        ],
    );
    sleep_ms(1000);
    let back = focused_window(dia);
    check(
        "switching back to Dia lands in your window, not the agent's",
        back == user_window,
        format!("focused {back}, yours {user_window}, the agent's {window:?}"),
    );
    Ok(())
}

fn dia_pid() -> Option<i32> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Dia"])
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
}

fn finder_pid() -> Option<i32> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Finder"])
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
}

fn front_pid() -> Option<i32> {
    probe(&["frontpid"]).ok()?.trim().parse().ok()
}

/// Dia's focused window, as Accessibility names it, 0 for none.
fn focused_window(pid: i32) -> u64 {
    probe(&["focused", &pid.to_string()])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Dia's windows on any screen.
fn windows(pid: i32) -> Vec<u64> {
    probe(&["windows", &pid.to_string()])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|w| w["layer"] == 0)
        .filter_map(|w| w["id"].as_u64())
        .collect()
}

/// An answer, or the error's text.
fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

fn mark(log: &Log) -> usize {
    log.lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn since(log: &Log, from: usize) -> Vec<String> {
    log.lock().unwrap_or_else(|e| e.into_inner())[from..].to_vec()
}

/// Wait up to `ms` for the page to report `event` after `from`.
fn seen(log: &Log, from: usize, event: &str, ms: u64) -> bool {
    let started = Instant::now();
    loop {
        if since(log, from).iter().any(|e| e == event) {
            return true;
        }
        if started.elapsed() > Duration::from_millis(ms) {
            return false;
        }
        sleep_ms(100);
    }
}

/// Serve the agent's page on a free loopback port, logging what it says.
fn serve(log: Log) -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                let _ = answer(stream, &log);
            });
        }
    });
    Ok(port)
}

fn answer(mut stream: TcpStream, log: &Log) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    let path = first.split_whitespace().nth(1).unwrap_or("/");
    let body = match path.split_once('?') {
        Some(("/log", query)) => {
            let event = query.strip_prefix("e=").unwrap_or_default();
            log.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(decode(event));
            String::new()
        }
        _ if path == "/agent" => AGENT_PAGE.to_string(),
        _ => String::new(),
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

/// A query value, percent-decoded.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
