//! An Electron window on an Agent Display, from `fixtures/electron`:
//! launch_app starts a new instance with arguments (issue #14); a click and
//! a double click on HTML5-draggable rows act like real ones and leave the
//! renderer alive (#18, #20); a drag from just outside the window's right
//! edge resizes it from the first move on (#15); and the screenshot after
//! each move shows the window at its new width (#16). An accessory app whose
//! window opens right on the display is listed too (#17). A window larger
//! than the display stays on it, and so does one its app grows past the
//! display or moves to the main screen after it shows (#19).
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e electron

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, instance, json, kill_ours, out, probe, run, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use base64::Engine;
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&electron);
        quit_harness();
        stop_test_app();
        result
    })
}

/// The harness window: its listing, title included.
fn harness(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"]
        .as_array()
        .and_then(|w| {
            w.iter()
                .find(|w| w["app"].as_str() == Some("Electron") && w["kind"] == "window")
        })
        .cloned())
}

/// A number the page put in its title, as in "w=640".
fn seen(window: &Value, key: &str) -> i64 {
    let title = window["title"].as_str().unwrap_or_default();
    title
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1)
}

fn gone(window: &Value) -> bool {
    window["title"]
        .as_str()
        .unwrap_or_default()
        .contains("renderer gone")
}

/// The window's right edge in a screenshot: the last green pixel in row `y`.
fn edge_in_shot(control: &Control, id: &Value, y: i64, name: &str) -> Result<i64> {
    let shot = control.call(
        "screenshot",
        json!({ "displayId": id, "format": "png", "settle": true }),
    )?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().context("image")?)?;
    let path = out().join(format!("{name}.png"));
    std::fs::write(&path, data)?;
    let span = probe(&[
        "span",
        path.to_str().unwrap(),
        &y.to_string(),
        "0",
        "255",
        "0",
    ])?;
    span.split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .context("no green in that row")
}

fn test(electron: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        // Wider than the image limit, so screenshot pixels aren't points, as
        // in issue #20.
        json!({ "sessionKey": "electron", "title": "Electron", "width": 1600, "height": 1000 }),
    )?;
    let id = display["displayId"].clone();
    // A regular app first, then an accessory app that shows its window
    // without activating, as development builds do.
    for background in [false, true] {
        let Some(window) = clicks(&control, &id, electron, background)? else {
            return Ok(());
        };
        if background {
            break;
        }
        resize(&control, &id, &window)?;
        quit_harness();
    }
    quit_harness();
    opened_there(&control, &id, electron)?;
    quit_harness();
    big(&control, &id, electron)?;
    for later in ["1700x1100", "main"] {
        quit_harness();
        moved_later(&control, &id, electron, later)?;
    }
    Ok(())
}

/// A window the app resizes past the display, or moves to the main screen,
/// 1.5 s after it shows, as an app restoring its saved bounds does (#19):
/// it is on the display 4 s after launch.
fn moved_later(
    control: &Control,
    id: &Value,
    electron: &std::path::Path,
    later: &str,
) -> Result<()> {
    let launched = control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_LATER": later },
        }),
    );
    let first = launched
        .as_ref()
        .ok()
        .and_then(|l| l["window"]["windowId"].as_i64());
    sleep_ms(4000);
    let now = harness(control, id)?;
    let frame = |w: &Value| format!("{}x{} at {},{}", w["width"], w["height"], w["x"], w["y"]);
    check(
        &format!(
            "a window its app makes {later} after it shows is on the display 4 s after launch"
        ),
        first.is_some() && now.as_ref().and_then(|w| w["windowId"].as_i64()) == first,
        format!(
            "launched {:?}, now {:?}",
            launched.as_ref().ok().map(|l| frame(&l["window"])),
            now.as_ref().map(frame)
        ),
    );
    Ok(())
}

