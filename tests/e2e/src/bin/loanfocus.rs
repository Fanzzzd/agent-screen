//! An Input Loan's focus step alone, many times on one display (issue
//! #121). Each round opens a fresh Electron harness as a background
//! accessory app, which AeroSpace has never seen in front, hands its window
//! over, and makes it main and its app frontmost as a loan does. AeroSpace
//! filed such a window in the user's focused workspace and laid it out on
//! their screen. The window must be on the Agent Display when the step
//! returns and stay there for 2.5 s, the user's windows must keep their
//! frames, and nothing may show on the user's screen.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e loanfocus
//!
//! E2E_LOOPS rounds, 20 by default, about 5 s each. Makes 1 display. Takes
//! the front app each round and gives it back: hands off.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, check, env_count, fixture, instance, json, locked, out, run, sh_lenient, skip,
    sleep_ms, start_test_app,
};
use anyhow::{Context, Result};

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
        let control = Control::connect()?;
        let key = "loanfocus";
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": key, "title": "Loan focus", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let result = rounds(&control, &id, &electron);
        quit_harness();
        let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
        result
    })
}

fn quit_harness() {
    let path = fixture("electron");
    let path = path.to_str().unwrap_or_default();
    sh_lenient("/usr/bin/pkill", &["-f", path]);
    for _ in 0..50 {
        if sh_lenient("/usr/bin/pgrep", &["-f", path])
            .trim()
            .is_empty()
        {
            return;
        }
        sleep_ms(100);
    }
}

fn harness(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["kind"] == "window")
            .cloned()
    }))
}

fn rounds(control: &Control, id: &Value, electron: &std::path::Path) -> Result<()> {
    let loops = env_count("E2E_LOOPS", 20.0) as usize;
    let mut moved = 0;
    let mut tripped = 0;
    let mut others = 0;
    for round in 1..=loops {
        if locked() {
            anyhow::bail!("the screen is locked: no app can come to the front");
        }
        sh_lenient(
            "/usr/bin/open",
            &[
                "-n",
                "-g",
                "--env",
                &format!("HARNESS_DISPLAY={id}"),
                "--env",
                "HARNESS_BACKGROUND=1",
                "-a",
                electron.to_str().unwrap_or_default(),
                "--args",
                fixture("electron").to_str().unwrap_or_default(),
            ],
        );
        let mut window = None;
        for _ in 0..50 {
            window = harness(control, id)?
                .filter(|w| w["title"].as_str().is_some_and(|t| t.contains("w=")));
            if window.is_some() {
                break;
            }
            sleep_ms(200);
        }
        let window = window.context("the harness window didn't show")?;
        control.call(
            "debugHandOver",
            json!({ "displayId": id, "windowId": window["windowId"] }),
        )?;
        sleep_ms(1000);
        let seen = control.call(
            "debugLoanFocus",
            json!({ "displayId": id, "windowId": window["windowId"], "watchMs": 2500 }),
        )?;
        let stayed = seen["came"].is_null()
            && seen["after"] == seen["before"]
            && seen["movedAfterReadyMs"].is_null();
        let touched = seen["othersChanged"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        moved += usize::from(!stayed);
        tripped += usize::from(seen["trips"].as_u64().unwrap_or(0) > 0);
        others += usize::from(touched);
        println!(
            "     round {round:2}: came {} ready {} AeroSpace moved it at {} ({} times), moved after ready {}, others changed {}, before {} after {}",
            seen["came"],
            seen["readyAtMs"],
            seen["movedAtMs"],
            seen["trips"],
            seen["movedAfterReadyMs"],
            seen["othersChanged"],
            seen["before"],
            seen["after"]
        );
        quit_harness();
        sleep_ms(500);
    }
    check(
        "AeroSpace never moves the loan's window off the Agent Display, nor the user's windows",
        moved == 0 && tripped == 0 && others == 0,
        format!(
            "{moved} of {loops} rounds left the window moved, {others} moved other windows; \
             AeroSpace moved it {tripped} times before the loan was ready"
        ),
    );
    Ok(())
}
