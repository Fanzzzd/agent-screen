//! YOLO's real input (ADR 0025): the grant opens at once, and an armed hold
//! keeps the user's input from revoking it.
//!
//! - With `idleMs` 0, a `real_input` click runs at once, opened by the mode.
//! - The second hold starts 1 s after the first at the soonest.
//! - Keys posted during a loan (a long drag) arrive after it, in order, in
//!   the stand-in front app's field (keyholder); moves posted during it do
//!   not take the pointer (the gate of 7.1).
//! - The Stop chord, ⌃⌥⌘., posted during a loan ends it and gives Ask.
//! - With Secure Input on, a request waits.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e yoloinput
//!
//! Manual: it holds your keys for up to 3 s at a time and moves your
//! pointer. Run it when you are away. Makes 1 display. Writes only the e2e
//! instance's `autonomy-e2e.json`, and puts it back.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Holder, Value, allow_on_your_screen, check, hid, hidclick_app, instance, json, locked,
    probe, put_back_autonomy, run, set_autonomy_file, sh_lenient, skip, sleep_ms, start_test_app,
    stop_test_app, wait_glow,
};
use anyhow::{Context, Result, ensure};

const KEY: &str = "yoloinput";
/// kCGEventFlagMaskControl | Alternate | Command.
const CHORD_FLAGS: &str = "1835008";
const PERIOD: &str = "47";

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
        let before = set_autonomy_file(&json!({ "mode": "ask" }))?;
        let pointer = hid(&["where"])?;
        let front = probe(&["frontpid"])?;
        let mut pid = None;
        let result = start_test_app(&[("AGENTSCREEN_LANG", "en")]).and_then(|()| {
            let control = Control::connect()?;
            let made = control.call(
                "createDisplay",
                json!({ "sessionKey": KEY, "title": "YOLO input", "width": 1280, "height": 800 }),
            )?;
            let id = made["displayId"].clone();
            let result = test(&control, &id, &mut pid);
            let _ = control.call("debugActivity", json!({ "clear": true }));
            let _ = control.call("debugShow", json!({ "surface": "autonomy-stop" }));
            let _ = control.call("debugShow", json!({ "surface": "autonomy-ask" }));
            let _ = control.call(
                "inputQueue",
                json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
            );
            if let Some(pid) = pid {
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

fn where_now() -> (f64, f64) {
    let at = hid(&["where"]).unwrap_or_default();
    let mut parts = at.split_whitespace().filter_map(|p| p.parse().ok());
    (parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0))
}

fn yolo(control: &Control, holder: &Holder) -> Result<()> {
    control.call("debugShow", json!({ "surface": "autonomy-yolo" }))?;
    let on = control.call("debugShow", json!({ "surface": "autonomy-yolo-confirm" }))?;
    ensure!(on["mode"] == "yolo", "setup: YOLO didn't turn on: {on}");
    // The confirmation's glow preview ends first. Turning YOLO on brought
    // Agent Screen forward, as the user's click in its settings would: the
    // stand-in goes back in front.
    sleep_ms(2200);
    // NSRunningApplication's activate is refused while Agent Screen is in
    // front; System Events' frontmost isn't.
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!(
                "tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true",
                holder.pid()
            ),
        ],
    );
    sleep_ms(800);
    ensure!(
        probe(&["frontpid"])?.trim() == holder.pid().to_string(),
        "setup: the stand-in isn't in front after YOLO came on"
    );
    Ok(())
}

