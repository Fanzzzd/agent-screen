//! A native pop-up button and combo box chosen with act_on_element
//! set_value while their menus stay closed (#79). The pop-up takes its item's
//! title as type-ahead, sent to the fixture's process only; the combo box
//! takes AXValue and AXConfirm. The fixture writes what it holds and how
//! often a menu opened into its title: tests/e2e/tools/menuchoice.swift.
//! A title with a space is refused before any key, as a space can open the
//! menu and an AppKit pop-up lists no items to tell whether the part before
//! it is enough. So are a pop-up that takes no focus and a WebKit page's
//! select. A wrong-case title, an earlier item that starts with the title,
//! a rejected combo commit and unsafe titles fail and say so; a Takeover
//! during either choice stops it.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e menuchoice
//!
//! 1 display. The fixture exits on its own within 55 s.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, check, instance, json, kill_ours, menu_choice_app, our_binary, out, run, sleep_ms,
    start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;

const KEY: &str = "menuchoice";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let fixture = menu_choice_app()?;
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test(&fixture);
        kill_ours(our_binary("MenuChoice"));
        stop_test_app();
        result
    })
}

fn test(fixture: &std::path::Path) -> Result<()> {
    let c = Control::connect()?;
    let display = c.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Menu choice", "width": 1100, "height": 700 }),
    )?;
    let id = display["displayId"].as_u64().context("display id")?;
    let marker = out().join("menuchoice-setter");
    let _ = std::fs::remove_file(&marker);
    let result = (|| -> Result<()> {
        let launch = c.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": fixture, "environment": {
                "MENU_DISPLAY_ID": id.to_string(), "MENU_SETTER_MARKER": marker,
            } }),
        )?;
        let window = launch["window"]["windowId"]
            .as_u64()
            .context("fixture window")?;
        let call = |method: &str, mut p: Value| {
            p["sessionKey"] = json!(KEY);
            p["displayId"] = json!(id);
            p["windowId"] = json!(window);
            c.call(method, p)
        };
        let title = || -> Result<String> {
            Ok(call("listWindows", json!({}))?["windows"]
                .as_array()
                .and_then(|a| a.iter().find(|w| w["windowId"] == window))
                .and_then(|w| w["title"].as_str())
                .unwrap_or_default()
                .to_owned())
        };
        let element = |role: &str, name: &str| -> Result<u64> {
            let tree = call("windowState", json!({}))?["tree"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let found = Regex::new(&format!(r#"\[(\d+)\] {role} "{name}""#))?
                .captures(&tree)
                .with_context(|| format!("no {name} in {tree}"))?;
            Ok(found[1].parse()?)
        };
        let set = |index: u64, value: &str| {
            call(
                "elementAction",
                json!({ "element": index, "action": "set_value", "value": value }),
            )
        };

        for (role, name, value, expected) in [
            ("popupbutton", "Fruit", "Cherry", "fruit=Cherry"),
            ("popupbutton", "Fruit", "Banana", "fruit=Banana"),
            ("combobox", "Color", "Blue", "color=Blue"),
        ] {
            let result = set(element(role, name)?, value);
            let mut shown = String::new();
            for _ in 0..20 {
                shown = title()?;
                if shown.contains(expected) {
                    break;
                }
                sleep_ms(50);
            }
            check(
                &format!("{name} chooses {value} without opening a menu"),
                result.is_ok() && shown.contains(expected) && shown.contains("opens=0"),
                format!("{result:?}; {shown}"),
            );
        }

        // A title with a space, which is never typed, a repeated key, CJK,
        // and a title an earlier item starts with.
        enum Outcome {
            Chosen,
            /// Refused before any key: the pop-up is as it was.
            Refused(&'static str),
            /// Typed, then the check failed and said so.
            Failed(&'static str),
        }
        let fruit = element("popupbutton", "Fruit")?;
        for (value, outcome) in [
            ("Dragon fruit", Outcome::Refused("never types one")),
            ("10", Outcome::Chosen),
            ("11", Outcome::Chosen),
            ("1", Outcome::Chosen),
            ("1", Outcome::Chosen),
            ("中文", Outcome::Chosen),
            ("Dragon", Outcome::Failed("can't reach")),
            ("Banana", Outcome::Chosen),
        ] {
            let before = title()?;
            let result = set(fruit, value);
            let mut shown = String::new();
            for _ in 0..20 {
                shown = title()?;
                if shown.contains(&format!("fruit={value} ")) {
                    break;
                }
                sleep_ms(50);
            }
            let says = |text: &str| {
                result
                    .as_ref()
                    .err()
                    .is_some_and(|e| format!("{e:#}").contains(text))
            };
            let (label, ok) = match outcome {
                Outcome::Chosen => (
                    "is chosen".to_string(),
                    result.is_ok() && shown.contains(&format!("fruit={value} ")),
                ),
                Outcome::Refused(text) => (
                    format!("is refused before any key, saying {text:?}"),
                    says(text) && says("Nothing was sent") && shown == before,
                ),
                Outcome::Failed(text) => (format!("fails saying {text:?}"), says(text)),
            };
            check(
                &format!("{value:?} {label}, with no menu opened"),
                ok && shown.contains("opens=0"),
                format!("{result:?}; {before} -> {shown}"),
            );
        }

        // Keys go out only once the pop-up is the app's focused element.
        let locked = set(element("popupbutton", "Locked")?, "Kiwi");
        let shown = title()?;
        check(
            "a pop-up that takes no focus gets no key",
            locked
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("focused element"))
                && shown.contains("locked=Apple ")
                && shown.contains("opens=0"),
            format!("{locked:?}; {shown}"),
        );
        // A WebKit page's select may open its menu on a key: none is sent.
        let web = set(element("popupbutton", "Web fruit")?, "Kiwi");
        check(
            "a WebKit page's select is refused before any key",
            web.as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("is neither")),
            format!("{web:?}"),
        );

        let color = element("combobox", "Color")?;
        let wrong_case = set(fruit, "banana");
        check(
            "a wrong-case item title fails and names what the pop-up shows",
            wrong_case
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("shows \"Banana\"")),
            format!("{wrong_case:?}"),
        );
        let commit = set(color, "Reject");
        check(
            "a combo commit the app rejects fails and names the value it restored",
            commit
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("Blue")),
            format!("{commit:?}"),
        );
        for value in ["Cherry\n", " Banana", "\u{f701}Banana"] {
            let rejected = set(fruit, value);
            check(
                &format!("the title {value:?} is refused before any key"),
                rejected
                    .as_ref()
                    .err()
                    .is_some_and(|e| format!("{e:#}").contains("invalid: a pop-up button takes")),
                format!("{rejected:?}"),
            );
        }

        // The fixture's setter writes the marker, then holds AXValue for
        // 0.4 s: the Takeover starts while it runs.
        let pending = c.begin(
            "elementAction",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window,
                "element": color, "action": "set_value", "value": "Slow" }),
        )?;
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            sleep_ms(10);
        }
        anyhow::ensure!(marker.exists(), "the fixture's slow setter never ran");
        call("debugTakeover", json!({ "active": true }))?;
        let during = pending.wait();
        call("debugTakeover", json!({ "active": false }))?;
        let shown = title()?;
        check(
            "a Takeover while AXValue is set stops the combo before AXConfirm",
            during
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("takeover"))
                && shown.contains("color=Blue"),
            format!("{during:?}; {shown}"),
        );

        // Within the 1.2 s wait for an earlier search to end.
        let pending = c.begin(
            "elementAction",
            json!({ "sessionKey": KEY, "displayId": id, "windowId": window,
                "element": fruit, "action": "set_value", "value": "Cherry" }),
        )?;
        sleep_ms(100);
        call("debugTakeover", json!({ "active": true }))?;
        let interrupted = pending.wait();
        call("debugTakeover", json!({ "active": false }))?;
        let shown = title()?;
        check(
            "a Takeover during pop-up type-ahead stops it before any key",
            interrupted
                .as_ref()
                .err()
                .is_some_and(|e| format!("{e:#}").contains("takeover"))
                && shown.contains("fruit=Banana"),
            format!("{interrupted:?}; {shown}"),
        );
        Ok(())
    })();
    // The fixture's window goes before its display does, so macOS can't
    // move it onto the user's screen.
    kill_ours(our_binary("MenuChoice"));
    sleep_ms(500);
    let _ = c.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
    result
}
