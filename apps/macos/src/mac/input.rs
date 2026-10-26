//! Input for windows on an Agent Display, from the Viewer or an Agent
//! Session. Events go straight to the owning process through SkyLight, so
//! the real cursor stays where the user left it and the user's frontmost app
//! keeps its place in the window stack (ADR 0001).
//!
//! Each display has one worker thread. The focus handshake needs short
//! sleeps, and window list lookups stay off the main thread.

use super::cg;
use super::windows::Grab;
use crate::keychord::{KeyChord, Modifiers};
use crate::model::{Rect, WindowTarget};
use futures::channel::oneshot;
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventType,
    CGMouseButton, CGScrollEventUnit, CGWindowLevelForKey, CGWindowLevelKey,
};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn SSPostEventToPid(pid: i32, event: &CGEvent, authenticate: bool) -> bool;
    fn SSEventSetWindowLocation(event: &CGEvent, point: CGPoint) -> bool;
    fn SSEventSetIntegerField(event: &CGEvent, field: u32, value: i64) -> bool;
    fn SSFocusWithoutRaise(pid: i32, window: u32) -> bool;
    fn SSFrontPid() -> i32;
    fn SSMakeSelfFront(window: u32) -> bool;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(name: &str) -> Option<Button> {
        match name {
            "left" => Some(Button::Left),
            "right" => Some(Button::Right),
            "middle" => Some(Button::Middle),
            _ => None,
        }
    }

    fn cg(self) -> CGMouseButton {
        match self {
            Button::Left => CGMouseButton::Left,
            Button::Right => CGMouseButton::Right,
            Button::Middle => CGMouseButton::Center,
        }
    }

    fn down(self) -> CGEventType {
        match self {
            Button::Left => CGEventType::LeftMouseDown,
            Button::Right => CGEventType::RightMouseDown,
            Button::Middle => CGEventType::OtherMouseDown,
        }
    }

    fn up(self) -> CGEventType {
        match self {
            Button::Left => CGEventType::LeftMouseUp,
            Button::Right => CGEventType::RightMouseUp,
            Button::Middle => CGEventType::OtherMouseUp,
        }
    }

    fn dragged(self) -> CGEventType {
        match self {
            Button::Left => CGEventType::LeftMouseDragged,
            Button::Right => CGEventType::RightMouseDragged,
            Button::Middle => CGEventType::OtherMouseDragged,
        }
    }
}

pub fn flags(m: Modifiers) -> CGEventFlags {
    let mut flags = CGEventFlags::empty();
    if m.control {
        flags |= CGEventFlags::MaskControl;
    }
    if m.shift {
        flags |= CGEventFlags::MaskShift;
    }
    if m.alt {
        flags |= CGEventFlags::MaskAlternate;
    }
    if m.command {
        flags |= CGEventFlags::MaskCommand;
    }
    if m.function {
        flags |= CGEventFlags::MaskSecondaryFn;
    }
    flags
}