/// An accessory app whose window opens right on the display, as one whose
/// saved bounds are there does (#17): it is listed as launched.
fn opened_there(control: &Control, id: &Value, electron: &std::path::Path) -> Result<()> {
    let launched = control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_BACKGROUND": "1", "HARNESS_DISPLAY": id.to_string() },
        }),
    );
    let mut window = None;
    for _ in 0..50 {
        window = harness(control, id)?;
        if window.is_some() {
            break;
        }
        sleep_ms(200);
    }
    check(
        "an accessory app whose window opens on the display is listed as launched",
        window.as_ref().is_some_and(|w| w["origin"] == "launched"),
        format!("launched {launched:?}, listed {window:?}"),
    );
    Ok(())
}

/// A window larger than the display is shrunk onto it, and stays there.
fn big(control: &Control, id: &Value, electron: &std::path::Path) -> Result<()> {
    let launched = control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_SIZE": "1700x1100" },
        }),
    );
    let first = launched
        .as_ref()
        .ok()
        .and_then(|l| l["window"]["windowId"].as_i64());
    sleep_ms(3000);
    let now = harness(control, id)?;
    check(
        "a window larger than the display is still on it 3 s after launch",
        first.is_some() && now.as_ref().and_then(|w| w["windowId"].as_i64()) == first,
        format!("launched {launched:?}, now {now:?}"),
    );
    Ok(())
}

fn quit_harness() {
    kill_ours(fixture("electron"));
    sleep_ms(1000);
}

/// Launch the harness and click its draggable rows; its window.
fn clicks(
    control: &Control,
    id: &Value,
    electron: &std::path::Path,
    background: bool,
) -> Result<Option<Value>> {
    let mode = if background {
        "an accessory app"
    } else {
        "a regular app"
    };
    let launched = control.call(
        "launchApp",
        json!({
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("electron").to_str()],
            "environment": { "HARNESS_BACKGROUND": if background { "1" } else { "0" } },
        }),
    );
    let mut window = None;
    for _ in 0..50 {
        window = harness(control, id)?.filter(|w| seen(w, "w") > 0);
        if window.is_some() {
            break;
        }
        sleep_ms(200);
    }
    check(
        &format!("launch_app starts {mode} with arguments, and lists its window"),
        window.is_some(),
        format!("{launched:?}"),
    );
    let Some(window) = window else {
        return Ok(None);
    };
    let (x, y, width) = (
        window["x"].as_i64().context("x")?,
        window["y"].as_i64().context("y")?,
        window["width"].as_i64().context("width")?,
    );
    // Rows sit below the title bar, 54 pt apart, in screenshot pixels.
    let scale = width as f64 / 640.0;
    let row = |n: i64| {
        (
            x + (60.0 * scale) as i64,
            y + ((28.0 + 8.0 + 23.0 + (n - 1) as f64 * 54.0) * scale) as i64,
        )
    };

    let (rx, ry) = row(1);
    control.call("click", json!({ "displayId": id, "x": rx, "y": ry }))?;
    sleep_ms(1200);
    let now = harness(control, id)?.context("window")?;
    check(
        &format!("{mode}: a click on a draggable row is one click, and the renderer lives"),
        !gone(&now) && seen(&now, "click") == 1 && seen(&now, "drag") == 0,
        now["title"].to_string(),
    );

    let (rx, ry) = row(2);
    control.call(
        "click",
        json!({ "displayId": id, "x": rx, "y": ry, "count": 2 }),
    )?;
    sleep_ms(1200);
    let now = harness(control, id)?.context("window")?;
    check(
        &format!(
            "{mode}: a double click on a draggable row is a double click, and the renderer lives"
        ),
        !gone(&now) && seen(&now, "dbl") == 1 && seen(&now, "drag") == 0,
        now["title"].to_string(),
    );

    // The fourth row is a button that changes the URL and the page.
    let (rx, ry) = row(4);
    control.call("click", json!({ "displayId": id, "x": rx, "y": ry }))?;
    sleep_ms(1500);
    let now = harness(control, id)?.context("window")?;
    check(
        &format!(
            "{mode}: a click carries a timestamp, and a soft navigation leaves the renderer alive"
        ),
        !gone(&now) && seen(&now, "nav") == 1 && seen(&now, "ts") > 0,
        now["title"].to_string(),
    );
    // A text field 8 pt above the bottom of the 480 pt window.
    let (fx, fy) = (
        x + (60.0 * scale) as i64,
        y + ((480.0 - 8.0 - 15.0) * scale) as i64,
    );
    control.call("click", json!({ "displayId": id, "x": fx, "y": fy }))?;
    control.call("type", json!({ "displayId": id, "text": "hi there" }))?;
    sleep_ms(800);
    let now = harness(control, id)?.context("window")?;
    check(
        &format!("{mode}: typed text reaches the page"),
        now["title"]
            .as_str()
            .unwrap_or_default()
            .ends_with("typed=hi there"),
        now["title"].to_string(),
    );
    // After a pause that lets the display's stream stop, an action's
    // screenshot waits for the change it makes.
    if !background {
        sleep_ms(6000);
        let (lx, ly) = (x + ((640.0 - 8.0 - 60.0) * scale) as i64, fy);
        control.call("click", json!({ "displayId": id, "x": lx, "y": ly }))?;
        let red = red_in_shot(control, id, fy - (40.0 * scale) as i64)?;
        check(
            "the screenshot after an action shows what the app drew a moment later",
            red,
            "the page was still green",
        );
        control.call("click", json!({ "displayId": id, "x": lx, "y": ly }))?;
        sleep_ms(1000);
    }
    Ok(Some(window))
}

