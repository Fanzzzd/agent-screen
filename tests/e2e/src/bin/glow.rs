//! The Red Glow (ADR 0013, ADR 0008): the red light along the edges of
//! your screens while an agent has, or may take, your pointer or keyboard.
//! Previewed Live, it shows one click-through window per user screen at
//! level 1000, none on an Agent Display, its bands rasterized and
//! breathing, and its alpha meets the contract in
//! `crates/model/src/glow.rs` on each screen: by distance from the glass's
//! edge at the middle of each edge, under the notch and into the top
//! corners, with the centre untouched. Armed meets it times 0.55 and
//! holds still; Off leaves no window. A screenshot of the Agent Display
//! during the preview has no red rim. A preview grant is Armed and goes
//! Off within a frame of the Input Bar's Stop. The Input Bar for a
//! Keyboard Borrow shows above the glow's level. A real click at 4 pt from
//! your main screen's left edge reaches the script's own window there,
//! through the glow.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e glow
//!
//! Shows the glow on your screens for about half a minute, and moves your
//! pointer for one click near the left edge of your main screen: keep your
//! hands off the Mac. Makes 1 display.

use std::process::ExitCode;

use agent_screen_model::glow::{self, CONTRACT, FOLLOW, Glow};
use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, debug_pid, hid, hidclick_app, instance, json,
    kill_ours, locked, our_binary, out, probe, run, sh, skip, sleep_ms, start_test_app,
    stop_test_app, windows_of,
};
use anyhow::{Context, Result};
use base64::Engine;

const KEY: &str = "glow";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        if locked() {
            skip("the screen is locked");
        }
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let pid = debug_pid().context("no test app")?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Glow", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let result = test(&control, &id, pid);
        let _ = show(&control, &id, "glow-off");
        kill_ours(our_binary("HIDClick"));
        let _ = control.call(
            "inputQueue",
            json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
        );
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        stop_test_app();
        result
    })
}

fn show(control: &Control, id: &Value, surface: &str) -> Result<Value> {
    control.call(
        "debugShow",
        json!({ "sessionKey": KEY, "surface": surface, "displayId": id }),
    )
}

/// The user's screens, by the window server: not an Agent Display, a
/// Spare or AeroSpace's closet.
fn user_screens() -> Result<usize> {
    let script = r#"ObjC.import("AppKit"); ObjC.import("CoreGraphics");
var s = $.NSScreen.screens, n = 0;
for (var i = 0; i < s.count; i++) {
  var id = ObjC.unwrap(s.objectAtIndex(i).deviceDescription.objectForKey("NSScreenNumber"));
  var vendor = $.CGDisplayVendorNumber(id), model = $.CGDisplayModelNumber(id);
  if (vendor != 0x5353 && !(vendor == 0x4145 && model == 1)) n++;
}
n"#;
    Ok(
        sh("/usr/bin/osascript", &["-l", "JavaScript", "-e", script])?
            .trim()
            .parse()?,
    )
}

fn key(d: f64) -> String {
    format!("{d:.0}")
}

fn alpha(screen: &Value, place: &str, d: f64) -> Option<f64> {
    screen["alpha"][place][key(d)].as_f64()
}

