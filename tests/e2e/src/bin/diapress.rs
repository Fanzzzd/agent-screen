//! Accessibility presses in real Dia, shared with the user (#222). It
//! measures whether AXEnhancedUserInterface makes Dia's page elements list
//! AXPress, and checks that act_on_element press reaches the page either
//! way. Dia 1.58 answers kAXErrorNotImplemented to the attribute and keeps
//! it false, so its elements list no AXPress and a press is a posted click.
//!
//! Dia is your own browser, so the script skips unless E2E_DIA=1. It never
//! closes, moves or navigates a window of yours, and you don't need to
//! touch the Mac. With your app in front (Finder when you start it from
//! Dia), it opens its page in a new Dia window on the Agent Display with
//! `launch_app` and reads the actions each page element lists. Then it
//! sets AXEnhancedUserInterface for Dia (debugEnhancedUi), reads them
//! again and prints both. `act_on_element press` on a button, a link and a
//! div with role=button must fire the page's click handler, through
//! accessibility where the element lists AXPress and as a posted click
//! where not. If Dia has a window of yours, the script brings Dia forward
//! twice, a few seconds each, without raising a window, so that window has
//! focus: a click on the button must go in through accessibility where
//! AXPress is listed, or wait in the Input Queue (withdrawn) where not,
//! while your window keeps focus and the front app stays Dia (sampled
//! every 10 ms); then a click on the text field and `type "abc"` print
//! whether Dia's fields take text through AXValue. It prints one
//! `place_window` of its own window and Dia's CPU time over 10 s with the
//! attribute set and unset, and checks your Dia windows' frames and
//! AeroSpace's focused workspace. At the end it puts back the attribute's
//! value it found, gives the front back to your app and closes only the
//! window it opened.
//!
//!     E2E_DIA=1 AGENTSCREEN_INSTANCE=e2e cargo e2e diapress
//!
//! Brings Dia forward at most twice, a few seconds each. Makes 1 display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, Value, check, cut, fronts_during, instance, json, probe, run, sh_lenient, skip,
    sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "diapress";

/// What the agent's page reported, in order.
type Log = Arc<Mutex<Vec<String>>>;

const AGENT_PAGE: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>diapress agent</title></head>
<body style="font: 16px -apple-system; padding: 24px">
<h1>diapress agent</h1>
<p><button id="b" aria-label="press button">Button</button></p>
<p><a id="l" href="#x" aria-label="press link">Link</a></p>
<div id="root"><p><div id="r" role="button" tabindex="0" aria-label="press role" style="display:inline-block;padding:6px 12px;border:1px solid #888">Role button</div></p></div>
<p><input id="f" aria-label="press field" placeholder="field"></p>
<p><select id="s" aria-label="press pick"><option>Alpha</option><option>Beta</option><option>Gamma</option></select></p>
<script>
const log = e => fetch('/log?e=' + encodeURIComponent(e)).catch(() => {});
const who = t => (t && t.closest && t.closest('[id]') || {}).id || '?';
for (const type of ['pointerdown', 'mousedown', 'click', 'change', 'keydown']) {
  document.addEventListener(type, ev => log(type + ' ' + who(ev.target) + (ev.key ? ' ' + ev.key : '')), true);
}
// A React-style handler: one listener at the root, not on the element.
document.getElementById('root').addEventListener('click', ev => {
  if (ev.target.closest('[role=button]')) log('root click r');
});
document.getElementById('f').addEventListener('input', ev => log('input f ' + ev.target.value));
log('loaded');
</script></body></html>"##;

