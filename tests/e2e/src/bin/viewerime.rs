//! The user's input method composes in the Viewer during a Takeover, and
//! the target gets only the finished text (issue #117, ADR 0008). The
//! script makes the input method's own calls on the Viewer's view,
//! setMarkedText and insertText, as pinyin makes them while the user types
//! "nihao" and picks 你好: Field gets nothing while the text is marked,
//! then exactly 你好, and the marked text sits at Field's caret. Keys typed
//! with a plain keyboard layout, keys with no text and Command shortcuts
//! go to Field as keys. A composition left when the Takeover ends is
//! dropped at once, even when the agent's request card is up, and text the
//! input method commits later never comes back; a click commits it. A
//! key the input method passes through, as in a view with no text, goes as
//! the key itself. In Field's password field keys skip the input method. A
//! field on an Electron page composes from the first word. With a keyboard
//! layout the Viewer never looks up the target's focus, and no key waits
//! on a lookup. The input method's part is played by the script, through
//! the view's own NSTextInputClient calls. Everything goes through the
//! Viewer's own handlers, so the user's keyboard, pointer and input source
//! are untouched.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerime
//!
//! Makes 2 displays: the stage and the one the Viewer shows. Needs
//! Field.app, which `cargo e2e ime` makes; the Electron part runs when
//! target/e2e/Electron.app is there.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, allow_on_your_screen, capture, check, field_app, fixture, instance, json, kill_ours,
    open_stage, our_binary, out, run, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use serde_json::Value;

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let field = field_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&field);
        kill_ours(our_binary("Field"));
        kill_ours(fixture("electron"));
        stop_test_app();
        result
    })
}

/// What Field holds: its text, its marked text, its password field and
/// the keys its key view got.
#[derive(Debug, Default, PartialEq)]
struct Field {
    text: String,
    marked: String,
    password: String,
    keys: String,
    down: String,
}

