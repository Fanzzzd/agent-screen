//! A click with your real pointer, after a background click had no effect
//! (ADR 0013, "Clicks"; issue #134). The fixture, tools/hidclick.swift, is
//! one big button that takes a click only while the HID system reads the
//! mouse button down, as some apps check. A click sent in the background
//! reaches it but stays red. The same click with realInput asks for your
//! pointer on the approval card, which names the click and the app;
//! allowed, as the card's Allow Queue does, the window comes forward, your
//! pointer clicks the button, it turns green, and the call reports route
//! real_input. The approval then ends at once and your pointer and front
//! app come back. A double click allowed the same way arrives as one
//! double click. A right click with realInput is refused at once, as the
//! menu it opened would close when the user's app came back. A click
//! declined reports input_pending and sends nothing, not even the
//! background click. A click whose target changes while the user decides,
//! the fixture turning white, clicks nothing once allowed and tells the
//! agent to take a new screenshot.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e realclick
//!
//! Makes 1 display. Moves your pointer for a second, twice: keep your hands
//! off the mouse and keyboard while the sign says so.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, check, hidclick_app, instance, json, locked, probe, run, skip, sleep_ms,
    start_test_app,
};
use anyhow::{Context, Result};

const KEY: &str = "realclick";

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
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
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
    if locked() {
        skip("the screen is locked, and this test moves the real pointer");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Real click", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let mut pid = None;
        let result = test(&control, &id, &mut pid);
        if let Some(pid) = pid {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.to_string())
                .status();
        }
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        result
    })
}