fn wait_typeahead(stopped: &std::sync::atomic::AtomicBool) -> bool {
    let until = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < until {
        if stopped.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        thread::sleep(
            Duration::from_millis(10).min(until.saturating_duration_since(Instant::now())),
        );
    }
    !stopped.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod popup_wait_tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    #[test]
    fn takeover_interrupts_the_typeahead_wait() {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            flag.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        assert!(!wait_typeahead(&stopped));
        trigger.join().unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}

/// Check the explicit window, not whichever window this display last typed
/// into. PID-posted keys in a foreground app otherwise reach its key window.
fn popup_keys_allowed(target: WindowTarget) -> bool {
    let Some(front) = front() else {
        return false;
    };
    agent_screen_model::typing::keys_reach_target(
        front == target.pid,
        front != target.pid || super::ax::focused_window_id(target.pid) == Some(target.window_id),
    )
}

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// How much of an agent's drag or text went out.
#[derive(Clone, Debug, PartialEq)]
pub enum Sent {
    All,
    /// The user took the display over partway through a drag, which let
    /// go here.
    Stopped((f64, f64)),
    /// The same, partway through text: how many characters went in.
    Typed(usize),
    /// All the text went out, but the field shows this, without some of it.
    Dropped(String),
    /// No window to send it to.
    Nowhere,
}

/// The text as typed: a Windows line ending, \r\n, is one Return.
pub fn typed_text(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// One display's input queue. Cheap to clone.
#[derive(Clone)]
pub struct Forwarder {
    jobs: mpsc::Sender<Job>,
    /// Set while the user has taken the display over: an agent's typing or
    /// drag still running stops at its next step.
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Default for Forwarder {
    fn default() -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("agent-screen-input".into())
            .spawn(move || {
                let mut worker = Worker::new();
                while let Ok(job) = rx.recv() {
                    job(&mut worker);
                }
            })
            .expect("input thread");
        Forwarder {
            jobs,
            stopped: Default::default(),
        }
    }
}

impl Forwarder {
    /// Whether the user is in control and the agent's input stops.
    pub fn agent_stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Stop the agent's long input at its next step, while the user is in
    /// control.
    pub fn stop_agent(&self, stop: bool) {
        self.stopped
            .store(stop, std::sync::atomic::Ordering::Relaxed);
        // A takeover lets go of the agent's held button at once, so the
        // user's pointer reaches apps again.
        if stop {
            self.send(|w| w.release_agent_button());
        }
    }

    fn send(&self, job: impl FnOnce(&mut Worker) + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }

    /// Run `job` on the worker and wait for it to finish.
    async fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Worker) -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = oneshot::channel();
        self.send(move |worker| {
            let _ = tx.send(job(worker));
        });
        rx.await.ok()
    }

    // MARK: Live input from the Viewer, one event at a time. Flags are the
    // NSEvent's own modifier flags, whose bits match CGEventFlags, so Caps
    // Lock and the left/right device bits survive.

    pub fn mouse_down(&self, at: (f64, f64), button: Button, clicks: i64, flags: CGEventFlags) {
        self.send(move |w| {
            if !w.grab(at, button) {
                let target = w.mouse_down(at, button, clicks, flags);
                w.pressed = target.and_then(|t| held(t, at));
            }
        });
    }

    pub fn mouse_dragged(&self, to: (f64, f64), button: Button, flags: CGEventFlags) {
        self.send(move |w| match &w.grab {
            Some(grab) => {
                if grab.moved(to) || w.grab_moved {
                    w.grab_moved = true;
                    grab.drag_to(to);
                }
            }
            None => w.mouse_dragged(to, button, flags),
        });
    }

    pub fn mouse_up(&self, at: (f64, f64), button: Button, clicks: i64, flags: CGEventFlags) {
        self.send(move |w| match w.grab.take() {
            Some(grab) if std::mem::take(&mut w.grab_moved) => {
                grab.drag_to(at);
            }
            // Pressed and released in place: a click on the title bar,
            // such as a double click that zooms the window.
            Some(grab) => {
                w.mouse_down(grab.from(), button, clicks, flags);
                w.mouse_up(grab.from(), button, clicks, flags);
            }
            None => {
                w.mouse_up(at, button, clicks, flags);
                if let Some(press) = w.pressed.take() {
                    keep_on_display(press, at);
                }
            }
        });
    }

    pub fn mouse_moved(&self, to: (f64, f64), flags: CGEventFlags) {
        self.send(move |w| w.mouse_moved(to, flags));
    }

    pub fn scroll(
        &self,
        at: (f64, f64),
        delta: (f64, f64),
        continuous: bool,
        phase: i64,
        momentum: i64,
        flags: CGEventFlags,
    ) {
        self.send(move |w| w.scroll(at, delta, continuous, phase, momentum, flags));
    }

    pub fn key(&self, code: u16, down: bool, flags: CGEventFlags, within: Rect) {
        self.send(move |w| w.viewer_key(code, down, flags, within));
    }

    pub fn flags_changed(&self, code: u16, flags: CGEventFlags) {
        self.send(move |w| w.flags_changed(code, flags));
    }

    // MARK: Whole gestures for Agent Sessions. Each returns once every event
    // is posted, so a tool call can take a screenshot straight after.

    /// The window clicked, or None when no window is at `at`.
    pub async fn click(
        &self,
        at: (f64, f64),
        button: Button,
        count: i64,
        m: Modifiers,
    ) -> Option<WindowTarget> {
        self.run(move |w| {
            let count = count.max(1);
            let mut hit = None;
            for index in 1..=count {
                // A double click on Close takes the window away after the
                // first click; the first hit is the answer.
                let Some(down) = w.mouse_down(at, button, index, flags(m)) else {
                    break;
                };
                hit.get_or_insert(down);
                thread::sleep(Duration::from_millis(30));
                w.mouse_up(at, button, index, flags(m));
                if index < count {
                    thread::sleep(Duration::from_millis(80));
                }
            }
            hit
        })
        .await
        .flatten()
    }

    /// A press-move-release that apps take as a drag: a hold before moving,
    /// so the press doesn't read as a click, small steps, and a pause at the
    /// end so drop targets see the pointer arrive before the release.
    pub async fn drag(&self, from: (f64, f64), to: (f64, f64), m: Modifiers) -> Sent {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            if w.mouse_down(from, Button::Left, 1, flags(m)).is_none() {
                return Sent::Nowhere;
            }
            thread::sleep(Duration::from_millis(100));
            const STEPS: i32 = 20;
            for step in 1..=STEPS {
                let t = step as f64 / STEPS as f64;
                let at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                w.mouse_dragged(at, Button::Left, flags(m));
                // The user took over: let go where the pointer is, short of
                // the drop the agent was heading for. Checked after a move,
                // so the release is at least a step away from the source,
                // though still inside a large one.
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    w.mouse_up(at, Button::Left, 1, flags(m));
                    return Sent::Stopped(at);
                }
                thread::sleep(Duration::from_millis(16));
            }
            thread::sleep(Duration::from_millis(100));
            w.mouse_up(to, Button::Left, 1, flags(m));
            Sent::All
        })
        .await
        .unwrap_or(Sent::Nowhere)
    }

    /// Half a click, for drags an agent drives step by step. The window hit
    /// by the press, or None when there is nothing under the point.
    pub async fn button(&self, at: (f64, f64), down: bool, m: Modifiers) -> Option<WindowTarget> {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            // A takeover since the agent asked: its press must not land
            // after the release the takeover sent.
            if down && stopped.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            if down {
                let hit = w.mouse_down(at, Button::Left, 1, flags(m));
                if let Some(g) = &mut w.gesture {
                    g.agent = true;
                }
                hit
            } else {
                let target = w.gesture.as_ref().map(|g| g.target);
                w.mouse_up(at, Button::Left, 1, flags(m));
                target
            }
        })
        .await
        .flatten()
    }

    /// The agent moves the pointer: a drag while it holds the button down.
    pub async fn move_to(&self, to: (f64, f64)) {
        self.run(move |w| {
            if w.gesture.as_ref().is_some_and(|g| g.agent) {
                w.mouse_dragged(to, Button::Left, CGEventFlags::empty());
            } else {
                w.mouse_moved(to, CGEventFlags::empty());
            }
        })
        .await;
    }

    pub async fn wheel(&self, at: (f64, f64), delta: (f64, f64), m: Modifiers) {
        self.run(move |w| w.scroll(at, delta, false, 0, 0, flags(m)))
            .await;
    }

    /// Type text as Unicode, independent of the keyboard layout. One
    /// character per event: many apps read only the first character of a
    /// key event, so batching drops text. Newlines and tabs are the Return
    /// and Tab keys, so they submit and move between fields. The events
    /// carry no modifiers, whatever the user holds down meanwhile. False
    /// when no window on the display can take the text.
    pub async fn type_text(&self, text: String, within: Rect) -> Sent {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            let Some(_held) = w.focus_something(within) else {
                return Sent::Nowhere;
            };
            let Some(target) = w.focused else {
                return Sent::Nowhere;
            };
            let text = typed_text(&text);
            // A field that says what it holds is read back after, unless
            // Return or Tab moves on from it.
            let field = (!text.contains(['\n', '\r', '\t']))
                .then(|| super::ax::focused_text(target))
                .flatten()
                .map(|f| {
                    let before = super::ax::string_attribute(&f, "AXValue").unwrap_or_default();
                    (f, before)
                });
            for (typed, character) in text.chars().enumerate() {
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    return Sent::Typed(typed);
                }
                let code = match character {
                    '\n' | '\r' => Some(36),
                    '\t' => Some(48),
                    _ => None,
                };
                let mut units = [0u16; 2];
                let units = character.encode_utf16(&mut units);
                for down in [true, false] {
                    let Some(event) =
                        CGEvent::new_keyboard_event(w.source.as_deref(), code.unwrap_or(0), down)
                    else {
                        continue;
                    };
                    CGEvent::set_flags(Some(&event), CGEventFlags::empty());
                    if code.is_none() {
                        unsafe {
                            CGEvent::keyboard_set_unicode_string(
                                Some(&event),
                                units.len() as _,
                                units.as_ptr(),
                            )
                        };
                    }
                    deliver(&event, target.pid, true);
                }
                thread::sleep(Duration::from_millis(4));
            }
            // A page can drop keys that come faster than its script runs
            // (#50): the text is in the field, or the call says it isn't.
            // A busy page is still taking keys in while its text grows, and
            // Chromium's accessibility shows nothing new until the page's
            // script is idle: judge once the text has changed and then held
            // still for 1.5 s, within 5 s.
            if let Some((field, before)) = field {
                let mut shown = before.clone();
                let mut still = 0;
                for _ in 0..200 {
                    let now = super::ax::string_attribute(&field, "AXValue").unwrap_or_default();
                    if agent_screen_model::typing::landed(&text, &now) {
                        return Sent::All;
                    }
                    still = if now == shown { still + 1 } else { 0 };
                    shown = now;
                    if still >= 60 && shown != before {
                        return Sent::Dropped(shown);
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
            Sent::All
        })
        .await
        .unwrap_or(Sent::Nowhere)
    }

    /// Type-select a closed pop-up. Unlike general text entry, this targets
    /// exactly one window and never wakes it with a click or sends arrows.
    pub async fn choose_closed_popup(
        &self,
        target: WindowTarget,
        element: super::ax::Sendable,
        title: String,
        within: Rect,
    ) -> bool {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            if !hits()
                .iter()
                .any(|h| h.host == target && within.contains(h.bounds.mid()))
            {
                return false;
            }
            let _held = AppKeys::hold(target.pid);
            // AppKit and Chromium retain a type-ahead search across calls.
            // A second title sent during that interval appends to the first.
            // Wait without opening a menu, then recheck placement before keys.
            if !wait_typeahead(&stopped) {
                return false;
            }
            if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            if !hits()
                .iter()
                .any(|h| h.host == target && within.contains(h.bounds.mid()))
            {
                return false;
            }
            if !popup_keys_allowed(target) {
                return false;
            }
            w.focus_waking(target, false);
            if !super::ax::set_attribute(
                element.get(),
                "AXFocused",
                objc2_core_foundation::CFBoolean::new(true),
            ) {
                return false;
            }
            for ch in title.chars() {
                if stopped.load(std::sync::atomic::Ordering::Relaxed) || !popup_keys_allowed(target)
                {
                    return false;
                }
                let mut units = [0u16; 2];
                let units = ch.encode_utf16(&mut units);
                for down in [true, false] {
                    let Some(event) = CGEvent::new_keyboard_event(w.source.as_deref(), 0, down)
                    else {
                        return false;
                    };
                    CGEvent::set_flags(Some(&event), CGEventFlags::empty());
                    unsafe {
                        CGEvent::keyboard_set_unicode_string(
                            Some(&event),
                            units.len() as _,
                            units.as_ptr(),
                        );
                    }
                    deliver(&event, target.pid, true);
                }
                thread::sleep(Duration::from_millis(4));
            }
            true
        })
        .await
        .unwrap_or(false)
    }

    /// Focus the display's window of app `pid` for a menu command: the one
    /// last clicked if it is that app's, else its topmost window there.
    /// False when the app has no window there: then a menu command would
    /// act on its key window elsewhere, maybe the user's.
    pub async fn focus_app(&self, pid: i32, within: Rect) -> bool {
        self.run(move |w| {
            let there: Vec<_> = hits()
                .into_iter()
                .filter(|h| h.layer == 0 && h.host.pid == pid && within.contains(h.bounds.mid()))
                .map(|h| h.host)
                .collect();
            // The one last clicked, if it is still there.
            let target = w
                .focused
                .filter(|t| there.contains(t))
                .or(there.first().copied());
            let Some(target) = target else {
                return false;
            };
            let _held = AppKeys::hold(pid);
            w.focus(target);
            true
        })
        .await
        .unwrap_or(false)
    }

    /// Note the app's key window, if it is the user's, to hand key focus
    /// back to once the agent is done: before an action that makes one of
    /// the agent's windows key, such as asking a running app for a new one.
    pub async fn remember_user_window(&self, pid: i32) {
        self.run(move |w| {
            if let Some(id) = super::ax::focused_window_id(pid)
                && !on_agent_display(id)
            {
                w.remember(WindowTarget { pid, window_id: id });
            }
        })
        .await;
    }

    /// Send keys to `target` from now on, as a click on it would.
    /// Make the first window of an app just started in the background
    /// take clicks: see `Worker::wake`.
    /// Spend the click a new window drops, without waiting for it: finding
    /// an empty spot asks the app, which takes a while just after it
    /// starts. Input sent after it on this display waits its turn.
    pub fn wake(&self, target: WindowTarget, bounds: Rect) {
        self.send(move |w| {
            let _held = AppKeys::hold(target.pid);
            w.wake(target, bounds);
        });
    }

    /// `wake`, waiting for it, for input that doesn't pass through here.
    pub async fn wake_now(&self, target: WindowTarget, bounds: Rect) {
        self.run(move |w| {
            let _held = AppKeys::hold(target.pid);
            w.wake(target, bounds);
        })
        .await;
    }

    pub async fn focus(&self, target: WindowTarget) {
        self.run(move |w| {
            let _held = AppKeys::hold(target.pid);
            w.focus(target)
        })
        .await;
    }

    /// Press a key chord and keep it down, for apps that act on held keys.
    /// The caller waits off the worker, so other input on the display isn't
    /// held up. The window it went to, or None when nothing can take keys.
    pub async fn chord(&self, chord: KeyChord, down: bool, within: Rect) -> Option<WindowTarget> {
        self.run(move |w| {
            let _held = w.focus_something(within)?;
            let target = w.focused?;
            w.key(chord.code, down, flags(chord.modifiers), true);
            Some(target)
        })
        .await
        .flatten()
    }

    /// Release a held chord in the window it was pressed in, even if keys
    /// go elsewhere now or it left the display.
    pub async fn release_chord(&self, chord: KeyChord, target: WindowTarget) {
        self.run(move |w| w.key_to(target, chord.code, false, flags(chord.modifiers), true))
            .await;
    }

    /// The user left the Viewer: hand focus back to their window in a
    /// shared app, if keys typed there took it.
    pub fn hand_back(&self) {
        self.send(|w| w.restore_focus());
    }

    /// Done with keys for now: hand focus back to the user's window in a
    /// shared app, if the agent's keys took it.
    pub async fn keys_done(&self) {
        self.run(|w| w.restore_focus()).await;
    }

    /// Forget the keyboard target if it is `target`, as when the user takes
    /// that window back.
    pub async fn forget_target(&self, target: WindowTarget) {
        self.run(move |w| {
            if w.focused == Some(target) {
                w.focused = None;
            }
        })
        .await;
    }

    /// The window keys would go to now, if any.
    pub async fn keys_target(&self, within: Rect) -> Option<WindowTarget> {
        self.run(move |w| {
            // No wake: its click would count as the user's input and stop
            // the borrow, which brings the app to the front itself.
            w.focus_something_waking(within, false)?;
            w.focused
        })
        .await
        .flatten()
    }

    /// False when no window on the display can take the keys.
    pub async fn press(&self, chord: KeyChord, count: i64, within: Rect) -> bool {
        self.run(move |w| {
            let Some(_held) = w.focus_something(within) else {
                return false;
            };
            for _ in 0..count.max(1) {
                w.key(chord.code, true, flags(chord.modifiers), true);
                thread::sleep(Duration::from_millis(20));
                w.key(chord.code, false, flags(chord.modifiers), true);
            }
            true
        })
        .await
        .unwrap_or(false)
    }
}

