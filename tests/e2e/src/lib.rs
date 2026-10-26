//! Helpers for the test scripts in `src/bin`: the debug app, the control
//! socket, the MCP server, real pointer input and screen captures.
//!
//! Some scripts drive the real cursor, so they wait for the user to leave
//! the keyboard and pointer alone, and put everything back: the cursor, the
//! front app, the AeroSpace workspace and the installed Agent Screen.

pub mod control;
pub mod mcp;

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

pub use control::Control;
pub use mcp::{McpClient, ToolResult, Window};
pub use serde_json::{Value, json};

/// The repository root.
pub fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

/// Where tools, captures and copies of the app go: `target/e2e`.
pub fn out() -> PathBuf {
    root().join("target/e2e")
}

/// `AGENTSCREEN_INSTANCE=e2e` talks to a test app beside the user's, on its
/// own socket.
pub fn instance() -> Option<String> {
    std::env::var("AGENTSCREEN_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
}

/// The debug app's socket. A build in the "Agent Screen Dev" bundle is the
/// `dev` instance unless AGENTSCREEN_INSTANCE names another.
pub fn socket_path() -> PathBuf {
    socket_of(Some(&instance().unwrap_or_else(|| "dev".into())))
}

/// The socket of an instance; `None` is the installed app's, and that of
/// any bundle not named "Agent Screen Dev", such as the ungranted copy.
pub fn socket_of(instance: Option<&str>) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let name = match instance {
        Some(i) => format!("control-{i}.sock"),
        None => "control.sock".into(),
    };
    Path::new(&home)
        .join("Library/Application Support/Agent Screen")
        .join(name)
}

pub fn debug_app() -> PathBuf {
    root().join("target/Agent Screen Dev.app")
}

pub const INSTALLED_APP: &str = "/Applications/Agent Screen.app";

/// The pid of this checkout's debug app. Other checkouts' debug apps and
/// `AgentScreen mcp` processes share its name, so the whole command line
/// must match.
pub fn debug_pid() -> Option<u32> {
    sh_lenient("/usr/bin/pgrep", &["-fx", &binary_of(&debug_app())])
        .lines()
        .next()
        .and_then(|p| p.parse().ok())
}

pub fn env_is(name: &str, value: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == value)
}

/// A count from the environment: unset or empty is the fallback, and
/// anything that isn't a number runs nothing.
pub fn env_count(name: &str, fallback: f64) -> f64 {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v.trim().parse().unwrap_or(f64::NAN),
        _ => fallback,
    }
}

/// Milliseconds since 1970, to make names unique.
pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// MARK: Commands

/// A command for the debug app or a test tool, without the shell's `DYLD_*`
/// variables. With `DYLD_LIBRARY_PATH=/opt/homebrew/lib`, as a shell profile
/// may set it, dyld loads Homebrew's libpng and libtiff in place of ImageIO's
/// own `libPng.dylib` and `libTIFF.dylib` (the file system ignores case).
/// ImageIO's calls into them then jump to dyld's missing-symbol address
/// 0xbad4007, and the first PNG encode, such as a screenshot, ends the
/// process with SIGBUS. The installed app, started by Launch Services, never
/// sees these variables.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("DYLD_") {
            command.env_remove(name);
        }
    }
    command
}

