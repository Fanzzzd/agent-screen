//! The agents' Chrome window stays where it is through a Takeover and the
//! hand-back (#319). launch_app opens a page in the agents' own Chrome
//! instance (ADR 0022) on the Agent Display; the agent asks for help, the
//! user takes the display over and hands it back without changing
//! anything: by a Takeover alone, the Help Card's Take Over, the Viewer's
//! button, and a click on the tab strip in the Viewer, with the Viewer on
//! a stage display. The window's frame, sampled every 50 ms from before
//! the request until 4 s after the hand-back, never changes, stays inside
//! the display, and list_windows still lists it there. The jump the issue
//! saw doesn't happen on every Mac: a last round moves the window during
//! the Takeover until only its left 20 points show, on the display's right
//! edge, and the hand-back must put it back. The Viewer takes the
//! keyboard, so its sign asks for hands off.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e chrometakeover
//!
//! With your Chrome running, the agents' instance starts beside it and the
//! script never touches your windows. Without it, a stand-in for yours
//! starts first on a throwaway profile, on the display. Makes 2 displays,
//! the stage and the Agent Display.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_screen_tests::{
    Control, check, cut, instance, json, open_stage, out, probe, quit_profile, run, running_headed,
    sh_lenient, skip, sleep_ms, start_test_app, stop_test_app, wait_for_unlocked, windows_of,
};
use anyhow::{Context, Result};

const KEY: &str = "chrometakeover";
const APP: &str = "Google Chrome";
const BUNDLE: &str = "com.google.Chrome";

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>chrometakeover</title></head>
<body style="font: 16px -apple-system; padding: 24px"><h1>chrometakeover</h1></body></html>"#;

fn pids() -> Vec<u32> {
    sh_lenient("/usr/bin/pgrep", &["-x", APP])
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// The test instance's profile for the agents' instance (ADR 0022).
fn agents_profile() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home)
        .join("Library/Application Support/Agent Screen")
        .join(format!(
            "Browser Profiles ({})",
            instance().unwrap_or_default()
        ))
        .join(BUNDLE)
}

fn serve() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = answer(stream);
            });
        }
    });
    Ok(port)
}

fn answer(mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{PAGE}",
        PAGE.len()
    )?;
    Ok(())
}

/// A display's global bounds: x, y, width, height.
fn bounds(id: u64) -> Result<[f64; 4]> {
    let said = probe(&["bounds", &id.to_string()])?;
    let v: Vec<f64> = said
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    v.try_into()
        .map_err(|_| anyhow::anyhow!("no bounds in {said:?}"))
}

type Frame = [f64; 4];
/// Each frame the window had, from when, in ms since the watch began.
type Seen = Arc<Mutex<Vec<(u128, Option<Frame>)>>>;

fn frame_of(pid: u32, window: u64) -> Option<Frame> {
    windows_of(pid)
        .into_iter()
        .find(|f| f.id == window)
        .map(|f| [f.x, f.y, f.width, f.height])
}

fn inside(f: Frame, d: [f64; 4]) -> bool {
    f[0] >= d[0] - 1.0
        && f[1] >= d[1] - 1.0
        && f[0] + f[2] <= d[0] + d[2] + 1.0
        && f[1] + f[3] <= d[1] + d[3] + 1.0
}

