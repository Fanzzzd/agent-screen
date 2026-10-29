//! Window Authorization on a real window (ADR 0013): a stage session
//! starts Field on a user stand-in stage, and the target session takes it
//! through the conversation and the session grant, never through a card it
//! didn't need. The script checks:
//!
//! - an app-scoped grant takes the window without a card, but not while a
//!   Takeover pauses it, and the hand-back resumes it;
//! - the user's Revoke, from the Viewer or Peek, leaves the Handover in place;
//! - returning all Handovers puts the window back where it was, at its frame,
//!   and leaves the window the agent launched;
//! - a returned window stays excluded when the user grants its app or all
//!   apps again, so place_window asks with a card;
//! - the user's words for that one window take it again and clear the
//!   exclusion, and that one-time instruction leaves no grant behind;
//! - the user's Take Back sends it home again and excludes it again.
//!
//! The Viewer and Peek open on the stage, out of the user's sight, and each
//! authorization state is captured under target/e2e with the language in its
//! name. AGENTSCREEN_LANG=zh runs it in Chinese.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e windowaccess
//!
//! Makes 2 displays: the stage and the target. No real pointer or keyboard
//! input, and nothing on the user's screens moves.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Frame, McpClient, ToolResult, Value, capture, check, debug_pid, field_app, instance,
    json, open_user_stage, out, probe, run, sh_lenient, sleep_ms, start_test_app, stop_test_app,
    windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let lang = std::env::var("AGENTSCREEN_LANG").unwrap_or_else(|_| "en".into());
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", &lang)])?;
        field_app()?;
        let tag = format!("--windowaccess-{}", std::process::id());
        let result = test(&lang, &tag);
        let _ = Control::connect().and_then(|c| c.call("debugShow", json!({ "surface": "hide" })));
        let _ =
            Control::connect().and_then(|c| c.call("debugShow", json!({ "surface": "collapse" })));
        // Ended in `test` before the displays; this catches a run that
        // failed before getting there. With one still running, the test app
        // and its stage stay up.
        if end_fixtures(&tag) {
            stop_test_app();
        } else {
            check(
                "the script's Field fixtures quit",
                false,
                "a Field started with this run's tag is still running; the test app is left running",
            );
        }
        result
    })
}

/// End the Field instances started with `tag`, and say whether none is
/// left. `--` keeps pkill from reading the tag, which starts with `--`, as
/// an option.
fn end_fixtures(tag: &str) -> bool {
    let left = || !sh_lenient("/usr/bin/pgrep", &["-f", "--", tag]).is_empty();
    sh_lenient("/usr/bin/pkill", &["-TERM", "-f", "--", tag]);
    for _ in 0..50 {
        if !left() {
            return true;
        }
        sleep_ms(100);
    }
    sh_lenient("/usr/bin/pkill", &["-KILL", "-f", "--", tag]);
    sleep_ms(500);
    !left()
}

/// Answer the display's Window Request once it shows, as the card's
/// button would.
fn answer_soon(display: u64, give: bool) -> std::thread::JoinHandle<Result<Value>> {
    std::thread::spawn(move || {
        sleep_ms(800);
        Control::connect()?.call(
            "debugAnswerWindow",
            json!({ "displayId": display, "give": give }),
        )
    })
}

/// A window's frame in global points, from the window server.
fn frame_of(pid: u32, window: u64) -> Option<Frame> {
    windows_of(pid).into_iter().find(|w| w.id == window)
}

fn same_frame(a: &Frame, b: &Frame) -> bool {
    (a.x - b.x).abs() <= 2.0
        && (a.y - b.y).abs() <= 2.0
        && (a.width - b.width).abs() <= 2.0
        && (a.height - b.height).abs() <= 2.0
}

fn ids(v: &Value) -> Vec<u64> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().or_else(|| x["windowId"].as_u64()))
                .collect()
        })
        .unwrap_or_default()
}

