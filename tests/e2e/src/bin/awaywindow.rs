//! place_window for a window the user put away (#221). With the user's
//! words for the app, a window minimized to the Dock comes to the display
//! as a Handover with no frame of it on the user's screens, where the
//! Dock's restore animation would draw it though the window list doesn't
//! place it there, and the front app stays the same. When the display ends
//! it goes back into the Dock at its old frame. What would change the
//! user's screen is refused and leaves Field as it was: an app the user
//! hid, whose other windows showing it would bring back, and an app with
//! another window on screen, here a 30-point one at the status bar's
//! level outside every display, which hiding the app would take away. A
//! window that won't move is put back in the Dock. A ScreenCaptureKit
//! capture of Field's pixels alone checks every frame.
//!
//! Field starts hidden through `open -g -j`, outside launch_app, so its
//! window is the user's and never shows: "minimize" minimizes it while the
//! app is hidden, then shows the app.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e awaywindow
//!
//! Makes 1 display. Field sits minimized in your Dock for a few seconds at
//! a time. E2E_AWAY_CASES=hidden,second runs only those cases, for a run on
//! code that would show the window on your screen in the others.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_screen_tests::{
    McpClient, Shown, check, field_app, instance, json, out, run, sh, sh_lenient, sign_step,
    sleep_ms, snapshot, start_test_app, stop_test_app, watch_shown,
};
use anyhow::{Context, Result, bail};

/// One way Field starts, and what place_window should do with it.
struct Case {
    name: &'static str,
    /// FIELD_AWAY: "minimize" or "hide".
    away: &'static str,
    /// Extra FIELD_ variables.
    env: &'static [&'static str],
    /// "minimized hidden" as Field starts and, for a refusal, stays.
    state: &'static str,
    /// Text the refusal must carry; None when the window should come.
    refused: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        name: "hidden",
        away: "hide",
        env: &["FIELD_SECOND=1"],
        state: "0 1",
        refused: Some("is hidden"),
    },
    Case {
        name: "second",
        away: "minimize",
        env: &["FIELD_SECOND=1"],
        state: "1 0",
        refused: Some("other windows"),
    },
    Case {
        name: "pinned",
        away: "minimize",
        env: &["FIELD_PIN=1"],
        state: "1 0",
        refused: Some("could not move the Field window"),
    },
    Case {
        name: "minimized",
        away: "minimize",
        env: &[],
        state: "1 0",
        refused: None,
    },
];

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
    let only = std::env::var("E2E_AWAY_CASES").unwrap_or_default();
    let cases: Vec<&Case> = CASES
        .iter()
        .filter(|c| only.is_empty() || only.split(',').any(|o| o == c.name))
        .collect();
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    let mut came = None;
    for case in cases {
        let name = case.name;
        sign_step(&format!("Field put away: {name} / Field 已收起：{name}"));
        let (field, state) = start_field(case)?;
        *pid = Some(field);
        let before = read(&state.with_extension("txt.away"))?;
        let frame = read(&state.with_extension("txt.awayframe"))?;
        check(
            &format!("{name}: Field starts put away"),
            before == case.state,
            format!("minimized, hidden: {before:?}"),
        );
        let front = snapshot().front;
        let (shown, placed) = watch_shown(&format!("awaywindow-{name}"), field, &[id], || {
            client.tool(
                "place_window",
                json!({
                    "display_id": id, "app": "Field",
                    "user_instruction": "Put my Field window on your display and check it",
                }),
            )
        })?;
        println!("     {name}: {}", describe(&shown));
        check(
            &format!("{name}: the front app stays {front:?}"),
            snapshot().front == front,
            snapshot().front,
        );
        check(
            &format!("{name}: no frame of Field showed on your screens"),
            shown.iter().any(|s| s.display != id)
                && shown.iter().all(|s| s.display == id || s.shown == 0),
            describe(&shown),
        );
        let listed = client.call("list_windows", json!({ "display_id": id }))?;
        match case.refused {
            Some(why) => {
                check(
                    &format!("{name}: place_window refuses, saying why"),
                    placed.is_error && placed.text().contains(why),
                    placed.text(),
                );
                check(
                    &format!("{name}: Field isn't on the display"),
                    !listed.text().contains("\"Field\""),
                    listed.text(),
                );
                // Undone steps settle within a moment.
                sleep_ms(500);
                let after = read(&state.with_extension("txt.away"))?;
                let frame_after = read(&state.with_extension("txt.awayframe"))?;
                check(
                    &format!("{name}: Field stays as the user left it"),
                    after == case.state && frame_after == frame,
                    format!("minimized, hidden: {after:?}, frame {frame_after:?} (was {frame:?})"),
                );
                sh_lenient("/bin/kill", &["-TERM", &field.to_string()]);
                *pid = None;
                sleep_ms(800);
            }
            None => {
                check(
                    &format!("{name}: place_window brings it, from the Dock, naming it"),
                    !placed.is_error
                        && placed.text().contains("\"fromDock\":true")
                        && placed.text().contains("\"windowId\"")
                        && placed.text().contains("\"title\":\"Field\""),
                    placed.text(),
                );
                check(
                    &format!("{name}: the window is on the display, handed over"),
                    listed.text().contains("\"Field\"") && listed.text().contains("\"handedOver\""),
                    listed.text(),
                );
                check(
                    &format!("{name}: the capture sees it on the display"),
                    shown.iter().any(|s| s.display == id && s.shown > 0),
                    describe(&shown),
                );
                came = Some((field, state, frame, front));
            }
        }
    }
    // The display ends: the window goes back into the Dock where it was.
    match came {
        Some((field, state, frame, front)) => {
            let (shown, ended) = watch_shown("awaywindow-home", field, &[], || {
                client.call("end_display", json!({ "display_id": id }))
            })?;
            check("home: the display ends", !ended.is_error, ended.text());
            let mut after = String::new();
            let mut frame_after = String::new();
            for _ in 0..30 {
                after = read(&state.with_extension("txt.away"))?;
                frame_after = read(&state.with_extension("txt.awayframe"))?;
                if after == "1 0" && frame_after == frame {
                    break;
                }
                sleep_ms(100);
            }
            println!("     home: {}", describe(&shown));
            check(
                "home: the window is back in the Dock at its old frame",
                after == "1 0" && frame_after == frame,
                format!("minimized, hidden: {after:?}, frame {frame_after:?} (was {frame:?})"),
            );
            check(
                "home: no frame of Field showed on your screens on the way",
                !shown.is_empty() && shown.iter().all(|s| s.shown == 0),
                describe(&shown),
            );
            check(
                &format!("home: the front app stays {front:?}"),
                snapshot().front == front,
                snapshot().front,
            );
        }
        None => {
            client.call("end_display", json!({ "display_id": id }))?;
        }
    }
    client.close();
    Ok(())
}

