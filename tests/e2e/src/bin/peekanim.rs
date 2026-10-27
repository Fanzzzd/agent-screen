//! Peek shows a tile for "+" at once and grows it in, with the card
//! following in steps at the display's rate, not one jump and not GPUI's
//! 30 fps for windows that aren't key, never two frames late in a row: the new display
//! is made only once the tile has grown in, as making one stops every
//! window on the Mac from drawing for a moment. A closed display's tile
//! shrinks out the same way, as it last looked, and once it has, Peek
//! stays at its new width:
//! neither the card it draws nor its window grows back for a frame. Peek is
//! open on a stage display while the shell makes a display, as a click on
//! "+" does, and then dismisses it. The stage recording of Peek is kept.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e peekanim
//!
//! Makes 2 displays: the stage and the new one.

use std::path::Path;
use std::process::ExitCode;

use agent_screen_tests::{
    Control, Frame, capture, check, cut_stage, debug_pid, instance, json, now_ms, open_stage, out,
    record_stage, run, sleep_ms, start_test_app, stop_test_app, windows_of,
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

/// How far the tile `kind` of display `id` had grown in, 0 to 1, each
/// frame from `from` on, from the test app's log.
fn tile(log: &Path, kind: &str, id: u64, from: u128) -> Vec<f64> {
    let key = format!("peek {kind} {id} ");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (shown, at) = l.split(&key).nth(1)?.split_once(" at ")?;
            (at.trim().parse::<u128>().ok()? >= from).then_some(shown.parse::<f64>().ok()?)
        })
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

/// The first width in `seen` larger than one before it, past `slack`.
fn regrowth(seen: &[f64], slack: f64) -> Option<(f64, f64)> {
    let mut least = f64::INFINITY;
    for &w in seen {
        if w > least + slack {
            return Some((least, w));
        }
        least = least.min(w);
    }
    None
}

/// Peek's window frame, read as fast as the probe answers, for `ms`.
fn watch_window(pid: u32, id: u64, ms: u128) -> std::thread::JoinHandle<Vec<(u128, Frame)>> {
    std::thread::spawn(move || {
        let start = now_ms();
        let mut seen = Vec::new();
        while now_ms() - start < ms {
            if let Some(w) = windows_of(pid).into_iter().find(|w| w.id == id) {
                seen.push((now_ms(), w));
            }
        }
        seen
    })
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
    let pid = debug_pid().context("no test app")?;
    let stage = open_stage(&control)?;
    // Watched, as a screen someone looks at: macOS slows the display link
    // of a display nothing shows to 20 Hz.
    let video = record_stage(&control, stage)?;
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
    let created = asked;
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
    // "+" grows a waiting tile, which the new display takes the place of.
    let mut came = tile(log, "pending", 0, created);
    came.extend(tile(log, "tile-in", id, created));
    check(
        "the new display's tile grows in once, not again when the card stops sliding",
        regrowth(&came.iter().map(|t| -t).collect::<Vec<_>>(), 0.01).is_none(),
        format!("{came:?}"),
    );
    let wide = grew.last().map_or(0., |&(_, w)| w);
    let asked = now_ms();
    let frames = watch_window(pid, peek.id, 2500);
    control.call(
        "debugShow",
        json!({ "surface": "dismiss-confirm", "displayId": id }),
    )?;
    let shrank = drawn(log, asked, 2500);
    let frames = frames.join().unwrap_or_default();
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
    let widths: Vec<f64> = shrank.iter().map(|&(_, w)| w).collect();
    check(
        "the card Peek draws never grows back while the closed display's tile shrinks out",
        regrowth(&widths, 0.5).is_none(),
        format!("grew back {:?}: {shrank:?}", regrowth(&widths, 0.5)),
    );
    let as_drawn = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&format!("peek leaving {id} as drawn")))
        .all(|l| l.ends_with("true"));
    check(
        "the closed display's tile shrinks out with its picture and label, not as a blank box",
        as_drawn,
        "a leaving tile was drawn blank",
    );
    let went = tile(log, "tile-out", id, asked);
    check(
        "the closed display's tile shrinks out once, not again when the card stops sliding",
        regrowth(&went, 0.01).is_none() && went.last().is_some_and(|&t| t < 0.01),
        format!("{went:?}"),
    );
    let sized: Vec<(u128, f64)> = frames.iter().map(|(t, w)| (t - asked, w.width)).collect();
    let mut changes = sized.clone();
    changes.dedup_by(|a, b| a.1 == b.1);
    // The window first spans the old and new card, then takes the new one.
    let narrowest = sized.iter().map(|&(_, w)| w).fold(f64::INFINITY, f64::min);
    let back = sized
        .iter()
        .skip_while(|&&(_, w)| w > narrowest + 0.5)
        .find(|&&(_, w)| w > narrowest + 0.5);
    check(
        "Peek's window doesn't widen again once it has shrunk",
        back.is_none(),
        format!("window widths (ms, width): {changes:?}"),
    );
    sleep_ms(1000);
    let after = windows_of(pid)
        .into_iter()
        .find(|w| w.id == peek.id)
        .context("Peek closed")?;
    let shot = capture(
        "peekanim-after",
        [after.x, after.y, after.width, after.height],
    )?;
    println!(
        "     after: Peek {} wide, in {}",
        after.width,
        shot.display()
    );
    control.call(
        "debugShow",
        json!({ "surface": "icon-hover", "displayId": 0 }),
    )?;
    let clip = out().join("peekanim-peek.mp4");
    // The widest the window was, which holds the card throughout.
    let widest = frames
        .iter()
        .map(|(_, w)| w)
        .max_by(|a, b| a.width.total_cmp(&b.width))
        .unwrap_or(&after);
    let region = [widest.x, widest.y, widest.width, widest.height];
    cut_stage(&control, stage, &video, region, &clip, 1200)?;
    println!("     clip: {}", clip.display());
    Ok(())
}
