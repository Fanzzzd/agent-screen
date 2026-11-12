//! A real click in the Viewer on a result in a real Qt app's list picks
//! that result. The user clicked a contact in WeCom's search results
//! through the Viewer and got the chat under the list, with 0.1.32 too:
//! the list closed before the click arrived. A Swift stand-in passed while
//! WeCom failed, so this one is Qt: tools/qtlist.py, a search box run by
//! PySide6 (`qt_list_app`), with its results in each kind of window a Qt
//! app opens them in: Qt's own pop-up, a borderless window that takes the
//! keys and closes when it loses key, as WeCom's does, and one that never
//! takes the keys and closes when its app loses active.
//!
//! For each kind, every way the user meets the list is a case:
//! - the agent picks a result, with no Viewer;
//! - the Viewer opens while the list is up, and leaves it up;
//! - the agent opens the results and the user clicks one while their own
//!   app is in front;
//! - the same while the list's app made itself the front app, as aTrust
//!   does and Qt does as it starts: Agent Screen taking the front then
//!   makes the list's app lose active;
//! - the user clicks a result while the Viewer is already in front.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e viewerqt
//!
//! Makes 2 displays: the stage and the list's. A sign on your screen counts
//! down from 3 and says when you have the machine back. The first run
//! downloads PySide6 into target/e2e/qtenv. The list's app comes to the
//! front for a moment in each kind's run; your app is in front again when
//! the run ends.

use std::process::{Command, ExitCode};

use agent_screen_tests::{
    Control, allow_on_your_screen, check, hid, instance, json, open_user_stage, out, qt_list_app,
    run, sh_lenient, sign_step, skip, sleep_ms, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};
use serde_json::Value;

const KEY: &str = "viewerqt";

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        let Some(app) = qt_list_app()? else {
            skip("no PySide6 for the Qt list; see the line above");
        };
        // AGENTSCREEN_NO_PRIVATE_PRESS=1 leaves the private press out.
        let off = std::env::var("AGENTSCREEN_NO_PRIVATE_PRESS").unwrap_or_default();
        allow_on_your_screen(
            "QtList",
            800,
            "Qt brings its app to the front as its first window shows, as Electron does, and the window shows where Qt puts it until Agent Screen moves it, 305 to 578 ms on 2026-11-12",
        );
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_NO_PRIVATE_PRESS", &off),
        ])?;
        let result = test(&app);
        // However it was started, its process runs the bundle's binary.
        let _ = Command::new("/usr/bin/pkill")
            .args(["-f", "QtList.app/Contents/MacOS/QtList"])
            .status();
        stop_test_app();
        result
    })
}

struct List {
    /// The bundle id of the user's front app when the run began.
    users: String,
    control: Control,
    mode: &'static str,
    id: u64,
    size: (f64, f64),
    state: std::path::PathBuf,
    /// The running list app's.
    pid: std::cell::Cell<i64>,
}

