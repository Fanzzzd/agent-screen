//! act_on_element press on an Electron app's menu item (issue #39). The
//! fixture's Go > Previous view has no shortcut and writes
//! `event.triggeredByAccelerator` from its click handler. AppKit runs an
//! accessibility press with no current event, and Electron reads anything
//! but a mouse event as a shortcut, which apps drop while a text field has
//! focus.
//!
//! With no field focused, the press goes through accessibility, arrives
//! as a shortcut, asks nothing and says so in a note. With the page's
//! field focused after typing in it, the press asks for the user's pointer
//! (ADR 0013, "Menu items of Electron apps"); allowed, as the card's Allow
//! Queue does, a real click on the Agent Display's menu bar and on the
//! item chooses it once, as a click. The field keeps its text and its
//! focus, the page sees no click or key, no menu stays open, and the
//! pointer and your front app come back. Declined, nothing is chosen.
//! With real_input false, the press goes through accessibility again.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e menupress
//!
//! Makes 1 display. Moves your pointer for a second: keep your hands off
//! the mouse and keyboard while the sign says so.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, check, fixture, instance, json, out, probe, run, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "menupress";

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> Point;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *mut std::ffi::c_void);
}

fn cursor() -> Point {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        let at = CGEventGetLocation(event);
        CFRelease(event);
        at
    }
}

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
    run(|| {
        allow_on_your_screen(
            "Electron",
            800,
            "Electron brings itself to the front as its first window shows and answers no accessibility call for about 0.5 s after, so nothing can move the window sooner (as in `electron`)",
        );
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let mut pid = None;
        let result = test(&electron, &mut pid);
        // Only the instance this script started: another script's may run.
        if let Some(pid) = pid {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.to_string())
                .status();
        }
        stop_test_app();
        result
    })
}

