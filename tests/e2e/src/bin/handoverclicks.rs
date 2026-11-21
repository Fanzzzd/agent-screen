//! Clicks on a React-style page in a Chromium window the user handed over
//! (#219). The issue's Dia window ignored `computer left_click` and
//! `act_on_element click` and `press` on the Vercel dashboard, while
//! focus and Return worked; text typed after a missed click reached the
//! page's shortcuts, and a native select couldn't be chosen by click.
//! The select's menu does open in the background (#276): it is a window
//! at the pop-up menu level, which list_windows leaves out, and its items
//! are in the window's state while it is open.
//!
//! A Chrome on a throwaway profile plays the user's browser. Agent Screen
//! has no DevTools pipe to it: the script starts it with no startup
//! window, which Chromium shows with activation, keeps it hidden from the
//! moment it checks in, and opens the fixture's window in the background
//! through a pipe only the script holds. Started with `open -j`, or with
//! its startup window, it took the user's front app. Then it shows again,
//! without coming to the front, on your main screen for the moment until
//! the window is handed over with `place_window` and the user's words, as
//! the user hands over a window they see. The fixture listens at
//! its root for pointer, mouse, click, focus and key events, as React
//! does, and logs each with `isTrusted`, `pointerType`, `detail` and its
//! target:
//!
//! - a button that acts on `click`;
//! - a trigger that opens a portal popover on `pointerdown`, as Radix
//!   menus do, ignoring an untrusted, non-primary or non-mouse press;
//! - a button that acts only when a trusted pointerdown, pointerup and
//!   click all reach it;
//! - a text field, with a page shortcut on F that fires when no field has
//!   focus;
//! - a native select.
//!
//! The script clicks each control through the agent's `click`, presses and
//! clicks them through `elementAction`, clicks the field and types, opens
//! the select by click and picks an item by click, then by focus and
//! type-ahead, then opens it and types, which only moves the menu's
//! highlight and says so, and picks with Return. A click right after each
//! choice must go out, as the chosen menu fades out rather than being
//! closed by the user. It records which events each route delivered. The
//! front app is sampled every 10 ms through the run: if the user's was
//! away for more than 100 ms before the handover, the script quits the
//! stand-in and stops, and the handed-over browser must never come to the
//! front.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e handoverclicks
//!
//! Needs Chrome not running: it starts Chrome on its own profile in /tmp,
//! never yours. Makes 1 display.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, cut, instance, json, out, probe, profile_pids,
    quit_profile, run, running_headed, sh_lenient, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

const KEY: &str = "handoverclicks";

type Log = Arc<Mutex<Vec<String>>>;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>handoverclicks</title>
<style>
body { font: 15px -apple-system; margin: 0; padding: 20px; }
button, select, input { font: inherit; margin: 6px; padding: 8px 14px; }
#portal div { position: fixed; left: 40px; bottom: 8px; padding: 10px; background: #eee; border: 1px solid #999; }
#palette { position: fixed; top: 10px; right: 10px; padding: 8px; background: #fd0; display: none; }
</style></head>
<body>
<div id="root">
  <h1>handoverclicks</h1>
  <div><button id="plain" aria-label="plain button">Plain</button>
  <button id="trigger" aria-label="pointer trigger">Menu</button>
  <button id="strict" aria-label="strict button">Strict</button></div>
  <div><input id="field" aria-label="test field" placeholder="type here"></div>
  <div><select id="sel" aria-label="environments"><option>Production</option><option>Preview</option><option>Development</option></select></div>
</div>
<div id="portal"></div>
<div id="palette">Find palette</div>
<script>
const log = e => fetch('/log?e=' + encodeURIComponent(e), { keepalive: true }).catch(() => {});
const root = document.getElementById('root');
const name = t => (t && (t.id || t.tagName)) || '?';
const describe = ev => [ev.type, name(ev.target), 'trusted=' + ev.isTrusted,
  ev.pointerType !== undefined ? 'ptype=' + ev.pointerType : '',
  ev.button !== undefined ? 'button=' + ev.button : '',
  ev.detail !== undefined ? 'detail=' + ev.detail : ''].filter(Boolean).join(' ');
