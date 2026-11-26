//! Auto and YOLO take a user window by the rules of ADR 0025. On the user
//! stand-in stage, Field copies play the user's windows; `debugActivity`
//! plays what the user is using, so the script sends no input:
//!
//! - Auto, with the window In Use: `place_window` waits, the display's line
//!   names the window, the icon counts nothing, and the window moves once
//!   the override stops naming it, as a Handover with `authority` `mode`.
//! - Take Back makes it a Kept Window: the next call opens a card.
//! - A Protected App's window opens a card.
//! - YOLO takes a window In Use at once.
//! - YOLO leaves a window another live session holds.
//! - The display's end returns the mode's Handovers to the stage.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e autowindow
//!
//! Makes 2 displays: the stage and the agent's. Writes only the e2e
//! instance's `autonomy-e2e.json`, and puts it back.

use std::process::ExitCode;
use std::time::Instant;

use agent_screen_tests::{
    Control, McpClient, ToolResult, Value, check, field_app, instance, json, open_user_stage,
    put_back_autonomy, run, set_autonomy_file, sh_lenient, sign_step, sleep_ms, start_test_app,
    stop_test_app, tool_app_as,
};
use anyhow::{Context, Result};

/// The Field copy the e2e file lists as a Protected App.
const GUARD_BUNDLE: &str = "app.agentscreen.e2e.autoguard";

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        // A fresh instance reads the file at launch.
        stop_test_app();
        let before = set_autonomy_file(&json!({ "mode": "ask", "protected": [GUARD_BUNDLE] }))?;
        let mut pids = Vec::new();
        let result = start_test_app(&[("AGENTSCREEN_LANG", "en")]).and_then(|()| test(&mut pids));
        if let Ok(control) = Control::connect() {
            let _ = control.call("debugActivity", json!({ "clear": true }));
            let _ = control.call("debugShow", json!({ "surface": "autonomy-ask" }));
        }
        for pid in pids {
            sh_lenient("/bin/kill", &["-TERM", &pid.to_string()]);
        }
        stop_test_app();
        put_back_autonomy(before);
        result
    })
}

/// Launch a Field app on the stage; the window and its pid.
fn launch(
    stage: &Control,
    stage_id: u64,
    app: &std::path::Path,
    pids: &mut Vec<u64>,
) -> Result<u64> {
    let field = stage.call(
        "launchApp",
        json!({
            "displayId": stage_id, "app": app,
            "arguments": [format!("--autowindow-{}", std::process::id())],
            "environment": { "FIELD_DISPLAY": stage_id.to_string(), "FIELD_LIFETIME_MS": "150000" }
        }),
    )?;
    pids.push(field["pid"].as_u64().context("its pid")?);
    field["window"]["windowId"].as_u64().context("its window")
}

/// Make `window` the user's, as a window of theirs on their screen is.
fn give_to_user(stage: &Control, stage_id: u64, window: u64) -> Result<()> {
    stage.call(
        "debugUserWindow",
        json!({ "displayId": stage_id, "windowId": window }),
    )?;
    Ok(())
}

/// Answer the card on display `id` once it shows, as its button would.
fn answer_soon(id: u64, give: bool) -> std::thread::JoinHandle<Result<Value>> {
    std::thread::spawn(move || {
        sleep_ms(800);
        Control::connect()?.call(
            "debugAnswerWindow",
            json!({ "displayId": id, "give": give }),
        )
    })
}

fn on_stage(stage: &Control, stage_id: u64, window: u64) -> Result<bool> {
    let listed = stage.call("listWindows", json!({ "displayId": stage_id }))?;
    Ok(listed["windows"]
        .as_array()
        .is_some_and(|ws| ws.iter().any(|w| w["windowId"] == window)))
}

fn mode(control: &Control, surface: &str) -> Result<()> {
    control.call("debugShow", json!({ "surface": surface }))?;
    Ok(())
}

fn by_mode(placed: &ToolResult) -> bool {
    !placed.is_error && placed.get("authority") == "mode"
}

