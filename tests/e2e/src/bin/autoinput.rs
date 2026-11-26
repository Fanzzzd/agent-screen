//! Auto opens the Input Grant itself while the user's hands rest (ADR
//! 0025), with `debugActivity` playing their idle time and the real
//! pointer doing the click:
//!
//! - At rest, a `real_input` click on the HIDClick fixture runs with no
//!   card: the Red Glow goes armed, live, off; `openedBy` is `mode`; the
//!   pointer and the stand-in front app (keyholder) are back after.
//! - With the hands busy the request waits, `waitingOn` `userActive`.
//! - A key the user presses during a mode's loan, posted untagged, ends the
//!   loan (a long drag); the armed hold delivers it to the stand-in's field, `interrupted`
//!   is set, and the next grant waits.
//! - A Protected App's target opens the card in Auto.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e autoinput
//!
//! Makes 1 display. Moves your pointer and puts a small window in front on
//! your screen for about a minute: keep your hands off the Mac. Writes only
//! the e2e instance's `autonomy-e2e.json`, and puts it back.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_screen_tests::{
    Control, Holder, Value, allow_on_your_screen, at_rest, check, hid, hidclick_app, instance,
    json, locked, probe, put_back_autonomy, run, set_autonomy_file, skip, sleep_ms, start_test_app,
    stop_test_app, tool_app_as, wait_glow, watch_glow,
};
use anyhow::{Context, Result, ensure};

