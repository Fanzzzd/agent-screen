//! The Viewer's header at its narrowest, on a 640 × 1600 display: the
//! primary button (Take Over, then Take Over for a Help Request, then
//! Let the agent Continue during a Takeover) beside Give | Take Back.
//! Captures each under target/e2e with the language in its name, and checks
//! that the header's buttons end inside the Viewer. The Viewer shows on your
//! screen for a few seconds.
//!
//!     AGENTSCREEN_INSTANCE=e2e AGENTSCREEN_LANG=zh cargo e2e viewerheader
//!
//! Makes 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, capture, check, instance, json, now_ms, run, sleep_ms, start_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("set AGENTSCREEN_INSTANCE");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[])?;
        let control = Control::connect()?;
        let session = format!("viewerheader-{}", now_ms());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        let id = client
            .call(
                "create_display",
                json!({ "title": "Book a flight", "width": 640, "height": 1600 }),
            )?
            .display_id()
            .context("create_display gave no display_id")?;
        let result = test(&control, &client, id);
        let _ = control.call("debugTakeover", json!({ "displayId": id, "active": false }));
        let _ = client.call("end_display", json!({ "display_id": id }));
        let _ = control.call("debugShow", json!({ "surface": "collapse" }));
        client.close();
        result
    })
}

fn test(control: &Control, client: &McpClient, id: u64) -> Result<()> {
    let lang = std::env::var("AGENTSCREEN_LANG").unwrap_or_else(|_| "system".into());
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    let shoot = |kind: &str| -> Result<()> {
        sleep_ms(500);
        let viewer = control.call("debugViewer", json!({}))?;
        let frame = rect(&viewer["frame"]).context("no Viewer frame")?;
        let file = capture(
            &format!("viewerheader-{lang}-{kind}"),
            [frame[0], frame[1], frame[2], 60.0],
        )?;
        println!("     {kind}: {} ({} pt wide)", file.display(), frame[2]);
        // Give | Take Back is the header's last text button. After it come
        // two 28-pt icon buttons, 8-pt gaps and 12 pt of padding: 84 pt.
        if let Some(menu) = rect(&viewer["takeBack"]) {
            check(
                &format!("{kind}: the header's buttons fit the Viewer"),
                menu[0] + menu[2] <= frame[2] - 84.0 + 0.5,
                format!("button {menu:?}, Viewer {frame:?}"),
            );
        }
        Ok(())
    };
    shoot("agent")?;
    client.call(
        "request_help",
        json!({ "display_id": id, "reason": "Sign in", "timeout_seconds": 1 }),
    )?;
    shoot("help")?;
    control.call("debugTakeover", json!({ "displayId": id, "active": true }))?;
    shoot("takeover")?;
    Ok(())
}

/// `[x, y, w, h]` from an array or an object with those keys.
fn rect(v: &serde_json::Value) -> Option<[f64; 4]> {
    let at = |i: usize, k: &str| v.get(i).or_else(|| v.get(k)).and_then(|n| n.as_f64());
    Some([
        at(0, "x")?,
        at(1, "y")?,
        at(2, "width").or_else(|| at(2, "w"))?,
        at(3, "height").or_else(|| at(3, "h"))?,
    ])
}
