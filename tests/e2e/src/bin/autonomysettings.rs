//! The Autonomy Mode as the user sets it (ADR 0025), through the surfaces
//! Settings' segments and buttons call:
//!
//! - Each segment sets its mode; YOLO needs the confirmation, and Cancel
//!   leaves the mode as it was.
//! - The e2e instance's file holds `auto` and never `yolo`.
//! - `autonomy-lock` and `autonomy-sleep` return YOLO to the stored mode,
//!   with the line saying why.
//! - `autonomy-stop`, as the Stop chord, gives Ask and unregisters it.
//! - The menu bar icon's `tint` is red only in YOLO, and the menu's items
//!   follow the mode.
//!
//! Settings shows on the user's screen. No display, no input.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e autonomysettings

use std::process::ExitCode;

use agent_screen_tests::{
    Control, Value, autonomy_file, check, instance, json, put_back_autonomy, run,
    set_autonomy_file, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::Result;

fn main() -> ExitCode {
    if instance().as_deref() != Some("e2e") {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        stop_test_app();
        let before = set_autonomy_file(&json!({ "mode": "ask" }))?;
        let result = start_test_app(&[("AGENTSCREEN_LANG", "en")]).and_then(|()| test());
        if let Ok(control) = Control::connect() {
            let _ = control.call("debugShow", json!({ "surface": "autonomy-ask" }));
            let _ = control.call("debugShow", json!({ "surface": "setup-close" }));
        }
        stop_test_app();
        put_back_autonomy(before);
        result
    })
}

fn show(control: &Control, surface: &str) -> Result<Value> {
    let reply = control.call("debugShow", json!({ "surface": surface }))?;
    sleep_ms(200);
    Ok(reply)
}

fn stored() -> String {
    autonomy_file()
        .ok()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default()
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let state = |c: &Control| c.call("debugAutonomy", json!({}));
    let icon = |c: &Control| c.call("debugIcon", json!({}));

    show(&control, "setup-autonomy")?;
    let s = state(&control)?;
    check("it starts in Ask", s["mode"] == "ask", s.to_string());
    check(
        "Ask registers no Stop chord",
        s["hotkey"] == false,
        s["hotkey"].to_string(),
    );
    check(
        "the menu in Ask has no Stop item and checks Ask",
        s["menu"]["stop"].is_null()
            && s["menu"]["modes"][0]["checked"] == true
            && s["menu"]["title"] == "Autonomy: Ask",
        s["menu"].to_string(),
    );

    show(&control, "autonomy-auto")?;
    let s = state(&control)?;
    check(
        "the Auto segment sets Auto",
        s["mode"] == "auto",
        s.to_string(),
    );
    check(
        "the file holds auto",
        stored().contains("\"auto\""),
        stored(),
    );
    check("Auto registers the Stop chord", s["hotkey"] == true, "");
    check(
        "the menu in Auto has Stop and Switch to Ask",
        s["menu"]["stop"] == "Stop and Switch to Ask" && s["menu"]["modes"][1]["checked"] == true,
        s["menu"].to_string(),
    );
    let i = icon(&control)?;
    check(
        "the icon is not tinted in Auto",
        i["tint"].is_null(),
        i.to_string(),
    );

    let r = show(&control, "autonomy-yolo")?;
    check(
        "the YOLO segment asks first and changes nothing yet",
        r["confirming"] == true && r["mode"] == "auto",
        r.to_string(),
    );
    let r = show(&control, "autonomy-yolo-cancel")?;
    check(
        "Cancel leaves Auto",
        r["confirming"] == false && r["mode"] == "auto",
        r.to_string(),
    );
    let refused = control.send("debugShow", json!({ "surface": "autonomy-yolo-confirm" }))?;
    check(
        "there is nothing to confirm without the question",
        refused.error.is_some(),
        format!("{:?}", refused.error),
    );

    show(&control, "autonomy-yolo")?;
    let r = show(&control, "autonomy-yolo-confirm")?;
    check(
        "confirming turns YOLO on",
        r["mode"] == "yolo",
        r.to_string(),
    );
    let s = state(&control)?;
    check(
        "YOLO keeps Auto as the stored mode",
        s["stored"] == "auto",
        s.to_string(),
    );
    check(
        "the file never stores yolo as the mode",
        serde_json::from_str::<Value>(&stored()).is_ok_and(|v| v["mode"] == "auto"),
        stored(),
    );
    let i = icon(&control)?;
    check("the icon is red in YOLO", i["tint"] == "red", i.to_string());
    check(
        "the menu names YOLO",
        s["menu"]["title"] == "Autonomy: YOLO" && s["menu"]["modes"][2]["checked"] == true,
        s["menu"].to_string(),
    );

    show(&control, "autonomy-lock")?;
    let s = state(&control)?;
    check(
        "a lock returns YOLO to the stored mode",
        s["mode"] == "auto" && s["yoloEnded"] == "locked",
        s.to_string(),
    );
    check(
        "the menu says why YOLO ended",
        s["menu"]["ended"] == "YOLO turned off when your Mac locked.",
        s["menu"].to_string(),
    );
    let i = icon(&control)?;
    check(
        "the icon is not red after",
        i["tint"].is_null(),
        i.to_string(),
    );

    show(&control, "autonomy-yolo")?;
    show(&control, "autonomy-yolo-confirm")?;
    show(&control, "autonomy-sleep")?;
    let s = state(&control)?;
    check(
        "sleep returns YOLO to the stored mode",
        s["mode"] == "auto" && s["yoloEnded"] == "slept",
        s.to_string(),
    );

    show(&control, "autonomy-yolo")?;
    show(&control, "autonomy-yolo-confirm")?;
    show(&control, "autonomy-stop")?;
    let s = state(&control)?;
    check(
        "the Stop chord gives Ask",
        s["mode"] == "ask",
        s.to_string(),
    );
    check("Ask unregisters the chord", s["hotkey"] == false, "");
    check(
        "the file holds ask",
        stored().contains("\"mode\":\"ask\""),
        stored(),
    );
    check(
        "the menu has no Stop item in Ask",
        s["menu"]["stop"].is_null(),
        s["menu"].to_string(),
    );

    show(&control, "autonomy-auto")?;
    show(&control, "autonomy-ask")?;
    let s = state(&control)?;
    check(
        "the Ask segment sets Ask",
        s["mode"] == "ask",
        s.to_string(),
    );
    Ok(())
}