fn test(field: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let key = "viewerime";
    let saved = out().join("viewerime-field.txt");
    let _ = std::fs::remove_file(&saved);
    // Field writes its line ten times a second.
    let read = || {
        sleep_ms(400);
        let line = std::fs::read_to_string(&saved).unwrap_or_default();
        let mut parts = line.split('\t').map(str::to_string);
        Field {
            text: parts.next().unwrap_or_default(),
            marked: parts.next().unwrap_or_default(),
            password: parts.next().unwrap_or_default(),
            keys: parts.next().unwrap_or_default(),
            down: parts.next().unwrap_or_default().trim().to_string(),
        }
    };
    // Field once it holds what is wanted, within 2 s; for text on its way.
    let until = |want: &dyn Fn(&Field) -> bool| {
        let mut now = read();
        for _ in 0..8 {
            if want(&now) {
                break;
            }
            now = read();
        }
        now
    };
    let holds = |text: &str| Field {
        text: text.into(),
        ..Field::default()
    };
    kill_ours(our_binary("Field"));
    open_stage(&control)?;
    let (width, height) = (900.0, 600.0);
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Viewer IME", "width": width, "height": height }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": field.to_str().unwrap(),
                "environment": { "FIELD_OUT": saved.to_str().unwrap(), "FIELD_CONTROLS": "1" } }),
    )?;
    sleep_ms(800);
    let windows = control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
    let window = windows["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["title"] == "Field"))
        .context(format!("no Field window: {windows}"))?;
    let number = |name: &str| window[name].as_f64().unwrap_or(0.0);
    // Field's content is 300 points high under its title bar: the text
    // view on top, the password field 232 points down, the key view 276.
    let content_top = number("y") + number("height") - 300.0;
    let at = |down: f64| {
        json!({
            "kind": "click",
            "x": (number("x") + number("width") / 2.0) / width,
            "y": (content_top + down) / height,
            "source": "inputMethod",
        })
    };
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    let input = |params: Value| control.call("debugViewerInput", params);
    let viewer = || control.call("debugViewer", json!({})).unwrap_or_default();
    let marked = || viewer()["marked"].as_str().map(str::to_string);
    // A key typed with `source`, the input method doing `ime` with it:
    // what the key came to.
    // What the Viewer knew of the focus when it routed the last key.
    let routed_by = std::cell::RefCell::new(String::new());
    let typed_key = |code: u16, characters: &str, source: &str, ime: &str| -> String {
        let mut key =
            json!({ "kind": "key", "code": code, "characters": characters, "source": source });
        if !ime.is_empty() {
            key["ime"] = json!(ime);
        }
        let reply = input(key);
        *routed_by.borrow_mut() = reply
            .as_ref()
            .map(|r| r["secrecy"].to_string())
            .unwrap_or_default();
        reply.map_or_else(|e| e.to_string(), |r| r["route"].to_string())
    };
    // The marked text once it is `want`, or as it is after 2 s.
    let until_marked = |want: Option<&str>| -> Option<String> {
        for _ in 0..40 {
            if marked().as_deref() == want {
                break;
            }
            sleep_ms(50);
        }
        marked()
    };

    // The user clicks Field's text in the Viewer with a keyboard layout: a
    // Takeover, and no lookup of the focus.
    let inspections = || viewer()["inspections"].as_u64();
    let before = inspections();
    let mut click = at(100.0);
    click["source"] = json!("layout");
    input(click)?;
    sleep_ms(600);
    check(
        "a click with a keyboard layout looks nothing up",
        before.is_some() && inspections() == before,
        format!("{before:?} then {:?}", inspections()),
    );
    let typed = control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "x" }),
    );
    check(
        "the click took the display over",
        typed.is_err(),
        format!("{typed:?}"),
    );
    // With an input method, the user clicks the field and types at once,
    // before the click's lookup can answer: the letter waits for it, then
    // pinyin marks it, and Field gets nothing.
    input(at(100.0))?;
    let from = routes(&control).len();
    let routed = typed_key(45, "n", "inputMethod", "mark:n");
    let shown = until_marked(Some("n"));
    let went = routes_after(&control, from, 1);
    check(
        "the first letter typed at once after a click composes",
        went == [(45, "inputMethod".to_string())] && shown.as_deref() == Some("n"),
        format!("{routed} then {went:?}: {shown:?}"),
    );
    for text in ["ni", "ni h", "ni hao"] {
        input(json!({ "kind": "mark", "text": text }))?;
    }
    check(
        "the Viewer shows the marked text",
        marked().as_deref() == Some("ni hao"),
        format!("{:?}", marked()),
    );
    check(
        "Field gets nothing while the text is marked",
        read() == holds(""),
        format!("{:?}", read()),
    );
    // The marked text and the input method's candidates sit at Field's
    // caret, at the start of its first line: under the title bar, not at
    // the click lower down.
    let shown = viewer();
    let rect = |name: &str| -> Vec<f64> {
        shown[name]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default()
    };
    let (frame, image, caret) = (rect("frame"), rect("image"), rect("caret"));
    let on_display = (caret.len() == 4 && image.len() == 4).then(|| {
        (
            (frame[0] + caret[0] - image[0]) / image[2] * width,
            (frame[1] + caret[1] + caret[3] / 2.0 - image[1]) / image[3] * height,
        )
    });
    check(
        "the caret is at the start of Field's first line, under its title bar",
        on_display.is_some_and(|(cx, cy)| {
            cx >= number("x")
                && cx < number("x") + number("width") / 3.0
                && cy >= number("y") + 28.0
                && cy < content_top + 60.0
        }),
        format!("{on_display:?} in {window}"),
    );
    if frame.len() == 4 {
        let shot = capture("viewerime-marked", [frame[0], frame[1], frame[2], frame[3]])?;
        println!("     the Viewer while composing: {}", shot.display());
    }
    // Return commits 你好: Field gets it as text.
    let routed = typed_key(36, "\r", "inputMethod", "insert:你好");
    check(
        "Return that commits sends the composed text",
        routed == "\"text\"",
        &routed,
    );
    let now = until(&|f| *f == holds("你好"));
    check(
        "Field gets exactly 你好 after the commit",
        now == holds("你好"),
        format!("{now:?}"),
    );
    check(
        "the Viewer shows no marked text after the commit",
        marked().is_none(),
        format!("{:?}", marked()),
    );

    // A plain layout: keys go to Field as keys, as before.
    for (code, characters) in [(0, "a"), (11, "b")] {
        let routed = typed_key(code, characters, "layout", "");
        check(
            "a key typed with a plain layout goes to Field as a key",
            routed == "\"raw\"",
            &routed,
        );
    }
    let now = until(&|f| *f == holds("你好ab"));
    check(
        "Field shows 你好ab",
        now == holds("你好ab"),
        format!("{now:?}"),
    );

    // The agent asks for the display back while the user composes, and the
    // user lets it continue on the card: the composition is dropped, at
    // once and for good.
    typed_key(45, "n", "inputMethod", "mark:ni");
    control.call(
        "requestControl",
        json!({ "sessionKey": key, "displayId": id, "reason": "I'll finish the form." }),
    )?;
    sleep_ms(400);
    control.call(
        "debugShow",
        json!({ "surface": "card-continue", "displayId": id }),
    )?;
    sleep_ms(600);
    check(
        "handing back on the card drops the composition",
        marked().is_none() && read() == holds("你好ab"),
        format!("{:?} {:?}", marked(), read()),
    );
    // An input method that commits when told to discard, late.
    input(json!({ "kind": "commit", "text": "ni" }))?;
    check(
        "text the input method commits after the hand-back is dropped",
        read() == holds("你好ab"),
        format!("{:?}", read()),
    );
    let typed = control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "x" }),
    );
    check(
        "the agent types once the display is handed back",
        typed.is_ok(),
        format!("{typed:?}"),
    );
    let now = until(&|f| *f == holds("你好abx"));
    check(
        "Field gets only the agent's text",
        now == holds("你好abx"),
        format!("{now:?}"),
    );
    // A new Takeover: a late commit from the old one is still dropped.
    input(at(100.0))?;
    sleep_ms(600);
    input(json!({ "kind": "commit", "text": "ni" }))?;
    check(
        "a late commit after the next click is dropped",
        marked().is_none() && read() == holds("你好abx"),
        format!("{:?} {:?}", marked(), read()),
    );
    typed_key(4, "h", "inputMethod", "mark:h");
    let routed = typed_key(49, " ", "inputMethod", "insert:好");
    // The next word's first letter, typed at once after the commit,
    // composes without waiting: text moves no focus.
    let started = std::time::Instant::now();
    let next = typed_key(45, "n", "inputMethod", "mark:n");
    let after_commit = started.elapsed().as_secs_f64() * 1000.0;
    let next = format!("{next} by {}", routed_by.borrow());
    let now = until(&|f| *f == holds("你好abx好"));
    check(
        "the next composition sends only its own text",
        routed == "\"text\"" && now == holds("你好abx好"),
        format!("{routed} {now:?}"),
    );
    check(
        "the first letter typed at once after a commit composes without waiting",
        next == "\"inputMethod\" by \"open\"" && marked().as_deref() == Some("n"),
        format!("{next}: {:?}", marked()),
    );
    println!("     first letter after a commit: {next}, its reply in {after_commit:.1} ms");
    // Korean 2-Set: Return after a syllable commits it, then asks for its
    // command: Field gets the syllable, then Return as the key.
    typed_key(5, "g", "inputMethod", "mark:하");
    let routed = typed_key(36, "\r", "inputMethod", "insertcommand:한");
    let now = until(&|f| *f == holds("你好abx好한\n"));
    check(
        "a command after composed text sends the text, then the key",
        routed == "\"textThenKey\"" && now == holds("你好abx好한\n"),
        format!("{routed} {now:?}"),
    );
    // A click while composing commits the marked text where it was typed.
    typed_key(4, "h", "inputMethod", "mark:ha");
    input(at(100.0))?;
    sleep_ms(300);
    let now = until(&|f| *f == holds("你好abx好한\nha"));
    check(
        "a click commits the marked text",
        marked().is_none() && now == holds("你好abx好한\nha"),
        format!("{:?} {now:?}", marked()),
    );

    // Field's password field, typed into at once after the click: the
    // letter waits for the lookup, then skips the input method.
    input(at(232.0))?;
    let (from, found) = (routes(&control).len(), secure_found(&control));
    let routed = typed_key(0, "a", "inputMethod", "mark:a");
    let now = until(&|f| f.password == "a");
    let went = routes_after(&control, from, 1);
    check(
        "the first letter typed at once into a password field goes as a key, never composed",
        went == [(0, "raw".to_string())] && secure_found(&control) == found && now.password == "a",
        format!(
            "{routed} then {went:?}, compositions dropped {found} then {}: {now:?}",
            secure_found(&control)
        ),
    );
    check(
        "nothing is marked over a password field",
        marked().is_none(),
        format!("{:?}", marked()),
    );
    check(
        "the Viewer sees the password field",
        viewer()["secure"] == true,
        viewer()["secure"].to_string(),
    );

    // A view that takes keys but holds no text, as a game: pinyin passes
    // its letters through, and they arrive as keys, keycodes and all, as
    // do arrows and shortcuts.
    input(at(276.0))?;
    sleep_ms(600);
    let routed = typed_key(0, "a", "inputMethod", "pass");
    check(
        "a letter the input method passes through goes as the key",
        routed == "\"raw\"",
        &routed,
    );
    let routed = typed_key(123, "\u{f702}", "inputMethod", "nothing");
    check(
        "Left Arrow the input method doesn't take goes as the key",
        routed == "\"raw\"",
        &routed,
    );
    let routed = input(
        json!({ "kind": "key", "code": 124, "command": 1, "characters": "\u{f703}", "source": "inputMethod", "ime": "mark:x" }),
    )
    .map_or_else(|e| e.to_string(), |r| r["route"].to_string());
    check(
        "a Command shortcut skips the input method",
        routed == "\"raw\"",
        &routed,
    );
    let now = until(&|f| f.keys == "a\u{f702}\u{f703}");
    check(
        "the key view got the letter and the arrows",
        now.keys == "a\u{f702}\u{f703}",
        format!("{now:?}"),
    );
    check(
        "Field's text is unchanged",
        read().text == "你好abx好한\nha",
        format!("{:?}", read()),
    );
    // Return held down in the key view: its press goes as the key and may
    // move the focus, so its repeat waits for the lookup. The user hands
    // the display back on the agent's request at once: the repeat is
    // dropped, and Return's release still reaches the key view, so it
    // isn't left down. (A Command shortcut's key stays down there: AppKit
    // sends no keyUp for one.)
    let enter = |phase: &str, repeat: u8| {
        input(
            json!({ "kind": "key", "code": 36, "characters": "\r", "source": "inputMethod",
                      "ime": "pass", "phase": phase, "repeat": repeat }),
        )
        .map_or_else(|e| e.to_string(), |r| r["route"].to_string())
    };
    control.call(
        "requestControl",
        json!({ "sessionKey": key, "displayId": id, "reason": "I'll finish the form." }),
    )?;
    sleep_ms(400);
    let pressed = enter("down", 0);
    let repeated = enter("down", 1);
    control.call(
        "debugShow",
        json!({ "surface": "card-continue", "displayId": id }),
    )?;
    sleep_ms(600);
    enter("up", 0);
    let now = until(&|f| !f.down.split(',').any(|c| c == "36"));
    check(
        "a key whose repeat was dropped with the Takeover comes up in the target",
        pressed == "\"raw\""
            && repeated == "\"held\""
            && !now.down.split(',').any(|c| c == "36")
            && now.keys.matches('\r').count() == 1,
        format!(
            "{pressed} then {repeated}: down {:?}, keys {:?}",
            now.down, now.keys
        ),
    );
    // Shift right after a click into the key view waits with the keys, so
    // the target gets Shift, the letter and Shift's release in order.
    input(at(276.0))?;
    let shift = |down: u8| {
        input(json!({ "kind": "shift", "down": down, "source": "inputMethod" }))
            .map_or_else(|e| e.to_string(), |r| r["secrecy"].to_string())
    };
    let pressed = shift(1);
    input(
        json!({ "kind": "key", "code": 0, "shift": 1, "characters": "A",
                  "source": "inputMethod", "ime": "pass" }),
    )?;
    shift(0);
    let now = until(&|f| f.keys.ends_with("[S]A[s]"));
    check(
        "Shift typed at once after a click waits with the letter and arrives in order",
        pressed == "\"unknown\"" && now.keys.ends_with("[S]A[s]"),
        format!("Shift by {pressed}: {:?}", now.keys),
    );
    // Field moves its focus to the password field by itself while the
    // user composes: the composition is dropped, never committed, and the
    // letters typed for it reach the field as keys. Taking the keyboard
    // selects what the password field holds, so they replace it. Nothing
    // the Viewer sees moves the focus here, so no lookup can come first:
    // the composition shows until the next one.
    input(at(100.0))?;
    let first = typed_key(45, "n", "inputMethod", "mark:n");
    let first = format!(
        "{first} by {}: {:?}",
        routed_by.borrow(),
        until_marked(Some("n"))
    );
    std::fs::write(format!("{}.focus", saved.display()), "")?;
    sleep_ms(500);
    let second = typed_key(34, "i", "inputMethod", "mark:ni");
    let second = format!("{second} by {}", routed_by.borrow());
    let now = until(&|f| f.password == "ni");
    check(
        "once the focus turns out to be a password field, no letter is lost",
        marked().is_none() && now.password == "ni" && now.text == "你好abx好한\nha",
        format!("{first}, {second}: {:?} {now:?}", marked()),
    );
    input(json!({ "kind": "commit", "text": "ni" }))?;
    let now = read();
    check(
        "the dropped composition never reaches the password field as text",
        now.password == "ni",
        format!("{now:?}"),
    );
    // Tab from the text view into the password field, then typing at once,
    // before any lookup answers: every letter goes to the field as a key.
    // Field moves its focus 60 ms after the Tab, as Chrome does: a look
    // right after the key still sees the text view.
    input(at(100.0))?;
    sleep_ms(600);
    let (from, found) = (routes(&control).len(), secure_found(&control));
    let tabbed = typed_key(48, "\t", "inputMethod", "nothing");
    for (code, c) in [(7, "x"), (16, "y"), (6, "z")] {
        typed_key(code, c, "inputMethod", &format!("mark:{c}"));
    }
    let now = until(&|f| f.password == "xyz");
    let went = routes_after(&control, from, 4);
    let raw = |code: u64| (code, "raw".to_string());
    check(
        "after Tab into a password field, pinyin's letters go as keys, never composed",
        went == [raw(48), raw(7), raw(16), raw(6)]
            && secure_found(&control) == found
            && marked().is_none(),
        format!(
            "{tabbed} then {went:?}, compositions dropped {found} then {}: {:?}",
            secure_found(&control),
            marked()
        ),
    );
    check(
        "the password field gets every letter",
        now.password == "xyz" && now.text == "你好abx好한\nha",
        format!("{now:?}"),
    );
    // Field turns secure input on while its password field has the
    // keyboard, and input methods are off everywhere then: leave it.
    input(at(100.0))?;
    sleep_ms(600);
    check(
        "no key waited on a lookup on the main thread",
        viewer()["mainThreadLookups"] == 0,
        viewer()["mainThreadLookups"].to_string(),
    );
    electron_field(&control, key, id, (width, height))?;
    // What the wait for the lookup costs.
    let shown = viewer();
    for (name, field) in [
        ("focus lookups", "lookupMs"),
        ("held keys waited", "heldMs"),
    ] {
        let mut list: Vec<f64> = shown[field]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let in_order: Vec<String> = list.iter().map(|ms| format!("{ms:.0}")).collect();
        list.sort_by(f64::total_cmp);
        if let (Some(p50), Some(max)) = (list.get(list.len() / 2), list.last()) {
            println!(
                "     {name}: {} times, p50 {p50:.1} ms, max {max:.1} ms; in order: {}",
                list.len(),
                in_order.join(" ")
            );
        }
    }
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}