/// Run a command and return its stdout, trimmed; fail when it fails.
pub fn sh(command: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Result<String> {
    let command = command.as_ref();
    let run = self::command(command)
        .args(args)
        .output()
        .with_context(|| format!("{} can't start", command.display()))?;
    let stdout = String::from_utf8_lossy(&run.stdout).trim().to_string();
    if !run.status.success() {
        let stderr = String::from_utf8_lossy(&run.stderr);
        let said = if stderr.trim().is_empty() {
            stdout.as_str()
        } else {
            stderr.trim()
        };
        bail!("{} {}: {said}", command.display(), args.join(" "));
    }
    Ok(stdout)
}

/// stdout of a command that may fail; empty when it can't start.
pub fn sh_lenient(command: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> String {
    self::command(command)
        .args(args)
        .output()
        .map(|run| String::from_utf8_lossy(&run.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Run an AppleScript; its output, empty when it fails.
pub fn osa(script: &str) -> String {
    sh_lenient("/usr/bin/osascript", &["-e", script])
}

/// Run an AppleScript; fail when it fails.
pub fn try_osa(script: &str) -> Result<String> {
    sh("/usr/bin/osascript", &["-e", script])
}

/// Quit an app by name; false when it fails, as when the app isn't running.
pub fn quit_app(name: &str) -> bool {
    try_osa(&format!("tell application \"{name}\" to quit")).is_ok()
}

/// Close an app's documents without saving them, then quit it; false when
/// it fails, as when the app isn't running.
pub fn quit_discarding(name: &str) -> bool {
    try_osa(&format!(
        "tell application \"{name}\" to close every document saving no\ntell application \"{name}\" to quit"
    ))
    .is_ok()
}

/// The text of TextEdit's front document, whole: windowState cuts long
/// values short.
pub fn document_text() -> String {
    osa("tell application \"TextEdit\" to get text of front document")
}

/// A string as JSON writes it.
pub fn quoted(s: &str) -> String {
    Value::from(s).to_string()
}

/// The first `n` characters of `s`.
pub fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Test displays made in the last hour, across every script and agent on
/// this Mac, when E2E_DISPLAYS_PER_HOUR sets a limit; none by default.
/// Each new display makes WindowServer and ColorSync reconfigure, and about
/// 60 in 11 minutes froze the Mac on 2026-10-20.
fn displays_per_hour() -> usize {
    std::env::var("E2E_DISPLAYS_PER_HOUR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX)
}

/// Count one display against the hourly budget, or fail once it is spent.
/// A reused spare counts too: a script can't tell beforehand.
pub fn spend_display() -> Result<()> {
    let ledger = Path::new("/tmp/agent-screen-test-displays");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut times: Vec<u64> = std::fs::read_to_string(ledger)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .filter(|&t: &u64| now.saturating_sub(t) < 3600)
        .collect();
    if times.len() >= displays_per_hour() {
        let wait = 3600 - now.saturating_sub(times[0]);
        bail!(
            "test scripts made {} displays in the last hour, the most this Mac takes safely; try again in {} min",
            times.len(),
            wait / 60 + 1
        );
    }
    times.push(now);
    let text: String = times.iter().map(|t| format!("{t}\n")).collect();
    std::fs::write(ledger, text)?;
    Ok(())
}

/// A Swift helper from `tools/`, built into target/e2e when its source is
/// newer than the binary.
pub fn tool(name: &str) -> Result<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tools/{name}.swift"));
    let binary = out().join(name);
    std::fs::create_dir_all(out())?;
    if modified(&binary) < modified(&source) {
        sh(
            "/usr/bin/swiftc",
            &[
                "-O",
                source.to_str().unwrap(),
                "-o",
                binary.to_str().unwrap(),
            ],
        )?;
    }
    Ok(binary)
}

/// tools/field.swift as an app, so launch_app takes it.
pub fn field_app() -> Result<PathBuf> {
    swift_app("field", "Field")
}

/// tools/login.swift as an app: a native login window.
pub fn login_app() -> Result<PathBuf> {
    swift_app("login", "Login")
}

/// A tool as an app named `name`, so launch_app takes it.
fn swift_app(tool_name: &str, name: &str) -> Result<PathBuf> {
    let app = out().join(format!("{name}.app"));
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    std::fs::copy(tool(tool_name)?, macos.join(name))?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.agentscreen.e2e.{tool_name}</string>
<key>CFBundleExecutable</key><string>{name}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#
        ),
    )?;
    sh(
        "/usr/bin/codesign",
        &["--force", "--sign", "-", app.to_str().unwrap()],
    )?;
    Ok(app)
}

pub fn probe(args: &[&str]) -> Result<String> {
    sh(tool("probe")?, args)
}

// MARK: The sign

/// Scripts that move the user's pointer, take their keyboard or input
/// source, mute their sound, or put windows, alerts, cards or focus on their
/// screen. Their sign asks the user to keep their hands off; the others'
/// only says a test runs.
const HANDS_OFF: &[&str] = &[
    "alerts",
    "browsers",
    "chrome",
    "displaychange",
    "flash",
    "helpcard",
    "ime",
    "keys",
    "launchhere",
    "load",
    "oneclick",
    "openpanel",
    "peekanim",
    "peekhover",
    "peekrapid",
    "requestwindow",
    "restart",
    "robust",
    "run",
    "settings",
    "shared",
    "sound",
    "statusicon",
    "timing",
];

/// The sign `run` shows on the main screen (`tools/overlay.swift`): a count
/// from 3 in the middle with the script's name, then a badge in the
/// bottom-right corner with the current step and each check, then the
/// result and "可以操作了" when the script ends, whatever the result: a
/// skip, an error and a panic too. One per process, however many
/// threads the script runs. E2E_NO_SIGN=1 turns it off.
struct Sign {
    child: std::process::Child,
    acks: std::io::BufReader<std::process::ChildStdout>,
    hidden: usize,
}

static SIGN: std::sync::Mutex<Option<Sign>> = std::sync::Mutex::new(None);

fn sign() -> std::sync::MutexGuard<'static, Option<Sign>> {
    SIGN.lock().unwrap_or_else(|e| e.into_inner())
}

impl Sign {
    fn say(&mut self, line: &str) {
        use std::io::Write;
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = writeln!(stdin, "{line}");
        }
    }
}

/// The running script's name, its binary's.
fn script_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "e2e".into())
}

