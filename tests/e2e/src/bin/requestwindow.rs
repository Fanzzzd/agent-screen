//! An agent asks for a window it didn't start through launch_app: the user
//! answers a Window Request with one click. An agent opened UTM with
//! `open -g -a UTM` and couldn't place its window, so it quit the VM to
//! start UTM again through launch_app. Here a separate stage session starts
//! Field, and the target session names it by app. The test answers the request as the card's
//! buttons do: Hand Over once, Not Now once.
//!
//! Needs Field.app, which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e requestwindow
//!
//! Makes 2 displays. Field stays on the stage until handed over.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, check, field_app, instance, json, open_user_stage, out, run, sh_lenient,
    sign_step, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if !out().join("Field.app").exists() {
        eprintln!("no Field.app; run `cargo e2e ime` once to make it");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        field_app()?;
        let mut pid = None;
        let result = test(&mut pid);
        if let Some(pid) = pid {
            sh_lenient("/bin/kill", &["-TERM", &format!("{pid}")]);
        }
        stop_test_app();
        result
    })
}

/// Answer the display's Window Request once it shows, as the card's
/// button would.
fn answer_soon(display: u64, give: bool) -> std::thread::JoinHandle<Result<serde_json::Value>> {
    std::thread::spawn(move || {
        sleep_ms(800);
        Control::connect()?.call(
            "debugAnswerWindow",
            json!({ "displayId": display, "give": give }),
        )
    })
}

fn test(pid: &mut Option<u64>) -> Result<()> {
    let stage = Control::connect()?;
    sign_step("opening the user stand-in stage / 打开模拟用户显示器");
    let stage_id = open_user_stage(&stage)?;
    let field = stage.call(
        "launchApp",
        json!({
            "displayId": stage_id, "app": out().join("Field.app"),
            "arguments": [format!("--requestwindow-{}", std::process::id())],
            "environment": { "FIELD_DISPLAY": stage_id.to_string(), "FIELD_LIFETIME_MS": "60000" }
        }),
    )?;
    *pid = field["pid"].as_u64();
    pid.context("Field's starting pid")?;
    let window = field["window"]["windowId"]
        .as_u64()
        .context("Field's window")?;
    sleep_ms(3500);
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    // The target session did not launch Field. It must ask for this
    // stage window through the same request path as a physical user window.
    sign_step("declining then granting the Window Request / 拒绝后同意窗口请求");
    // Not Now: the window stays where it is.
    let answer = answer_soon(id, false);
    let refused = client.tool("place_window", json!({ "display_id": id, "app": "Field" }))?;
    let answered = answer.join().expect("answer thread");
    println!("     answer: {answered:?}");
    check(
        "a request the user turns down says so",
        refused.is_error && refused.text().contains("declined"),
        refused.text(),
    );
    let on = |client: &McpClient| -> Result<bool> {
        Ok(client
            .call("list_windows", json!({ "display_id": id }))?
            .windows()
            .unwrap_or_default()
            .iter()
            .any(|w| w.app == "Field"))
    };
    let kept = stage.call("listWindows", json!({ "displayId": stage_id }))?;
    check(
        "the declined window stays on the user stand-in stage",
        !on(&client)?
            && kept["windows"]
                .as_array()
                .is_some_and(|ws| ws.iter().any(|w| w["windowId"] == window)),
        kept.to_string(),
    );

    // Hand Over: the window comes, as a Handover.
    let answer = answer_soon(id, true);
    let placed = client.tool("place_window", json!({ "display_id": id, "app": "Field" }))?;
    let answered = answer.join().expect("answer thread");
    println!("     answer: {answered:?}");
    check(
        "a request the user grants places the window",
        !placed.is_error,
        placed.text(),
    );
    sleep_ms(800);
    let listed = client.call("list_windows", json!({ "display_id": id }))?;
    check(
        "the window is on the display, handed over",
        listed.text().contains("\"handedOver\""),
        listed.text(),
    );
    if let Some(pid) = pid.take() {
        sh_lenient("/bin/kill", &["-TERM", &pid.to_string()]);
    }
    sleep_ms(300);
    client.call("end_display", json!({ "display_id": id }))?;
    stage.call("endDisplay", json!({ "displayId": stage_id }))?;
    client.close();
    Ok(())
}