impl List {
    fn state(&self) -> Value {
        std::fs::read_to_string(&self.state)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null)
    }

    fn wait(&self, done: impl Fn(&Value) -> bool) -> Value {
        self.wait_for(30, done)
    }

    fn wait_for(&self, tenths: u32, done: impl Fn(&Value) -> bool) -> Value {
        let mut now = self.state();
        for _ in 0..tenths {
            if done(&now) {
                break;
            }
            sleep_ms(100);
            now = self.state();
        }
        now
    }

    /// A global point in the list's window, in the display's points.
    fn on_display(&self, global: (f64, f64)) -> Result<(f64, f64)> {
        let listed = self.control.call(
            "listWindows",
            json!({ "sessionKey": KEY, "displayId": self.id }),
        )?;
        let window = listed["windows"]
            .as_array()
            .and_then(|w| {
                w.iter()
                    .find(|w| w["title"].as_str().is_some_and(|t| t.starts_with("qtlist")))
                    .cloned()
            })
            .context("no qtlist window on the display")?;
        let frame = &self.state()["window"];
        let f = |i: usize| frame[i].as_f64().unwrap_or(0.0);
        Ok((
            global.0 - f(0) + window["x"].as_f64().unwrap_or(0.0),
            global.1 - f(1) + window["y"].as_f64().unwrap_or(0.0),
        ))
    }

    /// The app's on-screen windows, global: Qt's own idea of where they are
    /// goes stale once Agent Screen moves its window.
    fn windows(&self) -> Vec<[f64; 4]> {
        sh_lenient(
            out().join("probe"),
            &["windows", &self.pid.get().to_string()],
        )
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|w| ["x", "y", "width", "height"].map(|k| w[k].as_f64().unwrap_or(0.0)))
        .collect()
    }

    /// The results' window, as the window list has it: the one the size Qt
    /// gave the list.
    fn list_window(&self, state: &Value) -> Option<[f64; 4]> {
        let popup = &state["popup"];
        let (w, h) = (popup[2].as_f64()?, popup[3].as_f64()?);
        (state["open"] == true)
            .then(|| self.windows())?
            .into_iter()
            .find(|f| (f[2] - w).abs() <= 2.0 && (f[3] - h).abs() <= 2.0)
    }

    /// Result `i`'s middle, in the display's points.
    fn item_on_display(&self, state: &Value, i: usize) -> Result<(f64, f64)> {
        let list = self
            .list_window(state)
            .context("the results aren't on screen")?;
        let item = &state["items"][i];
        let popup = &state["popup"];
        let f = |v: &Value, i: usize| v[i].as_f64().unwrap_or(0.0);
        let global = (
            list[0] + f(item, 0) - f(popup, 0) + f(item, 2) / 2.0,
            list[1] + f(item, 1) - f(popup, 1) + f(item, 3) / 2.0,
        );
        // Where the display is: the main window, global and on the display.
        let (wide, tall) = (f(&state["window"], 2), f(&state["window"], 3));
        let main = self
            .windows()
            .into_iter()
            .find(|f| (f[2] - wide).abs() <= 2.0 && (f[3] - tall).abs() <= 40.0)
            .context("no main window on screen")?;
        let on = self.on_display((f(&state["window"], 0), f(&state["window"], 1)))?;
        Ok((global.0 - main[0] + on.0, global.1 - main[1] + on.1))
    }

    fn center(rect: &Value) -> (f64, f64) {
        let f = |i: usize| rect[i].as_f64().unwrap_or(0.0);
        (f(0) + f(2) / 2.0, f(1) + f(3) / 2.0)
    }

    /// The user's app in front again, as when the run began.
    fn users_app_in_front(&self) {
        if self.users.is_empty() {
            return;
        }
        let _ = Command::new("/usr/bin/open")
            .args(["-b", &self.users])
            .status();
        for _ in 0..20 {
            if front_bundle() == self.users {
                break;
            }
            sleep_ms(100);
        }
        sleep_ms(200);
    }

    /// The list's app in front, as one that brings itself forward.
    fn lists_app_in_front(&self) {
        let _ = Command::new("/usr/bin/open")
            .args(["-b", "app.agentscreen.e2e.qtlist"])
            .status();
        for _ in 0..20 {
            if front_bundle() == "app.agentscreen.e2e.qtlist" {
                break;
            }
            sleep_ms(100);
        }
        sleep_ms(300);
    }

    /// An agent's call. Right after a real click the user still holds the
    /// display for a moment: Let Continue hands it back.
    fn agent(&self, method: &str, params: Value) -> Result<Value> {
        let mut tries = 0;
        loop {
            match self.control.call(method, params.clone()) {
                Ok(reply) => return Ok(reply),
                // A key right after the agent's click closed Qt's pop-up, a
                // menu to Agent Screen, isn't sent: the next one is.
                Err(error)
                    if tries < 10 && format!("{error:#}").contains("menu open on display") =>
                {
                    tries += 1;
                    sleep_ms(300);
                }
                Err(error) if tries < 10 && format!("{error:#}").contains("takeover") => {
                    tries += 1;
                    self.control.call(
                        "debugShow",
                        json!({ "surface": "primary", "displayId": self.id }),
                    )?;
                    sleep_ms(300);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The agent types into the search box: the results open.
    fn open(&self) -> Result<Value> {
        let search = self.on_display(Self::center(&self.state()["search"]))?;
        self.agent(
            "click",
            json!({ "sessionKey": KEY, "displayId": self.id, "x": search.0, "y": search.1 }),
        )?;
        sleep_ms(300);
        // What an earlier case left in the box goes.
        self.agent(
            "key",
            json!({ "sessionKey": KEY, "displayId": self.id, "keys": "cmd+a" }),
        )?;
        // A list in a window of its own gets it in two calls, as an agent
        // often types: the results open on the first, and the second goes
        // to the window that opened, as it went to WeCom's list. Qt's own
        // pop-up is a menu, which takes keys as its choice.
        let calls: &[&str] = if self.mode == "completer" {
            &["item"]
        } else {
            &["i", "tem"]
        };
        for &text in calls {
            self.agent(
                "type",
                json!({ "sessionKey": KEY, "displayId": self.id, "text": text }),
            )?;
            sleep_ms(300);
        }
        self.wait(|s| s["open"] == true && s["items"].as_array().is_some_and(|i| i.len() == 4));
        // Qt may move the list once more after it shows.
        sleep_ms(300);
        Ok(self.state())
    }

    /// Where item2 is in the Viewer, in global points.
    fn item2_in_viewer(&self, state: &Value) -> Result<(f64, f64)> {
        let at = self.item_on_display(state, 1)?;
        let viewer = self.control.call("debugViewer", json!({}))?;
        let image: Vec<f64> = viewer["image"]
            .as_array()
            .context("image")?
            .iter()
            .filter_map(|v| v.as_f64())
            .collect();
        Ok((
            image[0] + at.0 / self.size.0 * image[2],
            image[1] + at.1 / self.size.1 * image[3],
        ))
    }
}

/// The front app's bundle id, as `lsappinfo` says.
fn front_bundle() -> String {
    let front = sh_lenient("/usr/bin/lsappinfo", &["front"]);
    sh_lenient(
        "/usr/bin/lsappinfo",
        &["info", "-only", "bundleid", front.trim()],
    )
    .rsplit('=')
    .next()
    .unwrap_or_default()
    .trim()
    .trim_matches('"')
    .to_string()
}

/// The front app's name, as `lsappinfo` says.
fn front_app() -> String {
    let front = sh_lenient("/usr/bin/lsappinfo", &["front"]);
    sh_lenient(
        "/usr/bin/lsappinfo",
        &["info", "-only", "name", front.trim()],
    )
    .rsplit('=')
    .next()
    .unwrap_or_default()
    .trim()
    .trim_matches('"')
    .to_string()
}

fn test(app: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    open_user_stage(&control)?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "title": "Viewer Qt", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let size = (
        made["width"].as_f64().unwrap_or(1280.0),
        made["height"].as_f64().unwrap_or(800.0),
    );
    let users = front_bundle();
    let mut list = List {
        users,
        control,
        mode: "completer",
        id,
        size,
        state: out().join("qtlist.json"),
        pid: Default::default(),
    };
    for mode in ["completer", "keyed", "frameless"] {
        list.mode = mode;
        cases(&list, app)?;
        // The next kind of list starts in a new process.
        let _ = Command::new("/usr/bin/pkill")
            .args(["-f", "QtList.app/Contents/MacOS/QtList"])
            .status();
        sleep_ms(500);
        // A real click took the display over; Let Continue hands it back.
        list.control.call(
            "debugShow",
            json!({ "surface": "primary", "displayId": id }),
        )?;
        sleep_ms(300);
    }
    // The user's app in front again, as the run found it.
    list.users_app_in_front();
    list.control
        .call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }))?;
    Ok(())
}