const KEY: &str = "autoinput";
const GUARD_BUNDLE: &str = "app.agentscreen.e2e.autoguardclick";

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if locked() {
        skip("the screen is locked, and this test moves the real pointer");
    }
    run(|| {
        allow_on_your_screen(
            "keyholder",
            120_000,
            "the stand-in for your front app, on your screen on purpose",
        );
        stop_test_app();
        let before = set_autonomy_file(&json!({ "mode": "ask", "protected": [GUARD_BUNDLE] }))?;
        let pointer = hid(&["where"])?;
        let front = probe(&["frontpid"])?;
        let mut pids = Vec::new();
        let result = start_test_app(&[("AGENTSCREEN_LANG", "en")]).and_then(|()| {
            let control = Control::connect()?;
            let made = control.call(
                "createDisplay",
                json!({ "sessionKey": KEY, "title": "Auto input", "width": 1280, "height": 800 }),
            )?;
            let id = made["displayId"].clone();
            let result = test(&control, &id, &mut pids);
            let _ = control.call("debugActivity", json!({ "clear": true }));
            let _ = control.call("debugShow", json!({ "surface": "autonomy-ask" }));
            let _ = control.call(
                "inputQueue",
                json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
            );
            for pid in &pids {
                let _ = std::process::Command::new("/bin/kill")
                    .arg(pid.to_string())
                    .status();
            }
            let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
            result
        });
        let at: Vec<&str> = pointer.split_whitespace().collect();
        if let [x, y] = at[..] {
            let _ = hid(&["warp", x, y]);
        }
        if probe(&["frontpid"]).unwrap_or_default() != front {
            let _ = probe(&["activate", front.trim()]);
        }
        stop_test_app();
        put_back_autonomy(before);
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

/// Launch a HIDClick app on the display: its window and the middle of its
/// button.
fn launch(
    control: &Control,
    id: &Value,
    app: &std::path::Path,
    pids: &mut Vec<i64>,
) -> Result<(Value, (i64, i64))> {
    let launched = control.call(
        "launchApp",
        json!({
            "sessionKey": KEY, "displayId": id, "app": app.to_str(),
            "environment": { "HIDCLICK_DISPLAY": id.to_string() },
        }),
    )?;
    pids.push(launched["pid"].as_i64().context("its pid")?);
    let window = launched["window"]["windowId"].clone();
    let w = frame(control, id, &window)?;
    let (wx, wy, ww, wh) = (
        w["x"].as_i64().context("x")?,
        w["y"].as_i64().context("y")?,
        w["width"].as_i64().context("width")?,
        w["height"].as_i64().context("height")?,
    );
    // The button fills the window below its title bar.
    Ok((window, (wx + ww / 2, wy + 28 + (wh - 28) / 2)))
}

fn frame(control: &Control, id: &Value, window: &Value) -> Result<Value> {
    let list = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
    list["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["windowId"] == *window))
        .cloned()
        .context("the fixture's window left the display")
}

fn where_now() -> (f64, f64) {
    let at = hid(&["where"]).unwrap_or_default();
    let mut parts = at.split_whitespace().filter_map(|p| p.parse().ok());
    (parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0))
}

fn test(control: &Control, id: &Value, pids: &mut Vec<i64>) -> Result<()> {
    let status = || {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "status" }),
        )
    };
    let activity = |v: Value| control.call("debugActivity", v);
    let withdraw = |request: &Value| {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "withdraw", "requestId": request }),
        )
    };
    let (window, middle) = launch(control, id, &hidclick_app()?, pids)?;
    let title = || -> Result<String> {
        Ok(frame(control, id, &window)?["title"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };

    agent_screen_tests::countdown();
    let holder = Holder::start(120)?;
    sleep_ms(1500);
    holder.activate();
    sleep_ms(500);
    ensure!(
        probe(&["frontpid"])?.trim() == holder.pid().to_string(),
        "setup: the stand-in isn't in front"
    );
    let pointer = where_now();

    // At rest: the mode opens the grant, with no card.
    activity(json!({ "idleMs": 9000 }))?;
    control.call("debugShow", json!({ "surface": "autonomy-auto" }))?;
    sleep_ms(800);
    ensure!(at_rest(control), "setup: the hands don't read At Rest");
    let stop = AtomicBool::new(false);
    let (reply, (glow, opened)) = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| watch_glow(&stop));
        let reply = control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": middle.0, "y": middle.1,
                    "button": "left", "count": 1, "realInput": true }),
        );
        sleep_ms(800);
        stop.store(true, Ordering::Relaxed);
        (reply, watcher.join().unwrap_or_default())
    });
    let now = title()?;
    check(
        "at rest, a real_input click runs with no card and the user's pointer",
        reply.as_ref().is_ok_and(|r| r["route"] == "real_input") && seen(&now, "real") == 1,
        format!("{now:?} {reply:?}"),
    );
    let armed = glow.iter().position(|s| s == "armed");
    let live = glow.iter().position(|s| s == "live");
    check(
        "the Red Glow goes armed, live, then off",
        armed.is_some()
            && live.is_some()
            && armed < live
            && glow.last().is_some_and(|s| s == "off"),
        format!("{glow:?}"),
    );
    check(
        "the grant says the mode opened it",
        opened.iter().any(|o| o == "auto"),
        format!("{opened:?}"),
    );
    let queue = status()?;
    let back = where_now();
    let front = probe(&["frontpid"])?;
    check(
        "the grant ended by itself, and the pointer and the stand-in front app are back",
        queue["grantActive"] == false
            && front.trim() == holder.pid().to_string()
            && (back.0 - pointer.0).abs() < 1.5
            && (back.1 - pointer.1).abs() < 1.5,
        format!("{queue} front {front:?} pointer {pointer:?} -> {back:?}"),
    );

    // The hands busy: the request waits for them.
    activity(json!({ "idleMs": 1000 }))?;
    let call = control.begin(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": middle.0, "y": middle.1,
                "button": "left", "count": 1, "realInput": true }),
    )?;
    sleep_ms(1200);
    let queue = status()?;
    let request = &queue["requests"][0];
    check(
        "with the hands busy the request waits for them, with no grant",
        request["waitingOn"] == "userActive" && queue["grantActive"] == false,
        queue.to_string(),
    );
    withdraw(&request["requestId"])?;
    let reply = call.wait();
    check(
        "withdrawn, it sent nothing",
        reply.is_err() && seen(&title()?, "real") == 1,
        format!("{reply:?}"),
    );

    // The user's key during a mode's loan: a long drag, and a key as it
    // goes out.
    activity(json!({ "idleMs": 9000 }))?;
    sleep_ms(400);
    let typed_before = holder.text();
    let (dragged, live) = std::thread::scope(|scope| {
        let drag = scope.spawn(|| {
            control.call(
                "drag",
                json!({ "sessionKey": KEY, "displayId": id, "realInput": true,
                        "fromX": middle.0 - 150, "fromY": middle.1,
                        "toX": middle.0 + 150, "toY": middle.1 }),
            )
        });
        let live = wait_glow(control, "live", 8000);
        let _ = hid(&["key", "0"]);
        (drag.join().expect("drag thread"), live)
    });
    sleep_ms(1500);
    let queue = status()?;
    let state = control.call("debugAutonomy", json!({}))?;
    check(
        "the drag ran in a loan the mode opened, and the user's key stopped it",
        live && dragged.is_err(),
        format!("live {live} {dragged:?}"),
    );
    check(
        "the key ends the grant and marks the interruption",
        queue["grantActive"] == false && state["tracker"]["hands"]["interrupted"] == true,
        format!("{queue} {}", state["tracker"]["hands"]),
    );
    check(
        "the armed hold delivers the key to the stand-in's field, which is in front again",
        holder.text() == format!("{typed_before}a")
            && probe(&["frontpid"])?.trim() == holder.pid().to_string(),
        format!("{:?} -> {:?}", typed_before, holder.text()),
    );
    let call = control.begin(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": middle.0, "y": middle.1,
                "button": "left", "count": 1, "realInput": true }),
    )?;
    sleep_ms(1200);
    let queue = status()?;
    let waiting = queue["requests"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["state"] == "queued"))
        .cloned()
        .unwrap_or_default();
    check(
        "after an interruption the next grant waits for a longer rest",
        waiting["waitingOn"] == "userActive" && queue["grantActive"] == false,
        queue.to_string(),
    );
    if !waiting["requestId"].is_null() {
        withdraw(&waiting["requestId"])?;
    }
    let _ = call.wait();

    // A Protected App's target asks, in Auto too.
    let guard = tool_app_as("hidclick", "AutoGuardClick", "autoguardclick")?;
    let (_, at) = launch(control, id, &guard, pids)?;
    let call = control.begin(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": at.0, "y": at.1,
                "button": "left", "count": 1, "realInput": true }),
    )?;
    sleep_ms(1200);
    let queue = status()?;
    let icon = control.call("debugIcon", json!({}))?;
    let asking = queue["requests"]
        .as_array()
        .is_some_and(|r| r.iter().any(|r| r["waitingOn"] == "approval"));
    check(
        "a Protected App's target opens the card in Auto",
        asking && queue["grantActive"] == false && icon["askingCount"].as_u64() >= Some(1),
        format!("{queue} {icon}"),
    );
    control.call(
        "debugShow",
        json!({ "sessionKey": KEY, "surface": "input-queue-decline", "displayId": id }),
    )?;
    let reply = call.wait();
    check(
        "declined, it answers input_pending",
        reply
            .as_ref()
            .is_err_and(|e| e.to_string().contains("input_pending")),
        format!("{reply:?}"),
    );
    drop(holder);
    Ok(())
}
