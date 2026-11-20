//! A private DevTools pipe into the Chromium and Electron apps `launch_app`
//! starts (ADR 0010), and the drags that go through it (#37).
//!
//! Agent Screen starts such an app itself, not through `open`, with
//! `--remote-debugging-pipe`: Chromium then reads DevTools commands from fd
//! 3 and writes its answers to fd 4, which only Agent Screen holds. No port
//! opens, so no other process can connect. An app that strips the switch,
//! or Chrome on its default profile, never answers, and gets no pipe.
//!
//! A drag in a web page goes through `Input.dispatchMouseEvent` with the
//! button held, and an HTML5 drag through `Input.dispatchDragEvent`, inside
//! the browser: nothing reaches WindowServer, so the session's button state
//! and the user's pointer stay theirs.

use crate::model::{Rect, WindowTarget};
use agent_screen_model::devtools::{self as rules, DragRoute, Engine, PageView, Viewport};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Where a page the pipe asked for went.
#[derive(Debug)]
pub enum NewPage {
    /// Into a new window, which the launch's catch takes: the window that
    /// shows `target`.
    Window { target: String },
    /// Into a window that was open already, as a tab: Dia ignores
    /// `newWindow`. Its window's frame, in global points.
    Tab { target: String, bounds: Rect },
    /// Nowhere: the browser keeps its pages in windows Chromium gives no
    /// frame for, as Dia does, and made one that never shows, now closed.
    Frameless,
    /// The browser didn't say.
    Unknown,
}

/// Whether the app at `bundle` is built on Chromium: Chrome, Edge, Brave,
/// Arc and every Electron or CEF app ship Chromium's resources in a
/// framework of their own.
pub fn is_chromium(bundle: &Path) -> bool {
    let Ok(frameworks) = std::fs::read_dir(bundle.join("Contents/Frameworks")) else {
        return false;
    };
    frameworks.flatten().any(|f| {
        let f = f.path();
        f.extension().is_some_and(|e| e == "framework")
            && (f.join("Resources/chrome_100_percent.pak").exists()
                || f.join("Versions/Current/Resources/chrome_100_percent.pak")
                    .exists())
    })
}

/// The engine of the web pages an app shows: Chromium by its framework,
/// Gecko by Firefox's, and WebKit for any other app with a web area.
pub fn engine(pid: i32) -> Option<Engine> {
    let bundle = super::app::bundle_of(pid)?;
    Some(if is_chromium(&bundle) {
        Engine::Chromium
    } else if bundle.join("Contents/MacOS/XUL").exists() {
        Engine::Gecko
    } else {
        Engine::WebKit
    })
}

// MARK: Starting the app

const POSIX_SPAWN_SETSID: libc::c_short = 0x0400;

unsafe extern "C" {
    /// The child is responsible for itself in TCC, as an app `open` starts
    /// is: its camera or microphone prompts name it, not Agent Screen, and
    /// it gets none of Agent Screen's grants.
    fn responsibility_spawnattrs_setdisclaim(
        attrs: *mut libc::posix_spawnattr_t,
        disclaim: libc::c_int,
    ) -> libc::c_int;
}

fn c_string(s: &str) -> std::io::Result<std::ffi::CString> {
    std::ffi::CString::new(s).map_err(|_| std::io::Error::other("a NUL in an argument"))
}

/// Two pipes, every end close-on-exec, so no other child inherits them.
fn pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for fd in fds {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((fds[0], fds[1]))
}

/// The environment an app gets from Agent Screen: its own, without what
/// marks a process as Agent Screen, plus `extra`.
fn environment(extra: &[(String, String)]) -> Vec<String> {
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            !k.starts_with("AGENTSCREEN_")
                && !matches!(k.as_str(), "__CFBundleIdentifier" | "XPC_SERVICE_NAME")
        })
        .collect();
    for (k, v) in extra {
        vars.retain(|(name, _)| name != k);
        vars.push((k.clone(), v.clone()));
    }
    vars.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// Start the app at `bundle` with a DevTools pipe and `arguments` after the