/// A count the fixture writes into its title, as `real=1`.
fn seen(title: &str, key: &str) -> i64 {
    title
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn test(control: &Control, id: &Value, pid: &mut Option<i64>) -> Result<()> {
    let app = hidclick_app()?;
    let launched = control.call(
        "launchApp",
        json!({
            "sessionKey": KEY,
            "displayId": id,
            "app": app.to_str(),
            "environment": { "HIDCLICK_DISPLAY": id.to_string() },
        }),
    )?;
    *pid = launched["pid"].as_i64();
    let window = launched["window"]["windowId"].clone();
    let frame = || -> Result<Value> {
        let list = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
        list["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"] == window))
            .cloned()
            .context("the fixture's window left the display")
    };
    let title =
        || -> Result<String> { Ok(frame()?["title"].as_str().unwrap_or_default().to_string()) };
    let status = || {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "status" }),
        )
    };
    let w = frame()?;
    let (wx, wy, ww, wh) = (
        w["x"].as_i64().context("x")?,
        w["y"].as_i64().context("y")?,
        w["width"].as_i64().context("width")?,
        w["height"].as_i64().context("height")?,
    );
    // The button fills the window below its title bar.
    let middle = (wx + ww / 2, wy + 28 + (wh - 28) / 2);

    // A click the user answers from the card while the call waits.
    let answered = |surface: &str, at: (i64, i64), button: &str, count: i64| {
        let call = control.begin(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": at.0, "y": at.1,
                    "button": button, "count": count, "realInput": true }),
        )?;
        sleep_ms(800);
        let asked = status()?;
        control.call(
            "debugShow",
            json!({ "sessionKey": KEY, "surface": surface, "displayId": id }),
        )?;
        anyhow::Ok((asked, call.wait()))
    };

    // Red: the background click reaches the button, but the HID system
    // doesn't read its button down.
    let posted = control.call(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": middle.0, "y": middle.1 }),
    );
    sleep_ms(500);
    let after = title()?;
    check(
        "a click sent in the background reaches the button, which stays red: the HID button read up",
        posted.as_ref().is_ok_and(|r| r["route"].is_null())
            && seen(&after, "down") >= 1
            && seen(&after, "real") == 0,
        format!("{after:?} {posted:?}"),
    );

    // Green: the same click with the user's pointer, once allowed.
    let front = probe(&["frontpid"])?;
    let pointer = cursor();
    let (asked, reply) = answered("input-queue-allow", middle, "left", 1)?;
    let reason = asked["requests"][0]["reason"].as_str().unwrap_or_default();
    check(
        "the click asks the user first, and the request names the click and the app",
        asked["requests"][0]["state"] == "queued"
            && reason.starts_with("Click at (")
            && reason.contains("HIDClick"),
        asked.to_string(),
    );
    sleep_ms(300);
    let now = title()?;
    check(
        "allowed, the click runs with the user's pointer: the button turns green and the call says real_input",
        reply
            .as_ref()
            .is_ok_and(|r| r["route"] == "real_input" && r["waitedSeconds"].as_f64().is_some())
            && seen(&now, "real") == 1
            && seen(&now, "clicks") == 1
            && seen(&now, "down") == seen(&after, "down") + 1,
        format!("{now:?} {reply:?}"),
    );
    let put_back = |what: &str| -> Result<()> {
        let mut queue = status()?;
        for _ in 0..30 {
            if queue["grantActive"] == false {
                break;
            }
            sleep_ms(100);
            queue = status()?;
        }
        sleep_ms(500);
        let front_after = probe(&["frontpid"])?;
        let back = cursor();
        check(
            &format!(
                "after {what}, the approval has ended by itself, no button is held, and your front app and pointer are back"
            ),
            queue["grantActive"] == false
                && !(0..3).any(|b| unsafe { CGEventSourceButtonState(1, b) })
                && front_after == front
                && (back.x - pointer.x).abs() < 1.5
                && (back.y - pointer.y).abs() < 1.5,
            format!("{queue}, front {front:?} -> {front_after:?}, pointer {pointer:?} -> {back:?}"),
        );
        Ok(())
    };
    put_back("the click")?;

    // A double click, allowed: one double click, two real presses.
    let before = title()?;
    let (asked, reply) = answered("input-queue-allow", (middle.0 + 20, middle.1), "left", 2)?;
    sleep_ms(300);
    let now = title()?;
    check(
        "an allowed double click arrives as one double click with the real pointer",
        asked["requests"][0]["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("Double-click at ("))
            && reply.as_ref().is_ok_and(|r| r["route"] == "real_input")
            && seen(&now, "real") == seen(&before, "real") + 2
            && seen(&now, "clicks") == 2,
        format!("{before:?} -> {now:?} {reply:?}"),
    );
    put_back("the double click")?;

    // A right click with the user's pointer is refused at once: the menu it
    // opened would close when the user's front app came back.
    let before = title()?;
    let right = control.call(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": middle.0 - 20, "y": middle.1,
                "button": "right", "realInput": true }),
    );
    let queue = status()?;
    sleep_ms(300);
    let now = title()?;
    check(
        "a right click with realInput is refused at once, says why and what to use, and sends and queues nothing",
        right.as_ref().is_err_and(|e| {
            let e = e.to_string();
            e.contains("left clicks without Control only") && e.contains("show_menu")
        }) && seen(&now, "down") == seen(&before, "down")
            && queue["requests"]
                .as_array()
                .is_none_or(|r| r.iter().all(|r| r["state"] != "queued")),
        format!("{before:?} -> {now:?} {right:?} {queue}"),
    );

    // So is a Control-held left click, which opens the same menu.
    let control_click = control.call(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": middle.0 - 20, "y": middle.1,
                "modifiers": ["ctrl"], "realInput": true }),
    );
    let now = title()?;
    check(
        "a Control-held left click with realInput is refused the same way and sends nothing",
        control_click
            .as_ref()
            .is_err_and(|e| e.to_string().contains("show_menu"))
            && seen(&now, "down") == seen(&before, "down"),
        format!("{before:?} -> {now:?} {control_click:?}"),
    );

    // A click, declined: nothing is sent at all.
    let before = title()?;
    let (_, reply) = answered("input-queue-decline", (middle.0 - 20, middle.1), "left", 1)?;
    sleep_ms(500);
    let now = title()?;
    check(
        "declined, the click reports input_pending and nothing reaches the button, not even a background click",
        reply
            .as_ref()
            .is_err_and(|e| e.to_string().contains("input_pending"))
            && seen(&now, "down") == seen(&before, "down"),
        format!("{before:?} -> {now:?} {reply:?}"),
    );

    // A click whose target changed while the user decided: the fixture
    // turns white, as a list that reordered under the point. Allowed, the
    // loan clicks nothing and tells the agent to look again.
    let before = title()?;
    let at = (middle.0 - 40, middle.1);
    let call = control.begin(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": at.0, "y": at.1,
                "button": "left", "realInput": true }),
    )?;
    sleep_ms(800);
    let fixture = pid.context("the fixture's pid")?;
    std::process::Command::new("/bin/kill")
        .args(["-USR1", &fixture.to_string()])
        .status()?;
    sleep_ms(500);
    control.call(
        "debugShow",
        json!({ "sessionKey": KEY, "surface": "input-queue-allow", "displayId": id }),
    )?;
    let reply = call.wait();
    sleep_ms(300);
    let now = title()?;
    check(
        "allowed after what was under the point changed, nothing is clicked and the agent is told to take a new screenshot",
        reply
            .as_ref()
            .is_err_and(|e| e.to_string().contains("Take a new screenshot"))
            && seen(&now, "down") == seen(&before, "down"),
        format!("{before:?} -> {now:?} {reply:?}"),
    );
    put_back("the click that changed")?;
    Ok(())
}
