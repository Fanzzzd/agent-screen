//! The Input Queue's executor (ADR 0013) moves your real pointer. The
//! Electron harness, started with `open` and so without a DevTools pipe,
//! has a splitter that follows pointermove only while the session's left
//! button reads held, which no background route can make it do. A drag
//! there waits for approval; Allow All, as the card's button, lets it run
//! with the pointer: the splitter follows it, a red border shows on your
//! screens meanwhile, and the drag reports route real_input. A move of the
//! pointer the executor didn't make takes the approval back at once; the
//! cursor guard then keeps the pointer off the Agent Display. A second approved drag runs, and when
//! the session declares itself done, the pointer goes back to where it was
//! when the approval began.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e realdrag
//!
//! Makes 1 display. Moves your pointer for a few seconds: keep your hands
//! off the mouse and keyboard while the sign says so.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, check, debug_pid, fixture, instance, json, out, run, sh_lenient, skip,
    sleep_ms, start_test_app, tool, windows_of,
};
use anyhow::{Context, Result};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> Point;
    fn CGEventCreateMouseEvent(
        source: *const std::ffi::c_void,
        kind: u32,
        at: Point,
        button: u32,
    ) -> *mut std::ffi::c_void;
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
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

/// An untagged pointer move, as the user's own mouse makes.
fn nudge() {
    let at = cursor();
    unsafe {
        let event = CGEventCreateMouseEvent(
            std::ptr::null(),
            5, // kCGEventMouseMoved
            Point {
                x: at.x + 3.0,
                y: at.y,
            },
            0,
        );
        CGEventPost(0, event); // kCGHIDEventTap
        CFRelease(event);
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
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let pid = debug_pid().context("no test app")?;
        let control = Control::connect()?;
        let key = "realdrag";
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": key, "title": "Real drag", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let result = test(&control, key, &id, pid, &electron);
        sh_lenient(
            "/usr/bin/pkill",
            &["-f", fixture("electron").to_str().unwrap_or_default()],
        );
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": key, "displayId": id, "action": "done" }),
        );
        let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
        result
    })
}

fn harness(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["kind"] == "window")
            .cloned()
    }))
}

fn seen(window: &Value, key: &str) -> i64 {
    window["title"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn status(control: &Control, key: &str, id: &Value) -> Result<Value> {
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "status" }),
    )
}

/// The indicator: the app's windows above everything, at level 1000.
fn borders(pid: u32) -> usize {
    windows_of(pid).iter().filter(|w| w.layer == 1000).count()
}

/// Drag the splitter `by` points while the test allows it, as the user's
/// Allow All does. Returns the drag's reply.
fn approved_drag(
    control: &Control,
    key: &str,
    id: &Value,
    (sx, sy): (i64, i64),
    by: i64,
    pid: u32,
) -> Result<(Result<Value>, usize)> {
    let call = control.begin(
        "drag",
        json!({ "sessionKey": key, "displayId": id, "realInput": true,
                "fromX": sx, "fromY": sy, "toX": sx + by, "toY": sy }),
    )?;
    sleep_ms(800);
    control.call(
        "debugShow",
        json!({ "sessionKey": key, "surface": "input-queue-allow", "displayId": id }),
    )?;
    // The first loan waits for a second of quiet, then runs.
    let mut shown = 0;
    for _ in 0..20 {
        shown = shown.max(borders(pid));
        sleep_ms(100);
    }
    Ok((call.wait(), shown))
}

