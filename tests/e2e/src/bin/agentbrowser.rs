//! A Chromium browser the user already runs gets the agents' own instance
//! (#222, ADR 0020). launch_app with no arguments starts a second process
//! of the browser on Agent Screen's profile for agents, with the private
//! DevTools pipe, and opens its window on the Agent Display, showing
//! nothing on the user's screen and leaving the user's front app alone. A
//! click, typed text and a click on a submit button reach its page as
//! posted input. A second launch_app opens a new window in the same
//! instance, on the display. Touches no window of the user's browser.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e agentbrowser
//!
//! E2E_BROWSER names the browser: Dia (the default) needs your Dia running,
//! and the script skips without it. "Google Chrome" needs Chrome not
//! running: the script starts a stand-in for the user's Chrome on a
//! throwaway profile, on the display, first. Quits the instances it
//! started. Makes 1 display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, check, cut, instance, json, out, probe, quit_profile, run, running_headed,
    sh_lenient, skip, sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

const KEY: &str = "agentbrowser";
const TOKEN: &str = "abq3m";

type Log = Arc<Mutex<Vec<String>>>;

struct Browser {
    /// The name launch_app takes.
    app: &'static str,
    /// The main process's name, for pgrep.
    process: &'static str,
    bundle_id: &'static str,
}

fn browser() -> Browser {
    match std::env::var("E2E_BROWSER").as_deref() {
        Ok("Google Chrome") => Browser {
            app: "Google Chrome",
            process: "Google Chrome",
            bundle_id: "com.google.Chrome",
        },
        _ => Browser {
            app: "Dia",
            process: "Dia",
            bundle_id: "company.thebrowser.dia",
        },
    }
}

fn pids(process: &str) -> Vec<u32> {
    sh_lenient("/usr/bin/pgrep", &["-x", process])
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn front_pid() -> Option<u32> {
    probe(&["frontpid"]).ok()?.trim().parse().ok()
}

/// The test instance's profile for the agents' instance (ADR 0020).
fn agents_profile(bundle_id: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home)
        .join("Library/Application Support/Agent Screen")
        .join(format!(
            "Browser Profiles ({})",
            instance().unwrap_or_default()
        ))
        .join(bundle_id)
}

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let b = browser();
    let stand_in = b.app != "Dia";
    if !stand_in && pids(b.process).is_empty() {
        skip("Dia isn't running: this checks the agents' instance beside yours");
    }
    if stand_in && running_headed(b.process) {
        skip("Chrome is running; quit it to test with a stand-in for yours");
    }
    let log: Log = Arc::default();
    let port = match serve(log.clone()) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("no loopback server: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let profile = agents_profile(b.bundle_id);
    let user_profile = out().join("agentbrowser-user-profile");
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&b, stand_in, port, &log, &profile, &user_profile);
        quit_profile(&profile);
        if stand_in {
            quit_profile(&user_profile);
        }
        stop_test_app();
        result
    })
}

const AGENT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>agentbrowser</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>agentbrowser</h1>
<form id="form" aria-label="agent form">
  <input id="query" aria-label="agent query" placeholder="agent query">
  <button type="submit" aria-label="agent send">Send</button>
</form>
<script>
const log = e => fetch('/log?e=' + encodeURIComponent(e)).catch(() => {});
document.addEventListener('focusin', ev => log('focusin ' + (ev.target.id || ev.target.tagName)), true);
document.getElementById('form').addEventListener('submit', ev => { ev.preventDefault(); log('submit'); });
let last = null;
setInterval(() => {
  const v = document.getElementById('query').value;
  if (last !== v) { last = v; log('value ' + v); }
}, 100);
log('loaded');
</script></body></html>"#;

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
            ""
        }
        _ if path.starts_with("/agent") => AGENT_PAGE,
        _ => "",
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

/// Percent-decoding for the page's ASCII events.
fn decode(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&hex, 16).map_or('?', char::from));
            }
            '+' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn mark(log: &Log) -> usize {
    log.lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn seen_within(log: &Log, from: usize, event: &str, ms: u64) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(ms) {
        if log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .skip(from)
            .any(|e| e == event)
        {
            return true;
        }
        sleep_ms(100);
    }
    false
}

fn since(log: &Log, from: usize) -> Vec<String> {
    log.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .skip(from)
        .cloned()
        .collect()
}

/// A display's global bounds: x, y, width, height.
fn bounds(id: u64) -> Result<[f64; 4]> {
    let said = probe(&["bounds", &id.to_string()])?;
    let v: Vec<f64> = said
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    v.try_into()
        .map_err(|_| anyhow::anyhow!("no bounds in {said:?}"))
}

