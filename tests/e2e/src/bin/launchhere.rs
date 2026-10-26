//! An app the user opens from a launcher while working in a Viewer opens
//! its window on that Viewer's display (issue #42): during a Takeover the
//! user opened an app with Raycast, and its window opened on their own
//! screen. The test app is told it is the front app, as the user's click
//! in the Viewer makes it, and that a key went to Spotlight, as the global
//! key monitor reports one; then Field opens in the background, so it
//! never takes the user's keyboard.
//!
//! Needs Field.app, which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e launchhere
//!
//! Makes 2 displays: the stage and the one the Viewer shows. Field's window
//! may show on your screen for a moment.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, check, debug_pid, instance, json, kill_ours, open_stage,
    our_binary, out, pid_of, run, sh_lenient, sleep_ms, start_test_app, stop_test_app,
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

fn field_window(control: &Control, id: u64) -> Result<bool> {
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(listed["windows"]
        .as_array()
        .is_some_and(|w| w.iter().any(|w| w["app"] == "Field")))
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    kill_ours(our_binary("Field"));
    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "launchhere", "title": "Launch here", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    // The user works in the Viewer: Agent Screen is the front app.
    let own = debug_pid().context("no test app")?;
    control.call("debugFront", json!({ "pid": own }))?;
    // They type the app's name into Spotlight, which takes keys without
    // coming to the front, and press Return.
    let spotlight: i64 = sh_lenient("/usr/bin/pgrep", &["-x", "Spotlight"])
        .lines()
        .next()
        .and_then(|p| p.trim().parse().ok())
        .context("Spotlight isn't running")?;
    control.call("debugUserKey", json!({ "pid": spotlight }))?;
    sh_lenient(
        "/usr/bin/open",
        &["-g", "-n", out().join("Field.app").to_str().unwrap()],
    );
    let mut here = false;
    for _ in 0..60 {
        here = field_window(&control, id)?;
        if here {
            break;
        }
        sleep_ms(100);
    }
    let on_screen = pid_of("Field.app/Contents/MacOS/Field$")
        .map(agent_screen_tests::windows_of)
        .unwrap_or_default();
    check(
        "the app opened from Spotlight shows its window on the Viewer's display",
        here,
        format!("Field's windows: {on_screen:?}"),
    );
    kill_ours(our_binary("Field"));
    sleep_ms(500);

    // Typed into Spotlight after a click on their own screen, where Finder
    // came to the front: the app opens there, as before.
    let finder: i64 = sh_lenient("/usr/bin/pgrep", &["-x", "Finder"])
        .lines()
        .next()
        .and_then(|p| p.trim().parse().ok())
        .context("Finder isn't running")?;
    control.call("debugFront", json!({ "pid": finder }))?;
    control.call("debugUserKey", json!({ "pid": spotlight }))?;
    allow_on_your_screen(
        "Field",
        u64::MAX,
        "opened from Spotlight after a click on your screen, the app is yours and opens there",
    );
    sh_lenient(
        "/usr/bin/open",
        &["-g", "-n", out().join("Field.app").to_str().unwrap()],
    );
    let mut shown = Vec::new();
    for _ in 0..40 {
        shown = pid_of("Field.app/Contents/MacOS/Field$")
            .map(agent_screen_tests::windows_of)
            .unwrap_or_default();
        if !shown.is_empty() {
            break;
        }
        sleep_ms(100);
    }
    sleep_ms(1500);
    check(
        "after a click on the user's own screen, it opens there",
        !shown.is_empty() && !field_window(&control, id)?,
        format!("Field's windows: {shown:?}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "launchhere", "displayId": id }),
    )?;
    Ok(())
}