/// Samples the window's frame every 50 ms and keeps each change, in ms
/// since the watch began.
struct Watch {
    stop: Arc<AtomicBool>,
    seen: Seen,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    fn start(pid: u32, window: u64) -> Watch {
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Seen = Arc::default();
        let (s, v) = (stop.clone(), seen.clone());
        let thread = std::thread::spawn(move || {
            let began = std::time::Instant::now();
            let mut last: Option<Option<Frame>> = None;
            while !s.load(Ordering::SeqCst) {
                let now = frame_of(pid, window);
                if last != Some(now) {
                    v.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((began.elapsed().as_millis(), now));
                    last = Some(now);
                }
                sleep_ms(50);
            }
        });
        Watch {
            stop,
            seen,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Vec<(u128, Option<Frame>)> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    if !agent_screen_model::agent_browser::verified(BUNDLE) {
        skip("Chrome isn't on agent_browser::VERIFIED");
    }
    if !wait_for_unlocked() {
        return ExitCode::FAILURE;
    }
    let stand_in = !running_headed(APP);
    let port = match serve() {
        Ok(port) => port,
        Err(e) => {
            eprintln!("no loopback server: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let profile = agents_profile();
    let user_profile = out().join("chrometakeover-user-profile");
    run(|| {
        start_test_app(&[
            ("AGENTSCREEN_LANG", "en"),
            ("AGENTSCREEN_NO_SHARED_FALLBACK", "1"),
        ])?;
        let result = test(stand_in, port, &user_profile);
        quit_profile(&profile);
        if stand_in {
            quit_profile(&user_profile);
        }
        stop_test_app();
        result
    })
}

fn test(stand_in: bool, port: u16, user_profile: &Path) -> Result<()> {
    let control = Control::connect()?;
    // The Viewer opens on a stage display, out of the user's sight.
    open_stage(&control)?;
    let made = control.call(
        "createDisplay",
        json!({ "sessionKey": KEY, "client": "Claude Code", "title": "Chrome takeover", "width": 1280, "height": 800 }),
    )?;
    let id = made["displayId"].as_u64().context("display id")?;
    let display = bounds(id)?;
    if stand_in {
        // Stands in for the user's own Chrome.
        let _ = std::fs::remove_dir_all(user_profile);
        control.call(
            "launchApp",
            json!({ "sessionKey": KEY, "displayId": id, "app": APP, "arguments": [
                format!("--user-data-dir={}", user_profile.display()),
                "--no-first-run", "--no-default-browser-check",
            ] }),
        )?;
        sleep_ms(1000);
    }
    let before = pids();
    let launched = control.call(
        "launchApp",
        json!({ "sessionKey": KEY, "displayId": id, "app": APP,
                "open": format!("http://127.0.0.1:{port}/page") }),
    )?;
    let window = launched["window"]["windowId"]
        .as_u64()
        .with_context(|| format!("no window: {launched}"))?;
    let pid = pids()
        .into_iter()
        .find(|p| !before.contains(p))
        .with_context(|| format!("no agents' instance: {before:?} -> {:?}", pids()))?;
    let args = sh_lenient("/bin/ps", &["-o", "args=", "-p", &pid.to_string()]);
    check(
        "launch_app opened the page in the agents' instance, with the pipe",
        args.contains("--remote-debugging-pipe"),
        cut(&args, 300),
    );
    // Settled: the page loaded and the window stopped moving.
    sleep_ms(2000);
    let start = frame_of(pid, window).context("no frame for the agents' window")?;
    println!("display {display:?}, agents' window {window} at {start:?}");
    check(
        "the agents' window starts inside the display",
        inside(start, display),
        format!("{start:?} in {display:?}"),
    );

    // As the issue: the agent asks for help, the user takes over and hands
    // back. The Takeover alone, then through the Help Card's Take Over, the
    // Viewer's button, and a click on the empty part of the tab strip as
    // the user's first input.
    let rounds: [(&str, &[(&str, &str)]); 5] = [
        ("a Takeover", &[("takeover", "on"), ("takeover", "off")]),
        (
            "the Help Card's Take Over and Let Continue in the Viewer",
            &[("show", "help"), ("show", "primary")],
        ),
        (
            "Take Over and Let Continue in the Viewer",
            &[("show", "viewer"), ("show", "primary"), ("show", "primary")],
        ),
        (
            "a click on the tab strip in the Viewer and Let Continue",
            &[("show", "viewer"), ("click", "strip"), ("show", "primary")],
        ),
        // The jump the issue saw, which doesn't happen on every Mac: during
        // the Takeover the window leaves for no display. The hand-back puts
        // it back.
        (
            "a window that left for no display during the Takeover",
            &[("takeover", "on"), ("strand", ""), ("takeover", "off")],
        ),
    ];
    for (name, steps) in rounds {
        round(&control, id, display, pid, window, start, name, steps)?;
        // The Help Card and the Viewer close between rounds.
        control.call(
            "debugShow",
            json!({ "surface": "collapse", "displayId": id }),
        )?;
        sleep_ms(500);
    }
    Ok(())
}

/// Move the agents' page window to `left`, `top`, through the DevTools
/// pipe.
fn strand(control: &Control, pid: u32, left: f64, top: f64) -> Result<()> {
    let devtools = |method: &str, params: serde_json::Value| {
        control.call(
            "debugDevtools",
            json!({ "pid": pid, "method": method, "params": params }),
        )
    };
    let targets = devtools("Target.getTargets", json!({}))?;
    let target = targets["result"]["targetInfos"]
        .as_array()
        .and_then(|t| {
            t.iter()
                .find(|t| t["type"] == "page" && t["title"] == "chrometakeover")
        })
        .and_then(|t| t["targetId"].as_str())
        .with_context(|| format!("no chrometakeover page: {}", cut(&targets.to_string(), 400)))?
        .to_string();
    let window = devtools("Browser.getWindowForTarget", json!({ "targetId": target }))?;
    let window = window["result"]["windowId"]
        .as_i64()
        .with_context(|| format!("no window for the page: {window}"))?;
    devtools(
        "Browser.setWindowBounds",
        json!({ "windowId": window, "bounds": { "left": left as i64, "top": top as i64 } }),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn round(
    control: &Control,
    id: u64,
    display: [f64; 4],
    pid: u32,
    window: u64,
    start: Frame,
    name: &str,
    steps: &[(&str, &str)],
) -> Result<()> {
    let watch = Watch::start(pid, window);
    control.call(
        "requestHelp",
        json!({ "displayId": id, "reason": "Please check the page." }),
    )?;
    sleep_ms(500);
    let resumed = control.begin(
        "waitForControl",
        json!({ "displayId": id, "timeoutSeconds": 20 }),
    )?;
    let mut stranded = false;
    for &(kind, what) in steps {
        match kind {
            "strand" => {
                // Through Chrome's own window bounds, as Chrome moves it.
                // macOS keeps a title bar on some display, so the window
                // can't go above the main display here as in the issue:
                // its left 20 points stay on the Agent Display's right
                // edge, the rest right of it. The stage and the spares
                // are stacked above and below the display, none beside it.
                let (left, top) = (display[0] + display[2] - 20.0, start[1]);
                strand(control, pid, left, top)?;
                sleep_ms(300);
                let now = frame_of(pid, window);
                check(
                    &format!("{name}: only 20 points of the window show"),
                    now.is_some_and(|f| f[0] >= display[0] + display[2] - 21.0),
                    format!("{now:?}"),
                );
                stranded = true;
            }
            "click" => {
                // 60% across the window's top strip, 6 points down: where
                // a wake click would go.
                let x = (start[0] - display[0] + start[2] * 0.6) / display[2];
                let y = (start[1] - display[1] + 6.0) / display[3];
                control.call(
                    "debugViewerInput",
                    json!({ "kind": "click", "x": x, "y": y }),
                )?;
            }
            "takeover" => {
                control.call(
                    "debugTakeover",
                    json!({ "displayId": id, "active": what == "on" }),
                )?;
            }
            _ => {
                control.call("debugShow", json!({ "surface": what, "displayId": id }))?;
            }
        }
        sleep_ms(1500);
    }
    let outcome = resumed.wait()?;
    check(
        &format!("{name}: the hand-back answers the wait"),
        outcome["outcome"] == "returned",
        outcome.to_string(),
    );
    sleep_ms(4000);
    let seen = watch.finish();
    let end = frame_of(pid, window);
    let moved: Vec<String> = seen
        .iter()
        .filter(|(_, f)| *f != Some(start))
        .map(|(ms, f)| format!("{ms} ms: {f:?}"))
        .collect();
    if !stranded {
        check(
            &format!("{name}: the agents' window never moved"),
            moved.is_empty(),
            moved.join(", "),
        );
    }
    check(
        &format!("{name}: the agents' window is inside the display after the hand-back"),
        end.is_some_and(|f| inside(f, display)),
        format!("{end:?} in {display:?}"),
    );
    let listed = control.call("listWindows", json!({ "displayId": id }))?;
    let there = listed["windows"]
        .as_array()
        .is_some_and(|w| w.iter().any(|w| w["windowId"].as_u64() == Some(window)));
    check(
        &format!("{name}: list_windows still lists the agents' window on the display"),
        there,
        cut(&listed.to_string(), 400),
    );
    if let Some(f) = end.filter(|f| !inside(*f, display)) {
        // Put it back for the next round, as place_window would.
        println!("putting window {window} back from {f:?}");
        control.call(
            "debugMoveWindow",
            json!({ "pid": pid, "windowId": window, "x": start[0], "y": start[1] }),
        )?;
        sleep_ms(500);
    }
    Ok(())
}