/// Start Field put away as `case` says, outside launch_app: its pid and
/// the file its state goes to.
fn start_field(case: &Case) -> Result<(u64, PathBuf)> {
    let marker = format!("--awaywindow-{}-{}", std::process::id(), case.name);
    let state = out().join(format!("awaywindow-{}.txt", case.name));
    for ext in ["txt.away", "txt.awayframe"] {
        let _ = std::fs::remove_file(state.with_extension(ext));
    }
    let mut args: Vec<String> = ["-n", "-g", "-j"].map(String::from).to_vec();
    let mut env = vec![
        format!("FIELD_AWAY={}", case.away),
        format!("FIELD_OUT={}", state.display()),
        "FIELD_LIFETIME_MS=60000".into(),
    ];
    env.extend(case.env.iter().map(|e| e.to_string()));
    for e in env {
        args.push("--env".into());
        args.push(e);
    }
    args.push(out().join("Field.app").display().to_string());
    args.push("--args".into());
    args.push(marker.clone());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    sh("/usr/bin/open", &refs)?;
    let pid = pid_of(&marker)?;
    read(&state.with_extension("txt.away"))?;
    Ok((pid, state))
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

/// What Field wrote to `file`, once it has, trimmed.
fn read(file: &Path) -> Result<String> {
    for _ in 0..50 {
        if let Ok(text) = std::fs::read_to_string(file)
            && !text.trim().is_empty()
        {
            return Ok(text.trim().to_string());
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
