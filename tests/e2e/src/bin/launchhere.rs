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
//! opens on the stage, never on your screen.

use agent_screen_tests::stage_fixture::{self, StageFixture};
use std::{process::ExitCode, sync::Arc};

use agent_screen_tests::{
    Control, check, debug_pid, field_app, instance, json, open_user_stage, out, run, sh_lenient,
    sign_step, sleep_ms, start_test_app, stop_test_app,
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
        field_app()?;
        let result = test();
        if stage_fixture::cleanup_complete() {
            stop_test_app();
        }
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
    let stage = Arc::new(Control::connect()?);
    sign_step("opening the user stand-in stage / 打开模拟用户显示器");
    let stage_id = open_user_stage(&stage)?;
    let control = Control::connect()?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "launchhere", "title": "Launch here", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    // Prepared outside launch_app: a hardware claim grants no session ownership.
    let mut first = StageFixture::prepare(stage.clone(), stage_id)?;
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
    sign_step("opening Field from the Viewer / 从查看器打开 Field");
    first.show()?;
    let first_pid = first.pid();
    let mut here = false;
    for _ in 0..60 {
        here = field_window(&control, id)?;
        if here {
            break;
        }
        sleep_ms(100);
    }
    let on_screen = agent_screen_tests::windows_of(first_pid);
    check(
        "the app opened from Spotlight shows its window on the Viewer's display",
        here,
        format!("Field's windows: {on_screen:?}"),
    );
    first.close()?;
    sleep_ms(500);

    // Typed into Spotlight after a click on their own screen, where Finder
    // came to the front: the app opens there, as before.
    let finder: i64 = sh_lenient("/usr/bin/pgrep", &["-x", "Finder"])
        .lines()
        .next()
        .and_then(|p| p.trim().parse().ok())
        .context("Finder isn't running")?;
    let mut second = StageFixture::prepare(stage.clone(), stage_id)?;
    control.call("debugFront", json!({ "pid": finder }))?;
    control.call("debugUserKey", json!({ "pid": spotlight }))?;
    sign_step("opening Field outside the Viewer / 在查看器之外打开 Field");
    second.show()?;
    let second_pid = second.pid();
    let mut shown = Vec::new();
    for _ in 0..40 {
        shown = agent_screen_tests::windows_of(second_pid);
        if !shown.is_empty() {
            break;
        }
        sleep_ms(100);
    }
    sleep_ms(1500);
    check(
        "after a click outside the Viewer, Field stays on the user stand-in stage",
        !shown.is_empty() && !field_window(&control, id)? && field_window(&stage, stage_id)?,
        format!("Field's windows: {shown:?}"),
    );
    second.close()?;
    sleep_ms(300);
    control.call(
        "endDisplay",
        json!({ "sessionKey": "launchhere", "displayId": id }),
    )?;
    stage.call("endDisplay", json!({ "displayId": stage_id }))?;
    Ok(())
}
