//! Drags in a web page go through the app's private DevTools pipe (#37,
//! ADR 0010). Chromium reads a mouse event's `buttons` from WindowServer's
//! button state for the whole session, which events posted to one process
//! don't set, so a page saw every move of an agent's drag with no button
//! held.
//!
//! The Electron harness, started by launch_app with a pipe: its splitter,
//! which captures the pointer and follows pointermove only while `buttons`
//! has the left button, as react-resizable-panels does, follows a whole
//! drag and one sent as press, moves and release; an HTML5 drag of a row
//! drops on the page's drop target.
//!
//! The same harness started with `open`, without launch_app and so without
//! a pipe: a drag on its range input sets the value through accessibility
//! (ADR 0013); a drag or a press on the splitter queues an Input Request,
//! is declined as with Not Now, and nothing reaches the page.
//!
//! The same page in Chrome on a throwaway profile, below its tab strip and
//! toolbar, started by launch_app with a pipe: the splitter and the drop.
//!
//! Throughout, a sampler reads the session's left button every 2 ms, as
//! `[NSEvent pressedMouseButtons]` reads it in every app, and the cursor:
//! the button must never read down, and the cursor never go onto the Agent
//! Display.
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e webdrag
//!
//! Makes 1 display. Keep your hands off the mouse: the sampler takes a
//! press of yours for the agent's. Leaves Chrome's part out while Chrome
//! runs.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_screen_tests::{
    Control, Value, check, fixture, instance, json, out, quit_profile, run, running_headed,
    sh_lenient, skip, sleep_ms, start_test_app, stop_test_app, tool,
};
use anyhow::{Context, Result};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> Point;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *mut std::ffi::c_void);
}

/// Whether the session's left button reads held, as `[NSEvent
/// pressedMouseButtons]` reads it in every app (combined session state).
fn left_held() -> bool {
    unsafe { CGEventSourceButtonState(0, 0) }
}

fn cursor() -> Point {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        let at = CGEventGetLocation(event);
        CFRelease(event);
        at
    }
}

/// The Agent Display's frame in global points: x, y, width, height.
static DISPLAY: std::sync::OnceLock<[f64; 4]> = std::sync::OnceLock::new();

/// Reads the button and the cursor every 2 ms until stopped.
struct Sampler {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<(usize, usize, Vec<Point>)>,
}

impl Sampler {
    fn start() -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = stop.clone();
            move || {
                let (mut samples, mut held) = (0, 0);
                let mut places = vec![cursor()];
                while !stop.load(Ordering::Relaxed) {
                    samples += 1;
                    if left_held() {
                        held += 1;
                    }
                    let at = cursor();
                    if places.last() != Some(&at) {
                        places.push(at);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                (samples, held, places)
            }
        });
        Sampler { stop, thread }
    }

    fn finish(self) -> (usize, usize, Vec<Point>) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap_or_default()
    }
}

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    let electron = std::env::var("E2E_ELECTRON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| out().join("Electron.app"));
    if !electron.exists() {
        skip("no Electron.app; copy one to target/e2e or set E2E_ELECTRON");
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let sampler = Sampler::start();
        let result = test(&electron);
        quit_harness();
        let (samples, held, places) = sampler.finish();
        check(
            "the session's left button never read down, for any app",
            samples > 1000 && held == 0,
            format!("{held} of {samples} samples held"),
        );
        // The cursor is the user's: it may move a little under their hand,
        // but never onto the Agent Display.
        let display = DISPLAY.get().copied().unwrap_or_default();
        let on_display = places.iter().filter(|p| {
            p.x >= display[0]
                && p.x < display[0] + display[2]
                && p.y >= display[1]
                && p.y < display[1] + display[3]
        });
        check(
            "the cursor never went onto the Agent Display",
            display[2] > 0.0 && on_display.count() == 0,
            format!(
                "display {display:?}; cursor at {} places, first {:?}, last {:?}",
                places.len(),
                places.first(),
                places.last()
            ),
        );
        stop_test_app();
        result
    })
}

/// This checkout's harness only: another worktree may run its own.
fn quit_harness() {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &["-f", fixture("electron").to_str().unwrap()],
    );
    sleep_ms(1000);
}

/// The harness window, title included.
fn harness(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["kind"] == "window")
            .cloned()
    }))
}

