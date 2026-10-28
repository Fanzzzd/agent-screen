//! An app the user brings to the front whose windows are all on an Agent
//! Display brings Peek down, that display's tile ringed in orange with the
//! app's windows outlined, and Peek goes by itself after 5 s (#51). Field opens on
//! the user's screen, as their own app: brought to the front, nothing to
//! say. The agent asks for its window and the test hands it over; brought
//! to the front again, the reminder names the display, and once more right
//! after, it stays quiet. A Field the agent started brings itself to the
//! front with nobody to tell. `debugActivated` stands in for the app
//! coming to the front. A click on Field's Dock icon, which the user makes
//! while Field is already the front app and so activates nothing, names
//! the display too: `debugUserClick` at the icon's real place stands in
//! for the global monitor's report of the click.
//!
//! Needs Field.app, which `cargo e2e ime` makes.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e appwhere
//!
//! Makes 1 display. Field's window shows on your screen until handed over,
//! and the test instance's Peek comes down under its menu bar icon.

use std::process::ExitCode;

use agent_screen_tests::{
    Control, McpClient, Value, allow_on_your_screen, check, instance, json, out, pid_of, run,
    sh_lenient, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if !out().join("Field.app").exists() {
        eprintln!("no Field.app; run `cargo e2e ime` once to make it");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        quit_field();
        let result = test();
        quit_field();
        stop_test_app();
        result
    })
}

fn field_binary() -> String {
    out()
        .join("Field.app/Contents/MacOS/Field")
        .to_string_lossy()
        .into_owned()
}

/// This checkout's Field only: another worktree may run its own.
fn quit_field() {
    let _ = sh_lenient("/usr/bin/pkill", &["-f", &field_binary()]);
    sleep_ms(500);
}

/// The displays the reminder named when `pid` came to the front, and
/// what Peek shows of it.
fn activated(control: &Control, pid: u32) -> Result<(Vec<u64>, Value)> {
    reminded(control.call("debugActivated", json!({ "pid": pid }))?)
}

fn reminded(reply: Value) -> Result<(Vec<u64>, Value)> {
    let ids = reply["reminded"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    Ok((ids, reply["peek"].clone()))
}

/// Peek shows a reminder for display `id`, with a window outlined.
fn peek_rings(peek: &Value, id: u64) -> bool {
    peek["displayId"].as_u64() == Some(id)
        && peek["app"] == "Field"
        && peek["windows"].as_u64().unwrap_or(0) >= 1
}

/// The middle of the Dock icon named `name`, in global points, as the
/// Dock's accessibility tree says.
fn dock_icon(name: &str) -> Option<(f64, f64)> {
    let script = format!(
        r#"import AppKit
let dock = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first!
let app = AXUIElementCreateApplication(dock.processIdentifier)
func get(_ e: AXUIElement, _ a: String) -> AnyObject? {{ var v: AnyObject?; AXUIElementCopyAttributeValue(e, a as CFString, &v); return v }}
for list in (get(app, "AXChildren") as? [AXUIElement]) ?? [] {{
  for item in (get(list, "AXChildren") as? [AXUIElement]) ?? [] where (get(item, "AXTitle") as? String) == "{name}" {{
    var p = CGPoint.zero; var s = CGSize.zero
    AXValueGetValue(get(item, "AXPosition") as! AXValue, .cgPoint, &p)
    AXValueGetValue(get(item, "AXSize") as! AXValue, .cgSize, &s)
    print(p.x + s.width / 2, p.y + s.height / 2); exit(0)
  }}
}}"#
    );
    let out = sh_lenient("/usr/bin/swift", &["-e", &script]);
    let v: Vec<f64> = out
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    (v.len() == 2).then(|| (v[0], v[1]))
}

fn clicked(control: &Control, at: (f64, f64)) -> Result<(Vec<u64>, Value)> {
    reminded(control.call("debugUserClick", json!({ "x": at.0, "y": at.1 }))?)
}

fn test() -> Result<()> {
    let control = Control::connect()?;
    let client = McpClient::start(&[])?;
    let made = client.call("create_display", json!({ "width": 1280, "height": 800 }))?;
    let id = made.display_id().context("display id")?;
    let result = (|| -> Result<()> {
        // The user's own Field, on their screen until it is handed over.
        allow_on_your_screen(
            "Field",
            8000,
            "the script opens Field there as the user's app, then hands it over",
        );
        sh_lenient(
            "/usr/bin/open",
            &["-g", "-n", out().join("Field.app").to_str().unwrap()],
        );
        sleep_ms(2500);
        let pid = pid_of(&field_binary()).context("Field didn't start")?;
        let (named, peek) = activated(&control, pid)?;
        check(
            "an app with its window on the user's screen gets no reminder",
            named.is_empty() && peek.is_null(),
            format!("{named:?}, Peek {peek}"),
        );

        // Handed over to the agent's display.
        let answer = std::thread::spawn(move || {
            sleep_ms(800);
            Control::connect()?.call(
                "debugAnswerWindow",
                json!({ "displayId": id, "give": true }),
            )
        });
        let placed = client.tool("place_window", json!({ "display_id": id, "app": "Field" }))?;
        let _ = answer.join();
        check("the window is handed over", !placed.is_error, placed.text());
        sleep_ms(800);
        let (named, peek) = activated(&control, pid)?;
        check(
            "brought to the front, the reminder names the display its window is on",
            named == vec![id],
            format!("{named:?}, display {id}"),
        );
        check(
            "Peek comes down with that display's tile ringed and Field's window outlined",
            peek_rings(&peek, id),
            peek.to_string(),
        );
        let (again, _) = activated(&control, pid)?;
        check(
            "right after, it stays quiet",
            again.is_empty(),
            format!("{again:?}"),
        );

        // Field is the front app now: a click on its Dock icon activates
        // nothing, and the user still means to see it.
        sleep_ms(5500);
        let icon = dock_icon("Field").context("no Dock icon for Field")?;
        let (named, peek) = clicked(&control, icon)?;
        check(
            "a click on its Dock icon names the display, though nothing comes to the front",
            named == vec![id],
            format!("{named:?}, display {id}, icon at {icon:?}"),
        );
        check(
            "and brings Peek down at its tile",
            peek_rings(&peek, id),
            peek.to_string(),
        );
        sleep_ms(6000);
        let (elsewhere, peek) = clicked(&control, (icon.0, icon.1 - 300.0))?;
        check(
            "a click off the Dock says nothing",
            elsewhere.is_empty(),
            format!("{elsewhere:?}"),
        );
        check("Peek has gone by itself", peek.is_null(), peek.to_string());

        // A Field the agent started, bringing itself to the front.
        let launched = client.call(
            "launch_app",
            // An environment starts a new instance beside the user's.
            json!({ "display_id": id, "app": out().join("Field.app").to_str(),
                    "environment": { "FIELD_MIN_SIZE": "300,200" } }),
        )?;
        sleep_ms(1000);
        let started = launched
            .get("pid")
            .as_u64()
            .map(|p| p as u32)
            .or_else(|| {
                sh_lenient("/usr/bin/pgrep", &["-f", &field_binary()])
                    .lines()
                    .filter_map(|l| l.parse().ok())
                    .find(|&p| p != pid)
            })
            .context("the agent's Field didn't start")?;
        let (named, peek) = activated(&control, started)?;
        check(
            "an app the agent started gets no reminder when it brings itself to the front",
            named.is_empty() && peek.is_null(),
            format!("{named:?}, Peek {peek}"),
        );
        Ok(())
    })();
    let _ = client.call("end_display", json!({ "display_id": id }));
    client.close();
    result
}
