//! Reliability checks from the launch reviews: what an agent gets back, and
//! that nothing lands on the user's screen. Run with the app open and
//! TextEdit and Calculator not in use:
//!
//!     cargo e2e robust
//!
//! With SIDESCREEN_INSTANCE=e2e it starts the debug app beside the user's
//! and quits it afterwards, so it can run while the user works.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use sidescreen_tests::{
    Control, McpClient, ToolResult, Value, Window, check, cut, in_test_app, instance, json, now_ms,
    quit_app, quit_discarding, quoted, run, running, sh, sh_lenient, skip, sleep_ms, tool, try_osa,
    wait_for_idle, watch_flash,
};

/// An on-screen window, as tools/onscreen.swift lists it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct OnScreen {
    owner: String,
    name: String,
    layer: i64,
    x: f64,
    y: f64,
}

/// A starting app is hidden until its window is on the display, so it never
/// shows on the user's screen for more than two frames. A running app's new
/// window shows there until the catch moves it: flash measured 29 ms median,
/// 55 ms p90 and 138 ms at most over 30 launches.
const FLASH_LIMIT: i64 = 34;
const RUNNING_FLASH_LIMIT: i64 = 150;

struct Robust {
    client: McpClient,
    display_id: u64,
    lister: PathBuf,
}

impl Robust {
    fn computer(&self, mut args: Value) -> Result<ToolResult> {
        args["display_id"] = json!(self.display_id);
        self.client.tool("computer", args)
    }

    /// On-screen windows on a display ("main" or an id).
    fn windows_on(&self, display: &str) -> Result<Vec<Value>> {
        Ok(sh(&self.lister, &[display])?
            .lines()
            .filter(|l| !l.is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?)
    }

    /// Windows of an app on the user's main display, which must stay empty.
    fn on_main_display(&self, app: &str) -> Result<usize> {
        let found: Vec<OnScreen> = self
            .windows_on("main")?
            .into_iter()
            .filter_map(|w| serde_json::from_value::<OnScreen>(w).ok())
            .filter(|w| w.owner == app && w.layer == 0)
            .collect();
        for w in &found {
            println!("  on main: {} at {},{}", w.name, w.x, w.y);
        }
        Ok(found.len())
    }

    fn launch(&self, app: &str) -> Result<ToolResult> {
        self.client.tool(
            "launch_app",
            json!({ "display_id": self.display_id, "app": app }),
        )
    }

    fn list(&self, strays: bool) -> Result<ToolResult> {
        let mut args = json!({ "display_id": self.display_id });
        if strays {
            args["strays"] = json!(true);
        }
        self.client.tool("list_windows", args)
    }

    /// The display's windows of `app`; fails when list_windows lists none.
    fn windows_of(&self, app: &str) -> Result<Vec<Window>> {
        Ok(self
            .list(false)?
            .windows()
            .context("list_windows gave no windows")?
            .into_iter()
            .filter(|w| w.app == app)
            .collect())
    }

    fn state(&self, window_id: u64) -> Result<String> {
        Ok(self
            .client
            .tool(
                "get_window_state",
                json!({ "display_id": self.display_id, "window_id": window_id }),
            )?
            .text()
            .to_string())
    }

    fn takeover(&self, viewer: &Control, active: bool) -> Result<Value> {
        viewer.call(
            "debugTakeover",
            json!({ "displayId": self.display_id, "active": active }),
        )
    }
}

/// The first capture of `pattern` in `text`.
fn capture(pattern: &Regex, text: &str) -> Option<String> {
    pattern.captures(text).map(|c| c[1].to_string())
}

fn main() -> ExitCode {
    if running("TextEdit") || running("Calculator") {
        skip("TextEdit or Calculator is the user's");
    }
    // Launching windows can show them on the user's screen for a moment.
    if instance().is_some() && !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| in_test_app(test))
}

fn test() -> Result<()> {
    let lister = tool("onscreen")?;
    tool("flash")?;
    let client = McpClient::start(&[("CLAUDE_CODE_SESSION_ID", &format!("robust-{}", now_ms()))])?;
    let display_id = client
        .tool("create_display", json!({ "title": "Robust" }))?
        .display_id()
        .context("create_display gave no display")?;
    let robust = Robust {
        client,
        display_id,
        lister,
    };

    // Files TextEdit has open stay until the end: deleting one under it can
    // raise an autosave sheet later.
    let mut temporary = Vec::new();
    let result = body(&robust, &mut temporary);

    let _ = robust
        .client
        .tool("end_display", json!({ "display_id": display_id }));
    quit_discarding("TextEdit");
    quit_app("Calculator");
    for f in &temporary {
        sh_lenient("/bin/rm", &["-f", f]);
    }
    robust.client.close();
    result
}

