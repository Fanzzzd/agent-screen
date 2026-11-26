//! Helpers for the test scripts in `src/bin`: the debug app, the control
//! socket, the MCP server, real pointer input and screen captures.
//!
//! Some scripts drive the real cursor, so they wait for the user to leave
//! the keyboard and pointer alone, and put everything back: the cursor, the
//! front app, the AeroSpace workspace and the installed Agent Screen.

pub mod budget;
pub mod control;
pub mod manifest;
pub mod mcp;
pub mod plan;
pub mod select;

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

/// The e2e instance's Autonomy Mode file. Only the e2e instance's: no
/// script may write the user's (ADR 0025).
pub fn autonomy_file() -> Result<PathBuf> {
    if instance().as_deref() != Some("e2e") {
        bail!("the Autonomy Mode file is written only for AGENTSCREEN_INSTANCE=e2e");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Ok(Path::new(&home).join("Library/Application Support/Agent Screen/autonomy-e2e.json"))
}

/// Give the e2e instance `setting` as its stored Autonomy Mode before it
/// starts, and return what the file held, for `put_back_autonomy`.
pub fn set_autonomy_file(setting: &Value) -> Result<Option<String>> {
    let file = autonomy_file()?;
    let before = std::fs::read_to_string(&file).ok();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, setting.to_string())?;
    Ok(before)
}

/// Put back what `set_autonomy_file` found, or remove the file it made.
pub fn put_back_autonomy(before: Option<String>) {
    let Ok(file) = autonomy_file() else {
        return;
    };
    match before {
        Some(text) => {
            let _ = std::fs::write(&file, text);
        }
        None => {
            let _ = std::fs::remove_file(&file);
        }
    }
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

/// Build every Swift helper whose source is newer than its binary, before
/// the countdown, while the sign says the user can keep working. A helper
/// that fails to build fails later, where the script uses it.
fn prebuild_tools() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let stale: Vec<String> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "swift"))
        .filter(|p| modified(&out().join(p.file_stem().unwrap())) < modified(p))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    if stale.is_empty() {
        return;
    }
    sign_step("building test helpers, you can keep working / 正在编译测试工具，可以继续操作");
    for name in stale {
        let _ = tool(&name);
    }
}

/// tools/field.swift as an app, so launch_app takes it.
pub fn field_app() -> Result<PathBuf> {
    swift_app("field", "Field")
}

/// tools/`tool_name`.swift as an app named `name`, with a bundle id of its
/// own ending in `id`: another app to Agent Screen, for a rule that goes
/// by app.
pub fn tool_app_as(tool_name: &str, name: &str, id: &str) -> Result<PathBuf> {
    swift_app_as(tool_name, name, id)
}

/// tools/login.swift as an app: a native login window.
pub fn login_app() -> Result<PathBuf> {
    swift_app("login", "Login")
}

/// tools/latewindow.swift as an app: a loading window, then a main window
/// on the main screen seconds later.
pub fn late_window_app() -> Result<PathBuf> {
    swift_app("latewindow", "LateWindow")
}

/// tools/hidclick.swift as an app: a button that takes only a click the
/// HID system reads as held, as a real click is.
pub fn hidclick_app() -> Result<PathBuf> {
    swift_app("hidclick", "HIDClick")
}

/// tools/menuchoice.swift as an app: a closed pop-up button and a combo
/// box whose window title says what they hold and how often a menu opened.
pub fn menu_choice_app() -> Result<PathBuf> {
    swift_app("menuchoice", "MenuChoice")
}

/// tools/dropdown.swift as an app: a results list in a window of its own
/// over the main window's rows, closed by any press in another app, as
/// a Qt pop-up is.
pub fn dropdown_app() -> Result<PathBuf> {
    swift_app("dropdown", "Dropdown")
}

