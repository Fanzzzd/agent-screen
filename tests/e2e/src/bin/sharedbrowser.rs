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
//! key focus or sees an event. The agent's select gets a click, a press,
//! set_value and Return: none opens its menu, which would take the user's
//! keyboard, and each answer says nothing was chosen. A click on a link
//! whose page cancels it succeeds with changed false; the button and the
//! submit, which change the page, say changed true. Then, with your front app back, the same
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
    Control, Value, allow_on_your_screen, check, cut, fixture, instance, json, out, probe, restore,
    run, sh_lenient, skip, sleep_ms, snapshot, start_test_app, stop_test_app, tool, wait_for_idle,
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
                    "focus",
                    "active",
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

/// The title of the window an app's accessibility calls focused: the one
/// its keys go to.
fn focused_window(pid: &str) -> String {
    let script = format!(
        "tell application \"System Events\"
            tell (first process whose unix id is {pid})
                try
                    set w to value of attribute \"AXFocusedWindow\"
                    if w is missing value then return \"no focused window\"
                    return name of w
                on error e
                    return \"error: \" & e
                end try
            end tell
        end tell"
    );
    sh_lenient("/usr/bin/osascript", &["-e", &script])
        .trim()
        .to_string()
}

fn key(control: &Control, id: &Value, keys: &str) -> Result<Value> {
    let reply = control.call(
        "key",
        json!({ "sessionKey": KEY, "displayId": id, "keys": keys }),
    );
    sleep_ms(500);
    reply
}

/// Whether the agent's page moved its focus to `field` since `from`: a
/// focus event, or the focused element as the page polls it, since a page
/// whose window isn't key may get no focus event.
fn focus_moved(log: &std::path::Path, from: usize, field: &str) -> bool {
    let seen = lines(log, from);
    has(&seen, &format!("pair agent focus {field}"))
        || has(&seen, &format!("pair agent active {field}"))
}

/// The login form beside the user's window (#110): a click picks its
/// first field, Tab and shift+Tab move focus through accessibility, text
/// goes into the field Tab reached, and Return submits the form once, or
/// fails naming the button that submits it, which a click then presses.
/// The user's window keeps its focus throughout.
fn form(
    control: &Control,
    id: &Value,
    window_id: u64,
    log: &std::path::Path,
    pid: &str,
) -> Result<()> {
    let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });
    let tree1 = tree(control, id, window_id)?;
    let (_, first) = element(&tree1, "agent first")?;
    let (_, submit) = element(&tree1, "agent submit")?;
    let user_focus = focused_window(pid);
    let from = lines(log, 0).len();
    let picked = control.call("click", at(first));
    sleep_ms(300);

    let mark = lines(log, 0).len();
    let tabbed = key(control, id, "Tab");
    let (second, _) = element(&tree(control, id, window_id)?, "agent second")?;
    check(
        "Tab moves focus from the first field to the second in the agent's window",
        picked.is_ok()
            && tabbed.is_ok()
            && focus_moved(log, mark, "second")
            && second.contains("focused"),
        format!(
            "{} | {second} | log {:?}",
            cut(&said(&tabbed), 400),
            lines(log, mark)
        ),
    );
    check(
        "the reply says Tab order is an approximation",
        tabbed
            .as_ref()
            .is_ok_and(|v| v.to_string().contains("approximation")),
        cut(&said(&tabbed), 400),
    );
    let mark = lines(log, 0).len();
    let back = key(control, id, "shift+Tab");
    let (first_line, _) = element(&tree(control, id, window_id)?, "agent first")?;
    check(
        "shift+Tab moves it back to the first field",
        back.is_ok() && focus_moved(log, mark, "first") && first_line.contains("focused"),
        format!(
            "{} | {first_line} | log {:?}",
            cut(&said(&back), 400),
            lines(log, mark)
        ),
    );
    let mark = lines(log, 0).len();
    let again = key(control, id, "Tab");
    check(
        "Tab again reaches the second field",
        again.is_ok() && focus_moved(log, mark, "second"),
        format!("{} | log {:?}", cut(&said(&again), 400), lines(log, mark)),
    );
    let now = focused_window(pid);
    let seen = lines(log, from);
    check(
        "the user's window stays focused and logs no blur",
        now == user_focus && now == "pair user" && !has(&seen, "win user blur"),
        format!("focused window {user_focus:?} → {now:?} | log {seen:?}"),
    );

    let typed = control.call(
        "type",
        json!({ "sessionKey": KEY, "displayId": id, "text": "pw" }),
    );
    sleep_ms(500);
    let (second, _) = element(&tree(control, id, window_id)?, "agent second")?;
    check(
        "type goes into the field Tab reached",
        typed.is_ok() && second.contains("value=\"pw\""),
        format!("{} | {second}", cut(&said(&typed), 400)),
    );

    let submits = |log: &std::path::Path| {
        lines(log, from)
            .iter()
            .filter(|l| l.starts_with("pair agent login-submit"))
            .count()
    };
    let returned = key(control, id, "Return");
    let after_return = submits(log);
    // Without a route, the error must name the form's submit button, and
    // the agent's click on it submits.
    let named = returned
        .as_ref()
        .is_err_and(|e| e.to_string().contains("\"agent submit\""));
    // The page shows "sent", so the click's answer is a success.
    let clicked = if returned.is_err() && named && after_return == 0 {
        let clicked = control.call("click", at(submit));
        sleep_ms(500);
        Some(clicked)
    } else {
        None
    };
    let clicked_ok = clicked
        .as_ref()
        .is_some_and(|c| c.as_ref().is_ok_and(|v| v["changed"] == true));
    let clicked = clicked.as_ref().map(said);
    let seen = lines(log, from);
    check(
        "Return in the second field submits the form once, or names its submit button, which a click submits once",
        submits(log) == 1
            && has(&seen, "pair agent login-submit [\"\",\"pw\"]")
            && (returned.is_ok() || clicked_ok),
        format!(
            "{} | click {clicked:?} | log {seen:?}",
            cut(&said(&returned), 600)
        ),
    );
    let now = focused_window(pid);
    check(
        "the user's window is still the focused one",
        now == "pair user",
        now,
    );
    Ok(())
}

