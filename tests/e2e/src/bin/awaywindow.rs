//! place_window for a window the user put away (#221): an app whose only
//! window is minimized to the Dock, then an app the user hid. With the
//! user's words for the app, the window comes to the display as a
//! Handover, and no frame of it shows on the user's screens on the way,
//! where the Dock's restore animation would draw it though the window list
//! doesn't place it there. A ScreenCaptureKit capture of Field's pixels
//! alone checks every frame, and must see Field on the display after.
//!
//! Field starts hidden through `open -g -j`, outside launch_app, so its
//! window is the user's and never shows: "minimize" minimizes it while the
//! app is hidden, then shows the app.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e awaywindow
//!
//! Makes 1 display. Field sits minimized in your Dock for a few seconds.

use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, Shown, check, field_app, instance, json, out, run, sh, sh_lenient, sign_step,
    sleep_ms, start_test_app, stop_test_app, watch_shown,
};
use anyhow::{Context, Result, bail};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        field_app()?;
        let mut pid = None;
        let result = test(&mut pid);
        if let Some(pid) = pid {
            sh_lenient("/bin/kill", &["-TERM", &format!("{pid}")]);
        }
        stop_test_app();
        result
    })
}

fn test(pid: &mut Option<u64>) -> Result<()> {
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    for away in ["minimize", "hide"] {
        sign_step(&format!("Field put away: {away} / Field 已收起：{away}"));
        let marker = format!("--awaywindow-{}-{away}", std::process::id());
        let state = out().join(format!("awaywindow-{away}.txt"));
        let _ = std::fs::remove_file(state.with_extension("txt.away"));
        sh(
            "/usr/bin/open",
            &[
                "-n",
                "-g",
                "-j",
                "--env",
                &format!("FIELD_AWAY={away}"),
                "--env",
                &format!("FIELD_OUT={}", state.display()),
                "--env",
                "FIELD_LIFETIME_MS=60000",
                out().join("Field.app").to_str().unwrap(),
                "--args",
                &marker,
            ],
        )?;
        let field_pid = pid_of(&marker)?;
        *pid = Some(field_pid);
        let put_away = wait_for_file(&state.with_extension("txt.away"))?;
        let expected = if away == "minimize" { "1 0" } else { "0 1" };
        check(
            &format!("Field starts put away: {away}"),
            put_away.trim() == expected,
            format!("minimized, hidden: {put_away:?}"),
        );

        let (shown, placed) = watch_shown(&format!("awaywindow-{away}"), field_pid, &[id], || {
            client.tool(
                "place_window",
                json!({
                    "display_id": id, "app": "Field",
                    "user_instruction": "Put my Field window on your display and check it",
                }),
            )
        })?;
        println!("     {away}: {}", describe(&shown));
        check(
            &format!("place_window brings the {away} window with the user's words"),
            !placed.is_error,
            placed.text(),
        );
        let listed = client.call("list_windows", json!({ "display_id": id }))?;
        check(
            &format!("the {away} window is on the display, handed over"),
            listed.text().contains("\"Field\"") && listed.text().contains("\"handedOver\""),
            listed.text(),
        );
        check(
            &format!("the capture sees the {away} window on the display"),
            shown.iter().any(|s| s.display == id && s.shown > 0),
            describe(&shown),
        );
        check(
            &format!("no frame of the {away} window showed on your screens"),
            shown.iter().any(|s| s.display != id)
                && shown.iter().all(|s| s.display == id || s.shown == 0),
            describe(&shown),
        );
        if let Some(pid) = pid.take() {
            sh_lenient("/bin/kill", &["-TERM", &pid.to_string()]);
        }
        sleep_ms(800);
    }
    client.call("end_display", json!({ "display_id": id }))?;
    client.close();
    Ok(())
}

/// The pid of the Field started with `marker` among its arguments.
fn pid_of(marker: &str) -> Result<u64> {
    for _ in 0..50 {
        let found = sh_lenient("/usr/bin/pgrep", &["-f", "--", marker]);
        if let Some(pid) = found.lines().next().and_then(|l| l.trim().parse().ok()) {
            return Ok(pid);
        }
        sleep_ms(100);
    }
    bail!("Field with {marker} didn't start")
}

/// What Field wrote to `file`, once it has.
fn wait_for_file(file: &std::path::Path) -> Result<String> {
    for _ in 0..50 {
        if let Ok(text) = std::fs::read_to_string(file) {
            return Ok(text);
        }
        sleep_ms(100);
    }
    bail!("Field wrote no {}", file.display())
}

fn describe(shown: &[Shown]) -> String {
    if shown.is_empty() {
        return "no display was captured".into();
    }
    shown
        .iter()
        .map(|s| {
            format!(
                "display {}: {} of {} frames, first {:?} ms, last {:?} ms, {:?}",
                s.display, s.shown, s.frames, s.first, s.last, s.saved
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}