/// Watches every new process of the browser, sampling its windows, and
/// keeps those that showed outside the display.
struct Watcher {
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    fn start(process: &'static str, before: Vec<u32>, display: [f64; 4]) -> Watcher {
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let (s, v) = (stop.clone(), seen.clone());
        let thread = std::thread::spawn(move || {
            let [x, y, w, h] = display;
            while !s.load(Ordering::SeqCst) {
                for pid in pids(process).into_iter().filter(|p| !before.contains(p)) {
                    for f in windows_of(pid) {
                        let inside = f.x >= x - 1.0
                            && f.y >= y - 1.0
                            && f.x + f.width <= x + w + 1.0
                            && f.y + f.height <= y + h + 1.0;
                        if f.layer == 0 && !inside {
                            v.lock().unwrap_or_else(|e| e.into_inner()).push(format!(
                                "pid {pid} window {} at {},{} {}x{}",
                                f.id, f.x, f.y, f.width, f.height
                            ));
                        }
                    }
                }
                sleep_ms(20);
            }
        });
        Watcher {
            stop,
            seen,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        seen.dedup();
        seen
    }
}

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

fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// Posted: the answer names no accessibility route.
fn posted(reply: &Result<Value>) -> bool {
    reply
        .as_ref()
        .is_ok_and(|v| v.get("route").is_none_or(|r| r != "accessibility"))
}

fn test(
    b: &Browser,
    stand_in: bool,
    port: u16,
    log: &Log,
    profile: &Path,
    user_profile: &Path,
) -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Agent browser", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let result = (|| -> Result<()> {
        let display = bounds(id)?;
        if stand_in {
            // Stands in for the user's own Chrome.
            let _ = std::fs::remove_dir_all(user_profile);
            control.call(
                "launchApp",
                json!({ "sessionKey": KEY, "displayId": id, "app": b.app, "arguments": [
                    format!("--user-data-dir={}", user_profile.display()),
                    "--no-first-run", "--no-default-browser-check",
                ] }),
            )?;
            sleep_ms(1000);
        }
        let before = pids(b.process);
        let front = front_pid();
        println!("{} before: {before:?}, front pid {front:?}", b.process);
        let watcher = Watcher::start(b.process, before.clone(), display);
        let from = mark(log);
        let launched = control.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": b.app,
                    "open": format!("http://127.0.0.1:{port}/agent") }),
        )?;
        let window = launched["window"]["windowId"]
            .as_u64()
            .with_context(|| format!("no window: {launched}"))?;
        let after = pids(b.process);
        let new: Vec<u32> = after
            .iter()
            .copied()
            .filter(|p| !before.contains(p))
            .collect();
        let args = new
            .first()
            .map(|p| sh_lenient("/bin/ps", &["-o", "args=", "-p", &p.to_string()]))
            .unwrap_or_default();
        check(
            "launch_app started one new process, on the agents' profile, with the pipe",
            new.len() == 1
                && args.contains(&format!("--user-data-dir={}", profile.display()))
                && args.contains("--remote-debugging-pipe"),
            format!("new {new:?}: {}", cut(&args, 400)),
        );
        check(
            "the user's browser processes still run",
            before.iter().all(|p| after.contains(p)),
            format!("before {before:?}, after {after:?}"),
        );
        check(
            "the agent's page loaded",
            seen_within(log, from, "loaded", 15_000),
            format!("{:?}", since(log, from)),
        );
        check(
            "the user's front app didn't change",
            front_pid() == front,
            format!("{front:?} -> {:?}", front_pid()),
        );

        let state = control.call(
            "windowState",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
        )?;
        let tree = state["tree"].as_str().unwrap_or_default().to_string();
        let query = element(&tree, "agent query")?;
        let send = element(&tree, "agent send")?;
        let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });
        let clicked_at = mark(log);
        let clicked = control.call("click", at(query));
        check(
            "a click on the page's field is posted and focuses it",
            posted(&clicked) && seen_within(log, clicked_at, "focusin query", 3000),
            format!(
                "{} | page {:?}",
                cut(&said(&clicked), 400),
                since(log, clicked_at)
            ),
        );
        let typed = control.call(
            "type",
            json!({ "sessionKey": KEY, "displayId": id, "text": TOKEN }),
        );
        check(
            "typed text is posted and lands in the field",
            posted(&typed) && seen_within(log, clicked_at, &format!("value {TOKEN}"), 3000),
            format!(
                "{} | page {:?}",
                cut(&said(&typed), 400),
                since(log, clicked_at)
            ),
        );
        let sent_at = mark(log);
        let sent = control.call("click", at(send));
        check(
            "a click on the submit button is posted and submits the form",
            posted(&sent) && seen_within(log, sent_at, "submit", 3000),
            format!(
                "{} | page {:?}",
                cut(&said(&sent), 400),
                since(log, sent_at)
            ),
        );

        // Another session's launch, here on the same display: a new window
        // in the same instance.
        let again = control.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": b.app }),
        );
        let second = again
            .as_ref()
            .ok()
            .and_then(|v| v["window"]["windowId"].as_u64());
        let still = pids(b.process);
        let frames = new.first().map(|&p| windows_of(p)).unwrap_or_default();
        check(
            "a second launch_app opens a new window in the same agents' instance",
            second.is_some_and(|w| w != window)
                && still.len() == after.len()
                && frames.iter().any(|f| second.is_some_and(|w| f.id == w)),
            format!(
                "{} | pids {still:?} | its windows {frames:?}",
                cut(&said(&again), 400)
            ),
        );
        let outside = watcher.finish();
        check(
            "no window of the agents' instance showed off the display",
            outside.is_empty(),
            format!("{outside:?}"),
        );
        Ok(())
    })();
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}