/// The app's open menus, windows at the pop-up menu level, each closed at
/// once through accessibility, as the user's click would: an open menu
/// holds the user's keyboard, and a regression must not leave it so.
fn menus(pid: &str) -> usize {
    let open: Vec<Value> = probe(&["windows", pid])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|w| w["layer"] == 101)
        .collect();
    for w in &open {
        let n = |k: &str| w[k].as_f64().unwrap_or_default();
        let (x, y) = (n("x") + n("width") / 2.0, n("y") + n("height") / 2.0);
        let _ = probe(&["cancelmenu", &x.to_string(), &y.to_string()]);
    }
    open.len()
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

/// The agent's select and a link its page cancels, beside the user's
/// window (#110). A select's press opens a native menu in the user's
/// front app, which takes their keyboard, so no route may press it; and a
/// press the page ignores must not come back as a success.
fn select_and_link(
    control: &Control,
    id: &Value,
    window_id: u64,
    log: &std::path::Path,
    pid: &str,
) -> Result<()> {
    let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });
    let action = |element: u64, action: &str, value: Option<&str>| {
        let mut params = json!({
            "sessionKey": KEY,
            "displayId": id,
            "windowId": window_id,
            "element": element,
            "action": action,
        });
        if let Some(value) = value {
            params["value"] = json!(value);
        }
        let reply = control.call("elementAction", params);
        sleep_ms(500);
        reply
    };
    let tree0 = tree(control, id, window_id)?;
    let (pick_line, pick) = element(&tree0, "agent pick")?;
    let pick_number = number(&tree0, "agent pick")?;
    let (_, dead) = element(&tree0, "agent dead")?;
    let near: Vec<&str> = tree0
        .lines()
        .skip_while(|l| !l.contains("\"agent pick\""))
        .take(6)
        .collect();
    println!("     the select: {pick_line}; then {near:?}");
    let from = lines(log, 0).len();
    let refused_choice = |reply: &Result<Value>| {
        reply.as_ref().is_err_and(|e| {
            let e = e.to_string();
            e.contains("set_value") || e.contains("request_help")
        })
    };

    let clicked = control.call("click", at(pick));
    sleep_ms(700);
    let open = menus(pid);
    check(
        "a click on the agent's select opens no menu",
        open == 0,
        format!("{open} menus | {}", cut(&said(&clicked), 400)),
    );
    check(
        "and fails, saying nothing was chosen and naming set_value",
        refused_choice(&clicked),
        cut(&said(&clicked), 600),
    );

    let pressed = action(pick_number, "press", None);
    sleep_ms(200);
    let open = menus(pid);
    check(
        "act_on_element press on the select opens no menu and fails naming set_value",
        open == 0 && refused_choice(&pressed),
        format!("{open} menus | {}", cut(&said(&pressed), 600)),
    );

    let set = action(pick_number, "set_value", Some("Banana"));
    sleep_ms(200);
    let open = menus(pid);
    let seen = lines(log, from);
    check(
        "set_value on the select, in an app with no DevTools pipe, opens no menu and says nothing was chosen",
        open == 0 && set.is_err() && !has(&seen, "pair agent change"),
        format!("{open} menus | {} | log {seen:?}", cut(&said(&set), 600)),
    );

    let mark = lines(log, 0).len();
    let followed = control.call("click", at(dead));
    sleep_ms(500);
    let seen = lines(log, mark);
    check(
        "a click on a link whose page cancels it reaches the page",
        has(&seen, "pair agent dead-click"),
        format!("{} | log {seen:?}", cut(&said(&followed), 400)),
    );
    check(
        "and it succeeds, saying the press ran but nothing changed",
        followed.as_ref().is_ok_and(|v| {
            v["changed"] == false
                && v["note"]
                    .as_str()
                    .is_some_and(|n| n.contains("press ran, but nothing"))
        }),
        cut(&said(&followed), 600),
    );

    // shift+Tab from the link reaches the select; Return there must not
    // press it open.
    let back = key(control, id, "shift+Tab");
    let reached = back
        .as_ref()
        .is_ok_and(|v| v.to_string().contains("agent pick"));
    let returned = key(control, id, "Return");
    sleep_ms(200);
    let open = menus(pid);
    check(
        "Return on the select Tab reached opens no menu and fails naming set_value",
        reached && open == 0 && refused_choice(&returned),
        format!(
            "{open} menus | shift+Tab {} | Return {}",
            cut(&said(&back), 300),
            cut(&said(&returned), 600)
        ),
    );

    let seen = lines(log, from);
    let now = focused_window(pid);
    check(
        "the select chose nothing and the user's window stays focused",
        !has(&seen, "pair agent change") && now == "pair user" && !has(&seen, "win user blur"),
        format!("focused {now:?} | log {seen:?}"),
    );
    Ok(())
}