/// The page elements the script presses, by label, and the id the page
/// logs for each.
const PRESSED: [(&str, &str); 3] = [
    ("press button", "b"),
    ("press link", "l"),
    ("press role", "r"),
];

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
        let original = front_pid().context("no front app")?;
        let yours = if original == dia {
            finder_pid().context("Dia is in front and Finder isn't running")?
        } else {
            original
        };
        let workspace = aerospace_workspace();
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let found = enhanced(&control, dia, None, None)?;
        let was = found["enhanced"].as_bool();
        println!(
            "     Dia {dia}: AXManualAccessibility {}, AXEnhancedUserInterface {}",
            found["manual"], found["enhanced"]
        );
        control.call("debugFrontWait", json!({ "ms": 1500 }))?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Dia press", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let mut opened = None;
        let result = test(&control, &id, dia, yours, port, &log, &mut opened);
        // The attribute back as the script found it, your app back in
        // front, then only the script's window closes, on the display.
        let back = enhanced(&control, dia, Some(was.unwrap_or(false)), None);
        let now = back.as_ref().ok().and_then(|v| v["enhanced"].as_bool());
        check(
            "Dia's AXEnhancedUserInterface is back to the value the script found",
            now.unwrap_or(false) == was.unwrap_or(false),
            format!("found {was:?}, now {now:?}"),
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
        let _ = control.call("debugFrontWait", json!({ "ms": 0 }));
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        stop_test_app();
        check(
            "the app you were in is in front again",
            front_pid() == Some(original),
            format!("front {:?}, yours {original}", front_pid()),
        );
        let after = aerospace_workspace();
        check(
            "AeroSpace's focused workspace is the one you had",
            after == workspace,
            format!("before {workspace:?}, after {after:?}"),
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
    // Your app in front: Dia takes posted input, so the presses below
    // test accessibility alone.
    if front_pid() != Some(yours) {
        probe(&["activate", &yours.to_string()])?;
        sleep_ms(500);
    }
    let before = windows(dia);
    let yours_before = frames(dia);
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
    *opened = window.or_else(|| {
        sleep_ms(1000);
        windows(dia).into_iter().find(|w| !before.contains(w))
    });
    check(
        "launch_app opens a new Dia window on the display",
        window.is_some() && seen(log, 0, "loaded", 5000),
        cut(&said(&launched), 300),
    );
    let Some(window) = window else {
        bail!("no new Dia window");
    };
    let tree0 = tree(control, id, window)?;
    let lines: Vec<&str> = tree0.lines().filter(|l| l.contains("\"press ")).collect();
    println!("     the page's elements: {lines:?}");

    // Step 4: what Dia lists with AXManualAccessibility alone.
    let base = enhanced(control, dia, None, Some(window))?;
    let base_actions = page_actions(&base);
    println!("     actions before: {base_actions:?}");
    let base_press = presses(&base_actions);

    // Step 5: AXEnhancedUserInterface on.
    let started = Instant::now();
    let on = enhanced(control, dia, Some(true), Some(window))?;
    println!(
        "     set on: answered {}, now {}",
        on["answered"], on["enhanced"]
    );
    let mut on_actions = page_actions(&on);
    while presses(&on_actions) < 2 && started.elapsed() < Duration::from_secs(2) {
        sleep_ms(200);
        on_actions = page_actions(&enhanced(control, dia, None, Some(window))?);
    }
    println!(
        "     actions after {} ms: {on_actions:?}",
        started.elapsed().as_millis()
    );
    let took = on["enhanced"] == true;
    let lists = |label: &str| {
        on_actions
            .iter()
            .any(|(l, a)| l == label && a.iter().any(|a| a == "AXPress"))
    };
    let listed = lists("press button") && lists("press link");
    println!(
        "     Dia takes AXEnhancedUserInterface: {took}; the button and the link list AXPress with it: {listed} ({base_press} pressable before, {} after)",
        presses(&on_actions)
    );

    // Step 6: act_on_element press on each, through accessibility where
    // the element lists AXPress, as a posted click where it doesn't.
    let tree1 = tree(control, id, window)?;
    let mut fired = Vec::new();
    for (label, page_id) in PRESSED {
        let Ok(number) = number(&tree1, label) else {
            check(&format!("{label} is in the window state"), false, "");
            continue;
        };
        let from = mark(log);
        let reply = control.call(
            "elementAction",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window,
                    "element": number, "action": "press" }),
        );
        let clicked = seen(log, from, &format!("click {page_id}"), 1500);
        let page = since(log, from);
        // A posted click carries a note; a press through accessibility none.
        let route = match &reply {
            Ok(v) if v["note"].is_string() => "posted click",
            Ok(_) => "accessibility",
            Err(_) => "error",
        };
        let expected = if lists(label) {
            "accessibility"
        } else {
            "posted click"
        };
        println!("     press {label}: {route}, page {page:?}");
        fired.push((label, route, clicked, route == expected));
    }
    let press_fires = fired.len() == PRESSED.len()
        && fired
            .iter()
            .all(|(_, _, clicked, expected)| *clicked && *expected);
    check(
        "act_on_element press fires the page's click handler on the button, the link and the role button, through accessibility where AXPress is listed and as a posted click where not",
        press_fires,
        format!("{fired:?}"),
    );

    // Step 7: your Dia window focused, a click on the button beside it.
    let (_, button) = element(&tree1, "press button")?;
    let (_, field) = element(&tree1, "press field")?;
    let user_window = bring_dia(dia)?;
    let mut beside = false;
    if user_window == window {
        leave_dia(yours);
        println!(
            "     steps 7 and 4.3 skipped: Dia focused the script's window, so it has no window of yours to work beside (your Dia windows on screen: {})",
            yours_before.len()
        );
    } else {
        beside = true;
        let dia_text = dia.to_string();
        let from = mark(log);
        let focus = Arc::new(Mutex::new(Vec::new()));
        let (clicked, fronts) = fronts_during(3, Some(&dia_text), || {
            let watch = sample_focus(dia, focus.clone(), Duration::from_millis(1500));
            let reply = control.call(
                "click",
                json!({ "sessionKey": KEY, "displayId": id, "x": button.0, "y": button.1 }),
            );
            let _ = watch.join();
            reply
        })?;
        let page = since(log, from);
        let focus = focus.lock().unwrap_or_else(|e| e.into_inner()).clone();
        leave_dia(yours);
        withdraw(control, id, &clicked);
        println!(
            "     click beside your window: {}",
            cut(&said(&clicked), 400)
        );
        let (what, right) = if lists("press button") {
            (
                "goes in through accessibility, changed true",
                clicked
                    .as_ref()
                    .is_ok_and(|v| v["route"] == "accessibility" && v["changed"] == true)
                    && page.iter().any(|e| e == "click b"),
            )
        } else {
            (
                "lists no AXPress, so it waits in the Input Queue and nothing reaches the page",
                said(&clicked).contains("input_pending") && page.is_empty(),
            )
        };
        check(
            &format!("a click on the button beside your focused Dia window {what}"),
            right,
            format!("page {page:?}"),
        );
        check(
            "your Dia window keeps focus while it does (every 10 ms)",
            !focus.is_empty() && focus.iter().all(|w| *w == user_window),
            format!(
                "yours {user_window}, seen {:?}",
                focus.iter().collect::<std::collections::BTreeSet<_>>()
            ),
        );
        check(
            "the front app stays Dia, nothing else comes forward",
            fronts
                .iter()
                .filter(|l| !l.contains(" order:"))
                .all(|l| l.split_whitespace().nth(1) == Some(&dia_text)),
            format!("{fronts:?}"),
        );

        // 4.3: the text field beside your focused Dia window.
        bring_dia(dia)?;
        let from = mark(log);
        let picked = control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": field.0, "y": field.1 }),
        );
        let typed = control.call(
            "type",
            json!({ "sessionKey": KEY, "displayId": id, "text": "abc" }),
        );
        leave_dia(yours);
        let page = since(log, from);
        withdraw(control, id, &picked);
        withdraw(control, id, &typed);
        println!(
            "     field click: {} | type: {} | page {page:?}",
            cut(&said(&picked), 300),
            cut(&said(&typed), 300)
        );
        let through_value = typed.as_ref().is_ok_and(|v| v["route"] == "accessibility")
            && page.iter().any(|e| e == "input f abc");
        println!(
            "     4.3: type in Dia's field beside your window {}",
            if through_value {
                "went in through AXValue"
            } else if said(&typed).contains("input_pending") {
                "waited in the Input Queue (withdrawn)"
            } else {
                "failed"
            }
        );
    }

    // Step 8: side effects with it set.
    let placed_at = Instant::now();
    let placed = control.call(
        "placeWindow",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window,
                "x": 40, "y": 40, "width": 900, "height": 640 }),
    );
    let place_ms = placed_at.elapsed().as_millis();
    println!(
        "     place_window with it set: {place_ms} ms, {}",
        cut(&said(&placed), 200)
    );
    let cpu_on = cpu_seconds(Duration::from_secs(10));
    enhanced(control, dia, Some(false), None)?;
    let cpu_off = cpu_seconds(Duration::from_secs(10));
    println!("     Dia CPU over 10 s: set {cpu_on:.2} s, unset {cpu_off:.2} s");
    let after: Vec<_> = frames(dia).into_iter().filter(|f| f.0 != window).collect();
    check(
        "your Dia windows' frames are unchanged",
        after == yours_before,
        format!("before {yours_before:?}, after {after:?}"),
    );
    println!(
        "     Phase C probe (AXPress listed, press fires, click beside your window): {}",
        match (listed, beside) {
            (false, _) => "FAIL, Dia lists no AXPress",
            (true, false) => "UNKNOWN, no window of yours to click beside",
            (true, true) => "see the checks above",
        }
    );
    Ok(())
}

