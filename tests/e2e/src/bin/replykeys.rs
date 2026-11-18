//! The Help Card's reply field gives the keyboard back (ADR 0015), on the
//! real path: a real click on Reply makes the card key over the user's
//! app, real keys go into the field, a real Escape closes it, and the next
//! real keys reach the user's app again. Not the Viewer, which is keyable
//! and open beside it: a key there would start a Takeover and type into
//! the agent's app.
//!
//! Field stands in for the user's app, frontmost on the stage, which
//! stands in for the user's screen. The Help Request is the stage's own,
//! asked as a new MCP server asks it, and the Viewer shows the agent's
//! display.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e replykeys
//!
//! Makes 2 displays: the stage and the agent's display. A sign on your
//! screen counts down from 3, then the test moves your pointer and types
//! for a few seconds; your front app is activated again at the end.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, click_card_button, debug_pid, field_app, hid, instance, json, kill_ours,
    open_user_stage, our_binary, out, probe, run, sh_lenient, sign_step, sleep_ms, start_test_app,
    stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("set AGENTSCREEN_INSTANCE");
        return ExitCode::FAILURE;
    }
    let front = front_app();
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let field = field_app()?;
        let pid = debug_pid().context("no test app")?;
        let stage = Control::connect()?;
        let result = test(&stage, pid, &field);
        kill_ours(our_binary("Field"));
        stop_test_app();
        if let Some(front) = &front {
            let _ = sh_lenient(
                "/usr/bin/osascript",
                &[
                    "-e",
                    &format!("tell application id \"{front}\" to activate"),
                ],
            );
        }
        result
    })
}

/// The bundle id of the app in front, to bring it back afterwards.
fn front_app() -> Option<String> {
    let asn = sh_lenient("/usr/bin/lsappinfo", &["front"]);
    let info = sh_lenient(
        "/usr/bin/lsappinfo",
        &["info", "-only", "bundleid", asn.trim()],
    );
    info.split('"').nth(3).map(String::from)
}

/// The pid of the app in front.
fn front_pid() -> Option<i64> {
    let asn = sh_lenient("/usr/bin/lsappinfo", &["front"]);
    let info = sh_lenient("/usr/bin/lsappinfo", &["info", "-only", "pid", asn.trim()]);
    info.rsplit('=').next()?.trim().parse().ok()
}

fn rect(v: &serde_json::Value) -> Option<[f64; 4]> {
    let r: Vec<f64> = (0..4).filter_map(|i| v[i].as_f64()).collect();
    r.try_into().ok()
}

