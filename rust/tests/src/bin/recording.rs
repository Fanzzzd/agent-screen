//! record_display: a video of the display at its full size, and a timeline
//! of the agent's actions and the Takeovers with their offsets into it. A
//! second start is refused, and ending the display stops the recording.
//! The files go to ~/Movies/Sidescreen and are deleted at the end.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e recording

use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::Value;
use sidescreen_tests::{
    Control, check, instance, json, probe, quit_app, run, running, skip, sleep_ms, start_test_app,
    stop_test_app,
};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if running("Calculator") {
        skip("Calculator is running, and may be the user's");
    }
    run(|| {
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let mut made = Vec::new();
        let result = test(&mut made);
        quit_app("Calculator");
        stop_test_app();
        for file in made {
            let _ = std::fs::remove_file(file);
        }
        result
    })
}

fn path(reply: &Value, key: &str) -> Result<String> {
    reply[key]
        .as_str()
        .map(String::from)
        .context(format!("no {key}"))
}

fn test(made: &mut Vec<String>) -> Result<()> {
    let control = Control::connect()?;
    let key = "recording";
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Recorded", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].clone();
    control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": "Calculator" }),
    )?;

    let started = control.call("recordStart", json!({ "sessionKey": key, "displayId": id }))?;
    let (video, timeline) = (path(&started, "video")?, path(&started, "timeline")?);
    made.extend([video.clone(), timeline.clone()]);
    let again = control.call("recordStart", json!({ "sessionKey": key, "displayId": id }));
    check(
        "a second start is refused",
        again.is_err(),
        format!("{again:?}"),
    );

    sleep_ms(1000);
    control.call(
        "click",
        json!({ "sessionKey": key, "displayId": id, "x": 640, "y": 400 }),
    )?;
    sleep_ms(500);
    control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "12" }),
    )?;
    sleep_ms(500);
    control.call("debugTakeover", json!({ "displayId": id, "active": true }))?;
    sleep_ms(500);
    control.call("debugTakeover", json!({ "displayId": id, "active": false }))?;
    sleep_ms(1000);
    let stopped = control.call("recordStop", json!({ "sessionKey": key, "displayId": id }))?;
    let seconds = stopped["seconds"].as_f64().unwrap_or(0.0);
    check(
        "stop reports the recording",
        stopped["problem"].is_null() && seconds > 3.0 && stopped["actions"].as_i64() == Some(4),
        stopped.to_string(),
    );

    let lines: Vec<Value> = std::fs::read_to_string(&timeline)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let actions: Vec<&str> = lines.iter().filter_map(|l| l["action"].as_str()).collect();
    check(
        "the timeline has each action and Takeover, in order",
        actions == ["click", "type", "takeoverStart", "takeoverEnd"],
        format!("{actions:?}"),
    );
    let times: Vec<f64> = lines.iter().filter_map(|l| l["t"].as_f64()).collect();
    check(
        "each line has its seconds into the video, rising, within the video",
        times.len() == 4
            && times.windows(2).all(|w| w[0] <= w[1])
            && times[0] > 0.5
            && times[3] <= seconds,
        format!("{times:?} of {seconds} s"),
    );
    check(
        "a click keeps its coordinates and typing its text",
        lines[0]["x"] == 640 && lines[0]["y"] == 400 && lines[1]["text"] == "12",
        format!("{} {}", lines[0], lines[1]),
    );
    let info = probe(&["video", &video])?;
    let parts: Vec<f64> = info
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    check(
        "the video is as long as the recording, at the display's size",
        parts.len() == 3
            && (parts[0] - seconds).abs() < 1.5
            && parts[1] == 1280.0
            && parts[2] == 800.0,
        format!("{info} for {seconds} s"),
    );

    // Ending the display ends a recording.
    let started = control.call("recordStart", json!({ "sessionKey": key, "displayId": id }))?;
    made.extend([path(&started, "video")?, path(&started, "timeline")?]);
    sleep_ms(1500);
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    sleep_ms(2000);
    let info = probe(&["video", &path(&started, "video")?])?;
    check(
        "ending the display finishes its recording",
        info.split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|s| s > 1.0),
        info,
    );
    Ok(())
}
