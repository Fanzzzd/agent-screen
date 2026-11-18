//! An app launch_app starts shows a small loading window, then, seconds
//! after launch_app answered with it, closes it and opens its main window
//! on the main screen, where it keeps a saved frame. WeCom does this. The
//! main window belongs on the agent's display and in the claim file. Run
//! three times: once with the app idle, once with its main thread busy for
//! 1.5 s as it opens the main window, as a large app building it is, which
//! outlasts Accessibility's 1 s timeout for the catch's first look, and once
//! with the window on screen before the app goes busy for 1.5 s, so the
//! launch's watch finds a window the app can't describe yet. Its first ask
//! times out about 1.1 s in, and the app answers the next one about 1.5 s
//! in: a watch that took that answer as "no such window" left the window
//! on your screen.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e latewindow
//!
//! Then the app is hidden, as by Command-H, after its window showed: the
//! launch's watch leaves it hidden, as the window may be the user's.
//!
//! Makes 1 display and takes about 45 s. Before the fix, the busy app's
//! main window stayed on your screen, and AeroSpace tiled it there.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, ToolResult, Value, Window, allow_on_your_screen, check, debug_pid, instance, json,
    kill_ours, late_window_app, our_binary, run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

/// How long the app's main thread is busy as it opens its main window, ms,
/// and whether the window is on screen before that.
const CASES: [(u64, bool); 3] = [(0, false), (1500, false), (1500, true)];

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let app = late_window_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        kill_ours(our_binary("LateWindow"));
        let result = test(app.to_str().context("app path")?);
        kill_ours(our_binary("LateWindow"));
        stop_test_app();
        result
    })
}

/// The window ids in the test app's claim file.
fn claimed() -> Vec<u64> {
    let Some(pid) = debug_pid() else {
        return Vec::new();
    };
    let path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!(
        "Library/Application Support/AeroSpace/claims/{pid}.json"
    ));
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or_default();
    value["windows"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

fn test(app: &str) -> Result<()> {
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    for (busy, flush) in CASES {
        println!("main thread busy for {busy} ms as the main window opens, shown first: {flush}");
        if flush {
            allow_on_your_screen(
                "LateWindow",
                busy + 1000,
                "a window shown before its app goes busy can't be moved until the app answers",
            );
        } else if busy == 0 {
            allow_on_your_screen(
                "LateWindow",
                200,
                "a window an app opens by itself shows where the app puts it until Agent Screen hears of it and moves it, 64 ms for Field on 2026-10-26",
            );
        } else {
            allow_on_your_screen(
                "LateWindow",
                600,
                "a busy app's window shows once its main thread is free, and only then can Accessibility move it, 231 ms on 2026-11-06",
            );
        }
        main_window_lands(&client, id, app, busy, flush)?;
        kill_ours(our_binary("LateWindow"));
        sleep_ms(500);
    }
    println!("hidden after its window showed");
    hidden_app_stays_hidden(&client, id, app)?;
    kill_ours(our_binary("LateWindow"));
    sleep_ms(500);
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}

fn named_main(listed: &ToolResult) -> Option<Window> {
    listed
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.title == "Main")
}

fn main_window_lands(client: &McpClient, id: u64, app: &str, busy: u64, flush: bool) -> Result<()> {
    // 4 s after its first window: past the 3 s an agent's action counts
    // for, measured from launch_app's answer.
    let launched = client.call(
        "launch_app",
        json!({ "display_id": id, "app": app, "environment": {
            "LATE_MAIN_MS": "4000", "LATE_BUSY_MS": busy.to_string(),
            "LATE_FLUSH": if flush { "1" } else { "0" },
        } }),
    )?;
    let first = launched.window().context("the loading window")?;
    check(
        "launch_app answers with the loading window",
        first.title == "Loading",
        launched.text(),
    );
    // No action meanwhile, as the agent waits for the app.
    let mut main = None;
    for _ in 0..40 {
        sleep_ms(250);
        let here = client.call("list_windows", json!({ "display_id": id }))?;
        let strays = client.call("list_windows", json!({ "display_id": id, "strays": true }))?;
        main = named_main(&here).or(named_main(&strays));
        if main.is_some() {
            break;
        }
    }
    check("the app opened its main window", main.is_some(), "in 10 s");
    // Long enough for a busy app to answer, and for AeroSpace to move it
    // again.
    sleep_ms(busy + 1500);
    let listed = client.call("list_windows", json!({ "display_id": id }))?;
    let on_display = named_main(&listed);
    check(
        "the main window, opened late, is on the display",
        on_display.is_some(),
        listed.text(),
    );
    let strays = client.call("list_windows", json!({ "display_id": id, "strays": true }))?;
    check(
        "and isn't a stray on the user's screen",
        named_main(&strays).is_none(),
        strays.text(),
    );
    let window = on_display.or(main).map(|w| w.window_id);
    check(
        "the main window is claimed, so AeroSpace leaves it alone",
        window.is_some_and(|w| claimed().contains(&w)),
        format!("window {window:?}, claimed {:?}", claimed()),
    );
    Ok(())
}

/// Within the cold start's 30 s, an app hidden after its window showed
/// stays hidden: the window may be the user's, and they hid it.
fn hidden_app_stays_hidden(client: &McpClient, id: u64, app: &str) -> Result<()> {
    let launched = client.call(
        "launch_app",
        json!({ "display_id": id, "app": app, "environment": { "LATE_MAIN_MS": "60000" } }),
    )?;
    let first = launched.window().context("the loading window")?;
    check(
        "launch_app answers with the loading window",
        first.title == "Loading",
        launched.text(),
    );
    sleep_ms(500);
    let _ = std::process::Command::new("/usr/bin/pkill")
        .args(["-USR1", "-f"])
        .arg(our_binary("LateWindow"))
        .status();
    // The watch looks every 150 ms.
    sleep_ms(2000);
    let listed = client.call("list_windows", json!({ "display_id": id }))?;
    let shown = listed
        .windows()
        .unwrap_or_default()
        .into_iter()
        .any(|w| w.window_id == first.window_id);
    check(
        "the hidden app's window isn't shown again on the display",
        !shown,
        listed.text(),
    );
    Ok(())
}
