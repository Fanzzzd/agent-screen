//! type and key with via_input_method go through the input method (issue
//! #23, ADR 0008). In a text field on the display, Apple's pinyin turns
//! "nihao" and space into 你好, where plain typing gives the letters, and a
//! key through it starts a composition. The
//! user's front app, input source and AeroSpace workspace are the same
//! after. It waits for the user to leave the keyboard and pointer alone for
//! 3 s, since the call borrows their keyboard, and skips after 60 s, or
//! when a password field on the user's screen has the keyboard.
//!
//!     SIDESCREEN_INSTANCE=e2e cargo e2e ime

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use sidescreen_tests::{
    Control, check, instance, json, out, probe, run, sh, sh_lenient, skip, sleep_ms,
    start_test_app, stop_test_app, tool,
};

const PINYIN: &str = "com.apple.inputmethod.SCIM.ITABC";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with SIDESCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let mut waited = 0;
        while idle() < 3.0 {
            if waited > 60 {
                skip("the user kept using the keyboard or pointer");
            }
            sleep_ms(1000);
            waited += 1;
        }
        let field = field_app()?;
        start_test_app(&[("SIDESCREEN_LANG", "en")])?;
        let result = test(&field);
        let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
        stop_test_app();
        result
    })
}

fn idle() -> f64 {
    probe(&["idle"])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.0)
}

/// tools/field.swift as an app, so launch_app takes it.
fn field_app() -> Result<PathBuf> {
    let app = out().join("Field.app");
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    std::fs::copy(tool("field")?, macos.join("Field"))?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.sidescreen.e2e.field</string>
<key>CFBundleExecutable</key><string>Field</string>
<key>CFBundleName</key><string>Field</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#,
    )?;
    sh(
        "/usr/bin/codesign",
        &["--force", "--sign", "-", app.to_str().unwrap()],
    )?;
    Ok(app)
}

fn test(field: &Path) -> Result<()> {
    let control = Control::connect()?;
    let key = "ime";
    let saved = out().join("field.txt");
    let _ = std::fs::remove_file(&saved);
    let read = || std::fs::read_to_string(&saved).unwrap_or_default();
    let display = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "IME", "width": 1280, "height": 800 }),
    )?;
    let id = display["displayId"].clone();
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
    let (x, y) = (
        window["x"].as_f64().unwrap_or(0.0),
        window["y"].as_f64().unwrap_or(0.0),
    );
    let (w, h) = (
        window["width"].as_f64().unwrap_or(0.0),
        window["height"].as_f64().unwrap_or(0.0),
    );
    control.call(
        "click",
        json!({ "sessionKey": key, "displayId": id, "x": x + w / 2.0, "y": y + h / 2.0 }),
    )?;

    control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "ab" }),
    )?;
    sleep_ms(300);
    check("plain typing gives the letters", read() == "ab\t", read());

    let front = probe(&["front"])?;
    let source = probe(&["source"])?;
    let workspace = sh_lenient(
        "/opt/homebrew/bin/aerospace",
        &["list-workspaces", "--focused"],
    );
    let typed = control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "nihao ", "viaInputMethod": true, "inputSource": PINYIN }),
    );
    if let Err(e) = &typed
        && e.to_string().contains("secure input")
    {
        control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
        let _ = sh_lenient("/usr/bin/pkill", &["-f", "Field.app/Contents/MacOS/Field"]);
        stop_test_app();
        skip(
            "a password field on the user's screen has the keyboard; Sidescreen refused, as it should",
        );
    }
    check(
        "typing through the input method succeeds",
        typed.is_ok(),
        format!("{typed:?}"),
    );
    sleep_ms(300);
    check(
        "the input method composes nihao and space into 你好",
        read() == "ab你好\t",
        read(),
    );
    let pressed = control.call(
        "key",
        json!({ "sessionKey": key, "displayId": id, "keys": "n", "viaInputMethod": true, "inputSource": PINYIN }),
    );
    check(
        "a key through the input method succeeds",
        pressed.is_ok(),
        format!("{pressed:?}"),
    );
    sleep_ms(300);
    check(
        "the key starts a composition, left marked",
        read() == "ab你好n\tn",
        read(),
    );

    check(
        "the user's app is in front again",
        probe(&["front"])? == front,
        &front,
    );
    check(
        "the user's input source is back",
        probe(&["source"])? == source,
        &source,
    );
    check(
        "the AeroSpace workspace didn't change",
        sh_lenient(
            "/opt/homebrew/bin/aerospace",
            &["list-workspaces", "--focused"],
        ) == workspace,
        workspace.trim(),
    );
    let refused = control.call(
        "type",
        json!({ "sessionKey": key, "displayId": id, "text": "中", "viaInputMethod": true }),
    );
    check(
        "a character with no key is refused",
        refused.is_err(),
        format!("{refused:?}"),
    );
    control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }))?;
    Ok(())
}
