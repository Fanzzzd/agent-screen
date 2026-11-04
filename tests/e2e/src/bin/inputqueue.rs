//! The Input Queue's card and what an agent sees (ADR 0013), with a preview
//! intent, so nothing is dragged and no real input is sent: the card drops
//! from the menu bar icon asking for approval, shows the grant once
//! approved, and goes away when the grant idles out after 30 seconds. The
//! agent then sees approval_lapsed, and a wait or a retry of the same
//! operation returns at once instead of waiting a minute for a card that
//! won't come. The card fits its content, and during the grant the Input
//! Bar sits at the bottom of the screen, clear of the card; its Stop ends
//! the grant. A menu item an Electron app needs a real click for asks on
//! the same card, which names it. Captures the cards and the bar under
//! target/e2e. The card shows on your screen for about half a minute.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e inputqueue
//!
//! Makes 1 display.

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    Control, Frame, Value, capture, check, debug_pid, instance, json, run, sh_lenient, sleep_ms,
    start_test_app, tool, windows_of,
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
    // The queue card is the card's 360-pt-wide window.
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 360.0 && w.layer > 0)
    };
    // The Input Bar: a short window of the app's own, below the edge's
    // level 1000.
    let bar = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.layer > 0 && w.layer < 1000 && w.width != 360.0 && w.height < 120.0)
    };
    let edges = || windows_of(pid).iter().filter(|w| w.layer == 1000).count();
    let meets = |a: &Frame, b: &Frame| {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    };
    // Another window of the app over the card, apart from the
    // click-through edge.
    let covering = |c: &Frame| -> Vec<Frame> {
        windows_of(pid)
            .into_iter()
            .filter(|w| w.layer > 0 && w.layer != 1000 && w.id != c.id && meets(w, c))
            .collect()
    };
    // Each of `ids` was painted by the card's last render, inside its
    // window, and the window is as tall as the card's content.
    let fits = |c: &Frame, ids: &[&str]| -> (bool, String) {
        let painted = control.call("debugCard", json!({})).unwrap_or_default();
        let content = painted["queueHeight"].as_f64().unwrap_or(0.0);
        let mut ok = c.width > 0.0 && content > 0.0 && content <= c.height + 1.0;
        for id in ids {
            let b = &painted["buttons"][*id];
            let r: Vec<f64> = (0..4).filter_map(|i| b[i].as_f64()).collect();
            ok &= r.len() == 4
                && r[2] > 0.0
                && r[3] > 0.0
                && r[0] + r[2] <= c.width + 0.5
                && r[1] + r[3] <= c.height + 0.5;
        }
        (
            ok,
            format!(
                "window {}x{}, content {content}, painted {}",
                c.width, c.height, painted["buttons"]
            ),
        )
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
    check(
        "the asking card fits its content: shorter than the old 500 pt, nothing over it, no bar yet",
        asking
            .as_ref()
            .is_some_and(|c| c.height > 200.0 && c.height < 500.0 && covering(c).is_empty())
            && bar().is_none(),
        format!("{asking:?} bar {:?}", bar()),
    );
    let (ok, why) = asking.as_ref().map_or((false, "no card".into()), |c| {
        fits(c, &["input-request", "input-decline", "input-allow"])
    });
    check(
        "the asking card draws its request row and both buttons inside a window as tall as its content",
        ok,
        why,
    );
    if let Some(c) = &asking {
        let file = capture("inputqueue-ask", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }

    // The same preview token: approves the request already waiting.
    show("input-queue-approved")?;
    sleep_ms(800);
    let granted = card();
    let request = status()?;
    check(
        "Allow Queue keeps the card up for the grant, and the agent sees approved, not done",
        granted.is_some() && request["state"] == "approved",
        format!("{granted:?} {request}"),
    );
    let shown = bar();
    check(
        "during the grant the Input Bar and the red edge show, the bar clear of the card and nothing over the card",
        granted
            .as_ref()
            .is_some_and(|c| c.height < 500.0 && covering(c).is_empty())
            && shown.as_ref().is_some_and(|b| {
                granted.as_ref().is_some_and(|c| !meets(b, c))
                    && b.width < 900.0
                    && (40.0..80.0).contains(&b.height)
            })
            && edges() > 0,
        format!("card {granted:?} bar {shown:?} edges {}", edges()),
    );
    let (ok, why) = granted
        .as_ref()
        .map_or((false, "no card".into()), |c| fits(c, &["input-revoke"]));
    check(
        "the granted card draws Stop inside a window as tall as its content",
        ok,
        why,
    );
    if let Some(c) = &granted {
        let file = capture("inputqueue-granted", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }
    if let Some(b) = &shown {
        let file = capture("inputqueue-bar", [b.x, b.y, b.width, b.height])?;
        println!("     bar: {}", file.display());
    }
    // The red edge at the main screen's top-left corner and top middle,
    // where a built-in display has its rounded corner and its notch.
    let main: Vec<f64> = sh_lenient(tool("probe")?, &["main"])
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    if let [w, _] = main[..] {
        let corner = capture("inputqueue-edge-corner", [0.0, 0.0, 80.0, 60.0])?;
        let notch = capture("inputqueue-edge-notch", [w / 2.0 - 150.0, 0.0, 300.0, 60.0])?;
        println!("     edge: {} {}", corner.display(), notch.display());
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

    // done forgets the lapsed request, so the preview asks afresh; the
    // bar's Stop then ends the new grant, as the user's click would, and
    // the card asks again.
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    )?;
    show("input-queue-approved")?;
    sleep_ms(800);
    let again = bar();
    show("input-bar-stop")?;
    sleep_ms(800);
    let request = status()?;
    let asks = card();
    let (ok, why) = asks.as_ref().map_or((false, "no card".into()), |c| {
        fits(c, &["input-request", "input-allow"])
    });
    check(
        "after Stop the card asks again at its full size, its request row and Allow inside it",
        ok && asks.as_ref().is_some_and(|c| c.height > 200.0),
        format!("{asks:?} {why}"),
    );
    check(
        "the Input Bar's Stop ends the grant: the bar and edge go, and the card asks again",
        // Still its full height: a bar sized from its window shrank.
        again
            .as_ref()
            .is_some_and(|b| (40.0..80.0).contains(&b.height))
            && bar().is_none()
            && edges() == 0
            && request["state"] == "queued"
            && card().is_some()
            && icon() == "asking",
        format!(
            "bar before {again:?}, after {:?}, edges {} {request} icon {}",
            bar(),
            edges(),
            icon()
        ),
    );

    // A menu item an Electron app needs a real click for: the card names
    // it, beside the drag already waiting.
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    )?;
    show("input-queue-menu")?;
    sleep_ms(800);
    let menu = card();
    let (ok, why) = menu.as_ref().map_or((false, "no card".into()), |c| {
        fits(c, &["input-request", "input-allow"])
    });
    let painted = control.call("debugCard", json!({})).unwrap_or_default();
    check(
        "a menu item's request asks on the same card, its row and Allow inside it",
        ok && status()?["state"] == "queued",
        format!("{menu:?} {why} {painted}"),
    );
    if let Some(c) = &menu {
        let file = capture("inputqueue-menu", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }
    show("input-queue-decline")?;
    Ok(())
}
