//! place_window sets a window's frame on the display and returns the frame
//! the app ended with (#45). An agent checking a responsive layout resized
//! its window with System Events instead, and its display-relative
//! position put the window on the user's screen. Field holds to a 500×200
//! minimum here, as many apps do.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e placesize
//!
//! Makes 1 display. Nothing shows on your screen.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, ToolResult, check, field_app, instance, json, kill_ours, our_binary, run, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let field = field_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(field.to_str().unwrap());
        kill_ours(our_binary("Field"));
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
    (a - b).abs() <= 1.0
}

fn test(field: &str) -> Result<()> {
    let client = McpClient::start(&[])?;
    kill_ours(our_binary("Field"));
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    client.call(
        "launch_app",
        json!({ "display_id": id, "app": field, "environment": { "FIELD_MIN_SIZE": "500,200" } }),
    )?;
    sleep_ms(800);
    let listed = |client: &McpClient| -> Result<Option<agent_screen_tests::Window>> {
        Ok(client
            .call("list_windows", json!({ "display_id": id }))?
            .windows()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.app == "Field"))
    };
    let window = listed(&client)?.context("no Field window on the display")?;
    let wid = window.window_id;

    // A full frame, display-relative, lands where asked.
    let placed = client.call(
        "place_window",
        json!({ "display_id": id, "window_id": wid, "x": 100, "y": 120, "width": 900, "height": 500 }),
    )?;
    let got = frame(&placed);
    check(
        "place_window returns the frame it set",
        !placed.is_error
            && got.is_some_and(|[x, y, w, h]| {
                near(x, 100.0) && near(y, 120.0) && near(w, 900.0) && near(h, 500.0)
            }),
        placed.text(),
    );
    let now = listed(&client)?.context("Field left the display")?;
    check(
        "list_windows shows the same frame",
        near(now.x, 100.0) && near(now.y, 120.0) && near(now.width, 900.0),
        format!("{now:?}"),
    );

    // Below the app's minimum: the app's own width comes back, with a note.
    let small = client.call(
        "place_window",
        json!({ "display_id": id, "window_id": wid, "width": 300 }),
    )?;
    let got = frame(&small);
    check(
        "a width under the app's minimum returns the width the app kept",
        got.is_some_and(|[x, _, w, h]| near(w, 500.0) && near(x, 100.0) && near(h, 500.0)),
        small.text(),
    );
    check(
        "and says the window didn't take it",
        small
            .get("note")
            .as_str()
            .is_some_and(|n| n.contains("width 500 for 300")),
        small.text(),
    );

    // Wider than the display: as wide as the display, still on it.
    let wide = client.call(
        "place_window",
        json!({ "display_id": id, "window_id": wid, "x": 600, "width": 1400 }),
    )?;
    let got = frame(&wide);
    check(
        "a frame past the display's edge stays inside it",
        got.is_some_and(|[x, _, w, _]| x >= 0.0 && x + w <= 1281.0 && w >= 1200.0),
        wide.text(),
    );

    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}
