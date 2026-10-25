//! Peek keeps drawing while a display comes and goes. A new display makes
//! Peek wider, and WindowServer holds on to drawables meanwhile: a window
//! resized inside a GPUI update showed its old picture in its new frame,
//! with an empty strip, until a frame came hundreds of ms later, and a
//! window that waited for a drawable on the main thread froze the UI. Peek
//! is open on a stage display while a display is made and ended; the test
//! app's log must show no draw GPUI skipped, Peek must still draw, and the
//! new display must get the Agent Display wallpaper.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e displaychange
//!
//! Makes 2 displays: the stage and one of a size no spare has.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_screen_tests::{
    Control, capture, check, debug_pid, instance, json, now_ms, open_stage, out, pixel, run,
    sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    // The test app's own log, where GPUI says when it skipped a draw.
    let app_log = out().join("displaychange-app.log");
    // SAFETY: set before any thread starts.
    unsafe { std::env::set_var("E2E_LOG", &app_log) };
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en"), ("AGENTSCREEN_LOG_ALL", "1")])?;
        let result = test(&app_log);
        stop_test_app();
        result
    })
}

fn log_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(format!(
        "Library/Logs/Agent Screen/displays-{}.log",
        instance().unwrap_or_default()
    ))
}

fn read_from(file: &Path, at: u64) -> String {
    let bytes = std::fs::read(file).unwrap_or_default();
    String::from_utf8_lossy(&bytes[(at as usize).min(bytes.len())..]).to_string()
}

fn test(app_log: &Path) -> Result<()> {
    let control = Control::connect()?;
    let pid = debug_pid().context("no test app")?;
    open_stage(&control)?;
    let before: Vec<u64> = windows_of(pid).iter().map(|w| w.id).collect();
    // Hovering the menu bar icon opens Peek and holds it while it lasts.
    control.call(
        "debugShow",
        json!({ "surface": "icon-hover", "displayId": 1 }),
    )?;
    let peek = windows_of(pid)
        .into_iter()
        .find(|w| !before.contains(&w.id))
        .context("Peek didn't open")?;

    let from = std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0);
    // A size no spare has, so the display is new; Peek grows a card.
    let width = 1000 + 8 * ((now_ms() / 1000 % 50) as u64);
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "change", "title": "Change", "width": width, "height": 700 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    sleep_ms(2500);
    let grown = windows_of(pid)
        .into_iter()
        .find(|w| w.id == peek.id)
        .context("Peek closed")?;
    // Peek still draws out to its right edge, at the size it grew to.
    let shot = capture(
        "displaychange-peek",
        [grown.x, grown.y, grown.width, grown.height],
    )?;
    control.call(
        "endDisplay",
        json!({ "sessionKey": "change", "displayId": id }),
    )?;
    sleep_ms(1500);

    // Opening Peek counts too: the whole run.
    let skipped: Vec<String> = read_from(app_log, 0)
        .lines()
        .filter(|l| l.contains("already borrowed"))
        .map(String::from)
        .collect();
    check(
        "Peek draws at its new size at once as it opens and resizes, no draw skipped",
        skipped.is_empty(),
        format!("{} skipped: {skipped:#?}", skipped.len()),
    );
    let stalls: Vec<String> = read_from(&log_path(), from)
        .lines()
        .filter(|l| l.contains("main thread busy"))
        .map(String::from)
        .collect();
    // A frame AppKit asks for inside its transaction, as a resize does,
    // takes its drawable on the main thread, as GPUI does: while displays
    // change every window on the Mac stops drawing anyway (changecost).
    let waited = stalls
        .iter()
        .filter(|l| l.contains("Drawable") || l.contains("semaphore"))
        .count();
    println!(
        "     main thread waited for a drawable {waited} times, of {} stalls",
        stalls.len()
    );
    // Every new display gets its desktop picture, whichever way it came.
    let labelled = read_from(app_log, 0).contains(&format!("display {id}: desktop picture true"));
    check(
        "the new display gets the Agent Display wallpaper",
        labelled,
        format!(
            "no desktop picture line for display {id} in {}",
            app_log.display()
        ),
    );
    // The new display's card shows its picture, the blue of the wallpaper:
    // a window frozen on the frame it drew while resizing still says
    // "Waiting for picture…" on gray.
    let card = pixel(&shot, grown.width * 0.5, grown.height * 0.3);
    check(
        "Peek keeps drawing after it resized: the new display's card shows its picture",
        card.len() >= 3 && card[2] - card[0] > 20.0,
        format!("pixel {card:?} in {}", shot.display()),
    );
    let right = pixel(&shot, grown.width - 40.0, grown.height / 2.0);
    check(
        "Peek grew and draws out to its right edge",
        // Peek's gray, not black and not the wallpaper behind an empty
        // window.
        grown.width > peek.width
            && right.len() >= 3
            && right[0] > 25.0
            && (right[2] - right[0]).abs() < 10.0,
        format!(
            "Peek {} wide, then {}; pixel {right:?} in {}",
            peek.width,
            grown.width,
            shot.display()
        ),
    );
    control.call(
        "debugShow",
        json!({ "surface": "icon-hover", "displayId": 0 }),
    )?;
    Ok(())
}
