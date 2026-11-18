//! Real keys, through the input method the user has selected, typed into
//! the Viewer during a Takeover (ADR 0008). `viewerime` plays the input
//! method's part itself; this script presses the keys at the HID level, as
//! a keyboard does, after a real click on the Viewer's picture, so the
//! selected pinyin input method, WeType or Apple's Pinyin, composes in the
//! Viewer as it does for the user. It types "nihao" and picks the first
//! candidate four times: with 1 after a click, with 1 after a second
//! click, with Space straight after that commit, and with 1 typed at once
//! after a Return, whose letters wait for the focus lookup. Each time the
//! Viewer is the key window, every letter goes to the input method and
//! none to Field as a key, Field gets the candidate's Chinese text and
//! never composes by itself, and the input method's candidate window stays
//! up at the caret the Viewer gives it, on the user's screen, never on the
//! Agent Display, without jumping.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerwetype
//!
//! It takes the pointer and the keyboard for about 10 s: the Viewer opens
//! on your screen and becomes the key window, and your front app comes back
//! after. `imetype` stops and presses Escape when you touch the keyboard or
//! pointer. The script skips unless a pinyin input method is selected,
//! never changes the input source, and types nothing once the source is a
//! keyboard layout. It fails when the input method is in its English mode.
//! Makes 1 display. Needs Field.app, which `cargo e2e ime` makes.
//!
//! After a Return, Field reports its caret through the line break before
//! it, at the end of the line above, and the candidates show there; the
//! script checks them against the caret the Viewer reports, not the line.

use std::process::{ExitCode, Stdio};

use agent_screen_tests::{
    Control, check, command, debug_pid, field_app, hid, instance, json, kill_ours, locked, now_ms,
    our_binary, out, own_input_now, probe, run, sign_step, skip, sleep_ms, start_test_app,
    stop_test_app, tool, wait_for_idle,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;

const KEY: &str = "viewerwetype";
/// n, i, h, a, o on a US keyboard.
const LETTERS: [u16; 5] = [45, 34, 4, 0, 31];
const ONE: u16 = 18;
const SPACE: u16 = 49;
const RETURN: u16 = 36;
const ESCAPE: u16 = 53;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let source = probe(&["source"]).unwrap_or_default();
    let pinyin = source.to_lowercase().contains("pinyin")
        || source.starts_with("com.apple.inputmethod.SCIM");
    if !pinyin {
        skip(&format!(
            "select a pinyin input method (WeType or Pinyin - Simplified) first; the input source is {source}"
        ));
    }
    if locked() {
        skip("the screen is locked, and this test types with the real keyboard");
    }
    run(|| {
        let field = field_app()?;
        // A fresh app, so its log is this run's.
        stop_test_app();
        let log = std::env::var("E2E_LOG")
            .unwrap_or_else(|_| out().join("viewerwetype-app.log").display().to_string());
        unsafe { std::env::set_var("E2E_LOG", &log) };
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let pointer = hid(&["where"])?;
        let front = probe(&["frontpid"])?;
        let mut typed = false;
        let result = test(&control, &field, &source, &mut typed);
        // A composition left in the Viewer, or anywhere the keys went, is
        // cancelled, so no letter stays typed.
        let left = control.call("debugViewer", json!({})).ok();
        let composing = left.as_ref().is_none_or(|v| v["marked"].is_string());
        if typed && (result.is_err() || composing) {
            let _ = hid(&["key", &ESCAPE.to_string()]);
        }
        let _ = control.call("debugShow", json!({ "surface": "collapse" }));
        kill_ours(our_binary("Field"));
        if let Some((x, y)) = pointer.split_once(' ') {
            let _ = hid(&["warp", x, y]);
        }
        if probe(&["frontpid"])? != front {
            probe(&["activate", front.trim()])?;
        }
        println!("     the app's log: {log}");
        stop_test_app();
        result
    })
}

/// `[x, y, width, height]` from a JSON array.
fn rect(v: &Value) -> Option<[f64; 4]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
        a.get(3)?.as_f64()?,
    ])
}

fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// The gap between two rects, 0 when they touch or overlap.
fn gap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let dx = (b[0] - (a[0] + a[2])).max(a[0] - (b[0] + b[2])).max(0.0);
    let dy = (b[1] - (a[1] + a[3])).max(a[1] - (b[1] + b[3])).max(0.0);
    dx.hypot(dy)
}

/// One word typed and picked.
struct Case {
    name: &'static str,
    /// Click Field's text first, and wait for the focus lookup.
    click: bool,
    /// Press Return first, and type at once after it.
    after_return: bool,
    pick: u16,
}

