//! Posted input into a background Electron app leaves your front app in
//! front (#273). The Electron harness in HARNESS_PAIR mode has its agent
//! window on the display and its user window on your main screen; the
//! harness comes to the front once, so its user window is key, then a
//! stand-in for your own app (keyholder) takes the front. With the
//! harness in the background and its user window key, the agent clicks
//! its page and types a key, then types another after another app came
//! and went. Each wakes the agent's window, which must become the
//! harness's key window and take the input, and hands key status back to
//! the user's window after. Then the user's window closes, leaving the
//! agent's window key, and the agent clicks and types after another app
//! came and went: the focus record had gone to a window already key.
//!
//! The front app is sampled every 5 ms by four sources (probe
//! frontsources): AppKit's activation notices, NSWorkspace, LaunchServices
//! and WindowServer. None may name the harness, the stand-in must never
//! stop being active or lose its key window, and the harness must not
//! say `app active`, which Electron logs when AppKit calls it active: a
//! focus record told it so before (#273).
//!
//! Needs an Electron.app: `target/e2e/Electron.app`, or E2E_ELECTRON.
//! Puts the harness's user window and the stand-in on your main screen
//! for about 50 seconds and takes the front for the stand-in; keep your
//! hands off the keyboard and mouse.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e electronfront
//!
//! Makes 1 display.

use std::io::Read;
use std::process::{Child, ExitCode, Stdio};

use agent_screen_tests::{
    Control, Value, allow_on_your_screen, check, cut, fixture, instance, json, out, restore, run,
    sh_lenient, skip, sleep_ms, snapshot, start_test_app, stop_test_app, tool, wait_for_idle,
};
use anyhow::{Context, Result, bail};

const KEY: &str = "electronfront";

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
    if !wait_for_idle() {
        return ExitCode::FAILURE;
    }
    run(|| {
        allow_on_your_screen(
            "Electron",
            90_000,
            "the harness's user window stands in for your own window of that app, on your screen on purpose",
        );
        allow_on_your_screen(
            "keyholder",
            90_000,
            "the stand-in for your front app, on your screen on purpose",
        );
        let before = snapshot();
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let control = Control::connect()?;
        let made = control.call(
            "createDisplay",
            json!({ "sessionKey": KEY, "title": "Electron front", "width": 1280, "height": 800 }),
        )?;
        let id = made["displayId"].clone();
        let log = out().join("electronfront-keys.log");
        let _ = std::fs::remove_file(&log);
        let mut holder = None;
        let result = test(&control, &id, &electron, &log, &mut holder);
        if let Some(mut h) = holder {
            let _ = h.kill();
            let _ = h.wait();
        }
        quit_harness();
        let _ = control.call("endDisplay", json!({ "sessionKey": KEY, "displayId": id }));
        restore(&before);
        stop_test_app();
        result
    })
}

/// This checkout's harness only: another worktree may run its own.
fn quit_harness() {
    let _ = sh_lenient(
        "/usr/bin/pkill",
        &["-f", fixture("electron").to_str().unwrap_or_default()],
    );
    sleep_ms(1000);
}

/// The harness's log since `from` lines in, with each line's time.
fn timed(log: &std::path::Path, from: usize) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .skip(from)
        .map(String::from)
        .collect()
}

/// The harness's log since `from` lines in, without the times.
fn lines(log: &std::path::Path, from: usize) -> Vec<String> {
    timed(log, from)
        .iter()
        .map(|l| {
            l.split_once(' ')
                .map_or(l.as_str(), |(_, rest)| rest)
                .to_string()
        })
        .collect()
}

fn has(lines: &[String], wanted: &str) -> bool {
    lines.iter().any(|l| l.starts_with(wanted))
}

/// The window that last became the harness's key window. An app that
/// stops being active logs a blur for it, though it stays the app's key
/// window.
fn last_focus(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .rev()
        .find(|l| l.as_str() == "win user focus" || l.as_str() == "win agent focus")
        .cloned()
}

/// The window number of the harness's focused window, as Accessibility
/// names it: the one its keys go to.
fn focused(pid: &str) -> Result<String> {
    Ok(sh_lenient(tool("probe")?, &["focused", pid])
        .trim()
        .to_string())
}