/// Count down from 3 and return once the count is over, the sign shrunk to
/// its badge. A sign that can't start leaves the script to run without.
fn start_sign(name: &str) {
    if env_is("E2E_NO_SIGN", "1") {
        return;
    }
    let started = tool("overlay").and_then(|overlay| {
        Ok(command(overlay)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?)
    });
    let mut child = match started {
        Ok(child) => child,
        Err(e) => {
            println!("no sign: {e:#}");
            return;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut sign = Sign {
        child,
        acks: std::io::BufReader::new(stdout),
        hidden: 0,
    };
    {
        use std::io::BufRead;
        let mut ready = String::new();
        let _ = sign.acks.read_line(&mut ready);
    }
    if HANDS_OFF.contains(&name) {
        sign.say(&format!(
            "count 3 {name}: don't touch the mouse or keyboard / 请勿操作鼠标和键盘"
        ));
    } else {
        sign.say(&format!(
            "calm 3 {name}: Agent Screen e2e running / 测试运行中"
        ));
    }
    sleep_ms(3000);
    sign.say(&format!("badge {name}"));
    *self::sign() = Some(sign);
}

/// What the script does now, on the sign's badge.
pub fn sign_step(text: &str) {
    if let Some(sign) = sign().as_mut() {
        sign.say(&format!("step {}", text.replace('\n', " ")));
    }
}

/// How a script ended, as the sign shows it.
enum Outcome<'a> {
    Passed,
    Failed,
    Skipped(&'a str),
}

/// The sign shows the result and goes a few seconds later, by itself.
fn finish_sign(outcome: Outcome) {
    let Some(mut sign) = sign().take() else {
        return;
    };
    let name = script_name();
    let line = match outcome {
        Outcome::Passed => {
            let n = PASSES.load(Ordering::SeqCst);
            format!("pass 全部通过 / all passed ({n})\t{name}")
        }
        Outcome::Failed => {
            let n = failed().max(1);
            let first = FIRST_FAILURE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default();
            format!("fail {n} 项失败 / {n} failed\t{name}: {first}")
        }
        Outcome::Skipped(reason) => format!("skip 跳过 / skipped\t{name}: {reason}"),
    };
    sign.say(&format!("done {}", line.replace('\n', " ")));
    drop(sign.child.stdin.take());
}

/// The sign stays off the screen while this lives, so a capture of the
/// main screen doesn't show it.
pub struct SignHidden(());

pub fn hide_sign() -> SignHidden {
    if let Some(sign) = sign().as_mut() {
        sign.hidden += 1;
        if sign.hidden == 1 {
            use std::io::BufRead;
            sign.say("hide");
            // The overlay answers once WindowServer stops listing it.
            let mut ack = String::new();
            let _ = sign.acks.read_line(&mut ack);
        }
    }
    SignHidden(())
}

impl Drop for SignHidden {
    fn drop(&mut self) {
        if let Some(sign) = sign().as_mut() {
            sign.hidden = sign.hidden.saturating_sub(1);
            if sign.hidden == 0 {
                sign.say("show");
            }
        }
    }
}

/// Real pointer and key input; `hover` with a region captures it, without
/// the sign.
pub fn hid(args: &[&str]) -> Result<String> {
    let _hidden = (args.first() == Some(&"hover") && args.len() > 3).then(hide_sign);
    sh(tool("hid")?, args)
}

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// A region of the screen: x, y, width and height in global points.
pub type Rect = [f64; 4];

/// An on-screen window of a process, as `probe windows` lists it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Frame {
    pub id: u64,
    pub layer: i64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn windows_of(pid: u32) -> Vec<Frame> {
    probe(&["windows", &pid.to_string()])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub fn pid_of(pattern: &str) -> Option<u32> {
    sh_lenient("/usr/bin/pgrep", &["-f", pattern])
        .lines()
        .next()
        .and_then(|p| p.parse().ok())
}

pub fn running(name: &str) -> bool {
    !sh_lenient("/usr/bin/pgrep", &["-x", name]).is_empty()
}

/// `name` is running with windows: a headless browser that a script on this
/// Mac drives is no one's to look at.
pub fn running_headed(name: &str) -> bool {
    sh_lenient("/usr/bin/pgrep", &["-x", name])
        .lines()
        .any(|pid| !sh_lenient("/bin/ps", &["-o", "args=", "-p", pid]).contains("--headless"))
}

/// Quit the browser running on the throwaway profile `dir`, and no other
/// copy of it.
pub fn quit_profile(dir: &Path) {
    let _ = Command::new("/usr/bin/pkill")
        .args(["-TERM", "-f"])
        .arg(dir)
        .status();
    for _ in 0..50 {
        if sh_lenient("/usr/bin/pgrep", &["-f", &dir.to_string_lossy()]).is_empty() {
            return;
        }
        sleep_ms(100);
    }
}

/// The screen is locked: it shows no windows, can't be captured and takes
/// no input.
pub fn locked() -> bool {
    probe(&["locked"]).is_ok_and(|s| s == "1")
}

// MARK: Results

static FAILURES: AtomicIsize = AtomicIsize::new(0);
static PASSES: AtomicIsize = AtomicIsize::new(0);
static FIRST_FAILURE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn note_failure(label: &str) {
    FIRST_FAILURE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(|| label.to_string());
}

pub fn check(label: &str, ok: bool, detail: impl AsRef<str>) -> bool {
    let detail = detail.as_ref();
    let tag = if ok { "ok  " } else { "FAIL" };
    if detail.is_empty() {
        println!("{tag} {label}");
    } else {
        println!("{tag} {label}: {detail}");
    }
    sign_step(&format!("{} {label}", if ok { "✓" } else { "✗" }));
    if ok {
        PASSES.fetch_add(1, Ordering::SeqCst);
    } else {
        note_failure(label);
        fail();
    }
    ok
}

/// Count a failure without printing a check line.
pub fn fail() {
    FAILURES.fetch_add(1, Ordering::SeqCst);
}

pub fn failed() -> isize {
    FAILURES.load(Ordering::SeqCst)
}

/// Take back failures from a case whose run doesn't count.
pub fn voided(count: isize) {
    FAILURES.fetch_sub(count, Ordering::SeqCst);
}

/// Stop here without failing.
pub fn skip(reason: &str) -> ! {
    println!("skip: {reason}");
    finish_sign(Outcome::Skipped(reason));
    std::process::exit(0);
}

/// Run a script's body under the sign, then report: "all passed" or the
/// failure count. An error from the body counts as a failure. The sign
/// turns green whatever the result, a panic too.
pub fn run(body: impl FnOnce() -> Result<()>) -> ExitCode {
    start_sign(&script_name());
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            check("the script ran to the end", false, format!("{e:#}"));
        }
        Err(panic) => {
            fail();
            note_failure("the script panicked");
            finish_sign(Outcome::Failed);
            std::panic::resume_unwind(panic);
        }
    }
    let n = failed();
    if n > 0 {
        println!("{n} failed");
        finish_sign(Outcome::Failed);
        ExitCode::FAILURE
    } else {
        println!("all passed");
        finish_sign(Outcome::Passed);
        ExitCode::SUCCESS
    }
}

/// A capture of a screen region in global points, written under target/e2e,
/// with the sign off the screen.
pub fn capture(name: &str, [x, y, w, h]: Rect) -> Result<PathBuf> {
    let _hidden = hide_sign();
    let file = out().join(format!("{name}.png"));
    let region = [x, y, w, h].map(|v| v.round().to_string()).join(",");
    sh(
        "/usr/sbin/screencapture",
        &["-x", "-R", &region, file.to_str().unwrap()],
    )?;
    Ok(file)
}

pub fn digest(file: &Path) -> String {
    let bytes = std::fs::read(file).unwrap_or_default();
    Md5::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The color at a point of a capture taken at 2x, given in capture points.
pub fn pixel(file: &Path, x: f64, y: f64) -> Vec<f64> {
    probe(&[
        "pixel",
        file.to_str().unwrap(),
        &(x * 2.0).to_string(),
        &(y * 2.0).to_string(),
    ])
    .unwrap_or_default()
    .split(' ')
    .filter_map(|v| v.parse().ok())
    .collect()
}

/// Where a display point is on screen, given the Viewer's image rect.
pub fn on_screen(image: Rect, (width, height): (f64, f64), x: f64, y: f64) -> (f64, f64) {
    (
        image[0] + x * image[2] / width,
        image[1] + y * image[3] / height,
    )
}

// MARK: Waiting for the user

fn wait_until(limit: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if ready() {
            return true;
        }
        sleep_ms(5000);
    }
    false
}

fn idle_seconds() -> u64 {
    probe(&["idle"])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Wait until the user has been idle for 45 s, has no Agent Display open
/// (restarting the app ends every display) and the screen is unlocked.
/// E2E_NOW=1 skips the wait.
pub fn wait_for_quiet() -> bool {
    if env_is("E2E_NOW", "1") {
        return true;
    }
    wait_until(Duration::from_secs(3600), || {
        idle_seconds() >= 45 && probe(&["displays"]).is_ok_and(|s| s == "0") && !locked()
    })
}

/// Wait until the user has been idle for 45 s with the screen unlocked.
/// Tests beside the user's app that open windows need it: macOS shows a new
/// window on the user's screen for a moment before Agent Screen moves it.
pub fn wait_for_idle() -> bool {
    if env_is("E2E_NOW", "1") {
        return !locked();
    }
    wait_until(Duration::from_secs(3600), || {
        idle_seconds() >= 45 && !locked()
    })
}

/// Wait until the screen is unlocked.
pub fn wait_for_unlocked() -> bool {
    wait_until(Duration::from_secs(3600), || !locked())
}

/// What the tests must leave as they found it.
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub front: String,
    pub workspace: String,
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        front: osa(
            "tell application \"System Events\" to get name of first process whose frontmost is true",
        ),
        workspace: sh_lenient("aerospace", &["list-workspaces", "--focused"]),
    }
}

