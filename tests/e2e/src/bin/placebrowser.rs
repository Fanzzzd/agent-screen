//! place_window and launch_app with a Chromium browser that already runs,
//! on a throwaway profile. place_window reported "the app rejected the
//! change" for a Dia window that had moved and resized (#167); Chrome
//! answers these frame writes without an error, and placesize covers an
//! app that answers with one. A running browser opened a file path as a tab
//! of its front window, which may be the user's, and launch_app said
//! existing_window or still_opening (#168): a browser now gets a window of
//! its own for a file, as for a URL.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e placebrowser
//!
//! Makes 1 display. Skips when Chrome is running with windows. Quits the
//! copy it started.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, ToolResult, check, fixture, json, out, quit_profile, run, running_headed, skip,
    sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let profile = out().join("placebrowser-profile");
        let result = test(&profile);
        quit_profile(&profile);
        stop_test_app();
        result
    })
}

/// The `window` frame place_window returned, as [x, y, width, height].
fn frame(result: &ToolResult) -> Option<[f64; 4]> {
    let w = result.get("window");
    Some([
        w["x"].as_f64()?,
        w["y"].as_f64()?,
        w["width"].as_f64()?,
        w["height"].as_f64()?,
    ])
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 2.0
}

fn test(profile: &std::path::Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(profile);
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    let launched = client.call(
        "launch_app",
        json!({ "display_id": id, "app": "Google Chrome", "arguments": [
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run", "--no-default-browser-check", "about:blank",
        ] }),
    )?;
    let wid = launched.get("window")["windowId"]
        .as_u64()
        .with_context(|| format!("no Chrome window: {}", launched.text()))?;
    sleep_ms(1000);
    let listed = |client: &McpClient, wid: u64| -> Result<Option<agent_screen_tests::Window>> {
        Ok(client
            .call("list_windows", json!({ "display_id": id }))?
            .windows()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.window_id == wid))
    };

    // The frame from #167, taller than the display: Chrome takes what fits.
    for asked in [
        [0.0, 30.0, 960.0, 810.0],
        [100.0, 120.0, 900.0, 500.0],
        [40.0, 60.0, 1100.0, 700.0],
    ] {
        let [x, y, width, height] = asked;
        let placed = client.call(
            "place_window",
            json!({ "display_id": id, "window_id": wid, "x": x, "y": y, "width": width, "height": height }),
        )?;
        let now = listed(&client, wid)?.context("Chrome left the display")?;
        let got = frame(&placed);
        check(
            &format!("place_window {asked:?} succeeds"),
            !placed.is_error,
            placed.text(),
        );
        check(
            "and returns the frame the window has",
            got.is_some_and(|[gx, gy, gw, gh]| {
                near(gx, now.x) && near(gy, now.y) && near(gw, now.width) && near(gh, now.height)
            }),
            format!("{got:?} against {now:?}"),
        );
        check(
            "which moved toward the one asked for",
            near(now.x, x) && near(now.width, width.min(1280.0 - x)),
            format!("{now:?}"),
        );
    }

    // A file path for the running browser: a window of its own on the
    // display, showing the file.
    let page = fixture("web/signup.html");
    let opened = client.call(
        "launch_app",
        json!({ "display_id": id, "app": "Google Chrome", "open": page.display().to_string() }),
    )?;
    let file_window = opened.get("window")["windowId"].as_u64();
    check(
        "launch_app with a file path in a running browser opens a new window",
        !opened.is_error && file_window.is_some_and(|w| w != wid),
        opened.text(),
    );
    if let Some(file_window) = file_window {
        sleep_ms(500);
        let shown = listed(&client, file_window)?;
        check(
            "the new window is on the display and shows the file",
            shown.as_ref().is_some_and(|w| w.title.contains("Sign")),
            format!("{shown:?}"),
        );
    }

    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