fn test(pids: &mut Vec<u64>) -> Result<()> {
    let stage = Control::connect()?;
    let field = field_app()?;
    let guard = tool_app_as("field", "AutoGuard", "autoguard")?;
    let busy = tool_app_as("field", "AutoBusy", "autobusy")?;
    let held = tool_app_as("field", "AutoHeld", "autoheld")?;
    sign_step("opening the user stand-in stage / 打开模拟用户显示器");
    let stage_id = open_user_stage(&stage)?;
    let window = launch(&stage, stage_id, &field, pids)?;
    let guard_window = launch(&stage, stage_id, &guard, pids)?;
    let busy_window = launch(&stage, stage_id, &busy, pids)?;
    let held_window = launch(&stage, stage_id, &held, pids)?;
    sleep_ms(3000);
    // AutoHeld stays the stage session's: a live holder's window.
    for w in [window, guard_window, busy_window] {
        give_to_user(&stage, stage_id, w)?;
    }
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    let state = stage.call("debugAutonomy", json!({}))?;
    check(
        "the e2e file's Protected App is in the list",
        state["protected"]
            .as_array()
            .is_some_and(|l| l.iter().any(|b| b == GUARD_BUNDLE)),
        state["protected"].to_string(),
    );

    sign_step("Auto: a window in use waits / 自动：在用的窗口等待");
    stage.call(
        "debugActivity",
        json!({ "idleMs": 0, "activeWindow": window }),
    )?;
    mode(&stage, "autonomy-auto")?;
    sleep_ms(600);
    let waiting = client.begin("place_window", json!({ "display_id": id, "app": "Field" }))?;
    sleep_ms(1500);
    let state = stage.call("debugAutonomy", json!({}))?;
    let in_use = state["tracker"]["windows"].as_array().is_some_and(|ws| {
        ws.iter()
            .any(|w| w["windowId"] == window && w["inUse"] == true)
    });
    check(
        "the window the override names is In Use",
        in_use,
        state["tracker"].to_string(),
    );
    let line = stage.call(
        "debugShow",
        json!({ "surface": "window-wait", "displayId": id }),
    )?;
    check(
        "the display's line names the waiting window",
        line["wait"]["windowId"] == window && line["wait"]["preview"] == false,
        line.to_string(),
    );
    let icon = stage.call("debugIcon", json!({}))?;
    check(
        "the waiting request opens no card: the icon counts nothing",
        icon["askingCount"] == 0,
        icon.to_string(),
    );
    let cleared = Instant::now();
    stage.call("debugActivity", json!({ "clear": true }))?;
    stage.call("debugActivity", json!({ "idleMs": 9000 }))?;
    let placed = waiting.wait()?;
    let ms = cleared.elapsed().as_millis();
    check(
        "once unused, the window moves as the mode's Handover",
        by_mode(&placed),
        placed.text(),
    );
    check(
        "it moves soon after the user leaves it (2 s at most)",
        ms <= 2000,
        format!("{ms} ms"),
    );

    sign_step("Take Back keeps the window / 取回后窗口留下");
    sleep_ms(800);
    stage.call(
        "debugTakeBack",
        json!({ "displayId": id, "windowId": window }),
    )?;
    let state = stage.call("debugAutonomy", json!({}))?;
    check(
        "Take Back makes it a Kept Window",
        state["kept"]
            .as_array()
            .is_some_and(|k| k.iter().any(|w| *w == window)),
        state["kept"].to_string(),
    );
    let answer = answer_soon(id, true);
    let placed = client.tool("place_window", json!({ "display_id": id, "app": "Field" }))?;
    let answered = answer.join().expect("answer thread")?;
    check(
        "a Kept Window opens a card in Auto",
        answered["answered"] == true && !placed.is_error && placed.get("authority") != "mode",
        format!("{answered} {}", placed.text()),
    );

    sign_step("a Protected App opens a card / 受保护的 app 弹出卡片");
    let answer = answer_soon(id, false);
    let refused = client.tool(
        "place_window",
        json!({ "display_id": id, "app": "AutoGuard" }),
    )?;
    let answered = answer.join().expect("answer thread")?;
    check(
        "a Protected App's window opens a card in Auto",
        answered["answered"] == true && refused.is_error && refused.text().contains("declined"),
        format!("{answered} {}", refused.text()),
    );

    sign_step("YOLO takes a window in use / YOLO 立即取用");
    mode(&stage, "autonomy-yolo")?;
    mode(&stage, "autonomy-yolo-confirm")?;
    stage.call(
        "debugActivity",
        json!({ "idleMs": 0, "activeWindow": busy_window }),
    )?;
    let state = stage.call("debugAutonomy", json!({}))?;
    check(
        "YOLO is on",
        state["mode"] == "yolo",
        state["mode"].to_string(),
    );
    let started = Instant::now();
    let placed = client.tool(
        "place_window",
        json!({ "display_id": id, "app": "AutoBusy" }),
    )?;
    check(
        "YOLO takes a window In Use at once",
        by_mode(&placed) && started.elapsed().as_millis() < 5000,
        format!("{} ms {}", started.elapsed().as_millis(), placed.text()),
    );

    sign_step("a live session's window stays / 其他会话的窗口不动");
    // The stage session's own window: YOLO doesn't take it (ADR 0017).
    let refused = client.tool(
        "place_window",
        json!({ "display_id": id, "app": "AutoHeld" }),
    )?;
    check(
        "YOLO doesn't take a window another live session holds",
        refused.is_error && refused.get("authority") != "mode",
        refused.text(),
    );
    check(
        "it stays with its holder",
        on_stage(&stage, stage_id, held_window)?,
        "",
    );
    mode(&stage, "autonomy-ask")?;
    stage.call("debugActivity", json!({ "clear": true }))?;

    sign_step("ending the display returns the windows / 结束显示器后窗口归还");
    client.call("end_display", json!({ "display_id": id }))?;
    sleep_ms(2500);
    check(
        "the display's end returns the windows the mode and the user handed over",
        on_stage(&stage, stage_id, window)? && on_stage(&stage, stage_id, busy_window)?,
        "",
    );
    stage.call("endDisplay", json!({ "displayId": stage_id }))?;
    client.close();
    Ok(())
}