/// An element's number in a window state's tree.
fn number(tree: &str, pattern: &str) -> Result<i64> {
    let found = Regex::new(&format!(r"\[(\d+)\] {pattern}"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern} in\n{tree}"))?;
    Ok(found[1].parse()?)
}

/// A count the page writes into its title, as `down=2`.
fn seen(title: &str, key: &str) -> String {
    title
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .unwrap_or("0")
        .to_string()
}

fn lines(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

/// Lines the log gained after `before`, once it has kept still for `quiet`
/// ms, waiting at most 3 s for the first.
fn new_lines(log: &std::path::Path, before: usize, quiet: u64) -> Vec<String> {
    for _ in 0..30 {
        if lines(log).len() > before {
            break;
        }
        sleep_ms(100);
    }
    sleep_ms(quiet);
    lines(log).split_off(before.min(lines(log).len()))
}

fn test(electron: &std::path::Path, pid: &mut Option<i64>) -> Result<()> {
    let control = Control::connect()?;
    let log = out().join("menupress.log");
    let _ = std::fs::remove_file(&log);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Menu press", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].clone();
    let result = cases(&control, electron, &id, &log, pid);
    let _ = control.call(
        "inputQueue",
        json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
    );
    let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}

fn cases(
    control: &Control,
    electron: &std::path::Path,
    id: &Value,
    log: &std::path::Path,
    pid: &mut Option<i64>,
) -> Result<()> {
    let launched = control.call(
        "launchApp",
        json!({
            "sessionKey": KEY,
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_MENU_LOG": log.to_str() },
        }),
    )?;
    *pid = launched["pid"].as_i64();
    let app = pid.context("no pid")?.to_string();
    let window = launched["window"]["windowId"].clone();
    let at = |params: Value| {
        let mut params = params;
        params["sessionKey"] = json!(KEY);
        params["displayId"] = id.clone();
        params["windowId"] = window.clone();
        params
    };
    let state = || -> Result<String> {
        Ok(control.call("windowState", at(json!({})))?["tree"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    let title = || -> Result<String> {
        let list = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
        Ok(list["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"] == window))
            .and_then(|w| w["title"].as_str())
            .unwrap_or_default()
            .to_string())
    };
    let status = || {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "status" }),
        )
    };
    // The item's number, read from the Go menu as an agent would.
    let item = || -> Result<i64> {
        let go = number(&state()?, r#"menubaritem "Go""#)?;
        let menu = control.call(
            "elementAction",
            at(json!({ "element": go, "action": "press" })),
        )?;
        number(
            menu["menu"].as_str().unwrap_or_default(),
            r#"menuitem "Previous view""#,
        )
    };
    let press = |real_input: Option<bool>| -> Result<Result<Value>> {
        let element = item()?;
        let mut params = at(json!({ "element": element, "action": "press" }));
        if let Some(real) = real_input {
            params["realInput"] = json!(real);
        }
        Ok(control.call("elementAction", params))
    };
    // A press the user answers from the card while the call waits.
    let answered = |surface: &str| -> Result<Result<Value>> {
        let element = item()?;
        let call = control.begin(
            "elementAction",
            at(json!({ "element": element, "action": "press" })),
        )?;
        sleep_ms(800);
        let asked = status()?;
        control.call(
            "debugShow",
            json!({ "sessionKey": KEY, "surface": surface, "displayId": id }),
        )?;
        let reply = call.wait();
        println!("asked: {asked}");
        Ok(reply)
    };

    // (1) Nothing focused: accessibility, a shortcut, no request.
    let before = lines(log).len();
    let reply = press(None)?;
    let got = new_lines(log, before, 300);
    let queued = status()?;
    check(
        "with no field focused, the press goes through accessibility as Electron's shortcut and says so",
        got == ["previous accelerator=true"]
            && reply.as_ref().is_ok_and(|r| {
                r["route"].is_null()
                    && r["note"]
                        .as_str()
                        .is_some_and(|n| n.contains("triggeredByAccelerator"))
            }),
        format!("{got:?} {reply:?}"),
    );
    check(
        "a press in the background asks for nothing",
        queued["requests"].as_array().is_some_and(|r| r.is_empty())
            && queued["grantActive"] == false,
        queued.to_string(),
    );

    // Focus the field and type in it, as a person filling a form.
    let field = number(&state()?, "textfield")?;
    control.call(
        "elementAction",
        at(json!({ "element": field, "action": "focus" })),
    )?;
    control.call(
        "type",
        json!({ "sessionKey": KEY, "displayId": id, "text": "abc" }),
    )?;
    sleep_ms(400);
    let page = title()?;
    let front = probe(&["frontpid"])?;
    let pointer = cursor();

    // (2) Focused: the press asks for the pointer; allowed, it runs.
    let before = lines(log).len();
    let reply = answered("input-queue-allow")?;
    check(
        "with the field focused, the press waits for the user and, allowed, chooses the item with a real click",
        reply.as_ref().is_ok_and(|r| r["route"] == "real_input"),
        format!("{reply:?}"),
    );
    // (3) Once, as a click.
    let got = new_lines(log, before, 1500);
    check(
        "the item runs once, as a click: Electron says it wasn't a shortcut",
        got == ["previous accelerator=false"],
        format!("{got:?}"),
    );
    // (5) The approval ends by itself, and puts everything back.
    let mut queue = status()?;
    for _ in 0..30 {
        if queue["grantActive"] == false {
            break;
        }
        sleep_ms(100);
        queue = status()?;
    }
    sleep_ms(500);
    let open = probe(&["openmenus", &app])?;
    let front_after = probe(&["frontpid"])?;
    let back = cursor();
    check(
        "afterwards no menu is open, the approval has ended, and your front app and pointer are back",
        open.trim().is_empty()
            && queue["grantActive"] == false
            && front_after == front
            && (back.x - pointer.x).abs() < 1.5
            && (back.y - pointer.y).abs() < 1.5,
        format!(
            "open menus {open:?}, {queue}, front {front:?} -> {front_after:?}, pointer {pointer:?} -> {back:?}"
        ),
    );
    // (4) The field keeps its text and focus.
    let tree = state()?;
    check(
        "the field kept what was typed, and its focus",
        tree.contains(r#"textfield value="abc" focused"#),
        tree.lines()
            .find(|l| l.contains("textfield"))
            .unwrap_or_default(),
    );
    // (6) No click or key reached the page.
    let now = title()?;
    check(
        "the page saw no click and no key from the menu choice",
        ["down", "up", "click", "keys", "typed"]
            .iter()
            .all(|k| seen(&now, k) == seen(&page, k)),
        format!("{page:?} -> {now:?}"),
    );

    // (7) Declined: nothing runs, not even the shortcut.
    let before = lines(log).len();
    let reply = answered("input-queue-decline")?;
    let got = new_lines(log, before, 1500);
    check(
        "declined, the press reports input_pending and chooses nothing",
        got.is_empty()
            && reply
                .as_ref()
                .is_err_and(|e| e.to_string().contains("input_pending")),
        format!("{got:?} {reply:?}"),
    );

    // (8) real_input false: accessibility, as a shortcut, with the note.
    let before = lines(log).len();
    let reply = press(Some(false))?;
    let got = new_lines(log, before, 300);
    check(
        "with real_input false, the press goes through accessibility as the shortcut and says so",
        got == ["previous accelerator=true"]
            && reply.as_ref().is_ok_and(|r| {
                r["route"].is_null() && r["note"].as_str().is_some_and(|n| n.contains("real_input"))
            }),
        format!("{got:?} {reply:?}"),
    );
    Ok(())
}
