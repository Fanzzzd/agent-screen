//! Closed pop-up type-ahead and combo confirmation. The fixture counts menu opens.
//! AGENTSCREEN_INSTANCE=e2e cargo e2e menuchoice
use agent_screen_tests::{
    Control, check, finish_test_app, json, listening, menu_choice_app, out, run, skip, sleep_ms,
    start_test_app, stop_started_pid,
};
use anyhow::{Context, Result};
use regex::Regex;
use std::process::ExitCode;
fn main() -> ExitCode {
    if listening() {
        skip("the e2e socket is in use; leave that instance alone");
    }
    run(|| {
        let fixture = menu_choice_app()?;
        start_test_app(&[])?;
        let result = test(&fixture);
        finish_test_app(result)
    })
}
fn test(fixture: &std::path::Path) -> Result<()> {
    let c = Control::connect()?;
    let display = c.call(
        "createDisplay",
        json!({"sessionKey":"menuchoice","title":"Menu choice", "width":1100,"height":700}),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let marker = out().join("menuchoice-setter");
    let _ = std::fs::remove_file(&marker);
    let mut pid = None;
    let result = (|| -> Result<()> {
        let launch = c.call("launchApp", json!({"sessionKey":"menuchoice","displayId":id,"app":fixture,"environment":{"MENU_DISPLAY_ID":id.to_string(),"MENU_SETTER_MARKER":marker}}))?;
        pid = launch["pid"].as_u64();
        let window = launch["window"]["windowId"]
            .as_u64()
            .context("fixture window")?;
        let call = |method: &str, mut p: serde_json::Value| {
            p["sessionKey"] = json!("menuchoice");
            p["displayId"] = json!(id);
            p["windowId"] = json!(window);
            c.call(method, p)
        };
        for (role, name, value, expected) in [
            ("popupbutton", "Fruit", "Cherry", "fruit=Cherry"),
            ("popupbutton", "Fruit", "Banana", "fruit=Banana"),
            ("combobox", "Color", "Blue", "color=Blue"),
        ] {
            let tree = call("windowState", json!({}))?["tree"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let re = Regex::new(&format!(r#"\[(\d+)\] {role} "{name}""#))?;
            let index = re
                .captures(&tree)
                .with_context(|| format!("no {name} in {tree}"))?[1]
                .parse::<u64>()?;
            let result = call(
                "elementAction",
                json!({"element":index,"action":"set_value","value":value}),
            );
            let mut title = String::new();
            for _ in 0..20 {
                title = call("listWindows", json!({}))?["windows"]
                    .as_array()
                    .and_then(|a| a.iter().find(|w| w["windowId"] == window))
                    .and_then(|w| w["title"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                if title.contains(expected) {
                    break;
                }
                sleep_ms(50);
            }
            check(
                &format!("{name} chooses {value} without opening a menu"),
                result.is_ok() && title.contains(expected) && title.contains("opens=0"),
                format!("{result:?}; {title}"),
            );
        }
        let tree = call("windowState", json!({}))?["tree"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let re = Regex::new(r#"\[(\d+)\] popupbutton "Fruit""#)?;
        let index = re.captures(&tree).context("Fruit control")?[1].parse::<u64>()?;
        let wrong_case = call(
            "elementAction",
            json!({"element":index,"action":"set_value","value":"banana"}),
        );
        check(
            "a case-mismatched item title never reports an exact choice",
            wrong_case
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("shows \"Banana\"")),
            format!("{wrong_case:?}"),
        );
        let combo = Regex::new(r#"\[(\d+)\] combobox "Color""#)?;
        let combo_index = combo.captures(&tree).context("Color control")?[1].parse::<u64>()?;
        let commit = call(
            "elementAction",
            json!({"element":combo_index,"action":"set_value","value":"Reject"}),
        );
        check(
            "a rejected combo commit reports the restored value",
            commit
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("Blue")),
            format!("{commit:?}"),
        );
        let rejected = call(
            "elementAction",
            json!({"element":index,"action":"set_value","value":"Cherry\n"}),
        );
        check(
            "a title containing Return is rejected before typing",
            rejected
                .as_ref()
                .err()
                .is_some_and(|e| e.to_string().contains("without control characters")),
            format!("{rejected:?}"),
        );
        for value in [" Banana", "\u{f701}Banana"] {
            let rejected = call(
                "elementAction",
                json!({"element":index,"action":"set_value","value":value}),
            );
            check(
                "activation and navigation titles are rejected before typing",
                rejected
                    .as_ref()
                    .err()
                    .is_some_and(|e| format!("{e:#}").contains("without control characters")),
                format!("{rejected:?}"),
            );
        }
        let pending = c.begin("elementAction", json!({"sessionKey":"menuchoice","displayId":id,"windowId":window,"element":combo_index,"action":"set_value","value":"Slow"}))?;
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            sleep_ms(10);
        }
        anyhow::ensure!(marker.exists(), "slow setter marker missing");
        call("debugTakeover", json!({"active":true}))?;
        let during_takeover = pending.wait();
        let title = call("listWindows", json!({}))?["windows"][0]["title"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        call("debugTakeover", json!({"active":false}))?;
        check(
            "Takeover during AXValue prevents the subsequent combo confirmation",
            during_takeover
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("takeover"))
                && title.contains("color=Blue"),
            format!("{during_takeover:?}; {title}"),
        );
        let pending = c.begin("elementAction", json!({"sessionKey":"menuchoice","displayId":id,"windowId":window,"element":index,"action":"set_value","value":"Cherry"}))?;
        sleep_ms(100);
        call("debugTakeover", json!({"active":true}))?;
        let interrupted = pending.wait();
        call("debugTakeover", json!({"active":false}))?;
        let title = call("listWindows", json!({}))?["windows"][0]["title"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        check(
            "Takeover interrupts popup type-ahead before another choice is sent",
            interrupted
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("takeover"))
                && title.contains("fruit=Banana"),
            format!("{interrupted:?}; {title}"),
        );
        Ok(())
    })();
    // Only the fixture pid returned by our launch. End the display after it exits.
    if let Some(pid) = pid {
        stop_started_pid(pid as u32)?;
    }
    let _ = c.call(
        "endDisplay",
        json!({"sessionKey":"menuchoice","displayId":id}),
    );
    result
}
