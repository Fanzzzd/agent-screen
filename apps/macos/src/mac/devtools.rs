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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

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
}

fn channels() -> std::sync::MutexGuard<'static, HashMap<i32, Arc<Channel>>> {
    static CHANNELS: std::sync::LazyLock<Mutex<HashMap<i32, Arc<Channel>>>> =
        std::sync::LazyLock::new(Default::default);
    CHANNELS.lock().unwrap_or_else(|e| e.into_inner())
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
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        events.push_back(message);
        while events.len() > 64 {
            events.pop_front();
        }
    }

    fn send(&self, method: &str, params: Value, session: Option<&str>) -> mpsc::Receiver<Answer> {
        let (tx, rx) = mpsc::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
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
