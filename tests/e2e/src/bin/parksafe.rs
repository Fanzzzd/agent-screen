//! Refused parking never returns a usable Agent Display (#132).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e parksafe
//!
//! One empty display, no app launches or pointer input. Fault injection
//! refuses verification after the real configuration, so it exercises the
//! failure path without deliberately leaving a display beside the user's.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, check, instance, json, listening, run, sh, skip, sleep_ms, start_test_app,
    stop_test_app,
};
use anyhow::Result;

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        if listening() {
            skip("the e2e instance is already running; this test needs its own startup settings");
        }
        start_test_app(&[("AGENTSCREEN_TEST_PARK_REFUSED", "1")])?;
        let result = test();
        stop_test_app();
        result
    })
}

/// Include other instances and the closet: none of their origins may change.
fn online() -> Result<Vec<String>> {
    let text = sh(
        "/usr/bin/swift",
        &[
            "-e",
            "import CoreGraphics
var ids = [CGDirectDisplayID](repeating: 0, count: 64); var n: UInt32 = 0
CGGetOnlineDisplayList(64, &ids, &n)
for d in ids.prefix(Int(n)) { let b = CGDisplayBounds(d)
print(d, b.origin.x, b.origin.y, b.width, b.height) }",
        ],
    )?;
    let mut lines: Vec<_> = text.lines().map(str::to_string).collect();
    lines.sort();
    Ok(lines)
}

fn test() -> Result<()> {
    let before = online()?;
    let control = Control::connect()?;
    let reply = control.send(
        "createDisplay",
        json!({ "sessionKey": "parking-refused", "width": 1280, "height": 1600 }),
    )?;
    check(
        "unverified parking fails create_display instead of returning a display",
        reply.result.is_none()
            && reply.error.as_ref().is_some_and(|e| {
                e.code == "failed" && (e.message.contains("park") || e.message.contains("position"))
            }),
        format!("{reply:?}"),
    );
    // WindowServer removes a released virtual display asynchronously.
    let until = Instant::now() + Duration::from_secs(5);
    let after = loop {
        let after = online()?;
        if after == before || Instant::now() >= until {
            break after;
        }
        sleep_ms(100);
    };
    check(
        "the refused display is released and every existing display keeps its frame",
        after == before,
        format!("before {before:?}, after {after:?}"),
    );
    Ok(())
}