fn test(control: &Control, id: &Value, pid: &mut Option<i64>) -> Result<()> {
    let status = || {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "status" }),
        )
    };
    let wait_ended = || -> Result<Value> {
        let mut queue = status()?;
        for _ in 0..60 {
            if queue["grantActive"] == false {
                break;
            }
            sleep_ms(100);
            queue = status()?;
        }
        Ok(queue)
    };
    let launched = control.call(
        "launchApp",
        json!({
            "sessionKey": KEY, "displayId": id, "app": hidclick_app()?.to_str(),
            "environment": { "HIDCLICK_DISPLAY": id.to_string() },
        }),
    )?;
    *pid = launched["pid"].as_i64();
    let window = launched["window"]["windowId"].clone();
    let list = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
    let w = list["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["windowId"] == window))
        .cloned()
        .context("the fixture's window")?;
    let middle = (
        w["x"].as_i64().unwrap_or(0) + w["width"].as_i64().unwrap_or(0) / 2,
        w["y"].as_i64().unwrap_or(0) + 28 + (w["height"].as_i64().unwrap_or(0) - 28) / 2,
    );
    let click = |dx: i64| {
        control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": middle.0 + dx, "y": middle.1,
                    "button": "left", "count": 1, "realInput": true }),
        )
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
    yolo(control, &holder)?;
    control.call("debugActivity", json!({ "idleMs": 0 }))?;

    // At once, even with the hands busy; then a second hold after 1 s.
    let first = click(0);
    let second = click(10);
    wait_ended()?;
    check(
        "with the hands busy, YOLO's real_input click runs at once",
        first.as_ref().is_ok_and(|r| r["route"] == "real_input"),
        format!("{first:?}"),
    );
    let ended = first
        .as_ref()
        .ok()
        .and_then(|r| r["timing"]["endedAt"].as_u64());
    let started = second
        .as_ref()
        .ok()
        .and_then(|r| r["timing"]["startedAt"].as_u64());
    check(
        "the second hold starts 1 s after the first at the soonest",
        ended.zip(started).is_some_and(|(e, s)| s >= e + 950),
        format!("ended {ended:?} started {started:?}"),
    );

    // Keys and moves during a loan: a long drag, and the user's input as
    // it goes out.
    // Each drag a path of its own: the same call again would be a retry
    // of the finished one.
    let drag = |dy: i64| {
        control.call(
            "drag",
            json!({ "sessionKey": KEY, "displayId": id, "realInput": true,
                    "fromX": middle.0 - 150, "fromY": middle.1 + dy,
                    "toX": middle.0 + 150, "toY": middle.1 + dy }),
        )
    };
    sleep_ms(1200);
    let typed_before = holder.text();
    let saved = where_now();
    let (dragged, live, target, during) = std::thread::scope(|scope| {
        let dragging = scope.spawn(|| drag(0));
        let live = wait_glow(control, "live", 8000);
        let _ = hid(&["key", "0"]);
        let _ = hid(&["key", "11"]);
        let at = where_now();
        let target = (at.0 + 120.0, at.1 + 160.0);
        let _ = hid(&["hover", &target.0.to_string(), &target.1.to_string()]);
        let during = where_now();
        (dragging.join().expect("drag thread"), live, target, during)
    });
    let queue = wait_ended()?;
    sleep_ms(500);
    let state = control.call("debugAutonomy", json!({}))?;
    let back = where_now();
    check(
        "the drag ran in YOLO's loan, and the user's input didn't stop it",
        live && dragged.as_ref().is_ok_and(|r| r["route"] == "real_input"),
        format!("live {live} {dragged:?}"),
    );
    check(
        "moves posted during the loan don't take the pointer, which comes back after",
        ((during.0 - target.0).abs() > 20.0 || (during.1 - target.1).abs() > 20.0)
            && (back.0 - saved.0).abs() < 1.5
            && (back.1 - saved.1).abs() < 1.5,
        format!("target {target:?} during {during:?} saved {saved:?} back {back:?}"),
    );
    check(
        "the keys arrive after the loan, in order, in the stand-in's field",
        holder.text() == format!("{typed_before}ab")
            && probe(&["frontpid"])?.trim() == holder.pid().to_string()
            && queue["grantActive"] == false,
        format!(
            "{typed_before:?} -> {:?}; hold {}",
            holder.text(),
            state["armedHold"]
        ),
    );
    check(
        "a touch doesn't end YOLO",
        state["mode"] == "yolo",
        state["mode"].to_string(),
    );
    sleep_ms(1200);

    // The Stop chord during a loan.
    let typed_before = holder.text();
    let (dragged, live) = std::thread::scope(|scope| {
        let dragging = scope.spawn(|| drag(30));
        let live = wait_glow(control, "live", 8000);
        let _ = hid(&["key", PERIOD, CHORD_FLAGS]);
        (dragging.join().expect("drag thread"), live)
    });
    let queue = wait_ended()?;
    sleep_ms(500);
    let state = control.call("debugAutonomy", json!({}))?;
    check(
        "the Stop chord during a loan ends it and gives Ask",
        live && dragged.is_err()
            && queue["grantActive"] == false
            && state["mode"] == "ask"
            && state["hotkey"] == false,
        format!(
            "{dragged:?} {queue} mode {} hotkey {}",
            state["mode"], state["hotkey"]
        ),
    );
    check(
        "the chord reaches no window",
        holder.text() == typed_before,
        format!("{typed_before:?} -> {:?}", holder.text()),
    );
    control.call(
        "inputQueue",
        json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
    )?;

    // Secure Input: nothing goes out.
    yolo(control, &holder)?;
    control.call("debugActivity", json!({ "idleMs": 0, "secureInput": true }))?;
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
        "with Secure Input on, the request waits in YOLO too",
        waiting["waitingOn"] == "secureInput" && queue["grantActive"] == false,
        queue.to_string(),
    );
    if !waiting["requestId"].is_null() {
        control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "withdraw", "requestId": waiting["requestId"] }),
        )?;
    }
    let _ = call.wait();
    control.call("debugActivity", json!({ "clear": true }))?;
    control.call("debugShow", json!({ "surface": "autonomy-stop" }))?;
    drop(holder);
    Ok(())
}
