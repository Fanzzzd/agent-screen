//! Real Dia, shared with the user (#110): the user types in one Dia
//! window on their screen while the agent works in another window of the
//! same Dia process on its Agent Display. Dia is the user's front app, so
//! posted keys would reach the user's window, and a posted click would make
//! the agent's window key and take their typing. `sharedbrowser` checks the
//! same routes on an owned Electron fixture; this is the acceptance run in
//! the browser the issue names.
//!
//! Dia is your own browser, so the script skips unless E2E_DIA=1, and it
//! never quits, moves, focuses or navigates a window of yours. It serves
//! two pages from a loopback server and prints their address:
//!
//! - `/user`: open it in one of your Dia windows, click its text box and
//!   keep typing there for the whole run. The page reports focus, blur and
//!   that you are typing, never what you type, and whether the box ever
//!   holds the agent's text.
//! - `/agent`: `launch_app` opens it in a new Dia window on the Agent
//!   Display. It has a text field, a form with two buttons and a link, and
//!   logs its keys, clicks, focus, field values and submits. An image the
//!   server answers after 4 s holds its load back, so the check can tell
//!   that your focus came back once its URL showed, not once it loaded.
//!
//! Then: a click on the agent's field, `type`, Backspace, cmd+a with more
//! text, Tab and shift+Tab, a refused key that must name your window,
//! Return in the form's field, which must name the form's buttons, a click
//! on its submit button and a click on the link, which must navigate. Each
//! goes through accessibility, and your window keeps its focus, sees no blur
//! and never gets the agent's text. At the end the script closes only the
//! window it opened.
//!
//!     E2E_DIA=1 AGENTSCREEN_INSTANCE=e2e cargo e2e diashared
//!
//! Makes 1 display.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, check, cut, instance, json, probe, run, sh_lenient, sign_step, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "diashared";
/// The agent's text: you won't type it, so it in your box means the agent's
/// text reached your window.
const TOKEN: &str = "dsq7v";
/// How long the agent page's image takes, which its load waits for.
const SLOW_SECS: u64 = 4;

/// What the pages reported, in order: (page, event).
type Log = Arc<Mutex<Vec<(String, String, Instant)>>>;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if std::env::var("E2E_DIA").as_deref() != Ok("1") {
        skip(
            "Dia is your own browser: set E2E_DIA=1 and keep typing in a Dia window of yours while it runs",
        );
    }
    let Some(pid) = dia_pid() else {
        skip("Dia isn't running: open it and work in one of its windows");
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
        let user_url = format!("http://127.0.0.1:{port}/user");
        println!(
            "open {user_url} in one of your Dia windows, click its text box and keep typing there until the script ends"
        );
        sign_step(&format!("Dia: open {user_url} and keep typing"));
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        // A refused key waits in the Input Queue for Dia to leave the
        // front (#219); a short wait keeps the run short.
        control.call("debugFrontWait", json!({ "ms": 1500 }))?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Dia shared", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let mut opened = None;
        // After the app starts, which may take the front for a moment.
        let result = wait_for_user(&log, pid)
            .and_then(|()| test(&control, &id, pid, port, &log, &mut opened));
        if let Some(window) = opened {
            close(pid, window);
        }
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        stop_test_app();
        result
    })
}

fn dia_pid() -> Option<i32> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Dia"])
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
}

// MARK: The loopback server

const AGENT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diashared agent</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>diashared agent</h1>
<p><input id="name" aria-label="agent name" placeholder="agent name"></p>
<form id="form" aria-label="agent form">
  <input id="query" aria-label="agent query" placeholder="agent query">
  <button type="submit" aria-label="agent send">Send</button>
  <button type="button" aria-label="agent other">Other</button>
</form>
<p><a id="next" href="/next" aria-label="agent next link">Next page</a></p>
<img src="/slow" alt="" width="1" height="1">
<script>
const log = e => fetch('/log?w=agent&e=' + encodeURIComponent(e)).catch(() => {});
window.addEventListener('focus', () => log('win focus'));
window.addEventListener('blur', () => log('win blur'));
for (const t of ['keydown', 'click', 'focusin']) {
  document.addEventListener(t, ev => log(t + ' ' + (ev.target.id || ev.target.tagName)), true);
}
document.getElementById('form').addEventListener('submit', ev => {
  ev.preventDefault();
  log('submit');
  document.title = 'diashared agent sent';
});
const last = {};
setInterval(() => {
  for (const id of ['name', 'query']) {
    const v = document.getElementById(id).value;
    if (last[id] !== v) { last[id] = v; log('value ' + id + ' ' + v); }
  }
  const a = (document.activeElement && document.activeElement.id) || '';
  if (last.active !== a) { last.active = a; log('active ' + a); }
}, 100);
log('loaded');
</script></body></html>"#;

