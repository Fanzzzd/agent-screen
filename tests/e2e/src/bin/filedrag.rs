//! A file dragged from one app's window to another's on the same Agent
//! Display (issue #166), as from Finder to a web page. The fixtures,
//! FileSource and FileDrop from tools/filedrag.swift, run in two processes:
//! a file icon that starts a system drag session, and a view that takes a
//! dropped file URL. A system drag session follows the real pointer, so a
//! drag sent in the background can't carry the file across: the drag asks
//! for the user's pointer instead, and declined, sends nothing. Allowed, as
//! the card's Allow does, it runs with your pointer from the icon to the
//! drop view, which gets the file, and reports route real_input. A drop
//! window that moves while the user decides gets nothing: the allowed drag
//! presses nothing.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e filedrag
//!
//! Makes 1 display. Moves your pointer for a second: keep your hands off
//! the mouse and keyboard while the sign says so.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, file_drag_apps, hid, instance, json, locked,
    probe, run, skip, sleep_ms, start_test_app,
};
use anyhow::{Context, Result};

const KEY: &str = "filedrag";
const FILE: &str = "drop-me.txt";

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
            json!({ "sessionKey": KEY, "title": "File drag", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let mut pids = Vec::new();
        let pointer = cursor();
        let front = probe(&["frontpid"])?;
        let result = test(&control, &id, &mut pids);
        for pid in pids {
            let _ = std::process::Command::new("/bin/kill")
                .arg(pid.to_string())
                .status();
        }
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        let _ = hid(&["warp", &pointer.x.to_string(), &pointer.y.to_string()]);
        if probe(&["frontpid"])? != front {
            probe(&["activate", front.trim()])?;
        }
        result
    })
}

