//! Every answer to a Help Request, as the agent's request_help hears it
//! (ADR 0015). The user answers from the Help Card or its notification,
//! through the debug hooks the card's buttons and the notification's
//! actions call: Continue, Reply with their words, Give a Window (their
//! own Handover of a Field window from a user stand-in stage), and Take
//! Over with its hand-back. Also: an empty reply changes nothing, the
//! agent's own withdrawal never reads as the user's hand-back, an answer
//! that came after the call timed out reaches the next wait_for_control,
//! Later puts off one request and a new one with the same reason shows the
//! card again, and the user's answer in chat closes the request. Captures
//! the card and its reply field under target/e2e. The card shows on the
//! user's screen. The hooks open the reply field without taking the
//! keyboard, so the user's typing stays in their app.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e helpanswers
//!
//! Makes 2 displays: the stage and the agent's display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Frame, McpClient, capture, check, debug_pid, field_app, instance, json, now_ms,
    open_user_stage, out, run, sh_lenient, sleep_ms, start_test_app, windows_of,
};
use anyhow::{Context, Result};
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("set AGENTSCREEN_INSTANCE");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[])?;
        field_app()?;
        let pid = debug_pid().context("no test app")?;
        let stage = Control::connect()?;
        let stage_id = open_user_stage(&stage)?;
        let session = format!("helpanswers-{}", now_ms());
        let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &session)])?;
        let display_id = client
            .call("create_display", json!({ "title": "Book a flight" }))?
            .display_id()
            .context("create_display gave no display_id")?;
        let mut field = None;
        let result = test(&stage, stage_id, &client, pid, display_id, &mut field);
        if let Some(field) = field {
            sh_lenient("/bin/kill", &["-TERM", &field.to_string()]);
        }
        let _ = client.call("end_display", json!({ "display_id": display_id }));
        let _ = stage.call("endDisplay", json!({ "displayId": stage_id }));
        client.close();
        result
    })
}