/// Every case with the results in a `mode` window (tools/qtlist.py).
fn cases(list: &List, app: &std::path::Path) -> Result<()> {
    let (id, mode) = (list.id, list.mode);
    let log = out().join("qtlist.log");
    let _ = std::fs::remove_file(&list.state);
    let _ = std::fs::remove_file(&log);
    let label = |what: &str| format!("{mode}: {what}");
    let launched = list.agent(
        "launchApp",
        json!({
            "sessionKey": KEY,
            "displayId": id,
            "app": app.to_str(),
            "environment": {
                "QTLIST_STATE": list.state.to_str(),
                "QTLIST_LOG": log.to_str(),
                "QTLIST_POPUP": mode,
            },
        }),
    )?;
    println!("{mode}: launchApp: {launched}");
    list.pid.set(launched["pid"].as_i64().unwrap_or(0));
    // Qt takes a few seconds to start the first time.
    let started = list.wait_for(150, |s| s["rows"].as_array().is_some_and(|r| !r.is_empty()));
    check(
        &label("the Qt list app is up"),
        !started.is_null(),
        started.to_string(),
    );

    // 1. The agent picks a result.
    let opened = list.open()?;
    check(
        &label("the agent's typing opens the Qt results"),
        opened["open"] == true,
        opened.to_string(),
    );
    if opened["open"] == true {
        let at = list.item_on_display(&opened, 1)?;
        mark(list, "agent-pick");
        list.agent(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": at.0, "y": at.1 }),
        )?;
        let got = wait_pick(list);
        check(
            &label("the agent's click on a result picks it"),
            got["got"] == "item2" && is_fresh_pick(list),
            got["got"].to_string(),
        );
    }

    // 2. The Viewer opens while the results are up.
    let opened = list.open()?;
    list.control
        .call("debugShow", json!({ "surface": "viewer", "displayId": id }))?;
    sleep_ms(1200);
    let shown = list.state();
    check(
        &label("opening the Viewer leaves the Qt results up, with nothing picked"),
        opened["open"] == true && shown["open"] == true && shown["got"] == "item2",
        format!("before {}, after {}", opened["open"], shown),
    );

    // 3. The user clicks a result while another app is in front, and the
    // agent opened the results meanwhile, as it did in WeCom. Either the
    // user's own app is in front, or the list's app made itself the front
    // app, as aTrust does and Qt does as it starts: then Agent Screen
    // taking the front makes the list's app lose active.
    for (front, case) in [
        ("your app", "pick-from-other-app"),
        ("the list's app", "pick-from-own-app"),
    ] {
        sign_step("a click on item2 with another app in front / 别的 app 在前面时点 item2");
        if front == "your app" {
            list.users_app_in_front();
        } else {
            list.lists_app_in_front();
        }
        // The agent opens the results with that app in front, and the click
        // follows at once.
        let now = list.open()?;
        if !shows(
            &label(&format!("the results with {front} in front")),
            list,
            &now,
        ) {
            continue;
        }
        let at = list.item2_in_viewer(&now)?;
        let app = front_app();
        check(
            &label(&format!("{front} is in front before the click")),
            !app.starts_with("Agent Screen") && (front == "your app") != (app == "QtList"),
            &app,
        );
        mark(list, case);
        hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
        let got = wait_pick(list);
        check(
            &label(&format!(
                "with {front} in front, a real click in the Viewer on a Qt result picks it, not the row under it"
            )),
            got["got"] == "item2" && is_fresh_pick(list),
            got["got"].to_string(),
        );
        // Then the Viewer has the keyboard, as after any click (ADR 0008).
        let mut key = Value::Null;
        for _ in 0..20 {
            key = list.control.call("debugViewer", json!({}))?["key"].clone();
            if key == true {
                break;
            }
            sleep_ms(100);
        }
        check(
            &label(&format!(
                "with {front} in front, the Viewer is the key window after the click"
            )),
            key == true,
            key.to_string(),
        );
    }

    // 4. The user clicks a result while the Viewer is in front. The last
    // click took the display over; Let Continue hands it back.
    list.control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    sleep_ms(300);
    let opened = list.open()?;
    if shows(
        &label("the results with the Viewer in front"),
        list,
        &opened,
    ) {
        let at = list.item2_in_viewer(&opened)?;
        mark(list, "pick-with-viewer-in-front");
        hid(&["click", &at.0.to_string(), &at.1.to_string()])?;
        let got = wait_pick(list);
        check(
            &label(
                "with the Viewer in front, a real click on a Qt result picks it, not the row under it",
            ),
            got["got"] == "item2" && is_fresh_pick(list),
            got["got"].to_string(),
        );
    }

    println!(
        "{mode}: the Qt list's log:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    Ok(())
}

/// Whether the results are open and on the display, where the user sees
/// them, and checks it: Qt may count a list open that macOS keeps off
/// screen, and the user can't click that.
fn shows(what: &str, list: &List, state: &Value) -> bool {
    check(
        &format!("{what} show on the display"),
        list.list_window(state).is_some(),
        state.to_string(),
    )
}

/// The state once the click since the last case mark picked a result or
/// pressed a row: Qt's pop-up closes before it reports the pick.
fn wait_pick(list: &List) -> Value {
    for _ in 0..30 {
        let log = std::fs::read_to_string(out().join("qtlist.log")).unwrap_or_default();
        let last = log.rsplit("--- ").next().unwrap_or_default();
        if last.contains("pick ") || last.contains("press row") {
            break;
        }
        sleep_ms(100);
    }
    sleep_ms(100);
    list.state()
}

/// A line in the Qt list's log that the next case starts here.
fn mark(list: &List, case: &str) {
    if let Some(log) = list.state.parent().map(|d| d.join("qtlist.log")) {
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .and_then(|mut f| {
                std::io::Write::write_all(&mut f, format!("--- {case}\n").as_bytes())
            });
    }
}

/// Whether the log shows a pick of item2 after the last case mark, with no
/// press on a row: `got` alone still says item2 from an earlier case.
fn is_fresh_pick(list: &List) -> bool {
    let log = list
        .state
        .parent()
        .map(|d| d.join("qtlist.log"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let last = log.rsplit("--- ").next().unwrap_or_default();
    last.contains("pick item2") && !last.contains("press row")
}
