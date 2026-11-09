//! The user's input method composes in the Viewer during a Takeover, and
//! the target gets only the finished text (issue #117, ADR 0008). The
//! script makes the input method's own calls on the Viewer's view,
//! setMarkedText and insertText, as pinyin makes them while the user types
//! "nihao" and picks 你好: Field gets nothing while the text is marked,
//! then exactly 你好, and the marked text sits at Field's caret. Keys typed
//! with a plain keyboard layout, keys with no text and Command shortcuts
//! go to Field as keys. A composition left when the Takeover ends is
//! dropped and never comes back, even if the input method commits it
//! then; a click commits it. In Field's password field and in a view
//! that takes keys but holds no text, as a game, keys go as keys even with
//! an input method, and nothing is composed. A field on an Electron page
//! composes as Field's does. With a keyboard layout the Viewer never looks
//! up the target's focus. Everything goes through the
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
        }
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
    let routed_to = |code: u16, command: bool, source: &str| -> String {
        let routed = input(
            json!({ "kind": "key", "code": code, "command": u8::from(command), "source": source }),
        );
        routed.map_or_else(|e| e.to_string(), |r| r["route"].to_string())
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
    // A key with an input method selected goes to it, not to Field: the
    // Viewer looks up the focus it had not looked up before the key.
    let routed = routed_to(45, false, "inputMethod");
    check(
        "a letter typed with an input method goes to the input method",
        routed == "\"inputMethod\"",
        &routed,
    );
    check(
        "the Viewer sees editable text focused in Field",
        viewer()["focus"] == "text",
        viewer()["focus"].to_string(),
    );
    check(
        "Field got no key from it",
        read() == holds(""),
        format!("{:?}", read()),
    );

    // Pinyin marks the letters as the user types them.
    for text in ["n", "ni", "ni h", "ni hao"] {
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
    // While composing, Return goes to the input method too, even with a
    // plain layout reported.
    let routed = routed_to(36, false, "layout");
    check(
        "Return while composing goes to the input method",
        routed == "\"inputMethod\"",
        &routed,
    );
    // The user picks 你好.
    input(json!({ "kind": "commit", "text": "你好" }))?;
    check(
        "Field gets exactly 你好 after the commit",
        read() == holds("你好"),
        format!("{:?}", read()),
    );
    check(
        "the Viewer shows no marked text after the commit",
        marked().is_none(),
        format!("{:?}", marked()),
    );

    // A plain layout: keys go to Field as keys.
    for code in [0, 11] {
        let routed = routed_to(code, false, "layout");
        check(
            "a key typed with a plain layout goes to Field as a key",
            routed == "\"raw\"",
            &routed,
        );
    }
    check(
        "Field shows 你好ab",
        read() == holds("你好ab"),
        format!("{:?}", read()),
    );

    // The user starts composing and hands the display back: the
    // composition is dropped, nothing of it reaches Field, and the agent's
    // typing goes on.
    routed_to(45, false, "inputMethod");
    input(json!({ "kind": "mark", "text": "ni" }))?;
    control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    sleep_ms(600);
    check(
        "handing back drops the composition",
        marked().is_none(),
        format!("{:?}", marked()),
    );
    // An input method that commits when told to discard.
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
    check(
        "Field gets only the agent's text",
        read() == holds("你好abx"),
        format!("{:?}", read()),
    );
    // A new Takeover: the old composition stays gone.
    input(at(100.0))?;
    sleep_ms(600);
    let routed = routed_to(45, false, "inputMethod");
    check(
        "the next Takeover's key goes to the input method",
        routed == "\"inputMethod\"",
        &routed,
    );
    check(
        "no old marked text comes back",
        marked().is_none() && read() == holds("你好abx"),
        format!("{:?} {:?}", marked(), read()),
    );
    input(json!({ "kind": "commit", "text": "好" }))?;
    check(
        "the next commit sends only its own text",
        read() == holds("你好abx好"),
        format!("{:?}", read()),
    );
    // A click while composing commits the marked text where it was typed.
    routed_to(4, false, "inputMethod");
    input(json!({ "kind": "mark", "text": "ha" }))?;
    input(at(100.0))?;
    sleep_ms(300);
    check(
        "a click commits the marked text",
        marked().is_none() && read() == holds("你好abx好ha"),
        format!("{:?} {:?}", marked(), read()),
    );

    // Field's password field: keys go as keys, and nothing is composed or
    // drawn.
    input(at(232.0))?;
    sleep_ms(600);
    check(
        "the Viewer sees the password field",
        viewer()["focus"] == "secure",
        viewer()["focus"].to_string(),
    );
    let routed = routed_to(0, false, "inputMethod");
    check(
        "a letter in a password field goes as a key with an input method",
        routed == "\"raw\"",
        &routed,
    );
    input(json!({ "kind": "mark", "text": "ni" }))?;
    check(
        "nothing is marked over a password field",
        marked().is_none(),
        format!("{:?}", marked()),
    );
    check(
        "the password field got the key",
        read().password == "a",
        format!("{:?}", read()),
    );

    // A view that takes keys but holds no text, as a game.
    input(at(276.0))?;
    sleep_ms(600);
    check(
        "the Viewer sees no editable text in the key view",
        viewer()["focus"] == "other",
        viewer()["focus"].to_string(),
    );
    let routed = routed_to(0, false, "inputMethod");
    check(
        "a letter in a view with no text goes as a key with an input method",
        routed == "\"raw\"",
        &routed,
    );
    // Keys with no text and Command shortcuts never go to it either.
    let routed = routed_to(123, false, "inputMethod");
    check("Left Arrow goes as a key", routed == "\"raw\"", &routed);
    let routed = routed_to(124, true, "inputMethod");
    check(
        "a Command shortcut goes as a key",
        routed == "\"raw\"",
        &routed,
    );
    check(
        "the key view got the letter",
        read().keys.starts_with('a'),
        format!("{:?}", read()),
    );
    check(
        "Field's text is unchanged",
        read().text == "你好abx好ha",
        format!("{:?}", read()),
    );
    electron_field(&control, key, id, (width, height))?;
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}

/// A field on the Electron harness page composes as Field's does: the
/// page's elements are asked for, so the field reads as text.
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
    // The page's field: 8 pt from the bottom-left corner, 30 pt high.
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "source": "inputMethod",
                "x": (number("x") + 60.0) / width,
                "y": (number("y") + number("height") - 8.0 - 15.0) / height }),
    )?;
    // The first look at the app's focus waits for its page's elements.
    sleep_ms(3500);
    let routed = control.call(
        "debugViewerInput",
        json!({ "kind": "key", "code": 45, "source": "inputMethod" }),
    );
    check(
        "a letter in an Electron page's field goes to the input method",
        routed.as_ref().is_ok_and(|r| r["route"] == "inputMethod"),
        format!("{routed:?}"),
    );
    control.call(
        "debugViewerInput",
        json!({ "kind": "mark", "text": "ni hao" }),
    )?;
    control.call(
        "debugViewerInput",
        json!({ "kind": "commit", "text": "你好" }),
    )?;
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