/// Withdraw the input request a reply names, if any.
fn withdraw(control: &Control, id: &Value, reply: &Result<Value>) {
    if let Some(n) = request_number(reply) {
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "withdraw", "requestId": n }),
        );
    }
}

/// debugEnhancedUi: Dia's flags, with `on` set first and `window`'s page
/// elements listed.
fn enhanced(control: &Control, dia: i32, on: Option<bool>, window: Option<u64>) -> Result<Value> {
    let mut params = json!({ "pid": dia });
    if let Some(on) = on {
        params["on"] = json!(on);
    }
    if let Some(window) = window {
        params["windowId"] = json!(window);
    }
    control.call("debugEnhancedUi", params)
}

/// The labelled page elements, each with the actions it lists.
fn page_actions(state: &Value) -> Vec<(String, Vec<String>)> {
    state["elements"]
        .as_array()
        .map(|e| {
            e.iter()
                .filter_map(|e| {
                    let label = e["label"].as_str()?;
                    label.starts_with("press ").then(|| {
                        let actions = e["actions"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|a| a.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        (label.to_string(), actions)
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn presses(actions: &[(String, Vec<String>)]) -> usize {
    actions
        .iter()
        .filter(|(_, a)| a.iter().any(|a| a == "AXPress"))
        .count()
}

/// Dia forward without raising a window: the window you used last has
/// its focus. That window.
fn bring_dia(dia: i32) -> Result<u64> {
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!(
                "tell application \"System Events\" to set frontmost of (first process whose unix id is {dia}) to true"
            ),
        ],
    );
    let started = Instant::now();
    while (front_pid() != Some(dia) || focused_window(dia) == 0)
        && started.elapsed() < Duration::from_secs(2)
    {
        sleep_ms(100);
    }
    let focused = focused_window(dia);
    if front_pid() != Some(dia) || focused == 0 {
        bail!(
            "Dia didn't come forward with a focused window: front {:?}, focused {focused}",
            front_pid()
        );
    }
    Ok(focused)
}

fn leave_dia(yours: i32) {
    let _ = probe(&["activate", &yours.to_string()]);
    sleep_ms(500);
}

/// Dia's focused window every 10 ms for `span`.
fn sample_focus(
    dia: i32,
    into: Arc<Mutex<Vec<u64>>>,
    span: Duration,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let started = Instant::now();
        while started.elapsed() < span {
            let w = focused_window(dia);
            into.lock().unwrap_or_else(|e| e.into_inner()).push(w);
            sleep_ms(10);
        }
    })
}

/// CPU seconds all of Dia's processes use over `span`.
fn cpu_seconds(span: Duration) -> f64 {
    let before = dia_cpu();
    std::thread::sleep(span);
    let after = dia_cpu();
    after
        .iter()
        .map(|(pid, t)| t - before.get(pid).copied().unwrap_or(0.0))
        .filter(|d| *d >= 0.0)
        .sum()
}

/// Each Dia process's CPU time so far, in seconds.
fn dia_cpu() -> std::collections::HashMap<i32, f64> {
    sh_lenient("/bin/ps", &["-axo", "pid=,time=,command="])
        .lines()
        .filter(|l| l.contains("/Applications/Dia.app/"))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            Some((pid, cpu_time(parts.next()?)?))
        })
        .collect()
}

/// `ps`'s time, [[hh:]mm:]ss.cc, in seconds.
fn cpu_time(s: &str) -> Option<f64> {
    s.split(':').try_fold(0.0, |total, part| {
        Some(total * 60.0 + part.parse::<f64>().ok()?)
    })
}

fn aerospace_workspace() -> String {
    sh_lenient("aerospace", &["list-workspaces", "--focused"])
        .trim()
        .to_string()
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

/// Dia's on-screen windows' records, by id.
fn window_records(pid: i32) -> Vec<Value> {
    probe(&["windows", &pid.to_string()])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|w| w["layer"] == 0)
        .collect()
}

fn windows(pid: i32) -> Vec<u64> {
    window_records(pid)
        .iter()
        .filter_map(|w| w["id"].as_u64())
        .collect()
}

/// Your Dia windows' frames: id, x, y, width, height.
fn frames(pid: i32) -> Vec<(u64, i64, i64, i64, i64)> {
    let mut frames: Vec<_> = window_records(pid)
        .iter()
        .filter_map(|w| {
            let n = |k: &str| w[k].as_f64().map(|v| v.round() as i64);
            Some((
                w["id"].as_u64()?,
                n("x")?,
                n("y")?,
                n("width")?,
                n("height")?,
            ))
        })
        .collect();
    frames.sort();
    frames
}

fn tree(control: &Control, id: &Value, window: u64) -> Result<String> {
    let state = control.call(
        "windowState",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
    )?;
    Ok(state["tree"].as_str().unwrap_or_default().to_string())
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

/// The number get_window_state gives the element labelled `label`.
fn number(tree: &str, label: &str) -> Result<u64> {
    let (line, _) = element(tree, label)?;
    line.trim_start()
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .and_then(|n| n.parse().ok())
        .with_context(|| format!("no number in {line}"))
}

fn request_number(reply: &Result<Value>) -> Option<u64> {
    if let Ok(v) = reply {
        return v["requestId"].as_u64();
    }
    let text = said(reply);
    let at = text.find("input request ")? + "input request ".len();
    text[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
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
