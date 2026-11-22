//! An agent's window stays on its Agent Display while the idle app makes
//! spares through unit twins, and one macOS drops elsewhere as the displays
//! change is put back (#291). In a `diaqueue` run, a spare whose first
//! display got a taken unit number (a display made at -1280,0 beside the
//! main screen, another made, the first released) left the agent's Dia
//! window off its display, and the regather logged `put back: false` and
//! left it there.
//!
//! The test build counts you idle at once (AGENTSCREEN_IDLE_SECS=0), so a
//! tidy every 30 s (AGENTSCREEN_TIDY_SECS) makes the spares, and each
//! creation's first display counts as a unit twin
//! (AGENTSCREEN_TEST_UNIT_TWIN=1). The script makes a display and opens
//! Field on it with launch_app. Every 50 ms it notes where Field's window
//! is: it must exist the whole time, and stay on its display while the
//! first spare is made. Once that change's regather has settled
//! (`debugRegather`), the script moves the window onto the spare, as macOS
//! drops a window when the displays change, and checks it stays there
//! until the second spare's display change, whose regather must bring it
//! back. The old regather
//! counted a spare as an Agent Display and left it there, out of sight.
//! With E2E_LOG=FILE it also reads the app's log: the window is back, and
//! nothing went to your screen or vanished. E2E_DIA=1 uses a new window of
//! your running Dia in place of Field, and closes only that window.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e sparewindow
//!
//! Makes 1 display and two spares, each through a twin: 6 displays made,
//! 3 kept. Nothing shows on your screen.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, check, field_app, fixture, instance, json, kill_ours, our_binary, pid_of, probe, run,
    sh_lenient, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let dia = std::env::var("E2E_DIA").as_deref() == Ok("1");
    if dia && dia_pid().is_none() {
        skip("Dia isn't running");
    }
    run(|| {
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_IDLE_SECS", "0"),
            ("AGENTSCREEN_TEST_UNIT_TWIN", "1"),
            ("AGENTSCREEN_TIDY_SECS", "30"),
        ])?;
        let result = if dia {
            let page = format!("file://{}", fixture("web/keys.html").display());
            test("Dia", Some(&page))
        } else {
            let field = field_app()?;
            kill_ours(our_binary("Field"));
            let result = test(field.to_str().context("field path")?, None);
            kill_ours(our_binary("Field"));
            result
        };
        stop_test_app();
        result
    })
}

fn dia_pid() -> Option<i64> {
    sh_lenient("/usr/bin/pgrep", &["-x", "Dia"])
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
}

fn display_log() -> String {
    std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Library/Logs/Agent Screen/displays-e2e.log"),
    )
    .unwrap_or_default()
}

/// A display's frame in global points.
fn bounds(id: u64) -> Option<[f64; 4]> {
    let said = probe(&["bounds", &id.to_string()]).ok()?;
    let v: Vec<f64> = said
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// The window's frame, from every window of `pid`, on screen or not.
fn window(pid: i64, id: i64) -> Option<[f64; 4]> {
    probe(&["allwindows", &pid.to_string()])
        .ok()?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|w| w["id"].as_i64() == Some(id))
        .and_then(|w| {
            Some([
                w["x"].as_f64()?,
                w["y"].as_f64()?,
                w["width"].as_f64()?,
                w["height"].as_f64()?,
            ])
        })
}

fn inside(frame: [f64; 4], display: [f64; 4]) -> bool {
    let (mx, my) = (frame[0] + frame[2] / 2.0, frame[1] + frame[3] / 2.0);
    mx >= display[0]
        && mx < display[0] + display[2]
        && my >= display[1]
        && my < display[1] + display[3]
}

struct Watched {
    /// Milliseconds at which the window was in no window list.
    missing: Vec<u128>,
    /// When and where it was off its display.
    off: Vec<String>,
    /// Whether `until` held before the time ran out.
    done: bool,
}