const NEXT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diashared next</title></head>
<body><h1>diashared next</h1><script>fetch('/log?w=agent&e=arrived');</script></body></html>"#;

/// The user's page reports focus and that they type, never what.
const USER_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diashared user</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>diashared: type here</h1>
<p>Click the box and keep typing until the Agent Screen test ends. This page reports focus and that you type, never what you type.</p>
<textarea id="box" rows="12" cols="80" autofocus></textarea>
<script>
const log = e => fetch('/log?w=user&e=' + encodeURIComponent(e)).catch(() => {});
const box = document.getElementById('box');
window.addEventListener('focus', () => log('win focus'));
window.addEventListener('blur', () => log('win blur'));
let typed = 0;
document.addEventListener('keydown', () => {
  log('key');
  const now = Date.now();
  if (now - typed > 1000) { typed = now; log('typing'); }
}, true);
let focus = null, token = false;
setInterval(() => {
  const f = document.hasFocus();
  if (f !== focus) { focus = f; log('hasfocus ' + f); }
  if (!token && box.value.includes('TOKEN')) { token = true; log('token'); }
}, 100);
log('loaded');
</script></body></html>"#;

/// Serve the pages on a free loopback port, logging what they report.
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
    // The rest of the head; GET requests have no body.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    let path = first.split_whitespace().nth(1).unwrap_or("/");
    let (body, kind) = match path.split_once('?').map_or(path, |(p, _)| p) {
        "/agent" => (AGENT_PAGE.to_string(), "text/html"),
        "/next" => {
            note(log, "agent", "GET /next");
            (NEXT_PAGE.to_string(), "text/html")
        }
        "/user" => (USER_PAGE.replace("TOKEN", TOKEN), "text/html"),
        // Holds the agent page's load event back for a few seconds, so its
        // URL shows well before the page has loaded.
        "/slow" => {
            std::thread::sleep(Duration::from_secs(SLOW_SECS));
            note(log, "agent", "slow answered");
            (String::new(), "image/gif")
        }
        "/log" => {
            let query = path.split_once('?').map_or("", |(_, q)| q);
            let field = |name: &str| {
                query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
                    .map(decode)
                    .unwrap_or_default()
            };
            note(log, &field("w"), &field("e"));
            (String::new(), "text/plain")
        }
        _ => (String::new(), "text/plain"),
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

fn note(log: &Log, page: &str, event: &str) {
    log.lock().unwrap_or_else(|e| e.into_inner()).push((
        page.to_string(),
        event.to_string(),
        Instant::now(),
    ));
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
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What `page` reported from entry `from` on.
fn events(log: &Log, page: &str, from: usize) -> Vec<String> {
    log.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .skip(from)
        .filter(|(p, _, _)| p == page)
        .map(|(_, e, _)| e.clone())
        .collect()
}

fn mark(log: &Log) -> usize {
    log.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// Wait up to `ms` for `page` to report `event` after `from`.
fn seen_within(log: &Log, page: &str, from: usize, event: &str, ms: u64) -> bool {
    let started = Instant::now();
    loop {
        if events(log, page, from).iter().any(|e| e == event) {
            return true;
        }
        if started.elapsed() > Duration::from_millis(ms) {
            return false;
        }
        sleep_ms(100);
    }
}

/// The user's page shows they work in it: it has focus and they typed.
fn wait_for_user(log: &Log, pid: i32) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(300) {
        let user = events(log, "user", 0);
        let focused = user.iter().rev().find(|e| e.starts_with("hasfocus"));
        let typing = user.iter().any(|e| e == "typing");
        if focused.is_some_and(|f| f == "hasfocus true")
            && typing
            && front_pid() == Some(pid)
            && focused_window(pid) != 0
        {
            println!("your Dia window is ready");
            return Ok(());
        }
        sleep_ms(500);
    }
    bail!("the user page didn't report focus and typing within 5 minutes, with Dia in front")
}

// MARK: Dia, read only

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
fn windows(pid: i32) -> BTreeSet<u64> {
    probe(&["windows", &pid.to_string()])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|w| w["layer"] == 0)
        .filter_map(|w| w["id"].as_u64())
        .collect()
}

/// Close the window the script opened, and only that one.
fn close(pid: i32, window: u64) {
    let said = probe(&["close", &pid.to_string(), &window.to_string()]).unwrap_or_default();
    sleep_ms(1000);
    check(
        "the script closes the Dia window it opened",
        said.trim() == "closed" && !windows(pid).contains(&window),
        format!("{} | windows {:?}", said.trim(), windows(pid)),
    );
}

// MARK: The checks

/// The line of a labelled element in a window state's tree, and its
/// middle.
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

fn tree(control: &Control, id: &Value, window: u64) -> Result<String> {
    let state = control.call(
        "windowState",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
    )?;
    Ok(state["tree"].as_str().unwrap_or_default().to_string())
}

/// An answer, or the error's text.
fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// Went through accessibility, and says it can't tell whether the press
/// acted: what a press Dia ignores answers, as one on a slow link would.
fn unclear(reply: &Result<Value>) -> bool {
    reply.as_ref().is_ok_and(|v| {
        v["route"] == "accessibility"
            && v["changed"].is_null()
            && v["note"]
                .as_str()
                .is_some_and(|n| n.contains("Take a screenshot before retrying"))
    })
}

fn note_unclear(what: &str, reply: &Result<Value>) {
    if unclear(reply) {
        println!(
            "note: Dia ignored the accessibility press on {what}, and the answer says it can't tell"
        );
    }
}

fn through_accessibility(reply: &Result<Value>) -> bool {
    reply.as_ref().is_ok_and(|v| v["route"] == "accessibility")
}

/// The agent field's latest value, as its page reports it.
fn value(log: &Log, field: &str) -> Option<String> {
    let prefix = format!("value {field} ");
    events(log, "agent", 0).iter().rev().find_map(|e| {
        e.strip_prefix(&prefix)
            .map(String::from)
            .or_else(|| (*e == format!("value {field}")).then(String::new))
    })
}

/// Wait up to 2 s for the agent field to hold `wanted`.
fn value_becomes(log: &Log, field: &str, wanted: &str) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if value(log, field).as_deref() == Some(wanted) {
            return true;
        }
        sleep_ms(100);
    }
    false
}