/// A field on the Electron harness page composes as Field's does, from
/// the first word typed at once after the click: the keys wait for the
/// lookup, which only has to answer, not find the field, which Electron
/// may not report yet.
/// Each key press's final route so far, by key code, as the app reports
/// them.
fn routes(control: &Control) -> Vec<(u64, String)> {
    control.call("debugViewer", json!({})).unwrap_or_default()["routes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| Some((r[0].as_u64()?, r[1].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// How many compositions the app dropped as a lookup found a password
/// field: one the lookup came too late for.
fn secure_found(control: &Control) -> u64 {
    control.call("debugViewer", json!({})).unwrap_or_default()["secureFound"]
        .as_u64()
        .unwrap_or(u64::MAX)
}

/// The final routes of the key presses after the first `from`, waiting up
/// to 2 s until there are `count`.
fn routes_after(control: &Control, from: usize, count: usize) -> Vec<(u64, String)> {
    for _ in 0..40 {
        if routes(control).len() >= from + count {
            break;
        }
        sleep_ms(50);
    }
    routes(control).into_iter().skip(from).collect()
}

fn electron_field(
    control: &Control,
    key: &str,
    id: u64,
    (width, height): (f64, f64),
) -> Result<()> {
    let electron = out().join("Electron.app");
    if !electron.exists() {
        println!("     no target/e2e/Electron.app: the Electron field isn't checked");
        return Ok(());
    }
    // The agent starts it: the user hands the display back first.
    control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    sleep_ms(600);
    allow_on_your_screen(
        "Electron",
        800,
        "Electron may show its window on the main screen for a moment as it starts",
    );
    control.call(
        "launchApp",
        json!({ "sessionKey": key, "displayId": id, "app": electron.to_str(),
                "arguments": [fixture("electron").to_str()],
                "environment": { "HARNESS_BACKGROUND": "1", "HARNESS_DISPLAY": id.to_string() } }),
    )?;
    let harness = || -> Option<Value> {
        let list = control
            .call("listWindows", json!({ "displayId": id }))
            .ok()?;
        list["windows"]
            .as_array()?
            .iter()
            .find(|w| w["app"] == "Electron" && w["kind"] == "window")
            .cloned()
    };
    let mut window = None;
    for _ in 0..50 {
        window = harness();
        if window.is_some() {
            break;
        }
        sleep_ms(200);
    }
    let window = window.context("no Electron window")?;
    let number = |name: &str| window[name].as_f64().unwrap_or(0.0);
    // The page's fields, 30 pt high, 8 pt above the bottom: a text field
    // from the left edge, a password field 320 pt in.
    let click = |x: f64| {
        control.call(
            "debugViewerInput",
            json!({ "kind": "click", "source": "inputMethod",
                    "x": (number("x") + x) / width,
                    "y": (number("y") + number("height") - 8.0 - 15.0) / height }),
        )
    };
    let key = |code: u16, characters: &str, ime: &str| -> String {
        control
            .call(
                "debugViewerInput",
                json!({ "kind": "key", "code": code, "characters": characters,
                        "source": "inputMethod", "ime": ime }),
            )
            .map_or_else(
                |e| e.to_string(),
                |r| format!("{} by {}", r["route"], r["secrecy"]),
            )
    };
    // The password field first, before anything asked Electron for its
    // page: until the page is up, the lookup can't see the field, and a
    // letter typed at once must not compose.
    click(380.0)?;
    let (from, found) = (routes(control).len(), secure_found(control));
    let typed = key(0, "a", "mark:a");
    sleep_ms(800);
    let went = routes_after(control, from, 1);
    let title = harness()
        .map(|w| w["title"].to_string())
        .unwrap_or_default();
    check(
        "a letter typed at once into an Electron page's password field goes as a key, never composed",
        went == [(0, "raw".to_string())]
            && secure_found(control) == found
            && title.trim_matches('"').split(' ').any(|p| p == "secret=a"),
        format!(
            "{typed} then {went:?}, compositions dropped {found} then {}: {title}",
            secure_found(control)
        ),
    );
    // The page is up within 3 s of being asked for.
    sleep_ms(3000);
    click(60.0)?;
    // Typed at once: the keys wait for the click's lookup.
    let first = key(45, "n", "mark:n");
    key(34, "i", "mark:ni");
    let committed = key(49, " ", "insert:你好");
    check(
        "the first word in an Electron page's field composes",
        (first.starts_with("\"inputMethod\"") || first.starts_with("\"held\""))
            && (committed.starts_with("\"text\"") || committed.starts_with("\"held\"")),
        format!("{first} {committed}"),
    );
    sleep_ms(800);
    let title = harness()
        .map(|w| w["title"].to_string())
        .unwrap_or_default();
    check(
        "the Electron page's field gets exactly 你好",
        title.ends_with("typed=你好\""),
        &title,
    );
    Ok(())
}