fn agent_window(control: &Control, id: &Value) -> Result<Option<Value>> {
    let list = control.call("listWindows", json!({ "displayId": id }))?;
    Ok(list["windows"].as_array().and_then(|w| {
        w.iter()
            .find(|w| w["app"].as_str() == Some("Electron") && w["title"] == "pair agent")
            .cloned()
    }))
}

fn cycle_front_app(holder_pid: &str, label: &str) -> Result<()> {
    sh_lenient(
        "/usr/bin/osascript",
        &["-e", "tell application \"Finder\" to activate"],
    );
    sleep_ms(500);
    let mid_front = front_app();
    if mid_front != "Finder" {
        bail!("setup ({label}): Finder did not become frontmost (front app: {mid_front:?})");
    }
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            &format!(
                "tell application \"System Events\" to set frontmost of (first process whose unix id is {holder_pid}) to true"
            ),
        ],
    );
    sleep_ms(1200);
    let back_front = sh_lenient(tool("probe")?, &["frontpid"]);
    if back_front.trim() != holder_pid {
        bail!(
            "setup ({label}): stand-in did not regain frontmost (front pid {}, expected {holder_pid})",
            back_front.trim()
        );
    }
    Ok(())
}

fn front_app() -> String {
    sh_lenient(
        "/usr/bin/osascript",
        &[
            "-e",
            "tell application \"System Events\" to get name of first process whose frontmost is true",
        ],
    )
    .trim()
    .to_string()
}

/// The front app as each source names it, every 5 ms, while `act` runs
/// and for a moment after: "epoch-ms source pid name" lines, and
/// "epoch-ms key window" as the harness's focused window changes.
fn sources_during<T>(seconds: u64, pid: &str, act: impl FnOnce() -> T) -> Result<(T, Vec<String>)> {
    let sampler = std::process::Command::new(tool("probe")?)
        .args(["frontsources", &seconds.to_string(), pid])
        .stdout(Stdio::piped())
        .spawn()?;
    sleep_ms(300);
    let result = act();
    let output = sampler.wait_with_output()?;
    Ok((
        result,
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect(),
    ))
}

/// Lines of `sources` that name pid `pid` as the front app.
fn naming(sources: &[String], pid: &str) -> Vec<String> {
    sources
        .iter()
        .filter(|l| {
            let mut words = l.split_whitespace().skip(1);
            words.next() != Some("key") && words.next() == Some(pid)
        })
        .cloned()
        .collect()
}

/// The windows the harness named focused, in order.
fn key_windows(sources: &[String]) -> Vec<String> {
    sources
        .iter()
        .filter_map(|l| {
            let mut words = l.split_whitespace().skip(1);
            (words.next() == Some("key")).then(|| words.next().unwrap_or_default().to_string())
        })
        .collect()
}

/// What the stand-in printed so far, read without waiting.
fn holder_lines(out: &std::sync::Arc<std::sync::Mutex<String>>, from: usize) -> Vec<String> {
    out.lock()
        .map(|s| s.lines().skip(from).map(String::from).collect())
        .unwrap_or_default()
}

