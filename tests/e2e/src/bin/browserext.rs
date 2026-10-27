//! Drags in a page of a browser Agent Screen didn't start, through Agent
//! Screen's extension (ADR 0012, #104): the way into the browser the user
//! runs, signed in as them, without pressing their mouse button.
//!
//! Chrome for Testing on a throwaway profile loads the extension from
//! `apps/macos/resources/browser-extension`, and the profile's
//! `NativeMessagingHosts` points at the test app. The test app prefers the
//! extension over the pipe launch_app gives the browser, so the drags take
//! the extension's way:
//!
//! - a drag on the page's splitter holds the left button on every move;
//! - the browser shows its "started debugging" bar while the page is held;
//! - an HTML5 drag of a row drops on the page's drop target;
//! - after 20 s without input the extension lets the page go, and the bar
//!   goes.
//!
//! Needs Playwright's Chrome for Testing (`npx playwright install chromium`).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e browserext
//!
//! 1 display.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use agent_screen_tests::{
    Control, check, debug_app, fixture, json, out, quit_profile, root, run, skip, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;

const KEY: &str = "browserext";
const PORT: &str = "8767";

fn main() -> ExitCode {
    let Some(browser) = chrome_for_testing() else {
        skip("no Chrome for Testing in Playwright's cache");
    };
    run(|| {
        start_test_app(&[("AGENTSCREEN_TEST_EXTENSION_FIRST", "1")])?;
        let mut server = Command::new("/usr/bin/python3")
            .args(["-m", "http.server", PORT, "--bind", "127.0.0.1", "-d"])
            .arg(fixture("web"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let result = test(&browser);
        let _ = server.kill();
        quit_profile(&out().join("browserext-profile"));
        stop_test_app();
        result
    })
}

fn chrome_for_testing() -> Option<PathBuf> {
    let cache = PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/ms-playwright");
    let mut builds: Vec<PathBuf> = std::fs::read_dir(&cache)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("chromium-"))
        })
        .collect();
    builds.sort();
    builds.into_iter().rev().find_map(|b| {
        ["chrome-mac-arm64", "chrome-mac"]
            .iter()
            .map(|arch| b.join(arch).join("Google Chrome for Testing.app"))
            .find(|app| app.exists())
    })
}

/// The profile's host manifest, for a script that starts the test app's
/// binary as the e2e instance's host.
fn register_host(profile: &std::path::Path) -> Result<()> {
    let script = out().join("browserext-host.sh");
    let binary = debug_app().join("Contents/MacOS/AgentScreen");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nAGENTSCREEN_INSTANCE={} exec '{}' \"$@\"\n",
            agent_screen_tests::instance().unwrap_or_default(),
            binary.display()
        ),
    )?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    }
    let hosts = profile.join("NativeMessagingHosts");
    std::fs::create_dir_all(&hosts)?;
    std::fs::write(
        hosts.join("com.agentscreen.browser.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "com.agentscreen.browser",
            "description": "Agent Screen (e2e)",
            "path": script,
            "type": "stdio",
            "allowed_origins": ["chrome-extension://onlnbaoaokkbahdlaobgmampgmekddop/"],
        }))?,
    )?;
    Ok(())
}

fn center(tree: &str, pattern: &str) -> Result<(f64, f64)> {
    let found = Regex::new(&format!(r"{pattern}[^\n]* at \((\d+),(\d+)\)"))?
        .captures(tree)
        .with_context(|| format!("nothing matches {pattern} in\n{tree}"))?;
    Ok((found[1].parse()?, found[2].parse()?))
}

fn seen(title: &str, key: &str) -> i64 {
    title
        .split_whitespace()
        .find_map(|p| p.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn test(browser: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let profile = out().join("browserext-profile");
    let _ = std::fs::remove_dir_all(&profile);
    register_host(&profile)?;
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Browser extension", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": browser, "arguments": [
            format!("--user-data-dir={}", profile.display()),
            format!("--load-extension={}", root().join("apps/macos/resources/browser-extension").display()),
            "--disable-features=DisableLoadExtensionCommandLineSwitch",
            "--no-first-run", "--no-default-browser-check",
            format!("http://127.0.0.1:{PORT}/drag.html"),
        ] }),
    )?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .context("no window")?;
    let state = || -> Result<(String, String)> {
        let state = control.call(
            "windowState",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window }),
        )?;
        let list = control.call("listWindows", json!({ "displayId": id }))?;
        let title = list["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"].as_u64() == Some(window)))
            .and_then(|w| w["title"].as_str())
            .unwrap_or_default()
            .to_string();
        Ok((
            state["tree"].as_str().unwrap_or_default().to_string(),
            title,
        ))
    };
    // The page, and the extension's host connecting to the app.
    let mut tree = String::new();
    for _ in 0..20 {
        sleep_ms(500);
        tree = state()?.0;
        if tree.contains("splitter") {
            break;
        }
    }
    sleep_ms(3000);
    let split = center(&tree, "splitter")?;
    let (row, bin) = (center(&tree, "row")?, center(&tree, "bin")?);

    let dragged = control.call(
        "drag",
        json!({ "sessionKey": KEY, "displayId": id,
                "fromX": split.0, "fromY": split.1, "toX": split.0 + 80., "toY": split.1 }),
    );
    sleep_ms(500);
    let (tree, title) = state()?;
    check(
        "a drag through the extension holds the left button, and the splitter follows it",
        dragged.is_ok()
            && seen(&title, "free") == 0
            && seen(&title, "held") > 0
            && seen(&title, "split") >= 70,
        format!("{title} {dragged:?}"),
    );
    let bar = |tree: &str| tree.contains("started debugging this browser");
    check(
        "the browser shows its debugging bar while the extension holds the page",
        bar(&tree),
        tree.lines()
            .filter(|l| l.contains("debugging") || l.contains("Cancel"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    // The bar pushed the page down: find the row and bin again.
    let (row, bin) = (
        center(&tree, "row").unwrap_or(row),
        center(&tree, "bin").unwrap_or(bin),
    );
    let dropped = control.call(
        "drag",
        json!({ "sessionKey": KEY, "displayId": id,
                "fromX": row.0, "fromY": row.1, "toX": bin.0, "toY": bin.1 }),
    );
    sleep_ms(800);
    let (_, title) = state()?;
    check(
        "an HTML5 drag through the extension drops the row on the drop target",
        dropped.is_ok() && seen(&title, "dragged") == 1 && seen(&title, "dropped") == 1,
        format!("{title} {dropped:?}"),
    );
    // 20 s from the last command, the drop's; the bar takes a moment.
    let mut tree = String::new();
    for _ in 0..30 {
        sleep_ms(1000);
        tree = state()?.0;
        if !bar(&tree) {
            break;
        }
    }
    check(
        "within 30 s without input, the extension lets the page go and the bar goes",
        !bar(&tree),
        tree.lines()
            .filter(|l| l.contains("debugging") || l.contains("Cancel"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }))?;
    Ok(())
}
