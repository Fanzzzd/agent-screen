//! Private POSIX DevTools pipe (`--remote-debugging-pipe` on fds 3 and 4) for
//! Chromium and Electron apps launched by Agent Screen (ADR 0010).
//!
//! Uses `agent_screen_model::devtools` for NUL-delimited framing, viewport
//! selection, modifier bits, and drag path interpolation, with Wayland
//! `xdg_toplevel` `(screenX=0, screenY=0)` coordinate compensation.

use agent_screen_model::devtools::{self as rules, PageView, Viewport};
use agent_screen_model::keychord::Modifiers;
use agent_screen_model::model::{DisplayId, Rect, WindowTarget};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};

const HELLO_WAIT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Starting,
    Ready,
    Dead,
}

type Answer = Result<Value, String>;

pub struct PipeSetup {
    to_app_read: RawFd,
    to_app_write: RawFd,
    from_app_read: RawFd,
    from_app_write: RawFd,
}

impl PipeSetup {
    pub fn create() -> std::io::Result<Self> {
        let to_app = raw_cloexec_pipe()?;
        let from_app = match raw_cloexec_pipe() {
            Ok(p) => p,
            Err(e) => {
                unsafe {
                    libc::close(to_app.0);
                    libc::close(to_app.1);
                }
                return Err(e);
            }
        };
        Ok(Self {
            to_app_read: to_app.0,
            to_app_write: to_app.1,
            from_app_read: from_app.0,
            from_app_write: from_app.1,
        })
    }

    pub fn child_fds(&self) -> (RawFd, RawFd) {
        (self.to_app_read, self.from_app_write)
    }

    pub fn abort(self) {
        unsafe {
            libc::close(self.to_app_read);
            libc::close(self.to_app_write);
            libc::close(self.from_app_read);
            libc::close(self.from_app_write);
        }
    }
}