fn test(
    control: &Control,
    id: &Value,
    electron: &std::path::Path,
    log: &std::path::Path,
    holder: &mut Option<Child>,
) -> Result<()> {
    sh_lenient(
        "/usr/bin/open",
        &[
            "-n",
            "-g",
            "--env",
            "HARNESS_PAIR=1",
            "--env",
            &format!("HARNESS_DISPLAY={id}"),
            "--env",
            &format!("HARNESS_KEY_LOG={}", log.display()),
            "-a",
            electron.to_str().unwrap_or_default(),
            "--args",
            fixture("electron").to_str().unwrap_or_default(),
        ],
    );
    let mut window = None;
    for _ in 0..60 {
        if has(&lines(log, 0), "ready") {
            window = agent_window(control, id)?;
            if window.is_some() {
                break;
            }
        }
        sleep_ms(250);
    }
    let window = window.with_context(|| {
        format!(
            "the harness's agent window didn't show on the display; log: {:?}",
            lines(log, 0)
        )
    })?;
    let window_id = window["windowId"].as_u64().context("windowId")?;
    control.call(
        "debugHandOver",
        json!({ "displayId": id, "windowId": window_id }),
    )?;
    sleep_ms(800);
    let pid = lines(log, 0)
        .iter()
        .find_map(|l| l.strip_prefix("ready ").map(str::to_string))
        .context("the harness's log names no pid")?;
    let pid = pid.as_str();
    // The user working in the harness: it comes to the front, and its
    // user window becomes key.
    for _ in 0..2 {
        sh_lenient(
            "/usr/bin/osascript",
            &[
                "-e",
                &format!(
                    "tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true"
                ),
            ],
        );
        sleep_ms(1500);
        if last_focus(&lines(log, 0)).as_deref() == Some("win user focus")
            && front_app() == "Electron"
        {
            break;
        }
        println!("     the harness didn't come to the front with its user window key; once more");
    }
    // Then the user switches to another app: the stand-in takes the front,
    // and the harness keeps its user window as its key window.
    let mut child = agent_screen_tests::command(tool("keyholder")?)
        .args(["60", "app"])
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().context("keyholder stdout")?;
    let printed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let printed = printed.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = printed.lock() {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        });
    }
    let holder_pid = child.id().to_string();
    *holder = Some(child);
    sleep_ms(1500);
    let start = lines(log, 0);
    let front = sh_lenient(tool("probe")?, &["frontpid"]);
    let key_window = focused(pid)?;
    if front.trim() != holder_pid
        || last_focus(&start).as_deref() != Some("win user focus")
        || key_window == window_id.to_string()
    {
        bail!(
            "setup: the stand-in isn't in front with the harness's user window key: front pid {}, stand-in {holder_pid}, focused window {key_window}, log {start:?}, stand-in said {:?}",
            front.trim(),
            holder_lines(&printed, 0)
        );
    }
    println!("     harness pid {pid}, stand-in pid {holder_pid}");

    let tree = control.call(
        "windowState",
        json!({ "sessionKey": KEY, "displayId": id, "windowId": window_id }),
    )?;
    let tree = tree["tree"].as_str().unwrap_or_default().to_string();
    let field = point_of(&tree, "agent field")?;
    let first = point_of(&tree, "agent first")?;

    // 1. A click on the agent's field, then a key.
    let from = lines(log, 0).len();
    let said_from = holder_lines(&printed, 0).len();
    let ((clicked, keyed), sources) = sources_during(4, pid, || {
        let clicked = control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": field.0, "y": field.1 }),
        );
        sleep_ms(600);
        let keyed = control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": "q" }),
        );
        (clicked, keyed)
    })?;
    sleep_ms(500);
    report(
        "a click and a key into the background harness",
        log,
        from,
        &sources,
        (pid, window_id),
        ("user", &key_window),
        &holder_lines(&printed, said_from),
    );
    let seen = lines(log, from);
    // The field's own events: the click focused it, then the key reached
    // it. A dropped click would leave the key to the page, which logs
    // "keydown-page".
    let clicked_in = seen.iter().position(|l| l == "pair agent active field");
    let typed_in = seen.iter().position(|l| l == "pair agent keydown \"q\"");
    check(
        "the click focuses the agent's field and the key reaches it",
        clicked.is_ok()
            && keyed.is_ok()
            && matches!((clicked_in, typed_in), (Some(c), Some(k)) if c < k),
        format!(
            "{} | {} | log {seen:?}",
            cut(&said(&clicked), 300),
            cut(&said(&keyed), 300)
        ),
    );

    // 2. Another app takes the front and gives it back: the agent's
    // window lost key status, and the next key wakes it again.
    cycle_front_app(&holder_pid, "round 2")?;
    let from = lines(log, 0).len();
    let said_from = holder_lines(&printed, 0).len();
    let (keyed, sources) = sources_during(4, pid, || {
        control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": "w" }),
        )
    })?;
    sleep_ms(500);
    report(
        "a key after the front app changed",
        log,
        from,
        &sources,
        (pid, window_id),
        ("user", &key_window),
        &holder_lines(&printed, said_from),
    );
    let seen = lines(log, from);
    check(
        "the key reaches the agent's field",
        keyed.is_ok() && seen.iter().any(|l| l == "pair agent keydown \"w\""),
        format!("{} | log {seen:?}", cut(&said(&keyed), 300)),
    );

    // 3. The agent's window is already the harness's key window when the
    // front app changes: the user closed theirs, leaving the agent's the
    // app's only window, and the agent typed into it since.
    let closed = sh_lenient(tool("probe")?, &["close", pid, &key_window]);
    sleep_ms(800);
    let keyed = control.call(
        "key",
        json!({ "sessionKey": KEY, "displayId": id, "keys": "e" }),
    );
    sleep_ms(500);
    let agent_key = focused(pid)?;
    check(
        "setup: with the user's window closed, the agent's window is the harness's key window",
        closed.trim() == "closed" && keyed.is_ok() && agent_key == window_id.to_string(),
        format!(
            "close said {}, {}, focused window {agent_key}, the agent's {window_id}",
            closed.trim(),
            cut(&said(&keyed), 300)
        ),
    );
    cycle_front_app(&holder_pid, "round 3")?;
    let from = lines(log, 0).len();
    let said_from = holder_lines(&printed, 0).len();
    let ((clicked, keyed), sources) = sources_during(4, pid, || {
        let clicked = control.call(
            "click",
            json!({ "sessionKey": KEY, "displayId": id, "x": first.0, "y": first.1 }),
        );
        sleep_ms(600);
        let keyed = control.call(
            "key",
            json!({ "sessionKey": KEY, "displayId": id, "keys": "r" }),
        );
        (clicked, keyed)
    })?;
    sleep_ms(500);
    report(
        "a click and a key after the front app changed, the agent's window key before",
        log,
        from,
        &sources,
        (pid, window_id),
        ("agent", &window_id.to_string()),
        &holder_lines(&printed, said_from),
    );
    let seen = lines(log, from);
    let clicked_in = seen.iter().position(|l| l == "pair agent active first");
    let typed_in = seen
        .iter()
        .position(|l| l == "pair agent keydown first \"r\"");
    check(
        "the click focuses the agent's other field and the key reaches it, its window key before",
        clicked.is_ok()
            && keyed.is_ok()
            && matches!((clicked_in, typed_in), (Some(c), Some(k)) if c < k),
        format!(
            "{} | {} | log {seen:?}",
            cut(&said(&clicked), 300),
            cut(&said(&keyed), 300)
        ),
    );
    Ok(())
}