/// Whether window `id` is on one of Agent Screen's displays: an agent's,
/// never the user's to hand focus back to.
fn on_agent_display(id: u32) -> bool {
    super::windows::find(id).is_some_and(|w| {
        super::cg::active_displays()
            .into_iter()
            .filter(|&d| super::cg::is_agent_screen(d))
            .any(|d| super::cg::display_bounds(d).contains(w.bounds.mid()))
    })
}

/// The bounds of the display window `id` is on.
fn display_of(id: u32) -> Option<Rect> {
    let window = super::windows::find(id)?;
    super::cg::active_displays()
        .into_iter()
        .map(super::cg::display_bounds)
        .find(|area| area.contains(window.bounds.mid()))
}

/// The window at a global point's display, held at that point.
fn held(target: WindowTarget, at: (f64, f64)) -> Option<Grab> {
    let window = super::windows::find(target.window_id)?;
    let area = super::cg::active_displays()
        .into_iter()
        .map(super::cg::display_bounds)
        .find(|area| area.contains(at))?;
    Grab::held(target, window.bounds, at, area)
}

/// After the user's press and release in the Viewer: put the window back
/// on its display if the drag carried it off. An app that draws its own
/// title bar, as Electron apps do, has WindowServer move the window with
/// the real pointer, onto the user's screen, where their window manager
/// may then tile it. It lands where the drag would have taken it on the
/// display. Checked again for a while, since the window manager may move
/// it after the release.
fn keep_on_display(press: Grab, at: (f64, f64)) {
    // A click, or a drag inside the display: nothing to put back.
    if !press.left_display() {
        return;
    }
    thread::spawn(move || {
        for wait in [0, 150, 350, 700] {
            thread::sleep(Duration::from_millis(wait));
            if wait == 0 || press.left_display() {
                log::info!(
                    "input: window {} left its display in a drag, put back",
                    press.target.window_id
                );
                press.drag_to(at);
            }
        }
    });
}