fn median(mut ms: Vec<u128>) -> u128 {
    ms.sort_unstable();
    ms[ms.len() / 2]
}

/// What the check for a press's effect costs on a page of about 5000
/// elements: five clicks each on the counter button and the cancelled
/// link, with the check and without it (debugPressCheck), median times.
fn large_page(control: &Control, id: &Value, window_id: u64, log: &std::path::Path) -> Result<()> {
    let at = |(x, y): (i64, i64)| json!({ "sessionKey": KEY, "displayId": id, "x": x, "y": y });
    let tree0 = tree(control, id, window_id)?;
    let (_, grow) = element(&tree0, "agent grow")?;
    let (_, press) = element(&tree0, "agent press")?;
    let (_, dead) = element(&tree0, "agent dead")?;
    let mark = lines(log, 0).len();
    let grown = control.call("click", at(grow));
    sleep_ms(1500);
    check(
        "the agent's page grows by 2500 rows",
        has(&lines(log, mark), "pair agent grown 2500"),
        format!("{} | log {:?}", cut(&said(&grown), 300), lines(log, mark)),
    );
    let mut times = Vec::new();
    for on in [true, false] {
        control.call("debugPressCheck", json!({ "on": on }))?;
        for (name, point) in [("button", press), ("link", dead)] {
            let mut ms = Vec::new();
            let mut changed = Vec::new();
            for _ in 0..5 {
                let started = std::time::Instant::now();
                let reply = control.call("click", at(point));
                ms.push(started.elapsed().as_millis());
                changed.push(match &reply {
                    Ok(v) => v["changed"].to_string(),
                    Err(e) => format!("error: {}", cut(&e.to_string(), 200)),
                });
                sleep_ms(300);
            }
            println!(
                "     large page, check {}: {name} median {} ms of {ms:?}, changed {changed:?}",
                if on { "on" } else { "off" },
                median(ms.clone())
            );
            times.push((on, name, changed));
        }
    }
    control.call("debugPressCheck", json!({ "on": true }))?;
    check(
        "on the large page, the button's clicks say changed true and the link's changed false",
        times.iter().all(|(on, name, changed)| {
            let wanted = match (on, *name) {
                (false, _) => "null",
                (true, "button") => "true",
                (true, _) => "false",
            };
            changed.iter().all(|c| c == wanted)
        }),
        format!("{times:?}"),
    );
    Ok(())
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
    let pid = pid.as_str();
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
        "a click on the agent's button presses it, and the answer says the window changed (changed true)",
        pressed.as_ref().is_ok_and(|v| v["changed"] == true) && has(&seen, "pair agent click 1"),
        format!("{} | log {seen:?}", cut(&said(&pressed), 600)),
    );

    form(control, id, window_id, log, pid)?;
    select_and_link(control, id, window_id, log, pid)?;
    large_page(control, id, window_id, log)?;

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