/// Whether row `y` of the screenshot an action returns has the harness's red.
fn red_in_shot(control: &Control, id: &Value, y: i64) -> Result<bool> {
    let shot = control.call(
        "screenshot",
        json!({ "displayId": id, "format": "png", "settle": true }),
    )?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().context("image")?)?;
    let path = out().join("electron-later.png");
    std::fs::write(&path, data)?;
    let span = probe(&[
        "span",
        path.to_str().unwrap(),
        &y.to_string(),
        "255",
        "0",
        "0",
    ])?;
    Ok(span.split_whitespace().count() >= 2)
}

/// Resize the window by its right edge, pressing 2 px outside it.
fn resize(control: &Control, id: &Value, window: &Value) -> Result<()> {
    let (x, y, width, height) = (
        window["x"].as_i64().context("x")?,
        window["y"].as_i64().context("y")?,
        window["width"].as_i64().context("width")?,
        window["height"].as_i64().context("height")?,
    );
    let right = x + width;
    let middle = y + height / 2;
    let pressed = control.call(
        "mouseDown",
        json!({ "displayId": id, "x": right + 2, "y": middle }),
    );
    check(
        "a press just outside the right edge takes hold of it",
        pressed.is_ok(),
        format!("{pressed:?}"),
    );
    let mut widths = Vec::new();
    for (step, to) in [right - 80, right - 160].into_iter().enumerate() {
        control.call("move", json!({ "displayId": id, "x": to, "y": middle }))?;
        let edge = edge_in_shot(control, id, middle, &format!("electron-resize-{step}"))?;
        let now = harness(control, id)?.context("window")?;
        let listed = now["x"].as_i64().unwrap_or(0) + now["width"].as_i64().unwrap_or(0);
        widths.push((to, listed, edge));
    }
    let _ = control.call(
        "mouseUp",
        json!({ "displayId": id, "x": right - 160, "y": middle }),
    );
    let first = widths[0];
    check(
        "the first move after the press resizes the window",
        (first.1 - first.0).abs() <= 6,
        format!("moved to {}, edge at {}", first.0, first.1),
    );
    check(
        "the screenshot after each move shows the new edge",
        widths
            .iter()
            .all(|(_, listed, edge)| (listed - edge).abs() <= 6),
        format!("(target, listed edge, edge in screenshot): {widths:?}"),
    );
    Ok(())
}