fn body(r: &Robust, temporary: &mut Vec<String>) -> Result<()> {
    let client = &r.client;
    let display_id = r.display_id;
    let textarea_value = Regex::new(r#"textarea[^\n]*value="([^"]*)""#)?;
    let doc_text = |id: u64| -> Result<String> {
        Ok(capture(&textarea_value, &r.state(id)?).unwrap_or_default())
    };

    // An empty display: every screenshot succeeds, small, with no wallpaper.
    let mut sizes = Vec::new();
    let mut errors = Vec::new();
    for i in 0..20 {
        let shot = r.computer(json!({ "action": "screenshot" }))?;
        if shot.is_error {
            errors.push(shot.text().to_string());
            if errors.len() == 1 {
                println!(
                    "  on the display: {}",
                    Value::Array(r.windows_on(&display_id.to_string())?)
                );
            }
        } else {
            let image = shot.image().unwrap_or_default();
            sizes.push(image.len());
            // Keep a large one to see what got in.
            if image.len() >= 30_000 {
                std::fs::write(format!("/tmp/robust-large-{i}.jpg"), &image)?;
            }
        }
    }
    check(
        "20 screenshots of an empty display succeed",
        errors.is_empty(),
        format!(
            "{} failed{}",
            errors.len(),
            errors
                .first()
                .map_or(String::new(), |e| format!(", first: {e}"))
        ),
    );
    let largest = sizes.iter().max();
    check(
        "an empty display's screenshot is plain",
        largest.is_some_and(|&l| l < 30_000),
        format!("largest {} bytes", json!(largest)),
    );

    // TextEdit from not running: a document window on the agent's display.
    let (first_flash, launched) = watch_flash("TextEdit", || r.launch("TextEdit"))?;
    check(
        "TextEdit's first window shows on the user's screen 34 ms at most",
        first_flash.ms <= FLASH_LIMIT,
        format!("{} windows for {} ms", first_flash.windows, first_flash.ms),
    );
    let window = launched.window();
    check(
        "TextEdit's first launch opens a document window",
        window.as_ref().is_some_and(|w| w.app == "TextEdit"),
        launched.text(),
    );
    check(
        "no TextEdit window on the user's screen",
        r.on_main_display("TextEdit")? == 0,
        "",
    );
    if let Some(window) = &window {
        let id = window.window_id;
        check(
            "the new document starts empty, with nothing restored",
            doc_text(id)?.is_empty(),
            quoted(&doc_text(id)?),
        );
        // The judge's sequence: type, select all, type over it, with no click.
        r.computer(json!({ "action": "type", "text": "one two" }))?;
        r.computer(json!({ "action": "key", "text": "cmd+a" }))?;
        r.computer(json!({ "action": "type", "text": "X" }))?;
        check(
            "type, cmd+a, type replaces the text",
            doc_text(id)? == "X",
            quoted(&doc_text(id)?),
        );
        r.computer(json!({ "action": "key", "text": "cmd+a" }))?;
        r.computer(json!({ "action": "key", "text": "BackSpace" }))?;
        r.computer(json!({ "action": "type", "text": "Hello from an agent" }))?;
        check(
            "typing reaches the document",
            r.state(id)?.contains("Hello from an agent"),
            "",
        );

        // Menu shortcuts reach an app in the background.
        r.computer(json!({ "action": "key", "text": "cmd+a" }))?;
        r.computer(json!({ "action": "type", "text": "Replaced" }))?;
        let value = capture(&textarea_value, &r.state(id)?);
        check(
            "cmd+a selects all in a background app",
            value.as_deref() == Some("Replaced"),
            value.as_deref().map_or(String::new(), quoted),
        );

        // The user takes over in the middle of a long type: it stops there,
        // and the agent hears how far it got.
        {
            let viewer = Control::connect()?;
            let typing = client.begin(
                "computer",
                json!({ "display_id": display_id, "action": "type", "text": "x".repeat(2000) }),
            )?;
            sleep_ms(400);
            r.takeover(&viewer, true)?;
            let stopped = typing.wait()?;
            r.takeover(&viewer, false)?;
            drop(viewer);
            let text = stopped.text();
            check(
                "a takeover stops the agent's typing partway",
                Regex::new(r"took over display \d+ after \d+ of 2000 characters")?.is_match(text),
                cut(text, 140),
            );
            let typed: i64 = capture(&Regex::new(r"after (\d+) of")?, text)
                .and_then(|n| n.parse().ok())
                .unwrap_or(-1);
            check(
                "it stopped partway, not at the start or the end",
                typed > 0 && typed < 2000,
                typed.to_string(),
            );
        }

        // The same for a held key and a drag: both let go when the user steps in.
        {
            let viewer = Control::connect()?;
            let take_over = |mut args: Value| -> Result<(ToolResult, u128)> {
                args["display_id"] = json!(display_id);
                let work = client.begin("computer", args)?;
                let started = Instant::now();
                sleep_ms(300);
                r.takeover(&viewer, true)?;
                let reply = work.wait()?;
                let ms = started.elapsed().as_millis();
                r.takeover(&viewer, false)?;
                Ok((reply, ms))
            };
            let (held, ms) =
                take_over(json!({ "action": "hold_key", "text": "shift", "duration": 5 }))?;
            check(
                "a takeover releases a held key at once",
                held.text().contains("keys were released early") && ms < 1500,
                format!("{ms} ms: {}", cut(held.text(), 100)),
            );
            let w = window;
            let (dragged, _) = take_over(json!({
                "action": "left_click_drag",
                "start_coordinate": [w.x + 40.0, w.y + 60.0],
                "coordinate": [w.x + w.width - 40.0, w.y + w.height - 40.0],
            }))?;
            check(
                "a takeover stops a drag short of its target",
                dragged.text().contains("let go short of its target"),
                cut(dragged.text(), 120),
            );
        }
    }

    // Two sessions launching the same app at once each get their own window.
    {
        let other = McpClient::start(&[(
            "CLAUDE_CODE_SESSION_ID",
            &format!("robust-other-{}", now_ms()),
        )])?;
        let second = other
            .tool("create_display", json!({ "title": "Robust 2" }))?
            .display_id()
            .context("create_display gave no display")?;
        let a = client.begin(
            "launch_app",
            json!({ "display_id": display_id, "app": "TextEdit" }),
        )?;
        let b = other.begin(
            "launch_app",
            json!({ "display_id": second, "app": "TextEdit" }),
        )?;
        let (a, b) = (a.wait()?, b.wait()?);
        let wa = a.window().map(|w| w.window_id);
        let wb = b.window().map(|w| w.window_id);
        check(
            "concurrent launches get distinct windows",
            wa.is_some() && wb.is_some() && wa != wb,
            format!(
                "{} {} {} {}",
                json!(wa),
                json!(wb),
                cut(a.text(), 80),
                cut(b.text(), 80)
            ),
        );
        let on_second: Vec<u64> = other
            .tool("list_windows", json!({ "display_id": second }))?
            .windows()
            .context("list_windows gave no windows")?
            .iter()
            .map(|w| w.window_id)
            .collect();
        check(
            "the second session's window is on its display",
            wb.is_some_and(|wb| on_second.contains(&wb)),
            json!(on_second).to_string(),
        );
        check(
            "no window on the user's screen after concurrent launches",
            r.on_main_display("TextEdit")? == 0,
            "",
        );
        other.tool("end_display", json!({ "display_id": second }))?;
        other.close();
    }

    // Launched again while running: a second document, still not on the user's screen.
    let (again_flash, again) = watch_flash("TextEdit", || r.launch("TextEdit"))?;
    check(
        "a running app's new window shows on the user's screen 150 ms at most",
        again_flash.ms <= RUNNING_FLASH_LIMIT,
        format!("{} windows for {} ms", again_flash.windows, again_flash.ms),
    );
    check(
        "TextEdit launched again opens a window on the display",
        again.window().is_some_and(|w| w.app == "TextEdit"),
        again.text(),
    );
    check(
        "still no TextEdit window on the user's screen",
        r.on_main_display("TextEdit")? == 0,
        "",
    );

    // A file opened in the running app: the window showing it is the agent's.
    let file = format!("/tmp/sidescreen robust {}.txt", now_ms());
    std::fs::write(&file, "robust\n")?;
    let opened = client.tool(
        "launch_app",
        json!({ "display_id": display_id, "app": "TextEdit", "open": file }),
    )?;
    check(
        "a file opened in a running app lands on the display",
        opened
            .window()
            .is_some_and(|w| w.title.starts_with("sidescreen robust")),
        cut(opened.text(), 160),
    );
    check(
        "the file's window isn't on the user's screen",
        r.on_main_display("TextEdit")? == 0,
        "",
    );
    temporary.push(file);
    // The same with a file URL.
    let url_file = format!("/tmp/sidescreen url {}.txt", now_ms());
    std::fs::write(&url_file, "robust\n")?;
    let by_url = client.tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "TextEdit",
            "open": format!("file://{}", url_file.replace(' ', "%20")),
        }),
    )?;
    check(
        "a file:// URL opens like its path",
        by_url
            .window()
            .is_some_and(|w| w.title.starts_with("sidescreen url")),
        cut(by_url.text(), 160),
    );
    temporary.push(url_file);

    // Copy and paste between two documents, through accessibility.
    {
        let docs = r.windows_of("TextEdit")?;
        let textarea = Regex::new(r"\[(\d+)\] textarea")?;
        let area = |id: u64| -> Result<Option<u64>> {
            Ok(capture(&textarea, &r.state(id)?).and_then(|n| n.parse().ok()))
        };
        let act = |window_id: u64, element: Option<u64>, action: &str, value: Option<&str>| {
            let mut args = json!({
                "display_id": display_id,
                "window_id": window_id,
                "element": element,
                "action": action,
            });
            if let Some(value) = value {
                args["value"] = json!(value);
            }
            client.tool("act_on_element", args)
        };
        let from = docs.first().context("no TextEdit document")?;
        let to = docs.get(1).context("no second TextEdit document")?;
        let long = "Copied text that runs well past the eighty characters the window tree keeps of any value.";
        let source = area(from.window_id)?;
        act(from.window_id, source, "set_value", Some(long))?;
        let read = act(from.window_id, source, "read_text", None)?;
        check(
            "read_text returns the whole value",
            read.text().contains(long),
            cut(read.text(), 120),
        );
        let target = area(to.window_id)?;
        act(to.window_id, target, "set_value", Some("Before. After."))?;
        let pasted = act(to.window_id, target, "insert_text", Some("Pasted. "))?;
        let after = act(to.window_id, target, "read_text", None)?;
        check(
            "insert_text pastes into a background app",
            !pasted.is_error && after.text().contains("Pasted."),
            cut(after.text(), 120),
        );
    }

    // set_value on something without a settable value fails.
    r.launch("Calculator")?;
    let calc = r
        .windows_of("Calculator")?
        .into_iter()
        .next()
        .context("no Calculator window on the display")?;
    let tree = r.state(calc.window_id)?;
    let two: Option<u64> =
        capture(&Regex::new(r#"\[(\d+)\] button "2""#)?, &tree).and_then(|n| n.parse().ok());
    let set = client.tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": calc.window_id,
            "element": two,
            "action": "set_value",
            "value": "9",
        }),
    )?;
    check(
        "set_value on a button fails",
        set.is_error,
        cut(set.text(), 120),
    );

    // place_window takes only strays of apps this session started.
    let strays = r.list(true)?;
    check(
        "list_windows strays lists none when every window is on the display",
        strays.get("windows").as_array().is_some_and(Vec::is_empty),
        strays.structured_content.to_string(),
    );
    let not_ours = client.tool(
        "place_window",
        json!({ "display_id": display_id, "window_id": 1 }),
    )?;
    check(
        "place_window refuses a window that isn't a stray of the session's apps",
        not_ours.is_error,
        cut(not_ours.text(), 120),
    );

    // Calculator runs and has one window: a second launch makes none, says
    // so soon, and takes nothing.
    let started_again = Instant::now();
    let second = r.launch("Calculator")?;
    let took_ms = started_again.elapsed().as_millis();
    let structured = &second.structured_content;
    check(
        "launch_app on a running single-window app says no_new_window within 1.5 s",
        // A window of null, not a missing one.
        structured.get("window") == Some(&Value::Null)
            && structured["reason"] == "no_new_window"
            && took_ms < 1500,
        format!("{} in {took_ms} ms", cut(&structured.to_string(), 120)),
    );

    // Listing the tools tells the running app an agent loaded them, for the
    // setup window.
    client.request("tools/list", json!({}))?;
    sleep_ms(500);
    let ready = Control::connect()?.call("debugAgentReady", json!({}))?;
    check(
        "listing the tools tells the app which agent loaded them",
        ready["agent"] == "smoke",
        ready.to_string(),
    );

    // Window ownership: a window the agent's action opens goes to the display;
    // one that opens with no action near it stays put, and the agent may
    // place it.
    {
        let shown = r.windows_of("TextEdit")?;
        let front = shown.last().context("no TextEdit window on the display")?;
        // The title bar's right end, clear of the title: a click on a
        // document's title can open its rename popover.
        r.computer(json!({
            "action": "left_click",
            "coordinate": [front.x + front.width - 30.0, front.y + 10.0],
        }))?;
        let main_at_key = r.on_main_display("TextEdit")?;
        let key = r.computer(json!({ "action": "key", "text": "cmd+n" }))?;
        sleep_ms(1000);
        let added: Vec<Window> = r
            .windows_of("TextEdit")?
            .into_iter()
            .filter(|w| !shown.iter().any(|s| s.window_id == w.window_id))
            .collect();
        let described: Vec<Value> = added
            .iter()
            .map(|w| json!({ "title": w.title, "kind": w.kind, "size": [w.width, w.height] }))
            .collect();
        check(
            "a window the agent's key press opens lands on the display",
            added.len() == 1 && added[0].kind == "window",
            format!(
                "new on the display: {}; {main_at_key} -> {} on the user's screen; {}",
                json!(described),
                r.on_main_display("TextEdit")?,
                cut(key.text(), 80)
            ),
        );
        sleep_ms(3500);
        let main_before = r.on_main_display("TextEdit")?;
        try_osa("tell application \"TextEdit\" to make new document")?;
        sleep_ms(1500);
        check(
            "a window that opens long after the agent's last action stays put",
            r.on_main_display("TextEdit")? == main_before + 1,
            "",
        );
        let moved = r.computer(json!({ "action": "mouse_move", "coordinate": [1270, 790] }))?;
        check(
            "the agent's next screenshot names the window that stayed",
            moved.text().contains("call place_window with that id"),
            cut(moved.text(), 200),
        );
        let late_strays = r.list(true)?;
        let late = late_strays
            .windows()
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.app == "TextEdit");
        check(
            "that window is a stray the agent may place",
            late.is_some(),
            cut(&late_strays.structured_content.to_string(), 160),
        );
        if let Some(late) = late {
            let placed = client.tool(
                "place_window",
                json!({ "display_id": display_id, "window_id": late.window_id }),
            )?;
            check(
                "place_window moves it",
                !placed.is_error,
                cut(placed.text(), 120),
            );
        }
    }

    // The user takes a TextEdit window back: TextEdit is theirs from then on.
    // A window they make next stays on their screen and isn't a stray.
    let control = Control::connect()?;
    let mine = r
        .windows_of("TextEdit")?
        .into_iter()
        .next()
        .context("no TextEdit window on the display")?;
    control.call(
        "debugTakeBack",
        json!({ "displayId": display_id, "windowId": mine.window_id }),
    )?;
    try_osa("tell application \"TextEdit\" to make new document")?;
    sleep_ms(1500);
    let shown_after: Vec<String> = r
        .windows_of("TextEdit")?
        .iter()
        .map(|w| format!("{} {}", w.window_id, w.title))
        .collect();
    check(
        "after a take-back, a window the user makes stays on their screen",
        r.on_main_display("TextEdit")? >= 2,
        format!(
            "taken back {}; on the display: {}",
            mine.window_id,
            shown_after.join(", ")
        ),
    );
    let after = r.list(true)?;
    check(
        "after a take-back, strays lists none of the user's windows",
        after.get("windows").as_array().is_some_and(Vec::is_empty),
        cut(&after.structured_content.to_string(), 160),
    );
    let back = client.tool(
        "place_window",
        json!({ "display_id": display_id, "window_id": mine.window_id }),
    )?;
    check(
        "place_window refuses the window taken back",
        back.is_error && back.text().contains("took that window back"),
        cut(back.text(), 120),
    );
    Ok(())
}