/// Apps whose keys a display's worker is sending: one at a time per app.
/// An ANSI keyboard type. A key event carries the Mac's keyboard type, and
/// apps translate the key code through the layout for that type: on a JIS
/// type the ANSI `]` key types `[` and `[` types `@`.
const ANSI_KEYBOARD: i64 = 40;

static APPS_IN_USE: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
static APP_FREED: std::sync::Condvar = std::sync::Condvar::new();

/// Holds an app for one display's keys until dropped.
struct AppKeys(i32);

impl AppKeys {
    /// Waits for another display's keys to that app, at most 2 s, then
    /// goes ahead.
    fn hold(pid: i32) -> AppKeys {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut in_use = APPS_IN_USE.lock().unwrap_or_else(|e| e.into_inner());
        while in_use.contains(&pid) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            in_use = APP_FREED
                .wait_timeout(in_use, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        in_use.push(pid);
        AppKeys(pid)
    }
}

impl Drop for AppKeys {
    fn drop(&mut self) {
        let mut in_use = APPS_IN_USE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = in_use.iter().position(|&p| p == self.0) {
            in_use.swap_remove(i);
        }
        APP_FREED.notify_all();
    }
}

struct Gesture {
    target: WindowTarget,
    group: i64,
    /// Pressed by an agent's left_mouse_down, which holds it across calls.
    agent: bool,
    /// Where the pointer last was in this gesture.
    at: (f64, f64),
    /// Whether the button moved since the press.
    moved: bool,
}

struct Worker {
    source: Option<CFRetained<CGEventSource>>,
    focused: Option<WindowTarget>,
    /// `ACTIVATIONS` when `focused` was focused.
    focused_at: u64,
    /// The window that last had a click, and `ACTIVATIONS` then.
    woken: Option<(u32, u64)>,
    /// The user's window in a shared app whose key focus the agent's keys
    /// took; handed back when they are done.
    restore: Vec<WindowTarget>,
    /// The display's windows when keys last went out, to tell one a
    /// shortcut opened since.
    known: Vec<u32>,
    /// File panels clicked since they opened: those take keys.
    pressed_panels: Vec<u32>,