/// A number the page put in its title, as in "held=12"; 0 when absent.
fn seen(window: &Value, key: &str) -> i64 {
    window["title"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Wait for the harness's page; the middle of its splitter, in screenshot
/// pixels.
fn splitter(control: &Control, id: &Value) -> Result<(i64, i64)> {
    let mut window = None;
    for _ in 0..50 {
        window =
            harness(control, id)?.filter(|w| w["title"].as_str().is_some_and(|t| t.contains("w=")));
        if window.is_some() {
            break;
        }
        sleep_ms(200);
    }
    let window = window.context("the harness window didn't show")?;
    let (x, y) = (
        window["x"].as_i64().context("x")?,
        window["y"].as_i64().context("y")?,
    );
    // The splitter: 340 pt from the page's left, 200 to 360 pt down, below
    // a 28 pt title bar. The display is within the image budget, so
    // screenshot pixels are points.
    Ok((x + 346, y + 28 + 280))
}

/// Run `launch`, a launch_app call, while watching the front app and the
/// windows of `owner` on the main screen. The launched app may take the
/// front for a moment, as Electron does to show a window, whoever started
/// it; Agent Screen gives it back (#67), as for an app `open` started.
fn watched(owner: &str, launch: impl FnOnce() -> Result<Value>) -> Result<Value> {
    let probe = tool("probe")?;
    let lister = tool("onscreen")?;
    let fronts = std::process::Command::new(&probe)
        .args(["fronts", "5"])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let stop = Arc::new(AtomicBool::new(false));
    let shown = std::thread::spawn({
        let (stop, owner) = (stop.clone(), owner.to_string());
        move || {
            let mut shown = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                for line in sh_lenient(&lister, &["main"]).lines() {
                    let w: Value = serde_json::from_str(line).unwrap_or_default();
                    if w["owner"] == owner.as_str() && w["layer"] == 0 {
                        shown.push(line.to_string());
                    }
                }
            }
            shown
        }
    });
    sleep_ms(300);
    let launched = launch();
    sleep_ms(1500);
    stop.store(true, Ordering::Relaxed);
    let shown = shown.join().unwrap_or_default();
    let output = fronts.wait_with_output()?;
    let launched = launched?;
    let pid = launched["pid"].as_i64().unwrap_or(0).to_string();
    let changes: Vec<(u64, String)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.to_string()))
        })
        .collect();
    let held_ms: Vec<u64> = changes
        .windows(2)
        .filter(|w| w[0].1 == pid)
        .map(|w| w[1].0 - w[0].0)
        .collect();
    let kept = changes.last().is_some_and(|(_, p)| *p == pid);
    check(
        &format!("{owner}, started with a pipe, held the front at most 300 ms at a time"),
        !kept && held_ms.iter().all(|&ms| ms <= 300),
        format!("pid {pid} held the front for {held_ms:?} ms; changes {changes:?}"),
    );
    // Not a check: an app `open -g -j` starts shows the harness's window
    // on the main screen for a moment too, before Agent Screen moves it.
    println!(
        "     {owner}'s windows on your main screen while it started: {} samples",
        shown.len()
    );
    Ok(launched)
}

/// Start the harness through launch_app, which gives it a pipe.
fn start(control: &Control, id: &Value, electron: &std::path::Path) -> Result<(i64, i64)> {
    watched("Electron", || {
        control.call(
            "launchApp",
            json!({ "displayId": id, "app": electron.to_str(),
                    "arguments": [fixture("electron").to_str()] }),
        )
    })?;
    splitter(control, id)
}

fn test(electron: &std::path::Path) -> Result<()> {
    let control = Control::connect()?;
    let key = "webdrag";
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": key, "title": "Web drag", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].clone();
    let bounds: Vec<f64> = sh_lenient(tool("probe")?, &["bounds", &id.to_string()])
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    if let [x, y, w, h] = bounds[..] {
        let _ = DISPLAY.set([x, y, w, h]);
    }
    let result = (|| -> Result<()> {
        piped(&control, key, &id, electron)?;
        quit_harness();
        unpiped(&control, key, &id, electron)?;
        quit_harness();
        if running_headed("Google Chrome") {
            println!("     Chrome is running: its part is left out");
            return Ok(());
        }
        let result = chrome(&control, key, &id);
        quit_profile(&out().join("webdrag-chrome"));
        result
    })();
    let _ = control.call("endDisplay", json!({ "sessionKey": key, "displayId": id }));
    result
}

