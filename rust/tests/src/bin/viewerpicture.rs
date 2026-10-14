//! The Viewer shows the display it is open on, and keeps up with it. With
//! several virtual displays connected, a ScreenCaptureKit stream can carry
//! another virtual display's picture, such as AeroSpace's closet with the
//! user's hidden windows; the app then shows screenshots instead. That
//! happens only now and then, so a second round makes the app take every
//! frame for a misrouted one (SIDESCREEN_STAND_IN=1). The Viewer opens on
//! a stage display, so this runs at any time.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e viewerpicture

use std::process::ExitCode;

use anyhow::{Context, Result};
use sidescreen_tests::{
    Control, DEBUG_BINARY, Rect, capture, check, instance, json, open_stage, pid_of, probe,
    quit_app, run, running, sh_lenient, skip, sleep_ms, start_test_app, stop_test_app,
};

/// Mean RGB difference below which two captures show the same picture.
const SAME: i64 = 12;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running, and may be the user's");
    }
    run(|| {
        for forced in [false, true] {
            let env: &[(&str, &str)] = if forced {
                &[("SIDESCREEN_LANG", "en"), ("SIDESCREEN_STAND_IN", "1")]
            } else {
                &[("SIDESCREEN_LANG", "en")]
            };
            start_test_app(env)?;
            let result = round(forced);
            quit_app("Calculator");
            stop_test_app();
            result?;
        }
        Ok(())
    })
}

/// The Viewer's picture against a capture of the display itself, and
/// whether screenshots stood in for the stream.
fn compare(control: &Control, id: u64, name: &str) -> Result<(i64, bool)> {
    let viewer = control.call("debugViewer", json!({}))?;
    let image: Rect = serde_json::from_value(viewer["image"].clone())?;
    let bounds: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split(' ')
        .filter_map(|v| v.parse().ok())
        .collect();
    let shown = capture(&format!("{name}-viewer"), image)?;
    let display = capture(
        &format!("{name}-display"),
        [bounds[0], bounds[1], bounds[2], bounds[3]],
    )?;
    let difference = probe(&["differ", shown.to_str().unwrap(), display.to_str().unwrap()])?
        .parse()
        .context("differ")?;
    Ok((difference, viewer["standIn"] == true))
}

fn round(forced: bool) -> Result<()> {
    let control = Control::connect()?;
    open_stage(&control)?;
    let display = control.call(
        "createDisplay",
        json!({
            "sessionKey": "viewerpicture",
            "client": "Claude Code",
            "title": "Viewer picture",
            "width": 1280,
            "height": 800,
        }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(2500);
    let tag = if forced { "stand-in" } else { "stream" };
    let way = |stand_in: bool| {
        if stand_in {
            "screenshots"
        } else {
            "the stream"
        }
    };
    let (empty, stand_in) = compare(&control, id, &format!("viewerpicture-{tag}-empty"))?;
    let label = if forced {
        "screenshots stand in, and the Viewer shows its display"
    } else {
        "the Viewer shows its display"
    };
    check(
        label,
        empty < SAME && (stand_in || !forced),
        format!("mean difference {empty}, through {}", way(stand_in)),
    );

    control.call("launchApp", json!({ "displayId": id, "app": "Calculator" }))?;
    sleep_ms(2500);
    let (changed, stand_in) = compare(&control, id, &format!("viewerpicture-{tag}-calculator"))?;
    check(
        "it keeps up as the display changes",
        changed < SAME,
        format!("mean difference {changed}, through {}", way(stand_in)),
    );
    if let Some(pid) = pid_of(DEBUG_BINARY) {
        println!(
            "     app CPU over 3 s with the Viewer open, through {}: {:.0}%",
            way(stand_in),
            cpu_percent(pid, 3)
        );
    }
    Ok(())
}

/// CPU time a process used over `seconds`, as a share of one core.
fn cpu_percent(pid: u32, seconds: u64) -> f64 {
    let used = || {
        // "M:SS.ss", or "H:MM:SS" past an hour.
        sh_lenient("/bin/ps", &["-o", "time=", "-p", &pid.to_string()])
            .split(':')
            .fold(0.0, |total, part| {
                total * 60.0 + part.parse::<f64>().unwrap_or(0.0)
            })
    };
    let before = used();
    sleep_ms(seconds * 1000);
    (used() - before) / seconds as f64 * 100.0
}
