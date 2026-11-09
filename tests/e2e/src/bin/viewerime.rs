//! The user's input method composes in the Viewer during a Takeover, and
//! the target gets only the finished text (issue #117, ADR 0008). The
//! script makes the input method's own calls on the Viewer's view,
//! setMarkedText and insertText, as pinyin makes them while the user types
//! "nihao" and picks 你好: Field gets nothing while the text is marked,
//! then exactly 你好. Keys typed with a plain keyboard layout still go to
//! Field as keys. Keys typed with an input method go to it, not to Field,
//! unless Command is held. Everything goes through the Viewer's own
//! handlers, so the user's keyboard, pointer and input source are
//! untouched.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerime
//!
//! Makes 2 displays: the stage and the one the Viewer shows. Needs
//! Field.app, which `cargo e2e ime` makes.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, capture, check, field_app, instance, json, kill_ours, open_stage, our_binary, out,
    run, sleep_ms, start_test_app, stop_test_app,
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
        stop_test_app();
        result
    })
}

fn test(field: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let key = "viewerime";
    let saved = out().join("viewerime-field.txt");
    let _ = std::fs::remove_file(&saved);
    // Field writes "text\tmarked" ten times a second.
    let read = || {
        sleep_ms(400);
        std::fs::read_to_string(&saved).unwrap_or_default()
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
                "environment": { "FIELD_OUT": saved.to_str().unwrap() } }),
    )?;
    sleep_ms(800);
    let windows = control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
    let window = windows["windows"]
        .as_array()
        .and_then(|w| w.iter().find(|w| w["title"] == "Field"))
        .context(format!("no Field window: {windows}"))?;
    let number = |name: &str| window[name].as_f64().unwrap_or(0.0);
    let (x, y) = (
        (number("x") + number("width") / 2.0) / width,
        (number("y") + number("height") / 2.0) / height,
    );
    control.call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    // The user clicks Field's text in the Viewer: a Takeover.
    control.call(
        "debugViewerInput",
        json!({ "kind": "click", "x": x, "y": y }),
    )?;
    sleep_ms(600);
    let typed = control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "x" }),
    );
    check(
        "the click took the display over",
        typed.is_err(),
        format!("{typed:?}"),
    );
    let input = |params: Value| control.call("debugViewerInput", params);
    let marked = || -> Option<String> {
        control.call("debugViewer", json!({})).ok()?["marked"]
            .as_str()
            .map(str::to_string)
    };

    // A key with an input method selected goes to it, not to Field.
    let routed = input(json!({ "kind": "key", "code": 45, "source": "inputMethod" }));
    check(
        "a letter typed with an input method goes to the input method",
        routed.as_ref().is_ok_and(|r| r["route"] == "inputMethod"),
        format!("{routed:?}"),
    );
    check("Field got no key from it", read() == "\t", read());

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
        read() == "\t",
        read(),
    );
    // The marked text and the input method's candidates sit at Field's
    // caret, at the start of its first line, not at the click in the
    // middle of the window.
    let viewer = control.call("debugViewer", json!({}))?;
    let rect = |name: &str| -> Vec<f64> {
        viewer[name]
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
        "the caret is Field's, at its top left, not the click in its middle",
        on_display.is_some_and(|(cx, cy)| {
            cx >= number("x")
                && cx < number("x") + number("width") / 3.0
                && cy >= number("y")
                && cy < number("y") + number("height") / 3.0
        }),
        format!("{on_display:?} in {window}"),
    );
    if frame.len() == 4 {
        let shot = capture("viewerime-marked", [frame[0], frame[1], frame[2], frame[3]])?;
        println!("     the Viewer while composing: {}", shot.display());
    }
    // While composing, Return goes to the input method too, even with a
    // plain layout reported.
    let routed = input(json!({ "kind": "key", "code": 36, "source": "layout" }));
    check(
        "Return while composing goes to the input method",
        routed.as_ref().is_ok_and(|r| r["route"] == "inputMethod"),
        format!("{routed:?}"),
    );
    // The user picks 你好.
    input(json!({ "kind": "commit", "text": "你好" }))?;
    check(
        "Field gets exactly 你好 after the commit",
        read() == "你好\t",
        read(),
    );
    check(
        "the Viewer shows no marked text after the commit",
        marked().is_none(),
        format!("{:?}", marked()),
    );

    // A plain layout: keys go to Field as keys.
    for code in [0, 11] {
        let routed = input(json!({ "kind": "key", "code": code, "source": "layout" }));
        check(
            "a key typed with a plain layout goes to Field as a key",
            routed.as_ref().is_ok_and(|r| r["route"] == "raw"),
            format!("{routed:?}"),
        );
    }
    check("Field shows 你好ab", read() == "你好ab\t", read());
    // Command and keys with no text skip the input method.
    let routed = input(json!({ "kind": "key", "code": 123, "source": "inputMethod" }));
    check(
        "Left Arrow with nothing marked goes to Field as a key",
        routed.as_ref().is_ok_and(|r| r["route"] == "raw"),
        format!("{routed:?}"),
    );
    let routed =
        input(json!({ "kind": "key", "code": 124, "command": 1, "source": "inputMethod" }));
    check(
        "a Command shortcut goes to Field as a key",
        routed.as_ref().is_ok_and(|r| r["route"] == "raw"),
        format!("{routed:?}"),
    );
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}
