//! Your Dia in front, and an agent's Dia still works (#222). You keep one of
//! your Dia windows focused and typing while an agent launches Dia on its
//! Agent Display. Agent Screen starts the agents' own instance of Dia, a
//! separate process on its own profile (ADR 0020), so it is never your
//! front app: the agent's clicks and keys are posted to it and reach its
//! page, while your window keeps its focus and every key you type.
//!
//! Before #222 the agent's window opened in your Dia process. With your
//! window focused nothing could be posted to it (#110), and Dia's page
//! elements ignore accessibility presses, so the agent's clicks did
//! nothing.
//!
//! Dia is your own browser, so the script skips unless E2E_DIA=1. It never
//! quits, moves, focuses or navigates a window of yours. It serves two
//! pages from a loopback server and prints their address:
//!
//! - `/user`: open it in one of your Dia windows and click its text box.
//!   Keep Dia in front. When /tmp/hidtype/hidtype exists, the script runs
//!   it, and it types "a" as real key presses only while Dia is the front
//!   app, so you can keep your hands off; otherwise type there yourself for
//!   the whole run. The page reports focus, blur and that keys arrive,
//!   never what they are, and whether the box ever holds the agent's text.
//! - `/agent`: launch_app opens it on the Agent Display, in the agents'
//!   instance. It logs its clicks, keys, focus, field values and submits.
//!
//! Then: a click on the agent's field focuses it, typed text lands in it,
//! and a click on its submit button submits the form, each posted, with no
//! accessibility fallback. Your window keeps its focus, sees no blur, never
//! gets the agent's text and keeps getting your keys; Dia stays your front
//! app. At the end the script quits the agents' instance it started on the
//! test instance's profile.
//!
//!     E2E_DIA=1 AGENTSCREEN_INSTANCE=e2e cargo e2e diafocused
//!
//! Makes 1 display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, check, cut, instance, json, probe, quit_profile, run, sh_lenient, sign_step,
    skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "diafocused";
/// The agent's text: you won't type it, so it in your box means the agent's
/// text reached your window.
const TOKEN: &str = "dfq4k";
const HIDTYPE: &str = "/tmp/hidtype/hidtype";
const HIDTYPE_STOP: &str = "/tmp/hidtype/stop";

/// What the pages reported, in order: (page, event).
type Log = Arc<Mutex<Vec<(String, String)>>>;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if std::env::var("E2E_DIA").as_deref() != Ok("1") {
        skip(
            "Dia is your own browser: set E2E_DIA=1 and keep a Dia window of yours in front while it runs",
        );
    }
    let Some(pid) = dia_pids().first().copied() else {
        skip("Dia isn't running: open it and keep one of its windows in front");
    };
    let log: Log = Arc::default();
    let port = match serve(log.clone()) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("no loopback server: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let profile = agents_profile();
    run(|| {
        let user_url = format!("http://127.0.0.1:{port}/user");
        println!(
            "open {user_url} in one of your Dia windows, click its text box and keep Dia in front until the script ends"
        );
        sign_step(&format!(
            "Dia: open {user_url}, click its box, keep Dia in front"
        ));
        let typer = start_hidtype();
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Dia focused", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let result =
            wait_for_user(&log, pid).and_then(|()| test(&control, &id, pid, port, &log, &profile));
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        stop_hidtype(typer);
        quit_profile(&profile);
        stop_test_app();
        result
    })
}

/// The test instance's profile for the agents' Dia (ADR 0020).
fn agents_profile() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home)
        .join("Library/Application Support/Agent Screen")
        .join(format!(
            "Browser Profiles ({})",
            instance().unwrap_or_default()
        ))
        .join("company.thebrowser.dia")
}

fn dia_pids() -> Vec<i32> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Dia"])
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn start_hidtype() -> Option<std::process::Child> {
    if !Path::new(HIDTYPE).exists() {
        println!("no {HIDTYPE}: type in your Dia window yourself for the whole run");
        return None;
    }
    let _ = std::fs::remove_file(HIDTYPE_STOP);
    std::process::Command::new(HIDTYPE)
        .arg("600")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()
}

fn stop_hidtype(child: Option<std::process::Child>) {
    let Some(child) = child else {
        return;
    };
    let _ = std::fs::write(HIDTYPE_STOP, b"");
    if let Ok(output) = child.wait_with_output() {
        println!("{}", String::from_utf8_lossy(&output.stdout).trim());
    }
}

// MARK: The loopback server

const AGENT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diafocused agent</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>diafocused agent</h1>
<form id="form" aria-label="agent form">
  <input id="query" aria-label="agent query" placeholder="agent query">
  <button type="submit" aria-label="agent send">Send</button>
</form>
<script>
const log = e => fetch('/log?w=agent&e=' + encodeURIComponent(e)).catch(() => {});
for (const t of ['pointerdown', 'click', 'keydown', 'focusin']) {
  document.addEventListener(t, ev => log(t + ' ' + (ev.target.id || ev.target.tagName)), true);
}
document.getElementById('form').addEventListener('submit', ev => {
  ev.preventDefault();
  log('submit');
});
let last = null;
setInterval(() => {
  const v = document.getElementById('query').value;
  if (last !== v) { last = v; log('value ' + v); }
}, 100);
log('loaded');
</script></body></html>"#;

