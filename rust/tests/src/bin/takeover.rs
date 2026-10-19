//! Takeover through the Viewer, beside the user's Agent Screen and out of
//! their sight: the Viewer opens on a stage display, and the user's clicks
//! and keys are made inside the app, through the Viewer's own handler. The
//! user's pointer and keyboard stay untouched, so this runs at any time.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e takeover

use std::process::{Command, ExitCode, Stdio};

use agent_screen_tests::{
    Control, Rect, check, document_text, instance, json, open_stage, probe, quit_discarding,
    quoted, run, running, skip, sleep_ms, start_test_app, stop_test_app, type_in_viewer,
    wait_for_unlocked,
};
use anyhow::{Context, Result};

/// The user may switch apps meanwhile; the test's own apps must never come forward.
fn watch_front(stolen: &mut Vec<String>) -> Result<()> {
    let now = probe(&["front"])?;
    if now == "TextEdit" || now.starts_with("Agent Screen") {
        stolen.push(now);
    }
    Ok(())
}

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("TextEdit") {
        skip("TextEdit is the user's");
    }
    if !wait_for_unlocked() {
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let result = test(&control);
        quit_discarding("TextEdit");
        drop(control);
        stop_test_app();
        result
    })
}

fn test(control: &Control) -> Result<()> {
    let mut stolen = Vec::new();
    open_stage(control)?;
    let id = control.call(
        "createDisplay",
        json!({
            "sessionKey": "takeover",
            "client": "Claude Code",
            "title": "Takeover",
            "width": 800,
            "height": 500,
        }),
    )?["displayId"]
        .as_u64()
        .context("display id")?;
    control.call("launchApp", json!({ "displayId": id, "app": "TextEdit" }))?;
    control.call(
        "type",
        json!({ "displayId": id, "text": "Agent wrote this." }),
    )?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    watch_front(&mut stolen)?;

    let frame = control.call("debugViewer", json!({}))?["frame"].clone();
    let [fx, fy, fw, fh]: Rect = serde_json::from_value(frame.clone())?;
    // A missing size reads as NaN, which no comparison passes.
    let main: Vec<f64> = probe(&["main"])?
        .split(' ')
        .map(|v| v.parse().unwrap_or(f64::NAN))
        .collect();
    let (mw, mh) = (
        main.first().copied().unwrap_or(f64::NAN),
        main.get(1).copied().unwrap_or(f64::NAN),
    );
    let on_main = fx < mw && fx + fw > 0.0 && fy < mh && fy + fh > 0.0;
    check(
        "the Viewer opens on the stage, not the user's screen",
        !on_main,
        frame.to_string(),
    );

    // A screen recording spreads an overlay over every display, the agent's
    // too; clicks must still reach the app under it.
    let recording = Command::new("/usr/sbin/screencapture")
        .args([
            "-v",
            "-x",
            "-V",
            "30",
            "-R",
            &format!("{fx},{fy},40,40"),
            "/tmp/agent-screen-takeover.mov",
        ])
        // screencapture -v reads its stdin for a key to stop on; an open,
        // empty pipe keeps it recording.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    sleep_ms(1500);
    control.call(
        "requestHelp",
        json!({ "displayId": id, "reason": "Please check the draft." }),
    )?;
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": 0.5, "y": 0.7 }),
    )?;
    let blocked = control
        .call("type", json!({ "displayId": id, "text": "x" }))
        .is_err();
    check("the agent's input fails during a Takeover", blocked, "");
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": 0.5, "y": 0.7 }),
    )?;
    type_in_viewer(control, " User added this.", 0)?;
    // An input method such as pinyin composes the letters in the Viewer
    // and holds the last ones until the user picks: the text can't match.
    let source = probe(&["source"]).unwrap_or_default();
    let typed_at = std::time::Instant::now();
    while !document_text().contains("User added this.") && typed_at.elapsed().as_secs() < 3 {
        sleep_ms(100);
    }
    watch_front(&mut stolen)?;
    // SIGTERM lets screencapture end the recording itself.
    Command::new("/bin/kill")
        .args(["-TERM", &recording.id().to_string()])
        .status()?;
    if source.starts_with("com.apple.keylayout.") {
        check(
            "the user's keys reach the agent's app, while the screen is recorded",
            document_text().contains("User added this."),
            quoted(&document_text()),
        );
    } else {
        println!(
            "skip the user's keys reach the agent's app: {source} composes them; got {}",
            quoted(&document_text())
        );
    }

    let resumed = control.begin(
        "waitForControl",
        json!({ "displayId": id, "timeoutSeconds": 10 }),
    )?;
    control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    check(
        "Let Continue hands control back",
        resumed.wait()?["outcome"] == "returned",
        "",
    );
    control.call("type", json!({ "displayId": id, "text": " Agent again." }))?;
    sleep_ms(300);
    check(
        "the agent types again after the hand-back",
        document_text().ends_with("Agent again."),
        quoted(&document_text()),
    );
    watch_front(&mut stolen)?;
    check(
        "the test's apps never came to the front",
        stolen.is_empty(),
        stolen.join(" "),
    );
    Ok(())
}