/// switch, as `open -n` would start a new instance: its own session, its
/// own TCC responsibility, the working directory /, nothing on stdin and
/// stdout, and no other descriptor of Agent Screen's. The new pid; the
/// pipe registers once the app answers on it.
pub fn spawn(
    bundle: &Path,
    arguments: &[String],
    extra_environment: &[(String, String)],
) -> std::io::Result<i32> {
    let executable = super::app::main_executable(bundle)
        .ok_or_else(|| std::io::Error::other("no executable in the bundle"))?;
    let executable = c_string(&executable.to_string_lossy())?;
    let mut argv = vec![executable.clone(), c_string("--remote-debugging-pipe")?];
    for a in arguments {
        argv.push(c_string(a)?);
    }
    let envp = environment(extra_environment)
        .iter()
        .map(|e| c_string(e))
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut argv_ptrs: Vec<*mut libc::c_char> = argv.iter().map(|a| a.as_ptr() as *mut _).collect();
    argv_ptrs.push(std::ptr::null_mut());
    let mut envp_ptrs: Vec<*mut libc::c_char> = envp.iter().map(|e| e.as_ptr() as *mut _).collect();
    envp_ptrs.push(std::ptr::null_mut());

    // Commands go from `to_app.1` to the app's fd 3; answers come from its
    // fd 4 to `from_app.0`.
    let to_app = pipe()?;
    let from_app = match pipe() {
        Ok(p) => p,
        Err(e) => {
            unsafe {
                libc::close(to_app.0);
                libc::close(to_app.1);
            }
            return Err(e);
        }
    };
    let null = c"/dev/null";
    let root = c"/";
    let mut pid: libc::pid_t = 0;
    let result = unsafe {
        let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
        let mut attrs: libc::posix_spawnattr_t = std::ptr::null_mut();
        libc::posix_spawn_file_actions_init(&mut actions);
        libc::posix_spawnattr_init(&mut attrs);
        libc::posix_spawn_file_actions_addopen(&mut actions, 0, null.as_ptr(), libc::O_RDONLY, 0);
        libc::posix_spawn_file_actions_addopen(&mut actions, 1, null.as_ptr(), libc::O_WRONLY, 0);
        libc::posix_spawn_file_actions_addopen(&mut actions, 2, null.as_ptr(), libc::O_WRONLY, 0);
        libc::posix_spawn_file_actions_adddup2(&mut actions, to_app.0, 3);
        libc::posix_spawn_file_actions_adddup2(&mut actions, from_app.1, 4);
        libc::posix_spawn_file_actions_addchdir_np(&mut actions, root.as_ptr());
        let mut all: libc::sigset_t = 0;
        let mut none: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::sigemptyset(&mut none);
        libc::posix_spawnattr_setsigdefault(&mut attrs, &all);
        libc::posix_spawnattr_setsigmask(&mut attrs, &none);
        libc::posix_spawnattr_setflags(
            &mut attrs,
            (libc::POSIX_SPAWN_CLOEXEC_DEFAULT
                | libc::POSIX_SPAWN_SETSIGDEF
                | libc::POSIX_SPAWN_SETSIGMASK) as libc::c_short
                | POSIX_SPAWN_SETSID,
        );
        responsibility_spawnattrs_setdisclaim(&mut attrs, 1);
        let r = libc::posix_spawn(
            &mut pid,
            executable.as_ptr(),
            &actions,
            &attrs,
            argv_ptrs.as_ptr(),
            envp_ptrs.as_ptr(),
        );
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attrs);
        libc::close(to_app.0);
        libc::close(from_app.1);
        r
    };
    if result != 0 {
        unsafe {
            libc::close(to_app.1);
            libc::close(from_app.0);
        }
        return Err(std::io::Error::from_raw_os_error(result));
    }
    let channel = Arc::new(Channel {
        pid,
        writer: Mutex::new(unsafe { std::fs::File::from_raw_fd(to_app.1) }),
        next_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        events: Mutex::new(VecDeque::new()),
        state: Mutex::new(State::Starting),
        changed: Condvar::new(),
        sessions: Mutex::new(HashMap::new()),
        discover_id: AtomicU64::new(0),
        discovering: AtomicBool::new(false),
        creating: AtomicUsize::new(0),
        created_until: Mutex::new(None),
        addressless: Mutex::new(HashMap::new()),
    });
    channels().insert(pid, channel.clone());
    let reader = unsafe { std::fs::File::from_raw_fd(from_app.0) };
    {
        let channel = channel.clone();
        std::thread::Builder::new()
            .name("agent-screen-devtools".into())
            .spawn(move || channel.read(reader))?;
    }
    // Reap the app when it quits; its pipe goes with it.
    std::thread::Builder::new()
        .name("agent-screen-reap".into())
        .spawn(move || {
            let mut status = 0;
            unsafe { libc::waitpid(pid, &mut status, 0) };
            log::debug!("devtools: pid {pid} exited");
            channels().remove(&pid);
        })?;
    // Chromium answers once its browser process is up, in about a second;
    // one that ignores the switch never does.
    std::thread::Builder::new()
        .name("agent-screen-devtools-hello".into())
        .spawn(move || {
            let answer = channel.send("Browser.getVersion", json!({}), None);
            let state = match answer.recv_timeout(HELLO_WAIT) {
                Ok(Ok(version)) => {
                    log::info!(
                        "devtools: pid {pid} answers on its pipe: {}",
                        version["product"].as_str().unwrap_or("?")
                    );
                    State::Ready
                }
                _ => {
                    log::info!(
                        "devtools: pid {pid} didn't answer on its pipe; it has none (the app may strip the switch)"
                    );
                    State::Dead
                }
            };
            channel.set_state(state);
            if state == State::Dead {
                channels().remove(&pid);
            }
        })?;
    log::info!("devtools: started pid {pid} with a pipe");
    Ok(pid)
}

/// How long a starting app has to answer on its pipe before it counts as
/// one without.
const HELLO_WAIT: Duration = Duration::from_secs(30);

// MARK: The pipe

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Starting,
    Ready,
    Dead,
}

type Answer = Result<Value, String>;

pub struct Channel {
    pid: i32,
    writer: Mutex<std::fs::File>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, mpsc::Sender<Answer>>>,
    /// Recent events, for `Input.dragIntercepted`.
    events: Mutex<VecDeque<Value>>,
    state: Mutex<State>,
    changed: Condvar,
    /// Target id to the session attached to it.
    sessions: Mutex<HashMap<String, String>>,
    /// The id of the `Target.setDiscoverTargets` call, then whether it
    /// answered: target events before it are the targets already there.
    discover_id: AtomicU64,
    discovering: AtomicBool,
    /// `Target.createTarget` calls in flight, and until when a page that
    /// appears is still taken for theirs.
    creating: AtomicUsize,
    created_until: Mutex<Option<Instant>>,
    /// Pages from outside with no address yet, since when (#235).
    addressless: Mutex<HashMap<String, Instant>>,
}

fn channels() -> std::sync::MutexGuard<'static, HashMap<i32, Arc<Channel>>> {
    static CHANNELS: std::sync::LazyLock<Mutex<HashMap<i32, Arc<Channel>>>> =
        std::sync::LazyLock::new(Default::default);
    CHANNELS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The agents' own browser instances (ADR 0022), shared by every session:
/// each pid with when it started and its `--user-data-dir=` argument.
fn agents() -> std::sync::MutexGuard<'static, HashMap<i32, (Instant, String)>> {
    static AGENTS: std::sync::LazyLock<Mutex<HashMap<i32, (Instant, String)>>> =
        std::sync::LazyLock::new(Default::default);
    AGENTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Launches and quits of the agents' instances (ADR 0022).
static USE: Mutex<agent_screen_model::agent_browser::AgentsUse> =
    Mutex::new(agent_screen_model::agent_browser::AgentsUse::new());
static USE_CHANGED: std::sync::Condvar = std::sync::Condvar::new();

fn using() -> std::sync::MutexGuard<'static, agent_screen_model::agent_browser::AgentsUse> {
    USE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A launch's hold on the agents' instances: no idle quit starts until it
/// drops.
pub struct AgentsHold(());

impl Drop for AgentsHold {
    fn drop(&mut self) {
        using().release();
        USE_CHANGED.notify_all();
    }
}

