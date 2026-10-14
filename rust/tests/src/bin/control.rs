//! Control state against the running debug app: Takeover refusal and
//! wait_for_control. Needs no Screen Recording or Accessibility grant.
//!
//!     cargo e2e control

use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;
use sidescreen_tests::{Control, McpClient, Value, check, json, now_ms, run, sleep_ms};

/// A second control connection standing in for the user's Viewer.
fn debug(method: &str, params: Value) -> Result<Value> {
    Control::connect()?.call(method, params)
}

fn takeover(display_id: u64, active: bool) -> Result<Value> {
    debug(
        "debugTakeover",
        json!({ "displayId": display_id, "active": active }),
    )
}

fn main() -> ExitCode {
    run(|| {
        let session = format!("control-{}", now_ms());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        let created = client.call("create_display", json!({ "title": "Control test" }))?;
        let display_id = created.display_id().unwrap_or(0);

        takeover(display_id, true)?;
        let refused = client.call(
            "computer",
            json!({ "display_id": display_id, "action": "left_click", "coordinate": [100, 100] }),
        )?;
        check(
            "input refused during Takeover",
            refused.is_error && refused.first_text().starts_with("takeover"),
            refused.first_text(),
        );

        let timed_out = client.call(
            "wait_for_control",
            json!({ "display_id": display_id, "timeout_seconds": 1 }),
        )?;
        check(
            "wait_for_control times out",
            timed_out.get("control") == "user",
            "",
        );

        let started = Instant::now();
        let waiting = client.begin(
            "wait_for_control",
            json!({ "display_id": display_id, "timeout_seconds": 10 }),
        )?;
        sleep_ms(800);
        takeover(display_id, false)?;
        let returned = waiting.wait_printed()?;
        check(
            "wait_for_control returns on hand-back",
            returned.get("control") == "agent",
            format!("{} ms", started.elapsed().as_millis()),
        );

        let immediate = client.call("wait_for_control", json!({ "display_id": display_id }))?;
        check(
            "wait_for_control returns at once with control",
            immediate.get("control") == "agent",
            "",
        );

        // A Help Request holds the agent until the user takes over and hands back.
        let help = client.begin(
            "request_help",
            json!({
                "display_id": display_id,
                "reason": "Sign in to the test site",
                "timeout_seconds": 10,
            }),
        )?;
        sleep_ms(500);
        let still_waiting = client.call(
            "wait_for_control",
            json!({ "display_id": display_id, "timeout_seconds": 1 }),
        )?;
        check(
            "Help Request holds the agent before any Takeover",
            still_waiting.get("control") == "user",
            "",
        );
        takeover(display_id, true)?;
        sleep_ms(300);
        takeover(display_id, false)?;
        let helped = help.wait_printed()?;
        check(
            "request_help returns when the Takeover ends",
            helped.get("control") == "agent",
            "",
        );

        // Nobody answers: the agent hears the request is still open, and can take it back.
        let unanswered = client.call(
            "request_help",
            json!({ "display_id": display_id, "reason": "Enter the code", "timeout_seconds": 1 }),
        )?;
        let note = unanswered.get("note").as_str().unwrap_or("");
        check(
            "an unanswered Help Request says it stays open",
            note.contains("request stays open"),
            note,
        );
        let withdrawn = client.call(
            "request_help",
            json!({ "display_id": display_id, "withdraw": true }),
        )?;
        check(
            "request_help withdraw takes the request back",
            withdrawn.get("withdrawn") == true,
            withdrawn.structured_content.to_string(),
        );
        let after = client.call(
            "wait_for_control",
            json!({ "display_id": display_id, "timeout_seconds": 1 }),
        )?;
        check(
            "after a withdraw the agent has control",
            after.get("control") == "agent",
            after.structured_content.to_string(),
        );

        let no_window = client.call(
            "get_window_state",
            json!({ "display_id": display_id, "window_id": 1 }),
        )?;
        check(
            "get_window_state rejects a window not on the display",
            no_window.first_text().starts_with("not_found"),
            "",
        );

        let unknown = client.call(
            "set_disposition",
            json!({ "display_id": display_id, "window_id": 1, "disposition": "keep" }),
        )?;
        check(
            "set_disposition rejects a window not on the display",
            unknown.is_error,
            "",
        );

        takeover(display_id, true)?;
        let waiting = client.begin(
            "wait_for_control",
            json!({ "display_id": display_id, "timeout_seconds": 10 }),
        )?;
        sleep_ms(500);
        client.call("end_display", json!({ "display_id": display_id }))?;
        let ended = waiting.wait_printed()?;
        check(
            "wait_for_control returns when the display ends",
            ended.get("control") == "none",
            "",
        );

        let after_end = client.call(
            "set_disposition",
            json!({ "display_id": display_id, "window_id": 1, "disposition": "close" }),
        )?;
        check(
            "an ended display refuses the agent",
            after_end.first_text().starts_with("ended"),
            "",
        );
        Ok(())
    })
}