/// Bind fd 3 (read from daemon) and fd 4 (write to daemon) inside
/// `Command::pre_exec` with `FD_CLOEXEC` cleared so Chromium inherits them.
///
/// # Safety
/// Must only be called in the forked child before `exec` (`pre_exec`).
pub unsafe fn pre_exec_dup_fds(to_app_read: RawFd, from_app_write: RawFd) -> std::io::Result<()> {
    unsafe {
        let mut src4 = from_app_write;
        if src4 == 3 {
            src4 = libc::dup(src4);
            if src4 < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        dup_clear_cloexec(to_app_read, 3)?;
        dup_clear_cloexec(src4, 4)?;
        if src4 != from_app_write {
            libc::close(src4);
        }
        Ok(())
    }
}

unsafe fn dup_clear_cloexec(src: RawFd, dst: RawFd) -> std::io::Result<()> {
    unsafe {
        if src == dst {
            let flags = libc::fcntl(dst, libc::F_GETFD);
            if flags < 0 || libc::fcntl(dst, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        } else if libc::dup2(src, dst) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

fn raw_cloexec_pipe() -> std::io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for &fd in &fds {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            let err = std::io::Error::last_os_error();
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            return Err(err);
        }
    }
    Ok((fds[0], fds[1]))
}

pub struct Channel {
    display_id: DisplayId,
    pid: i32,
    writer: Mutex<std::fs::File>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, mpsc::Sender<Answer>>>,
    events: Mutex<VecDeque<Value>>,
    state: Mutex<State>,
    changed: Condvar,
    sessions: Mutex<HashMap<String, String>>,
}

type ChannelMap = HashMap<(DisplayId, i32), Arc<Channel>>;

static CHANNELS: LazyLock<Mutex<ChannelMap>> = LazyLock::new(Default::default);

fn channels() -> std::sync::MutexGuard<'static, ChannelMap> {
    CHANNELS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn register_spawned(display_id: DisplayId, pid: i32, setup: PipeSetup) {
    unsafe {
        libc::close(setup.to_app_read);
        libc::close(setup.from_app_write);
    }
    let writer = unsafe { std::fs::File::from_raw_fd(setup.to_app_write) };
    let reader = unsafe { std::fs::File::from_raw_fd(setup.from_app_read) };

    let channel = Arc::new(Channel {
        display_id,
        pid,
        writer: Mutex::new(writer),
        next_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        events: Mutex::new(VecDeque::new()),
        state: Mutex::new(State::Starting),
        changed: Condvar::new(),
        sessions: Mutex::new(HashMap::new()),
    });
    channels().insert((display_id, pid), channel.clone());

    let reader_ch = channel.clone();
    let _ = std::thread::Builder::new()
        .name(format!("as-cdp-read-{pid}"))
        .spawn(move || reader_ch.read_loop(reader));

    let _ = std::thread::Builder::new()
        .name(format!("as-cdp-hello-{pid}"))
        .spawn(move || {
            let answer = channel.send("Browser.getVersion", json!({}), None);
            let st = match answer.recv_timeout(HELLO_WAIT) {
                Ok(Ok(_)) => State::Ready,
                _ => State::Dead,
            };
            channel.set_state(st);
            if st == State::Dead {
                channels().remove(&(display_id, pid));
            }
        });
}

pub fn remove_display_channels(display_id: DisplayId) {
    channels().retain(|&(d, _), _| d != display_id);
}

pub fn channel_for_window(
    display_id: DisplayId,
    window_pid: i32,
    wait: Duration,
) -> Option<Arc<Channel>> {
    let candidate = {
        let map = channels();
        if let Some(ch) = map.get(&(display_id, window_pid)) {
            Some(ch.clone())
        } else {
            let mut found = None;
            let mut cur = window_pid;
            for _ in 0..8 {
                let Some(ppid) = parent_pid(cur) else {
                    break;
                };
                if ppid <= 1 {
                    break;
                }
                if let Some(ch) = map.get(&(display_id, ppid)) {
                    found = Some(ch.clone());
                    break;
                }
                cur = ppid;
            }
            found
        }
    }?;

    let started = Instant::now();
    let ready = {
        let mut state = candidate.state.lock().unwrap_or_else(|e| e.into_inner());
        while *state == State::Starting {
            let left = wait.checked_sub(started.elapsed())?;
            state = candidate
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *state == State::Ready
    };
    ready.then_some(candidate)
}

fn parent_pid(pid: i32) -> Option<i32> {
    if pid <= 1 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rfind(')')?;
    let fields: Vec<&str> = stat[after_comm + 1..].split_whitespace().collect();
    fields.get(1)?.parse::<i32>().ok()
}

impl Channel {
    fn set_state(&self, next: State) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *s != State::Dead {
            *s = next;
        }
        self.changed.notify_all();
    }

    fn read_loop(&self, mut reader: std::fs::File) {
        let mut buffer = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            buffer.extend_from_slice(&chunk[..n]);
            for raw in rules::take_messages(&mut buffer) {
                if let Ok(msg) = serde_json::from_slice::<Value>(&raw) {
                    self.take_message(msg);
                }
            }
        }
        self.set_state(State::Dead);
        for (_, waiting) in self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
        {
            let _ = waiting.send(Err("the app closed its DevTools pipe".into()));
        }
        let mut map = channels();
        if map
            .get(&(self.display_id, self.pid))
            .is_some_and(|c| std::ptr::eq(&**c, self))
        {
            map.remove(&(self.display_id, self.pid));
        }
    }

    fn take_message(&self, msg: Value) {
        if let Some(id) = msg["id"].as_u64() {
            if let Some(waiting) = self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
            {
                let ans = match msg.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("CDP error").to_string()),
                    None => Ok(msg["result"].clone()),
                };
                let _ = waiting.send(ans);
            }
            return;
        }
        if msg["method"] == "Target.detachedFromTarget"
            && let Some(sess) = msg["params"]["sessionId"].as_str()
        {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, s| s != sess);
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        events.push_back(msg);
        while events.len() > 64 {
            events.pop_front();
        }
    }

    fn send(&self, method: &str, params: Value, session: Option<&str>) -> mpsc::Receiver<Answer> {
        let (tx, rx) = mpsc::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx.clone());
        let mut bytes = serde_json::to_vec(&msg).unwrap_or_default();
        bytes.push(0);
        if self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&bytes)
            .is_err()
        {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            let _ = tx.send(Err("the app's DevTools pipe is closed".into()));
        }
        rx
    }

    fn call(&self, method: &str, params: Value, session: Option<&str>) -> Answer {
        self.send(method, params, session)
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|_| Err(format!("{method} got no answer")))
    }

    fn session(&self, target: &str) -> Result<String, String> {
        if let Some(existing) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(target)
        {
            return Ok(existing.clone());
        }
        let attached = self.call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
        )?;
        let s = attached["sessionId"]
            .as_str()
            .ok_or("missing sessionId")?
            .to_string();
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(target.to_string(), s.clone());
        Ok(s)
    }

    fn intercepted(&self, session: &str) -> Option<Value> {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let idx = events.iter().position(|e| {
            e["method"] == "Input.dragIntercepted" && e["sessionId"].as_str() == Some(session)
        })?;
        events.remove(idx).map(|e| e["params"]["data"].clone())
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

const VIEW_JS: &str = "JSON.stringify([screenX, screenY, outerWidth, outerHeight, innerWidth, \
                       innerHeight, devicePixelRatio, document.visibilityState])";

fn parse_page_view(value: &Value) -> Option<PageView> {
    let text = value["result"]["value"].as_str()?;
    let arr: Vec<Value> = serde_json::from_str(text).ok()?;
    let n = |i: usize| arr.get(i).and_then(Value::as_f64);
    Some(PageView {
        screen: (n(0)?, n(1)?),
        outer: (n(2)?, n(3)?),
        inner: (n(4)?, n(5)?),
        device_pixel_ratio: n(6)?,
        visible: arr.get(7).and_then(Value::as_str) == Some("visible"),
    })
}

struct Page {
    channel: Arc<Channel>,
    session: String,
    viewport: Viewport,
}

/// Choose a page viewport, compensating for Wayland `xdg_toplevel` reporting
/// `(screenX=0, screenY=0)` regardless of Sway's floating window position.
pub fn choose_wayland_aware(
    bounds: Rect,
    area: Option<Rect>,
    backing: f64,
    views: &[PageView],
) -> Option<(usize, Viewport)> {
    if let Some(found) = rules::choose(bounds, area, backing, views) {
        return Some(found);
    }
    let local_bounds = Rect::new(0.0, 0.0, bounds.width, bounds.height);
    let local_area = area.map(|a| Rect::new(a.x - bounds.x, a.y - bounds.y, a.width, a.height));
    if let Some((idx, mut vp)) = rules::choose(local_bounds, local_area, backing, views) {
        vp.origin.0 += bounds.x;
        vp.origin.1 += bounds.y;
        return Some((idx, vp));
    }
    let visible: Vec<usize> = (0..views.len()).filter(|&i| views[i].visible).collect();
    if visible.len() == 1 {
        let idx = visible[0];
        let p = &views[idx];
        let zoom = match area {
            Some(a) if p.inner.0 > 0.0 => a.width / p.inner.0,
            _ if backing > 0.0 && p.device_pixel_ratio > 0.0 => p.device_pixel_ratio / backing,
            _ => 1.0,
        };
        let origin = match area {
            Some(a) => (a.x, a.y),
            None => (bounds.x, bounds.max_y() - p.inner.1 * zoom),
        };
        return Some((idx, Viewport { origin, zoom }));
    }
    None
}

fn page_at(
    channel: Arc<Channel>,
    bounds: Rect,
    area: Option<Rect>,
    backing: f64,
    at: (f64, f64),
) -> Result<Page, String> {
    let targets = channel.call("Target.getTargets", json!({}), None)?;
    let target_ids: Vec<String> = targets["targetInfos"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|t| matches!(t["type"].as_str(), Some("page" | "webview")))
                .filter_map(|t| t["targetId"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let sessions: Vec<String> = target_ids
        .iter()
        .filter_map(|t| channel.session(t).ok())
        .collect();
    let asked: Vec<_> = sessions
        .iter()
        .map(|s| {
            channel.send(
                "Runtime.evaluate",
                json!({ "expression": VIEW_JS, "returnByValue": true, "silent": true }),
                Some(s),
            )
        })
        .collect();

    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut views = Vec::new();
    let mut of = Vec::new();
    for (s, rx) in sessions.iter().zip(asked) {
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(Ok(val)) = rx.recv_timeout(left)
            && let Some(v) = parse_page_view(&val)
        {
            views.push(v);
            of.push(s.clone());
        }
    }

    let (chosen, viewport) = choose_wayland_aware(bounds, area, backing, &views)
        .ok_or_else(|| "no page of the app matches that window".to_string())?;
    if !viewport.rect(&views[chosen]).contains(at) {
        return Err("point is outside the web viewport".into());
    }
    Ok(Page {
        channel,
        session: of[chosen].clone(),
        viewport,
    })
}

/// Perform a pointer or HTML5 drag over the app's DevTools pipe if `from`
/// lands inside a piped web viewport. Returns `Ok(true)` when handled over CDP.
#[allow(clippy::too_many_arguments)]
pub fn try_drag_via_pipe(
    display_id: DisplayId,
    window: WindowTarget,
    bounds: Rect,
    web_area: Option<Rect>,
    backing: f64,
    from: (f64, f64),
    to: (f64, f64),
    modifiers: Modifiers,
) -> Result<bool, String> {
    let Some(channel) = channel_for_window(display_id, window.pid, Duration::from_secs(2)) else {
        return Ok(false);
    };
    let Ok(page) = page_at(channel, bounds, web_area, backing, from) else {
        return Ok(false);
    };

    let mod_bits = rules::modifier_bits(
        modifiers.alt,
        modifiers.control,
        modifiers.command,
        modifiers.shift,
    );
    let mut drag = Drag::press(page, from, mod_bits)?;
    for pt in rules::path(from, to, 16) {
        drag.move_to(pt)?;
        std::thread::sleep(Duration::from_millis(10));
    }
    drag.release(to)?;
    Ok(true)
}

struct Drag {
    page: Page,
    data: Option<Value>,
    modifiers: i64,
}

impl Drag {
    fn mouse(&self, kind: &str, at: (f64, f64), buttons: i64) -> Result<(), String> {
        let (x, y) = self.page.viewport.to_page(at);
        let mut params = json!({
            "type": kind,
            "x": x,
            "y": y,
            "modifiers": self.modifiers,
            "button": "left",
            "buttons": buttons,
            "pointerType": "mouse",
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
                json!({
                    "type": kind,
                    "x": x,
                    "y": y,
                    "data": data,
                    "modifiers": self.modifiers,
                }),
                Some(&self.page.session),
            )
            .map(|_| ())
    }

    fn press(page: Page, at: (f64, f64), modifiers: i64) -> Result<Self, String> {
        page.channel.forget_intercepted(&page.session);
        page.channel.call(
            "Input.setInterceptDrags",
            json!({ "enabled": true }),
            Some(&page.session),
        )?;
        let d = Self {
            page,
            data: None,
            modifiers,
        };
        d.mouse("mousePressed", at, 1)?;
        Ok(d)
    }

    fn move_to(&mut self, at: (f64, f64)) -> Result<(), String> {
        if self.data.is_none() {
            self.mouse("mouseMoved", at, 1)?;
            if let Some(data) = self.page.channel.intercepted(&self.page.session) {
                self.drag_event("dragEnter", at, &data)?;
                self.data = Some(data);
            }
            return Ok(());
        }
        let data = self.data.clone().unwrap_or_default();
        self.drag_event("dragOver", at, &data)
    }

    fn release(mut self, at: (f64, f64)) -> Result<(), String> {
        if self.data.is_none() {
            std::thread::sleep(Duration::from_millis(30));
            if let Some(data) = self.page.channel.intercepted(&self.page.session) {
                self.drag_event("dragEnter", at, &data)?;
                self.data = Some(data);
            }
        }
        let drag_res = match self.data.take() {
            Some(data) => self
                .drag_event("dragOver", at, &data)
                .and_then(|_| self.drag_event("drop", at, &data)),
            None => Ok(()),
        };
        let rel = self.mouse("mouseReleased", at, 0);
        let _ = self.page.channel.call(
            "Input.setInterceptDrags",
            json!({ "enabled": false }),
            Some(&self.page.session),
        );
        drag_res.and(rel)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectChoice {
    Unpiped,
    NotSelect,
    Chosen,
    Already,
    Disabled,
    Missing { shown: String, near: Option<String> },
    Failed(String),
}

const CHOOSE_JS: &str = r#"((x, y, title) => {
  let doc = document, hit = null;
  for (let depth = 0; depth < 8; depth++) {
    hit = doc.elementFromPoint(x, y);
    if (!hit || !(hit instanceof doc.defaultView.HTMLIFrameElement)) break;
    let inner = null;
    try { inner = hit.contentDocument; } catch (e) { inner = null; }
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
  const i = options.findIndex(o => o.text === title || o.label === title || o.value === title);
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

pub fn choose_select_option(
    display_id: DisplayId,
    window: WindowTarget,
    bounds: Rect,
    web_area: Option<Rect>,
    backing: f64,
    at: (f64, f64),
    title: &str,
) -> SelectChoice {
    let Some(channel) = channel_for_window(display_id, window.pid, Duration::from_millis(800))
    else {
        return SelectChoice::Unpiped;
    };
    let Ok(page) = page_at(channel, bounds, web_area, backing, at) else {
        return SelectChoice::Unpiped;
    };
    let (x, y) = page.viewport.to_page(at);
    let expr = format!("{CHOOSE_JS}({x}, {y}, {})", json!(title));
    let ans = match page.channel.call(
        "Runtime.evaluate",
        json!({ "expression": expr, "returnByValue": true, "silent": true }),
        Some(&page.session),
    ) {
        Ok(v) => v,
        Err(e) => return SelectChoice::Failed(e),
    };
    let Some(text) = ans["result"]["value"].as_str() else {
        return SelectChoice::Failed("script returned no value".into());
    };
    let parsed: Value = serde_json::from_str(text).unwrap_or_default();
    match parsed["kind"].as_str() {
        Some("chosen") => SelectChoice::Chosen,
        Some("already") => SelectChoice::Already,
        Some("disabled") => SelectChoice::Disabled,
        Some("none") => SelectChoice::NotSelect,
        Some("missing") => SelectChoice::Missing {
            shown: parsed["shown"].as_str().unwrap_or_default().to_string(),
            near: parsed["near"].as_str().map(String::from),
        },
        _ => SelectChoice::Unpiped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_zero_screen_origin_shifts_viewport_into_sway_window_bounds() {
        let bounds = Rect::new(40.0, 60.0, 900.0, 640.0);
        let web_area = Some(Rect::new(40.0, 140.0, 900.0, 560.0));
        let views = [PageView {
            screen: (0.0, 0.0),
            outer: (900.0, 640.0),
            inner: (900.0, 560.0),
            device_pixel_ratio: 1.0,
            visible: true,
        }];
        let (chosen, vp) = choose_wayland_aware(bounds, web_area, 1.0, &views).unwrap();
        assert_eq!(chosen, 0);
        assert_eq!(vp.origin, (40.0, 140.0));
        assert_eq!(vp.to_page((240.0, 340.0)), (200.0, 200.0));
    }
}
