//! Record the clips of the promo video (promo/), out of the user's sight:
//! an agent applies a coupon and fills a checkout page in Chrome on its own
//! display, seen in the Viewer, then asks for help with the card number.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e promo
//!
//! The display wears promo/public/wallpaper.png, through the wallpaper
//! setting, which it puts back after. Chrome runs on a throwaway profile.
//! Skips when Chrome is running with windows, and quits the copy it started. Writes
//! promo/public/agent.mp4. Needs ffmpeg.

use std::process::ExitCode;

use anyhow::{Context, Result};
use regex::Regex;
use sidescreen_tests::{
    Control, Rect, check, cut_stage, fixture, json, open_stage, out, put_back_wallpaper,
    quit_profile, record_stage, root, run, running_headed, skip, sleep_ms, start_test_app,
    stop_test_app, wear_promo_wallpaper,
};

fn main() -> ExitCode {
    if running_headed("Google Chrome") {
        skip("Chrome is running; quit it to record");
    }
    let saved = wear_promo_wallpaper();
    let code = run(|| {
        start_test_app(&[])?;
        let control = Control::connect()?;
        let result = record(&control);
        quit_profile(&out().join("promo-chrome-profile"));
        stop_test_app();
        result
    });
    put_back_wallpaper(saved);
    code
}

/// The center of the element `role "label"` in a window state's tree.
fn center(tree: &str, element: &str) -> Result<(f64, f64)> {
    let pattern = format!(r#"{element}[^\n]* at \((\d+),(\d+)\)"#);
    let found = Regex::new(&pattern)?
        .captures(tree)
        .with_context(|| format!("no {element}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

fn record(control: &Control) -> Result<()> {
    // The stage on a connection of its own: a connection's session is the
    // display it created last.
    let stager = Control::connect()?;
    let stage = open_stage(&stager)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": "promo", "client": "Claude Code", "title": "Buy 4 Acme Cloud seats", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let profile = out().join("promo-chrome-profile");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/checkout.html").display());
    let launched = control.call(
        "launchApp",
        json!({ "displayId": id, "app": "Google Chrome", "arguments": [
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run", "--no-default-browser-check", page,
        ] }),
    )?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .context("no Chrome window")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1500);
    let state = control.call(
        "windowState",
        json!({ "displayId": id, "windowId": window }),
    )?;
    let tree = state["tree"].as_str().context("no tree")?.to_string();
    let viewer = control.call("debugViewer", json!({}))?;
    let frame: Rect = serde_json::from_value(viewer["frame"].clone())?;

    let video = record_stage(&stager, stage)?;
    sleep_ms(1500);

    let click = |element: &str, wait: u64| -> Result<()> {
        let (x, y) = center(&tree, element)?;
        control.call("click", json!({ "displayId": id, "x": x, "y": y }))?;
        sleep_ms(wait);
        Ok(())
    };
    let typed = |text: &str, wait: u64| -> Result<()> {
        control.call("type", json!({ "displayId": id, "text": text }))?;
        sleep_ms(wait);
        Ok(())
    };
    click(r#"textfield "Coupon""#, 400)?;
    typed("LAUNCH20", 500)?;
    click(r#"button "Apply""#, 1200)?;
    click(r#"textfield "Company""#, 400)?;
    typed("Lovelace Labs", 500)?;
    click(r#"textfield "Billing email""#, 400)?;
    typed("billing@lovelace.dev", 700)?;
    click(r#"textfield "Card number""#, 1500)?;
    control.call(
        "requestHelp",
        json!({ "displayId": id, "reason": "Checkout needs a card number. Can you enter it, then hand it back?" }),
    )?;
    sleep_ms(4000);
    control.call("withdrawHelp", json!({ "displayId": id }))?;
    sleep_ms(1000);
    let after = control.call(
        "windowState",
        json!({ "displayId": id, "windowId": window }),
    )?;
    let after = after["tree"].as_str().unwrap_or_default();
    check(
        "the agent's clicks and typing all landed",
        after.contains("Pay $76.80") && after.contains(r#"value="billing@lovelace.dev""#),
        after
            .lines()
            .filter(|l| l.contains("textfield"))
            .collect::<Vec<_>>()
            .join("; "),
    );

    let public = root().parent().unwrap().join("promo/public");
    std::fs::create_dir_all(&public)?;
    let clip = public.join("agent.mp4");
    cut_stage(&stager, stage, &video, frame, &clip, 1920)?;
    println!("{}", clip.display());
    Ok(())
}