/// Hold the agents' instances for a launch that may open a window in one.
/// Waits for a quit that started to end, up to 10 s, so the launch then
/// finds the instance gone and starts a new one.
pub fn hold_agents() -> AgentsHold {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut state = using();
    while !state.hold() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warn!(
                "devtools: a quit of the agents' browser still runs after 10 s; launching anyway"
            );
            state.force_hold();
            break;
        }
        state = USE_CHANGED
            .wait_timeout(state, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    AgentsHold(())
}

/// Run `quit` unless a launch holds the agents' instances, or anyway with
/// `force`. Whether it ran.
fn quitting<T>(force: bool, quit: impl FnOnce() -> T) -> Option<T> {
    if !using().begin_quit(force) {
        return None;
    }
    let done = quit();
    using().end_quit();
    USE_CHANGED.notify_all();
    Some(done)
}

/// Note that `pid` is an agents' browser instance Agent Screen started with
/// `profile_argument`, its `--user-data-dir=` switch. A watch quits it once
/// its pipe counts no page.
pub fn mark_agents(pid: i32, profile_argument: &str) {
    agents().insert(pid, (Instant::now(), profile_argument.to_string()));
    std::thread::spawn(move || {
        if let Some(c) = channel(pid, Duration::from_secs(15)) {
            c.discover();
        }
    });
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(2));
                end_agents(false);
            }
        });
    });
}

/// Whether `pid` is an agents' browser instance Agent Screen started.
pub fn is_agents(pid: i32) -> bool {
    agents().contains_key(&pid)
}

/// Whether any agents' browser instance Agent Screen started may run.
pub fn any_agents() -> bool {
    !agents().is_empty()
}

/// Whether `pid` still is that instance: a pid can be reused after a crash.
fn still_agents(pid: i32) -> bool {
    let Some(argument) = agents().get(&pid).map(|(_, a)| a.clone()) else {
        return false;
    };
    let ours = super::app::arguments(pid).is_some_and(|argv| argv.contains(&argument));
    if !ours {
        agents().remove(&pid);
    }
    ours
}

/// Close the agents' window `target` through its instance's pipe: Dia's
/// close button takes no accessibility press. Whether it asked.
pub fn close_agents_window(target: WindowTarget) -> bool {
    if !is_agents(target.pid) {
        return false;
    }
    let Some(frame) = super::windows::find(target.window_id).map(|w| w.bounds) else {
        return false;
    };
    channel(target.pid, Duration::from_secs(1)).is_some_and(|c| c.close_window_at(frame) > 0)
}

/// Quit the agents' browser instance `pid`: hidden first, through its pipe,
/// then, if it still runs after 1 s, with SIGTERM, and after 2 s more with
/// SIGKILL. It is Agent Screen's own child, on the agents' profile, checked
/// by its command line before any signal.
/// Skipped while a launch holds the agents' instances, which may be
/// opening a window in this one.
pub fn quit_agents(pid: i32) -> bool {
    quitting(false, || quit_now(pid)).unwrap_or(false)
}