fn test(
    control: &Control,
    key: &str,
    id: &Value,
    pid: u32,
    electron: &std::path::Path,
) -> Result<()> {
    sh_lenient(
        "/usr/bin/open",
        &[
            "-n",
            "-g",
            "--env",
            &format!("HARNESS_DISPLAY={id}"),
            "--env",
            "HARNESS_BACKGROUND=1",
            "-a",
            electron.to_str().unwrap_or_default(),
            "--args",
            fixture("electron").to_str().unwrap_or_default(),
        ],
    );
    let mut window = None;
    for _ in 0..50 {
        window =
            harness(control, id)?.filter(|w| w["title"].as_str().is_some_and(|t| t.contains("w=")));
        if window.is_some() {
            break;
        }
        sleep_ms(200);
    }
    let window = window.context("the harness window didn't show")?;
    // Handed over, as the user hands their own browser window to an
    // agent: the agent's from then on, so AeroSpace leaves it alone.
    control.call(
        "debugHandOver",
        json!({ "displayId": id, "windowId": window["windowId"] }),
    )?;
    sleep_ms(1000);
    let window = harness(control, id)?.context("the harness left the display")?;
    let (wx, wy) = (
        window["x"].as_i64().context("x")?,
        window["y"].as_i64().context("y")?,
    );
    // The splitter's middle: 346 pt in, 280 down, below a 28 pt title bar.
    let splitter = (wx + 346, wy + 28 + 280);

    let (dragged, shown) = approved_drag(control, key, id, splitter, 80, pid)?;
    sleep_ms(400);
    let now = harness(control, id)?.context("window")?;
    check(
        "an approved drag runs with the user's pointer: the splitter sees the button held and follows",
        dragged.as_ref().is_ok_and(|r| r["route"] == "real_input")
            && seen(&now, "held") > 0
            && (70..=90).contains(&seen(&now, "split")),
        format!("{} {dragged:?}", now["title"].as_str().unwrap_or_default()),
    );
    check(
        "a red border shows on the user's screens during the approval",
        shown > 0,
        format!("{shown} border window(s)"),
    );

    // The user's own move takes the approval back. Nothing puts the
    // pointer back for them, but it doesn't stay on the Agent Display,
    // where they couldn't see it.
    nudge();
    sleep_ms(600);
    let state = status(control, key, id)?;
    let after = cursor();
    let bounds: Vec<f64> = sh_lenient(tool("probe")?, &["bounds", &id.to_string()])
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let on_display = matches!(bounds[..], [x, y, w, h]
        if after.x >= x && after.x < x + w && after.y >= y && after.y < y + h);
    check(
        "a move the executor didn't make ends the approval at once, and the pointer comes off the Agent Display",
        state["grantActive"] == false && borders(pid) == 0 && !on_display,
        format!("{state} cursor {after:?} display {bounds:?}"),
    );

    // Another approval: when the session is done, the pointer goes back.
    let start = cursor();
    // The splitter stays put; the page counts how far the pointer went.
    let (dragged, _) = approved_drag(control, key, id, splitter, -40, pid)?;
    let moved = cursor();
    let now = harness(control, id)?.context("window")?;
    check(
        "a second approved drag runs too, and the pointer stays where the drag left it until the approval ends",
        (-50..=-30).contains(&seen(&now, "split"))
            && ((moved.x - start.x).abs() > 50.0 || (moved.y - start.y).abs() > 50.0),
        format!(
            "{} {start:?} -> {moved:?}",
            now["title"].as_str().unwrap_or_default()
        ),
    );
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    )?;
    sleep_ms(1500);
    let back = cursor();
    let state = status(control, key, id)?;
    check(
        "after the session is done, the approval ends and the pointer is back where it was",
        dragged.as_ref().is_ok_and(|r| r["route"] == "real_input")
            && state["grantActive"] == false
            && (back.x - start.x).abs() < 1.5
            && (back.y - start.y).abs() < 1.5
            && borders(pid) == 0,
        format!("{dragged:?} {start:?} -> {moved:?} -> {back:?} {state}"),
    );
    // Take control back during an actual held-button sequence, not just
    // during the grace period after it. Poll the owned page's down count
    // so the synthetic user move lands after the press reached the page.
    let before = seen(&harness(control, id)?.context("window")?, "down");
    let pending = control.begin(
        "drag",
        json!({
            "sessionKey": key, "displayId": id, "realInput": true,
            "fromX": splitter.0, "fromY": splitter.1,
            "toX": splitter.0 + 240, "toY": splitter.1,
        }),
    )?;
    sleep_ms(600);
    control.call(
        "debugShow",
        json!({
            "sessionKey": key, "displayId": id, "surface": "input-queue-allow",
        }),
    )?;
    let mut pressed = false;
    for _ in 0..150 {
        if harness(control, id)?.is_some_and(|w| seen(&w, "down") > before) {
            pressed = true;
            break;
        }
        sleep_ms(20);
    }
    nudge();
    let interrupted = pending.wait();
    sleep_ms(600);
    let state = status(control, key, id)?;
    check(
        "a user move during the drag stops it, releases its button and never reports completed",
        pressed
            && interrupted
                .as_ref()
                .is_err_and(|e| e.to_string().contains("stopped partway"))
            && state["grantActive"] == false
            && !unsafe { CGEventSourceButtonState(0, 0) }
            && state["requests"][0]["state"] == "stopped"
            && state["requests"][0]["stopReason"] == "User",
        format!("pressed {pressed}, {interrupted:?}, {state}"),
    );
    Ok(())
}