/// The glow's windows, as `debugGlow` reads them, against the contract for
/// `state`: each screen's windows and, read from its capture, the alpha
/// by distance in from the glass's edge.
fn judge(label: &str, reply: &Value, state: Glow, screens: usize) {
    let all = reply["screens"].as_array().cloned().unwrap_or_default();
    check(
        &format!("{label}: state {}", state.name()),
        reply["state"] == state.name(),
        reply["state"].to_string(),
    );
    check(
        &format!(
            "{label}: one visible window per user screen ({screens}), click-through, at level 1000"
        ),
        all.len() == screens
            && all
                .iter()
                .all(|s| s["visible"] == true && s["ignoresMouse"] == true && s["level"] == 1000),
        cut(reply),
    );
    check(
        &format!("{label}: no glow window on an Agent Display, a Spare or the closet"),
        reply["onAgentDisplays"] == 0,
        reply["onAgentDisplays"].to_string(),
    );
    check(
        &format!("{label}: the bands are rasterized"),
        reply["rasterized"] == true,
        reply["rasterized"].to_string(),
    );
    for screen in &all {
        let name = format!("{label}, display {}", screen["displayId"]);
        let mut table = Vec::new();
        let mut ok = true;
        for place in ["left", "right", "top", "bottom"] {
            let mut row = format!("{place:>6}:");
            for c in CONTRACT {
                let a = alpha(screen, place, c.0);
                row += &format!(
                    " {}={}",
                    key(c.0),
                    a.map_or("-".into(), |a| format!("{a:.3}"))
                );
                ok &= a.is_some_and(|a| glow::meets(c, a, state));
            }
            table.push(row);
        }
        let want: Vec<String> = CONTRACT
            .iter()
            .filter_map(|&c| {
                glow::expected(c, state).map(|(a, t)| format!("{}={a:.3}±{t:.3}", key(c.0)))
            })
            .collect();
        check(
            &format!("{name}: the alpha meets the contract at the middle of each edge"),
            ok,
            format!("want {}\n{}", want.join(" "), table.join("\n")),
        );
        // At d = 8, the glow follows the glass into the corners and under
        // the notch.
        let edge = ["left", "right", "top", "bottom"]
            .iter()
            .filter_map(|p| alpha(screen, p, 8.0))
            .sum::<f64>()
            / 4.0;
        let scale = if state == Glow::Armed {
            glow::ARMED.pulse
        } else {
            1.0
        };
        let follows: Vec<String> = ["corner", "cornerRight", "notch"]
            .iter()
            .filter_map(|p| alpha(screen, p, 8.0).map(|a| (p, a)))
            .map(|(p, a)| format!("{p} {a:.3}"))
            .collect();
        check(
            &format!("{name}: into the top corners and under the notch the glow follows the glass"),
            ["corner", "cornerRight", "notch"].iter().all(|p| {
                screen["alpha"][p].is_null()
                    || alpha(screen, p, 8.0).is_some_and(|a| (a - edge).abs() <= FOLLOW * scale)
            }) && !follows.is_empty(),
            format!("edge {edge:.3}: {}", follows.join(", ")),
        );
        check(
            &format!("{name}: the screen's centre is untouched"),
            screen["alpha"]["centre"].as_f64() == Some(0.0),
            screen["alpha"]["centre"].to_string(),
        );
        println!(
            "     {name}, scale {}:\n{}",
            screen["scale"],
            table.join("\n")
        );
    }
}

fn cut(v: &Value) -> String {
    agent_screen_tests::cut(&v.to_string(), 600)
}

/// Pixels in the outer 4 points of the Agent Display's screenshot that
/// read as the glow's red.
fn red_rim(control: &Control, id: &Value) -> Result<(usize, usize)> {
    let shot = control.call("screenshot", json!({ "displayId": id, "format": "png" }))?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(shot["data"].as_str().context("image data")?)?;
    let path = out().join("glow-agent-display.png");
    std::fs::write(&path, data)?;
    let size = probe(&["imagesize", path.to_str().unwrap()]).unwrap_or_default();
    let (w, h) = size
        .split_once(' ')
        .and_then(|(w, h)| Some((w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?)))
        .unwrap_or((1280.0, 800.0));
    let mut points = Vec::new();
    for i in 1..20 {
        let t = f64::from(i) / 20.0;
        points.extend([
            (2.0, t * h),
            (w - 3.0, t * h),
            (t * w, 2.0),
            (t * w, h - 3.0),
        ]);
    }
    let red = points
        .iter()
        .filter(|(x, y)| {
            let px: Vec<f64> = probe(&[
                "pixel",
                path.to_str().unwrap(),
                &x.to_string(),
                &y.to_string(),
            ])
            .unwrap_or_default()
            .split(' ')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
            px.len() == 3 && px[0] > px[1] + 60.0 && px[0] > px[2] + 60.0
        })
        .count();
    Ok((red, points.len()))
}

/// Each glow window alone, under target/e2e, for a person to look at: no
/// other window's content is in it.
fn picture(pid: u32, name: &str) {
    for (n, w) in windows_of(pid)
        .iter()
        .filter(|w| w.layer == 1000)
        .enumerate()
    {
        let file = out().join(format!("{name}-{n}.png"));
        let _ = sh(
            "/usr/sbin/screencapture",
            &["-x", "-o", "-l", &w.id.to_string(), file.to_str().unwrap()],
        );
    }
}