// React attaches its listeners at the root and dispatches from there.
for (const type of ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'click', 'focusin', 'change']) {
  root.addEventListener(type, ev => log(describe(ev)), true);
}
let moved = 0;
root.addEventListener('pointermove', () => { if (moved++ === 0) log('pointermove first'); }, true);
// A Radix-style trigger: opens on a primary, trusted mouse pointerdown.
let strict = {};
root.addEventListener('pointerdown', ev => {
  if (ev.target.id === 'trigger' && ev.isTrusted && ev.button === 0 && !ev.ctrlKey) {
    ev.preventDefault();
    document.getElementById('portal').innerHTML = '<div id="pop" role="menu" aria-label="popover">Popover open</div>';
    log('popover opened ptype=' + ev.pointerType);
  }
  if (ev.target.id === 'strict') strict = { down: ev.isTrusted };
});
root.addEventListener('pointerup', ev => { if (ev.target.id === 'strict') strict.up = ev.isTrusted; });
root.addEventListener('click', ev => {
  if (ev.target.id === 'plain') log('plain acted trusted=' + ev.isTrusted);
  if (ev.target.id === 'strict') log(strict.down && strict.up && ev.isTrusted ? 'strict acted' : 'strict refused ' + JSON.stringify(strict));
  strict = {};
});
// A page shortcut, as Vercel's F for Find: only when no field has focus.
document.addEventListener('keydown', ev => {
  const field = ['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement && document.activeElement.tagName);
  if (!field && ev.key.toLowerCase() === 'f') { document.getElementById('palette').style.display = 'block'; log('shortcut F'); }
});
let last = null;
setInterval(() => {
  const v = document.getElementById('field').value + '|' + document.getElementById('sel').value;
  if (v !== last) { last = v; log('value ' + v); }
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
                let mut stream = stream;
                let mut reader = BufReader::new(&stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    return;
                }
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    header.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = if let Some(e) = path.strip_prefix("/log?e=") {
                    let decoded = decode(e);
                    log.lock().unwrap_or_else(|p| p.into_inner()).push(decoded);
                    String::new()
                } else {
                    PAGE.to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.read(&mut [0; 1]);
            });
        }
    });
    Ok(port)
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("20");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b' '));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mark(log: &Log) -> usize {
    log.lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn since(log: &Log, from: usize) -> Vec<String> {
    log.lock().unwrap_or_else(|e| e.into_inner())[from..].to_vec()
}

/// The page's events since `from` once `done` says they're all in, or
/// after 3 s. A window shown a moment ago, as the handed-over one, may
/// deliver them late.
fn wait_for(log: &Log, from: usize, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let events = since(log, from);
        if done(&events) || std::time::Instant::now() >= until {
            return events;
        }
        sleep_ms(100);
    }
}

fn profile() -> PathBuf {
    out().join("handoverclicks-profile")
}

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it so a Chrome on a throwaway profile can play your browser");
    }
    let log: Log = Arc::default();
    run(|| {
        let port = serve(log.clone())?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(port, &log);
        quit_profile(&profile());
        stop_test_app();
        result
    })
}

/// The element labelled `label` in a window state tree: its number and its
/// center on the display.
fn element(tree: &str, label: &str) -> Result<(u64, (f64, f64))> {
    find(tree, &format!("\"{label}\""))
}

/// The item titled `title` of a native menu open over the window, in its
/// window state tree: the select's own items under its popupbutton hold
/// the title as a value and have no real frames.
fn menu_item(tree: &str, title: &str) -> Result<(u64, (f64, f64))> {
    find(tree, &format!("menuitem \"{title}\""))
}