fn quit_now(pid: i32) -> bool {
    if !still_agents(pid) {
        return false;
    }
    let ended = |ms: u64| {
        (0..ms / 100).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            (unsafe { libc::kill(pid, 0) }) != 0
        })
    };
    // Hidden first: Dia moves a window it closes to the middle of the main
    // screen, where the user would see it. Hiding activates nothing.
    super::app::hide(pid);
    for _ in 0..10 {
        if super::app::is_hidden(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let closed = channel(pid, Duration::from_secs(1)).is_some_and(|c| c.close_browser());
    let gone = ended(1000)
        || (unsafe { libc::kill(pid, libc::SIGTERM) }) != 0
        || ended(2000)
        || (unsafe { libc::kill(pid, libc::SIGKILL) }) != 0
        || ended(1000);
    log::info!(
        "devtools: quit the agents' browser instance {pid}: asked through its pipe {closed}, ended {gone}"
    );
    agents().remove(&pid);
    gone
}

/// What to do now that the agents' instance `pid` came to the front by
/// something other than Agent Screen, from the user's latest input.
pub fn on_agents_activated(pid: i32) -> agent_screen_model::agent_browser::AgentsActivated {
    let users_running = super::app::bundle_of(pid).is_some_and(|bundle| {
        super::app::running_pids(&bundle)
            .iter()
            .any(|&p| !is_agents(p))
    });
    agent_screen_model::agent_browser::agents_activated(
        &super::catch::recent_presses(pid),
        users_running,
    )
}

/// Start the user's own browser of the agents' instance `pid`, a new
/// instance on its normal profile, through LaunchServices. A plain launch
/// would hand it to the agents' instance again. Whether it asked.
pub fn start_users_browser(pid: i32) -> bool {
    let Some(bundle) = super::app::bundle_of(pid) else {
        return false;
    };
    // A link that brought the instance forward opens the user's browser
    // with its page: a second start would add an empty window.
    std::thread::sleep(Duration::from_millis(500));
    if forwarded_recently() {
        log::info!("devtools: a link already opened the user's browser");
        return false;
    }
    super::app::open_new_instance(&bundle, None, &users_browser_arguments());
    log::info!(
        "devtools: the user launched their browser and the agents' instance {pid} got it; opening theirs: {}",
        bundle.display()
    );
    true
}

/// When Agent Screen last forwarded a link to the user's browser.
static FORWARDED: Mutex<Option<Instant>> = Mutex::new(None);

fn forwarded_recently() -> bool {
    FORWARDED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
}

/// Test builds may start the user's browser with these arguments, such as
/// a stand-in profile, so a test never opens the user's own.
fn users_browser_arguments() -> Vec<String> {
    if !cfg!(debug_assertions) {
        return Vec::new();
    }
    std::env::var("AGENTSCREEN_USERS_BROWSER_ARGS")
        .ok()
        .and_then(|a| serde_json::from_str(&a).ok())
        .unwrap_or_default()
}

/// Move the page `target`, which a link from outside opened in the agents'
/// instance `pid`, to the user's own browser: close it there and open
/// `url` through LaunchServices in a new instance on the user's profile.
/// When the user's browser runs, that new instance hands the page to it
/// and quits, as Chromium does with a second start on one profile (#235).
fn forward_link(pid: i32, target: &str, url: &str) {
    let Some(bundle) = super::app::bundle_of(pid) else {
        return;
    };
    *FORWARDED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    let closed = channel(pid, Duration::from_secs(1)).is_some_and(|c| c.close_target(target));
    super::app::open_new_instance(&bundle, Some(url), &users_browser_arguments());
    log::info!(
        "devtools: a link reached the agents' instance {pid}; closed its page {closed} and opened it in the user's browser"
    );
}

/// Quit each agents' browser instance that has nothing left, as its pipe
/// counts pages, or every one when `all`, as Agent Screen quits: left
/// running, it would take the user's Dock click, `open -a` and links once
/// the user's own browser isn't running, since macOS hands those to any
/// running instance.
pub fn end_agents(all: bool) {
    let pids: Vec<(i32, Instant)> = agents().iter().map(|(p, (at, _))| (*p, *at)).collect();
    for (pid, started) in pids {
        if unsafe { libc::kill(pid, 0) } != 0 {
            agents().remove(&pid);
            continue;
        }
        let idle = || {
            agent_screen_model::agent_browser::idle(
                started.elapsed().as_secs(),
                channel(pid, Duration::from_secs(1)).and_then(|c| c.page_count()),
            )
        };
        // A launch that takes the instance after the count keeps it: the
        // quit doesn't start while the launch holds it.
        if all || idle() {
            quitting(all, || quit_now(pid));
        }
    }
}

/// Whether Agent Screen holds a live pipe to `pid`.
pub fn has_pipe(pid: i32) -> bool {
    channels()
        .get(&pid)
        .is_some_and(|c| *c.state.lock().unwrap_or_else(|e| e.into_inner()) != State::Dead)
}

/// The app's pipe, once it answers on it; waits up to `wait` for a pipe
/// still starting.
pub fn channel(pid: i32, wait: Duration) -> Option<Arc<Channel>> {
    let channel = channels().get(&pid).cloned()?;
    let started = Instant::now();
    let mut state = channel.state.lock().unwrap_or_else(|e| e.into_inner());
    while *state == State::Starting {
        let left = wait.checked_sub(started.elapsed())?;
        state = channel
            .changed
            .wait_timeout(state, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    (*state == State::Ready).then(|| channel.clone())
}

impl Channel {
    fn set_state(&self, state: State) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *s != State::Dead {
            *s = state;
        }
        self.changed.notify_all();
    }

    fn read(&self, mut reader: std::fs::File) {
        let mut buffer = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            buffer.extend_from_slice(&chunk[..n]);
            for message in rules::take_messages(&mut buffer) {
                let Ok(message) = serde_json::from_slice::<Value>(&message) else {
                    continue;
                };
                self.take(message);
            }
        }
        log::debug!("devtools: pid {}'s pipe closed", self.pid);
        self.set_state(State::Dead);
        for (_, waiting) in self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
        {
            let _ = waiting.send(Err("the app closed its DevTools pipe".into()));
        }
        let mut all = channels();
        if all.get(&self.pid).is_some_and(|c| std::ptr::eq(&**c, self)) {
            all.remove(&self.pid);
        }
    }

    fn take(&self, message: Value) {
        if let Some(id) = message["id"].as_u64() {
            if id == self.discover_id.load(Ordering::SeqCst) {
                self.discovering.store(true, Ordering::SeqCst);
            }
            let waiting = self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            if let Some(waiting) = waiting {
                let answer = match message.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(message["result"].clone()),
                };
                let _ = waiting.send(answer);
            }
            return;
        }
        if message["method"] == "Target.detachedFromTarget"
            && let Some(session) = message["params"]["sessionId"].as_str()
        {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, s| s != session);
        }
        if self.discovering.load(Ordering::SeqCst)
            && matches!(
                message["method"].as_str(),
                Some("Target.targetCreated" | "Target.targetInfoChanged")
            )
        {
            self.page_seen(
                &message["params"]["targetInfo"],
                message["method"] == "Target.targetCreated",
            );
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        events.push_back(message);
        while events.len() > 64 {
            events.pop_front();
        }
    }

    fn send(&self, method: &str, params: Value, session: Option<&str>) -> mpsc::Receiver<Answer> {
        self.send_as(
            self.next_id.fetch_add(1, Ordering::SeqCst),
            method,
            params,
            session,
        )
    }

    fn send_as(
        &self,
        id: u64,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> mpsc::Receiver<Answer> {
        let (tx, rx) = mpsc::channel();
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx.clone());
        let mut bytes = serde_json::to_vec(&message).unwrap_or_default();
        bytes.push(0);
        let written = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&bytes);
        if written.is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            let _ = tx.send(Err("the app's DevTools pipe is closed".into()));
        }
        rx
    }

    fn call(&self, method: &str, params: Value, session: Option<&str>) -> Answer {
        let gate = super::real_input::process_gate(self.pid);
        let _input = method
            .starts_with("Input.")
            .then(|| gate.read().unwrap_or_else(|e| e.into_inner()));
        self.send(method, params, session)
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|_| Err(format!("{method} got no answer")))
    }

    /// Open a new browser window showing `url`, at `frame` in global
    /// points, in the background. Chromium makes it there, so it never
    /// shows elsewhere first.
    pub fn new_window(&self, url: &str, frame: Rect) -> Result<NewPage, String> {
        let before = self.page_windows();
        self.creating.fetch_add(1, Ordering::SeqCst);
        let created = self.call(
            "Target.createTarget",
            json!({
                "url": url,
                "newWindow": true,
                "background": true,
                "left": frame.x.round() as i64,
                "top": frame.y.round() as i64,
                "width": frame.width.round() as i64,
                "height": frame.height.round() as i64,
            }),
            None,
        );
        // Its own target event may come just after the answer.
        *self.created_until.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Instant::now() + Duration::from_secs(1));
        self.creating.fetch_sub(1, Ordering::SeqCst);
        let Some(target) = created?["targetId"].as_str().map(String::from) else {
            return Ok(NewPage::Unknown);
        };
        let Some((id, bounds)) = self.window_of(&target) else {
            return Ok(NewPage::Unknown);
        };
        // Dia answers a new window with one of no size that never shows,
        // and keeps its pages in its own windows, which the pipe can't
        // open: the launch opens the page in its window on the display.
        if bounds.width == 0.0 && bounds.height == 0.0 {
            self.close_target(&target);
            log::info!(
                "devtools: pid {} gave window {id} no frame; the page goes into its open window",
                self.pid
            );
            return Ok(NewPage::Frameless);
        }
        let existed = before.contains(&id);
        log::info!(
            "devtools: pid {} opened target {target} in window {id} at {bounds:?}, which existed before: {existed}",
            self.pid
        );
        Ok(if existed {
            NewPage::Tab { target, bounds }
        } else {
            NewPage::Window { target }
        })
    }

    /// Load `url` in the visible page of the window on `display`, in global
    /// points, for a browser that gives its windows no frame (Dia): each
    /// page says whether it is visible and where its window is.
    pub fn navigate_visible_page(&self, display: Rect, url: &str) -> Result<(), String> {
        let targets = self.call("Target.getTargets", json!({}), None)?;
        let pages: Vec<String> = targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str().map(String::from))
            .collect();
        let shown = pages
            .iter()
            .find_map(|target| {
                let session = self.session(target).ok()?;
                let seen = self
                    .call(
                        "Runtime.evaluate",
                        json!({
                            "expression": "[document.visibilityState, screenX + outerWidth / 2, screenY + outerHeight / 2]",
                            "returnByValue": true,
                        }),
                        Some(&session),
                    )
                    .ok()?;
                let v = &seen["result"]["value"];
                log::info!("devtools: pid {} page {target} says {v}", self.pid);
                let visible = v[0] == "visible";
                let mid = (v[1].as_f64()?, v[2].as_f64()?);
                (visible && display.contains(mid)).then_some(session)
            })
            .ok_or_else(|| format!("none of its {} pages is visible on this display", pages.len()))?;
        self.call("Page.navigate", json!({ "url": url }), Some(&shown))
            .map(|_| ())
    }

    /// A command for a test, in the page `target` when given.
    pub fn raw(&self, method: &str, params: Value, target: Option<&str>) -> Answer {
        let session = target.map(|t| self.session(t)).transpose()?;
        self.call(method, params, session.as_deref())
    }

    /// Report the browser's new pages, for links that reach the agents'
    /// instance (#235).
    fn discover(&self) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.discover_id.store(id, Ordering::SeqCst);
        let rx = self.send_as(
            id,
            "Target.setDiscoverTargets",
            json!({ "discover": true }),
            None,
        );
        if let Err(e) = rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|_| Err("no answer".into()))
        {
            log::warn!("devtools: pid {} reports no new pages: {e}", self.pid);
        }
    }

    /// A page target appeared (`created`) or changed: a link from outside
    /// goes to the user's own browser. Only a page that appeared since
    /// discovery counts; a change is followed only for one still waiting
    /// for its address. Runs on the reader thread, so it never waits on the
    /// pipe.
    fn page_seen(&self, info: &Value, created: bool) {
        use agent_screen_model::agent_browser::{LINK_SECS, NewPage, new_page};
        if info["type"] != "page" || !is_agents(self.pid) {
            return;
        }
        let Some(target) = info["targetId"].as_str() else {
            return;
        };
        let url = info["url"].as_str().unwrap_or_default();
        let mut addressless = self.addressless.lock().unwrap_or_else(|e| e.into_inner());
        addressless.retain(|_, at| at.elapsed() < Duration::from_secs(LINK_SECS));
        let opener = info.get("openerId").is_some();
        let (through_pipe, driven) = if created {
            (
                self.creating.load(Ordering::SeqCst) > 0
                    || self
                        .created_until
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_some_and(|until| Instant::now() < until),
                super::catch::agent_drove(self.pid),
            )
        } else {
            (false, false)
        };
        let decided = if created {
            new_page(opener, through_pipe, driven, url)
        } else if addressless.contains_key(target) {
            // It was a page from outside with no address: only the address
            // is new.
            new_page(false, false, false, url)
        } else {
            return;
        };
        match decided {
            NewPage::Wait => {
                log::debug!(
                    "devtools: pid {} page {target} has no address yet",
                    self.pid
                );
                addressless
                    .entry(target.to_string())
                    .or_insert_with(Instant::now);
            }
            NewPage::Keep => {
                log::debug!(
                    "devtools: pid {} keeps its new page {target}: opener {opener}, through the pipe {through_pipe}, agent drove it {driven}",
                    self.pid
                );
                addressless.remove(target);
            }
            NewPage::Forward => {
                addressless.remove(target);
                let (pid, target, url) = (self.pid, target.to_string(), url.to_string());
                std::thread::spawn(move || forward_link(pid, &target, &url));
            }
        }
    }

    /// How many page targets the browser has: its tabs, across its windows.
    pub fn page_count(&self) -> Option<usize> {
        let targets = self.call("Target.getTargets", json!({}), None).ok()?;
        Some(
            targets["targetInfos"]
                .as_array()?
                .iter()
                .filter(|t| t["type"] == "page")
                .count(),
        )
    }

    /// Quit the browser, as its own Quit command would.
    pub fn close_browser(&self) -> bool {
        self.call("Browser.close", json!({}), None).is_ok()
    }

    /// Close the pages of the browser window whose frame, in global points,
    /// is `frame`. How many it closed.
    pub fn close_window_at(&self, frame: Rect) -> usize {
        let Ok(targets) = self.call("Target.getTargets", json!({}), None) else {
            return 0;
        };
        targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str())
            .filter(|id| {
                self.window_of(id).is_some_and(|(_, bounds)| {
                    agent_screen_model::agent_browser::same_frame(bounds, frame)
                })
            })
            .filter(|id| self.close_target(id))
            .count()
    }

    /// The browser window that shows target `id`, and its frame in global
    /// points.
    pub fn window_of(&self, id: &str) -> Option<(i64, Rect)> {
        let window = self
            .call(
                "Browser.getWindowForTarget",
                json!({ "targetId": id }),
                None,
            )
            .ok()?;
        let b = &window["bounds"];
        Some((
            window["windowId"].as_i64()?,
            Rect::new(
                b["left"].as_f64()?,
                b["top"].as_f64()?,
                b["width"].as_f64()?,
                b["height"].as_f64()?,
            ),
        ))
    }

    /// Set the browser window whose frame is `current` to `frame`, both in
    /// global points, through Chromium: Dia ignores `--window-size` and
    /// keeps its first window at the size of the main screen. Only that
    /// window: the instance is shared by every session.
    pub fn fit_window(&self, current: Rect, frame: Rect) -> bool {
        let Ok(targets) = self.call("Target.getTargets", json!({}), None) else {
            return false;
        };
        let window = targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str())
            .filter_map(|id| self.window_of(id))
            .find(|(_, bounds)| agent_screen_model::agent_browser::same_frame(*bounds, current))
            .map(|(id, _)| id);
        let Some(id) = window else {
            return false;
        };
        // A maximized or full-screen window takes no frame until normal.
        let _ = self.call(
            "Browser.setWindowBounds",
            json!({ "windowId": id, "bounds": { "windowState": "normal" } }),
            None,
        );
        self.call(
            "Browser.setWindowBounds",
            json!({ "windowId": id, "bounds": {
                "left": frame.x.round() as i64,
                "top": frame.y.round() as i64,
                "width": frame.width.round() as i64,
                "height": frame.height.round() as i64,
            } }),
            None,
        )
        .is_ok()
    }

    /// Close a page target, such as a tab opened in the wrong window.
    pub fn close_target(&self, target: &str) -> bool {
        self.call("Target.closeTarget", json!({ "targetId": target }), None)
            .is_ok()
    }

    /// The browser window ids of this process's pages.
    fn page_windows(&self) -> Vec<i64> {
        let Ok(targets) = self.call("Target.getTargets", json!({}), None) else {
            return Vec::new();
        };
        let mut windows: Vec<i64> = targets["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str())
            .filter_map(|id| self.window_of(id).map(|(id, _)| id))
            .collect();
        windows.sort_unstable();
        windows.dedup();
        windows
    }

    /// The session attached to a target, attaching once.
    fn session(&self, target: &str) -> Result<String, String> {
        if let Some(s) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(target)
        {
            return Ok(s.clone());
        }
        let attached = self.call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
        )?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or("no session")?
            .to_string();
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(target.to_string(), session.clone());
        Ok(session)
    }

    /// The data of an HTML5 drag the page started in `session`, taken.
    fn intercepted(&self, session: &str) -> Option<Value> {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let i = events.iter().position(|e| {
            e["method"] == "Input.dragIntercepted" && e["sessionId"].as_str() == Some(session)
        })?;
        events.remove(i).map(|e| e["params"]["data"].clone())
    }

    fn forget_intercepted(&self, session: &str) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|e| {
                !(e["method"] == "Input.dragIntercepted"
                    && e["sessionId"].as_str() == Some(session))
            });
    }
}