    gesture: Option<Gesture>,
    /// The window the user holds by its title bar in the Viewer, and
    /// whether it has moved since the press.
    grab: Option<Grab>,
    grab_moved: bool,
    /// The window the user's press in the Viewer went to, as it was then.
    pressed: Option<Grab>,
    cache: Option<(Vec<Hit>, Instant)>,
    seed: u64,
}

#[derive(Clone, Copy)]
struct Hit {
    /// Where input goes: for a file panel, the process that draws it.
    target: WindowTarget,
    /// The app's own window: for a file panel, the empty one in front.
    host: WindowTarget,
    bounds: Rect,
    layer: i64,
}

impl Worker {
    fn new() -> Self {
        Worker {
            source: CGEventSource::new(CGEventSourceStateID::HIDSystemState),
            focused: None,
            focused_at: 0,
            woken: None,
            restore: Vec::new(),
            known: Vec::new(),
            pressed_panels: Vec::new(),

            gesture: None,
            grab: None,
            grab_moved: false,
            pressed: None,
            cache: None,
            seed: std::process::id() as u64 ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// A click group id: coalesces the events of one gesture.
    fn next_group(&mut self) -> i64 {
        // xorshift; any non-zero 31-bit number will do.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed % (i32::MAX as u64 - 1)) as i64 + 1
    }

    /// Make `target` the window keys go to, without raising it.
    ///
    /// Focused before, it is focused again once the system's front app has
    /// changed since: the change, even a moment's while another app starts,
    /// takes key status from it, and Chrome then ignores clicks in its page.
    ///
    /// The app's key window may have moved to another of its windows
    /// since, as when the user clicked theirs or the agent's keys handed
    /// focus back: then focus the target again, and remember the window to
    /// hand focus back to once these keys are done.
    fn focus(&mut self, target: WindowTarget) {
        self.focus_waking(target, true);
    }

    /// `focus`, with `wake` false where a click there would get in the way,
    /// as in a drag by the title bar.
    fn focus_waking(&mut self, target: WindowTarget, wake: bool) {
        let before = super::ax::focused_window_id(target.pid);
        if let Some(other) = before.filter(|&id| id != target.window_id) {
            let other = WindowTarget {
                pid: target.pid,
                window_id: other,
            };
            log::debug!(
                "input: keys in {target:?}'s app were in window {}",
                other.window_id
            );
            if !on_agent_display(other.window_id) {
                self.remember(other);
            }
            // A focus record doesn't make another window of an app in the
            // background key again; a click does.
            self.woken = None;
        } else if self.focused == Some(target) && self.focused_at == activations() {
            return;
        }
        // A window of an app in the background becomes key on a click and
        // drops that click: a new window, or any once the front app has
        // changed. Spend one where nothing is pressed, unless it had one
        // since. Never in the user's front app, where it would take keys.
        if wake && self.woken != Some((target.window_id, activations())) && !is_front(target.pid) {
            let empty = cg::window_bounds(target.window_id)
                .filter(|_| on_agent_display(target.window_id))
                .and_then(|bounds| title_bar_point(target, bounds));
            if let Some(at) = empty {
                log::debug!("input: waking {target:?}");
                self.wake_at(target, at);
                return;
            }
            log::debug!("input: no empty title bar to wake {target:?}");
        }
        self.send_focus(target);
    }

    fn send_focus(&mut self, target: WindowTarget) {
        // Let AppKit in the target update its key-window routing before
        // events arrive (cua uses the same 50 ms settle).
        if unsafe { SSFocusWithoutRaise(target.pid, target.window_id) } {
            thread::sleep(Duration::from_millis(50));
        }
        self.focused = Some(target);
        self.focused_at = activations();
    }

    /// Hand key focus back to the user's window the agent's keys took it
    /// from. The agent's window is focused again on its next keys.
    /// The user's window of an app to hand key focus back to: the latest,
    /// one per app.
    fn remember(&mut self, window: WindowTarget) {
        self.restore.retain(|w| w.pid != window.pid);
        self.restore.push(window);
    }

    fn restore_focus(&mut self) {
        for window in std::mem::take(&mut self.restore) {
            // Not in the middle of another display's keys to the app.
            let _held = AppKeys::hold(window.pid);
            // Only while an agent's window, this one's or another
            // session's, still has the keys: a window the user picked
            // since is left alone.
            let now = super::ax::focused_window_id(window.pid);
            // Still the user's, as when an action that will take it is
            // still running: keep it for that action's hand-back.
            if now == Some(window.window_id) {
                self.restore.push(window);
                continue;
            }
            // The app didn't answer in time, as under load: try again on
            // the next hand-back rather than lose the user's window.
            if now.is_none() {
                self.restore.push(window);
                continue;
            }
            if !now.is_some_and(on_agent_display) {
                continue;
            }
            // The app came to the front meanwhile: the focus record is
            // refused there, so make the user's window main through
            // accessibility, as their click would.
            if !unsafe { SSFocusWithoutRaise(window.pid, window.window_id) } {
                super::ax::make_main(window);
            }
        }
    }

    fn mouse_down(
        &mut self,
        at: (f64, f64),
        button: Button,
        clicks: i64,
        flags: CGEventFlags,
    ) -> Option<WindowTarget> {
        // A button an agent left held is released before anyone presses
        // again, such as the user taking over.
        self.release_agent_button();
        let Some(hit) = self.hit(at, true).or_else(|| self.edge_hit(at)) else {
            log::info!("input: no window at {at:?}");
            return None;
        };
        let target = hit.target;
        log::debug!("input: press at {at:?} goes to {target:?}");
        let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
        if hit.layer == popup_menu
            && matches!(button, Button::Left)
            && super::ax::press_menu_item_at(hit.host.pid, at)
        {
            // No gesture: the release that follows has nothing to send.
            log::debug!("input: pressed the menu item at {at:?}");
            return Some(target);
        }
        if hit.target != hit.host {
            self.pressed_panels.push(target.window_id);
        }
        {
            let _held = AppKeys::hold(target.pid);
            self.focus(target);
        }
        // Keys go to the window clicked, not to one that opened earlier.
        if let Some(within) = display_of(target.window_id) {
            // The list the press was hit-tested in, taken a moment ago.
            let list = self.cache.as_ref().map_or_else(hits, |(l, _)| l.clone());
            self.known = list
                .iter()
                .filter(|h| h.layer == 0 && within.contains(h.bounds.mid()))
                .map(|h| h.host.window_id)
                .collect();
        }
        let group = self.next_group();
        self.gesture = Some(Gesture {
            target,
            group,
            agent: false,
            at,
            moved: false,
        });
        self.post(CGEventType::MouseMoved, at, target, button, 0, group, flags);
        self.post(button.down(), at, target, button, clicks, group, flags);
        // Dropped or not, the press made the window key.
        self.woken = Some((target.window_id, activations()));
        Some(target)
    }

    /// Hold the window whose title bar the user pressed in the Viewer, to
    /// move it as they drag. The window gets key focus as a click would
    /// give it; nothing is posted until the release.
    fn grab(&mut self, at: (f64, f64), button: Button) -> bool {
        self.grab = None;
        self.grab_moved = false;
        if button != Button::Left {
            return false;
        }
        self.release_agent_button();
        let Some(hit) = self.hit(at, true) else {
            return false;
        };
        let Some(area) = cg::active_displays()
            .into_iter()
            .map(cg::display_bounds)
            .find(|area| area.contains(at))
        else {
            return false;
        };
        if hit.layer != 0 || hit.target != hit.host {
            return false;
        }
        let Some(grab) = Grab::title_bar(hit.target, hit.bounds, at, area) else {
            return false;
        };
        {
            let _held = AppKeys::hold(hit.target.pid);
            self.focus_waking(hit.target, false);
        }
        log::debug!(
            "input: holding window {} by its title bar",
            hit.target.window_id
        );
        self.grab = Some(grab);
        true
    }

    /// Let go of a button an agent left held, where its pointer was, not
    /// where the user now presses: an agent's drag drops where it was.
    fn release_agent_button(&mut self) {
        if let Some(g) = self.gesture.take_if(|g| g.agent) {
            self.post(
                Button::Left.up(),
                g.at,
                g.target,
                Button::Left,
                1,
                g.group,
                CGEventFlags::empty(),
            );
        }
    }

    fn mouse_dragged(&mut self, to: (f64, f64), button: Button, flags: CGEventFlags) {
        let Some(g) = &mut self.gesture else { return };
        let (from, first) = (g.at, !std::mem::replace(&mut g.moved, true));
        g.at = to;
        let (target, group) = (g.target, g.group);
        // AppKit starts a resize, or a drag in an app, once the pointer
        // has gone 3 to 5 points from the press, and loses a move that jumps
        // past that at once. A hand's first moves are a point or so each: so
        // are these, up to 8 points, before the agent's move.
        if first {
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let length = dx.hypot(dy);
            for k in 1..(length.ceil() as i32).min(9) {
                let t = k as f64 / length;
                let step = (from.0 + dx * t, from.1 + dy * t);
                self.post(button.dragged(), step, target, button, 1, group, flags);
                thread::sleep(Duration::from_millis(16));
            }
            // The resize takes a moment to start: a move sent sooner is lost.
            thread::sleep(Duration::from_millis(100));
        }
        self.post(button.dragged(), to, target, button, 1, group, flags);
    }

    fn mouse_up(&mut self, at: (f64, f64), button: Button, clicks: i64, flags: CGEventFlags) {
        let Some(g) = self.gesture.take() else { return };
        self.post(button.up(), at, g.target, button, clicks, g.group, flags);
    }

    fn mouse_moved(&mut self, to: (f64, f64), flags: CGEventFlags) {
        if self.gesture.is_some() {
            return;
        }
        let Some(hit) = self.hit(to, false) else {
            return;
        };
        self.post(
            CGEventType::MouseMoved,
            to,
            hit.target,
            Button::Left,
            0,
            0,
            flags,
        );
    }

    fn scroll(
        &mut self,
        at: (f64, f64),
        delta: (f64, f64),
        continuous: bool,
        phase: i64,
        momentum: i64,
        flags: CGEventFlags,
    ) {
        let Some(hit) = self.hit(at, false) else {
            return;
        };
        // On a Mac the window under the pointer takes the scroll, focused
        // or not. Apps here see events posted to them, and some hand those
        // to their focused view, so focus the window under the pointer when
        // a gesture starts, as a click would. Wheel ticks have no phase.
        let starts = matches!(phase, 1 | 128) || (phase == 0 && momentum == 0);
        if starts && self.focused != Some(hit.target) {
            let _held = AppKeys::hold(hit.target.pid);
            self.focus(hit.target);
        }
        let units = if continuous {
            CGScrollEventUnit::Pixel
        } else {
            CGScrollEventUnit::Line
        };
        let Some(event) = CGEvent::new_scroll_wheel_event2(
            self.source.as_deref(),
            units,
            2,
            wheel_steps(delta.1),
            wheel_steps(delta.0),
            0,
        ) else {
            return;
        };
        let e = Some(&*event);
        CGEvent::set_location(e, CGPoint::new(at.0, at.1));
        CGEvent::set_flags(e, flags);
        CGEvent::set_integer_value_field(
            e,
            CGEventField::ScrollWheelEventIsContinuous,
            continuous as i64,
        );
        CGEvent::set_integer_value_field(e, CGEventField::ScrollWheelEventScrollPhase, phase);
        CGEvent::set_integer_value_field(e, CGEventField::ScrollWheelEventMomentumPhase, momentum);
        if continuous {
            CGEvent::set_double_value_field(
                e,
                CGEventField::ScrollWheelEventPointDeltaAxis1,
                delta.1,
            );
            CGEvent::set_double_value_field(
                e,
                CGEventField::ScrollWheelEventPointDeltaAxis2,
                delta.0,
            );
        }
        stamp_routing(&event, at, hit.target);
        // AppKit routes a scroll by its location in the window, and drops it
        // when that point misses: give it relative to the window.
        unsafe {
            SSEventSetWindowLocation(
                &event,
                CGPoint::new(at.0 - hit.bounds.x, at.1 - hit.bounds.y),
            )
        };
        deliver(&event, hit.target.pid, false);
    }

    /// A key the user types in the Viewer. The app may have moved its key
    /// window since the last key, or a shortcut opened a window: pick and
    /// focus as for an agent's keys, holding the app the same way.
    fn viewer_key(&mut self, code: u16, down: bool, flags: CGEventFlags, within: Rect) {
        // A key-up goes where its key-down went.
        if !down {
            self.key(code, down, flags, false);
            return;
        }
        // Only a window on the Viewer's display: the one focused may have
        // closed or left, and the app would send the key to its own key
        // window, maybe the user's.
        let Some(_held) = self.focus_something(within) else {
            log::debug!("input: key {code} dropped, no window on the display");
            return;
        };
        self.key(code, down, flags, false);
    }

    /// A key to the focused window, already picked and focused. `ansi` for
    /// an agent's chord, whose codes are ANSI key positions.
    fn key(&mut self, code: u16, down: bool, flags: CGEventFlags, ansi: bool) {
        let Some(target) = self.focused else {
            log::debug!("input: key {code} dropped, no window focused");
            return;
        };
        self.key_to(target, code, down, flags, ansi);
    }

    fn key_to(
        &mut self,
        target: WindowTarget,
        code: u16,
        down: bool,
        flags: CGEventFlags,
        ansi: bool,
    ) {
        let Some(event) = CGEvent::new_keyboard_event(self.source.as_deref(), code, down) else {
            return;
        };
        CGEvent::set_flags(Some(&event), flags);
        if ansi {
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::KeyboardEventKeyboardType,
                ANSI_KEYBOARD,
            );
        }
        deliver(&event, target.pid, true);
    }

    fn flags_changed(&mut self, code: u16, flags: CGEventFlags) {
        let Some(target) = self.focused else { return };
        let Some(event) = CGEvent::new_keyboard_event(self.source.as_deref(), code, true) else {
            return;
        };
        CGEvent::set_type(Some(&event), CGEventType::FlagsChanged);
        CGEvent::set_flags(Some(&event), flags);
        deliver(&event, target.pid, true);
    }

    /// Keys go to the last clicked window. When nothing on this display has
    /// been clicked yet, pick its topmost window so typing still lands.
    ///
    /// Holds the app for this display's keys until the guard drops, so
    /// another session typing into the same app can't make its own window
    /// key in the middle.
    /// A file panel takes keys only once clicked: no focus record makes
    /// it key. Click the strip along its top edge, above its controls,
    /// before its first keys.
    fn wake_panel(&mut self, panel: Hit) {
        let id = panel.target.window_id;
        if self.pressed_panels.contains(&id) {
            return;
        }
        log::debug!("input: waking file panel {:?}", panel.target);
        self.wake(panel.target, panel.bounds);
        self.pressed_panels.push(id);
    }

    /// Click the strip along a window's top edge, above its controls. A
    /// window that no focus record makes key, such as a file panel or the
    /// first window of an app just started in the background, becomes key
    /// on a click, and drops that click.
    fn wake(&mut self, target: WindowTarget, bounds: Rect) {
        let middle = (bounds.x + bounds.width / 2.0, bounds.y + 6.0);
        self.wake_at(target, title_bar_point(target, bounds).unwrap_or(middle));
    }

    fn wake_at(&mut self, target: WindowTarget, at: (f64, f64)) {
        self.send_focus(target);
        self.woken = Some((target.window_id, activations()));
        let group = self.next_group();
        let none = CGEventFlags::empty();
        let left = Button::Left;
        self.post(CGEventType::MouseMoved, at, target, left, 0, group, none);
        self.post(left.down(), at, target, left, 1, group, none);
        self.post(left.up(), at, target, left, 1, group, none);
        thread::sleep(Duration::from_millis(50));
    }

    fn focus_something(&mut self, within: Rect) -> Option<AppKeys> {
        self.focus_something_waking(within, true)
    }

    fn focus_something_waking(&mut self, within: Rect, wake: bool) -> Option<AppKeys> {
        let Some(target) = self.pick_for_keys(within) else {
            // The window left the display, as when the user took it back:
            // keys must not follow it to the user's screen.
            self.focused = None;
            return None;
        };
        let held = AppKeys::hold(target.host.pid);
        log::debug!(
            "input: keys go to {:?} (app window {:?})",
            target.target,
            target.host
        );
        if target.target != target.host {
            self.wake_panel(target);
        }
        self.focus_waking(target.target, wake);
        Some(held)
    }

    /// The window on the display keys go to: the one last focused, or a
    /// window its app opened on the display since, as a shortcut such as
    /// cmd+n does, or else the topmost.
    fn pick_for_keys(&mut self, within: Rect) -> Option<Hit> {
        let list = hits();
        // A panel shown again reuses its window, and needs waking again.
        self.pressed_panels
            .retain(|&id| list.iter().any(|h| h.target.window_id == id));
        let on: Vec<_> = list
            .iter()
            .filter(|h| within.contains(h.bounds.mid()))
            .collect();
        let cached = self
            .focused
            .and_then(|t| list.iter().find(|h| h.host == t || h.target == t))
            .filter(|h| within.intersects(&h.bounds));
        // Documents only: a suggestions popup or panel is no new window.
        let opened = cached.and_then(|c| {
            on.iter()
                .find(|h| {
                    h.layer == 0
                        && h.host.pid == c.host.pid
                        && !self.known.contains(&h.host.window_id)
                })
                .map(|h| **h)
        });
        let cached = cached.copied();
        self.known = on
            .iter()
            .filter(|h| h.layer == 0)
            .map(|h| h.host.window_id)
            .collect();
        opened
            .or(cached)
            .or_else(|| on.iter().find(|h| h.layer == 0).or(on.first()).map(|h| **h))
    }

    #[allow(clippy::too_many_arguments)]
    fn post(
        &self,
        kind: CGEventType,
        at: (f64, f64),
        target: WindowTarget,
        button: Button,
        click_state: i64,
        group: i64,
        flags: CGEventFlags,
    ) {
        let Some(event) = CGEvent::new_mouse_event(
            self.source.as_deref(),
            kind,
            CGPoint::new(at.0, at.1),
            button.cg(),
        ) else {
            return;
        };
        CGEvent::set_flags(Some(&event), flags);
        unsafe {
            SSEventSetIntegerField(&event, 1, click_state); // kCGMouseEventClickState
            SSEventSetIntegerField(&event, 3, button.cg().0 as i64); // kCGMouseEventButtonNumber
            SSEventSetIntegerField(&event, 7, 3); // kCGMouseEventSubtype: touch
            if group != 0 {
                SSEventSetIntegerField(&event, 58, group); // click group: coalesces one gesture
            }
        }
        stamp_routing(&event, at, target);
        // One route only: posting through SkyLight and the public API both
        // delivers every event twice to AppKit targets.
        deliver(&event, target.pid, false);
    }

    /// The topmost resizable window whose edge is just outside a point:
    /// macOS takes a press there as the start of a resize.
    fn edge_hit(&mut self, at: (f64, f64)) -> Option<Hit> {
        const MARGIN: f64 = 4.0;
        let hit = self
            .cache
            .as_ref()?
            .0
            .iter()
            .find(|h| h.layer == 0 && h.bounds.inset(-MARGIN, -MARGIN).contains(at))
            .copied()?;
        super::ax::window_element(hit.target)
            .is_some_and(|w| super::ax::settable(&w, "AXSize"))
            .then_some(hit)
    }

    /// Topmost window at a global point. Moves and scrolls reuse a list up to
    /// 100 ms old; clicks always query WindowServer.
    fn hit(&mut self, at: (f64, f64), fresh: bool) -> Option<Hit> {
        let now = Instant::now();
        let reuse = !fresh
            && self
                .cache
                .as_ref()
                .is_some_and(|(_, t)| now - *t < Duration::from_millis(100));
        if !reuse {
            self.cache = Some((hits(), now));
        }
        self.cache
            .as_ref()?
            .0
            .iter()
            .find(|h| h.bounds.contains(at))
            .copied()
    }
}

/// Fields WindowServer and Chromium use to route an event to a window that
/// is neither key nor under the real cursor.
fn stamp_routing(event: &CGEvent, at: (f64, f64), target: WindowTarget) {
    let window = target.window_id as i64;
    // The point in the window, from its top-left corner. AppKit takes it as
    // is for an event with a time, where it would work it out for one
    // without.
    let origin = cg::window_bounds(target.window_id).map_or((0.0, 0.0), |b| (b.x, b.y));
    unsafe {
        SSEventSetIntegerField(event, 40, target.pid as i64); // target pid
        SSEventSetIntegerField(event, 51, window); // window number
        SSEventSetIntegerField(event, 91, window); // window under pointer
        SSEventSetIntegerField(event, 92, window); // window under pointer that can handle it
        SSEventSetWindowLocation(event, CGPoint::new(at.0 - origin.0, at.1 - origin.1));
    }
}

fn deliver(event: &CGEvent, pid: i32, authenticate: bool) {
    super::ime::note_posted();
    stamp_time(event);
    if !unsafe { SSPostEventToPid(pid, event, authenticate) } {
        CGEvent::post_to_pid(pid, Some(event));
    }
}

/// Give an event the time it is sent, as hardware input has. A new event's
/// time is 0, and Chromium checks that input is newer than the page: a click
/// that changes the URL then kills the page's renderer. AppKit reads an
/// event with a time at the window location `stamp_routing` sets.
fn stamp_time(event: &CGEvent) {
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock: libc::clockid_t) -> u64;
    }
    // Nanoseconds of uptime, which AppKit reads as the event's time.
    let now = unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) };
    CGEvent::set_timestamp(Some(event), now);
}