/// tools/qtlist.py as an app: a real Qt search box with a list of results,
/// run by PySide6 from a venv in target/e2e/qtenv. The first call
/// makes the venv with uv and downloads PySide6 (about 350 MB). None when
/// uv is missing or the download fails, with the reason printed.
pub fn qt_list_app() -> Result<Option<PathBuf>> {
    let env = out().join("qtenv");
    let python = env.join("bin/python");
    let has_qt = |python: &Path| {
        Command::new(python)
            .args(["-I", "-c", "import PySide6.QtWidgets"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if !has_qt(&python) {
        let Ok(uv) = sh("/usr/bin/which", &["uv"]) else {
            println!("no uv to make the Qt venv; install it with `brew install uv`");
            return Ok(None);
        };
        let uv = uv.trim().to_string();
        println!("making target/e2e/qtenv with PySide6, once");
        let made = sh(
            &uv,
            &[
                "venv",
                "-q",
                // tools/qtlaunch.c loads a framework build: uv's own Pythons
                // are none, Homebrew's and python.org's are. Any other is
                // caught below, and the script skips.
                "--python-preference",
                "only-system",
                "-p",
                "3.13",
                env.to_str().unwrap(),
            ],
        )
        .and_then(|_| {
            sh(
                &uv,
                &[
                    "pip",
                    "install",
                    "-q",
                    "-p",
                    python.to_str().unwrap(),
                    "PySide6-Essentials",
                ],
            )
        });
        if let Err(error) = made {
            println!("could not install PySide6: {error}");
            return Ok(None);
        }
        if !has_qt(&python) {
            println!("PySide6 installed but doesn't import");
            return Ok(None);
        }
    }
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools");
    let script = tools.join("qtlist.py");
    let app = out().join("QtList.app");
    let _ = std::fs::remove_dir_all(&app);
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    // The bundle's executable is tools/qtlaunch.c, which runs the script
    // with the framework's Python in its own process, so the pid launch_app
    // starts is the app: Homebrew's python starts Python.app as a second
    // process, a shell launcher execs a path launch_app doesn't know, and
    // Python itself stops at the switch launch_app passes every app.
    let paths = sh(
        &python,
        &[
            "-I",
            "-c",
            "import sys, sysconfig; print(sys.base_prefix + '/Python'); print(sysconfig.get_paths()['purelib'])",
        ],
    )?;
    let mut paths = paths.lines();
    let (library, packages) = (
        paths.next().context("no Python library")?,
        paths.next().context("no site-packages")?,
    );
    if !Path::new(library).exists() {
        println!(
            "the venv's Python is no framework build ({library} is missing); make target/e2e/qtenv with Homebrew's or python.org's 3.13"
        );
        return Ok(None);
    }
    sh(
        "/usr/bin/clang",
        &[
            "-O1",
            &format!("-DPYLIB={}", quoted(library)),
            &format!("-DPYPATH={}", quoted(packages)),
            &format!("-DSCRIPT={}", quoted(&script.display().to_string())),
            tools.join("qtlaunch.c").to_str().unwrap(),
            "-o",
            macos.join("QtList").to_str().unwrap(),
        ],
    )?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.agentscreen.e2e.qtlist</string>
<key>CFBundleExecutable</key><string>QtList</string>
<key>CFBundleName</key><string>QtList</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#,
    )?;
    sh(
        "/usr/bin/codesign",
        &["--force", "--sign", "-", app.to_str().unwrap()],
    )?;
    Ok(Some(app))
}

/// tools/watch.swift as an app: buttons that change its window seconds
/// after a press, for wait_for_change.
pub fn watch_app() -> Result<PathBuf> {
    swift_app("watch", "Watch")
}

/// tools/filedrag.swift as two apps, FileSource and FileDrop: a file icon
/// that starts a system drag session, and a view that takes a dropped
/// file. Separate bundle ids, so launch_app starts each in its own process.
pub fn file_drag_apps() -> Result<(PathBuf, PathBuf)> {
    Ok((
        swift_app_as("filedrag", "FileSource", "filesource")?,
        swift_app_as("filedrag", "FileDrop", "filedrop")?,
    ))
}

/// A tool as an app named `name`, so launch_app takes it.
fn swift_app(tool_name: &str, name: &str) -> Result<PathBuf> {
    swift_app_as(tool_name, name, tool_name)
}

/// A tool as an app named `name` with the bundle id ending in `id`.
fn swift_app_as(tool_name: &str, name: &str, id: &str) -> Result<PathBuf> {
    let app = out().join(format!("{name}.app"));
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    std::fs::copy(tool(tool_name)?, macos.join(name))?;
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.agentscreen.e2e.{id}</string>
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

/// The sign `run` shows on every real screen (`tools/overlay.swift`): a
/// badge in the bottom-right corner while the script prepares, a count
/// from 3 in the middle with the script's name before it acts, then the
/// badge again with the current step and each check, then the
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

/// Set once `run` has started the body: only then does an action count
/// down first.
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static COUNTDOWN: std::sync::Once = std::sync::Once::new();

/// Count down from 3 on the sign, once per run, and return once the count
/// is over. A script calls it after its setup, right before its first step
/// that needs hands off. A script that doesn't call it counts down at its
/// first real input, create_display, launch_app or debugShow. Later calls return at
/// once; other threads wait for the count to end. Outside `run`, nothing.
pub fn countdown() {
    if !RUNNING.load(Ordering::SeqCst) {
        return;
    }
    COUNTDOWN.call_once(|| start_sign(&script_name()));
}

/// Count down first when a tool or control method is one that acts.
pub(crate) fn countdown_before(method: &str) {
    // debugShow puts a surface on the user's screen.
    const ACTS: &[&str] = &[
        "create_display",
        "createDisplay",
        "launch_app",
        "launchApp",
        "debugShow",
    ];
    if ACTS.contains(&method) {
        countdown();
    }
}

/// Count down from 3 and return once the count is over, the sign shrunk to
/// its badge, for a script that needs the user's hands off or asks them to
/// act. Any other script stays on Agent Displays: its badge only says it
/// runs, with no count and no watch for the user's input. A sign that can't
/// start leaves the script to run without.
fn start_sign(name: &str) {
    let acts = manifest::user_acts(name);
    if !counts_down() {
        sign_step("running, you can keep working / 运行中，可以继续操作");
        return;
    }
    if !open_sign() {
        return;
    }
    let Some(mut sign) = self::sign().take() else {
        return;
    };
    match acts {
        Some(text) => sign.say(&format!("count 3 {name}: {text}")),
        None => sign.say(&format!(
            "count 3 {name}: don't touch the mouse or keyboard / 请勿操作鼠标和键盘"
        )),
    }
    sleep_ms(3000);
    sign.say(&format!("badge {name}"));
    sign.say(if acts.is_some() {
        "step running / 运行中"
    } else {
        "step running, hands off / 运行中，请勿操作"
    });
    *self::sign() = Some(sign);
    // The user's input is what the script asked for.
    if acts.is_none() {
        watch_for_you();
    }
}

/// Start the sign's process, once: the wait for the user shows on it too.
/// False when there is none.
fn open_sign() -> bool {
    if env_is("E2E_NO_SIGN", "1") {
        return false;
    }
    if sign().is_some() {
        return true;
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
            return false;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return false;
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
    sign.say(&format!("badge {}", script_name()));
    *self::sign() = Some(sign);
    true
}

/// When the script last sent real pointer or key input itself, in ms since
/// 1970: the watch for the user's input leaves it out.
static OWN_INPUT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// When the user first used the Mac during the run, in seconds into it.
static YOU_ACTED: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

/// With E2E_ABORT_ON_INPUT=1, a run the user touched stops acting: from
/// then on every action, new display and real input of the script fails,
/// so it ends through its usual cleanup. Reads, ends and closes still go.
pub fn stop_if_you_acted(what: &str) -> Result<()> {
    const ACTS: &[&str] = &[
        "click",
        "mouseDown",
        "mouseUp",
        "move",
        "drag",
        "scroll",
        "type",
        "key",
        "holdKey",
        "elementAction",
        "launchApp",
        "placeWindow",
        "createDisplay",
        "computer",
        "act_on_element",
        "launch_app",
        "place_window",
        "create_display",
        "hid",
    ];
    if !env_is("E2E_ABORT_ON_INPUT", "1") || !ACTS.contains(&what) {
        return Ok(());
    }
    match *YOU_ACTED.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(at) => {
            anyhow::bail!("stopped: you used the Mac {at} s into the run, so {what} wasn't sent")
        }
        None => Ok(()),
    }
}

/// Watch for the user's mouse and keyboard while the script runs. A run
/// they touched may fail for that alone: the sign and the result say so.
fn watch_for_you() {
    let started = Instant::now();
    std::thread::spawn(move || {
        loop {
            sleep_ms(500);
            let own = OWN_INPUT.load(Ordering::SeqCst) as u128;
            let ours = own > 0 && now_ms().saturating_sub(own) < 1500;
            if idle_seconds() == 0 && !ours && started.elapsed() > Duration::from_secs(1) {
                let at = started.elapsed().as_secs();
                *YOU_ACTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(at);
                println!(
                    "     you used the mouse or keyboard {at} s into the run; results may be off"
                );
                sign_step("you used the Mac: results may be off / 检测到你的操作，结果可能不准");
                return;
            }
        }
    });
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
    countdown();
    stop_if_you_acted("hid")?;
    let _hidden = (args.first() == Some(&"hover") && args.len() > 3).then(hide_sign);
    OWN_INPUT.store(now_ms() as u64, Ordering::SeqCst);
    let result = sh(tool("hid")?, args);
    OWN_INPUT.store(now_ms() as u64, Ordering::SeqCst);
    result
}

/// A script posts real input of its own now, through another tool than
/// `hid`: the watch for the user's input leaves it out.
pub fn own_input_now() {
    countdown();
    OWN_INPUT.store(now_ms() as u64, Ordering::SeqCst);
}

/// Click a painted Help Card button in the test app through real HID input.
/// Leave the pointer there: moving it back would revoke an Input Grant.
/// The caller restores the pointer after the grant ends.
pub fn click_card_button(control: &Control, pid: u32, button: &str) -> Result<(f64, f64)> {
    let card = windows_of(pid)
        .into_iter()
        .find(|w| w.width == 320.0 && w.layer > 0)
        .context("no test card; refusing to click")?;
    let painted = control.call("debugCard", json!({}))?;
    let b = &painted["buttons"][button];
    let rect: Vec<f64> = (0..4).filter_map(|i| b[i].as_f64()).collect();
    anyhow::ensure!(
        rect.len() == 4
            && rect[0] >= 0.0
            && rect[1] >= 0.0
            && rect[2] > 0.0
            && rect[3] > 0.0
            && rect[0] + rect[2] <= card.width + 0.5
            && rect[1] + rect[3] <= card.height + 0.5,
        "invalid {button} bounds: {painted}"
    );
    let x = card.x + rect[0] + rect[2] / 2.0;
    let y = card.y + rect[1] + rect[3] / 2.0;
    println!(
        "     actual HID click {button} at ({x}, {y}), card {} bounds {b}",
        card.id
    );
    hid(&["click", &x.to_string(), &y.to_string(), "--stay"])?;
    Ok((x, y))
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

/// Kill the processes whose command line holds `path`, a path inside this
/// checkout: a run of the same script in another worktree has its own path,
/// so its fixtures live on.
pub fn kill_ours(path: impl AsRef<Path>) {
    let _ = Command::new("/usr/bin/pkill")
        .arg("-f")
        .arg(path.as_ref())
        .status();
}

/// The executable of the app `name`.app this checkout builds in `out()`,
/// by its real path, which process listings show: `target` may link to a
/// shared target directory.
pub fn our_binary(name: &str) -> PathBuf {
    let path = out().join(format!("{name}.app/Contents/MacOS/{name}"));
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// Quit every browser process on the profile folder `dir`: TERM, then KILL
/// after 5 s, as a throwaway profile's browser has nothing to save. The
/// processes are found by their exact `--user-data-dir=` argument, never
/// by a pattern, so a folder such as `Browser Profiles (e2e)` matches. A
/// process still running after that fails the script.
pub fn quit_profile(dir: &Path) {
    let gone = |signal: libc::c_int| {
        for pid in profile_pids(dir) {
            unsafe { libc::kill(pid, signal) };
        }
        for _ in 0..50 {
            if profile_pids(dir).is_empty() {
                return true;
            }
            sleep_ms(100);
        }
        false
    };
    if profile_pids(dir).is_empty() || gone(libc::SIGTERM) || gone(libc::SIGKILL) {
        return;
    }
    check(
        &format!("the browser on {} quit", dir.display()),
        false,
        format!("still running: {:?}", profile_pids(dir)),
    );
}

/// The processes whose command line has `--user-data-dir=<dir>` as a whole
/// argument.
pub fn profile_pids(dir: &Path) -> Vec<i32> {
    let listing = sh_lenient("/bin/ps", &["-axww", "-o", "pid=,command="]);
    listing
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim_start().split_once(' ')?;
            names_profile(command, dir)
                .then(|| pid.parse().ok())
                .flatten()
        })
        .filter(|&pid: &i32| pid != std::process::id() as i32)
        .collect()
}

/// Whether the command line `command`, as `ps` prints it, passes
/// `--user-data-dir=<dir>` as a whole argument.
fn names_profile(command: &str, dir: &Path) -> bool {
    let wanted = format!("--user-data-dir={}", dir.display());
    command.match_indices(&wanted).any(|(at, _)| {
        let before = command[..at].chars().next_back();
        let after = command[at + wanted.len()..].chars().next();
        before.is_none_or(|c| c == ' ') && after.is_none_or(|c| c == ' ')
    })
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

/// Stop here without failing, unless the test showed a window on the
/// user's screen first.
pub fn skip(reason: &str) -> ! {
    println!("skip: {reason}");
    if !finish_sighting() {
        finish_sign(Outcome::Failed);
        std::process::exit(1);
    }
    finish_sign(Outcome::Skipped(reason));
    std::process::exit(0);
}

/// Run a script's body under the sign, then report: "all passed" or the
/// failure count. An error from the body counts as a failure. The sign
/// turns green whatever the result, a panic too.
pub fn run(body: impl FnOnce() -> Result<()>) -> ExitCode {
    start_sighting();
    let name = script_name();
    // The sign says the user can keep working until the countdown, which
    // comes right before the script acts: the build, the app's start and
    // the script's setup come first. A failed build fails the script when
    // it starts the app.
    open_sign();
    if manifest::needs_app(&name) {
        let _ = bundle_debug();
    }
    // The Swift helpers and fixture apps too: built on first use, they
    // compiled after the countdown and kept a hands-off user waiting.
    prebuild_tools();
    sign_step("preparing, you can keep working / 准备中，可以继续操作");
    RUNNING.store(true, Ordering::SeqCst);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            check("the script ran to the end", false, format!("{e:#}"));
        }
        Err(panic) => {
            fail();
            note_failure("the script panicked");
            finish_sighting();
            finish_sign(Outcome::Failed);
            std::panic::resume_unwind(panic);
        }
    }
    finish_sighting();
    if let Some(at) = *YOU_ACTED.lock().unwrap_or_else(|e| e.into_inner()) {
        println!(
            "note: you used the mouse or keyboard {at} s into the run; a failure may come from that"
        );
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

// MARK: Your screen

/// tools/sighting.swift, watching the user's displays for the test's windows
/// while `run` runs the script.
static SIGHTING: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

/// When the watch began: sightings count their times from it.
static SIGHTING_SINCE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// What a script lets show on the user's screen: an owner ("" for any),
/// how long each of its windows may show there, in ms, and from when, in
/// ms since the watch began.
static ALLOWED: std::sync::Mutex<Vec<(String, u64, i64)>> = std::sync::Mutex::new(Vec::new());

/// Let `owner`'s windows ("" for any process of the test) that first show
/// from now on stay on the user's screen for up to `ms` each, for the
/// reason `why`, printed with the result. The default is none at all: give
/// a budget only for what Agent Screen can't prevent, as a window macOS or
/// the app shows before any call reaches it, or for a window the script
/// puts there on purpose, as `alerts` does.
pub fn allow_on_your_screen(owner: &str, ms: u64, why: &str) {
    println!("allowed on your screen from here: {owner:?} for {ms} ms, {why}");
    let from = SIGHTING_SINCE
        .get()
        .map_or(0, |since| since.elapsed().as_millis() as i64);
    ALLOWED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((owner.to_string(), ms, from));
}

/// A window of the test's that showed on the user's screen.
#[derive(Debug, Deserialize)]
pub struct Sighting {
    pub owner: String,
    pub pid: i64,
    pub path: String,
    pub title: String,
    pub layer: i64,
    pub frame: [f64; 4],
    /// When it first and last showed, in ms since the script started.
    pub first: i64,
    pub last: i64,
    /// How long it showed, in all.
    pub ms: i64,
    /// The 1-minute load average as it first showed.
    #[serde(default)]
    pub load: f64,
}

/// The 1-minute load average above which a time budget isn't judged. The
/// scripts measure a window's time on the user's screen by the wall clock,
/// so on a busy Mac the time Agent Screen and the app wait for a core
/// counts against Agent Screen: with the load at 9 to 72, a running app's
/// new window showed 158 to 226 ms against its 150, nearly all of it in
/// the app's Accessibility answers to the catch. Over 30 launches on a
/// quiet Mac it showed 29 ms in the median and 138 ms at most.
pub const IDLE_LOAD: f64 = 4.0;

/// The 1-minute load average.
pub fn load_average() -> f64 {
    let mut load = [0.0f64; 1];
    // SAFETY: getloadavg writes at most `nelem` doubles into the buffer.
    if unsafe { libc::getloadavg(load.as_mut_ptr(), 1) } == 1 {
        load[0]
    } else {
        0.0
    }
}

/// Check that something took `ms` at most `limit` ms. Over it with the
/// load above `IDLE_LOAD`, `load` at the time, it's not judged: the line
/// says so and counts neither way.
pub fn check_within(label: &str, ms: i64, limit: i64, load: f64, detail: impl AsRef<str>) -> bool {
    let detail = detail.as_ref();
    if ms > limit && load > IDLE_LOAD {
        println!(
            "not judged {label}: {detail}, over {limit} ms at load {load:.1}, above {IDLE_LOAD}"
        );
        return true;
    }
    check(label, ms <= limit, detail)
}

/// Start watching the user's displays for windows of processes the test
/// started. E2E_NO_SIGHTING=1 turns it off.
fn start_sighting() {
    if env_is("E2E_NO_SIGHTING", "1") {
        return;
    }
    // The watcher times its sightings from `since`, and allowances count
    // from SIGHTING_SINCE: both clocks start here. Before, the watcher timed
    // from its own launch, some time after the spawn, so a window the script
    // allowed looked older than the allowance.
    let _ = SIGHTING_SINCE.set(Instant::now());
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let mark = root().to_string_lossy().into_owned() + "/";
    // The watcher compares real executable paths: a `target` that is a
    // symlink, as in a worktree sharing another's build, would hide neither.
    let real = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    let overlay = real(out().join("overlay")).to_string_lossy().into_owned();
    let app = real(debug_app()).to_string_lossy().into_owned() + "/";
    let started = tool("sighting").and_then(|watcher| {
        Ok(command(watcher)
            .args([
                &std::process::id().to_string(),
                &since.to_string(),
                &mark,
                &overlay,
                &app,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?)
    });
    match started {
        Ok(child) => *SIGHTING.lock().unwrap_or_else(|e| e.into_inner()) = Some(child),
        Err(e) => println!("not watching your screen: {e:#}"),
    }
}

/// The sightings past what `allowed` lets show, and those only over their
/// allowance's time at a load above `IDLE_LOAD`, which aren't judged: the
/// wall clock counts the waits for a core. A window with no allowance
/// fails at any load.
fn judge_sightings<'a>(
    seen: &'a [Sighting],
    allowed: &[(String, u64, i64)],
) -> (Vec<&'a Sighting>, Vec<&'a Sighting>) {
    let mut offenders = Vec::new();
    let mut unjudged = Vec::new();
    for s in seen {
        let budgets: Vec<u64> = allowed
            .iter()
            .filter(|(owner, _, from)| (owner.is_empty() || *owner == s.owner) && s.first >= *from)
            .map(|(_, ms, _)| *ms)
            .collect();
        if budgets.iter().any(|ms| s.ms as u64 <= *ms) {
            continue;
        }
        if !budgets.is_empty() && s.load > IDLE_LOAD {
            unjudged.push(s);
        } else {
            offenders.push(s);
        }
    }
    (offenders, unjudged)
}

/// Stop watching and check that nothing the test started showed on the
/// user's screen, past what the script allowed. False when something did.
fn finish_sighting() -> bool {
    let Some(mut child) = SIGHTING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return true;
    };
    drop(child.stdin.take());
    let Ok(output) = child.wait_with_output() else {
        return true;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let _ = std::fs::write(
        out().join(format!("sightings-{}.jsonl", script_name())),
        text.as_bytes(),
    );
    let allowed = ALLOWED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let seen: Vec<Sighting> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let (offenders, unjudged) = judge_sightings(&seen, &allowed);
    let lines = |list: &[&Sighting]| -> String {
        list.iter()
            .map(|s| {
                let [x, y, w, h] = s.frame;
                format!(
                    "\n  {} (pid {}, layer {}) {:?} at {x},{y} {w}x{h}: {} ms, from {} to {} ms, load {:.1}",
                    s.owner, s.pid, s.layer, s.title, s.ms, s.first, s.last, s.load
                )
            })
            .collect()
    };
    if !unjudged.is_empty() {
        println!(
            "not judged: {} window(s) over their allowance at a load above {IDLE_LOAD}{}",
            unjudged.len(),
            lines(&unjudged)
        );
    }
    check(
        "nothing the test started showed on your screen",
        offenders.is_empty(),
        format!(
            "{} window(s), {} allowed, {} not judged{}",
            offenders.len(),
            seen.len() - offenders.len() - unjudged.len(),
            unjudged.len(),
            lines(&offenders)
        ),
    )
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

/// Wait for `ready`, which says what it still waits for. The sign shows
/// that from the start, so the user knows the script will begin as soon as
/// they stop, and can stop now rather than leave.
fn wait_until(limit: Duration, mut ready: impl FnMut() -> Option<&'static str>) -> bool {
    let start = Instant::now();
    let mut shown = None;
    while start.elapsed() < limit {
        let Some(waiting) = ready() else {
            return true;
        };
        if shown != Some(waiting) {
            println!("waiting: {waiting}");
            if open_sign() {
                sign_step(&format!("waiting / 等待中: {waiting}"));
            }
            shown = Some(waiting);
        }
        sleep_ms(500);
    }
    false
}

/// Whether the running script needs the user's hands off or asks them to
/// act: only then does it count down and wait for them to stop.
fn counts_down() -> bool {
    let name = script_name();
    manifest::hands_off(&name) || manifest::user_acts(&name).is_some()
}

/// Seconds without mouse or keyboard input before a script starts:
/// E2E_IDLE_SECS, 3 by default.
fn quiet_seconds() -> u64 {
    std::env::var("E2E_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

const YOU: &str = "starts 3 s after you stop using the mouse and keyboard / 停手 3 秒后自动开始";
const LOCK: &str = "the screen is locked / 屏幕已锁定";
const DISPLAYS: &str =
    "close your Agent Displays first; restarting the app ends them / 请先关闭 Agent Display";

fn idle_seconds() -> u64 {
    probe(&["idle"])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Wait until the user has stopped for a moment (`quiet_seconds`), has no
/// Agent Display open (restarting the app ends every display) and the
/// screen is unlocked. E2E_NOW=1 skips the wait.
pub fn wait_for_quiet() -> bool {
    if env_is("E2E_NOW", "1") {
        return true;
    }
    wait_until(Duration::from_secs(3600), || {
        if locked() {
            Some(LOCK)
        } else if !probe(&["displays"]).is_ok_and(|s| s == "0") {
            Some(DISPLAYS)
        } else {
            (idle_seconds() < quiet_seconds() && counts_down()).then_some(YOU)
        }
    })
}

/// Wait until the user has stopped for a moment with the screen unlocked.
/// Tests beside the user's app that open windows need it: macOS shows a new
/// window on the user's screen for a moment before Agent Screen moves it.
pub fn wait_for_idle() -> bool {
    if env_is("E2E_NOW", "1") {
        return !locked();
    }
    wait_until(Duration::from_secs(3600), || {
        if locked() {
            Some(LOCK)
        } else {
            (idle_seconds() < quiet_seconds() && counts_down()).then_some(YOU)
        }
    })
}

/// Wait until the screen is unlocked.
pub fn wait_for_unlocked() -> bool {
    wait_until(Duration::from_secs(3600), || locked().then_some(LOCK))
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

pub fn binary_of(app: &Path) -> String {
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

/// Build the debug bundle, once per script: `run` builds it before the
/// countdown, and starting the app later reuses that build.
fn bundle_debug() -> Result<()> {
    static BUILT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            sign_step(
                "building the test app, you can keep working / 正在编译测试 app，可以继续操作",
            );
            sh(root().join("apps/macos/scripts/bundle.sh"), &["debug"])
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        })
        .clone()
        .map_err(|e| anyhow::anyhow!(e))
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
    sign_step("starting the test app / 正在启动测试 app");
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
    // An instance another checkout left, or this checkout's from before the
    // build, would answer for code the script doesn't test. Scripts run one
    // at a time under the shared lock, so none is using it.
    if listening() && !current_instance() {
        stop_test_app();
    }
    if listening() {
        return Ok(());
    }
    spawn_debug(env)?;
    if !wait_listening() {
        bail!("the test app didn't start");
    }
    Ok(())
}

/// Whether the running debug app is this checkout's, started after its
/// binary was last built.
fn current_instance() -> bool {
    let Some(pid) = debug_pid() else {
        return false;
    };
    let built = modified(Path::new(&binary_of(&debug_app())))
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    match (started_at(pid), built) {
        (Some(started), Some(built)) => started >= built,
        _ => false,
    }
}

/// When process `pid` started, in seconds since 1970.
fn started_at(pid: u32) -> Option<u64> {
    let start = sh_lenient("/bin/ps", &["-o", "lstart=", "-p", &pid.to_string()]);
    sh_lenient(
        "/bin/date",
        &["-j", "-f", "%a %b %d %T %Y", start.trim(), "+%s"],
    )
    .trim()
    .parse()
    .ok()
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

/// A stage that supplies the user-display role to the normal window
/// ownership rules. The independent sighting watcher still sees hardware.
pub fn open_user_stage(control: &Control) -> Result<u64> {
    let id = open_stage(control)?;
    control.call(
        "debugShow",
        json!({ "surface": "userStage", "displayId": id }),
    )?;
    Ok(id)
}

/// Let Continue on display `id`: a real click in the Viewer took the
/// display over, and this hands it back to the agent.
pub fn let_continue(control: &Control, id: u64) -> Result<()> {
    control.call(
        "debugShow",
        json!({ "surface": "primary", "displayId": id }),
    )?;
    sleep_ms(300);
    Ok(())
}

/// The window on display `id` whose title starts with `prefix`, as
/// `listWindows` lists it for the session `key`.
pub fn window_titled(control: &Control, key: &str, id: u64, prefix: &str) -> Result<Value> {
    let listed = control.call("listWindows", json!({ "sessionKey": key, "displayId": id }))?;
    listed["windows"]
        .as_array()
        .and_then(|w| {
            w.iter()
                .find(|w| w["title"].as_str().is_some_and(|t| t.starts_with(prefix)))
                .cloned()
        })
        .with_context(|| format!("no {prefix} window on the display"))
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

/// What tools/shown.swift captured of one process on one display: the
/// frames it got, and how many held any of the process's pixels.
#[derive(Debug, Deserialize)]
pub struct Shown {
    pub display: u64,
    pub frames: i64,
    pub shown: i64,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub saved: Option<String>,
}

/// Capture what process `pid` draws on the user's displays, and on the
/// `extra` displays, while `work` runs: pixels at 60 fps, so a minimize
/// or restore animation counts though the window list doesn't place the
/// window there. `name` names the saved first frame of each display.
pub fn watch_shown<T>(
    name: &str,
    pid: u64,
    extra: &[u64],
    work: impl FnOnce() -> Result<T>,
) -> Result<(Vec<Shown>, T)> {
    let prefix = out().join(name).to_string_lossy().into_owned();
    let mut args = vec![pid.to_string(), prefix];
    args.extend(extra.iter().map(u64::to_string));
    // ScreenCaptureKit leaves a display out of its list now and then while
    // displays change: try again, and fail rather than prove nothing.
    let mut tries = 0;
    let mut watcher = loop {
        tries += 1;
        let mut watcher = command(tool("shown")?)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting shown")?;
        // Its captures run once it says so on stderr.
        let mut err = std::io::BufReader::new(watcher.stderr.take().context("shown's stderr")?);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut err, &mut line)?;
        if line.trim() == "ready" {
            break watcher;
        }
        let _ = watcher.kill();
        let _ = watcher.wait();
        if tries == 3 {
            bail!("shown: {}", line.trim());
        }
        println!("     shown: {}; trying again", line.trim());
        sleep_ms(500);
    };
    // Frames already on screen arrive first: let them pass.
    sleep_ms(300);
    let done = work();
    // The animation may run on after the call answers.
    sleep_ms(800);
    drop(watcher.stdin.take());
    let out = watcher.wait_with_output()?;
    let done = done?;
    let seen = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
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

// MARK: Front and window order while an action runs

/// The front app's changes while `act` runs, from `probe fronts`, polled
/// every 10 ms: "epoch-ms pid name" per line.
///
/// With `harness`, the window order too: "epoch-ms order:above" when the
/// harness's top window on the main screen is above the front app's,
/// "order:below" or "order:apart" otherwise, at each change.
pub fn fronts_during<T>(
    seconds: u64,
    harness: Option<&str>,
    act: impl FnOnce() -> T,
) -> Result<(T, Vec<String>)> {
    let probe = tool("probe")?;
    let fronts = std::process::Command::new(&probe)
        .args(["fronts", &seconds.to_string()])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let order = match harness {
        Some(pid) => Some(
            std::process::Command::new(&probe)
                .args(["above", pid, &seconds.to_string()])
                .stdout(std::process::Stdio::piped())
                .spawn()?,
        ),
        None => None,
    };
    sleep_ms(200);
    let result = act();
    let output = fronts.wait_with_output()?;
    let mut lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(String::from)
        .collect();
    if let Some(order) = order {
        let output = order.wait_with_output()?;
        lines.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|l| l.split_once(' '))
                .map(|(at, how)| format!("{at} order:{how}")),
        );
        lines.sort();
    }
    Ok((result, lines))
}

/// The window of the app sampled with `fronts_during` rose above your front app's after it had been
/// below it: `order:above` lines after the first `order:below`.
pub fn raised(fronts: &[String]) -> Vec<String> {
    let Some(below) = fronts.iter().position(|l| l.ends_with(" order:below")) else {
        return Vec::new();
    };
    fronts[below..]
        .iter()
        .filter(|l| l.ends_with(" order:above"))
        .cloned()
        .collect()
}

/// The app `harness` came back to the front after another app had it: lines of
/// `fronts` naming pid `harness` after one that doesn't.
pub fn taken_back(fronts: &[String], harness: &str) -> Vec<String> {
    let fronts: Vec<String> = fronts
        .iter()
        .filter(|l| !l.contains(" order:"))
        .cloned()
        .collect();
    let is_harness = |l: &String| l.split_whitespace().nth(1) == Some(harness);
    let Some(left) = fronts.iter().position(|l| !is_harness(l)) else {
        return Vec::new();
    };
    fronts[left..]
        .iter()
        .filter(|l| is_harness(l))
        .cloned()
        .collect()
}

// MARK: Autonomy Modes

/// tools/keyholder.swift as the user's front app on their screen, its
/// field's text reported (ADR 0025's scripts): keys the armed hold held
/// for the user land there.
pub struct Holder {
    child: std::process::Child,
    printed: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Holder {
    /// Start it for `seconds`; it takes the front and its field the
    /// keyboard.
    pub fn start(seconds: u32) -> Result<Holder> {
        use std::io::Read;
        let mut child = command(tool("keyholder")?)
            .arg(seconds.to_string())
            .env("KEYHOLDER_TEXT", "1")
            .stdout(Stdio::piped())
            .spawn()?;
        let mut stdout = child.stdout.take().context("keyholder's stdout")?;
        let printed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = printed.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = sink.lock() {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        });
        Ok(Holder { child, printed })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything it printed: key changes and each text, in order.
    pub fn printed(&self) -> String {
        self.printed.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The field's text now.
    pub fn text(&self) -> String {
        let printed = self.printed.lock().map(|s| s.clone()).unwrap_or_default();
        printed
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("text "))
            .unwrap_or_default()
            .to_string()
    }

    /// Bring it to the front again.
    pub fn activate(&self) {
        let _ = probe(&["activate", &self.pid().to_string()]);
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The Red Glow's states and the mode that opened the Input Grant
/// (`debugAutonomy`'s `grant`), as a second connection reads them every
/// 30 ms until `stop` is set.
pub fn watch_glow(stop: &std::sync::atomic::AtomicBool) -> (Vec<String>, Vec<String>) {
    let mut glow = Vec::new();
    let mut opened = Vec::new();
    let Ok(control) = Control::connect() else {
        return (glow, opened);
    };
    while !stop.load(Ordering::Relaxed) {
        if let Ok(g) = control.call("debugGlow", serde_json::json!({ "alpha": false }))
            && let Some(state) = g["state"].as_str()
            && glow.last().map(String::as_str) != Some(state)
        {
            glow.push(state.to_string());
        }
        if let Ok(a) = control.call("debugAutonomy", serde_json::json!({}))
            && let Some(by) = a["grant"].as_str()
            && opened.last().map(String::as_str) != Some(by)
        {
            opened.push(by.to_string());
        }
        sleep_ms(30);
    }
    (glow, opened)
}

/// Wait up to `ms` for the Red Glow to read `state`.
pub fn wait_glow(control: &Control, state: &str, ms: u64) -> bool {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        if control
            .call("debugGlow", serde_json::json!({ "alpha": false }))
            .is_ok_and(|g| g["state"] == state)
        {
            return true;
        }
        sleep_ms(15);
    }
    false
}

/// Whether the user's hands are At Rest to the e2e instance now.
pub fn at_rest(control: &Control) -> bool {
    control
        .call("debugAutonomy", serde_json::json!({}))
        .is_ok_and(|s| s["tracker"]["hands"]["atRest"] == true)
}

#[cfg(test)]
mod tests {
    use super::names_profile;
    use std::path::Path;

    #[test]
    fn a_profile_with_parentheses_and_spaces_matches_only_as_a_whole_argument() {
        let dir = Path::new("/U/Agent Screen/Browser Profiles (e2e)/com.google.Chrome");
        let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let agents = format!(
            "{chrome} --remote-debugging-pipe --user-data-dir={} --no-first-run",
            dir.display()
        );
        assert!(names_profile(&agents, dir));
        assert!(names_profile(
            &format!("{chrome} --user-data-dir={}", dir.display()),
            dir
        ));
        assert!(!names_profile(
            &format!("{chrome} --user-data-dir={}2", dir.display()),
            dir
        ));
        assert!(!names_profile(chrome, dir));
        assert!(!names_profile(
            &format!(
                "{chrome} --user-data-dir=/U/Agent Screen/Browser Profiles e2e/com.google.Chrome"
            ),
            dir
        ));
    }

    fn sighting(owner: &str, ms: i64, load: f64) -> super::Sighting {
        super::Sighting {
            owner: owner.into(),
            pid: 1,
            path: String::new(),
            title: String::new(),
            layer: 0,
            frame: [0.0; 4],
            first: 1000,
            last: 1000 + ms,
            ms,
            load,
        }
    }

    #[test]
    fn a_window_over_its_allowance_fails_at_idle_and_is_not_judged_under_load() {
        let allowed = vec![("TextEdit".to_string(), 150, 0)];
        let seen = [
            sighting("TextEdit", 120, 0.5),
            sighting("TextEdit", 300, 1.0),
            sighting("TextEdit", 187, 12.0),
            sighting("Calculator", 40, 12.0),
        ];
        let (offenders, unjudged) = super::judge_sightings(&seen, &allowed);
        let ms = |list: &[&super::Sighting]| list.iter().map(|s| s.ms).collect::<Vec<_>>();
        // At idle a 300 ms flash fails; with no allowance a window fails at
        // any load.
        assert_eq!(ms(&offenders), [300, 40]);
        assert_eq!(ms(&unjudged), [187]);
    }
}
