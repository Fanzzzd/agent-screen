//! Conversation hand-back, pause and window authorization through MCP.
//! Uses one empty Agent Display; does not launch apps or move user windows.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e conversation

use std::process::ExitCode;

use agent_screen_tests::{Control, McpClient, check, instance, json, run, start_test_app};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("set AGENTSCREEN_INSTANCE");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[])?;
        let session = format!("conversation-{}", std::process::id());
        let other_session = format!("conversation-other-{}", std::process::id());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        let other = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &other_session)])?;
        let display_id = client
            .call(
                "create_display",
                json!({"title": "Conversation authorization"}),
            )?
            .display_id()
            .context("create_display gave no display_id")?;
        let result = test(&client, &other, display_id);
        let _ = client.call("end_display", json!({"display_id": display_id}));
        client.close();
        other.close();
        result
    })
}

fn test(client: &McpClient, other: &McpClient, display_id: u64) -> Result<()> {
    let status = || {
        client.call(
            "window_access",
            json!({"display_id": display_id, "action": "status"}),
        )
    };
    let grant = || {
        client.call(
            "window_access",
            json!({
                "display_id": display_id,
                "action": "grant",
                "all": true,
                "user_instruction": "You may take the windows you need for this session.",
            }),
        )
    };
    let pause = || {
        client.call(
            "request_control",
            json!({
                "display_id": display_id,
                "control": "user",
                "user_instruction": "Pause while I check the result.",
            }),
        )
    };
    let resume = || {
        client.call(
            "request_control",
            json!({
                "display_id": display_id,
                "user_instruction": "I'm done, continue.",
            }),
        )
    };

    let r = status()?;
    check(
        "a new session has no window authorization",
        !r.is_error && r.get("scope") == "none" && r.get("paused") == false,
        r.text(),
    );
    let r = client.call(
        "window_access",
        json!({"display_id": display_id, "action": "grant", "all": true}),
    )?;
    check("grant needs the user's instruction", r.is_error, r.text());
    let r = client.call(
        "request_control",
        json!({"display_id": display_id, "control": "user"}),
    )?;
    check("pause needs the user's instruction", r.is_error, r.text());
    let r = client.call(
        "window_access",
        json!({
            "display_id": display_id,
            "action": "grant",
            "all": true,
            "user_instruction": "   ",
        }),
    )?;
    check("blank words do not authorize access", r.is_error, r.text());
    let r = status()?;
    check(
        "rejected instructions leave access and control unchanged",
        !r.is_error && r.get("scope") == "none" && r.get("paused") == false,
        r.text(),
    );

    let r = grant()?;
    check(
        "a conversation instruction grants access without a card answer",
        !r.is_error && r.get("scope") == "all" && r.get("changed") == true,
        r.text(),
    );
    let r = other.call(
        "window_access",
        json!({"display_id": display_id, "action": "status"}),
    )?;
    check(
        "another session cannot inspect the grant",
        r.is_error,
        r.text(),
    );
    let r = other.call(
        "window_access",
        json!({"display_id": display_id, "action": "revoke"}),
    )?;
    check(
        "another session cannot revoke the grant",
        r.is_error,
        r.text(),
    );
    let r = other.call(
        "request_control",
        json!({
            "display_id": display_id,
            "control": "user",
            "user_instruction": "Pause.",
        }),
    )?;
    check(
        "another session cannot pause the display",
        r.is_error,
        r.text(),
    );

    let r = pause()?;
    check(
        "chat can begin a Takeover without a card answer",
        !r.is_error && r.get("control") == "user",
        r.text(),
    );
    let r = status()?;
    check(
        "a Takeover pauses rather than clears the grant",
        !r.is_error && r.get("scope") == "all" && r.get("paused") == true,
        r.text(),
    );
    let r = client.call(
        "wait_for_control",
        json!({"display_id": display_id, "timeout_seconds": 1}),
    )?;
    check(
        "waiting does not hand control back",
        r.get("control") == "user",
        r.text(),
    );
    let r = resume()?;
    check(
        "an explicit chat hand-back ends the Takeover",
        !r.is_error && r.get("control") == "agent",
        r.text(),
    );
    let r = status()?;
    check(
        "hand-back resumes the existing grant",
        !r.is_error && r.get("scope") == "all" && r.get("paused") == false,
        r.text(),
    );

    pause()?;
    let r = client.call(
        "window_access",
        json!({
            "display_id": display_id,
            "action": "revoke",
            "user_instruction": "Cancel the window authorization.",
        }),
    )?;
    check(
        "authorization can be revoked during a Takeover",
        !r.is_error && r.get("scope") == "none" && r.get("paused") == true,
        r.text(),
    );
    resume()?;
    let r = status()?;
    check(
        "hand-back does not resurrect revoked access",
        !r.is_error && r.get("scope") == "none" && r.get("paused") == false,
        r.text(),
    );
    let r = client.call(
        "window_access",
        json!({
            "display_id": display_id,
            "action": "return",
            "all": true,
            "user_instruction": "Return every window I handed over.",
        }),
    )?;
    check(
        "returning Handovers on an empty display returns nothing",
        !r.is_error && r.get("returned") == &json!([]),
        r.text(),
    );

    grant()?;
    let control = Control::connect()?;
    control.call(
        "debugShow",
        json!({"surface": "access-revoke", "displayId": display_id}),
    )?;
    let r = status()?;
    check(
        "the user's revoke action works without another chat instruction",
        !r.is_error && r.get("scope") == "none",
        r.text(),
    );

    let r = grant()?;
    check(
        "access is granted before ending the display",
        !r.is_error && r.get("scope") == "all",
        r.text(),
    );
    let r = client.call("end_display", json!({"display_id": display_id}))?;
    check("the authorized display ends", !r.is_error, r.text());
    let r = grant()?;
    check(
        "an ended display cannot regain access",
        r.is_error,
        r.text(),
    );
    Ok(())
}