/// The user's page reports focus and that keys arrive, never which.
const USER_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>diafocused user</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>diafocused: keep this in front</h1>
<p>Click the box and keep Dia in front until the Agent Screen test ends. This page reports focus and that keys arrive, never what they are.</p>
<textarea id="box" rows="12" cols="80" autofocus></textarea>
<script>
const log = e => fetch('/log?w=user&e=' + encodeURIComponent(e)).catch(() => {});
const box = document.getElementById('box');
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
    let (body, kind) = match path.split_once('?').map_or(path, |(p, _)| p) {
        "/agent" => (AGENT_PAGE.to_string(), "text/html"),
        "/user" => (USER_PAGE.replace("TOKEN", TOKEN), "text/html"),
        "/log" => {
            let query = path.split_once('?').map_or("", |(_, q)| q);
            let field = |name: &str| {
                query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
                    .map(decode)
                    .unwrap_or_default()
            };
            log.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((field("w"), field("e")));
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

/// A query value, percent-decoded.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
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

fn events(log: &Log, page: &str, from: usize) -> Vec<String> {
    log.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .skip(from)
        .filter(|(p, _)| p == page)
        .map(|(_, e)| e.clone())
        .collect()
}

fn mark(log: &Log) -> usize {
    log.lock().unwrap_or_else(|e| e.into_inner()).len()
}

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

// MARK: Dia, read only

fn front_pid() -> Option<i32> {
    probe(&["frontpid"]).ok()?.trim().parse().ok()
}

fn focused_window(pid: i32) -> u64 {
    probe(&["focused", &pid.to_string()])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Your page has focus, keys arrive, and Dia is in front.
fn wait_for_user(log: &Log, pid: i32) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(300) {
        let user = events(log, "user", 0);
        let focused = user.iter().rev().find(|e| e.starts_with("hasfocus"));
        if focused.is_some_and(|f| f == "hasfocus true")
            && user.iter().any(|e| e == "typing")
            && front_pid() == Some(pid)
            && focused_window(pid) != 0
        {
            println!("your Dia window is ready");
            return Ok(());
        }
        sleep_ms(500);
    }
    bail!("the user page didn't report focus and keys within 5 minutes, with Dia in front")
}

// MARK: The checks

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
    control: &Control,
    id: &Value,
    user_pid: i32,
    port: u16,
    log: &Log,
    profile: &Path,
) -> Result<()> {
    let user_window = focused_window(user_pid);
    let from = mark(log);
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Dia",
                "open": format!("http://127.0.0.1:{port}/agent") }),
    )?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .with_context(|| format!("no window: {launched}"))?;
    let agents: Vec<i32> = dia_pids().into_iter().filter(|&p| p != user_pid).collect();
    let args = agents
        .first()
        .map(|p| sh_lenient("/bin/ps", &["-o", "args=", "-p", &p.to_string()]))
        .unwrap_or_default();
    check(
        "launch_app started the agents' own Dia, on its own profile, with the pipe",
        agents.len() == 1
            && args.contains(&profile.display().to_string())
            && args.contains("--remote-debugging-pipe"),
        format!("other Dia pids {agents:?}: {}", cut(&args, 300)),
    );
    check(
        "the agent's page loaded",
        seen_within(log, "agent", from, "loaded", 15_000),
        format!("{:?}", events(log, "agent", from)),
    );
    check(
        "Dia is still your front app, with your window focused",
        front_pid() == Some(user_pid) && focused_window(user_pid) == user_window,
        format!(
            "front {:?}, focused {} (was {user_window})",
            front_pid(),
            focused_window(user_pid)
        ),
    );

    let state = control.call(
        "windowState",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
    )?;
    let tree = state["tree"].as_str().unwrap_or_default().to_string();
    let query = element(&tree, "agent query")?;
    let send = element(&tree, "agent send")?;
    let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });

    let click_mark = mark(log);
    let clicked = control.call("click", at(query));
    check(
        "a click on the agent's field is posted and focuses it",
        posted(&clicked) && seen_within(log, "agent", click_mark, "focusin query", 3000),
        format!(
            "{} | page {:?}",
            cut(&said(&clicked), 600),
            events(log, "agent", click_mark)
        ),
    );
    let typed = control.call(
        "type",
        json!({ "sessionKey": KEY, "displayId": id, "text": TOKEN }),
    );
    check(
        "typed text is posted and lands in the agent's field",
        posted(&typed) && seen_within(log, "agent", click_mark, &format!("value {TOKEN}"), 3000),
        format!(
            "{} | page {:?}",
            cut(&said(&typed), 600),
            events(log, "agent", click_mark)
        ),
    );
    let send_mark = mark(log);
    let sent = control.call("click", at(send));
    check(
        "a click on the agent's submit button is posted and submits the form",
        posted(&sent) && seen_within(log, "agent", send_mark, "submit", 3000),
        format!(
            "{} | page {:?}",
            cut(&said(&sent), 600),
            events(log, "agent", send_mark)
        ),
    );

    // Your keys still arrive in your page after the agent's input.
    let keys_mark = mark(log);
    let still = seen_within(log, "user", keys_mark, "key", 3000);
    let user = events(log, "user", from);
    let touched: Vec<&String> = user
        .iter()
        .filter(|e| ["win blur", "hasfocus false", "token"].contains(&e.as_str()))
        .collect();
    check(
        "your window kept its focus and your keys, saw no blur and never got the agent's text",
        touched.is_empty() && still && focused_window(user_pid) == user_window,
        format!(
            "your page {touched:?}, keys after {still}, focused {} (was {user_window})",
            focused_window(user_pid)
        ),
    );
    check(
        "Dia is still your front app",
        front_pid() == Some(user_pid),
        format!("front {:?}", front_pid()),
    );
    Ok(())
}