pub fn restore(before: &Snapshot) {
    let now = snapshot();
    if !before.front.is_empty() && now.front != before.front {
        osa(&format!(
            "tell application \"{}\" to activate",
            before.front
        ));
    }
    if !before.workspace.is_empty() && now.workspace != before.workspace {
        sh_lenient("aerospace", &["workspace", &before.workspace]);
    }
    let after = snapshot();
    check(
        "front app and workspace unchanged",
        after == *before,
        format!(
            "{}/{} → {}/{}",
            before.front, before.workspace, after.front, after.workspace
        ),
    );
}

// MARK: The app

fn binary_of(app: &Path) -> String {
    format!("{}/Contents/MacOS/AgentScreen", app.display())
}

/// A normal quit hands each display's windows back; a kill leaves them to
/// macOS, which moves them onto the user's screen.
pub fn stop_app(app: &Path) {
    let binary = binary_of(app);
    if pid_of(&binary).is_none() {
        return;
    }
    osa(&format!("tell application \"{}\" to quit", app.display()));
    for _ in 0..40 {
        if pid_of(&binary).is_none() {
            break;
        }
        sleep_ms(100);
    }
}

pub fn listening() -> bool {
    listening_at(&socket_path())
}

fn listening_at(socket: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

fn wait_listening() -> bool {
    wait_listening_at(&socket_path())
}

fn wait_listening_at(socket: &Path) -> bool {
    for _ in 0..60 {
        if listening_at(socket) {
            return true;
        }
        sleep_ms(250);
    }
    false
}

fn bundle_debug() -> Result<()> {
    sh(root().join("apps/macos/scripts/bundle.sh"), &["debug"]).map(|_| ())
}

/// Start the debug app's binary. Started from here, it borrows this
/// terminal's privacy grants. E2E_LOG=FILE keeps its log.
fn spawn_debug(env: &[(&str, &str)]) -> Result<()> {
    let (stdout, stderr): (Stdio, Stdio) = match std::env::var("E2E_LOG") {
        Ok(file) => {
            let log = std::fs::File::create(file)?;
            (log.try_clone()?.into(), log.into())
        }
        Err(_) => (Stdio::null(), Stdio::null()),
    };
    // Its own process group, so Ctrl-C on a script leaves it running.
    command(binary_of(&debug_app()))
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0)
        .spawn()?;
    Ok(())
}