// MARK: Pages

/// What a page says of where it is.
const VIEW: &str = "JSON.stringify([screenX, screenY, outerWidth, outerHeight, innerWidth, \
                    innerHeight, devicePixelRatio, document.visibilityState])";

fn page_view(value: &Value) -> Option<PageView> {
    let text = value["result"]["value"].as_str()?;
    let v: Vec<Value> = serde_json::from_str(text).ok()?;
    let n = |i: usize| v.get(i).and_then(Value::as_f64);
    Some(PageView {
        screen: (n(0)?, n(1)?),
        outer: (n(2)?, n(3)?),
        inner: (n(4)?, n(5)?),
        device_pixel_ratio: n(6)?,
        visible: v.get(7).and_then(Value::as_str) == Some("visible"),
    })
}

/// A page under a point, and where its viewport is.
pub struct Page {
    channel: Arc<Channel>,
    session: String,
    viewport: Viewport,
    window: WindowTarget,
}

impl Page {
    /// The window the page shows in.
    pub fn window(&self) -> WindowTarget {
        self.window
    }
}

/// The page of the app's that shows at a global point of `window`, whose
/// frame is `bounds`; `area` is the web area accessibility found there.
fn page_at(
    channel: Arc<Channel>,
    window: WindowTarget,
    bounds: Rect,
    area: Option<Rect>,
    at: (f64, f64),
) -> Result<Page, String> {
    let targets = channel.call("Target.getTargets", json!({}), None)?;
    let targets: Vec<String> = targets["targetInfos"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter(|t| matches!(t["type"].as_str(), Some("page" | "webview")))
                .filter_map(|t| t["targetId"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let sessions: Vec<String> = targets
        .iter()
        .filter_map(|t| channel.session(t).ok())
        .collect();
    // Ask every page at once: a busy background tab mustn't hold up the
    // one in front.
    let asked: Vec<_> = sessions
        .iter()
        .map(|s| {
            channel.send(
                "Runtime.evaluate",
                json!({ "expression": VIEW, "returnByValue": true, "silent": true }),
                Some(s),
            )
        })
        .collect();
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut views = Vec::new();
    let mut of = Vec::new();
    for (session, answer) in sessions.iter().zip(asked) {
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(Ok(value)) = answer.recv_timeout(left)
            && let Some(view) = page_view(&value)
        {
            views.push(view);
            of.push(session.clone());
        }
    }
    let backing = super::cg::active_displays()
        .into_iter()
        .find(|&d| super::cg::display_bounds(d).contains(at))
        .and_then(|d| {
            let points = super::cg::display_bounds(d).width;
            super::cg::pixel_size(d).map(|(w, _)| w as f64 / points)
        })
        .unwrap_or(1.0);
    let (chosen, viewport) = rules::choose(bounds, area, backing, &views).ok_or_else(|| {
        log::info!("devtools: no page of {views:?} fits window {bounds:?}, area {area:?}");
        "no page of the app's shows there".to_string()
    })?;
    if !viewport.rect(&views[chosen]).contains(at) {
        return Err("the point is outside the page".into());
    }
    log::debug!(
        "devtools: the page at {at:?} is {:?}, viewport {viewport:?}",
        views[chosen]
    );
    Ok(Page {
        channel,
        session: of[chosen].clone(),
        viewport,
        window,
    })
}

/// How a drag that starts at a point goes out, and what it needs.
pub enum Route {
    Pipe(Page),
    Posted,
    /// A web page with no pipe: the app's name.
    Refused(String),
    /// A pipe, but no page of the app's matches the point.
    Unmapped(String),
}

/// How a drag starting at the global point `at` goes out: through the
/// window's app's pipe when it is in a web page and the app has one, or
/// nowhere when it is in a Chromium or WebKit page and the app has none.
/// Blocks on the app; call it off the main thread.
pub fn route(window: Option<(WindowTarget, Rect)>, at: (f64, f64)) -> Route {
    let Some((target, bounds)) = window else {
        return Route::Posted;
    };
    let area = super::ax::web_area_at(target, at);
    let piped = channel(target.pid, Duration::from_secs(3));
    let engine = area.and_then(|_| engine(target.pid));
    log::debug!(
        "devtools: drag at {at:?} in {target:?}: web area {area:?}, {engine:?}, pipe {}",
        piped.is_some()
    );
    // Without accessibility's page tree, a piped app's page is still where
    // the app says it is.
    if area.is_none()
        && let Some(channel) = piped.clone()
        && let Ok(page) = page_at(channel, target, bounds, None, at)
    {
        return Route::Pipe(page);
    }
    match rules::drag_route(area.is_some(), engine, piped.is_some()) {
        DragRoute::Posted => Route::Posted,
        DragRoute::Refused => {
            Route::Refused(super::app::name(target.pid).unwrap_or_else(|| "the app".into()))
        }
        DragRoute::Pipe => match piped.map(|c| page_at(c, target, bounds, area, at)) {
            Some(Ok(page)) => Route::Pipe(page),
            Some(Err(e)) => Route::Unmapped(e),
            None => Route::Posted,
        },
    }
}

// MARK: Drags

/// A drag held through the pipe, from the press to the release.
pub struct Drag {
    page: Page,
    /// What the page put in an HTML5 drag it started, once it has.
    data: Option<Value>,
    at: (f64, f64),
    modifiers: i64,
}

impl Drag {
    fn mouse(&self, kind: &str, at: (f64, f64), buttons: i64) -> Result<(), String> {
        let (x, y) = self.page.viewport.to_page(at);
        let mut params = json!({
            "type": kind, "x": x, "y": y, "modifiers": self.modifiers,
            "button": "left", "buttons": buttons, "pointerType": "mouse",
        });
        if kind != "mouseMoved" {
            params["clickCount"] = json!(1);
        }
        self.page
            .channel
            .call("Input.dispatchMouseEvent", params, Some(&self.page.session))
            .map(|_| ())
    }

    fn drag_event(&self, kind: &str, at: (f64, f64), data: &Value) -> Result<(), String> {
        let (x, y) = self.page.viewport.to_page(at);
        self.page
            .channel
            .call(
                "Input.dispatchDragEvent",
                json!({ "type": kind, "x": x, "y": y, "data": data, "modifiers": self.modifiers }),
                Some(&self.page.session),
            )
            .map(|_| ())
    }

    /// Press the left button at `at` in the page. An HTML5 drag the page
    /// starts from here is caught in the browser, not handed to macOS.
    pub fn press(page: Page, at: (f64, f64), modifiers: i64) -> Result<Drag, String> {
        page.channel.forget_intercepted(&page.session);
        page.channel.call(
            "Input.setInterceptDrags",
            json!({ "enabled": true }),
            Some(&page.session),
        )?;
        let drag = Drag {
            page,
            data: None,
            at,
            modifiers,
        };
        drag.mouse("mousePressed", at, 1)?;
        Ok(drag)
    }

    /// Move with the button held; once the page has started an HTML5 drag,
    /// the drag moves over its targets.
    pub fn move_to(&mut self, at: (f64, f64)) -> Result<(), String> {
        self.at = at;
        if self.data.is_none() {
            self.mouse("mouseMoved", at, 1)?;
            if let Some(data) = self.page.channel.intercepted(&self.page.session) {
                log::debug!("devtools: the page started an HTML5 drag");
                self.drag_event("dragEnter", at, &data)?;
                self.data = Some(data);
            }
            return Ok(());
        }
        let data = self.data.clone().unwrap_or_default();
        self.drag_event("dragOver", at, &data)
    }

    /// Release at `at`: an HTML5 drag drops there.
    pub fn release(mut self, at: (f64, f64)) -> Result<(), String> {
        if self.data.is_none() {
            // A drag the page started on the last move.
            std::thread::sleep(Duration::from_millis(30));
            if let Some(data) = self.page.channel.intercepted(&self.page.session) {
                self.drag_event("dragEnter", at, &data)?;
                self.data = Some(data);
            }
        }
        let result = match self.data.take() {
            Some(data) => self
                .drag_event("dragOver", at, &data)
                .and_then(|_| self.drag_event("drop", at, &data)),
            None => Ok(()),
        };
        let released = self.mouse("mouseReleased", at, 0);
        self.stop_intercepting();
        result.and(released)
    }

    /// Let go where the drag is, dropping nothing: the user took over.
    pub fn cancel(mut self) {
        if let Some(data) = self.data.take() {
            let _ = self.drag_event("dragCancel", self.at, &data);
        }
        let _ = self.mouse("mouseReleased", self.at, 0);
        self.stop_intercepting();
    }

    pub fn at(&self) -> (f64, f64) {
        self.at
    }

    pub fn window(&self) -> u32 {
        self.page.window.window_id
    }

    fn stop_intercepting(&self) {
        let _ = self.page.channel.call(
            "Input.setInterceptDrags",
            json!({ "enabled": false }),
            Some(&self.page.session),
        );
    }
}

// MARK: Selects

/// What became of choosing a web page's `<select>` option through the pipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectChoice {
    /// No pipe, or the point is in a frame from another site, which the
    /// page's script can't look into: choose another way.
    Unpiped,
    /// The script reached the page or frame at the point and found no
    /// select there, as for a button that opens a menu.
    NotSelect,
    /// The select at the point shows something other than accessibility
    /// said of the element: it may be another one. Nothing changed.
    Elsewhere,
    /// The option is chosen, with one input and one change event.
    Chosen,
    /// The select already shows the option: nothing was sent.
    Already,
    /// No option has the title; the select shows `shown`. `near` is an
    /// option whose title differs only in case.
    Missing { shown: String, near: Option<String> },
    /// The select, its fieldset, the option or its optgroup is disabled:
    /// nothing changed.
    Disabled,
    /// The page didn't answer, so the choice may or may not have run.
    Unknown(String),
    /// The user took over first: nothing was sent.
    Stopped,
}

/// Finds the select under the point, in the page or a frame of the same
/// site, and chooses the option titled as the agent asked, or says why
/// not. One script run: no option in between is chosen, and the page gets
/// one input event and one change event, as from the user's choice in the
/// open menu. A page script sees them as untrusted (`isTrusted` false), as
/// for any script's events.
const CHOOSE: &str = r#"((x, y, title, expected) => {
  let doc = document, hit = null;
  for (let depth = 0; depth < 8; depth++) {
    hit = doc.elementFromPoint(x, y);
    if (!hit || !(hit instanceof doc.defaultView.HTMLIFrameElement)) break;
    let inner = null;
    try { inner = hit.contentDocument; } catch (e) { inner = null; }
    // A frame from another site: only accessibility can tell its select.
    if (!inner) return JSON.stringify({ kind: 'unreachable' });
    const box = hit.getBoundingClientRect();
    x -= box.left + hit.clientLeft;
    y -= box.top + hit.clientTop;
    doc = inner;
  }
  const select = hit && hit.closest ? hit.closest('select') : null;
  if (!select || select.multiple) return JSON.stringify({ kind: 'none' });
  const options = Array.from(select.options);
  const shown = () => select.selectedIndex >= 0 ? options[select.selectedIndex].text : '';
  if (shown() !== expected) return JSON.stringify({ kind: 'elsewhere' });
  const i = options.findIndex(o => o.text === title || o.label === title);
  if (i < 0) {
    const near = options.find(o => o.text.toLowerCase() === title.toLowerCase());
    return JSON.stringify({ kind: 'missing', shown: shown(), near: near ? near.text : null });
  }
  if (select.selectedIndex === i) return JSON.stringify({ kind: 'already' });
  if (select.matches(':disabled') || options[i].matches(':disabled')) {
    return JSON.stringify({ kind: 'disabled' });
  }
  select.selectedIndex = i;
  select.dispatchEvent(new Event('input', { bubbles: true }));
  select.dispatchEvent(new Event('change', { bubbles: true }));
  return JSON.stringify({ kind: 'chosen' });
})"#;