fn piped(control: &Control, key: &str, id: &Value, electron: &std::path::Path) -> Result<()> {
    // left_click_drag.
    let (sx, sy) = start(control, id, electron)?;
    let dragged = control.call(
        "drag",
        json!({ "sessionKey": key, "displayId": id,
                "fromX": sx, "fromY": sy, "toX": sx + 80, "toY": sy }),
    );
    sleep_ms(500);
    let now = harness(control, id)?.context("window")?;
    check(
        "a drag's pointermoves have the left button held, and the splitter follows it",
        dragged.is_ok()
            && seen(&now, "free") == 0
            && seen(&now, "held") > 0
            && seen(&now, "split") >= 70,
        format!("{} {dragged:?}", now["title"].as_str().unwrap_or_default()),
    );
    quit_harness();

    // left_mouse_down, two mouse_move, left_mouse_up.
    let (sx, sy) = start(control, id, electron)?;
    let at = |x: i64| json!({ "sessionKey": key, "displayId": id, "x": x, "y": sy });
    let steps = [
        control.call("mouseDown", at(sx)),
        control.call("move", at(sx + 30)),
        control.call("move", at(sx + 60)),
        control.call("mouseUp", at(sx + 60)),
    ];
    sleep_ms(500);
    let now = harness(control, id)?.context("window")?;
    check(
        "moves between mouse_down and mouse_up have the left button held, and the splitter follows",
        steps.iter().all(|s| s.is_ok())
            && seen(&now, "free") == 0
            && seen(&now, "held") > 0
            && seen(&now, "split") >= 50,
        format!(
            "{} {:?}",
            now["title"].as_str().unwrap_or_default(),
            steps.iter().filter(|s| s.is_err()).collect::<Vec<_>>()
        ),
    );

    // An HTML5 drag: row 1 into the drop target. Row 1's middle is 31 pt
    // down the page, the drop target's 405; from the splitter, 346 pt from
    // the left, and 280 down.
    let (row, bin) = ((sx - 246, sy - 249), (sx + 94, sy + 125));
    let dragged = control.call(
        "drag",
        json!({ "sessionKey": key, "displayId": id,
                "fromX": row.0, "fromY": row.1, "toX": bin.0, "toY": bin.1 }),
    );
    sleep_ms(800);
    let now = harness(control, id)?.context("window")?;
    check(
        "an HTML5 drag of a row drops it on the page's drop target",
        dragged.is_ok() && seen(&now, "drag") == 1 && seen(&now, "dropped") == 1,
        format!("{} {dragged:?}", now["title"].as_str().unwrap_or_default()),
    );
    Ok(())
}

/// The harness started with `open`, not launch_app: no pipe.
fn unpiped(control: &Control, key: &str, id: &Value, electron: &std::path::Path) -> Result<()> {
    // Right on the display, without taking the front, as an accessory app
    // whose saved bounds are there.
    sh_lenient(
        "/usr/bin/open",
        &[
            "-n",
            "-g",
            "--env",
            &format!("HARNESS_DISPLAY={id}"),
            "--env",
            "HARNESS_BACKGROUND=1",
            "-a",
            electron.to_str().unwrap_or_default(),
            "--args",
            fixture("electron").to_str().unwrap_or_default(),
        ],
    );
    let (sx, sy) = splitter(control, id)?;
    let window = harness(control, id)?.context("window")?;
    let (wx, wy) = (
        window["x"].as_i64().context("x")?,
        window["y"].as_i64().context("y")?,
    );
    // Accessibility reads the page only once it is turned on.
    let _ = control.call(
        "windowState",
        json!({ "sessionKey": key, "displayId": id, "windowId": window["windowId"] }),
    );

    // The range input: 380 to 580 pt across, 250 to 270 down, value 20 of
    // 0 to 100, so its thumb is about 44 pt in. A drag from the thumb 60 pt
    // to the right moves it by the drag's distance, about 30. A grab away
    // from the thumb says nothing about the slider's direction and is left
    // to the queue.
    let (lx, ly) = (wx + 424, wy + 28 + 260);
    let slid = control.call(
        "drag",
        json!({ "sessionKey": key, "displayId": id,
                "fromX": lx, "fromY": ly, "toX": lx + 60, "toY": ly }),
    );
    sleep_ms(500);
    let now = harness(control, id)?.context("window")?;
    let level = seen(&now, "level");
    check(
        "a slider in a page with no pipe moves through accessibility, the way and about as far as the drag, with no press sent",
        slid.as_ref().is_ok_and(|r| r["route"] == "accessibility")
            && (40..=60).contains(&level)
            && seen(&now, "down") == 0,
        format!("{} {slid:?}", now["title"].as_str().unwrap_or_default()),
    );

    // The splitter has no accessibility value: declining its request as
    // Not Now does must leave it untouched. Approved execution has its own
    // realdrag script, with an explicit hands-off countdown.
    let asked = std::time::Instant::now();
    let dragged = control.begin(
        "drag",
        json!({ "sessionKey": key, "displayId": id,
                "fromX": sx, "fromY": sy, "toX": sx + 80, "toY": sy }),
    )?;
    sleep_ms(800);
    control.call(
        "debugShow",
        json!({
            "sessionKey": key, "displayId": id, "surface": "input-queue-decline",
        }),
    )?;
    let dragged = dragged.wait();
    let waited = asked.elapsed().as_secs();
    sleep_ms(500);
    let now = harness(control, id)?.context("window")?;
    let message = dragged
        .as_ref()
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    check(
        "a drag in a page with no pipe queues an Input Request, waits for an answer, and sends nothing",
        message.contains("input_pending")
            && message.contains("No drag or press was sent")
            && message.contains("declined")
            && seen(&now, "down") == 0
            && seen(&now, "held") == 0,
        format!(
            "{}s {} {message}",
            waited,
            now["title"].as_str().unwrap_or_default()
        ),
    );

    // A press there queues too, with no half-click; the moves and the
    // release after it find nothing held.
    let at =
        |x: i64| json!({ "sessionKey": key, "displayId": id, "x": x, "y": sy, "realInput": false });
    let down = control.begin("mouseDown", at(sx))?;
    sleep_ms(800);
    control.call(
        "debugShow",
        json!({
            "sessionKey": key, "displayId": id, "surface": "input-queue-decline",
        }),
    )?;
    let down = down.wait();
    let moved = control.call("move", at(sx + 30));
    let up = control.call("mouseUp", at(sx));
    sleep_ms(300);
    let now = harness(control, id)?.context("window")?;
    check(
        "a press in a page with no pipe queues, sends no press, and the splitter never follows",
        down.as_ref()
            .is_err_and(|e| e.to_string().contains("input_pending"))
            && seen(&now, "down") == 0
            && seen(&now, "held") == 0
            && seen(&now, "split") == 0,
        format!(
            "{} down {down:?} move {moved:?} up {up:?}",
            now["title"].as_str().unwrap_or_default()
        ),
    );
    let _ = control.call(
        "inputQueue",
        json!({ "sessionKey": key, "displayId": id, "action": "done" }),
    );
    Ok(())
}

