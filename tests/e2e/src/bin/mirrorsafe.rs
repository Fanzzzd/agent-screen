//! A new display that lands on the user's screen goes at once, and the
//! next create_display makes none (#220).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e mirrorsafe
//!
//! One empty display, no app launches or pointer input. Fault injection
//! reports the new display as landed on the user's screen, so the release
//! path runs without macOS mirroring anything or resizing the user's screen.

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
        start_test_app(&[("AGENTSCREEN_TEST_LANDED_ON_USER", "1")])?;
        let result = test();
        stop_test_app();
        result
    })
}

/// Include other instances and the closet: none of their frames may change.
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

fn display_log() -> String {
    std::env::var("HOME")
        .ok()
        .and_then(|home| {
            std::fs::read_to_string(
                std::path::PathBuf::from(home).join("Library/Logs/Agent Screen/displays-e2e.log"),
            )
            .ok()
        })
        .unwrap_or_default()
}

/// The display log's lines written since it was `from` bytes long.
fn log_since(from: usize) -> Vec<String> {
    display_log()
        .get(from..)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn refused_with_do_not_retry(reply: &agent_screen_tests::control::Reply) -> bool {
    reply.result.is_none()
        && reply.error.as_ref().is_some_and(|e| {
            e.code == "failed" && e.message.contains("Do not retry create_display")
        })
}

fn test() -> Result<()> {
    let before = online()?;
    let control = Control::connect()?;
    let start = display_log().len();

    let asked = Instant::now();
    let reply = control.send(
        "createDisplay",
        json!({ "sessionKey": "landed-on-user", "width": 1280, "height": 800 }),
    )?;
    let took = asked.elapsed();
    check(
        "a display on the user's screen fails create_display and tells the agent not to retry",
        refused_with_do_not_retry(&reply),
        format!("{reply:?}"),
    );
    let lines = log_since(start);
    let created = lines.iter().filter(|l| l.contains(" created ")).count();
    let released_at_once = lines
        .iter()
        .any(|l| l.contains(" released ") && l.contains("on the user's screen"));
    check(
        "the display is released as on the user's screen, not after parking fails",
        created == 1 && released_at_once && !lines.iter().any(|l| l.contains("parking failed")),
        format!("{lines:#?}"),
    );
    // Parking used to try for about 10 s before it gave up (#220).
    check(
        "create_display answers within 5 s",
        took < Duration::from_secs(5),
        format!("{} ms", took.as_millis()),
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
        "every existing display keeps its frame",
        after == before,
        format!("before {before:?}, after {after:?}"),
    );

    let again = display_log().len();
    let reply = control.send(
        "createDisplay",
        json!({ "sessionKey": "landed-on-user-retry", "width": 1280, "height": 800 }),
    )?;
    let lines = log_since(again);
    check(
        "the retry is refused without making a display",
        refused_with_do_not_retry(&reply) && !lines.iter().any(|l| l.contains(" created ")),
        format!("{reply:?} {lines:#?}"),
    );
    check(
        "the displays are as before the retry",
        online()? == before,
        "a display came or went",
    );
    Ok(())
}
