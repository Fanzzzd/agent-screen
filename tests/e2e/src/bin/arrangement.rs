//! New displays park in a compact grid off the top-right corner of the
//! user's displays (`arrangement::plan`): the first touches them only at
//! that corner, the next sits beside it while the row is no wider than the
//! user's displays. The user's displays and other instances' don't move.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e arrangement
//!
//! Makes 2 displays, one at a time.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, run, sh, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        stop_test_app();
        result
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    id: u64,
    vendor: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Every online display, AeroSpace's closet left out.
fn online() -> Result<Vec<Shown>> {
    let out = sh(
        "/usr/bin/swift",
        &[
            "-e",
            "import CoreGraphics
var ids = [CGDirectDisplayID](repeating: 0, count: 32); var n: UInt32 = 0
CGGetOnlineDisplayList(32, &ids, &n)
for d in ids.prefix(Int(n)) { let b = CGDisplayBounds(d)
print(d, CGDisplayVendorNumber(d), CGDisplayModelNumber(d), b.origin.x, b.origin.y, b.width, b.height) }",
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let v: Vec<f64> = l
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect();
            let [id, vendor, model, x, y, width, height] = v[..] else {
                return None;
            };
            let closet = vendor as u32 == 0x4145 && model as u32 == 1;
            (!closet).then_some(Shown {
                id: id as u64,
                vendor: vendor as u32,
                x,
                y,
                width,
                height,
            })
        })
        .collect())
}

fn create(control: &Control, key: &str) -> Result<u64> {
    control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": key, "width": 1280, "height": 800 }),
    )?["displayId"]
        .as_u64()
        .context("display id")
}

fn test() -> Result<()> {
    let before = online()?;
    let users: Vec<Shown> = before
        .iter()
        .copied()
        .filter(|d| d.vendor != 0x5353)
        .collect();
    let (left, top) = users
        .iter()
        .fold((f64::MAX, f64::MAX), |(l, t), d| (l.min(d.x), t.min(d.y)));
    let right = users.iter().map(|d| d.x + d.width).fold(f64::MIN, f64::max);
    let width = right - left;

    let a = Control::connect()?;
    let first = create(&a, "a")?;
    sleep_ms(1500);
    let b = Control::connect()?;
    let second = create(&b, "b")?;
    sleep_ms(1500);
    let after = online()?;
    let at = |id: u64| after.iter().find(|d| d.id == id).copied();
    let (one, two) = (
        at(first).context("first display")?,
        at(second).context("second display")?,
    );

    for d in [one, two] {
        check(
            &format!("display {} is above and right of the user's displays", d.id),
            d.y + d.height <= top && d.x >= right,
            format!("{d:?}, user's displays span x {left}..{right} from y {top}"),
        );
    }
    check(
        "the first touches the user's displays' row only at the top-right corner",
        one.y + one.height == top,
        format!("{one:?}"),
    );
    let beside = two.x == one.x + one.width && two.y == one.y;
    let above = two.x == one.x && two.y + two.height == one.y;
    check(
        "the second sits beside the first while the row fits, else above it",
        if one.width + two.width <= width {
            beside
        } else {
            above
        },
        format!("{one:?} {two:?}, row up to {width}"),
    );
    let still: Vec<&Shown> = before
        .iter()
        .filter(|d| {
            after
                .iter()
                .any(|a| a.id == d.id && (a.x, a.y) != (d.x, d.y))
        })
        .collect();
    check(
        "no display that was there before moved",
        still.is_empty(),
        format!("moved: {still:?}"),
    );

    a.call(
        "endDisplay",
        json!({ "sessionKey": "a", "displayId": first }),
    )?;
    b.call(
        "endDisplay",
        json!({ "sessionKey": "b", "displayId": second }),
    )?;
    Ok(())
}
