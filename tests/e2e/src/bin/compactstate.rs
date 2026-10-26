//! get_window_state with max_depth and act_on_element with result: changes
//! or none send a fraction of the full tree (#30). On an Electron mail app
//! each call sent about 200 elements, over 6,000 tokens, and every press
//! sent them again. Here Calculator stands in.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e compactstate
//!
//! Makes 1 display. Skips while Calculator runs, since it may be yours;
//! quits Calculator after.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, ToolResult, check, instance, json, quit_app, run, running, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running and may be yours");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        quit_app("Calculator");
        stop_test_app();
        result
    })
}

fn button(state: &ToolResult, label: &str) -> Option<u64> {
    Regex::new(&format!(r#"\[(\d+)\] button "{label}""#))
        .ok()?
        .captures(state.first_text())?[1]
        .parse()
        .ok()
}

fn has_image(result: &ToolResult) -> bool {
    result.image().is_some()
}

fn test() -> Result<()> {
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    client.call(
        "launch_app",
        json!({ "display_id": id, "app": "Calculator" }),
    )?;
    sleep_ms(1000);
    let window = client
        .call("list_windows", json!({ "display_id": id }))?
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.app == "Calculator")
        .context("no Calculator window on the display")?
        .window_id;
    let at = |extra: serde_json::Value| {
        let mut args = json!({ "display_id": id, "window_id": window });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };

    let full = client.tool("get_window_state", at(json!({})))?;
    let full_len = full.first_text().len();
    println!("     full tree: {full_len} bytes");
    let shallow = client.tool("get_window_state", at(json!({ "max_depth": 0 })))?;
    let shallow_len = shallow.first_text().len();
    println!(
        "     max_depth 0: {shallow_len} bytes\n{}",
        shallow.first_text()
    );
    check(
        "max_depth leaves out deeper elements and says how many",
        shallow_len * 2 < full_len && shallow.first_text().contains("more inside"),
        format!("{shallow_len} of {full_len} bytes"),
    );

    // The full tree again, so changes compare with it.
    let state = client.tool("get_window_state", at(json!({})))?;
    let seven = button(&state, "7").context("no button 7")?;
    let pressed = client.tool(
        "act_on_element",
        at(json!({ "element": seven, "action": "press", "result": "changes" })),
    )?;
    let changes_len = pressed.first_text().len();
    println!(
        "     press 7, changes: {changes_len} bytes\n{}",
        pressed.first_text()
    );
    check(
        "a press with result changes lists only the changed elements",
        !pressed.is_error
            && changes_len * 4 < full_len
            && pressed.first_text().contains("~ ")
            && pressed.first_text().contains("7"),
        pressed.first_text(),
    );
    check("and still shows the window", has_image(&pressed), "");

    let eight = button(&state, "8").context("no button 8")?;
    let quiet = client.tool(
        "act_on_element",
        at(json!({ "element": eight, "action": "press", "result": "none" })),
    )?;
    println!("     press 8, none: {}", quiet.first_text());
    check(
        "a press with result none says only that it is done",
        quiet.first_text() == "Done." && !has_image(&quiet),
        quiet.first_text(),
    );

    // The default stays the whole state.
    let default = client.tool(
        "act_on_element",
        at(json!({ "element": seven, "action": "press" })),
    )?;
    check(
        "a press without result returns the whole window state",
        default.first_text().contains("elements.") && default.first_text().len() * 2 > full_len,
        format!("{} bytes", default.first_text().len()),
    );

    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