/// Build the debug bundle and run it in place of the installed app.
pub fn start_debug_app(env: &[(&str, &str)]) -> Result<()> {
    bundle_debug()?;
    stop_app(Path::new(INSTALLED_APP));
    stop_app(&debug_app());
    spawn_debug(env)?;
    wait_listening();
    Ok(())
}

pub fn stop_debug_app() {
    stop_app(&debug_app());
}

/// The debug app as a second instance beside the user's Agent Screen, which
/// keeps running with its displays. Needs AGENTSCREEN_INSTANCE. Agent-side
/// tests run this way at any time: their input goes to app processes, not
/// to the user's pointer and keyboard.
pub fn start_test_app(env: &[(&str, &str)]) -> Result<()> {
    if instance().is_none() {
        bail!("set AGENTSCREEN_INSTANCE to run beside the user's app");
    }
    bundle_debug()?;
    if listening() {
        return Ok(());
    }
    spawn_debug(env)?;
    if !wait_listening() {
        bail!("the test app didn't start");
    }
    Ok(())
}

/// With AGENTSCREEN_INSTANCE, run `body` in the test app beside the user's,
/// started for it and quit after it; without, in the app that runs.
pub fn in_test_app(body: impl FnOnce() -> Result<()>) -> Result<()> {
    if instance().is_some() {
        start_test_app(&[])?;
    }
    let result = body();
    if instance().is_some() {
        stop_test_app();
    }
    result
}