fn find(tree: &str, needle: &str) -> Result<(u64, (f64, f64))> {
    let line = tree
        .lines()
        .find(|l| l.contains(needle))
        .with_context(|| format!("no {needle} in the tree:\n{}", cut(tree, 2000)))?;
    let number = line
        .trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .and_then(|n| n.parse().ok())
        .with_context(|| format!("no number in {line}"))?;
    let at = line
        .split(" at (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .with_context(|| format!("no position in {line}"))?;
    let (x, y) = at.split_once(',').context("x,y")?;
    Ok((number, (x.trim().parse()?, y.trim().parse()?)))
}

/// The test's own DevTools pipe to the stand-in: Agent Screen has none to
/// it, as to a browser the user started.
struct Pipe {
    to: std::sync::Mutex<std::fs::File>,
    from: std::sync::Mutex<BufReader<std::fs::File>>,
    next: std::sync::atomic::AtomicU64,
}

impl Pipe {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut message = json!({ "id": id, "method": method, "params": params })
            .to_string()
            .into_bytes();
        message.push(0);
        self.to
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&message)?;
        let mut from = self.from.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let mut buf = Vec::new();
            if from.read_until(0, &mut buf)? == 0 {
                anyhow::bail!("the stand-in's pipe closed before it answered {method}");
            }
            buf.pop();
            let answer: Value = serde_json::from_slice(&buf)?;
            if answer["id"].as_u64() == Some(id) {
                if let Some(error) = answer.get("error") {
                    anyhow::bail!("{method}: {error}");
                }
                return Ok(answer["result"].clone());
            }
        }
    }
}

/// Start Chrome on `dir` with no startup window, which Chromium shows with
/// activation, and a DevTools pipe on fds 3 and 4 that only this test holds.
fn start_stand_in(dir: &std::path::Path) -> Result<(std::process::Child, Pipe)> {
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;
    let (mut to_app, mut from_app) = ([0; 2], [0; 2]);
    if unsafe { libc::pipe(to_app.as_mut_ptr()) } != 0
        || unsafe { libc::pipe(from_app.as_mut_ptr()) } != 0
    {
        anyhow::bail!("no pipe: {}", std::io::Error::last_os_error());
    }
    let (child_in, child_out) = (to_app[0], from_app[1]);
    let mut command =
        std::process::Command::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    command
        .args([
            &format!("--user-data-dir={}", dir.display()),
            "--remote-debugging-pipe",
            "--no-startup-window",
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(child_in, 3) < 0 || libc::dup2(child_out, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    unsafe {
        libc::close(child_in);
        libc::close(child_out);
    }
    let pipe = Pipe {
        to: std::sync::Mutex::new(unsafe { std::fs::File::from_raw_fd(to_app[1]) }),
        from: std::sync::Mutex::new(BufReader::new(unsafe {
            std::fs::File::from_raw_fd(from_app[0])
        })),
        next: std::sync::atomic::AtomicU64::new(1),
    };
    Ok((child, pipe))
}

/// The browser window of `pid`, hidden or not: the largest at layer 0.
fn hidden_window(pid: i32) -> Option<u64> {
    probe(&["allwindows", &pid.to_string()])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|w| w["layer"].as_i64() == Some(0) && w["width"].as_f64().unwrap_or(0.0) > 400.0)
        .max_by_key(|w| w["width"].as_f64().unwrap_or(0.0) as i64)
        .and_then(|w| w["id"].as_u64())
}

fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => cut(&v.to_string(), 300),
        Err(e) => format!("error: {}", cut(&e.to_string(), 300)),
    }
}