/// A window of the input method on screen: its number and its frame.
type Shown = (u64, [f64; 4]);

/// What `imetype` wrote: when the first letter and the pick went down, and
/// the input method's windows on screen at each listing.
#[derive(Default)]
struct Typed {
    first_key: u128,
    pick: u128,
    listings: Vec<(u128, Vec<Shown>)>,
    abort: Option<String>,
    tap: bool,
}

fn parse(file: &std::path::Path) -> Typed {
    let mut typed = Typed::default();
    let (mut letters, mut picking) = (false, false);
    for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let t = v["t"].as_u64().unwrap_or(0) as u128;
        if v["tap"].as_bool() == Some(true) {
            typed.tap = true;
        }
        if let Some(why) = v["abort"].as_str() {
            typed.abort = Some(why.into());
        }
        match v["mark"].as_str() {
            Some("letters") => letters = true,
            Some("pick") => picking = true,
            _ => {}
        }
        if v["key"].is_number() {
            if picking && typed.pick == 0 {
                typed.pick = t;
            } else if letters && typed.first_key == 0 {
                typed.first_key = t;
            }
        }
        if let Some(windows) = v["windows"].as_array() {
            let shown = windows
                .iter()
                .filter_map(|w| {
                    Some((
                        w["id"].as_u64()?,
                        [
                            w["x"].as_f64()?,
                            w["y"].as_f64()?,
                            w["w"].as_f64()?,
                            w["h"].as_f64()?,
                        ],
                    ))
                })
                .collect();
            typed.listings.push((t, shown));
        }
    }
    typed
}