/// Whole wheel steps, at least one for any movement.
fn wheel_steps(delta: f64) -> i32 {
    if delta == 0.0 {
        0
    } else if delta < 0.0 {
        delta.round().min(-1.0) as i32
    } else {
        delta.round().max(1.0) as i32
    }
}

/// Windows input can reach, front to back. The desktop is left out on
/// purpose: a click on Finder's desktop window counts as a wallpaper click,
/// and with "click wallpaper to reveal desktop" on (the default since macOS
/// 14) it slides every window on every display aside, the user's included.
fn hits() -> Vec<Hit> {
    let menu = CGWindowLevelForKey(CGWindowLevelKey::MainMenuWindowLevelKey) as i64;
    let status = CGWindowLevelForKey(CGWindowLevelKey::StatusWindowLevelKey) as i64;
    let mut list = cg::on_screen_windows()
        .into_iter()
        .filter(|w| {
            // From 1000 up are overlays that take no clicks, such as the
            // one screencapture spreads over every display while recording.
            (0..1000).contains(&w.layer)
                && w.layer != menu
                && w.layer != status
                && w.owner_name != "Window Server"
                && w.owner_name != "Dock"
                && w.alpha > 0.0
        })
        .map(|w| Hit {
            host: w.target,
            target: w.target,
            bounds: w.bounds,
            layer: w.layer,
        })
        .collect::<Vec<_>>();
    // An app's file panel is drawn by another process, behind an empty
    // window of the app's: input sent to the app never reaches it.
    let panels = cg::remote_panels();
    for hit in &mut list {
        let same = |a: f64, b: f64| (a - b).abs() <= 1.0;
        if let Some(panel) = panels.iter().find(|p| {
            p.target.pid != hit.target.pid
                && p.layer == hit.layer
                && same(p.bounds.x, hit.bounds.x)
                && same(p.bounds.y, hit.bounds.y)
                && same(p.bounds.width, hit.bounds.width)
                && same(p.bounds.height, hit.bounds.height)
        }) {
            hit.target = panel.target;
        }
    }
    list
}

