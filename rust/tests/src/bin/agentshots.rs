//! Screenshots agents get: a display wider than the image budget comes back
//! scaled down whole, not cropped (issue #13), and a zoom over the whole
//! screenshot, at its reported size, shows the same picture.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e agentshots

use std::process::ExitCode;

use anyhow::{Context, Result};
use base64::Engine;
use agent_screen_tests::{
    Control, check, instance, json, probe, quit_app, run, running, skip, sleep_ms, start_test_app,
    stop_test_app,
};

const SAME: i64 = 12;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running, and may be the user's");
    }
    run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let result = test();
        quit_app("Calculator");
        stop_test_app();
        result
    })
}

/// Write a screenshot reply's image to the capture folder.
fn save(reply: &serde_json::Value, name: &str) -> Result<String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(reply["data"].as_str().context("image data")?)?;
    let path = agent_screen_tests::out().join(format!("{name}.png"));
    std::fs::write(&path, data)?;
    Ok(path.to_string_lossy().into_owned())
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "shots", "title": "Shots", "width": 1600, "height": 1000 }),
    )?;
    let id = display["displayId"].clone();
    control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?;
    sleep_ms(1500);
    let shot = control.call("screenshot", json!({ "displayId": id, "format": "png" }))?;
    let (width, height) = (
        shot["width"].as_f64().context("width")?,
        shot["height"].as_f64().context("height")?,
    );
    let whole = control.call(
        "screenshot",
        json!({ "displayId": id, "format": "png", "region": [0, 0, width, height] }),
    );
    check(
        "a zoom over the whole screenshot, at its reported size, works",
        whole.is_ok(),
        format!("{width}×{height}: {:?}", whole.as_ref().err()),
    );
    let whole = whole?;
    let a = save(&shot, "agentshots-screenshot")?;
    let b = save(&whole, "agentshots-zoom")?;
    let difference: i64 = probe(&["differ", &a, &b])?.parse().context("differ")?;
    check(
        "the screenshot is the whole display scaled down, as the zoom shows it",
        difference < SAME,
        format!("{width}×{height}, mean difference {difference}"),
    );
    Ok(())
}