fn test(stage: &Control, pid: u32, field_path: &std::path::Path) -> Result<()> {
    // The agent's display first, from a connection of its own: a stage
    // made first was once parked over another display.
    let agent_conn = Control::connect()?;
    let agent = agent_conn.call(
        "createDisplay",
        json!({ "sessionKey": "replykeys", "title": "Reply keys", "width": 1280, "height": 800 }),
    )?;
    let agent_id = agent["displayId"].as_u64().context("display id")?;
    let stage_id = open_user_stage(stage)?;
    let saved = out().join("replykeys-field.txt");
    let _ = std::fs::remove_file(&saved);
    let field = stage.call(
        "launchApp",
        json!({
            "displayId": stage_id, "app": field_path,
            "arguments": [format!("--replykeys-{}", std::process::id())],
            "environment": {
                "FIELD_OUT": saved.to_str().unwrap(),
                "FIELD_DISPLAY": stage_id.to_string(),
                "FIELD_LIFETIME_MS": "60000",
            }
        }),
    )?;
    let field_pid = field["pid"].as_i64().context("Field's pid")?;
    let window = field["window"]["windowId"].clone();

    // The Viewer on the agent's display, beside the user's app.
    stage.call(
        "debugShow",
        json!({ "surface": "viewer", "displayId": agent_id }),
    )?;
    sleep_ms(1200);
    let viewer = stage.call("debugViewer", json!({}))?;
    let frame = rect(&viewer["frame"]).context("no Viewer frame")?;
    let bounds: Vec<f64> = probe(&["bounds", &stage_id.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let bounds: [f64; 4] = bounds.try_into().ok().context("the stage's bounds")?;
    // Field, 600 by about 330 points, at the stage's left edge, where
    // the Viewer leaves part of its text view uncovered.
    let (fw, fh) = (600.0, 332.0);
    let at = (bounds[0] + 10.0, bounds[1] + 200.0);
    let text_at = (
        bounds[0] + 10.0 + (frame[0] - bounds[0] - 10.0).min(fw) / 2.0,
        at.1 + 32.0 + 150.0,
    );
    anyhow::ensure!(
        frame[0] - bounds[0] - 10.0 > 80.0
            || text_at.1 < frame[1]
            || text_at.1 > frame[1] + frame[3],
        "the Viewer {frame:?} covers Field's text view on the stage {bounds:?}"
    );
    let moved = stage.call(
        "debugMoveWindow",
        json!({ "pid": field_pid, "windowId": window, "x": at.0, "y": at.1 }),
    )?;
    check(
        "Field goes beside the Viewer on the stage",
        moved["moved"] == true,
        format!("{moved} at {at:?}, {fw}x{fh}, click {text_at:?}, Viewer {frame:?}"),
    );
    sleep_ms(300);
    let read = || {
        std::fs::read_to_string(&saved)
            .unwrap_or_default()
            .split('\t')
            .next()
            .unwrap_or_default()
            .to_string()
    };

    // The user's app in front, typing.
    sign_step("click Field, type / 点按 Field 并输入");
    hid(&["click", &text_at.0.to_string(), &text_at.1.to_string()])?;
    sleep_ms(600);
    // Keys go only to Field: with another app in front they would land in
    // the user's own app.
    let front = front_pid();
    if front != Some(field_pid) {
        let windows = probe(&["windows", &field_pid.to_string()]).unwrap_or_default();
        anyhow::bail!(
            "Field isn't in front after the click (front pid {front:?}); its windows: {windows}"
        );
    }
    hid(&["type", "x", "40"])?;
    sleep_ms(300);
    check(
        "the user's app has the keyboard before the card",
        read() == "x",
        read(),
    );

    // The stage asks for help, as a new MCP server does.
    stage.call(
        "requestHelp",
        json!({ "displayId": stage_id, "reason": "Sign in to United.", "answers": true }),
    )?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while windows_of(pid)
        .iter()
        .all(|w| !(w.width == 320.0 && w.layer > 0))
        && std::time::Instant::now() < deadline
    {
        sleep_ms(100);
    }
    sleep_ms(500);
    sign_step("Reply, type, Escape / 回复、输入、Esc");
    click_card_button(stage, pid, "help-reply")?;
    sleep_ms(400);
    hid(&["type", "ab", "40"])?;
    sleep_ms(300);
    let card = stage.call("debugCard", json!({}))?;
    check(
        "a real click on Reply gives the field the keyboard",
        card["key"] == true && card["reply"]["chars"] == 2 && read() == "x",
        format!("{card} / field {:?}", read()),
    );
    hid(&["key", "53"])?;
    sleep_ms(400);
    hid(&["type", "cd", "40"])?;
    sleep_ms(400);
    let card = stage.call("debugCard", json!({}))?;
    let viewer = stage.call("debugViewer", json!({}))?;
    check(
        "after Escape the user's keys reach their app again",
        read() == "xcd",
        format!("field {:?}", read()),
    );
    check(
        "the card closed the field and has no keyboard",
        card["reply"].is_null() && card["key"] == false && card["cardKeys"] == false,
        card.to_string(),
    );
    check(
        "the Viewer didn't take the keyboard",
        viewer["key"] == false,
        viewer["key"].to_string(),
    );
    let wait = agent_conn.call(
        "waitForControl",
        json!({ "displayId": agent_id, "timeoutSeconds": 1 }),
    )?;
    check(
        "no Takeover started on the agent's display",
        wait["resolution"] == "handedBack",
        wait.to_string(),
    );
    let _ = stage.call("withdrawHelp", json!({ "displayId": stage_id }));
    let _ = agent_conn.call("endDisplay", json!({ "displayId": agent_id }));
    let _ = stage.call("endDisplay", json!({ "displayId": stage_id }));
    Ok(())
}