/// What the user's page reported that shows the agent reached it: a blur,
/// lost focus, or the agent's text in their box.
fn user_touched(log: &Log, from: usize) -> Vec<String> {
    events(log, "user", from)
        .into_iter()
        .filter(|e| matches!(e.as_str(), "win blur" | "hasfocus false" | "token"))
        .collect()
}

fn test(
    control: &Control,
    id: &Value,
    pid: i32,
    port: u16,
    log: &Log,
    opened: &mut Option<u64>,
) -> Result<()> {
    let user_window = focused_window(pid);
    let before = windows(pid);
    let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });
    let key = |keys: &str| {
        let reply = control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": keys }),
        );
        sleep_ms(500);
        reply
    };
    let click = |point: (i64, i64)| {
        let reply = control.call("click", at(point));
        sleep_ms(500);
        reply
    };
    let type_text = |text: &str| {
        let reply = control.call(
            "type",
            json!({ "sessionKey": KEY, "displayId": id, "text": text }),
        );
        sleep_ms(500);
        reply
    };

    // A new Dia window on the display, opened to the agent's page.
    let launch_mark = mark(log);
    let launch_started = Instant::now();
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Dia", "open": format!("http://127.0.0.1:{port}/agent") }),
    );
    let replied = launch_started.elapsed().as_millis();
    let window = launched
        .as_ref()
        .ok()
        .and_then(|v| v["window"]["windowId"].as_u64())
        .filter(|w| !before.contains(w));
    *opened = window;
    let loaded = seen_within(log, "agent", launch_mark, "loaded", 5000);
    check(
        "launch_app opens a new Dia window on the display showing the agent's page",
        window.is_some() && loaded,
        cut(&said(&launched), 400),
    );
    let Some(window) = window else {
        bail!("no new Dia window");
    };
    // The new window took Dia's key focus; Agent Screen hands it back.
    let back = {
        let started = Instant::now();
        while focused_window(pid) != user_window && started.elapsed() < Duration::from_secs(3) {
            sleep_ms(100);
        }
        focused_window(pid) == user_window
    };
    // Your page lost focus while the new window was key, if it reported
    // that at all. launch_app must give it back as soon as the new window
    // shows the agent's URL, before that page's slow image lets it finish
    // loading: every call hands focus back as it ends, so a hand-back only
    // after the call's own wait for the page would come after the image.
    let launch_events: Vec<(String, String, Instant)> = {
        let all = log.lock().unwrap_or_else(|e| e.into_inner());
        all.iter().skip(launch_mark).cloned().collect()
    };
    let at = |page: &str, event: &str, after: usize| {
        launch_events
            .iter()
            .skip(after)
            .position(|(p, e, _)| p == page && e == event)
            .map(|i| i + after)
    };
    let lost = at("user", "hasfocus false", 0);
    let regained = lost.and_then(|i| at("user", "hasfocus true", i));
    let slow = at("agent", "slow answered", 0);
    let early = match (lost, regained, slow) {
        // Your page never saw its window lose focus: nothing to hand back.
        (None, _, _) => true,
        (Some(_), Some(r), Some(s)) => r < s,
        (Some(_), Some(_), None) => true,
        (Some(_), None, _) => false,
    };
    // When each page reported what, in ms from the launch_app call.
    let ms = |t: &Instant| t.duration_since(launch_started).as_millis();
    let timeline: Vec<String> = launch_events
        .iter()
        .filter(|(_, e, _)| e != "typing" && e != "key")
        .map(|(p, e, t)| format!("{} {p} {e}", ms(t)))
        .collect();
    println!("launch_app answered at {replied} ms; pages: {timeline:?}");
    let key_window_ms = match (lost, regained) {
        (Some(l), Some(r)) => Some(ms(&launch_events[r].2) - ms(&launch_events[l].2)),
        _ => None,
    };
    let launch_user = events(log, "user", launch_mark);
    check(
        "launch_app gives your Dia window its focus back once the agent's URL shows, before that page loads",
        back && early,
        format!(
            "focused window {user_window} → {} | lost at {lost:?}, regained at {regained:?}, page's image answered at {slow:?} | your page {launch_user:?}",
            focused_window(pid)
        ),
    );
    let leaked: Vec<_> = events(log, "agent", launch_mark)
        .into_iter()
        .filter(|e| e.starts_with("keydown"))
        .collect();
    check(
        "none of your keys reached the agent's page during launch_app",
        leaked.is_empty(),
        format!(
            "{leaked:?} | your window was not key for {key_window_ms:?} ms, launch_app answered at {replied} ms"
        ),
    );

    // Your keys typed while the new window had Dia's key focus were held
    // and sent on to your window once it had its focus back.
    let held = control.call("debugKeyHold", json!({})).unwrap_or_default();
    let held_downs = held["keyDowns"].as_u64().unwrap_or(0) as usize;
    let until = launch_started + Duration::from_millis(replied as u64 + 500);
    let reached = {
        let all = log.lock().unwrap_or_else(|e| e.into_inner());
        all.iter()
            .skip(launch_mark)
            .filter(|(p, e, t)| p == "user" && e == "key" && *t <= until)
            .count()
    };
    println!(
        "your keys were held for {} ms: {held}",
        held["ms"].as_u64().unwrap_or(0)
    );
    check(
        "every key held during launch_app reached your window after the hand-back",
        held["end"] == "HandedBack" && held["sent"] == true && reached >= held_downs,
        format!("hold {held} | your page got {reached} keys during launch_app"),
    );

    // The premise of every check after: Dia in front, your window focused.
    if front_pid() != Some(pid) || focused_window(pid) != user_window {
        bail!("Dia must stay your front app with your window focused; keep typing in it");
    }
    let from = mark(log);
    let t = tree(control, id, window)?;
    let name = element(&t, "agent name")?;
    let query = element(&t, "agent query")?;
    let send = element(&t, "agent send")?;
    let next = element(&t, "agent next link")?;

    let picked = click(name);
    check(
        "a click on the agent's text field picks it through accessibility",
        through_accessibility(&picked),
        cut(&said(&picked), 400),
    );
    let text = format!("{TOKEN}-agent");
    let typed = type_text(&text);
    check(
        "type sets the agent's field",
        through_accessibility(&typed) && value_becomes(log, "name", &text),
        format!(
            "{} | value {:?}",
            cut(&said(&typed), 300),
            value(log, "name")
        ),
    );
    let deleted = key("BackSpace");
    let shorter = &text[..text.len() - 1];
    check(
        "BackSpace deletes the last character of the agent's field",
        deleted.is_ok() && value_becomes(log, "name", shorter),
        format!(
            "{} | value {:?}",
            cut(&said(&deleted), 300),
            value(log, "name")
        ),
    );
    let all = key("cmd+a");
    let replaced = type_text(&format!("{TOKEN}-all"));
    check(
        "cmd+a then type replaces the agent field's text",
        all.is_ok() && replaced.is_ok() && value_becomes(log, "name", &format!("{TOKEN}-all")),
        format!(
            "{} | {} | value {:?}",
            cut(&said(&all), 200),
            cut(&said(&replaced), 200),
            value(log, "name")
        ),
    );

    let tab_mark = mark(log);
    let tabbed = key("Tab");
    check(
        "Tab moves focus from the agent's name field to its query field",
        tabbed.is_ok() && seen_within(log, "agent", tab_mark, "active query", 1500),
        format!(
            "{} | page {:?}",
            cut(&said(&tabbed), 300),
            events(log, "agent", tab_mark)
        ),
    );
    let typed_query = type_text("q1");
    check(
        "type goes into the field Tab reached",
        typed_query.is_ok() && value_becomes(log, "query", "q1"),
        format!(
            "{} | value {:?}",
            cut(&said(&typed_query), 300),
            value(log, "query")
        ),
    );
    let back_mark = mark(log);
    let shifted = key("shift+Tab");
    check(
        "shift+Tab moves focus back to the name field",
        shifted.is_ok() && seen_within(log, "agent", back_mark, "active name", 1500),
        format!(
            "{} | page {:?}",
            cut(&said(&shifted), 300),
            events(log, "agent", back_mark)
        ),
    );

    let refused = key("cmd+l");
    let refusal = said(&refused);
    check(
        "a key with no route is refused and names your focused window",
        refused.is_err()
            && refusal.contains("their window")
            // Dia's titles start with the profile, as "Personal: ".
            && (refusal.contains("diashared user\"")
                || refusal.contains(&format!("window {user_window}"))),
        cut(&refusal, 400),
    );

    let _ = click(query);
    let return_mark = mark(log);
    let returned = key("Return");
    sleep_ms(500);
    check(
        "Return in the form's field names its buttons and submits nothing",
        returned
            .as_ref()
            .is_err_and(|e| e.to_string().contains("\"agent send\""))
            && !events(log, "agent", return_mark)
                .iter()
                .any(|e| e == "submit"),
        cut(&said(&returned), 400),
    );
    let send_mark = mark(log);
    let sent = click(send);
    let submitted = seen_within(log, "agent", send_mark, "submit", 2000);
    note_unclear("the submit button", &sent);
    check(
        "a click on the form's submit button submits it, or says it can't tell and submits nothing",
        (through_accessibility(&sent) && submitted) || (unclear(&sent) && !submitted),
        format!(
            "{} | page {:?}",
            cut(&said(&sent), 1500),
            events(log, "agent", send_mark)
        ),
    );

    // The issue's original report: a link click went nowhere.
    let link_mark = mark(log);
    let followed = click(next);
    let arrived = seen_within(log, "agent", link_mark, "arrived", 5000);
    note_unclear("the link", &followed);
    check(
        "a click on the agent's link navigates its window, or says it can't tell and goes nowhere",
        (through_accessibility(&followed) && arrived) || (unclear(&followed) && !arrived),
        format!(
            "{} | page {:?}",
            cut(&said(&followed), 1500),
            events(log, "agent", link_mark)
        ),
    );

    let touched = user_touched(log, from);
    check(
        "your window kept its focus, saw no blur and never got the agent's text",
        touched.is_empty() && focused_window(pid) == user_window,
        format!(
            "your page {touched:?} | focused window {user_window} → {}",
            focused_window(pid)
        ),
    );
    check(
        "Dia is still your front app",
        front_pid() == Some(pid),
        format!("front pid {:?}", front_pid()),
    );
    let mut kept = before.clone();
    kept.retain(|w| windows(pid).contains(w));
    check(
        "all your Dia windows are still open",
        kept == before,
        format!("{before:?} → {:?}", windows(pid)),
    );
    Ok(())
}