/// The middle of a labelled element in a window state's tree.
fn center(tree: &str, label: &str) -> Result<(i64, i64)> {
    let line = tree
        .lines()
        .find(|l| l.contains(&format!("\"{label}\"")))
        .with_context(|| format!("no {label} in the tree"))?;
    let at = line
        .split(" at (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .with_context(|| format!("no position in {line}"))?;
    let (x, y) = at.split_once(',').context("x,y")?;
    Ok((x.trim().parse()?, y.trim().parse()?))
}

/// Chrome on a throwaway profile, started by launch_app with a pipe, below
/// its tab strip and toolbar: the same splitter and HTML5 drop.
fn chrome(control: &Control, key: &str, id: &Value) -> Result<()> {
    let profile = out().join("webdrag-chrome");
    let _ = std::fs::remove_dir_all(&profile);
    let page = format!("file://{}", fixture("web/drag.html").display());
    let launched = watched("Google Chrome", || {
        control.call(
            "launchApp",
            json!({ "sessionKey": key, "displayId": id, "app": "Google Chrome", "arguments": [
                format!("--user-data-dir={}", profile.display()),
                "--no-first-run", "--no-default-browser-check", page,
            ] }),
        )
    })?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .context("no window")?;
    sleep_ms(1500);
    let state = control.call(
        "windowState",
        json!({ "sessionKey": key, "displayId": id, "windowId": window }),
    )?;
    let tree = state["tree"].as_str().unwrap_or_default();
    let title = || -> Result<String> {
        let list = control.call("listWindows", json!({ "displayId": id }))?;
        Ok(list["windows"]
            .as_array()
            .and_then(|w| w.iter().find(|w| w["windowId"].as_u64() == Some(window)))
            .and_then(|w| w["title"].as_str())
            .unwrap_or_default()
            .to_string())
    };
    let split = center(tree, "splitter")?;
    let (row, bin) = (center(tree, "row")?, center(tree, "bin")?);
    let dragged = control.call(
        "drag",
        json!({ "sessionKey": key, "displayId": id,
                "fromX": split.0, "fromY": split.1, "toX": split.0 + 80, "toY": split.1 }),
    );
    sleep_ms(500);
    let now = json!({ "title": title()? });
    check(
        "in Chrome, a drag's pointermoves have the left button held, and the splitter follows it",
        dragged.is_ok()
            && seen(&now, "free") == 0
            && seen(&now, "held") > 0
            && seen(&now, "split") >= 70,
        format!("{} {dragged:?}", now["title"].as_str().unwrap_or_default()),
    );
    let dragged = control.call(
        "drag",
        json!({ "sessionKey": key, "displayId": id,
                "fromX": row.0, "fromY": row.1, "toX": bin.0, "toY": bin.1 }),
    );
    sleep_ms(800);
    let now = json!({ "title": title()? });
    check(
        "in Chrome, an HTML5 drag of a row drops it on the page's drop target",
        dragged.is_ok() && seen(&now, "dragged") == 1 && seen(&now, "dropped") == 1,
        format!("{} {dragged:?}", now["title"].as_str().unwrap_or_default()),
    );
    Ok(())
}
