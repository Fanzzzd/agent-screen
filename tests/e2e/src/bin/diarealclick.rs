//! A click with your real pointer in an agent's window of your own Dia
//! (#255). Dia takes the loan's AXFrontmost write and stays behind, so the
//! loan must bring it forward another way, or the click fails with "the
//! app didn't come to the front".
//!
//! Dia is your own browser, so the script skips unless E2E_DIA=1. It opens
//! its agent page, one big button, in a new Dia window on the Agent Display
//! with `launch_app`, and clicks it with realInput twice, allowed as the
//! card's Allow does: once while another app is in front (Finder when you
//! start it from Dia), and once while Dia is in front with your window
//! focused. Each click must reach the agent's page with route real_input.
//! Afterwards the app that was in front is back, your Dia windows have
//! the frames they had, AeroSpace shows the workspaces it showed, and with
//! Dia in front your window is Dia's focused and main window again. At the
//! end it gives the front back to your app and closes only the window it
//! opened, on the display.
//!
//!     E2E_DIA=1 AGENTSCREEN_INSTANCE=e2e cargo e2e diarealclick
//!
//! Moves your pointer for a second, twice, and brings Dia forward twice:
//! keep your hands off the Mac while the sign says so. Makes 1 display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, check, cut, fronts_during, instance, json, locked, own_input_now, probe, run,
    sh_lenient, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "diarealclick";

/// What the agent's page reported, in order.
type Log = Arc<Mutex<Vec<String>>>;

const AGENT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diarealclick agent</title></head>
<body style="margin: 0">
<button id="go" style="position: fixed; inset: 0; width: 100%; height: 100%; font: 32px -apple-system">diarealclick agent</button>
<script>
const log = e => fetch('/log?e=' + encodeURIComponent(e)).catch(() => {});
document.getElementById('go').addEventListener('click', ev => log('click ' + ev.isTrusted));
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
    if locked() {
        skip("the screen is locked, and this test moves the real pointer");
    }
    let Some(dia) = pid_of("Dia") else {
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
        let original = front_pid().context("no front app")?;
        let other = if original == dia {
            pid_of("Finder").context("Dia is in front and Finder isn't running")?
        } else {
            original
        };
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Dia real click", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let mut opened = None;
        let result = test(&control, &id, dia, other, port, &log, &mut opened);
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
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
    other: i32,
    port: u16,
    log: &Log,
    opened: &mut Option<u64>,
) -> Result<()> {
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
        .filter(|w| !before.iter().any(|(id, _)| id == w));
    // A launch that gave up may still have made the window (#270): it is
    // the script's all the same.
    *opened = window.or_else(|| {
        sleep_ms(1000);
        windows(dia)
            .into_iter()
            .map(|(id, _)| id)
            .find(|w| !before.iter().any(|(id, _)| id == w))
    });
    check(
        "launch_app opens a new Dia window on the display",
        window.is_some() && seen(log, 0, "loaded", 5000),
        cut(&said(&launched), 300),
    );
    let Some(window) = window else {
        bail!("no new Dia window");
    };
    let list = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
    let frame = list["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["windowId"] == window))
        .cloned()
        .context("the agent's Dia window isn't on the display")?;
    let at = |key: &str| frame[key].as_i64().unwrap_or_default();
    // Below Dia's toolbar, on the button that fills the page.
    let point = (at("x") + at("width") / 2, at("y") + at("height") / 2 + 40);
    let yours: Vec<(u64, String)> = windows(dia)
        .into_iter()
        .filter(|(w, _)| *w != window)
        .collect();

    for round in ["another app in front", "your Dia window in front"] {
        let front = if round.starts_with("your") {
            dia
        } else {
            other
        };
        probe(&["activate", &front.to_string()])?;
        let started = Instant::now();
        while front_pid() != Some(front) && started.elapsed() < Duration::from_secs(3) {
            sleep_ms(100);
        }
        sleep_ms(500);
        if front_pid() != Some(front) {
            bail!(
                "{round}: pid {front} didn't come forward: front {:?}",
                front_pid()
            );
        }
        let focused = focused_window(dia);
        let spaces = aerospace_visible();
        let from = mark(log);
        let call = control.begin(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": point.0, "y": point.1,
                    "button": "left", "count": 1, "realInput": true }),
        )?;
        sleep_ms(800);
        let (reply, fronts) = fronts_during(6, None, || {
            // The loan moves and clicks your pointer: that's the script's
            // input, not yours.
            let loaning = AtomicBool::new(true);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    while loaning.load(Ordering::Relaxed) {
                        own_input_now();
                        sleep_ms(100);
                    }
                });
                let _ = control.call(
                    "debugShow",
                    json!({ "sessionKey": KEY, "surface": "input-queue-allow", "displayId": id }),
                );
                let reply = call.wait();
                // The pointer and front app come back after the reply.
                sleep_ms(1500);
                loaning.store(false, Ordering::Relaxed);
                reply
            })
        })?;
        let page = since(log, from);
        check(
            &format!("{round}: the real click reaches the agent's Dia page with route real_input"),
            reply.as_ref().is_ok_and(|r| r["route"] == "real_input")
                && page.iter().any(|e| e == "click true"),
            format!("{} | page {page:?}", cut(&said(&reply), 400)),
        );
        println!("     {round}: fronts {fronts:?}");
        check(
            &format!("{round}: the app that was in front is in front again"),
            front_pid() == Some(front),
            format!("front {:?}, was {front}", front_pid()),
        );
        let now: Vec<(u64, String)> = windows(dia)
            .into_iter()
            .filter(|(w, _)| *w != window)
            .collect();
        let moved: Vec<_> = yours.iter().filter(|w| !now.contains(w)).collect();
        check(
            &format!("{round}: your Dia windows keep their frames"),
            moved.is_empty(),
            format!("{moved:?} -> {now:?}"),
        );
        let spaces_now = aerospace_visible();
        check(
            &format!("{round}: AeroSpace shows the workspaces it showed"),
            spaces_now == spaces,
            format!("{spaces:?} -> {spaces_now:?}"),
        );
        if front == dia {
            let (focused_now, main) = (focused_window(dia), main_window(dia));
            check(
                &format!("{round}: your window is Dia's focused and main window again"),
                focused_now == focused && main == focused,
                format!(
                    "focused {focused_now}, main {main}, yours {focused}, the agent's {window}"
                ),
            );
        }
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
        sleep_ms(1000);
    }
    Ok(())
}

fn pid_of(name: &str) -> Option<i32> {
    sh_lenient("/usr/bin/pgrep", &["-x", name])
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
}

fn front_pid() -> Option<i32> {
    probe(&["frontpid"]).ok()?.trim().parse().ok()
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

/// The app's focused window, as Accessibility names it, 0 for none.
fn focused_window(pid: i32) -> u64 {
    probe(&["focused", &pid.to_string()])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// The app's main window, as Accessibility names it, 0 for none.
fn main_window(pid: i32) -> u64 {
    probe(&["mainwindow", &pid.to_string()])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Dia's normal windows on screen, each with its frame.
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
