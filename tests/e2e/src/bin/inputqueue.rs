//! The Input Queue's card and what an agent sees (ADR 0013), with a preview
//! intent, so nothing is dragged and no real input is sent: the card drops
//! from the menu bar icon asking for approval, shows the grant once
//! approved, and goes away when the grant idles out after 30 seconds. The
//! agent then sees approval_lapsed, and a wait or a retry of the same
//! operation returns at once instead of waiting a minute for a card that
//! won't come. The card fits its content, and during the grant the Input
//! Bar sits at the bottom of the screen, clear of the card; its Stop ends
//! the grant. A menu item an Electron app needs a real click for asks on
//! the same card, which names it, and so does a double click. Three
//! preview drags ask on one card, and allowed, with the second shown
//! running, the card shows Stop. Captures the cards and the bar under
//! target/e2e. The card shows on your screen for about half a minute.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e inputqueue
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e inputqueue -- --buttons-only
//!
//! --buttons-only clicks the actual Allow and Not Now buttons through HID
//! input and checks grant state. No queued action executes. The sign counts
//! down first; the pointer and front app are restored after cleanup.
//!
//! Makes 1 display.

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    Control, Frame, Value, capture, check, click_card_button, debug_pid, hid, instance, json,
    probe, run, sh_lenient, sleep_ms, start_test_app, tool, windows_of,
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
        let buttons_only = std::env::args().any(|arg| arg == "--buttons-only");
        let saved = if buttons_only {
            Some((hid(&["where"])?, probe(&["frontpid"])?))
        } else {
            None
        };
        let result = if buttons_only {
            test_buttons(&control, key, &id, pid)
        } else {
            test(&control, key, &id, pid)
        };
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": key, "displayId": id, "action": "done" }),
        );
        let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
        if let Some((pointer, front)) = saved {
            let at: Vec<&str> = pointer.split_whitespace().collect();
            if let [x, y] = at[..] {
                let _ = hid(&["warp", x, y]);
            }
            if probe(&["frontpid"])? != front {
                probe(&["activate", front.trim()])?;
            }
        }
        result
    })
}

/// Click the painted buttons through the HID helper, never an approval hook.
/// The preview has no executable action. Keep the pointer still after Allow:
/// the helper's normal return movement would itself revoke a successful grant.
fn test_buttons(control: &Control, key: &str, id: &Value, pid: u32) -> Result<()> {
    let queue = |action: &str| {
        control.call(
            "inputQueue",
            json!({ "sessionKey": key, "displayId": id, "action": action }),
        )
    };
    let click = |button: &str| click_card_button(control, pid, button);
    control.call(
        "debugShow",
        json!({ "sessionKey": key, "surface": "input-queue", "displayId": id }),
    )?;
    sleep_ms(800);
    let before = queue("status")?;
    anyhow::ensure!(
        before["grantActive"] == false,
        "unexpected existing grant: {before}"
    );
    click("input-allow")?;
    let allowed = queue("status")?;
    check(
        "a real Allow click opens the grant",
        allowed["grantActive"] == true,
        format!("before {before}; after {allowed}"),
    );
    sleep_ms(1200);
    let settled = queue("status")?;
    check(
        "the grant survives the next queue tick without further input",
        settled["grantActive"] == true,
        format!("{settled}"),
    );
    queue("done")?;
    control.call(
        "debugShow",
        json!({ "sessionKey": key, "surface": "input-queue", "displayId": id }),
    )?;
    sleep_ms(800);
    click("input-decline")?;
    sleep_ms(200);
    let declined = queue("status")?;
    check(
        "a real Not Now click declines the preview request",
        declined["grantActive"] == false && declined["requests"][0]["state"] == "declined",
        format!("{declined}"),
    );
    Ok(())
}

fn test(control: &Control, key: &str, id: &Value, pid: u32) -> Result<()> {
    let show = |surface: &str| {
        control.call(
            "debugShow",
            json!({ "sessionKey": key, "surface": surface, "displayId": id }),
        )
    };
    // The queue card is the Help Card's 320-pt-wide window.
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };
    // The Input Bar: a short window of the app's own, below the edge's
    // level 1000.
    let bar = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.layer > 0 && w.layer < 1000 && w.width != 320.0 && w.height < 120.0)
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
        "Allow keeps the card up for the grant, and the agent sees approved, not done",
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

    // A double click asks on the same card, its point on the picture.
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    )?;
    control.call(
        "debugShow",
        json!({ "sessionKey": key, "surface": "input-queue-click", "displayId": id, "presses": 2 }),
    )?;
    sleep_ms(800);
    let click = card();
    let (ok, why) = click.as_ref().map_or((false, "no card".into()), |c| {
        fits(c, &["input-request", "input-decline", "input-allow"])
    });
    check(
        "a double click's request asks on the same card, its reason and both buttons inside it",
        ok && status()?["state"] == "queued",
        format!("{click:?} {why}"),
    );
    if let Some(c) = &click {
        let file = capture("inputqueue-click", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }
    show("input-queue-decline")?;

    // Three drags at once: the card numbers them on the picture and lists
    // them; then allowed, with the second shown running, it shows its
    // step and Stop. None runs: their calls never wait.
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    )?;
    show("input-queue-drags")?;
    sleep_ms(800);
    let many = card();
    let (ok, why) = many.as_ref().map_or((false, "no card".into()), |c| {
        fits(
            c,
            &[
                "input-request",
                "input-request-last",
                "input-decline",
                "input-allow",
            ],
        )
    });
    // The list's first and last rows, three rows apart: the last row's
    // top is at least two rows below the first's.
    let painted = control.call("debugCard", json!({})).unwrap_or_default();
    let row = |id: &str| -> Option<(f64, f64)> {
        let b = &painted["buttons"][id];
        Some((b[1].as_f64()?, b[3].as_f64()?))
    };
    let listed = match (row("input-request"), row("input-request-last")) {
        (Some((top, height)), Some((last, _))) => height > 10.0 && last >= top + 2.0 * height - 1.0,
        _ => false,
    };
    check(
        "three drags ask on one card that lists all three, the list and both buttons inside a window as tall as its content",
        ok && listed && many.as_ref().is_some_and(|c| c.height > 300.0),
        format!(
            "{why}, rows {:?} {:?}",
            row("input-request"),
            row("input-request-last")
        ),
    );
    if let Some(c) = &many {
        let file = capture("inputqueue-many", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }
    show("input-queue-decline")?;
    control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    )?;
    show("input-queue-running")?;
    sleep_ms(800);
    let running = card();
    let (ok, why) = running
        .as_ref()
        .map_or((false, "no card".into()), |c| fits(c, &["input-revoke"]));
    check(
        "the running step's card draws Stop inside a window as tall as its content, the bar shows",
        ok && bar().is_some(),
        format!("{why} bar {:?}", bar()),
    );
    if let Some(c) = &running {
        let file = capture("inputqueue-running", [c.x, c.y, c.width, c.height])?;
        println!("     card: {}", file.display());
    }
    show("input-bar-stop")?;
    Ok(())
}
