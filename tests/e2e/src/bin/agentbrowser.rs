//! A Chromium browser the user already runs gets the agents' own instance
//! (#222, ADR 0022). launch_app with no arguments starts a second process
//! of the browser on Agent Screen's profile for agents, with the private
//! DevTools pipe, and opens its window on the Agent Display, showing
//! nothing on the user's screen and leaving the user's front app in front
//! through every launch, sampled every 50 ms. A click, typed text and a
//! click on a submit button reach its page as posted input. Another
//! launch_app opens its page through the pipe in the same instance, on the
//! display. A link from another app reaches the user's instance, not the
//! agents'. As Agent Screen quits, it closes the agents' windows and quits
//! the instance. Touches no window of the user's browser.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e agentbrowser
//!
//! Chrome by default, which needs Chrome not running: the script starts a
//! stand-in for the user's Chrome on a throwaway profile, on the display,
//! first. E2E_BROWSER=Dia with E2E_DIA=1 checks the same beside your
//! running Dia, whose windows it never touches, and sends it no link: the
//! first launch passes no URL, the page opens through the agents' instance
//! after, and the test app runs with the shared fallback off, so a refusal
//! fails the test, records nothing and opens nothing in your Dia. Makes 1
//! display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, capture, check, cut, instance, json, out, probe, profile_pids, quit_profile,
    run, running_headed, sh_lenient, skip, sleep_ms, start_test_app, stop_test_app, windows_of,
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
        Ok("Dia") => Browser {
            app: "Dia",
            process: "Dia",
            bundle_id: "company.thebrowser.dia",
        },
        _ => Browser {
            app: "Google Chrome",
            process: "Google Chrome",
            bundle_id: "com.google.Chrome",
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

/// The test instance's profile for the agents' instance (ADR 0022).
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
    if !stand_in && std::env::var("E2E_DIA").as_deref() != Ok("1") {
        skip("E2E_DIA=1 isn't set: the Dia check starts a second Dia beside yours");
    }
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
        // Beside your Dia, a refusal fails the test and records nothing,
        // never opening the window in your Dia.
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_NO_SHARED_FALLBACK", "1"),
        ])?;
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
    // Only the agents' profile has the cookie the agent page sets: a link
    // that carries it reached the agents' instance.
    let mut agents_cookie = false;
    while reader.read_line(&mut line)? > 2 {
        if line.to_ascii_lowercase().starts_with("cookie:") && line.contains("who=agents") {
            agents_cookie = true;
        }
        line.clear();
    }
    let path = first.split_whitespace().nth(1).unwrap_or("/");
    if let Some(n) = path.strip_prefix("/link?n=") {
        let reached = if agents_cookie { "agents" } else { "user" };
        log.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("link {n} {reached}"));
    }
    let cookie = if path.starts_with("/agent") {
        "Set-Cookie: who=agents; Path=/; Max-Age=600\r\n"
    } else {
        ""
    };
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
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{cookie}Connection: close\r\n\r\n{body}",
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

/// Where a link from another app goes: `open -g -b` asks LaunchServices,
/// as a link clicked in another app does, without activating anything.
/// Only with the stand-in, never your Dia. `n` names the link in the log;
/// the answer is "agents" when it carried the agents' profile's cookie,
/// "user" when it reached the stand-in.
fn link(b: &Browser, port: u16, log: &Log, n: u32) -> Option<String> {
    let from = mark(log);
    sh_lenient(
        "/usr/bin/open",
        &[
            "-g",
            "-b",
            b.bundle_id,
            &format!("http://127.0.0.1:{port}/link?n={n}"),
        ],
    );
    let prefix = format!("link {n} ");
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if let Some(e) = since(log, from)
            .into_iter()
            .find(|e| e.starts_with(&prefix))
        {
            return Some(e[prefix.len()..].to_string());
        }
        sleep_ms(100);
    }
    None
}

/// The user's front app, sampled every 50 ms through every launch: a new
/// window of the agents' instance must never make it front, not even for a
/// moment. Each time another app was front, from when to when, in ms since
/// the watch began.
/// Another front app, from when to when, in ms.
type Fronts = Arc<Mutex<Vec<(Option<u32>, u128, u128)>>>;