/// One round's checks: no source names the harness, the stand-in stayed
/// active with its window key, and the harness never called itself
/// active.
fn report(
    what: &str,
    log: &std::path::Path,
    from: usize,
    sources: &[String],
    (pid, window_id): (&str, u64),
    (after, after_window): (&str, &str),
    holder: &[String],
) {
    let seen = timed(log, from);
    println!("     {what}:");
    for l in sources {
        println!("       front {l}");
    }
    for l in &seen {
        println!("       harness {l}");
    }
    for l in holder {
        println!("       stand-in {l}");
    }
    let named = naming(sources, pid);
    check(
        &format!("{what}: no source names the harness as the front app"),
        named.is_empty(),
        format!("{named:?}"),
    );
    let lost: Vec<&String> = holder
        .iter()
        .filter(|l| l.ends_with(" inactive") || l.ends_with(" resign"))
        .collect();
    check(
        &format!("{what}: your app stays active with its window key"),
        lost.is_empty(),
        format!("{lost:?}"),
    );
    let seen = lines(log, from);
    check(
        &format!("{what}: the harness never becomes the active app"),
        !has(&seen, "app active"),
        format!("{seen:?}"),
    );
    let keys = key_windows(sources);
    check(
        &format!("{what}: the agent's window becomes the harness's key window"),
        keys.contains(&window_id.to_string()),
        format!("focused windows {keys:?}, the agent's {window_id}"),
    );
    let key_window = focused(pid).unwrap_or_default();
    check(
        &format!("{what}: the harness's {after} window is its key window after"),
        key_window == after_window,
        format!("focused window {key_window}, the {after}'s {after_window}"),
    );
}

/// An answer, or the error's text.
fn said(reply: &Result<Value>) -> String {
    match reply {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// The middle of a labelled element in a window state's tree.
fn point_of(tree: &str, label: &str) -> Result<(i64, i64)> {
    let line = tree
        .lines()
        .find(|l| l.contains(&format!("\"{label}\"")))
        .with_context(|| format!("no {label} in the tree:\n{}", cut(tree, 2000)))?;
    let at = line
        .split(" at (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .with_context(|| format!("no position in {line}"))?;
    let (x, y) = at.split_once(',').context("x,y")?;
    Ok((x.trim().parse()?, y.trim().parse()?))
}