/// A key chord through the public API, for apps that need no focus
/// handshake: a running app's New Window shortcut.
pub fn post_key_to_pid(pid: i32, chord: KeyChord) {
    for down in [true, false] {
        if let Some(event) = CGEvent::new_keyboard_event(None, chord.code, down) {
            CGEvent::set_flags(Some(&event), flags(chord.modifiers));
            stamp_time(&event);
            super::ime::note_posted();
            CGEvent::post_to_pid(pid, Some(&event));
        }
    }
}

/// How many times the system's front app has changed.
static ACTIVATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The front app's pid, 0 for none.
static FRONT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub fn note_activation(front: Option<i32>) {
    // Already noted at the user's press in the Viewer.
    let own = std::process::id() as i32;
    if front == Some(own) && FRONT.load(std::sync::atomic::Ordering::Relaxed) == own {
        return;
    }
    FRONT.store(front.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    let n = ACTIVATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    log::debug!("input: front app changed ({n}): {front:?}");
}

/// The user pressed in the Viewer, window number `viewer`, which brings
/// Agent Screen to the front with the press (issue #41).
///
/// macOS says so only after the press is on its way: until then the app
/// the press goes to still counts as the front one, which gets no wake
/// click. And Agent Screen, a menu bar app, becomes active without
/// becoming WindowServer's front process: an app that made itself active
/// before, as aTrust does when it shows its login window, stays that, and
/// a Chromium window of it then drops every press. So Agent Screen takes
/// that too, as the app the user is clicking.
pub fn note_viewer_press(viewer: u32) {
    let own = std::process::id() as i32;
    if front() != Some(own) {
        note_activation(Some(own));
    }
    let server = unsafe { SSFrontPid() };
    if server != own && !unsafe { SSMakeSelfFront(viewer) } {
        log::warn!("input: WindowServer's front process stays {server}");
    }
}

pub fn is_front(pid: i32) -> bool {
    FRONT.load(std::sync::atomic::Ordering::Relaxed) == pid
}

/// The front app as the last activation named it, without asking.
pub fn front() -> Option<i32> {
    Some(FRONT.load(std::sync::atomic::Ordering::Relaxed)).filter(|&pid| pid != 0)
}

fn activations() -> u64 {
    ACTIVATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// A point on the strip along a window's top edge with nothing to press
/// there, as a tab in a full tab bar would be.
fn title_bar_point(target: WindowTarget, bounds: Rect) -> Option<(f64, f64)> {
    [0.5, 0.6, 0.7, 0.8, 0.4, 0.3]
        .map(|f| (bounds.x + bounds.width * f, bounds.y + 6.0))
        .into_iter()
        .find(|&at| super::ax::on_title_bar(target, bounds, at))
}

#[cfg(test)]
mod typing_tests {
    #[test]
    fn a_windows_line_ending_is_one_return() {
        assert_eq!(super::typed_text("a\r\nb\nc\rd"), "a\nb\nc\rd");
    }
}