fn test(port: u16, log: &Log) -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Handover clicks", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    // Every front app change through the run, notified and polled every
    // 10 ms: the handed-over browser must never take the front.
    let mut fronts = std::process::Command::new(agent_screen_tests::tool("probe")?)
        .args(["fronts", "180"])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let seen: Log = Arc::default();
    {
        let seen = seen.clone();
        let stdout = fronts.stdout.take().context("probe stdout")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                seen.lock().unwrap_or_else(|e| e.into_inner()).push(line);
            }
        });
    }
    // How long the user's front app, the first one seen, was away, in ms,
    // counting a change still going on.
    let away = || -> u128 {
        let lines = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let parsed: Vec<(u128, String)> = lines
            .iter()
            .filter_map(|l| {
                let mut p = l.split_whitespace();
                Some((p.next()?.parse().ok()?, p.next()?.to_string()))
            })
            .collect();
        let Some((_, users)) = parsed.first().cloned() else {
            return 0;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut longest = 0;
        for (i, (at, pid)) in parsed.iter().enumerate() {
            if *pid != users {
                let until = parsed.get(i + 1).map_or(now, |n| n.0);
                longest = longest.max(until.saturating_sub(*at));
            }
        }
        longest
    };
    let stand_in = std::cell::Cell::new(0i32);
    let result = (|| -> Result<()> {
        // The user's browser, started with no pipe and hidden before it
        // shows a window, as a browser the user hid. Started through
        // LaunchServices with `open -j`, it came to the front as it
        // started, whatever `-g` asked.
        let dir = profile();
        let _ = std::fs::remove_dir_all(&dir);
        let url = format!("http://127.0.0.1:{port}/");
        let (chrome, pipe) = start_stand_in(&dir)?;
        let mut hider = std::process::Command::new(agent_screen_tests::tool("probe")?)
            .args(["hideloop", &chrome.id().to_string(), "20"])
            .spawn()?;
        // Its pipe answers once its browser process is up. AppKit runs its
        // launch handling once its main thread wakes: an accessibility
        // request wakes it, and a window that shows before the handling
        // comes to the front (#241).
        pipe.call("Browser.getVersion", json!({}))?;
        let _ = probe(&["focused", &chrome.id().to_string()]);
        sleep_ms(300);
        pipe.call(
            "Target.createTarget",
            json!({ "url": url, "newWindow": true, "background": true, "width": 1100, "height": 680 }),
        )?;
        let mut window = None;
        for _ in 0..100 {
            sleep_ms(100);
            let gone = away();
            if gone > 100 {
                let _ = hider.kill();
                quit_profile(&profile());
                anyhow::bail!(
                    "the user's front app was away for {gone} ms while the stand-in started; stopped"
                );
            }
            let main = profile_pids(&dir).into_iter().find(|&p| {
                !sh_lenient("/bin/ps", &["-o", "args=", "-p", &p.to_string()]).contains("--type=")
            });
            if let Some(pid) = main
                && let Some(id) = hidden_window(pid)
            {
                window = Some((pid, id));
                break;
            }
        }
        let (pid, window_id) = window.context("the stand-in Chrome showed no window")?;
        stand_in.set(pid);
        // Once its window exists, hidden, stop hiding it; it shows again
        // just before the handover.
        sleep_ms(500);
        let _ = hider.kill();
        let _ = hider.wait();
        let _ = &pipe;
        // The user's front app must not have changed while the stand-in
        // started; if it did, stop before anything else happens.
        let gone = away();
        if gone > 100 {
            quit_profile(&profile());
            anyhow::bail!(
                "the user's front app was away for {gone} ms while the stand-in started; stopped"
            );
        }
        // The user hands over a window they see: shown again, without
        // coming to the front, on their screen until the handover moves it.
        allow_on_your_screen(
            "Google Chrome",
            3000,
            "the stand-in's window is the user's own browser window, on their screen until they hand it over",
        );
        let shown = probe(&["unhide", &pid.to_string()]).unwrap_or_default();
        anyhow::ensure!(
            shown.trim() == "shown",
            "the stand-in Chrome didn't show again: {shown}"
        );
        let placed = control.call(
            "placeWindow",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window_id,
                    "userInstruction": "Take my Chrome window with the test page.", "timeoutSeconds": 10 }),
        );
        check(
            "the user's Chrome window is handed over onto the display",
            placed.is_ok(),
            said(&placed),
        );
        sleep_ms(1500);
        let tree = || -> Result<String> {
            let state = control.call(
                "windowState",
                json!({ "sessionKey": KEY, "displayId": id, "windowId": window_id }),
            )?;
            Ok(state["tree"].as_str().unwrap_or_default().to_string())
        };
        let t = tree()?;
        let click = |(x, y): (f64, f64)| {
            let r = control.call(
                "click",
                json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y }),
            );
            sleep_ms(700);
            r
        };
        let act = |element: u64, action: &str, value: Option<&str>| {
            let mut params = json!({ "sessionKey": KEY, "displayId": id, "windowId": window_id,
                                     "element": element, "action": action });
            if let Some(v) = value {
                params["value"] = json!(v);
            }
            let r = control.call("elementAction", params);
            sleep_ms(700);
            r
        };
        let report = |what: &str, reply: &Result<Value>, events: &[String]| {
            println!("{what}: {} | page {events:?}", said(reply));
        };

        // Posted clicks.
        let (plain_n, plain) = element(&t, "plain button")?;
        let (trigger_n, trigger) = element(&t, "pointer trigger")?;
        let (strict_n, strict) = element(&t, "strict button")?;
        let (field_n, field) = element(&t, "test field")?;
        let (sel_n, sel) = element(&t, "environments")?;
        let has = |events: &[String], e: &str| events.iter().any(|x| x.starts_with(e));
        let from = mark(log);
        let r = click(plain);
        let ev = wait_for(log, from, |ev| has(ev, "plain acted"));
        report("click plain", &r, &ev);
        check(
            "a click on a button fires trusted pointer, mouse and click events, and its click handler",
            has(&ev, "pointerdown plain trusted=true ptype=mouse")
                && has(&ev, "mousedown plain trusted=true")
                && has(&ev, "pointerup plain trusted=true")
                && has(&ev, "click plain trusted=true")
                && has(&ev, "plain acted trusted=true"),
            format!("{ev:?}"),
        );
        let from = mark(log);
        let r = click(trigger);
        let ev = wait_for(log, from, |ev| has(ev, "popover opened"));
        report("click trigger", &r, &ev);
        check(
            "a click on a pointerdown trigger opens its portal popover",
            has(&ev, "popover opened ptype=mouse"),
            format!("{ev:?}"),
        );
        let from = mark(log);
        let r = click(strict);
        let ev = wait_for(log, from, |ev| has(ev, "strict acted"));
        report("click strict", &r, &ev);
        check(
            "a click on a button that wants trusted pointerdown, pointerup and click acts",
            has(&ev, "strict acted"),
            format!("{ev:?}"),
        );
        // A click in the field, then typing: the text goes into the field,
        // never to the page's F shortcut.
        let from = mark(log);
        let r = click(field);
        let typed = control.call(
            "type",
            json!({ "sessionKey": KEY, "displayId": id, "text": "Find me" }),
        );
        let ev = wait_for(log, from, |ev| has(ev, "value Find me|"));
        report("click field, type", &r, &ev);
        check(
            "a click on a text field focuses it, and typed text lands there, not in the page's shortcut",
            has(&ev, "focusin field") && has(&ev, "value Find me|") && !has(&ev, "shortcut F"),
            format!("{} | {ev:?}", said(&typed)),
        );

        // Accessibility routes: which events each delivers is the browser's.
        for (label, n) in [
            ("plain", plain_n),
            ("trigger", trigger_n),
            ("strict", strict_n),
        ] {
            for action in ["press", "click"] {
                let wanted = match label {
                    "plain" => "plain acted",
                    "trigger" => "popover opened",
                    _ => "strict acted",
                };
                let from = mark(log);
                let r = act(n, action, None);
                let ev = wait_for(log, from, |ev| has(ev, wanted));
                report(&format!("elementAction {action} {label}"), &r, &ev);
                if action == "click" {
                    check(
                        &format!(
                            "act_on_element click on the {label} control acts as a click does"
                        ),
                        has(&ev, wanted),
                        format!("{} | {ev:?}", said(&r)),
                    );
                }
            }
        }
        let _ = field_n;

        // The native select: open it with a click, pick an item with a
        // click; then by focus and type-ahead.
        let from = mark(log);
        let r = click(sel);
        // The select's menu is a window of its own at the pop-up menu
        // level, which list_windows leaves out; the window's state lists
        // its items with their frames while it is open (#276). It may take
        // a moment to show.
        let open_menu = || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let t = tree().unwrap_or_default();
                if menu_item(&t, "Preview").is_ok() || std::time::Instant::now() >= until {
                    break t;
                }
                sleep_ms(200);
            }
        };
        let menu = open_menu();
        let menus = probe(&["windows", &pid.to_string()])
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"layer\":101"))
            .count();
        println!("menus {menus}, tree after the click: {}", cut(&menu, 1500));
        check(
            "a click opens the native select's menu on the display, its items in the window state",
            menus > 0 && menu_item(&menu, "Preview").is_ok(),
            format!("open {} | {menus} menu windows", said(&r)),
        );
        let picked = match menu_item(&menu, "Preview") {
            Ok((_, at)) => click(at),
            Err(e) => Err(e),
        };
        // The next click goes out at once: the chosen menu, flashing its
        // item and fading out, is closing, not closed by the user.
        let next = click(plain);
        let ev = wait_for(log, from, |ev| {
            has(ev, "value Find me|Preview") && has(ev, "plain acted")
        });
        report("select by clicks, then a click", &picked, &ev);
        check(
            "a click on an item of the open menu picks it",
            has(&ev, "value Find me|Preview") && has(&ev, "change sel trusted=true"),
            format!("pick {} | {ev:?}", said(&picked)),
        );
        check(
            "a click right after the choice goes out, not refused as if the user closed the menu",
            next.is_ok() && has(&ev, "plain acted"),
            format!("{} | {ev:?}", said(&next)),
        );
        let from = mark(log);
        let set = act(sel_n, "set_value", Some("Development"));
        let ev = since(log, from);
        report("select set_value", &set, &ev);
        check(
            "set_value, with no pipe to this Chrome, fails and says to open the menu with a click",
            set.as_ref()
                .err()
                .is_some_and(|e| e.to_string().contains("click the select")),
            said(&set),
        );
        let from = mark(log);
        let focused = act(sel_n, "focus", None);
        let typed = control.call(
            "type",
            json!({ "sessionKey": KEY, "displayId": id, "text": "Dev" }),
        );
        let ev = wait_for(log, from, |ev| {
            ev.iter().any(|e| e.ends_with("|Development"))
        });
        report("select focus + type", &typed, &ev);
        check(
            "focus and type-ahead choose a select item",
            ev.iter().any(|e| e.ends_with("|Development")),
            format!("focus {} | type {} | {ev:?}", said(&focused), said(&typed)),
        );
        // With its menu open, typed text moves the menu's highlight and
        // chooses nothing; the reply says so, and Return chooses (#276).
        let r = click(sel);
        let menu = open_menu();
        let from = mark(log);
        let typed = control.call(
            "type",
            json!({ "sessionKey": KEY, "displayId": id, "text": "Pro" }),
        );
        let noted = typed
            .as_ref()
            .ok()
            .and_then(|v| v["note"].as_str())
            .is_some_and(|n| n.contains("Return"));
        let ev = since(log, from);
        check(
            "with the menu open, typing chooses nothing and the reply says to press Return",
            menu_item(&menu, "Production").is_ok() && noted && !has(&ev, "change"),
            format!("press {} | type {} | {ev:?}", said(&r), said(&typed)),
        );
        let returned = control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": "Return" }),
        );
        let next = click(plain);
        let ev = wait_for(log, from, |ev| {
            ev.iter().any(|e| e.ends_with("|Production")) && has(ev, "plain acted")
        });
        report("menu open, type + Return, then a click", &returned, &ev);
        check(
            "Return chooses the highlighted item, and a click right after goes out",
            ev.iter().any(|e| e.ends_with("|Production"))
                && next.is_ok()
                && has(&ev, "plain acted"),
            format!(
                "return {} | click {} | {ev:?}",
                said(&returned),
                said(&next)
            ),
        );
        let state = tree().unwrap_or_default();
        println!("final tree excerpt: {}", cut(&state, 1500));
        Ok(())
    })();
    quit_profile(&profile());
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    let _ = fronts.kill();
    let _ = fronts.wait();
    sleep_ms(100);
    let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).join("\n");
    println!("front changes: {:?}", seen.lines().collect::<Vec<_>>());
    let theirs: Vec<&str> = seen
        .lines()
        .filter(|l| l.split_whitespace().nth(1) == Some(&stand_in.get().to_string()))
        .collect();
    check(
        "the handed-over browser never came to the front, sampled every 10 ms and notified",
        stand_in.get() > 0 && theirs.is_empty(),
        format!("{theirs:?}"),
    );
    result
}