fn test(control: &Control, id: &Value, pid: u32) -> Result<()> {
    let screens = user_screens()?;
    let read = || control.call("debugGlow", json!({}));

    show(control, id, "glow-live")?;
    let live = read()?;
    judge("Live", &live, Glow::Live, screens);
    picture(pid, "glow-live");
    let calm = live["reduceMotion"] == true;
    check(
        "Live breathes, unless macOS reduces motion",
        calm || live["animations"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "breath")),
        live["animations"].to_string(),
    );
    let (red, of) = red_rim(control, id)?;
    check(
        "a screenshot of the Agent Display during the glow has no red rim",
        red == 0,
        format!("{red} of {of} points in its outer 4 pt read red"),
    );

    show(control, id, "glow-armed")?;
    let armed = read()?;
    judge("Armed", &armed, Glow::Armed, screens);
    picture(pid, "glow-armed");
    // A window built again just now still fades in, for 120 ms.
    let still = || -> Result<Value> {
        let first = armed["animations"].clone();
        if first.as_array().is_some_and(Vec::is_empty) {
            return Ok(first);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        Ok(control.call("debugGlow", json!({ "alpha": false }))?["animations"].clone())
    };
    let still = still()?;
    check(
        "Armed holds still: no animation on its bands",
        still.as_array().is_some_and(Vec::is_empty),
        still.to_string(),
    );

    show(control, id, "glow-off")?;
    let off = read()?;
    check(
        "glow-off: state off, no glow window visible",
        off["state"] == "off"
            && off["preview"].is_null()
            && off["screens"]
                .as_array()
                .is_none_or(|s| s.iter().all(|s| s["visible"] == false))
            && windows_of(pid).iter().all(|w| w.layer != 1000),
        cut(&off),
    );

    // A preview grant, as the card's Allow: Armed, with nothing to run.
    // The Input Bar's Stop ends it, and the glow goes within a frame.
    show(control, id, "input-queue-approved")?;
    let granted = control.call("debugGlow", json!({ "alpha": false }))?;
    check(
        "an open grant with nothing running is Armed",
        granted["state"] == "armed",
        granted["state"].to_string(),
    );
    show(control, id, "input-bar-stop")?;
    let stopped = control.call("debugGlow", json!({ "alpha": false }))?;
    // Armed while cleanup puts the pointer and front back, then off at
    // once.
    let ms = stopped["stopToOffMs"].as_f64();
    let restore = stopped["restoreMs"].as_f64().unwrap_or(0.0);
    check(
        "the Input Bar's Stop turns the glow off within a frame of the end of cleanup",
        stopped["state"] == "off" && ms.is_some_and(|ms| ms - restore <= 17.0),
        format!(
            "state {}, {ms:?} ms from Stop to off, {restore} ms of it restoring",
            stopped["state"]
        ),
    );
    let _ = control.call(
        "inputQueue",
        json!({ "sessionKey": KEY, "displayId": id, "action": "done" }),
    );

    // The Input Bar as during a Keyboard Borrow, just above the glow's
    // level; its Stop takes it away.
    let bar = || {
        windows_of(pid)
            .into_iter()
            .find(|w| w.layer == 1001 && w.height < 120.0)
    };
    show(control, id, "input-bar-keyboard")?;
    let shown = bar();
    if let Some(b) = &shown {
        let _ =
            agent_screen_tests::capture("glow-input-bar-keyboard", [b.x, b.y, b.width, b.height]);
    }
    show(control, id, "input-bar-stop")?;
    check(
        "during a Keyboard Borrow the Input Bar shows above the glow's level, and its Stop takes it away",
        shown.is_some() && bar().is_none(),
        format!("{shown:?} then {:?}", bar()),
    );

    click_through(control, id)
}

/// A real click at 4 pt from the main screen's left edge, through the
/// glow, reaches the script's own window there.
fn click_through(control: &Control, id: &Value) -> Result<()> {
    let counts = out().join("glow-hidclick.txt");
    let _ = std::fs::remove_file(&counts);
    let app = hidclick_app()?;
    allow_on_your_screen(
        "HIDClick",
        15_000,
        "the strip the click through the glow lands on",
    );
    let _strip = std::process::Command::new(our_binary("HIDClick"))
        .env("HIDCLICK_EDGE", "left")
        .env("HIDCLICK_OUT", &counts)
        .spawn()
        .with_context(|| format!("starting {}", app.display()))?;
    let mut strip = None;
    let mut strip_pid = None;
    for _ in 0..50 {
        strip_pid = agent_screen_tests::pid_of(our_binary("HIDClick").to_str().unwrap());
        strip = strip_pid
            .map(windows_of)
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.x == 0.0 && w.width == 60.0);
        if strip.is_some() {
            break;
        }
        sleep_ms(200);
    }
    let Some(strip) = strip else {
        check(
            "the script's strip shows at the main screen's left edge",
            false,
            format!("{strip_pid:?}"),
        );
        return Ok(());
    };
    let pointer = hid(&["where"])?;
    show(control, id, "glow-live")?;
    let live = control.call("debugGlow", json!({ "alpha": false }))?;
    let y = strip.y + strip.height / 2.0;
    hid(&["click", "4", &y.to_string()])?;
    sleep_ms(400);
    let seen = std::fs::read_to_string(&counts).unwrap_or_default();
    kill_ours(our_binary("HIDClick"));
    show(control, id, "glow-off")?;
    let at: Vec<&str> = pointer.split_whitespace().collect();
    if let [x, y] = at[..] {
        let _ = hid(&["warp", x, y]);
    }
    check(
        "a click at 4 pt from the screen's edge goes through the Live glow to the window under it",
        live["state"] == "live" && seen.contains("down=1 "),
        format!("glow {}, strip says {seen:?}", live["state"]),
    );
    Ok(())
}