/// Choose the option titled `title` of the select at the global point `at`
/// of `window`, whose frame is `bounds`, through the app's pipe: nothing
/// reaches WindowServer, and no menu opens. `shown` is what accessibility
/// says the element shows: a select at the point that shows something
/// else may be another one, and is left alone. `stopped` is asked just
/// before the page's script runs. Blocks on the app; call it off the main
/// thread.
pub fn choose_option(
    window: WindowTarget,
    bounds: Rect,
    at: (f64, f64),
    title: &str,
    shown: &str,
    stopped: impl Fn() -> bool,
) -> SelectChoice {
    let Some(area) = super::ax::web_area_at(window, at) else {
        return SelectChoice::Unpiped;
    };
    let Some(channel) = channel(window.pid, Duration::from_millis(500)) else {
        return SelectChoice::Unpiped;
    };
    let Ok(page) = page_at(channel, window, bounds, Some(area), at) else {
        return SelectChoice::Unpiped;
    };
    let (x, y) = page.viewport.to_page(at);
    let expression = format!("{CHOOSE}({x}, {y}, {}, {})", json!(title), json!(shown));
    if stopped() {
        return SelectChoice::Stopped;
    }
    let answer = page.channel.call(
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true, "silent": true }),
        Some(&page.session),
    );
    let value = match answer {
        Ok(value) => value,
        Err(e) => return SelectChoice::Unknown(e),
    };
    let Some(text) = value["result"]["value"].as_str() else {
        return SelectChoice::Unknown("the page's script returned nothing".into());
    };
    let answer: Value = serde_json::from_str(text).unwrap_or_default();
    let string = |key: &str| answer[key].as_str().map(String::from);
    match answer["kind"].as_str() {
        Some("chosen") => SelectChoice::Chosen,
        Some("already") => SelectChoice::Already,
        Some("disabled") => SelectChoice::Disabled,
        Some("elsewhere") => SelectChoice::Elsewhere,
        Some("none") => SelectChoice::NotSelect,
        Some("missing") => SelectChoice::Missing {
            shown: string("shown").unwrap_or_default(),
            near: string("near"),
        },
        _ => SelectChoice::Unpiped,
    }
}

/// What an agent hears when its drag lands in a web page of an app that
/// has no pipe.
pub fn refused_message(app: &str) -> String {
    format!(
        "{app} shows a web page there, and a drag in a web page needs a DevTools pipe into the app, which {app} doesn't have: launch_app didn't start it with one. Agent Screen won't press the user's own mouse button for it, so nothing was sent. To drag here, start a new instance with launch_app (for a browser, with arguments such as --user-data-dir=/tmp/profile, as a browser's default profile takes no pipe). A fresh profile has separate sign-ins and does not keep the user's existing login session. If that session is required, keep this window and call request_help for the user to do the drag; starting another profile will not add a pipe to this window. To select text, click and then shift-click, or use the keyboard."
    )
}

#[cfg(test)]
mod refusal_tests {
    #[test]
    fn an_unpiped_page_has_no_promise_of_existing_login_or_a_new_drag_capability() {
        let message = super::refused_message("Dia");
        assert!(message.contains("does not keep the user's existing login session"));
        assert!(message.contains("keep this window and call request_help"));
        assert!(message.contains("will not add a pipe to this window"));
        assert!(message.contains("won't press the user's own mouse button"));
    }
}