fn test(lang: &str, tag: &str) -> Result<()> {
    let stage = Control::connect()?;
    let stage_id = open_user_stage(&stage)?;
    // The target display first: making it moves the stage's origin, and
    // the user's window's home is read after that.
    let session = format!("windowaccess-{}", std::process::id());
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
    let id = client
        .call(
            "create_display",
            json!({ "title": "Window access", "width": 1280, "height": 800 }),
        )?
        .display_id()
        .context("create_display gave no display_id")?;
    let result = user_window(&stage, stage_id, tag).and_then(|(user_pid, window, home)| {
        steps(&stage, &client, id, window, user_pid, &home, lang, tag)
    });
    // The fixtures go before either display: a user-owned Field on a
    // stage that ends would land on the user's screen.
    let ended = end_fixtures(tag);
    check(
        "the script's Field fixtures quit before the displays end",
        ended,
        "a Field started with this run's tag is still running",
    );
    if !ended {
        eprintln!(
            "fixture cleanup failed; holding both sessions open until their Field processes exit"
        );
        while !end_fixtures(tag) {
            sleep_ms(1000);
        }
    }
    let _ = client.call("end_display", json!({ "display_id": id }));
    client.close();
    let _ = stage.call("endDisplay", json!({ "displayId": stage_id }));
    result
}

/// The user's Field window on the stage. The stage session launches it,
/// so it is claimed before it shows and AeroSpace leaves it on the stage,
/// then gives it up (`debugUserWindow`): a window a session launched is
/// that session's, not the user's to give. Field quits within 60 s when
/// given a lifetime, so it gets none and the script ends it.
fn user_window(stage: &Control, stage_id: u64, tag: &str) -> Result<(u32, u64, Frame)> {
    let field = stage.call(
        "launchApp",
        json!({
            "displayId": stage_id, "app": out().join("Field.app"),
            "arguments": [format!("{tag}-user")],
            "environment": { "FIELD_DISPLAY": stage_id.to_string() }
        }),
    )?;
    let pid = field["pid"].as_u64().context("Field's pid")? as u32;
    let window = field["window"]["windowId"]
        .as_u64()
        .context("Field's window")?;
    sleep_ms(1500);
    stage.call(
        "debugUserWindow",
        json!({ "displayId": stage_id, "windowId": window }),
    )?;
    let home = frame_of(pid, window).context("Field's window on the stage")?;
    let bounds: Vec<f64> = probe(&["bounds", &stage_id.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    anyhow::ensure!(
        bounds.len() == 4
            && home.x >= bounds[0]
            && home.y >= bounds[1]
            && home.x < bounds[0] + bounds[2]
            && home.y < bounds[1] + bounds[3],
        "the user's Field window {home:?} isn't on the stage {bounds:?}"
    );
    anyhow::ensure!(
        claimed(window),
        "the user's Field window isn't in the claim file, so AeroSpace could take it"
    );
    println!("     user window {window} at {home:?}");
    Ok((pid, window, home))
}

/// Whether the test app's claim file names the window, so AeroSpace leaves
/// it where it is.
fn claimed(window: u64) -> bool {
    let Some(pid) = debug_pid() else {
        return false;
    };
    let file = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!(
        "Library/Application Support/AeroSpace/claims/{pid}.json"
    ));
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|v| ids(&v["windows"]).contains(&window))
}