/// Note where the window is every 50 ms, until `until` holds and `tail`
/// seconds more, or `max` passes.
fn watch(
    pid: i64,
    wid: i64,
    id: u64,
    max: Duration,
    tail: u64,
    mut until: impl FnMut() -> bool,
) -> Watched {
    let start = Instant::now();
    let mut watched = Watched {
        missing: Vec::new(),
        off: Vec::new(),
        done: false,
    };
    let mut done_at: Option<Instant> = None;
    while start.elapsed() < max {
        let t = start.elapsed().as_millis();
        match (window(pid, wid), bounds(id)) {
            (None, _) => watched.missing.push(t),
            (Some(frame), Some(display)) if !inside(frame, display) => watched
                .off
                .push(format!("{t}ms {frame:?} not on {display:?}")),
            _ => {}
        }
        if done_at.is_none() && until() {
            done_at = Some(Instant::now());
            watched.done = true;
        }
        if done_at.is_some_and(|d| d.elapsed() > Duration::from_secs(tail)) {
            break;
        }
        sleep_ms(50);
    }
    watched
}

/// The window existed the whole time and ended on its display.
fn stayed(when: &str, watched: &Watched, pid: i64, wid: i64, id: u64) {
    check(
        &format!("{when}, the agent's window exists the whole time"),
        watched.missing.is_empty(),
        format!("missing at {:?} ms", watched.missing),
    );
    let last = window(pid, wid).zip(bounds(id));
    check(
        &format!("{when}, the agent's window ends on its Agent Display"),
        last.is_some_and(|(f, d)| inside(f, d)),
        format!("{last:?}; off meanwhile: {:?}", watched.off),
    );
    if !watched.off.is_empty() {
        println!(
            "     {when}: off its display in {} samples, first {}",
            watched.off.len(),
            watched.off[0]
        );
    }
}

/// The latest regather's generation, and whether it runs.
fn regather(control: &Control) -> Result<(u64, bool)> {
    let v = control.call("debugRegather", json!({}))?;
    Ok((
        v["generation"].as_u64().context("generation")?,
        v["running"].as_bool().context("running")?,
    ))
}

/// Wait up to 40 s for a regather newer than `after` to finish: its
/// generation, or None.
fn regather_settles(control: &Control, after: u64) -> Result<Option<u64>> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(40) {
        let (generation, running) = regather(control)?;
        if generation > after && !running {
            return Ok(Some(generation));
        }
        sleep_ms(100);
    }
    Ok(None)
}

/// The display log's creations and releases since `from`.
fn changes(from: usize) {
    let log = display_log();
    let lines: Vec<&str> = log[from.min(log.len())..]
        .lines()
        .filter(|l| {
            [" created ", " released ", " kept ", " parked "]
                .iter()
                .any(|k| l.contains(k))
        })
        .collect();
    println!("     display changes:\n       {}", lines.join("\n       "));
}

fn app_log() -> String {
    std::env::var("E2E_LOG")
        .ok()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default()
}

