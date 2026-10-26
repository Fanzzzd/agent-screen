//! One click on a text field of an inactive window, through the Viewer,
//! puts the keyboard in that field (issue #41): the user clicked the
//! password field of aTrust's login window, an Electron app, on the Agent
//! Display and had to click again before typing reached it. The user's
//! last click went to another app on the display. A real click activates
//! Agent Screen, which a `debugViewerInput` click doesn't, so this one is
//! real: the pointer goes to the Viewer on the stage display for the click
//! and comes back. `ONECLICK_INJECT=1` clicks through the Viewer's handler
//! instead. The key afterwards goes through the Viewer's handler.
//!
//! Needs `target/e2e/Electron.app` (or E2E_ELECTRON) and Field.app,
//! which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e oneclick
//!
//! Makes 2 displays: the stage and the login window's. A sign on your
//! screen counts down from 3, then shows each step while the test moves
//! your pointer and makes the fixture and the test app active, and says
//! when you have the machine back; your front app is activated again.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, hid, instance, json, kill_ours, open_stage, our_binary, out, probe,
    run, sh_lenient, sign_step, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    let field = out().join("Field.app");
    if !field.exists() {
        eprintln!("no Field.app; run `cargo e2e ime` once to make it");
        return ExitCode::FAILURE;
    }
    let front = front_app();
    run(|| {
        // AGENTSCREEN_NO_CLAIMS=1 leaves AeroSpace guessing whose the
        // login window is.
        let unclaimed = std::env::var("AGENTSCREEN_NO_CLAIMS").unwrap_or_default();
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_NO_CLAIMS", &unclaimed),
        ])?;
        let result = test(&electron, &field);
        kill_ours(fixture("login"));
        kill_ours(our_binary("Field"));
        stop_test_app();
        if let Some(front) = &front {
            let _ = sh_lenient(
                "/usr/bin/osascript",
                &[
                    "-e",
                    &format!("tell application id \"{front}\" to activate"),
                ],
            );
        }
        result
    })
}

/// The bundle id of the app in front, to bring it back afterwards.
fn front_app() -> Option<String> {
    let asn = sh_lenient("/usr/bin/lsappinfo", &["front"]);
    let info = sh_lenient(
        "/usr/bin/lsappinfo",
        &["info", "-only", "bundleid", asn.trim()],
    );
    info.split('"').nth(3).map(String::from)
}

/// The login window, whose title says which field has focus and what
/// each holds: "login focus=user|password|none text=USER|PASSWORD".
fn login(control: &Control, id: u64) -> Result<Option<Value>> {
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(listed["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["title"].as_str().is_some_and(|t| t.starts_with("login")))
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

fn test(electron: &Path, field: &Path) -> Result<()> {
    let control = Control::connect()?;
    let key = "oneclick";
    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "One click", "width": 1280, "height": 600 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let (dw, dh) = (
        display["width"].as_f64().unwrap_or(1280.0),
        display["height"].as_f64().unwrap_or(600.0),
    );
    let real = std::env::var_os("ONECLICK_INJECT").is_none();
    // A real run: aTrust makes itself the active app as it shows its
    // window, and a click makes the test app active, so keys typed
    // meanwhile would go to them: the sign `run` counted down first.
    if real {
        sign_step("1/3 the login app opens and comes to the front / 登录窗口打开");
    }
    let launched = control.call(
        "launchApp",
        json!({
            "sessionKey": key,
            "displayId": id,
            "app": electron.to_str(),
            "arguments": [fixture("login").to_str()],
            "environment": { "LOGIN_ACTIVATE": if real { "1" } else { "0" } },
        }),
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
    // The app made itself the active app: AeroSpace, holding a window it
    // doesn't know for a second, must leave this one as it is. Laid out,
    // it grows to its workspace's width.
    sleep_ms(1500);
    let stayed = login(&control, id)?;
    check(
        "AeroSpace leaves the login window alone after its app activates",
        stayed
            .as_ref()
            .is_some_and(|w| w["width"] == window["width"] && w["height"] == window["height"]),
        format!("{} then {stayed:?}", window),
    );
    let (wx, wy, wh) = (
        window["x"].as_f64().context("window x")?,
        window["y"].as_f64().context("window y")?,
        window["height"].as_f64().context("window height")?,
    );

    // The user's last click goes to another app on the display.
    if real {
        sign_step("2/3 a click on another app on the display / 点另一个应用");
    }
    let other = control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": field.to_str() }),
    )?;
    // Field opens over the login window: it goes below it, where a click
    // on it doesn't raise it over the password field.
    let origin: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let moved = control.call(
        "debugMoveWindow",
        json!({
            "pid": other["pid"].as_i64().context("Field's pid")?,
            "windowId": other["window"]["windowId"],
            "x": origin[0] + 20.0,
            "y": origin[1] + 420.0,
        }),
    )?;
    check(
        "Field moves below the login window",
        moved["moved"] == true,
        moved.to_string(),
    );
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": 320.0 / dw, "y": 430.0 / dh }),
    )?;
    sleep_ms(800);
    check(
        "the user field holds the login window's keyboard",
        title(&control, id).contains("focus=user"),
        title(&control, id),
    );

    let viewer = control.call("debugViewer", json!({}))?;
    let image: Vec<f64> = viewer["image"]
        .as_array()
        .context("image")?
        .iter()
        .filter_map(|v| v.as_f64())
        .collect();
    // The password field's middle: 180 across and 122 down the 200 of
    // content, under the title bar.
    let (px, py) = (wx + 180.0, wy + (wh - 200.0) + 122.0);
    if std::env::var_os("ONECLICK_INJECT").is_some() {
        control.call("debugFront", json!({ "pid": login_pid }))?;
        control.call(
            "debugViewerInput",
            json!({ "kind": "click", "x": px / dw, "y": py / dh }),
        )?;
    } else {
        sign_step("3/3 one click on the password field / 点密码框");
        let at = (image[0] + px / dw * image[2], image[1] + py / dh * image[3]);
        hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
        sleep_ms(600);
    }
    sleep_ms(600);
    check(
        "one click puts the keyboard in the password field",
        title(&control, id).contains("focus=password"),
        title(&control, id),
    );
    // "a", through the Viewer's own handler.
    control.call("debugViewerInput", json!({ "kind": "key", "code": 0 }))?;
    sleep_ms(400);
    check(
        "the key typed next lands in the password field",
        title(&control, id).contains("text=|a "),
        title(&control, id),
    );
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}
