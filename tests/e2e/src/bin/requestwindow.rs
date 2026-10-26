//! An agent asks for a window it didn't start through launch_app: the user
//! answers a Window Request with one click. An agent opened UTM with
//! `open -g -a UTM` and couldn't place its window, so it quit the VM to
//! start UTM again through launch_app. Here Field opens the same way, and
//! the agent names it by app. The test answers the request as the card's
//! buttons do: Hand Over once, Not Now once.
//!
//! Needs Field.app, which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e requestwindow
//!
//! Makes 1 display. Field's window shows on your screen until handed over.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, check, instance, json, kill_ours, our_binary, out, run, sh_lenient,
    sleep_ms, start_test_app, stop_test_app,
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
        let result = test();
        kill_ours(our_binary("Field"));
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

fn test() -> Result<()> {
    let client = McpClient::start(&[])?;
    kill_ours(our_binary("Field"));
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    // Started outside Agent Screen, as by a script.
    sh_lenient(
        "/usr/bin/open",
        &["-g", "-n", out().join("Field.app").to_str().unwrap()],
    );
    sleep_ms(2500);

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
    check("the window stays on the user's screen", !on(&client)?, "");

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
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