/// Quit the test instance through its socket; AppleScript would find the
/// user's app by bundle id.
pub fn stop_test_app() {
    if !listening() {
        return;
    }
    if let Ok(control) = Control::connect() {
        // The app can quit before it answers; the loop below waits either way.
        let _ = control.send("debugQuit", json!({}));
    }
    for _ in 0..50 {
        if !listening() {
            break;
        }
        sleep_ms(100);
    }
}

/// Start a copy under another bundle id, so it has no privacy grants.
pub fn start_ungranted_app() -> Result<PathBuf> {
    bundle_debug()?;
    let copy = out().join("Ungranted.app");
    let copy_str = copy.to_str().unwrap();
    sh("/bin/rm", &["-rf", copy_str])?;
    sh("/usr/bin/ditto", &[debug_app().to_str().unwrap(), copy_str])?;
    sh(
        "/usr/libexec/PlistBuddy",
        &[
            "-c",
            "Set :CFBundleIdentifier app.agentscreen.e2e-ungranted",
            &format!("{copy_str}/Contents/Info.plist"),
        ],
    )?;
    let identity = sh(
        "/bin/sh",
        &[
            "-c",
            "security find-identity -v -p codesigning | awk -F'\"' '/Apple Development/ { print $2; exit }'",
        ],
    )?;
    let identity = if identity.is_empty() { "-" } else { &identity };
    sh(
        "/usr/bin/codesign",
        &["--force", "--sign", identity, copy_str],
    )?;
    // Each run starts with no grants and no entries in the lists.
    sh_lenient(
        "/usr/bin/tccutil",
        &["reset", "All", "app.agentscreen.e2e-ungranted"],
    );
    stop_app(Path::new(INSTALLED_APP));
    stop_app(&debug_app());
    // Opened by launchd, it answers for itself instead of borrowing ours.
    sh("/usr/bin/open", &[copy_str])?;
    wait_listening_at(&socket_of(None));
    Ok(copy)
}

