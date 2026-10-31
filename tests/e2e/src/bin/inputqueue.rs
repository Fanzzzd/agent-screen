//! The Input Queue's card and what an agent sees (ADR 0013), with a preview
//! intent, so nothing is dragged and no real input is sent: the card drops
//! from the menu bar icon asking for approval, shows the grant once
//! approved, and goes away when the grant idles out after 30 seconds. The
//! agent then sees approval_lapsed, and a wait or a retry of the same
//! operation returns at once instead of waiting a minute for a card that
//! won't come. Captures both cards under target/e2e. The card shows on
//! your screen for about half a minute.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e inputqueue
//!
//! Makes 1 display.

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    Control, Frame, Value, capture, check, debug_pid, instance, json, run, sleep_ms,
    start_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let pid = debug_pid().context("no test app")?;
        let control = Control::connect()?;
        let key = "inputqueue";
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": key, "title": "Input queue" }),
        )?;
        let id = made["displayId"].clone();
        let result = test(&control, key, &id, pid);
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": key, "displayId": id, "action": "done" }),
        );
        let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
        result
    })
}

fn test(control: &Control, key: &str, id: &Value, pid: u32) -> Result<()> {
    let show = |surface: &str| {
        control.call(
            "debugShow",
            json!({ "sessionKey": key, "surface": surface, "displayId": id }),
        )
    };
    // The queue card is the card's 320-pt-wide window.
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };
    let status = || -> Result<Value> {
        let state = control.call(
            "inputQueue",
            json!({ "sessionKey": key, "displayId": id, "action": "status" }),
        )?;
        Ok(state["requests"][0].clone())
    };
    let icon = || -> String {
        control
            .call("debugIcon", json!({}))
            .ok()
            .and_then(|v| v["state"].as_str().map(String::from))
            .unwrap_or_default()
    };

    check("no card before a request", card().is_none(), "");
    show("input-queue")?;
    sleep_ms(800);
    let asking = card();
    let request = status()?;
    check(
        "a request shows the queue card, the icon asks, and the agent sees queued",
        asking.is_some() && icon() == "asking" && request["state"] == "queued",
        format!("{asking:?} icon {} {request}", icon()),
    );
    if let Some(c) = asking {
        let file = capture("inputqueue-ask", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }

    // The same preview token: approves the request already waiting.
    show("input-queue-approved")?;
    sleep_ms(800);
    let granted = card();
    let request = status()?;
    check(
        "Allow All keeps the card up for the grant, and the agent sees approved, not done",
        granted.is_some() && request["state"] == "approved",
        format!("{granted:?} {request}"),
    );
    if let Some(c) = granted {
        let file = capture("inputqueue-granted", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }

    // Nothing runs, so the grant idles out after 30 seconds.
    let since = Instant::now();
    let mut lapsed = Value::Null;
    while since.elapsed().as_secs() < 40 {
        lapsed = status()?;
        if lapsed["state"] == "approval_lapsed" {
            break;
        }
        sleep_ms(500);
    }
    sleep_ms(1500);
    check(
        "after the idle grant ends, the agent sees approval_lapsed with a next step, and no card asks again",
        lapsed["state"] == "approval_lapsed"
            && lapsed["next"].is_string()
            && card().is_none()
            && icon() != "asking",
        format!("{lapsed} card {:?} icon {}", card(), icon()),
    );

    let request_id = lapsed["requestId"].clone();
    let asked = Instant::now();
    let waited = control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "wait",
                "requestId": request_id, "timeout": 60 }),
    )?;
    let wait_ms = asked.elapsed().as_millis();
    let asked = Instant::now();
    let retried = control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "request",
                "operation": format!("preview:{id}"),
                "reason": "Preview a queued drag on this display", "timeout": 60 }),
    )?;
    let retry_ms = asked.elapsed().as_millis();
    check(
        "a wait and a retry of the same operation return at once with approval_lapsed, and ask nothing",
        wait_ms < 3000
            && retry_ms < 3000
            && waited["requests"][0]["state"] == "approval_lapsed"
            && retried["requests"][0]["requestId"] == request_id
            && retried["requests"][0]["state"] == "approval_lapsed"
            && card().is_none(),
        format!("wait {wait_ms} ms, retry {retry_ms} ms, {retried}"),
    );
    Ok(())
}
