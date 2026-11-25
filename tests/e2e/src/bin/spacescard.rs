//! The Spaces card, which tells the user once that every display shares
//! one macOS Space ("Displays have separate Spaces" off), shown with
//! `debugShow` in English and Chinese: it drops from the menu bar icon at
//! the Help Card's width, its two buttons fit it, Not Now hides it, and a
//! preview marks nothing as seen. It never opens System Settings. No
//! display; the card shows on your screen for a few seconds. Captures it
//! under target/e2e.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e spacescard

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Frame, Value, capture, check, debug_pid, instance, json, run, sleep_ms,
    start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let mut result = Ok(());
        for lang in ["en", "zh-Hans"] {
            stop_test_app();
            start_test_app(&[("AGENTSCREEN_LANG", lang)])?;
            let pid = debug_pid().context("no test app")?;
            let control = Control::connect()?;
            agent_screen_tests::countdown();
            result = result.and(test(&control, pid, lang));
            drop(control);
        }
        stop_test_app();
        result
    })
}

fn test(control: &Control, pid: u32, lang: &str) -> Result<()> {
    let card = || -> Option<Frame> {
        windows_of(pid)
            .into_iter()
            .find(|w| w.width == 320.0 && w.layer > 0)
    };
    let state = || -> Result<Value> { control.call("debugCard", json!({})) };
    let noticed = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/Agent Screen/spaces-noticed-e2e");
    let seen_before = noticed.exists();

    check(
        &format!("{lang}: no Spaces card before"),
        card().is_none(),
        "",
    );
    control.call("debugShow", json!({ "surface": "spaces-card" }))?;
    sleep_ms(600);
    let shown = card();
    let debug = state()?;
    check(
        &format!("{lang}: the card drops at the Help Card's width"),
        shown.is_some() && debug["spaces"]["shown"] == true,
        format!("{shown:?} {}", debug["spaces"]),
    );
    let width = debug["width"].as_f64().unwrap_or(0.0);
    let height = shown.as_ref().map_or(0.0, |c| c.height);
    for id in ["spaces-open", "spaces-later"] {
        let b = &debug["buttons"][id];
        let [x, y, w, h] = [0, 1, 2, 3].map(|i| b[i].as_f64().unwrap_or(-1.0));
        check(
            &format!("{lang}: {id} fits the card"),
            x >= 0.0 && w > 0.0 && x + w <= width && y + h <= height,
            format!("{b} in {width}×{height}"),
        );
    }
    if let Some(c) = &shown {
        println!(
            "  captured {}",
            capture(&format!("spacescard-{lang}"), [c.x, c.y, c.width, c.height])?.display()
        );
    }
    control.call("debugShow", json!({ "surface": "spaces-card-later" }))?;
    sleep_ms(500);
    let after = state()?;
    check(
        &format!("{lang}: Not Now hides the card, and the preview marks nothing seen"),
        card().is_none()
            && after["spaces"]["shown"] == false
            && after["spaces"]["wanted"] == false
            && noticed.exists() == seen_before,
        after["spaces"].to_string(),
    );
    Ok(())
}