pub fn reopen_installed() {
    if Path::new(INSTALLED_APP).exists() {
        sh_lenient("/usr/bin/open", &[INSTALLED_APP]);
    }
}

// MARK: The Viewer

/// Open the stage: a display the size of a laptop screen, where the Viewer
/// opens out of the user's sight.
pub fn open_stage(control: &Control) -> Result<u64> {
    let stage = control.call(
        "createDisplay",
        json!({ "sessionKey": "stage", "title": "Stage", "width": 1728, "height": 1117 }),
    )?;
    control.call(
        "debugShow",
        json!({ "surface": "stage", "displayId": stage["displayId"] }),
    )?;
    stage["displayId"].as_u64().context("no stage display id")
}

/// Start recording the stage display, the stand-in for the user's screen,
/// through Agent Screen's own recording. `stage` is the connection that opened
/// it. Unlike `screencapture -R`, it lays no dimming veil over the other
/// displays, which the Viewer would show.
pub fn record_stage(stage: &Control, id: u64) -> Result<PathBuf> {
    let started = stage.call(
        "recordStart",
        json!({ "sessionKey": "stage", "displayId": id }),
    )?;
    Ok(PathBuf::from(
        started["video"].as_str().context("no video path")?,
    ))
}

/// Stop the stage recording and write the part of it showing `region`, in
/// global points, to `clip` at 30 fps, `width` pixels wide.
pub fn cut_stage(
    stage: &Control,
    id: u64,
    video: &Path,
    region: Rect,
    clip: &Path,
    width: u32,
) -> Result<()> {
    stage.call(
        "recordStop",
        json!({ "sessionKey": "stage", "displayId": id }),
    )?;
    let bounds: Vec<f64> = probe(&["bounds", &id.to_string()])?
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    let [x, y, w, _] = bounds[..] else {
        bail!("no bounds for display {id}");
    };
    let pixels: f64 = sh(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width",
            "-of",
            "csv=p=0",
            &video.to_string_lossy(),
        ],
    )?
    .trim()
    .parse()?;
    let scale = pixels / w;
    let [rx, ry, rw, rh] = region;
    let crop = format!(
        "crop={}:{}:{}:{},fps=30,scale={width}:-2:flags=lanczos",
        (rw * scale).round(),
        (rh * scale).round(),
        ((rx - x) * scale).round(),
        ((ry - y) * scale).round()
    );
    let output = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(video)
        .args([
            "-vf", &crop, "-an", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18",
        ])
        .arg(clip)
        .output()
        .context("ffmpeg can't start")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let _ = std::fs::remove_file(video);
    let _ = std::fs::remove_file(video.with_extension("timeline.jsonl"));
    Ok(())
}