fn test(
    stage: &Control,
    stage_id: u64,
    client: &McpClient,
    pid: u32,
    display_id: u64,
    field_pid: &mut Option<u64>,
) -> Result<()> {
    let answer = |answer: &str, text: &str, via: &str| -> Result<Value> {
        stage.call(
            "debugAnswerHelp",
            json!({ "displayId": display_id, "answer": answer, "text": text, "via": via }),
        )
    };
    let show = |surface: &str| {
        stage.call(
            "debugShow",
            json!({ "surface": surface, "displayId": display_id }),
        )
    };
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };
    let wait_card = |visible: bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let frame = card();
            if frame.is_some() == visible || std::time::Instant::now() >= deadline {
                return frame;
            }
            sleep_ms(50);
        }
    };
    let ask = |timeout: u64| {
        json!({
            "display_id": display_id,
            "reason": "Sign in to United. It wants a code sent to your phone.",
            "timeout_seconds": timeout,
        })
    };
    let lang = std::env::var("AGENTSCREEN_LANG").unwrap_or_else(|_| "system".into());

    // Continue, from the card.
    let waiting = client.begin("request_help", ask(30))?;
    let shown = wait_card(true);
    check(
        "a Help Request shows the card",
        shown.is_some(),
        json!(shown).to_string(),
    );
    if let Some(s) = shown {
        let file = capture(
            &format!("helpanswers-{lang}-card"),
            [s.x, s.y, s.width, s.height],
        )?;
        println!("     card: {}", file.display());
    }
    let painted = stage.call("debugCard", json!({}))?;
    let width = painted["width"].as_f64().unwrap_or(0.0);
    let fits = |id: &str| -> bool {
        painted["buttons"][id].as_array().is_some_and(|r| {
            let (x, w) = (r[0].as_f64().unwrap_or(-1.0), r[2].as_f64().unwrap_or(0.0));
            x >= 13.5 && x + w <= width - 13.5 && w > 20.0
        })
    };
    let buttons = [
        "help-take-over",
        "help-continue",
        "help-reply",
        "help-give",
        "help-later",
    ];
    check(
        "every answer's button fits the card",
        buttons.iter().all(|b| fits(b)),
        painted["buttons"].to_string(),
    );
    answer("continue", "", "")?;
    let r = waiting.wait()?;
    check(
        "Continue tells the agent it may go on without a Takeover",
        r.get("control") == "agent"
            && r.get("answer") == "continue"
            && r.text().contains("without taking over"),
        r.text(),
    );
    check(
        "Continue puts the card away",
        wait_card(false).is_none(),
        "",
    );

    // Reply, typed into the card's field.
    let words = "Use the work account, then pick an aisle seat.";
    let waiting = client.begin("request_help", ask(30))?;
    wait_card(true);
    show("card-reply")?;
    sleep_ms(400);
    if let Some(s) = card() {
        let file = capture(
            &format!("helpanswers-{lang}-reply"),
            [s.x, s.y, s.width, s.height],
        )?;
        println!("     reply field: {}", file.display());
    }
    let painted = stage.call("debugCard", json!({}))?;
    check(
        "Reply opens the field with Send and Cancel",
        ["reply-field", "reply-send", "reply-cancel"]
            .iter()
            .all(|b| painted["buttons"].get(*b).is_some()),
        painted["buttons"].to_string(),
    );
    let empty = answer("reply", "   ", "")?;
    check(
        "an empty reply answers nothing",
        empty["answered"] == false && card().is_some(),
        empty.to_string(),
    );
    answer("reply", words, "")?;
    let r = waiting.wait()?;
    check(
        "a reply reaches the agent word for word, as the user's",
        r.get("answer") == "instructions"
            && r.get("user_reply") == words
            && r.text().contains(&format!("\\\"{words}\\\""))
            && r.get("control") == "agent",
        r.text(),
    );

    // Continue and Reply, from the notification's actions.
    let waiting = client.begin("request_help", ask(30))?;
    wait_card(true);
    answer("continue", "", "notification")?;
    let r = waiting.wait()?;
    check(
        "the notification's Continue answers the request",
        r.get("answer") == "continue",
        r.text(),
    );
    let chinese = "用工作账号登录，验证码 482913";
    let waiting = client.begin("request_help", ask(30))?;
    wait_card(true);
    answer("reply", chinese, "notification")?;
    let r = waiting.wait()?;
    check(
        "the notification's Reply reaches the agent word for word",
        r.get("answer") == "instructions" && r.get("user_reply") == chinese,
        r.text(),
    );

    // Give a Window: the user's own Handover of their Field window.
    let field = stage.call(
        "launchApp",
        json!({
            "displayId": stage_id, "app": out().join("Field.app"),
            "arguments": [format!("--helpanswers-{}", std::process::id())],
            "environment": { "FIELD_DISPLAY": stage_id.to_string(), "FIELD_LIFETIME_MS": "90000" }
        }),
    )?;
    *field_pid = field["pid"].as_u64();
    let window = field["window"]["windowId"]
        .as_u64()
        .context("Field's window")?;
    sleep_ms(1500);
    stage.call(
        "debugUserWindow",
        json!({ "displayId": stage_id, "windowId": window }),
    )?;
    let waiting = client.begin("request_help", ask(30))?;
    wait_card(true);
    stage.call(
        "debugHandOver",
        json!({ "displayId": display_id, "windowId": window }),
    )?;
    let r = waiting.wait()?;
    check(
        "the user's Handover answers the request and names the window",
        r.get("answer") == "handedOver"
            && r.get("window")["window_id"] == window
            && r.get("window")["app"] == "Field"
            && r.text().contains("on your display"),
        r.text(),
    );
    // The window list can lag the move a moment.
    let mut listed = client.call("list_windows", json!({ "display_id": display_id }))?;
    for _ in 0..15 {
        if listed.text().contains("\"handedOver\"") {
            break;
        }
        sleep_ms(200);
        listed = client.tool("list_windows", json!({ "display_id": display_id }))?;
    }
    check(
        "the window is on the agent's display, handed over",
        listed.text().contains("\"handedOver\""),
        listed.text(),
    );

    // Take Over: only the hand-back answers it.
    let waiting = client.begin("request_help", ask(30))?;
    wait_card(true);
    stage.call(
        "debugTakeover",
        json!({ "displayId": display_id, "active": true }),
    )?;
    check(
        "a Takeover puts the card away",
        wait_card(false).is_none(),
        "",
    );
    let refused = answer("continue", "", "")?;
    check(
        "during the Takeover Continue answers nothing",
        refused["answered"] == false,
        refused.to_string(),
    );
    stage.call(
        "debugTakeover",
        json!({ "displayId": display_id, "active": false }),
    )?;
    let r = waiting.wait()?;
    check(
        "the hand-back answers it: the user took over",
        r.get("answer") == "tookOver" && r.get("control") == "agent",
        r.text(),
    );

    // The agent's own withdrawal.
    let waiting = client.begin("request_help", ask(30))?;
    wait_card(true);
    // The withdrawal comes from the same session while its call waits.
    let withdrawn = client.tool(
        "request_help",
        json!({ "display_id": display_id, "withdraw": true }),
    );
    let r = waiting.wait()?;
    check(
        "a withdrawal never reads as the user's hand-back",
        r.get("withdrawn") == true && !r.text().contains("You have control"),
        format!(
            "{} / {}",
            r.text(),
            withdrawn.map(|w| w.text().to_string()).unwrap_or_default()
        ),
    );

    // An answer after the call timed out comes with the next wait.
    let r = client.call("request_help", ask(1))?;
    check(
        "an unanswered request times out and stays open",
        r.get("control") == "user" && r.text().contains("hasn't answered your Help Request"),
        r.text(),
    );
    answer("reply", "Back now. Go ahead.", "")?;
    let r = client.call(
        "wait_for_control",
        json!({ "display_id": display_id, "timeout_seconds": 5 }),
    )?;
    check(
        "the next wait_for_control hears the late reply",
        r.get("answer") == "instructions" && r.get("user_reply") == "Back now. Go ahead.",
        r.text(),
    );

    // Later puts off this request; the same reason again is a new one.
    client.call("request_help", ask(1))?;
    wait_card(true);
    answer("later", "", "")?;
    check("Later puts the card away", wait_card(false).is_none(), "");
    client.call("request_help", ask(1))?;
    check(
        "a new request with the same reason shows the card again",
        wait_card(true).is_some(),
        "",
    );

    // The user answers in chat; the agent relays their words.
    let r = client.call(
        "request_help",
        json!({ "display_id": display_id, "user_instruction": "I signed in on my phone, go on." }),
    )?;
    check(
        "the user's words in chat close the request",
        r.get("answeredIn") == "chat" && wait_card(false).is_none(),
        r.text(),
    );
    let r = client.call(
        "wait_for_control",
        json!({ "display_id": display_id, "timeout_seconds": 1 }),
    )?;
    check(
        "nothing waits for the user afterwards",
        r.get("control") == "agent",
        r.text(),
    );
    Ok(())
}
