//! The Viewer shows the display it is open on. ScreenCaptureKit tells
//! displays apart by unit number, and WindowServer can give a new virtual
//! display the unit AeroSpace's closet has: captures of it then show the
//! closet, the user's hidden windows. The app makes such a display again
//! until it gets a unit of its own. On this Mac the second display after
//! the stage used to get the closet's unit, so the test makes two.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e viewerpicture

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Context, Result};
use agent_screen_tests::{
    Control, Rect, capture, check, instance, json, open_stage, probe, run, sleep_ms,
    start_test_app, stop_test_app,
};

/// Mean RGB difference below which two captures show the same picture.
const SAME: i64 = 12;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let result = test();
        stop_test_app();
        result
    })
}

/// Unit number of every online display.
fn units() -> Result<HashMap<u64, u64>> {
    Ok(probe(&["units"])?
        .lines()
        .filter_map(|l| {
            let (id, unit) = l.split_once(' ')?;
            Some((id.parse().ok()?, unit.parse().ok()?))
        })
        .collect())
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    open_stage(&control)?;
    let mut ids = Vec::new();
    for key in ["first", "second"] {
        let display = control.call(
            "createDisplay",
            json!({ "sessionKey": key, "title": key, "width": 1280, "height": 800 }),
        )?;
        ids.push(display["displayId"].as_u64().context("display id")?);
    }
    let units = units()?;
    for &id in &ids {
        let unit = units.get(&id).copied().context("unit")?;
        let twins: Vec<u64> = units
            .iter()
            .filter(|&(&other, &u)| other != id && u == unit)
            .map(|(&other, _)| other)
            .collect();
        check(
            &format!("display {id} has a unit number of its own"),
            twins.is_empty(),
            format!("unit {unit}, shared with {twins:?}"),
        );
    }
    for &id in &ids {
        control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
        sleep_ms(2500);
        let viewer = control.call("debugViewer", json!({}))?;
        let image: Rect = serde_json::from_value(viewer["image"].clone())?;
        let bounds: Vec<f64> = probe(&["bounds", &id.to_string()])?
            .split(' ')
            .filter_map(|v| v.parse().ok())
            .collect();
        let shown = capture(&format!("viewerpicture-{id}-viewer"), image)?;
        let display = capture(
            &format!("viewerpicture-{id}-display"),
            [bounds[0], bounds[1], bounds[2], bounds[3]],
        )?;
        let difference: i64 =
            probe(&["differ", shown.to_str().unwrap(), display.to_str().unwrap()])?
                .parse()
                .context("differ")?;
        check(
            &format!("the Viewer shows display {id}, by its frames' screen rect"),
            viewer["misrouted"] == false && difference < SAME,
            format!(
                "misrouted {}, mean difference {difference}",
                viewer["misrouted"]
            ),
        );
    }
    Ok(())
}