/// `app` on the display, showing `page` when it is a browser. A Dia
/// window the script opened is closed at the end; only that one.
fn test(app: &str, page: Option<&str>) -> Result<()> {
    let control = Control::connect()?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": "sparewindow", "title": "Spare window", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let mut launch = json!({ "sessionKey": "sparewindow", "displayId": id, "app": app });
    if let Some(page) = page {
        launch["open"] = json!(page);
    }
    let launched = control.call("launchApp", launch)?;
    let wid = launched["window"]["windowId"]
        .as_i64()
        .context("launched window id")?;
    let pid = launched["window"]["pid"]
        .as_i64()
        .or_else(|| match page {
            Some(_) => dia_pid(),
            None => pid_of(our_binary("Field").to_str()?).map(i64::from),
        })
        .context("the app's pid")?;
    let from_app = app_log().len();
    let from = display_log().len();
    let spares = move || -> Vec<u64> {
        let log = display_log();
        log[from.min(log.len())..]
            .lines()
            .filter(|l| l.contains("spare made while idle"))
            .filter_map(|l| {
                l.split("kept display=")
                    .nth(1)?
                    .split(' ')
                    .next()?
                    .parse()
                    .ok()
            })
            .collect()
    };
    // The first spare, made through a unit twin: a display made, another
    // made, the first released, the second parked.
    let watched = watch(pid, wid, id, Duration::from_secs(60), 5, || {
        !spares().is_empty()
    });
    changes(from);
    let twins = || {
        let log = display_log();
        log[from.min(log.len())..]
            .lines()
            .filter(|l| l.contains(" unit twin") || l.ends_with(" filler"))
            .count()
    };
    check(
        "the idle app makes a spare through a unit twin while the agent's window is open",
        watched.done && twins() >= 1,
        format!("spares {:?}, twins released {}", spares(), twins()),
    );
    stayed("as the first spare was made", &watched, pid, wid, id);
    let Some(&spare) = spares().first() else {
        bail!("no spare");
    };
    // The first change's regather must be over first, or it, not the
    // second spare's, would bring the window back. A tidy every 30 s
    // (AGENTSCREEN_TIDY_SECS) leaves room between them.
    let settled = regather_settles(&control, 0)?;
    check(
        "the first spare's regather settles before the second spare",
        settled.is_some() && spares().len() == 1,
        format!("regather {settled:?}, spares {:?}", spares()),
    );
    let Some(first) = settled else {
        bail!("the regather never settled");
    };
    // As macOS drops a window elsewhere when the displays change: the
    // agent's window on the spare, which no session shows. The next
    // display change, the second spare, must bring it back.
    let onto = bounds(spare).context("the spare's frame")?;
    let moved = control.call(
        "debugMoveWindow",
        json!({ "pid": pid, "windowId": wid, "x": onto[0] + 100.0, "y": onto[1] + 100.0 }),
    )?;
    sleep_ms(500);
    let there = window(pid, wid);
    check(
        "the agent's window is moved onto the spare",
        moved["moved"] == true && there.is_some_and(|f| inside(f, onto)),
        format!("{moved} | window {there:?}, spare {onto:?}"),
    );
    // Nothing brings it back before the next display change.
    sleep_ms(1500);
    let (generation, _) = regather(&control)?;
    let still = window(pid, wid);
    check(
        "the window stays on the spare until the displays change again",
        generation == first && still.is_some_and(|f| inside(f, onto)),
        format!("regather {generation} (was {first}), window {still:?}"),
    );
    let watched = watch(pid, wid, id, Duration::from_secs(60), 1, || {
        spares().len() >= 2
    });
    let after = regather_settles(&control, first)?;
    check(
        "the idle app makes a second spare through a unit twin, and its regather settles",
        watched.done && twins() >= 2 && after.is_some(),
        format!(
            "spares {:?}, twins released {}, regather {after:?}",
            spares(),
            twins()
        ),
    );
    stayed("as the second spare was made", &watched, pid, wid, id);
    changes(from);
    if std::env::var("E2E_LOG").is_ok() {
        let log = app_log();
        let lines: Vec<&str> = log[from_app.min(log.len())..]
            .lines()
            .filter(|l| {
                l.contains("as the displays changed")
                    || l.contains("after the displays changed")
                    || l.contains("tries to put it back")
            })
            .collect();
        println!("     regather log:\n       {}", lines.join("\n       "));
        check(
            "the regather put the window back, and handed nothing to your screen",
            lines
                .iter()
                .any(|l| l.contains(&format!("window {wid} is back on display {id}")))
                && !lines
                    .iter()
                    .any(|l| l.contains("tries to put it back") || l.contains("is gone after")),
            format!("{lines:#?}"),
        );
    }
    if page.is_some() && window(pid, wid).is_some() {
        let said = probe(&["close", &pid.to_string(), &wid.to_string()]).unwrap_or_default();
        println!("     closing the script's Dia window: {}", said.trim());
        sleep_ms(1000);
    }
    let _ = control.call(
        "endDisplay",
        json!({ "sessionKey": "sparewindow", "displayId": id }),
    );
    Ok(())
}