/// A count or name the fixture writes into its title, as `drops=1`.
fn seen(title: &str, key: &str) -> String {
    title
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

fn count(title: &str, key: &str) -> i64 {
    seen(title, key).parse().unwrap_or(0)
}

fn test(control: &Control, id: &Value, pids: &mut Vec<i64>) -> Result<()> {
    let dir = std::env::temp_dir().join("agent-screen-filedrag");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(FILE);
    std::fs::write(&file, "dropped by filedrag\n")?;
    let (source_app, drop_app) = file_drag_apps()?;
    // A system drag session shows a transparent window of the source app
    // over every screen at the dragging level, 500, while it runs. One run
    // measured 6046 ms for it, so allow 10 s.
    allow_on_your_screen(
        "FileSource",
        10_000,
        "the system drag session's transparent window",
    );
    let mut launch = |app: &std::path::Path, role: &str, x: i64| -> Result<(i64, Value)> {
        let launched = control.call(
            "launchApp",
            json!({
                "sessionKey": KEY,
                "displayId": id,
                "app": app.to_str(),
                "environment": {
                    "FILEDRAG_ROLE": role,
                    "FILEDRAG_DISPLAY": id.to_string(),
                    "FILEDRAG_X": x.to_string(),
                    "FILEDRAG_FILE": file.to_str(),
                },
            }),
        )?;
        let pid = launched["pid"].as_i64().context("no pid")?;
        pids.push(pid);
        Ok((pid, launched["window"]["windowId"].clone()))
    };
    let (_, source) = launch(&source_app, "source", 120)?;
    let (drop_pid, drop) = launch(&drop_app, "target", 700)?;
    let frame = |window: &Value| -> Result<Value> {
        let list = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
        list["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"] == *window))
            .cloned()
            .context("a fixture's window left the display")
    };
    let title = |window: &Value| -> Result<String> {
        Ok(frame(window)?["title"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    // The middle of a window's view, below its 28 pt title bar.
    let middle = |window: &Value| -> Result<(i64, i64)> {
        let w = frame(window)?;
        Ok((
            w["x"].as_i64().context("x")? + w["width"].as_i64().context("width")? / 2,
            w["y"].as_i64().context("y")? + 28 + (w["height"].as_i64().context("height")? - 28) / 2,
        ))
    };
    // Each window takes its own place 2 s after it shows.
    sleep_ms(2500);
    let (from, to) = (middle(&source)?, middle(&drop)?);
    anyhow::ensure!(
        (to.0 - from.0).abs() > 400,
        "the fixtures' windows overlap: {from:?} {to:?}"
    );
    let status = || {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "status" }),
        )
    };
    // A drag the user answers from the card, as Allow or Not Now does,
    // while the call waits; `meanwhile` runs before the answer. Each drag
    // starts `dy` lower, as a retry of the same drag finds the answer the
    // last one got.
    let answered = |dy: i64, real: bool, surface: &str, meanwhile: &dyn Fn() -> Result<()>| {
        let mut params = json!({ "sessionKey": KEY, "displayId": id,
                "fromX": from.0, "fromY": from.1 + dy, "toX": to.0, "toY": to.1 + dy });
        if real {
            params["realInput"] = json!(true);
        }
        let call = control.begin("drag", params)?;
        sleep_ms(800);
        let asked = status()?;
        meanwhile()?;
        if asked["waiting"].as_i64() > Some(0) {
            control.call(
                "debugShow",
                json!({ "sessionKey": KEY, "surface": surface, "displayId": id }),
            )?;
        }
        anyhow::Ok((asked, call.wait()))
    };

    // The background route: the source gets the press, but a system drag
    // session follows the real pointer, so the file can't arrive. The drag
    // asks for the user's pointer instead of saying it was posted.
    let (asked, dragged) = answered(0, false, "input-queue-decline", &|| Ok(()))?;
    sleep_ms(500);
    let (src, dst) = (title(&source)?, title(&drop)?);
    println!("     background: {src:?} {dst:?} {dragged:?}");
    check(
        "a drag between two apps' windows asks for the user's pointer, and declined sends nothing",
        asked["waiting"] == 1
            && dragged
                .as_ref()
                .is_err_and(|e| e.to_string().contains("No drag or press was sent"))
            && count(&src, "down") == 0
            && count(&dst, "drops") == 0,
        format!("{src:?} {dst:?} asked {asked} -> {dragged:?}"),
    );

    // Allowed, the drag runs with the user's pointer from the icon in one
    // window to the view in the other, which gets the file.
    let (_, dragged) = answered(10, true, "input-queue-allow", &|| Ok(()))?;
    sleep_ms(500);
    let (src, dst) = (title(&source)?, title(&drop)?);
    check(
        "allowed, the drag runs with the user's pointer and the other app's window gets the file",
        dragged.as_ref().is_ok_and(|r| r["route"] == "real_input")
            && count(&src, "began") >= 1
            && seen(&src, "op") == "1"
            && count(&dst, "drops") == 1
            && seen(&dst, "got") == FILE
            && !unsafe { CGEventSourceButtonState(0, 0) },
        format!("{src:?} {dst:?} {dragged:?}"),
    );
    // A web page needs a few dragover events before the drop.
    check(
        "the drop window heard the drag over it a few times before the drop",
        count(&dst, "over") >= 3,
        format!("{dst:?}"),
    );

    // The session is done, so the approval ends and the next drag asks
    // again instead of running at once.
    control.call(
        "inputQueue",
        json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
    )?;
    sleep_ms(1500);

    // The drop window moves while the user decides: allowed, the drag
    // presses nothing, as its end point no longer lands where the agent
    // aimed.
    let downs = count(&title(&source)?, "down");
    let before = frame(&drop)?["x"].as_i64();
    let (_, dragged) = answered(20, true, "input-queue-allow", &|| {
        std::process::Command::new("/bin/kill")
            .args(["-USR1", &drop_pid.to_string()])
            .status()?;
        // A background app may take a moment to act on the signal.
        for _ in 0..30 {
            if frame(&drop)?["x"].as_i64() != before {
                return Ok(());
            }
            sleep_ms(100);
        }
        anyhow::bail!("the drop window didn't move")
    })?;
    sleep_ms(500);
    let (src, dst) = (title(&source)?, title(&drop)?);
    check(
        "a drop window that moved while the user decided gets nothing, and nothing is pressed",
        dragged
            .as_ref()
            .is_err_and(|e| e.to_string().contains("drop window"))
            && count(&src, "down") == downs
            && count(&dst, "drops") == 1
            && !unsafe { CGEventSourceButtonState(0, 0) },
        format!("{src:?} {dst:?} {dragged:?}"),
    );
    Ok(())
}