struct FrontWatch {
    stop: Arc<AtomicBool>,
    seen: Fronts,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FrontWatch {
    fn start(front: Option<u32>) -> FrontWatch {
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Fronts = Arc::default();
        let (s, v) = (stop.clone(), seen.clone());
        let thread = std::thread::spawn(move || {
            let began = Instant::now();
            let mut last = front;
            while !s.load(Ordering::SeqCst) {
                let now = front_pid();
                let at = began.elapsed().as_millis();
                let mut v = v.lock().unwrap_or_else(|e| e.into_inner());
                if now != front {
                    match v.last_mut() {
                        Some((pid, _, until)) if *pid == now && last == now => *until = at,
                        _ => v.push((now, at, at)),
                    }
                }
                last = now;
                drop(v);
                sleep_ms(50);
            }
        });
        FrontWatch {
            stop,
            seen,
            thread: Some(thread),
        }
    }

    /// The other front apps it saw, in order.
    fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(pid, from, until)| format!("{pid:?} from {from} to {until} ms"))
            .collect()
    }
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
        if stand_in {
            // LaunchServices hands a link to the running instance whatever
            // its profile: the reason the agents' instance must not outlive
            // its displays.
            let reached = link(b, port, log, 1);
            check(
                "with one instance running, on its own profile, a link from another app reaches it",
                reached.as_deref() == Some("user") && pids(b.process) == before,
                format!("{reached:?}, pids {before:?} -> {:?}", pids(b.process)),
            );
        }
        let watcher = Watcher::start(b.process, before.clone(), display);
        let fronts = FrontWatch::start(front);
        let from = mark(log);
        let url = format!("http://127.0.0.1:{port}/agent");
        // The user's own windows, which must stay as they are.
        let theirs = |pids: &[u32]| -> Vec<u64> {
            let mut ids: Vec<u64> = pids
                .iter()
                .flat_map(|&p| windows_of(p))
                .filter(|f| f.layer == 0)
                .map(|f| f.id)
                .collect();
            ids.sort_unstable();
            ids
        };
        let user_windows = theirs(&before);
        // Your Dia gets no URL: should Dia hand a refused start's command
        // line to yours, it would open there. The page opens through the
        // agents' instance after.
        let launched = control.call(
            "launchApp",
            if stand_in {
                json!({ "sessionKey": KEY, "displayId": id, "app": b.app, "open": url })
            } else {
                json!({ "sessionKey": KEY, "displayId": id, "app": b.app })
            },
        )?;
        let first = launched["window"]["windowId"]
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
            "launch_app answers with the agents' window, not shared",
            launched.get("shared").is_none(),
            cut(&launched.to_string(), 400),
        );
        let window = if stand_in {
            first
        } else {
            let paged = control.call(
                "launchApp",
                json!({ "sessionKey": KEY, "displayId": id, "app": b.app, "open": url }),
            )?;
            let window = paged["window"]["windowId"]
                .as_u64()
                .with_context(|| format!("no window: {paged}"))?;
            let frames = new.first().map(|&p| windows_of(p)).unwrap_or_default();
            // Dia may put the page in its open window as a tab.
            check(
                "a second launch_app opens the page in the same agents' instance, on the display",
                pids(b.process) == after && frames.iter().any(|f| f.id == window),
                format!(
                    "{} window | {} | its windows {frames:?}",
                    if window == first { "the same" } else { "a new" },
                    cut(&paged.to_string(), 400)
                ),
            );
            window
        };
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
        // For a look at what a fresh profile shows first, such as onboarding.
        if let Ok(file) = capture("agentbrowser-first", display) {
            println!("the display after the first launch: {}", file.display());
        }

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

        check(
            "the user's own windows are as they were",
            theirs(&before) == user_windows,
            format!("{user_windows:?} -> {:?}", theirs(&before)),
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
            "another launch_app opens a window, or a tab in the open one, in the same agents' instance",
            still.len() == after.len() && frames.iter().any(|f| second.is_some_and(|w| f.id == w)),
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
        let others = fronts.finish();
        check(
            "the user's front app stayed in front through every launch",
            others.is_empty(),
            format!("front {front:?}, then {others:?}"),
        );
        if let Ok(file) = capture("agentbrowser-second", display) {
            println!("the display after the second launch: {}", file.display());
        }
        if stand_in {
            let reached = link(b, port, log, 2);
            check(
                "with the agents' instance beside the user's, a link from another app reaches the user's",
                reached.as_deref() == Some("user") && pids(b.process) == after,
                format!("{reached:?}, pids {after:?} -> {:?}", pids(b.process)),
            );
        }
        Ok(())
    })();
    // The stand-in goes before its display: a display that goes first hands
    // its windows to your screen.
    if stand_in {
        quit_profile(user_profile);
    }
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    // Agent Screen closes the agents' windows through the pipe as it quits,
    // then quits the instance, so it never takes the user's Dock click or
    // links. The sighting watch fails the script if a window reached your
    // screen meanwhile.
    stop_test_app();
    // Chromium takes a moment to quit after Agent Screen asks.
    let mut left = profile_pids(profile);
    for _ in 0..50 {
        if left.is_empty() {
            break;
        }
        sleep_ms(100);
        left = profile_pids(profile);
    }
    check(
        "Agent Screen quit the agents' instance as it quit",
        left.is_empty(),
        format!("still running: {left:?}"),
    );
    result
}
