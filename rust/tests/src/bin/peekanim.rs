//! Peek shows a tile for "+" at once and grows it in, with the card
//! following in steps at the display's rate, not one jump and not GPUI's
//! 30 fps for windows that aren't key, never two frames late in a row: the new display
//! is made only once the tile has grown in, as making one stops every
//! window on the Mac from drawing for a moment. A closed display's tile
//! shrinks out the same way. Peek is open on a stage display while the shell makes a
//! display, as a click on "+" does, and then dismisses it.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e peekanim
//!
//! Makes 2 displays: the stage and the new one.

use std::path::Path;
use std::process::ExitCode;

use agent_screen_tests::{
    Control, DEBUG_BINARY, check, instance, json, now_ms, open_stage, out, pid_of, record_stage,
    run, sleep_ms, start_test_app, stop_test_app, windows_of,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    // The test app's log, where Peek notes each width it draws at.
    let log = out().join("peekanim-app.log");
    // SAFETY: set before any thread starts.
    unsafe { std::env::set_var("E2E_LOG", &log) };
    run(|| {
        // No spares made meanwhile: the display "+" makes is new.
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_IDLE_SECS", "1000000"),
        ])?;
        let result = test(&log);
        stop_test_app();
        result
    })
}

/// The widths Peek drew at from `from` on, with when, from the test app's
/// log, `ms` later.
fn drawn(log: &Path, from: u128, ms: u64) -> Vec<(u128, f64)> {
    sleep_ms(ms);
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let rest = l.split("peek drawn ").nth(1)?;
            let (width, at) = rest.split_once(" at ")?;
            Some((at.trim().parse::<u128>().ok()?, width.parse::<f64>().ok()?))
        })
        .filter(|&(at, _)| at >= from)
        .collect()
}

/// Widths strictly between the first and the last.
fn steps(seen: &[(u128, f64)], from: f64) -> Vec<f64> {
    let Some(&(_, to)) = seen.last() else {
        return Vec::new();
    };
    let (low, high) = (from.min(to), from.max(to));
    seen.iter()
        .map(|&(_, w)| w)
        .filter(|&w| w > low + 0.5 && w < high - 0.5)
        .collect()
}

/// The longest wait between two drawn widths over the animation, from the
/// first: its last pixel's ease-out tail can come later.
fn longest_wait(seen: &[(u128, f64)]) -> u128 {
    let start = seen.first().map_or(0, |&(t, _)| t);
    // RESIZE, 260 ms.
    let during: Vec<&(u128, f64)> = seen
        .iter()
        .take_while(|&&(t, _)| t - start <= 270)
        .collect();
    during
        .windows(2)
        .map(|p| p[1].0 - p[0].0)
        .max()
        .unwrap_or(0)
}

fn newest_display() -> Option<u64> {
    let log = std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("HOME").ok()?)
            .join("Library/Logs/Agent Screen/displays-e2e.log"),
    )
    .ok()?;
    log.lines()
        .rev()
        .find(|l| l.contains(" created display=") || l.contains(" reused display="))?
        .split("display=")
        .nth(1)?
        .split(' ')
        .next()?
        .parse()
        .ok()
}

fn test(log: &Path) -> Result<()> {
    let control = Control::connect()?;
    let pid = pid_of(DEBUG_BINARY).context("no test app")?;
    let stage = open_stage(&control)?;
    // Watched, as a screen someone looks at: macOS slows the display link
    // of a display nothing shows to 20 Hz.
    record_stage(&control, stage)?;
    let before: Vec<u64> = windows_of(pid).iter().map(|w| w.id).collect();
    control.call(
        "debugShow",
        json!({ "surface": "icon-hover", "displayId": 1 }),
    )?;
    sleep_ms(600);
    let peek = windows_of(pid)
        .into_iter()
        .find(|w| !before.contains(&w.id))
        .context("Peek didn't open")?;

    let asked = now_ms();
    control.call("debugShow", json!({ "surface": "create" }))?;
    let grew = drawn(log, asked, 2500);
    let first = grew
        .iter()
        .find(|&&(_, w)| w > peek.width + 0.5)
        .map(|&(t, _)| t - asked);
    check(
        "a tile for the new display shows at once, before macOS has made it",
        first.is_some_and(|t| t < 100),
        format!("Peek drawn wider after {first:?} ms"),
    );
    let growing = steps(&grew, peek.width);
    check(
        "Peek grows in steps at the display's rate as the tile grows in",
        growing.len() >= 8 && longest_wait(&grew) < 40,
        format!(
            "{} steps, longest wait {} ms: {grew:?}",
            growing.len(),
            longest_wait(&grew)
        ),
    );

    let id = newest_display().context("no display made")?;
    sleep_ms(1500);
    let wide = grew.last().map_or(0., |&(_, w)| w);
    let asked = now_ms();
    control.call(
        "debugShow",
        json!({ "surface": "dismiss-confirm", "displayId": id }),
    )?;
    let shrank = drawn(log, asked, 2500);
    let shrinking = steps(&shrank, wide);
    check(
        "Peek shrinks in steps at the display's rate as the closed display's tile shrinks out",
        shrinking.len() >= 8
            && shrank.last().is_some_and(|&(_, w)| w < wide)
            && longest_wait(&shrank) < 40,
        format!(
            "{} steps, longest wait {} ms: {shrank:?}",
            shrinking.len(),
            longest_wait(&shrank)
        ),
    );
    control.call(
        "debugShow",
        json!({ "surface": "icon-hover", "displayId": 0 }),
    )?;
    Ok(())
}