fn test(control: &Control, field: &std::path::Path, source: &str, typed: &mut bool) -> Result<()> {
    let saved = out().join("viewerwetype-field.txt");
    let _ = std::fs::remove_file(&saved);
    kill_ours(our_binary("Field"));
    let (width, height) = (900.0, 600.0);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Viewer WeType", "width": width, "height": height }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": field.to_str().unwrap(),
                "environment": { "FIELD_OUT": saved.to_str().unwrap() } }),
    )?;
    sleep_ms(800);
    let windows = control.call("listWindows", json!({ "sessionKey": KEY, "displayId": id }))?;
    let window = windows["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["title"] == "Field"))
        .context(format!("no Field window: {windows}"))?;
    let number = |name: &str| window[name].as_f64().unwrap_or(0.0);
    let agent_display: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let agent_display: [f64; 4] = agent_display
        .try_into()
        .ok()
        .context("the display's bounds")?;
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    let viewer = || control.call("debugViewer", json!({})).unwrap_or_default();
    let shown = viewer();
    let frame = rect(&shown["frame"]).context("no Viewer frame")?;
    let image = rect(&shown["image"]).context("no Viewer image")?;
    // Field's text view fills its content, 300 points high under the title
    // bar: a point 100 points down it, in the Viewer's picture on the
    // user's screen.
    let content_top = number("y") + number("height") - 300.0;
    let at = (
        image[0] + (number("x") + number("width") / 2.0) / width * image[2],
        image[1] + (content_top + 100.0) / height * image[3],
    );
    // Field's line: its text, and the text its own text view has marked.
    let read = || {
        let line = std::fs::read_to_string(&saved).unwrap_or_default();
        let mut parts = line.split('\t').map(str::to_string);
        (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default().trim().to_string(),
        )
    };
    let imetype = tool("imetype")?;
    sign_step("typing with your keyboard / 正在使用你的键盘，请勿操作");
    if !wait_for_idle() {
        bail!("the screen is locked");
    }
    let cases = [
        Case {
            name: "1 after a click",
            click: true,
            after_return: false,
            pick: ONE,
        },
        Case {
            name: "1 after a second click",
            click: true,
            after_return: false,
            pick: ONE,
        },
        Case {
            name: "Space after a commit",
            click: false,
            after_return: false,
            pick: SPACE,
        },
        Case {
            name: "1 at once after Return",
            click: false,
            after_return: true,
            pick: ONE,
        },
    ];
    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        if case.click {
            hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
            // The click's focus lookup, so the letters go straight on.
            for _ in 0..15 {
                if viewer()["secrecy"] == "open" {
                    break;
                }
                sleep_ms(100);
            }
        }
        let before = viewer();
        // Never type unless the keyboard is the test app's: the keys would
        // go to the user's own window.
        let front = probe(&["frontpid"])?;
        if debug_pid().map(|pid| pid.to_string()) != Some(front.clone()) {
            check(
                &format!("{name}: the test app has the keyboard"),
                false,
                format!("the front app is {front}"),
            );
            bail!("the test app is not in front; nothing typed");
        }
        check(
            &format!("{name}: the Viewer is the key window"),
            before["key"] == true,
            format!("key {}", before["key"]),
        );
        check(
            &format!("{name}: the Viewer's input context is the current one"),
            before["inputContext"] == true,
            format!("inputContext {}", before["inputContext"]),
        );
        // The user may have switched since the script began.
        if before["source"] != "inputMethod" {
            bail!(
                "{name}: the input source is now a keyboard layout ({}); nothing typed",
                probe(&["source"]).unwrap_or_default()
            );
        }
        let routes_before = before["routes"].as_array().map_or(0, Vec::len);
        let (text_before, _) = {
            sleep_ms(150);
            read()
        };
        let mut steps: Vec<String> = Vec::new();
        if case.after_return {
            steps.extend([format!("k:{RETURN}"), "w:30".into()]);
        }
        steps.push("m:letters".into());
        for code in LETTERS {
            steps.extend([format!("k:{code}"), "w:90".into()]);
        }
        steps.extend([
            "w:400".into(),
            "m:pick".into(),
            format!("k:{}", case.pick),
            "w:400".into(),
        ]);
        let events = out().join(format!("viewerwetype-{index}.jsonl"));
        *typed = true;
        let mut child = command(&imetype)
            .arg(&events)
            .args(&steps)
            .stdout(Stdio::null())
            .spawn()?;
        // Field meanwhile: every text it held, and any text it marked. And
        // the caret the Viewer gives the input method, once it composes.
        let mut held: Vec<(u128, String, String)> = Vec::new();
        let mut caret = None;
        let status = loop {
            own_input_now();
            let now = read();
            if held
                .last()
                .is_none_or(|(_, t, m)| (t, m) != (&now.0, &now.1))
            {
                held.push((now_ms(), now.0, now.1));
            }
            if caret.is_none() {
                let now = viewer();
                if now["marked"].is_string() {
                    caret = rect(&now["caret"]);
                }
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            sleep_ms(40);
        };
        own_input_now();
        sleep_ms(400);
        let (text, _) = read();
        let after = viewer();
        let typed = parse(&events);
        if let Some(why) = &typed.abort {
            bail!("{name}: stopped, {why}; Escape pressed");
        }
        check(
            &format!("{name}: the keys were typed"),
            status.success(),
            format!("imetype {status}"),
        );
        let routes: Vec<(u64, String)> = after["routes"]
            .as_array()
            .map(|r| {
                r.iter()
                    .skip(routes_before)
                    .map(|r| {
                        (
                            r[0].as_u64().unwrap_or(0),
                            r[1].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        println!("     {name}: routes {routes:?}");
        let keys = LETTERS.len() + 1 + usize::from(case.after_return);
        check(
            &format!("{name}: every key reached the Viewer"),
            routes.len() == keys,
            format!("{} of {keys}: {routes:?}", routes.len()),
        );
        let letters: Vec<_> = routes
            .iter()
            .filter(|(code, _)| LETTERS.contains(&(*code as u16)))
            .collect();
        check(
            &format!("{name}: every letter went to the input method, none to Field as a key"),
            letters.len() == LETTERS.len()
                && letters.iter().all(|(_, route)| route == "inputMethod"),
            format!("{letters:?}"),
        );
        // Through the current context, the input method gave every letter
        // back to type as it is: WeType does that in its English mode.
        if before["inputContext"] == true
            && !letters.is_empty()
            && letters.iter().all(|(_, route)| route == "raw")
        {
            println!(
                "     {name}: the input method passed every letter through, as in its English mode (Shift switches it)"
            );
        }
        let picked = routes
            .iter()
            .rev()
            .find(|(code, _)| *code == u64::from(case.pick));
        check(
            &format!("{name}: the pick sent the input method's text"),
            picked.is_some_and(|(_, route)| route == "text"),
            format!("{picked:?}"),
        );
        let prefix = if case.after_return {
            format!("{text_before}\n")
        } else {
            text_before.clone()
        };
        let got = text.strip_prefix(&prefix).map(str::to_string);
        println!("     {name}: Field {text_before:?} → {text:?}");
        let chinese =
            |t: &str| !t.is_empty() && t.chars().all(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
        check(
            &format!("{name}: Field got the candidate's Chinese text, and no letter"),
            got.as_deref().is_some_and(chinese),
            format!("{text_before:?} → {text:?}"),
        );
        let stray: Vec<_> = held
            .iter()
            .filter(|(_, t, m)| {
                !m.is_empty()
                    || t.strip_prefix(&text_before)
                        .is_none_or(|t| t.chars().any(|c| c.is_ascii_alphanumeric()))
            })
            .map(|(_, t, m)| format!("{t:?} marked {m:?}"))
            .collect();
        check(
            &format!("{name}: Field never held a letter nor composed by itself"),
            stray.is_empty(),
            stray.join(", "),
        );
        check(
            &format!("{name}: no composition was dropped for a password field"),
            after["secureFound"] == before["secureFound"],
            format!("{} → {}", before["secureFound"], after["secureFound"]),
        );
        check(
            &format!("{name}: the composition ended"),
            after["marked"].is_null(),
            format!("marked {}", after["marked"]),
        );
        // The candidate window: a window of the input method that was not
        // up before the first letter, which leaves out its mode badge or
        // toolbar. From 250 ms after the first letter, by when the input
        // method has drawn it, to the pick.
        let standing: Vec<u64> = typed
            .listings
            .iter()
            .filter(|(t, _)| *t < typed.first_key)
            .flat_map(|(_, shown)| shown.iter().map(|(id, _)| *id))
            .collect();
        let candidates = |shown: &[Shown]| -> Vec<[f64; 4]> {
            shown
                .iter()
                .filter(|(id, frame)| {
                    !standing.contains(id) && frame[2] >= 40.0 && frame[3] >= 20.0
                })
                .map(|(_, frame)| *frame)
                .collect()
        };
        let composing: Vec<Vec<[f64; 4]>> = typed
            .listings
            .iter()
            .filter(|(t, _)| *t >= typed.first_key + 250 && *t < typed.pick)
            .map(|(_, shown)| candidates(shown))
            .collect();
        let up: Vec<[f64; 4]> = composing.iter().flatten().copied().collect();
        let gone = composing.iter().filter(|shown| shown.is_empty()).count();
        let span = |i: usize| {
            let values = up.iter().map(|r| r[i]);
            values.clone().fold(f64::MIN, f64::max) - values.fold(f64::MAX, f64::min)
        };
        let moved = if up.is_empty() {
            (0.0, 0.0)
        } else {
            (span(0), span(1))
        };
        println!(
            "     {name}: candidates up in {} of {} listings, first at {:?}, moved {:.0} x {:.0} pt{}",
            composing.len() - gone,
            composing.len(),
            up.first(),
            moved.0,
            moved.1,
            if typed.tap {
                ""
            } else {
                "; imetype could not watch for your input"
            },
        );
        // Apple's Pinyin draws its candidates in another process: nothing
        // to watch then.
        if up.is_empty() && !source.contains("wetype") {
            println!("     {name}: no candidate window of {source} seen; its place is not checked");
            continue;
        }
        check(
            &format!("{name}: the candidates stayed up while the letters were typed"),
            !up.is_empty() && gone <= 1,
            format!("gone in {gone} of {} listings", composing.len()),
        );
        let on_display: Vec<[f64; 4]> = typed
            .listings
            .iter()
            .flat_map(|(_, shown)| candidates(shown))
            .filter(|r| intersects(*r, agent_display))
            .collect();
        check(
            &format!("{name}: the candidates never showed on the Agent Display"),
            on_display.is_empty(),
            format!("{:?} on {agent_display:?}", on_display.first()),
        );
        // The caret the Viewer gave the input method, on the user's screen.
        let caret = caret
            .or_else(|| rect(&before["caret"]))
            .map(|c| [frame[0] + c[0], frame[1] + c[1], c[2], c[3]]);
        let far = up
            .iter()
            .map(|r| caret.map_or(f64::MAX, |c| gap(*r, c)))
            .fold(0.0, f64::max);
        check(
            &format!("{name}: the candidates sat at the target's caret in the Viewer"),
            !up.is_empty() && far <= 80.0,
            format!(
                "{far:.0} pt from the caret {caret:?}, candidates {:?}",
                up.first()
            ),
        );
        check(
            &format!("{name}: the candidates did not jump"),
            !up.is_empty() && moved.0 <= 12.0 && moved.1 <= 12.0,
            format!("moved {:.0} x {:.0} pt", moved.0, moved.1),
        );
    }
    let last = viewer();
    println!(
        "     held keys waited {} ms; lookups took {} ms",
        last["heldMs"], last["lookupMs"]
    );
    control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }))?;
    Ok(())
}
