//! A Control Request (#80): during a Takeover the agent asks for its display
//! back with request_control. The card shows under the menu bar icon; Keep
//! Control tells the waiting agent the user kept it and leaves the
//! Takeover; Let … Continue hands the display back; a timeout or a
//! withdrawal leaves the Takeover. Without a Takeover the call answers at
//! once and nothing shows. Captures the card under target/e2e. The card
//! shows on the user's screen for a few seconds.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e requestcontrol

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
        let session = format!("requestcontrol-{}", now_ms());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        let display_id = client
            .call("create_display", json!({ "title": "Book a flight" }))?
            .display_id()
            .context("create_display gave no display_id")?;
        let result = test(&control, &client, pid, display_id);
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
    let takeover = |active: bool| {
        control.call(
            "debugTakeover",
            json!({ "displayId": display_id, "active": active }),
        )
    };
    // The card: the app's 320-pt-wide window, if it is on screen.
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };
    let ask = |timeout: u64| {
        json!({
            "display_id": display_id,
            "reason": "You're signed in; I'll pick the seats now.",
            "timeout_seconds": timeout,
        })
    };
    // Whether the user still operates the display: a short wait times out.
    let user_has_it = || -> Result<bool> {
        let r = client.call(
            "wait_for_control",
            json!({ "display_id": display_id, "timeout_seconds": 1 }),
        )?;
        Ok(r.get("control") == "user")
    };

    let r = client.call("request_control", ask(5))?;
    check(
        "without a Takeover, request_control answers at once: control agent",
        r.get("control") == "agent" && card().is_none(),
        r.text(),
    );

    takeover(true)?;
    let r = client.call("request_control", ask(1))?;
    check(
        "during a Takeover, an unanswered request times out with the request still open",
        r.get("control") == "user" && r.text().contains("stays open"),
        r.text(),
    );
    sleep_ms(300);
    let shown = card();
    check(
        "the request shows the card",
        shown.is_some(),
        json!(shown).to_string(),
    );
    if let Some(s) = shown {
        let file = capture("requestcontrol", [s.x, s.y, s.width, s.height])?;
        println!("     card: {}", file.display());
    }

    // Keep Control while the agent waits.
    let waiting = client.begin("request_control", ask(20))?;
    sleep_ms(500);
    show("card-keep-control")?;
    let r = waiting.wait()?;
    check(
        "Keep Control tells the waiting agent the user kept it",
        r.get("control") == "user" && r.get("kept") == true,
        r.text(),
    );
    sleep_ms(300);
    check("Keep Control puts the card away", card().is_none(), "");
    check("Keep Control leaves the Takeover", user_has_it()?, "");

    // Let … Continue while the agent waits.
    let waiting = client.begin("request_control", ask(20))?;
    sleep_ms(500);
    show("card-continue")?;
    let r = waiting.wait()?;
    check(
        "Let … Continue hands the display back to the waiting agent",
        r.get("control") == "agent",
        r.text(),
    );
    sleep_ms(300);
    check("Let … Continue puts the card away", card().is_none(), "");
    check("the agent has the display", !user_has_it()?, "");

    // Withdrawn.
    takeover(true)?;
    client.call("request_control", ask(1))?;
    let r = client.call(
        "request_control",
        json!({ "display_id": display_id, "withdraw": true }),
    )?;
    sleep_ms(300);
    check(
        "a withdrawn request puts the card away and leaves the Takeover",
        r.get("withdrawn") == true && card().is_none() && user_has_it()?,
        r.text(),
    );
    takeover(false)?;
    Ok(())
}
