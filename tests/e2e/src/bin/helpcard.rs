//! The Help Card: it drops from the menu bar icon for a Help Request, Later
//! puts it away, a new request brings it back, and Take Over or the Viewer
//! showing that display hides it. The request's steps show in the Viewer
//! during the Takeover. Captures the card and the Viewer header
//! under target/e2e. The card shows on the user's screen for a few seconds.
//!
//!     AGENTSCREEN_INSTANCE=e2e E2E_NOW=1 cargo e2e helpcard

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Frame, McpClient, capture, check, debug_pid, instance, json, now_ms, run, sleep_ms,
    start_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("set AGENTSCREEN_INSTANCE");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[])?;
        let pid = debug_pid().context("no test app")?;
        let control = Control::connect()?;
        let session = format!("helpcard-{}", now_ms());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        let display_id = client
            .call("create_display", json!({ "title": "Book a flight" }))?
            .display_id()
            .context("create_display gave no display_id")?;
        let result = test(&control, &client, pid, display_id);
        // Cleanup goes on to close the client either way.
        let _ = client.call("end_display", json!({ "display_id": display_id }));
        client.close();
        drop(control);
        result
    })
}

fn test(control: &Control, client: &McpClient, pid: u32, display_id: u64) -> Result<()> {
    let show = |surface: &str| {
        control.call(
            "debugShow",
            json!({ "surface": surface, "displayId": display_id }),
        )
    };
    // The card: the app's 320-pt-wide window, if it is on screen.
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };

    check("no card without a Help Request", card().is_none(), "");
    window_request(control, pid, display_id)?;

    client.call(
        "request_help",
        json!({
            "display_id": display_id,
            "reason": "Sign in to United. It wants a code sent to your phone.",
            "timeout_seconds": 1,
        }),
    )?;
    sleep_ms(600);
    let shown = card();
    check(
        "a Help Request shows the card",
        shown.is_some(),
        json!(shown).to_string(),
    );
    if let Some(s) = shown {
        let file = capture("helpcard", [s.x, s.y, s.width, s.height])?;
        println!("     card: {}", file.display());
    }

    show("card-later")?;
    sleep_ms(300);
    check("Later puts the card away", card().is_none(), "");

    client.call(
        "request_help",
        json!({
            "display_id": display_id,
            "reason": "Enter the code from your phone",
            "steps": [
                "Click the code field",
                "Type the six digits from the text message",
                "Click Verify, then hand the display back",
            ],
            "timeout_seconds": 1,
        }),
    )?;
    sleep_ms(600);
    check("a new reason brings the card back", card().is_some(), "");

    show("card-open")?;
    sleep_ms(600);
    check(
        "the Viewer on that display hides the card",
        card().is_none(),
        "",
    );
    let viewer = windows_of(pid)
        .into_iter()
        .find(|w| w.width > 400.0 && w.layer > 0);
    if let Some(v) = viewer {
        let file = capture("viewer-header", [v.x, v.y, v.width, 60.0])?;
        println!("     header: {}", file.display());
    }
    let before = image_offset(control);
    // Tidying up only; the checks after it don't depend on it.
    let _ = show("collapse");
    sleep_ms(500);

    show("card-take-over")?;
    sleep_ms(600);
    check("Take Over hides the card", card().is_none(), "");
    // Three lines of steps push the display down in the Viewer.
    let during = image_offset(control);
    check(
        "the Viewer lists the request's steps during the Takeover",
        matches!((before, during), (Some(b), Some(d)) if d > b + 30.0),
        format!("display image {before:?} pt below the Viewer's top, then {during:?}"),
    );
    if let Some(v) = windows_of(pid)
        .into_iter()
        .find(|w| w.width > 400.0 && w.layer > 0)
    {
        let file = capture("viewer-steps", [v.x, v.y, v.width, 140.0])?;
        println!("     steps: {}", file.display());
    }
    control.call(
        "debugTakeover",
        json!({ "displayId": display_id, "active": false }),
    )?;
    sleep_ms(300);
    check(
        "the Takeover's end answers the request: no card",
        card().is_none(),
        "",
    );
    Ok(())
}

/// The Window Request's card, for a short app name and a long one: its
/// captures go under target/e2e with the language in their names.
fn window_request(control: &Control, pid: u32, display_id: u64) -> Result<()> {
    let lang = std::env::var("AGENTSCREEN_LANG").unwrap_or_else(|_| "system".into());
    for (name, app, title) in [
        ("short", "TextEdit", "Notes.txt"),
        (
            "long",
            "Visual Studio Code - Insiders",
            "agent-screen — apps/macos/src/ui/help_card.rs — a very long window title",
        ),
    ] {
        control.call(
            "debugShow",
            json!({ "surface": "window-request", "displayId": display_id, "app": app, "title": title }),
        )?;
        sleep_ms(300);
        let shown = windows_of(pid)
            .into_iter()
            .find(|w| w.layer > 0 && w.height < 200.0 && w.width < 450.0);
        check(
            &format!("a Window Request for {app} shows its card"),
            shown.is_some(),
            json!(shown).to_string(),
        );
        if let Some(s) = shown {
            let file = capture(
                &format!("windowrequest-{lang}-{name}"),
                [s.x, s.y, s.width, s.height],
            )?;
            println!("     card: {}", file.display());
        }
        control.call(
            "debugShow",
            json!({ "surface": "card-keep", "displayId": display_id }),
        )?;
        sleep_ms(400);
    }
    Ok(())
}

/// How far below the Viewer's top its display image starts, in points.
fn image_offset(control: &Control) -> Option<f64> {
    let viewer = control.call("debugViewer", json!({})).ok()?;
    let top = |v: &serde_json::Value| v.get(1).or_else(|| v.get("y")).and_then(|y| y.as_f64());
    Some(top(&viewer["image"])? - top(&viewer["frame"])?)
}