/// The US key code of a character the scripts type.
fn key_code(ch: char) -> Option<u8> {
    Some(match ch {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        'o' => 31,
        'u' => 32,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        'k' => 40,
        'n' => 45,
        'm' => 46,
        ' ' => 49,
        '.' => 47,
        ',' => 43,
        _ => return None,
    })
}

/// Type in the Viewer as the user, through its own key handler, with
/// `gap_ms` after each key.
pub fn type_in_viewer(control: &Control, text: &str, gap_ms: u64) -> Result<()> {
    for ch in text.chars() {
        let lower = ch.to_ascii_lowercase();
        control.call(
            "debugViewerInput",
            json!({ "kind": "key", "code": key_code(lower), "shift": u8::from(ch != lower) }),
        )?;
        sleep_ms(gap_ms);
    }
    Ok(())
}

// MARK: Flash

/// What tools/flash.swift saw of an app's windows on the user's main display.
#[derive(Debug, Deserialize)]
pub struct Flash {
    pub windows: i64,
    pub ms: i64,
}

/// Watch the main display for `app`'s windows while `work` runs.
pub fn watch_flash<T>(app: &str, work: impl FnOnce() -> Result<T>) -> Result<(Flash, T)> {
    let mut watcher = command(tool("flash")?)
        .arg(app)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("starting flash")?;
    sleep_ms(100);
    let done = work();
    drop(watcher.stdin.take());
    let out = watcher.wait_with_output()?;
    let done = done?;
    let seen = serde_json::from_slice(&out.stdout).context("flash's output")?;
    Ok((seen, done))
}

fn support() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/Agent Screen")
}

/// For recordings: the user's chosen agent display wallpaper, if any,
/// moved aside, and `picture` put in its place, as the app's wallpaper
/// setting keeps it. Give the result to `put_back_wallpaper`.
pub fn wear_wallpaper(picture: &Path) -> Option<(std::path::PathBuf, Vec<u8>)> {
    let saved = std::fs::read_dir(support()).ok().and_then(|entries| {
        entries.flatten().map(|e| e.path()).find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("wallpaper-"))
        })
    });
    let saved = saved.and_then(|path| {
        let bytes = std::fs::read(&path).ok()?;
        std::fs::remove_file(&path).ok()?;
        Some((path, bytes))
    });
    let _ = std::fs::copy(picture, support().join("wallpaper-recording.png"));
    saved
}

pub fn put_back_wallpaper(saved: Option<(std::path::PathBuf, Vec<u8>)>) {
    let _ = std::fs::remove_file(support().join("wallpaper-recording.png"));
    if let Some((path, bytes)) = saved {
        let _ = std::fs::write(path, bytes);
    }
}

#[cfg(test)]
mod tests {
    /// Every hands-off name is a script, so a rename can't drop its warning.
    #[test]
    fn hands_off_names_scripts() {
        let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin");
        for name in super::HANDS_OFF {
            assert!(bin.join(format!("{name}.rs")).exists(), "no script {name}");
        }
    }
}