#[allow(clippy::too_many_arguments)]
fn steps(
    stage: &Control,
    client: &McpClient,
    id: u64,
    window: u64,
    user_pid: u32,
    home: &Frame,
    lang: &str,
    tag: &str,
) -> Result<()> {
    let status = || -> Result<ToolResult> {
        client.call(
            "window_access",
            json!({ "display_id": id, "action": "status" }),
        )
    };
    let words = |text: &str, extra: Value| -> Value {
        let mut v = extra;
        v["display_id"] = json!(id);
        v["user_instruction"] = json!(text);
        v
    };
    let on_display = || -> Result<Vec<agent_screen_tests::Window>> {
        Ok(client
            .call("list_windows", json!({ "display_id": id }))?
            .windows()
            .unwrap_or_default())
    };
    let origin_of = |w: u64| -> Result<Option<String>> {
        let listed = client.call("list_windows", json!({ "display_id": id }))?;
        Ok(listed.get("windows").as_array().and_then(|ws| {
            ws.iter()
                .find(|x| x["windowId"].as_u64() == Some(w))
                .and_then(|x| x["origin"].as_str().map(str::to_string))
        }))
    };
    let pid = debug_pid().context("no test app")?;
    let shoot_viewer = |name: &str| -> Result<()> {
        stage.call("debugShow", json!({ "surface": "access", "displayId": id }))?;
        sleep_ms(700);
        let viewer = stage.call("debugViewer", json!({}))?;
        let f: Vec<f64> = viewer["frame"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        anyhow::ensure!(f.len() == 4, "no Viewer frame: {viewer}");
        let file = capture(
            &format!("windowaccess-{lang}-viewer-{name}"),
            [f[0], f[1], f[2], f[3].min(180.0)],
        )?;
        println!("     Viewer {name}: {}", file.display());
        Ok(())
    };
    let shoot_peek = |name: &str| -> Result<()> {
        let before: Vec<u64> = windows_of(pid).iter().map(|w| w.id).collect();
        stage.call("debugShow", json!({ "surface": "peek" }))?;
        sleep_ms(900);
        let peek = windows_of(pid)
            .into_iter()
            .filter(|w| !before.contains(&w.id))
            .max_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height)));
        match peek {
            Some(p) => {
                let file = capture(
                    &format!("windowaccess-{lang}-peek-{name}"),
                    [p.x, p.y, p.width, p.height],
                )?;
                println!("     Peek {name}: {}", file.display());
            }
            None => println!("     Peek {name}: no new window to capture"),
        }
        stage.call("debugShow", json!({ "surface": "hide" }))?;
        sleep_ms(400);
        Ok(())
    };

    // The agent's own window, which no return may send anywhere.
    let launched = client.call(
        "launch_app",
        json!({
            "display_id": id, "app": out().join("Field.app"),
            "arguments": [format!("{tag}-agent")],
            "environment": { "FIELD_DISPLAY": id.to_string() }
        }),
    )?;
    let own = launched
        .window()
        .map(|w| w.window_id)
        .context(format!("launch_app gave no window: {}", launched.text()))?;
    check(
        "the agent's own Field window is on its display",
        on_display()?.iter().any(|w| w.window_id == own),
        launched.text(),
    );

    // App-scoped grant, paused by a Takeover.
    let r = client.call(
        "window_access",
        words(
            "You may take my Field windows for this task.",
            json!({ "action": "grant", "apps": ["Field"] }),
        ),
    )?;
    check(
        "an app-scoped grant names Field by its bundle id",
        !r.is_error && r.get("scope") == "apps" && r.text().contains("app.agentscreen.e2e.field"),
        r.text(),
    );
    shoot_viewer("apps")?;
    shoot_peek("apps")?;
    client.call(
        "request_control",
        words("Pause, I want to look.", json!({ "control": "user" })),
    )?;
    let r = status()?;
    check(
        "a Takeover pauses the grant",
        r.get("scope") == "apps" && r.get("paused") == true,
        r.text(),
    );
    shoot_viewer("paused")?;
    shoot_peek("paused")?;
    let r = client.call(
        "place_window",
        json!({ "display_id": id, "app": "Field", "timeout_seconds": 2 }),
    )?;
    check(
        "the paused grant takes nothing",
        r.is_error && frame_of(user_pid, window).is_some_and(|f| same_frame(&f, home)),
        r.text(),
    );
    client.call("request_control", words("Done, continue.", json!({})))?;
    let r = client.call(
        "place_window",
        json!({ "display_id": id, "app": "Field", "timeout_seconds": 2 }),
    )?;
    sleep_ms(800);
    check(
        "after the hand-back the grant takes the user's Field window without a card",
        !r.is_error
            && r.get("handedOver") == true
            && origin_of(window)? == Some("handedOver".into()),
        r.text(),
    );
    anyhow::ensure!(
        origin_of(window)? == Some("handedOver".into()),
        "the user's window wasn't handed over; the steps after this need it"
    );
    check(
        "the grant took the user's window, not the agent's",
        on_display()?.iter().any(|w| w.window_id == window)
            && on_display()?.iter().any(|w| w.window_id == own),
        format!("{:?}", on_display()?),
    );

    // The user's Revoke leaves the Handover where it is.
    stage.call(
        "debugShow",
        json!({ "surface": "access-revoke", "displayId": id }),
    )?;
    sleep_ms(500);
    let r = status()?;
    check(
        "the user's Revoke ends the grant",
        r.get("scope") == "none" && ids(r.get("handovers")).contains(&window),
        r.text(),
    );
    check(
        "Revoke returns no window",
        on_display()?.iter().any(|w| w.window_id == window),
        format!("{:?}", on_display()?),
    );
    shoot_viewer("revoked")?;

    // Return all: home, at its frame; the agent's window stays.
    let r = client.call(
        "window_access",
        words(
            "Give me back the windows I handed over.",
            json!({ "action": "return", "all": true }),
        ),
    )?;
    sleep_ms(1000);
    check(
        "return all returns the Handover only",
        !r.is_error && ids(r.get("returned")) == vec![window],
        r.text(),
    );
    let back = frame_of(user_pid, window);
    check(
        "the returned window is back on the stage at its frame",
        back.as_ref().is_some_and(|f| same_frame(f, home)),
        format!("{back:?}, home {home:?}"),
    );
    let here = on_display()?;
    check(
        "the agent's own window stays on its display",
        here.iter().any(|w| w.window_id == own) && !here.iter().any(|w| w.window_id == window),
        format!("{here:?}"),
    );
    let r = status()?;
    check(
        "a returned window is excluded",
        ids(r.get("excluded")).contains(&window) && ids(r.get("handovers")).is_empty(),
        r.text(),
    );

    // Granting its app, or all apps, again doesn't cover it.
    for (scope, grant) in [
        ("apps", json!({ "action": "grant", "apps": ["Field"] })),
        ("all", json!({ "action": "grant", "all": true })),
    ] {
        client.call(
            "window_access",
            words("You may take my Field windows again.", grant),
        )?;
        let answer = answer_soon(id, false);
        let r = client.call(
            "place_window",
            json!({ "display_id": id, "app": "Field", "timeout_seconds": 10 }),
        )?;
        let answered = answer.join().expect("answer thread")?;
        check(
            &format!("after a regrant of {scope}, the excluded window needs a card"),
            r.is_error
                && answered["answered"] == true
                && frame_of(user_pid, window).is_some_and(|f| same_frame(&f, home)),
            format!("{} / {answered}", r.text()),
        );
        let s = status()?;
        check(
            &format!("the regrant of {scope} keeps the exclusion"),
            s.get("scope") == scope && ids(s.get("excluded")).contains(&window),
            s.text(),
        );
    }
    shoot_viewer("all")?;
    shoot_peek("all")?;

    // One instruction for that window takes it and clears the exclusion;
    // it leaves no grant of its own.
    client.call(
        "window_access",
        words("Stop taking my windows.", json!({ "action": "revoke" })),
    )?;
    let r = client.call(
        "place_window",
        words(
            "Take that Field window again.",
            json!({ "window_id": window, "timeout_seconds": 2 }),
        ),
    )?;
    sleep_ms(800);
    check(
        "the user's words take the excluded window at once",
        !r.is_error
            && r.get("handedOver") == true
            && origin_of(window)? == Some("handedOver".into()),
        r.text(),
    );
    let s = status()?;
    check(
        "the explicit instruction clears its exclusion and grants nothing",
        s.get("scope") == "none"
            && !ids(s.get("excluded")).contains(&window)
            && ids(s.get("handovers")).contains(&window),
        s.text(),
    );

    // The user's Take Back: home again, and excluded again. Only for a
    // window that is a Handover here: Take Back of any other sends it to
    // the user's main screen.
    anyhow::ensure!(
        origin_of(window)? == Some("handedOver".into()),
        "the user's window isn't a Handover on the display; no Take Back"
    );
    stage.call(
        "debugTakeBack",
        json!({ "displayId": id, "windowId": window }),
    )?;
    sleep_ms(800);
    let back = frame_of(user_pid, window);
    check(
        "Take Back puts the window back at its frame",
        back.as_ref().is_some_and(|f| same_frame(f, home)),
        format!("{back:?}, home {home:?}"),
    );
    let s = status()?;
    check(
        "Take Back excludes the window again",
        ids(s.get("excluded")).contains(&window),
        s.text(),
    );
    // No grant, and the one-time words don't carry over: a card.
    let answer = answer_soon(id, false);
    let r = client.call(
        "place_window",
        json!({ "display_id": id, "app": "Field", "timeout_seconds": 10 }),
    )?;
    let answered = answer.join().expect("answer thread")?;
    check(
        "without new words, taking it again asks with a card",
        r.is_error && answered["answered"] == true,
        format!("{} / {answered}", r.text()),
    );
    check(
        "the agent's own window is still on its display at the end",
        on_display()?.iter().any(|w| w.window_id == own),
        format!("{:?}", on_display()?),
    );
    Ok(())
}
