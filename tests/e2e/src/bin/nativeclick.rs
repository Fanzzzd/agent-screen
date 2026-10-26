//! One click on a text field of an inactive native window, through the
//! Viewer, puts the keyboard in that field (issue #41): the user clicked a
//! field of aTrust's login window on the Agent Display and had to click
//! again before typing reached it. The user's last click went to another
//! app on the display. Clicks and keys go through the Viewer's own
//! handler, so the user's pointer and keyboard are untouched, and neither
//! app ever becomes the active app.
//!
//! The last click before it goes to another app, then to the login app's
//! other window; the third round also has Agent Screen think the login app
//! made itself the front app, as aTrust does when it shows its window.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e nativeclick
//!
//! Makes 2 displays: the stage and the login window's.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, field_app, instance, json, kill_ours, login_app, open_stage, our_binary, probe,
    run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let login = login_app()?;
        let field = field_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(login.to_str().unwrap(), field.to_str().unwrap());
        kill_ours(our_binary("Login"));
        kill_ours(our_binary("Field"));
        stop_test_app();
        result
    })
}

/// The login window, whose title says which field has focus and what
/// each holds: "login focus=user|password|none text=USER|PASSWORD key=0|1".
fn login(control: &Control, id: u64) -> Result<Option<Value>> {
    titled(control, id, "login")
}

fn titled(control: &Control, id: u64, prefix: &str) -> Result<Option<Value>> {
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(listed["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["title"].as_str().is_some_and(|t| t.starts_with(prefix)))
            .cloned()
    }))
}

fn title(control: &Control, id: u64) -> String {
    login(control, id)
        .ok()
        .flatten()
        .and_then(|w| w["title"].as_str().map(String::from))
        .unwrap_or_default()
}

fn test(login_path: &str, field_path: &str) -> Result<()> {
    let control = Control::connect()?;
    let key = "nativeclick";
    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Native click", "width": 1280, "height": 600 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let (dw, dh) = (
        display["width"].as_f64().unwrap_or(1280.0),
        display["height"].as_f64().unwrap_or(600.0),
    );
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": login_path }),
    )?;
    let login_pid = launched["pid"].as_i64().context("the login app's pid")?;
    let mut window = None;
    for _ in 0..50 {
        window = login(&control, id)?.filter(|w| w["title"] != "login");
        if window.is_some() {
            break;
        }
        sleep_ms(100);
    }
    let window = window.context("no login window")?;
    let (wx, wy, wh) = (
        window["x"].as_f64().context("window x")?,
        window["y"].as_f64().context("window y")?,
        window["height"].as_f64().context("window height")?,
    );
    let field = control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": field_path }),
    )?;
    // Field opens over the login window: it goes below it, where a click
    // on it doesn't raise it over the login fields.
    let origin: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let moved = control.call(
        "debugMoveWindow",
        json!({
            "pid": field["pid"].as_i64().context("Field's pid")?,
            "windowId": field["window"]["windowId"],
            "x": origin[0] + 20.0,
            "y": origin[1] + 400.0,
        }),
    )?;
    check(
        "Field moves below the login window",
        moved["moved"] == true,
        moved.to_string(),
    );
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);

    // The fields' middles, under the title bar: user 62, password 122 down
    // the 200 points of content.
    let top = wy + (wh - 200.0);
    // The app's other window opens over the login window: it goes to the
    // right of it.
    let other = titled(&control, id, "other")?.context("no second login window")?;
    let moved = control.call(
        "debugMoveWindow",
        json!({
            "pid": login_pid,
            "windowId": other["windowId"],
            "x": origin[0] + 900.0,
            "y": origin[1] + 40.0,
        }),
    )?;
    check(
        "the login app's other window moves beside the login window",
        moved["moved"] == true,
        moved.to_string(),
    );
    let other_at = (1050.0, 200.0);
    let field_at = (320.0, 480.0);
    let rounds = [
        // The user's last click went to another app on the display.
        (
            "password",
            122.0,
            0u16,
            "a",
            field_at,
            false,
            "after a click in another app",
        ),
        // To another window of the login app: the login window is inactive.
        (
            "user",
            62.0,
            11u16,
            "b",
            other_at,
            false,
            "after a click in the app's other window",
        ),
        // The same, with the login app the front app, as aTrust makes itself.
        (
            "password",
            122.0,
            8u16,
            "c",
            other_at,
            true,
            "with its app in front",
        ),
    ];
    let mut typed_user = String::new();
    let mut typed_password = String::new();
    for (name, down, code, letter, last, front, how) in rounds {
        control.call(
            "debugViewerInput",
            json!({ "kind": "click", "x": last.0 / dw, "y": last.1 / dh }),
        )?;
        sleep_ms(600);
        let before = title(&control, id);
        if front {
            control.call("debugFront", json!({ "pid": login_pid }))?;
        }
        control.call(
            "debugViewerInput",
            json!({ "kind": "click", "x": (wx + 180.0) / dw, "y": (top + down) / dh }),
        )?;
        sleep_ms(600);
        let after = title(&control, id);
        check(
            &format!("one click puts the keyboard in the {name} field {how}"),
            after.contains(&format!("focus={name}")),
            format!("{before} then {after}"),
        );
        control.call("debugViewerInput", json!({ "kind": "key", "code": code }))?;
        sleep_ms(400);
        if name == "user" {
            typed_user.push_str(letter);
        } else {
            typed_password.push_str(letter);
        }
        let typed = title(&control, id);
        check(
            &format!("the key typed next lands in the {name} field {how}"),
            typed.contains(&format!("text={typed_user}|{typed_password} ")),
            typed,
        );
    }
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}
