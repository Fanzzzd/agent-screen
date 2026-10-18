//! A display takes the lowest serial no other display of the app holds.
//! macOS keeps a ColorSync record for every display serial it has seen and
//! reads all of them once per online display whenever displays change; past
//! 50 ms WindowServer asks again every 5 s, for good. Reused serials keep
//! that list as long as the most displays held at once.
//!
//! Three displays end: two stay as spares, the third goes. Two displays of
//! new sizes take the spares, resized; the next display must get the third
//! one's serial.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e serials

use std::process::ExitCode;

use anyhow::{Context, Result};
use sidescreen_tests::{
    Control, check, instance, json, probe, run, sleep_ms, start_test_app, stop_test_app,
};

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

fn create(control: &Control, key: &str, width: u32, height: u32) -> Result<u64> {
    control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": key, "width": width, "height": height }),
    )?["displayId"]
        .as_u64()
        .context("display id")
}

fn display_log() -> Result<String> {
    Ok(std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var("HOME")?)
            .join("Library/Logs/Sidescreen/displays-e2e.log"),
    )?)
}

/// The serial the display log gave display `id` when it was made.
fn serial_of(id: u64) -> Result<Option<u32>> {
    Ok(display_log()?
        .lines()
        .rev()
        .find(|line| line.contains(&format!("created display={id} ")))
        .and_then(|line| line.split("serial=").nth(1))
        .and_then(|serial| serial.trim().parse().ok()))
}

fn test() -> Result<()> {
    // A connection is one Agent Session, so each display gets its own.
    let sizes = [(1280, 800), (1200, 750), (1100, 700)];
    let mut made = Vec::new();
    for (i, (width, height)) in sizes.into_iter().enumerate() {
        let control = Control::connect()?;
        let key = format!("s{i}");
        let id = create(&control, &key, width, height)?;
        made.push((control, key, id, serial_of(id)?));
    }
    let serials: Vec<_> = made.iter().map(|(_, _, _, s)| *s).collect();
    check(
        "the first displays count up from the instance's first serial",
        serials == [Some(10_001), Some(10_002), Some(10_003)],
        format!("{serials:?}"),
    );
    for (control, key, id, _) in &made {
        control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    }
    // An ended display goes once its windows have; wait for the third.
    let third = made[2].2;
    let gone = (0..50).any(|_| {
        sleep_ms(100);
        display_log().is_ok_and(|log| log.contains(&format!("released display={third} ")))
    });
    check(
        "the third display is released",
        gone,
        format!("display {third}"),
    );
    // The two kept spares take any size: a mode change, no new display.
    let spares: Vec<u64> = made[..2].iter().map(|(_, _, id, _)| *id).collect();
    let mut taking = Vec::new();
    for (i, (width, height)) in [(1000, 640), (1040, 660)].into_iter().enumerate() {
        let control = Control::connect()?;
        let key = format!("t{i}");
        let id = create(&control, &key, width, height)?;
        let bounds = probe(&["bounds", &id.to_string()])?;
        let size: Vec<f64> = bounds
            .split_whitespace()
            .skip(2)
            .filter_map(|n| n.parse().ok())
            .collect();
        check(
            "a spare of another size takes the new size",
            spares.contains(&id) && size == [width as f64, height as f64],
            format!("display {id} of spares {spares:?}, now {bounds}"),
        );
        let shot = control.call("screenshot", json!({ "sessionKey": key, "displayId": id }))?;
        let (w, h) = (
            shot["width"].as_f64().unwrap_or(0.0),
            shot["height"].as_f64().unwrap_or(0.0),
        );
        check(
            "its screenshot has the new shape",
            w > 0.0 && (w / h - width as f64 / height as f64).abs() < 0.01,
            format!("{w}×{h}"),
        );
        taking.push((control, key, id));
    }
    let control = Control::connect()?;
    let next = create(&control, "next", 1000, 640)?;
    let serial = serial_of(next)?;
    check(
        "the next display takes the serial the released one freed",
        serial == Some(10_003),
        format!("display {next}, serial {serial:?}"),
    );
    control.call(
        "endDisplay",
        json!({ "sessionKey": "next", "displayId": next }),
    )?;
    for (control, key, id) in &taking {
        control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    }
    Ok(())
}
