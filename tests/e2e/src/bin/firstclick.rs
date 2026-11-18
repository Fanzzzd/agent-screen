//! One click on a page opened in a browser that already runs reaches the
//! page (#79's comment, #28): launch_app opens the page in a new window of
//! the running Chrome, and a single click on the page's button runs its
//! script. Reported for Dia, where the first click only made the window
//! active and the second did the work.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e firstclick
//!
//! Chrome on a throwaway profile; skips when Chrome runs with windows.
//! 1 display.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, fixture, json, out, quit_profile, run, running_headed, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;

const KEY: &str = "firstclick";

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to test");
    }
    run(|| {
        start_test_app(&[])?;
        let result = test();
        quit_profile(&out().join("firstclick-profile"));
        stop_test_app();
        result
    })
}

fn center(tree: &str, pattern: &str) -> Result<(f64, f64)> {
    let found = Regex::new(&format!(r"{pattern}[^\n]* at \((\d+),(\d+)\)"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern} in\n{tree}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("firstclick-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "First click", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Google Chrome", "arguments": [
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run", "--no-default-browser-check", "about:blank",
        ] }),
    )?;
    sleep_ms(1500);
    // Chrome runs: the page opens in a new window of it.
    let page = format!("file://{}", fixture("web/signup.html").display());
    let opened = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": "Google Chrome", "open": page }),
    )?;
    println!(
        "     launch_app answered {}",
        opened.to_string().chars().take(300).collect::<String>()
    );
    let window = opened["window"]["windowId"].as_u64().context("no window")?;
    sleep_ms(1500);
    let state = || -> Result<String> {
        let state = control.call(
            "windowState",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
        )?;
        Ok(state["tree"].as_str().unwrap_or_default().to_string())
    };
    let submit = center(&state()?, r#"button "Create account""#)?;
    control.call(
        "click",
        json!({ "sessionKey": KEY, "displayId": id, "x": submit.0, "y": submit.1 }),
    )?;
    sleep_ms(700);
    let once = state()?.contains("Enter a valid email address.");
    check(
        "one click on the new window's button runs the page's script",
        once,
        "the page shows no error after one click",
    );
    if !once {
        control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": submit.0, "y": submit.1 }),
        )?;
        sleep_ms(700);
        println!(
            "     a second click runs it: {}",
            state()?.contains("Enter a valid email address.")
        );
    }
    Ok(())
}
