//! Input for windows on an Agent Display, from the Viewer or an Agent
//! Session. Events go straight to the owning process through SkyLight, so
//! the real cursor stays where the user left it and the user's frontmost app
//! keeps its place in the window stack (ADR 0001).
//!
//! Each display has one worker thread. The focus handshake needs short
//! sleeps, and window list lookups stay off the main thread.

use super::cg;
use super::click_diagnostics::{self as diagnostics, Click, Event as ClickEvent, Phase};
use super::windows::Grab;
use crate::keychord::{KeyChord, Modifiers};
use crate::model::{Rect, WindowTarget};
use agent_screen_model::key_window::{AppWindow, KeyWindow, key_window};
use agent_screen_model::shared_app::{ClickEnd, bound_click_reaches, click_end, press_still_bound};
use agent_screen_model::viewer_keys::{Focus, Look, Secrecy, look_again_unanswered, settled};
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
    fn SSSetFrontProcess(pid: i32, window: u32) -> bool;
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
    /// The target's app came to the front partway through text, where its
    /// focused window may be the user's: how many characters went in
    /// before (#110).
    Fronted(usize),
    /// No window to send it to.
    Nowhere,
}

/// Keys refused because the app's focused window, which every key posted
/// to the app reaches, is off the Agent Displays (#120).
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub app: String,
    /// The focused window's title, when Accessibility names the window.
    pub window: Option<String>,
}

pub use agent_screen_model::typing::typed_text;

/// What came of `Forwarder::click_in`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clicked {
    /// The window clicked; None when nothing was sent.
    pub hit: Option<WindowTarget>,
    /// A takeover stopped clicks still to go.
    pub stopped: bool,
    /// A click checked for the user's focus sent nothing: the user's
    /// window of the app of this window, the one hosting the point, had
    /// focus as it was about to go out.
    pub front_took: Option<WindowTarget>,
    /// A press of a click bound to an element's window was held back: the
    /// window there was no longer the element's.
    pub moved: bool,
}

impl Clicked {
    /// How the click ended (`click_end`), once a takeover is ruled out.
    pub fn end(&self) -> ClickEnd {
        click_end(self.hit.is_some(), self.front_took.is_some(), self.moved)
    }
}

/// How far typing a closed pop-up button's item title went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupTyped {
    /// Every character went out.
    Typed,
    /// The user took over after this many characters.
    Stopped(usize),
    /// The button's window is no longer on the display: nothing typed.
    Gone,
    /// Its app's key window is off the Agent Displays, maybe the user's:
    /// nothing typed. `take_refused` says which.
    Refused,
    /// The button didn't become its app's focused element: nothing typed.
    Unfocused,
    /// The keys held a space, which is never typed: nothing typed.
    Spaced,
    /// Another window of the app took key focus, or the button's window
    /// left the display, after this many characters.
    Interrupted(usize),
    /// The next key would have come so long after the last that the app's
    /// search might have ended, and a space then presses the button and
    /// opens its menu: stopped after this many characters.
    Slow(usize),
}

/// The longest gap between two keys of a title. AppKit and Chromium end a
/// type-ahead search about a second after its last key, and a key after
/// that starts a new search. No space is ever typed, so a late key can't
/// open the menu, but it could choose another item: the gap is measured
/// as the key goes out, after the process's real-input gate.
const KEY_GAP: Duration = Duration::from_millis(300);

/// After a key, give the app a moment to take it and to run what a new
/// choice triggers, such as a change handler that focuses another field,
/// so the focus check before the next key sees it: until the pop-up shows
/// another item, then a little longer, or at most `KEY_SETTLE` in all.
/// Well under `KEY_GAP`, which the next key must still meet.
/// `before` is what the pop-up showed just before the key.
fn settle_key(element: &super::ax::Sendable, before: &Option<String>) {
    let start = Instant::now();
    while start.elapsed() < KEY_SETTLE {
        thread::sleep(Duration::from_millis(15));
        if super::ax::string_attribute(element.get(), "AXValue") != *before {
            thread::sleep(Duration::from_millis(80));
            return;
        }
    }
}

/// How long `settle_key` waits for the pop-up to show a new item.
const KEY_SETTLE: Duration = Duration::from_millis(90);

/// Wait out a type-ahead search an earlier title may have started, or
/// until the user takes over. False on a Takeover.
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

/// One display's input queue. Cheap to clone.
#[derive(Clone)]
pub struct Forwarder {
    jobs: mpsc::Sender<Job>,
    /// Set while the user has taken the display over: an agent's typing or
    /// drag still running stops at its next step.
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// What has the keyboard in the target, for the Viewer's input
    /// method, looked up on a thread of its own.
    probe: std::sync::Arc<(std::sync::Mutex<Probe>, std::sync::Condvar)>,
}

/// Lookups of what has the keyboard in the target and its caret, in
/// global points. They run on their own thread, never the one that posts
/// input: a hung app answers each accessibility call only at its timeout.
/// A lookup asked for while one is waiting replaces it.
#[derive(Default, Clone, Copy)]
struct Probe {
    /// The point of the user's last press in the Viewer.
    pressed: Option<(f64, f64)>,
    /// The focus as the target last reported it since that press.
    focus: Focus,
    /// The caret of the focused text element, as the target reports it
    /// since that press.
    reported: Option<Rect>,
    /// The last lookup asked for, and the last answered.
    asked: u64,
    answered: u64,
    /// Answers to lookups asked up to here are out of date: a press came
    /// since.
    floor: u64,
    /// The latest lookup asked for has answered since the focus last may
    /// have moved.
    known: bool,
    /// When the latest lookup was asked for, to measure how long it took.
    asked_at: Option<(u64, std::time::Instant)>,
    /// The focused element as last looked up, as a hash.
    element: Option<u64>,
    /// The focus may have moved, and no look has settled where it is
    /// (`viewer_keys::settled`): the element focused before, and the look
    /// just before this one.
    unsettled: bool,
    /// When the focus last may have moved: once the key or click that may
    /// have moved it went out.
    moved_at: Option<std::time::Instant>,
    /// The point of the click the focus may have moved on, until a look
    /// settles it.
    clicked: Option<(f64, f64)>,
    /// When the lookups for the latest ask began; a look again keeps it.
    chain_at: Option<std::time::Instant>,
    before: Option<u64>,
    previous: Option<Look>,
    /// The lookup waiting to run: the window keys went to, the display,
    /// and its number.
    waiting: Option<(Option<WindowTarget>, Rect, u64)>,
}

impl Probe {
    fn ask(&mut self) -> u64 {
        self.chain_at = Some(std::time::Instant::now());
        self.ask_again()
    }

    /// Another look for the same lookup.
    fn ask_again(&mut self) -> u64 {
        self.asked += 1;
        self.asked_at = Some((self.asked, std::time::Instant::now()));
        self.asked
    }

    /// A press: what was looked up before it may have changed.
    fn press(&mut self, at: (f64, f64)) {
        self.pressed = Some(at);
        self.focus = Focus::Other;
        self.reported = None;
        self.floor = self.asked;
        self.moved();
        self.clicked = Some(at);
    }

    /// The focus may have moved: unknown until a look settles it.
    fn moved(&mut self) {
        self.known = false;
        self.unsettled = true;
        self.moved_at = Some(std::time::Instant::now());
        self.before = self.element;
        self.previous = None;
    }

    /// A lookup asked for since the last press has yet to answer.
    fn pending(&self) -> bool {
        self.asked > self.answered.max(self.floor)
    }

    /// Whether the focus is a password field, as last looked up, or
    /// unknown since it may have moved.
    fn secrecy(&self) -> Secrecy {
        match (self.known, self.focus) {
            (false, _) => Secrecy::Unknown,
            (true, Focus::Secure) => Secrecy::Secure,
            (true, _) => Secrecy::Open,
        }
    }

    /// Keep an answer unless a newer one, or a press, came since it was
    /// asked for.
    #[cfg(test)]
    fn answer(&mut self, ticket: u64, focus: Focus, reported: Option<Rect>) -> bool {
        let seen = Seen {
            focus,
            ..Seen::default()
        };
        self.answer_look(ticket, seen, reported) != Answered::Stale
    }

    /// Keep an answer unless a newer one, or a press, came since it was
    /// asked for. Only the latest lookup tells where the focus is now, and
    /// after the focus may have moved, only once a look settles it: until
    /// then the answer asks for another look.
    fn answer_look(&mut self, ticket: u64, seen: Seen, reported: Option<Rect>) -> Answered {
        if ticket <= self.floor || ticket <= self.answered {
            return Answered::Stale;
        }
        self.answered = ticket;
        self.focus = seen.focus;
        self.reported = reported;
        self.element = seen.element;
        if ticket != self.asked {
            return Answered::Kept;
        }
        if let Some((asked, at)) = self.asked_at
            && asked == ticket
        {
            note_lookup(at.elapsed());
        }
        let ms = |at: Option<std::time::Instant>| {
            at.map_or(u64::MAX, |at| at.elapsed().as_millis() as u64)
        };
        // A transient failure: look again while keys may wait for it.
        if seen.focus == Focus::Unanswered {
            self.known = false;
            return if look_again_unanswered(ms(self.chain_at)) {
                Answered::Again
            } else {
                Answered::Kept
            };
        }
        let look = Look {
            focus: seen.focus,
            element: seen.element,
            // Nothing focused is away from no click; an element without a
            // frame may be.
            under_click: self
                .clicked
                .filter(|_| seen.element.is_some())
                .map(|at| seen.frame.is_some_and(|frame| frame.contains(at))),
        };
        if !self.unsettled || settled(self.before, self.previous, look, ms(self.moved_at)) {
            self.known = true;
            self.unsettled = false;
            self.clicked = None;
            return Answered::Kept;
        }
        self.known = false;
        self.previous = Some(look);
        Answered::Again
    }

    /// The reported caret, else a caret-sized rect at the press.
    fn rect(self) -> Option<Rect> {
        const HEIGHT: f64 = 18.0;
        self.reported.or_else(|| {
            self.pressed
                .map(|(x, y)| Rect::new(x, y - HEIGHT / 2.0, 0.0, HEIGHT))
        })
    }
}

/// What a lookup saw: the focus, the focused element as a hash, and its
/// frame in global points.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    focus: Focus,
    element: Option<u64>,
    frame: Option<Rect>,
}

/// What came of a lookup's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answered {
    /// Out of date: a newer lookup, or a press, came since it was asked.
    Stale,
    Kept,
    /// Kept, but it doesn't settle where the focus moved: look again.
    Again,
}

/// How long after an unsettled look the next one runs, and how long after
/// the key or click at the soonest: long enough for an app that moves its
/// focus some time after the key, as Chrome's renderer does, to have.
const LOOK_AGAIN: Duration = Duration::from_millis(40);
const LOOK_AGAIN_AFTER_MOVE: Duration = Duration::from_millis(100);

/// How long to wait before the next look, `since` the focus may have moved.
fn look_again_in(since: Option<Duration>) -> Duration {
    let rest = since.map_or(Duration::ZERO, |since| {
        LOOK_AGAIN_AFTER_MOVE.saturating_sub(since)
    });
    LOOK_AGAIN.max(rest)
}

impl Default for Forwarder {
    fn default() -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("agent-screen-input".into())
            .spawn(move || {
                let mut worker = Worker::new();
                loop {
                    // A hand-back that failed runs again on its own, unless
                    // other input comes first.
                    let job = match worker.retry_at {
                        Some(at) => {
                            match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                                Ok(job) => job,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    worker.restore_focus();
                                    continue;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                        None => match rx.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        },
                    };
                    job(&mut worker);
                }
            })
            .expect("input thread");
        let probe: std::sync::Arc<(std::sync::Mutex<Probe>, std::sync::Condvar)> =
            Default::default();
        let weak = std::sync::Arc::downgrade(&probe);
        thread::Builder::new()
            .name("agent-screen-focus".into())
            .spawn(move || inspect_loop(weak))
            .expect("focus thread");
        Forwarder {
            jobs,
            stopped: Default::default(),
            probe,
        }
    }
}

/// How long the latest focus lookups took, from asking to the answer, in
/// milliseconds, for tests: keys wait for them.
static LOOKUP_MS: std::sync::Mutex<Vec<f64>> = std::sync::Mutex::new(Vec::new());

fn note_lookup(took: Duration) {
    let mut list = LOOKUP_MS.lock().unwrap_or_else(|e| e.into_inner());
    if list.len() >= 256 {
        list.remove(0);
    }
    list.push(took.as_secs_f64() * 1000.0);
}

/// How long the latest focus lookups took, in milliseconds.
pub fn lookup_ms() -> Vec<f64> {
    LOOKUP_MS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Run the lookups a display's Viewer asks for, the latest first, until
/// its Forwarder is gone.
fn inspect_loop(probe: std::sync::Weak<(std::sync::Mutex<Probe>, std::sync::Condvar)>) {
    loop {
        let Some(shared) = probe.upgrade() else {
            return;
        };
        let (lock, changed) = &*shared;
        let waiting = {
            let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
            if p.waiting.is_none() {
                p = changed
                    .wait_timeout(p, Duration::from_secs(1))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            p.waiting.take()
        };
        let Some((focused, within, ticket)) = waiting else {
            continue;
        };
        let target = keys_target_now(focused, within);
        let (focus, reported, element, frame) =
            target.map_or((Focus::Other, None, None, None), super::ax::text_focus);
        log::debug!("input: focus of {target:?} is {focus:?}, caret {reported:?}");
        let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
        let seen = Seen {
            focus,
            element,
            frame,
        };
        let answered = p.answer_look(ticket, seen, reported);
        changed.notify_all();
        drop(p);
        match answered {
            Answered::Stale => {}
            // Keys held for the answer go on, and a composition already
            // started must not reach a password field, committed or not.
            Answered::Kept => super::native::main_async(super::viewer_input::lookup_answered),
            Answered::Again => {
                let since = {
                    let p = lock.lock().unwrap_or_else(|e| e.into_inner());
                    p.moved_at.map(|at| at.elapsed())
                };
                std::thread::sleep(look_again_in(since));
                let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
                // Unless a newer lookup was asked for meanwhile.
                if p.waiting.is_none() && p.asked == ticket {
                    let again = p.ask_again();
                    p.waiting = Some((focused, within, again));
                }
            }
        }
    }
}

/// The window keys would go to now: the one focused while it is on the
/// display, else the display's topmost.
fn keys_target_now(focused: Option<WindowTarget>, within: Rect) -> Option<WindowTarget> {
    let list = hits();
    focused
        .filter(|t| {
            list.iter()
                .any(|h| (h.host == *t || h.target == *t) && within.intersects(&h.bounds))
        })
        .or_else(|| {
            list.iter()
                .filter(|h| within.contains(h.bounds.mid()))
                .find(|h| h.layer == 0)
                .map(|h| h.target)
        })
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

    /// Run `job` on the input thread once the events sent before it are
    /// delivered.
    pub fn after(&self, job: impl FnOnce() + Send + 'static) {
        self.send(move |_| job());
    }

    // MARK: Live input from the Viewer, one event at a time. Flags are the
    // NSEvent's own modifier flags, whose bits match CGEventFlags, so Caps
    // Lock and the left/right device bits survive.

    pub fn mouse_down(
        &self,
        at: (f64, f64),
        button: Button,
        clicks: i64,
        flags: CGEventFlags,
        trace: Option<Click>,
    ) {
        self.probe
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .press(at);
        self.send(move |w| {
            diagnostics::within(trace, Phase::Down, || {
                w.drag_guard = None;
                let grabbed = w.grab(at, button);
                if grabbed && let Some(grab) = w.grab {
                    w.drag_guard = Some(ViewerDragGuard::new(grab.target.window_id));
                }
                diagnostics::emit(|| ClickEvent::Grab { held: grabbed });
                if !grabbed {
                    w.user_press = true;
                    let target = w.mouse_down(at, button, clicks, flags);
                    w.user_press = false;
                    w.pressed_moved = false;
                    w.last_escape_check = None;
                    w.pressed = target.and_then(|t| held(t, at));
                }
            })
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
            None => {
                if let Some(press) = w.pressed {
                    w.pressed_moved |= press.moved(to);
                    if w.pressed_moved && w.drag_guard.is_none() {
                        w.drag_guard = Some(ViewerDragGuard::new(press.target.window_id));
                    }
                }
                w.mouse_dragged(to, button, flags);
                if w.pressed_moved
                    && let Some(press) = w.pressed
                    && w.last_escape_check
                        .is_none_or(|t| t.elapsed() >= Duration::from_millis(50))
                {
                    w.last_escape_check = Some(Instant::now());
                    if press.left_display() {
                        w.mouse_up(to, button, 1, flags);
                        press.restore_to(to);
                        w.pressed = None;
                        w.pressed_moved = false;
                        w.last_escape_check = None;
                        w.grab = Some(press);
                        w.grab_moved = true;
                        w.grab_watch = true;
                    }
                }
            }
        });
    }

    pub fn mouse_up(
        &self,
        at: (f64, f64),
        button: Button,
        clicks: i64,
        flags: CGEventFlags,
        trace: Option<Click>,
    ) {
        self.send(move |w| {
            diagnostics::within(trace, Phase::Up, || {
                w.drag_guard = None;
                match w.grab.take() {
                    Some(grab) if std::mem::take(&mut w.grab_moved) => {
                        if std::mem::take(&mut w.grab_watch) {
                            grab.restore_to(at);
                            keep_on_display(grab, at, true);
                        } else {
                            grab.drag_to(at);
                        }
                    }
                    // Pressed and released in place: a click on the title bar,
                    // such as a double click that zooms the window.
                    Some(grab) => {
                        w.grab_watch = false;
                        w.mouse_down(grab.from(), button, clicks, flags);
                        w.mouse_up(grab.from(), button, clicks, flags);
                    }
                    None => {
                        w.mouse_up(at, button, clicks, flags);
                        let drag_moved = std::mem::take(&mut w.pressed_moved);
                        if let Some(press) = w.pressed.take() {
                            keep_on_display(press, at, drag_moved);
                        }
                    }
                }
            })
        });
    }

    pub fn mouse_moved(&self, to: (f64, f64), flags: CGEventFlags) {
        self.send(move |w| {
            w.drag_guard = None;
            w.mouse_moved(to, flags);
        });
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

    /// Text the user's input method finished in the Viewer: to the window
    /// the user's keys go to, as Unicode text.
    pub fn text(&self, text: String, within: Rect) {
        self.send(move |w| w.viewer_text(&text, within));
    }

    /// Look up what has the keyboard in the target and its caret, once
    /// the input queued before has gone out: the window the user's keys go
    /// to, or the display's topmost one. Nobody waits for it: keys go by
    /// what was last looked up. With `moved`, the focus may have moved, and
    /// whether it is a password field is unknown until this answers.
    pub fn inspect(&self, within: Rect, moved: bool) {
        let probe = self.probe.clone();
        let ticket = {
            let mut p = probe.0.lock().unwrap_or_else(|e| e.into_inner());
            if moved {
                p.moved();
            }
            p.ask()
        };
        // The worker only notes where its keys go; the lookup runs on
        // the focus thread.
        self.send(move |w| {
            let (lock, changed) = &*probe;
            let mut p = lock.lock().unwrap_or_else(|e| e.into_inner());
            // The key or click before went out just now: the app gets it
            // from here.
            if moved {
                p.moved_at = Some(std::time::Instant::now());
            }
            p.waiting = Some((w.focused, within, ticket));
            changed.notify_all();
        });
    }

    /// Look up the focus unless a lookup is already on its way: keys
    /// typed faster than lookups answer would otherwise keep the latest
    /// one ahead of every answer.
    pub fn inspect_unless_pending(&self, within: Rect) {
        let pending = self
            .probe
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending();
        if !pending {
            self.inspect(within, false);
        }
    }

    /// Whether the target's focus is a password field, as last looked up.
    pub fn secrecy(&self) -> Secrecy {
        self.probe
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .secrecy()
    }

    /// How many lookups the Viewer has asked for, for tests.
    pub fn inspections(&self) -> u64 {
        self.probe.0.lock().unwrap_or_else(|e| e.into_inner()).asked
    }

    /// Where the user's text goes on the display, in global points: the
    /// caret the target last reported, else the point last pressed.
    pub fn caret(&self) -> Option<Rect> {
        self.probe
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rect()
    }

    // MARK: Whole gestures for Agent Sessions. Each returns once every event
    // is posted, so a tool call can take a screenshot straight after.

    /// The window clicked, or None when no window is at `at`, and whether
    /// a takeover stopped clicks still to go. When `own` names an element's
    /// own windows, each press goes out only if the window at `at`, by the
    /// window that hosts it, is one of them (`bound_click_reaches`). With
    /// `check_front`, each goes out only if posted input reaches no window
    /// of the user's then (`posts_to`), checked after the wait for another
    /// display's input, just before it goes out. Else nothing is sent and
    /// the hit is None.
    pub async fn click_in(
        &self,
        at: (f64, f64),
        button: Button,
        count: i64,
        m: Modifiers,
        own: Option<Vec<u32>>,
        check_front: bool,
    ) -> Clicked {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            w.expect = own;
            w.check_front = check_front;
            w.front_took = None;
            w.moved = false;
            let count = count.max(1);
            let mut hit = None;
            let mut stopped_now = false;
            for index in 1..=count {
                // A takeover since the agent asked: no press of its lands on
                // the display the user now operates (#287).
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    stopped_now = true;
                    break;
                }
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
            w.expect = None;
            w.check_front = false;
            Clicked {
                hit,
                stopped: stopped_now,
                front_took: std::mem::take(&mut w.front_took),
                moved: std::mem::take(&mut w.moved),
            }
        })
        .await
        .unwrap_or_default()
    }

    /// A press-move-release that apps take as a drag: a hold before moving,
    /// so the press doesn't read as a click, small steps, and a pause at the
    /// end so drop targets see the pointer arrive before the release.
    pub async fn drag(&self, from: (f64, f64), to: (f64, f64), m: Modifiers) -> Sent {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            // A takeover since the agent asked: nothing is pressed. The
            // caller reads `agent_stopped`.
            if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                return Sent::Nowhere;
            }
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

    /// Wheel clicks as a wheel sends them: a few lines per event, a frame
    /// apart. Chrome scrolls at most a page for one event, so 800 clicks in
    /// one event moved a long list no further than 3 (#104).
    pub async fn wheel(&self, at: (f64, f64), delta: (f64, f64), m: Modifiers) {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            for (i, step) in wheel_chunks(delta).into_iter().enumerate() {
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                if i > 0 {
                    thread::sleep(WHEEL_GAP);
                }
                w.scroll(at, step, false, 0, 0, flags(m));
            }
        })
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
            // The caller checked where keys go with the app in the
            // background. Brought to the front since, it sends keys to its
            // focused window, maybe the user's: stop before the next key.
            let front_at_start = is_front(target.pid);
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
                if !front_at_start && is_front(target.pid) {
                    return Sent::Fronted(typed);
                }
                w.character(target, character);
                thread::sleep(Duration::from_millis(4));
            }
            // A page can drop keys that come faster than its script runs
            // (#50): the text is in the field, or the call says it isn't.
            // A busy page is still taking keys in while its text grows, and
            // Chromium's accessibility shows nothing new until the page's
            // script is idle: judge once the text has changed and then held
            // still for 1.5 s, within 5 s.
            // A field that never changes, as a word typed over the same
            // word, is given up on sooner. A Takeover ends the wait.
            if let Some((field, before)) = field {
                use agent_screen_model::typing::{READ_EVERY_MS, Readback, readback};
                let mut shown = before.clone();
                let mut still = 0;
                for _ in 0..200 {
                    if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                        return Sent::Typed(text.chars().count());
                    }
                    let now = super::ax::string_attribute(&field, "AXValue").unwrap_or_default();
                    still = if now == shown { still + 1 } else { 0 };
                    shown = now;
                    match readback(&text, &before, &shown, still) {
                        Readback::Landed | Readback::Unconfirmed => return Sent::All,
                        Readback::Dropped => return Sent::Dropped(shown),
                        Readback::Wait => thread::sleep(Duration::from_millis(READ_EVERY_MS)),
                    }
                }
            }
            Sent::All
        })
        .await
        .unwrap_or(Sent::Nowhere)
    }

    /// Type `keys` into a closed pop-up button, as type-ahead, with keys
    /// posted to the button's process only. No menu opens, so nothing takes
    /// the user's keyboard (ADR 0001): the keys never hold a space, and no
    /// click wakes the window. The first key goes out only once the button
    /// is its app's focused element. Each key goes out only while the
    /// button's window is on this display and is its app's key window, the
    /// app hasn't come to the front, no Takeover started, and the last key
    /// went out less than `KEY_GAP` before.
    ///
    /// A user who clicks their own window of the same app between the check
    /// and the key could still get that one key: the check is an AX call,
    /// the key a post, and nothing makes them one step.
    pub async fn choose_closed_popup(
        &self,
        target: WindowTarget,
        element: super::ax::Sendable,
        keys: String,
        within: Rect,
    ) -> PopupTyped {
        if keys.contains(char::is_whitespace) || keys.is_empty() {
            return PopupTyped::Spaced;
        }
        let stopped = self.stopped.clone();
        self.run(move |w| {
            let is_stopped = || stopped.load(std::sync::atomic::Ordering::Relaxed);
            let placed =
                || cg::window_bounds(target.window_id).is_some_and(|b| within.contains(b.mid()));
            if !placed() {
                return PopupTyped::Gone;
            }
            let _held = AppKeys::hold(target.pid);
            // Keys typed while an earlier search goes on add to it.
            if !wait_typeahead(&stopped) {
                return PopupTyped::Stopped(0);
            }
            if !placed() {
                return PopupTyped::Gone;
            }
            w.refused = None;
            w.focus_waking(target, false);
            if is_stopped() {
                return PopupTyped::Stopped(0);
            }
            if !w.keys_reach(target) {
                return PopupTyped::Refused;
            }
            if !super::ax::is_window_or_sheet_focused(target) && !is_front(target.pid) {
                let _ = super::ax::make_main(target);
                w.send_focus(target);
            }
            if is_stopped() {
                return PopupTyped::Stopped(0);
            }
            // The keys go to the app's focused element, which must be the
            // button: read it back. An ExtensionKit `.appex` button may report
            // its own service pid while hosted inside `target.pid` (#205).
            let element_pid = super::ax::pid_of(element.get()).unwrap_or(target.pid);
            let key_pid = if element_pid != target.pid && cg::is_remote_view_service(element_pid) {
                element_pid
            } else {
                target.pid
            };
            let focused_button = || {
                super::ax::is_focused_element(target.pid, element.get())
                    || (element_pid != target.pid
                        && super::ax::is_focused_element(element_pid, element.get()))
            };
            if !super::ax::set_attribute(
                element.get(),
                "AXFocused",
                objc2_core_foundation::CFBoolean::new(true),
            ) {
                return PopupTyped::Unfocused;
            }
            // Chromium moves focus a moment after it answers: look for up
            // to 300 ms, and send nothing if it never arrives.
            let asked = Instant::now();
            while !focused_button() {
                if is_stopped() {
                    return PopupTyped::Stopped(0);
                }
                if asked.elapsed() >= Duration::from_millis(300) {
                    return PopupTyped::Unfocused;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let front_at_start = is_front(target.pid);
            let mut last: Option<Instant> = None;
            let mut shown = None;
            for (typed, character) in keys.chars().enumerate() {
                if typed > 0 {
                    settle_key(&element, &shown);
                }
                if is_stopped() {
                    return PopupTyped::Stopped(typed);
                }
                shown = super::ax::string_attribute(element.get(), "AXValue");
                let mut units = [0u16; 2];
                let units = character.encode_utf16(&mut units);
                let events: Vec<_> = [true, false]
                    .into_iter()
                    .filter_map(|down| {
                        let event = CGEvent::new_keyboard_event(w.source.as_deref(), 0, down)?;
                        CGEvent::set_flags(Some(&event), CGEventFlags::empty());
                        unsafe {
                            CGEvent::keyboard_set_unicode_string(
                                Some(&event),
                                units.len() as _,
                                units.as_ptr(),
                            )
                        };
                        Some(event)
                    })
                    .collect();
                // A real-input loan holds the gate: measure the gap once
                // it is ours, just before the key goes out.
                let gate = super::real_input::process_gate(key_pid);
                let input = gate.read().unwrap_or_else(|e| e.into_inner());
                if is_stopped() {
                    return PopupTyped::Stopped(typed);
                }
                // The app sends every key to its key window and focused
                // element: another one now would get the rest, as a window
                // the user clicked, or a field the page's change handler or
                // the pop-up's action focused. A Take Back moves the window
                // off the display. Checked once per key, with the gate held
                // just before the key, so nothing comes between the check
                // and the key; a second check would push the key past
                // KEY_GAP.
                let reaches_button = !(!front_at_start && is_front(target.pid))
                    && placed()
                    && (super::ax::is_window_or_sheet_focused(target)
                        || (key_pid != target.pid
                            && super::ax::focused_window_id(target.pid).is_none()))
                    && focused_button();
                if !reaches_button {
                    return PopupTyped::Interrupted(typed);
                }
                if last.is_some_and(|at| at.elapsed() >= KEY_GAP) {
                    return PopupTyped::Slow(typed);
                }
                for event in &events {
                    post_now(event, key_pid, true);
                }
                last = Some(Instant::now());
                drop(input);
                thread::sleep(Duration::from_millis(4));
            }
            PopupTyped::Typed
        })
        .await
        .unwrap_or(PopupTyped::Gone)
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
            let focused = super::ax::focused_window_id(pid);
            let agents = focused.is_some_and(on_agent_display);
            match agent_screen_model::shared_app::user_window_to_remember(focused, agents) {
                Some(id) => {
                    log::debug!("input: pid {pid}'s user window {id} has focus; handing it back after");
                    w.remember(WindowTarget { pid, window_id: id });
                }
                None => log::debug!(
                    "input: pid {pid}'s focused window {focused:?} isn't the user's; nothing to hand back"
                ),
            }
        })
        .await;
    }

    /// Note `window` as the user's, to hand key focus back to at the next
    /// [`Self::keys_done`]: the agent's keys meanwhile used up an earlier
    /// note of it.
    pub async fn remember_window(&self, window: WindowTarget) {
        self.run(move |w| w.remember(window)).await;
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
        let stopped = self.stopped.clone();
        self.run(move |w| {
            // A takeover since the agent asked: no key goes down. The caller
            // reads `agent_stopped`.
            if down && stopped.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
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

    /// Why the last keys found nowhere to go, if the app's focused window
    /// was off the Agent Displays; cleared by reading it.
    pub async fn take_refused(&self) -> Option<Refused> {
        self.run(|w| w.refused.take()).await.flatten()
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

    /// How many times the chord went out, or None when no window on the
    /// display can take the keys. Fewer than `count` when the target's app
    /// came to the front meanwhile, where its focused window may be the
    /// user's (#110).
    pub async fn press(&self, chord: KeyChord, count: i64, within: Rect) -> Option<i64> {
        let stopped = self.stopped.clone();
        self.run(move |w| {
            let _held = w.focus_something(within)?;
            let pid = w.focused?.pid;
            let front_at_start = is_front(pid);
            for pressed in 0..count.max(1) {
                // A takeover stops the presses still to go, the first
                // included (#287). The caller reads `agent_stopped`.
                if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    return Some(pressed);
                }
                if !front_at_start && is_front(pid) {
                    return Some(pressed);
                }
                w.key(chord.code, true, flags(chord.modifiers), true);
                thread::sleep(Duration::from_millis(20));
                w.key(chord.code, false, flags(chord.modifiers), true);
            }
            Some(count.max(1))
        })
        .await
        .flatten()
    }
}

/// Whether window `id` is on one of Agent Screen's displays: an agent's,
/// never the user's to hand focus back to.
pub fn on_agent_display(id: u32) -> bool {
    super::windows::find(id).is_some_and(|w| on_agent_display_at(w.bounds))
}

/// Whether a window with global `bounds` is on one of Agent Screen's
/// displays.
pub fn on_agent_display_at(bounds: Rect) -> bool {
    super::cg::active_displays()
        .into_iter()
        .filter(|&d| super::cg::is_agent_screen(d))
        .any(|d| super::cg::display_bounds(d).contains(bounds.mid()))
}

/// Where keys posted to `target`'s app go, asked up to `tries` times 40 ms
/// apart until they stay on the Agent Displays, with the app's windows as
/// last listed.
fn key_window_now(target: WindowTarget, tries: usize) -> (KeyWindow, Vec<super::windows::Window>) {
    let mut last = (KeyWindow::Elsewhere(None), Vec::new());
    for attempt in 0..tries {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(40));
        }
        let focused = super::ax::focused_window_id(target.pid);
        if focused == Some(target.window_id) {
            return (KeyWindow::Target, Vec::new());
        }
        let windows: Vec<_> = super::windows::all()
            .into_iter()
            .filter(|w| w.target.pid == target.pid)
            .collect();
        let listed: Vec<_> = windows
            .iter()
            .map(|w| AppWindow {
                id: w.target.window_id,
                on_agent_display: on_agent_display_at(w.bounds),
            })
            .collect();
        let reach = key_window(target.window_id, focused, &listed);
        if reach.safe() {
            return (reach, windows);
        }
        last = (reach, windows);
    }
    last
}

/// The bounds of the display window `id` is on.
fn display_of(id: u32) -> Option<Rect> {
    let window = super::windows::find(id)?;
    super::cg::active_displays()
        .into_iter()
        .map(super::cg::display_bounds)
        .find(|area| area.contains(window.bounds.mid()))
}

/// The window at a global point's display, held at that point. If `target` is
/// an attached modal sheet, watches its parent host window instead.
fn held(target: WindowTarget, at: (f64, f64)) -> Option<Grab> {
    let host = super::ax::host_window_target(target);
    let watch = agent_screen_model::viewer_press::press_watch_target(target, host);
    let window = super::windows::find(watch.window_id)?;
    let display = super::cg::active_displays()
        .into_iter()
        .find(|&id| super::cg::display_bounds(id).contains(at))?;
    Grab::held(watch, window.bounds, at, display)
}

/// After the user's press and release in the Viewer: put the window back
/// on its display if the drag carried it off. An app that draws its own
/// title bar, as Electron apps do, has WindowServer move the window with
/// the real pointer, onto the user's screen, where their window manager
/// may then tile it. Posted drag events are processed asynchronously, so
/// any non-grabbed drag monitors `[0, 40, 150, 350, 700]` ms after release
/// even when the window has not left at `t = 0` ms yet.
const VIEWER_DRAG_GUARD_TTL: Duration = Duration::from_secs(10);

static NEXT_VIEWER_DRAG_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

type ActiveViewerDragMap = std::collections::HashMap<u32, Vec<(u64, Instant)>>;

static ACTIVE_VIEWER_DRAGS: std::sync::LazyLock<std::sync::Mutex<ActiveViewerDragMap>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// RAII token marking `window_id` as actively being dragged in the Viewer or
/// watched by `keep_on_display`, so `guard_placed` pauses only for that window
/// rather than for the whole Takeover (`L5`), with a 10 s auto-expiry if a
/// Viewer release never arrives (`L7`).
pub(crate) struct ViewerDragGuard {
    window_id: u32,
    token: u64,
}

impl ViewerDragGuard {
    pub(crate) fn new(window_id: u32) -> Self {
        Self::new_at(window_id, Instant::now())
    }

    fn new_at(window_id: u32, now: Instant) -> Self {
        let token = NEXT_VIEWER_DRAG_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut map) = ACTIVE_VIEWER_DRAGS.lock() {
            let entries = map.entry(window_id).or_default();
            entries.retain(|(_, started)| {
                now.saturating_duration_since(*started) < VIEWER_DRAG_GUARD_TTL
            });
            entries.push((token, now));
        }
        Self { window_id, token }
    }
}

impl Drop for ViewerDragGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = ACTIVE_VIEWER_DRAGS.lock()
            && let Some(entries) = map.get_mut(&self.window_id)
        {
            entries.retain(|(token, _)| *token != self.token);
            if entries.is_empty() {
                map.remove(&self.window_id);
            }
        }
    }
}

pub fn viewer_watching_window(window_id: u32) -> bool {
    viewer_watching_window_at(window_id, Instant::now())
}

fn viewer_watching_window_at(window_id: u32, now: Instant) -> bool {
    let Ok(mut map) = ACTIVE_VIEWER_DRAGS.lock() else {
        return false;
    };
    let Some(entries) = map.get_mut(&window_id) else {
        return false;
    };
    entries.retain(|(_, started)| now.saturating_duration_since(*started) < VIEWER_DRAG_GUARD_TTL);
    if entries.is_empty() {
        map.remove(&window_id);
        false
    } else {
        true
    }
}

fn keep_on_display(press: Grab, at: (f64, f64), drag_moved: bool) {
    let left_now = press.left_display();
    if !agent_screen_model::viewer_press::should_watch_after_release(
        drag_moved,
        press.from(),
        at,
        left_now,
    ) {
        return;
    }
    let _watch_guard = ViewerDragGuard::new(press.target.window_id);
    thread::spawn(move || {
        let _keep_guard = _watch_guard;
        for wait in [0, 40, 150, 350, 700] {
            if wait > 0 {
                thread::sleep(Duration::from_millis(wait));
            }
            if press.left_display() {
                log::info!(
                    "input: window {} left its display in a drag, put back",
                    press.target.window_id
                );
                press.restore_to(at);
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
    /// took, with its failed hand-backs; handed back when they are done.
    restore: Vec<(WindowTarget, u8)>,
    /// When a failed hand-back runs again.
    retry_at: Option<Instant>,
    /// The display's windows when keys last went out, to tell one a
    /// shortcut opened since.
    known: Vec<u32>,
    /// File panels clicked since they opened: those take keys.
    pressed_panels: Vec<u32>,
    /// Set when keys were refused, as their app's focused window was off
    /// the Agent Displays.
    refused: Option<Refused>,

    gesture: Option<Gesture>,
    /// The window the user holds by its title bar in the Viewer, and
    /// whether it has moved since the press.
    grab: Option<Grab>,
    grab_moved: bool,
    grab_watch: bool,
    /// The window the user's press in the Viewer went to, as it was then.
    pressed: Option<Grab>,
    pressed_moved: bool,
    last_escape_check: Option<Instant>,
    drag_guard: Option<ViewerDragGuard>,
    cache: Option<(Vec<Hit>, Instant)>,
    /// Whether the press being handled is the user's, in the Viewer.
    user_press: bool,
    /// For a click that stands for an element's press: the element's own
    /// windows. A press whose window there, by the window its panels
    /// belong to, is another goes nowhere (`Center::of`).
    expect: Option<Vec<u32>>,
    /// For an agent's click on an element (`Forwarder::click_in`): check
    /// the user's focus again as the press goes out.
    check_front: bool,
    /// The window whose app's user window took focus as a click checked for
    /// it was about to go out, so nothing was sent.
    front_took: Option<WindowTarget>,
    /// Set when a bound click's press was held back because the element's
    /// window was no longer at the point.
    moved: bool,
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
    /// For a popover whose content another process draws (#306), that
    /// content's frame inside `bounds`. `target` draws only this part:
    /// a point outside it, such as on the arrow, goes to `host`. None for
    /// a file panel drawn at the host's frame, and for any other window.
    content: Option<Rect>,
}

fn record_hit(hit: Option<Hit>) {
    diagnostics::emit(|| ClickEvent::Hit {
        target: hit.map(|h| h.target),
        host: hit.map(|h| h.host),
        layer: hit.map(|h| h.layer),
        bounds: hit.map(|h| h.bounds),
    });
}

impl Worker {
    fn new() -> Self {
        Worker {
            source: CGEventSource::new(CGEventSourceStateID::HIDSystemState),
            focused: None,
            focused_at: 0,
            woken: None,
            restore: Vec::new(),
            retry_at: None,
            known: Vec::new(),
            pressed_panels: Vec::new(),
            refused: None,

            gesture: None,
            grab: None,
            grab_moved: false,
            grab_watch: false,
            pressed: None,
            pressed_moved: false,
            last_escape_check: None,
            drag_guard: None,
            cache: None,
            user_press: false,
            expect: None,
            check_front: false,
            front_took: None,
            moved: false,
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
        diagnostics::emit(|| ClickEvent::Focus {
            target,
            ax_focused_window: before,
        });
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
        // Nor while the app names any window key, the target included:
        // AppKit in the app would take it for its app's activation (#273).
        let record = agent_screen_model::shared_app::focus_record_needed(before);
        // A window of an app in the background becomes key on a click and
        // drops that click: a new window, or any once the front app has
        // changed. Spend one where nothing is pressed, unless it had one
        // since. Never in the user's front app, where it would take keys.
        if wake && self.woken != Some((target.window_id, activations())) && !is_front(target.pid) {
            let bounds =
                cg::window_bounds(target.window_id).filter(|_| on_agent_display(target.window_id));
            let at = bounds.and_then(|bounds| {
                title_bar_point(target, bounds).or_else(|| beside(target, bounds))
            });
            if let Some(at) = at {
                log::debug!(
                    "input: waking {target:?} at {at:?}, key window {before:?}, focus record {record}"
                );
                self.wake_at(target, at, record);
                return;
            }
            log::debug!("input: no empty title bar to wake {target:?}");
        }
        if record {
            self.send_focus(target);
        } else {
            self.mark_focused(target);
        }
    }

    fn send_focus(&mut self, target: WindowTarget) {
        // Let AppKit in the target update its key-window routing before
        // events arrive (cua uses the same 50 ms settle).
        let accepted = unsafe { SSFocusWithoutRaise(target.pid, target.window_id) };
        diagnostics::emit(|| ClickEvent::FocusRecord { target, accepted });
        if accepted {
            thread::sleep(Duration::from_millis(50));
        }
        self.mark_focused(target);
    }

    /// `target` is the window keys go to from now on.
    fn mark_focused(&mut self, target: WindowTarget) {
        self.focused = Some(target);
        self.focused_at = activations();
    }

    /// The user's window of an app to hand key focus back to: the latest,
    /// one per app.
    fn remember(&mut self, window: WindowTarget) {
        self.restore.retain(|(w, _)| w.pid != window.pid);
        self.restore.push((window, 0));
    }

    /// Hand key focus back to the user's window the agent's keys took it
    /// from. The agent's window is focused again on its next keys.
    fn restore_focus(&mut self) {
        use agent_screen_model::shared_app::{
            HAND_BACK_RETRY_MS, HandBack, after_hand_back, hand_back,
        };
        self.retry_at = None;
        for (window, failures) in std::mem::take(&mut self.restore) {
            // Not in the middle of another display's keys to the app.
            let _held = AppKeys::hold(window.pid);
            let now = super::ax::focused_window_id(window.pid);
            // The window the agent's keys and clicks went to, in that app.
            let agent = self
                .focused
                .filter(|f| f.pid == window.pid && f.window_id != window.window_id);
            let agent_main = agent
                .filter(|f| on_agent_display(f.window_id))
                .and_then(super::ax::is_main)
                .unwrap_or(false);
            log::debug!(
                "input: hand-back to {window:?}: focused {now:?}, agent's window {agent:?} main {agent_main}"
            );
            let step = hand_back(
                window.window_id,
                now,
                now.is_some_and(on_agent_display),
                agent_main,
            );
            // Still the user's, as when an action that will take it is
            // still running: keep it for that action's hand-back. Or the
            // app didn't answer in time, as under load: try again on the
            // next hand-back rather than lose the user's window. A window
            // the user picked since is left alone.
            //
            // Otherwise the agent's window is key: a focus record would
            // move nothing and only tell the app it became active (#273).
            // Make the user's window main through accessibility, as their
            // click would, so switching back to the app lands in it.
            let written = step == HandBack::Restore
                && {
                    let main = super::ax::make_main(window);
                    log::debug!(
                        "input: focus back to {window:?} from {now:?}: made main through accessibility {main}, focused now {:?}",
                        super::ax::focused_window_id(window.pid)
                    );
                    main
                };
            match after_hand_back(step, written, failures) {
                Some(failures) => {
                    if failures > 0 {
                        log::debug!(
                            "input: hand-back to {window:?} failed {failures} times; again in {HAND_BACK_RETRY_MS} ms"
                        );
                        self.retry_at =
                            Some(Instant::now() + Duration::from_millis(HAND_BACK_RETRY_MS));
                    }
                    self.restore.push((window, failures));
                }
                None if step != HandBack::Leave && !written => {
                    log::warn!("input: gave up handing focus back to {window:?}");
                }
                None => {}
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
        let hit = self.hit(at, true).or_else(|| self.edge_hit(at));
        record_hit(hit);
        // A click for an element's press goes only to the element's own
        // window, checked here, as the press goes out: another window that
        // covers it since, or none, gets nothing.
        if let Some(own) = &self.expect
            && !bound_click_reaches(hit.map(|h| h.host.window_id), own)
        {
            log::info!(
                "input: press at {at:?} found {:?} there, not the element's window {own:?}; nothing sent",
                hit.map(|h| h.host.window_id)
            );
            self.moved = true;
            return None;
        }
        let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
        let Some(hit) = hit else {
            // Clicking empty space on a display where an NSMenu (`layer == 101`)
            // is stuck open should still cancel that menu (#207).
            if matches!(button, Button::Left)
                && let Some(within) = cg::active_displays()
                    .into_iter()
                    .map(cg::display_bounds)
                    .find(|b| b.contains(at))
            {
                let windows = self.cache.as_ref().map_or_else(hits, |(l, _)| l.clone());
                let mut cancelled = None;
                for m in &windows {
                    if m.layer == popup_menu
                        && m.bounds.intersects(&within)
                        && !m.bounds.contains(at)
                    {
                        let pids = menu_pids_for(m, &windows);
                        if super::ax::cancel_menu_at(&pids, m.bounds.mid()) {
                            cancelled = Some(m.host);
                        }
                    }
                }
                if let Some(host) = cancelled {
                    return Some(host);
                }
            }
            log::info!("input: no window at {at:?}");
            return None;
        };
        let target = hit.target;
        log::debug!("input: press at {at:?} goes to {target:?}");
        // The windows the press was hit-tested in, taken a moment ago.
        let windows = self.cache.as_ref().map_or_else(hits, |(l, _)| l.clone());
        let within_display = cg::active_displays()
            .into_iter()
            .map(cg::display_bounds)
            .find(|b| b.contains(at))
            .or_else(|| display_of(target.window_id));
        if hit.layer == popup_menu
            && matches!(button, Button::Left)
            && super::ax::press_menu_item_at(&menu_pids_for(&hit, &windows), at)
        {
            // No gesture: the release that follows has nothing to send.
            log::debug!("input: pressed the menu item at {at:?}");
            return Some(target);
        }
        // An open pop-up menu runs a modal tracking loop in AppKit / `.appex`
        // that ignores posted clicks outside its window (#207). Cancel it
        // first when a click on the same display lands outside the menu.
        if hit.layer != popup_menu
            && matches!(button, Button::Left)
            && let Some(within) = within_display
        {
            for m in &windows {
                if m.layer == popup_menu && m.bounds.intersects(&within) && !m.bounds.contains(at) {
                    let _ = super::ax::cancel_menu_at(&menu_pids_for(m, &windows), m.bounds.mid());
                }
            }
        }
        if hit.target != hit.host {
            self.pressed_panels.push(target.window_id);
        }
        // A list of the front app takes the user's press as it is: making it key
        // would take key from the window it belongs to, and a list that
        // closes then closes before the press lands. An app in the
        // background drops a press on a window that isn't key, and a file
        // panel is another process's, so there the focus goes as before.
        // Only for the user's press: an agent's click focuses as before, so
        // the user's key window is remembered and handed back.
        if self.user_press
            && hit.target == hit.host
            && is_front(target.pid)
            && is_list(&windows, at)
        {
            log::debug!("input: press on a list of the front app at {at:?}, focus kept");
        } else {
            let _held = AppKeys::hold(target.pid);
            // The hold can wait up to 2 s for another display's keys:
            // check an element's click again before the focus goes out.
            if !self.still_goes(at, hit) {
                return None;
            }
            self.focus(target);
        }
        // The focus can wait 50 ms to settle, or click to wake the window,
        // and the process's input gate waits out a real-input loan: an
        // element's click checks once more with the gate held, so nothing
        // comes between the check and the press.
        let gate = super::real_input::process_gate(target.pid);
        let _input = gate.read().unwrap_or_else(|e| e.into_inner());
        if !self.still_goes(at, hit) {
            return None;
        }
        // Keys go to the window clicked, not to one that opened earlier.
        if let Some(within) = within_display {
            self.known = windows
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
        for (kind, state) in [(CGEventType::MouseMoved, 0), (button.down(), clicks)] {
            if let Some(event) = self.mouse_event(kind, at, target, button, state, group, flags) {
                post_now(&event, target.pid, false);
            }
        }
        // Dropped or not, the press made the window key.
        self.woken = Some((target.window_id, activations()));
        Some(target)
    }

    /// Whether an element's click (`Forwarder::click_in`) may still press
    /// `hit` at `at`, after a wait: hit-tested again, the point shows the
    /// same window, one of the element's own (`press_still_bound`), and
    /// posted input reaches no window of the user's (`posts_to`). On a miss
    /// it records why, and nothing is sent. Any other click goes on.
    fn still_goes(&mut self, at: (f64, f64), hit: Hit) -> bool {
        if let Some(own) = &self.expect {
            let ids = |h: &Hit| (h.target.window_id, h.host.window_id);
            let now = topmost_hit_in(&hits(), at).map(|h| ids(&h));
            if !press_still_bound(ids(&hit), now, own) {
                log::info!(
                    "input: press at {at:?} now finds {now:?} there, not {:?} of the element's windows {own:?}; nothing sent",
                    ids(&hit)
                );
                self.moved = true;
                return false;
            }
        }
        if self.check_front && !super::shared_app::posts_to(hit.host) {
            log::info!(
                "input: press at {at:?}: the user's window of its app has focus now; nothing sent"
            );
            self.front_took = Some(hit.host);
            return false;
        }
        true
    }

    /// Hold the window whose title bar the user pressed in the Viewer, to
    /// move it as they drag. The window gets key focus as a click would
    /// give it; nothing is posted until the release.
    fn grab(&mut self, at: (f64, f64), button: Button) -> bool {
        self.grab = None;
        self.grab_moved = false;
        self.grab_watch = false;
        if button != Button::Left {
            return false;
        }
        self.release_agent_button();
        let hit = self.hit(at, true);
        record_hit(hit);
        let Some(hit) = hit else {
            return false;
        };
        let Some(display) = cg::active_displays()
            .into_iter()
            .find(|&id| cg::display_bounds(id).contains(at))
        else {
            return false;
        };
        if hit.layer != 0 || hit.target != hit.host {
            return false;
        }
        let Some(grab) = Grab::title_bar(hit.target, hit.bounds, at, display) else {
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
        let Some(event) = scroll_event(
            self.source.as_deref(),
            at,
            delta,
            continuous,
            phase,
            momentum,
            flags,
        ) else {
            return;
        };
        let (routed, _) = stamp_routing(&event, at, hit.target);
        // AppKit routes a scroll by its location in the window, and drops it
        // when that point misses: give it relative to the window that draws it.
        let local = hit.scroll_location(at, routed);
        unsafe { SSEventSetWindowLocation(&event, CGPoint::new(local.0, local.1)) };
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

    /// Text the user's input method finished in the Viewer: picked and
    /// focused as for the user's keys.
    fn viewer_text(&mut self, text: &str, within: Rect) {
        let Some(_held) = self.focus_something(within) else {
            log::debug!("input: composed text dropped, no window on the display");
            return;
        };
        let Some(target) = self.focused else {
            return;
        };
        for character in text.chars() {
            self.character(target, character);
            thread::sleep(Duration::from_millis(4));
        }
    }

    /// One character to `target` as Unicode text, whatever the keyboard
    /// layout, with no modifiers. Newlines and tabs are the Return and Tab
    /// keys, so they submit and move between fields.
    fn character(&self, target: WindowTarget, character: char) {
        let code = match character {
            '\n' | '\r' => Some(36),
            '\t' => Some(48),
            _ => None,
        };
        let mut units = [0u16; 2];
        let units = character.encode_utf16(&mut units);
        for down in [true, false] {
            let Some(event) =
                CGEvent::new_keyboard_event(self.source.as_deref(), code.unwrap_or(0), down)
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
        if !panel.needs_wake(&self.pressed_panels) {
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
        self.wake_at(
            target,
            title_bar_point(target, bounds).unwrap_or(middle),
            true,
        );
    }

    /// The click, after a focus record when `record`.
    fn wake_at(&mut self, target: WindowTarget, at: (f64, f64), record: bool) {
        if record {
            self.send_focus(target);
        } else {
            self.mark_focused(target);
        }
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
        self.refused = None;
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
        self.wake_panel(target);
        self.focus_waking(target.target, wake);
        // Without the wake, as for `keys_target`, nothing goes out yet.
        if wake && !self.keys_reach(target.target) {
            return None;
        }
        Some(held)
    }

    /// Whether keys posted to `target`'s app now stay on the Agent
    /// Displays. The app sends them to its focused window, which may be
    /// the user's in an app they use too: the focus record alone doesn't
    /// take key focus from another window of an app in the background, and
    /// the wake click may not have found a spot (#120). AppKit may take a
    /// moment to report the click's change. Notes why when they don't.
    fn keys_reach(&mut self, target: WindowTarget) -> bool {
        let (mut reach, mut windows) = key_window_now(target, 5);
        // In the background the app makes a window its key window through
        // accessibility, as for its own main window, without coming to the
        // front; the window is on an Agent Display. Never in the user's
        // front app, where it would take their keys.
        if !reach.safe() && !is_front(target.pid) && super::ax::make_main(target) {
            log::debug!("input: made {target:?} main through accessibility");
            (reach, windows) = key_window_now(target, 3);
        }
        let KeyWindow::Elsewhere(focused) = reach else {
            return true;
        };
        log::warn!(
            "input: keys for {target:?} refused: its app's focused window {focused:?} is off the Agent Displays"
        );
        let window = focused.and_then(|id| windows.iter().find(|w| w.target.window_id == id));
        self.refused = Some(Refused {
            app: windows
                .first()
                .map(|w| w.app.clone())
                .or_else(|| super::windows::find(target.window_id).map(|w| w.app))
                .unwrap_or_else(|| "the app".into()),
            window: window.map(|w| w.title.clone()),
        });
        false
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
        let cached = focused_hit(&list, self.focused).filter(|h| within.intersects(&h.bounds));
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
        if let Some(event) = self.mouse_event(kind, at, target, button, click_state, group, flags) {
            // One route only: posting through SkyLight and the public API
            // both delivers every event twice to AppKit targets.
            deliver(&event, target.pid, false);
        }
    }

    /// The mouse event `post` sends, stamped for `target`, not sent.
    #[allow(clippy::too_many_arguments)]
    fn mouse_event(
        &self,
        kind: CGEventType,
        at: (f64, f64),
        target: WindowTarget,
        button: Button,
        click_state: i64,
        group: i64,
        flags: CGEventFlags,
    ) -> Option<CFRetained<CGEvent>> {
        let event = CGEvent::new_mouse_event(
            self.source.as_deref(),
            kind,
            CGPoint::new(at.0, at.1),
            button.cg(),
        )?;
        CGEvent::set_flags(Some(&event), flags);
        unsafe {
            SSEventSetIntegerField(&event, 1, click_state); // kCGMouseEventClickState
            SSEventSetIntegerField(&event, 3, button.cg().0 as i64); // kCGMouseEventButtonNumber
            SSEventSetIntegerField(&event, 7, 3); // kCGMouseEventSubtype: touch
            if group != 0 {
                SSEventSetIntegerField(&event, 58, group); // click group: coalesces one gesture
            }
        }
        let (bounds, local) = stamp_routing(&event, at, target);
        if matches!(
            kind,
            CGEventType::LeftMouseDown
                | CGEventType::LeftMouseUp
                | CGEventType::RightMouseDown
                | CGEventType::RightMouseUp
                | CGEventType::OtherMouseDown
                | CGEventType::OtherMouseUp
        ) {
            diagnostics::emit(|| ClickEvent::Stamp {
                event_type: kind.0,
                target,
                global: at,
                bounds,
                local,
            });
        }
        Some(event)
    }

    /// The topmost resizable window whose edge is just outside a point:
    /// macOS takes a press there as the start of a resize. Follows visual
    /// Z-order so the Viewer never clicks through a visible window onto one
    /// hidden below (`M2`).
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

    /// Topmost window at a global point in visual Z-order (`M2`). Moves and
    /// scrolls reuse a list up to 100 ms old; clicks always query WindowServer.
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
        topmost_hit_in(&self.cache.as_ref()?.0, at)
    }
}

/// The hit of the window keys last went to, as it takes keys: a popover
/// whose arrow was clicked is the app's window, not the view that draws
/// its content (#306). A file panel takes them by its app's window.
fn focused_hit(hits: &[Hit], focused: Option<WindowTarget>) -> Option<Hit> {
    let focused = focused?;
    let hit = hits
        .iter()
        .find(|h| h.host == focused || h.target == focused)?;
    Some(match hit.content {
        Some(_) if hit.target != focused => Hit {
            target: hit.host,
            content: None,
            ..*hit
        },
        _ => *hit,
    })
}

fn topmost_hit_in(hits: &[Hit], at: (f64, f64)) -> Option<Hit> {
    hits.iter()
        .find(|h| h.bounds.contains(at))
        .map(|h| h.at(at))
}

impl Hit {
    /// Where a point on this window goes: a point of a popover outside
    /// the content another process draws, such as on its arrow, goes to
    /// the app's window.
    fn at(self, at: (f64, f64)) -> Hit {
        match self.content {
            Some(content) if !content.contains(at) => Hit {
                target: self.host,
                content: None,
                ..self
            },
            _ => self,
        }
    }

    /// Whether keys need a click on this window first: a file panel drawn
    /// at the app's window's frame takes keys only once clicked. A popover
    /// another process draws gets none: the click would land on its
    /// controls before the key.
    fn needs_wake(&self, pressed_panels: &[u32]) -> bool {
        self.target != self.host
            && self.content.is_none()
            && !pressed_panels.contains(&self.target.window_id)
    }

    /// The point AppKit routes a scroll by: relative to the frame
    /// WindowServer reports for the target, else to the content or window
    /// frame hit-tested.
    fn scroll_location(&self, at: (f64, f64), routed: Option<Rect>) -> (f64, f64) {
        diagnostics::local_point(at, routed.or(self.content).or(Some(self.bounds)))
    }
}

/// Fields WindowServer and Chromium use to route an event to a window that
/// is neither key nor under the real cursor.
fn stamp_routing(
    event: &CGEvent,
    at: (f64, f64),
    target: WindowTarget,
) -> (Option<Rect>, (f64, f64)) {
    stamp_routing_using(
        at,
        target,
        |_| cg::routing_window_bounds(target),
        |field, value| unsafe {
            SSEventSetIntegerField(event, field, value);
        },
        |local| unsafe {
            SSEventSetWindowLocation(event, CGPoint::new(local.0, local.1));
        },
    )
}

fn stamp_routing_using(
    at: (f64, f64),
    target: WindowTarget,
    lookup: impl FnOnce(u32) -> Option<Rect>,
    mut field: impl FnMut(u32, i64),
    location: impl FnOnce((f64, f64)),
) -> (Option<Rect>, (f64, f64)) {
    let bounds = lookup(target.window_id);
    let local = diagnostics::local_point(at, bounds);
    let window = target.window_id as i64;
    field(40, target.pid as i64); // target pid
    field(51, window); // window number
    field(91, window); // window under pointer
    field(92, window); // window under pointer that can handle it
    location(local);
    (bounds, local)
}

fn deliver(event: &CGEvent, pid: i32, authenticate: bool) {
    let gate = super::real_input::process_gate(pid);
    let _input = gate.read().unwrap_or_else(|e| e.into_inner());
    post_now(event, pid, authenticate);
}

/// `deliver` with the process's real-input gate already held.
fn post_now(event: &CGEvent, pid: i32, authenticate: bool) {
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

/// Construct scroll metadata without posting it or reading any app state.
fn scroll_event(
    source: Option<&CGEventSource>,
    at: (f64, f64),
    delta: (f64, f64),
    continuous: bool,
    phase: i64,
    momentum: i64,
    flags: CGEventFlags,
) -> Option<CFRetained<CGEvent>> {
    let units = if continuous {
        CGScrollEventUnit::Pixel
    } else {
        CGScrollEventUnit::Line
    };
    let event = CGEvent::new_scroll_wheel_event2(
        source,
        units,
        2,
        wheel_steps(delta.1),
        wheel_steps(delta.0),
        0,
    )?;
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
        CGEvent::set_double_value_field(e, CGEventField::ScrollWheelEventPointDeltaAxis1, delta.1);
        CGEvent::set_double_value_field(e, CGEventField::ScrollWheelEventPointDeltaAxis2, delta.0);
    }
    Some(event)
}

use agent_screen_model::scroll::{wheel_chunks, wheel_steps};

const WHEEL_GAP: Duration = Duration::from_millis(12);

/// What a drag with the user's pointer reads before each event: the
/// windows input can reach, as `hits` lists them. The full window list,
/// where `remote_panels` looks, costs several times the on-screen one, so
/// the panels are read again only when an on-screen window appears, goes,
/// moves or changes level. A file panel moves and shows with the empty
/// window of its app's in front of it, so a change to a panel is a change
/// on screen too. A panel that closed while that window stayed reads as no
/// window, which refuses at the press and the release. The drag's own
/// image, which moves with the pointer, isn't a change.
#[derive(Default)]
pub struct DragSight {
    panels: std::cell::RefCell<Option<(Vec<PanelKey>, Vec<cg::WindowInfo>)>>,
}

type PanelKey = (WindowTarget, i64, Rect);

/// The windows under a drag at one moment, from `DragSight::look`.
pub struct DragScene {
    hits: Vec<Hit>,
    frames: Vec<(u32, Rect)>,
    source: WindowTarget,
    drop: WindowTarget,
    area: Rect,
}

impl DragSight {
    /// The windows now, read once for every point a check looks at. `source`
    /// and `drop` are the windows the drag starts and ends in on `area`.
    pub fn look(&self, source: WindowTarget, drop: WindowTarget, area: Rect) -> DragScene {
        let menu = CGWindowLevelForKey(CGWindowLevelKey::MainMenuWindowLevelKey) as i64;
        let status = CGWindowLevelForKey(CGWindowLevelKey::StatusWindowLevelKey) as i64;
        let dragging = CGWindowLevelForKey(CGWindowLevelKey::DraggingWindowLevelKey) as i64;
        // Only windows a press can reach are read in full: the menu bar
        // strips and status items, most of the list, are sorted out first.
        let on_screen = cg::on_screen_where(|_, layer, _| {
            (0..1000).contains(&layer) && layer != menu && layer != status
        });
        let frames = on_screen
            .iter()
            .map(|w| (w.target.window_id, w.bounds))
            .collect();
        let mut hits = reachable(on_screen, menu, status);
        let key: Vec<PanelKey> = hits
            .iter()
            .filter(|h| !(h.layer == dragging && h.target.pid == source.pid))
            .map(|h| (h.target, h.layer, h.bounds))
            .collect();
        let mut cache = self.panels.borrow_mut();
        if cache.as_ref().is_none_or(|(seen, _)| *seen != key) {
            *cache = Some((key, cg::remote_panels()));
        }
        if let Some((_, panels)) = cache.as_ref() {
            let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
            mark_panels(&mut hits, panels, popup_menu);
        }
        DragScene {
            hits,
            frames,
            source,
            drop,
            area,
        }
    }
}

impl DragScene {
    /// A window's frame, None when it isn't on screen as a window a press
    /// can reach.
    pub fn frame(&self, window_id: u32) -> Option<Rect> {
        self.frames
            .iter()
            .find(|(id, _)| *id == window_id)
            .map(|(_, frame)| *frame)
    }

    /// The app window under a point of the drag: as `window_at`, but the
    /// drag's own image doesn't cover it, sheets and panels (levels 0..=8)
    /// count by their host window, and a pop-up menu the drag itself opened
    /// on `area` counts as `drop` (#205).
    pub fn under(&self, at: (f64, f64)) -> Option<WindowTarget> {
        let dragging = CGWindowLevelForKey(CGWindowLevelKey::DraggingWindowLevelKey) as i64;
        let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
        resolve_drag_under(
            &self.hits,
            (self.source, self.drop),
            self.area,
            (dragging, popup_menu),
            cg::is_remote_view_service,
            at,
        )
    }
}

fn resolve_drag_under(
    hits: &[Hit],
    (source, drop): (WindowTarget, WindowTarget),
    area: Rect,
    (dragging, popup_menu): (i64, i64),
    is_remote: impl Fn(i32) -> bool,
    at: (f64, f64),
) -> Option<WindowTarget> {
    let hit = hits
        .iter()
        .filter(|h| !(h.layer == dragging && h.target.pid == source.pid))
        .find(|h| h.bounds.contains(at))?;
    if (0..=8).contains(&hit.layer) {
        Some(hit.host)
    } else if hit.layer == popup_menu
        && area.contains(at)
        && (hit.host.pid == source.pid || is_remote(hit.host.pid))
    {
        Some(drop)
    } else {
        None
    }
}

/// Pure check returning `(host_window, owner_display)` when the topmost
/// reachable window at `at` is a document/panel (`layer 0..=8`) recorded on a
/// display other than `display` (`M2`). Agent pointer tools refuse instead of
/// operating that foreign window or clicking through it to a hidden window below.
fn foreign_recorded_top_window_in(
    hits: &[Hit],
    display: crate::model::DisplayId,
    at: (f64, f64),
    recorded_owners: &std::collections::BTreeMap<u32, crate::model::DisplayId>,
) -> Option<(WindowTarget, crate::model::DisplayId)> {
    let top = topmost_hit_in(hits, at)?;
    if !(0..=8).contains(&top.layer) {
        return None;
    }
    let owner = recorded_owners.get(&top.host.window_id).copied()?;
    (owner != display).then_some((top.host, owner))
}

pub fn foreign_window_at(
    display: crate::model::DisplayId,
    at: (f64, f64),
) -> Option<(WindowTarget, crate::model::DisplayId)> {
    // Only inspects `top.host` and `top.layer`, which `mark_panels` never
    // changes, so no `remote_panels` (`OptionAll`) scan is needed.
    let owners = super::windows::recorded_owners_snapshot();
    foreign_recorded_top_window_in(&hits_on_screen(), display, at, &owners)
}

/// Combined foreign-window guard and top-window lookup for an agent click
/// at `at` on `display`, using a single hit list. Returns `Err((host, owner))`
/// when a foreign recorded window is topmost at `at`, else `Ok(window_at)`.
pub fn click_check_at(
    display: crate::model::DisplayId,
    at: (f64, f64),
) -> Result<Option<(WindowTarget, Rect)>, (WindowTarget, crate::model::DisplayId)> {
    let list = hits();
    let owners = super::windows::recorded_owners_snapshot();
    click_check_in(&list, display, at, &owners)
}

fn click_check_in(
    list: &[Hit],
    display: crate::model::DisplayId,
    at: (f64, f64),
    owners: &std::collections::BTreeMap<u32, crate::model::DisplayId>,
) -> Result<Option<(WindowTarget, Rect)>, (WindowTarget, crate::model::DisplayId)> {
    if let Some(foreign) = foreign_recorded_top_window_in(list, display, at, owners) {
        return Err(foreign);
    }
    // A popover's arrow or border is the app's window, as a press there
    // goes to it (`Hit::at`); its content is another process's view.
    let target = topmost_hit_in(list, at)
        .filter(|h| h.layer == 0 && h.target == h.host)
        .map(|h| (h.target, h.bounds));
    Ok(target)
}

/// Candidate Accessibility owner PIDs for an open `NSMenu` (`layer == 101`):
/// the menu's own target/host PIDs plus, when drawn by an out-of-process
/// `.xpc`/`.appex` view service, only the matching remote-panel host PID or
/// the single frontmost host window under/adjacent to `menu.bounds` (L7, #207).
fn menu_pids_for(menu: &Hit, windows: &[Hit]) -> Vec<i32> {
    let on_display = cg::active_displays()
        .into_iter()
        .map(cg::display_bounds)
        .find(|b| b.intersects(&menu.bounds));
    resolve_menu_pids(menu, windows, on_display, cg::is_remote_view_service)
}

fn resolve_menu_pids(
    menu: &Hit,
    windows: &[Hit],
    on_display: Option<Rect>,
    is_remote: impl Fn(i32) -> bool,
) -> Vec<i32> {
    let mut pids = vec![menu.target.pid];
    if menu.host.pid != menu.target.pid {
        pids.push(menu.host.pid);
        return pids;
    }
    if is_remote(menu.target.pid) {
        // First prefer an explicit remote-panel window backed by this `.appex`.
        for w in windows {
            if (0..=8).contains(&w.layer)
                && w.target.pid == menu.target.pid
                && w.host.pid != menu.target.pid
                && on_display.is_none_or(|d| w.bounds.intersects(&d))
            {
                if !pids.contains(&w.host.pid) {
                    pids.push(w.host.pid);
                }
                return pids;
            }
        }
        // Otherwise pick only the frontmost host window on that display whose
        // frame intersects or sits within 48 pt of the open menu (`windows` is
        // in front-to-back Z-order), never unrelated apps on the display (L7).
        for w in windows {
            if (0..=8).contains(&w.layer)
                && on_display.is_none_or(|d| w.bounds.intersects(&d))
                && w.bounds.inset(-48.0, -48.0).intersects(&menu.bounds)
                && !pids.contains(&w.host.pid)
            {
                pids.push(w.host.pid);
                break;
            }
        }
    }
    pids
}

/// Windows input can reach, front to back, without scanning off-screen
/// remote panels.
fn hits_on_screen() -> Vec<Hit> {
    let menu = CGWindowLevelForKey(CGWindowLevelKey::MainMenuWindowLevelKey) as i64;
    let status = CGWindowLevelForKey(CGWindowLevelKey::StatusWindowLevelKey) as i64;
    cg::on_screen_hit_candidates(menu, status)
        .into_iter()
        .map(|(target, layer, bounds)| Hit {
            host: target,
            target,
            bounds,
            layer,
            content: None,
        })
        .collect()
}

/// Short-lived cache of `cg::remote_panels()` keyed by the on-screen window
/// layout (`PanelKey`), matching `DragSight`'s invariant with a 500 ms TTL so
/// an out-of-process `.appex` swap behind an unmoved host window refreshes too.
const PANEL_CACHE_TTL: Duration = Duration::from_millis(500);
type PanelCacheEntry = (Vec<PanelKey>, Instant, Vec<cg::WindowInfo>);
static PANEL_CACHE: std::sync::Mutex<Option<PanelCacheEntry>> = std::sync::Mutex::new(None);

fn panel_keys(list: &[Hit]) -> Vec<PanelKey> {
    list.iter().map(|h| (h.host, h.layer, h.bounds)).collect()
}

fn panel_cache_valid(entry: &PanelCacheEntry, key: &[PanelKey], now: Instant) -> bool {
    entry.0.as_slice() == key && now.saturating_duration_since(entry.1) < PANEL_CACHE_TTL
}

fn cached_remote_panels(list: &[Hit]) -> Vec<cg::WindowInfo> {
    let key = panel_keys(list);
    let now = Instant::now();
    if let Some(entry) = PANEL_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        && panel_cache_valid(entry, &key, now)
    {
        return entry.2.clone();
    }
    let panels = cg::remote_panels();
    *PANEL_CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((key, Instant::now(), panels.clone()));
    panels
}

/// Windows input can reach, front to back. The desktop is left out on
/// purpose: a click on Finder's desktop window counts as a wallpaper click,
/// and with "click wallpaper to reveal desktop" on (the default since macOS
/// 14) it slides every window on every display aside, the user's included.
fn hits() -> Vec<Hit> {
    let mut list = hits_on_screen();
    let panels = cached_remote_panels(&list);
    let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
    mark_panels(&mut list, &panels, popup_menu);
    list
}

#[cfg(test)]
fn hit_list(
    on_screen: Vec<cg::WindowInfo>,
    panels: Vec<cg::WindowInfo>,
    menu: i64,
    status: i64,
    popup_menu: i64,
    is_remote: impl Fn(i32) -> bool,
) -> Vec<Hit> {
    let mut list = reachable(on_screen, menu, status);
    mark_panels_using(&mut list, &panels, popup_menu, is_remote);
    list
}

/// The on-screen windows a press can reach, front to back, each its own
/// target until `mark_panels`.
fn reachable(on_screen: Vec<cg::WindowInfo>, menu: i64, status: i64) -> Vec<Hit> {
    on_screen
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
            content: None,
        })
        .collect()
}

/// An app's file panel is drawn by another process, behind an empty
/// window of the app's, and a popover's content (Finder's Share pop-up,
/// #306) inside the app's popover window: input sent to the app never
/// reaches it. Point each such hit at the process that draws it.
fn mark_panels(list: &mut [Hit], panels: &[cg::WindowInfo], popup_menu: i64) {
    mark_panels_using(list, panels, popup_menu, cg::is_remote_view_service);
}

fn mark_panels_using(
    list: &mut [Hit],
    panels: &[cg::WindowInfo],
    popup_menu: i64,
    is_remote: impl Fn(i32) -> bool,
) {
    use agent_screen_model::remote_view::{self, Host};
    let remote: Vec<bool> = list.iter().map(|h| is_remote(h.target.pid)).collect();
    let hosts: Vec<Host> = list
        .iter()
        .map(|h| Host {
            frame: h.bounds,
            layer: h.layer,
            // An open menu keeps its own routing (#207).
            allow_inset: h.layer != popup_menu,
        })
        .collect();
    let views: Vec<(Rect, i64)> = panels.iter().map(|p| (p.bounds, p.layer)).collect();
    let fits = remote_view::assign(&hosts, &views, |h, v| {
        !remote[h] && panels[v].target.pid != list[h].target.pid
    });
    for (hit, fit) in list.iter_mut().zip(fits) {
        let Some((v, fit)) = fit else {
            continue;
        };
        let target = panels[v].target;
        if fit == remote_view::Fit::Inset {
            log::debug!(
                "input: popover {:?} drawn by remote view {target:?}",
                hit.host
            );
            hit.content = Some(views[v].0);
        }
        hit.target = target;
    }
}

/// The app window at a global point, as a press there would hit it, and
/// its frame. None for a menu, a pop-up, or a panel another process draws.
/// The window at `at` by the window that hosts it, and the frame of what is
/// there: the window a press at `at` goes to belongs to it. The rule for a
/// click bound to an element's windows (`bound_click_reaches`).
pub fn host_at(at: (f64, f64)) -> Option<(WindowTarget, Rect)> {
    topmost_hit_in(&hits(), at).map(|h| (h.host, h.bounds))
}

pub fn window_at(at: (f64, f64)) -> Option<(WindowTarget, Rect)> {
    topmost_hit_in(&hits(), at)
        .filter(|h| h.layer == 0 && h.target == h.host)
        .map(|h| (h.target, h.bounds))
}

/// The window under a `real_input` drag's start or end: includes sheets and
/// panels (levels 0..=8) by their host window (#205).
pub fn real_drag_window_at(at: (f64, f64)) -> Option<(WindowTarget, Rect)> {
    hits()
        .into_iter()
        .find(|h| h.bounds.contains(at))
        .filter(|h| (0..=8).contains(&h.layer))
        .map(|h| (h.host, h.bounds))
}

/// What a `real_input` click hits on a display: either a display window
/// directly (levels 0..=8, sheets and hosted panels included) or an open
/// pop-up menu (level 101) owned by an app on that display (#205).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RealClickHit {
    pub window: WindowTarget,
    pub window_frame: Rect,
    pub menu: Option<(WindowTarget, Rect)>,
    pub ax_pid: i32,
}

pub fn real_click_at(at: (f64, f64), display_windows: &[WindowTarget]) -> Option<RealClickHit> {
    let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
    resolve_real_click(
        &hits(),
        display_windows,
        popup_menu,
        cg::is_remote_view_service,
        at,
    )
}

/// Whether `at` is still directly over `expected` (`menu` when clicking an
/// open pop-up menu, else `window`).
pub fn real_click_still_under(
    at: (f64, f64),
    window: WindowTarget,
    menu: Option<WindowTarget>,
) -> bool {
    let popup_menu = CGWindowLevelForKey(CGWindowLevelKey::PopUpMenuWindowLevelKey) as i64;
    let Some(hit) = hits().into_iter().find(|h| h.bounds.contains(at)) else {
        return false;
    };
    match menu {
        Some(menu) => hit.layer == popup_menu && hit.host == menu,
        None => (0..=8).contains(&hit.layer) && hit.host == window,
    }
}

fn resolve_real_click(
    hits: &[Hit],
    display_windows: &[WindowTarget],
    popup_menu: i64,
    is_remote: impl Fn(i32) -> bool,
    at: (f64, f64),
) -> Option<RealClickHit> {
    // Resolved at the point: a popover's arrow or border is the app's, so
    // Accessibility reads the app there and the remote view on its content.
    let hit = topmost_hit_in(hits, at)?;
    if (0..=8).contains(&hit.layer) && display_windows.contains(&hit.host) {
        return Some(RealClickHit {
            window: hit.host,
            window_frame: hit.bounds,
            menu: None,
            ax_pid: hit.target.pid,
        });
    }
    if hit.layer == popup_menu {
        let owner = hits.iter().find(|w| {
            (0..=8).contains(&w.layer)
                && display_windows.contains(&w.host)
                && (w.host.pid == hit.host.pid || is_remote(hit.host.pid))
        })?;
        return Some(RealClickHit {
            window: owner.host,
            window_frame: owner.bounds,
            menu: Some((hit.host, hit.bounds)),
            ax_pid: hit.target.pid,
        });
    }
    None
}

/// Whether a press at a global point hits a list or panel, so Agent Screen
/// takes the front only after its release (`viewer_press`).
pub fn on_list(at: (f64, f64)) -> bool {
    // Pid and bounds are all the rule reads: no panel lookup, as this runs
    // on the main thread at every press.
    is_list(&hits_on_screen(), at)
}

/// Whether a press at `at` hits a list or panel of `list`'s windows, front
/// to back (`viewer_press`).
fn is_list(list: &[Hit], at: (f64, f64)) -> bool {
    use agent_screen_model::viewer_press::{Front, Window, front_for_press};
    let windows: Vec<Window> = list
        .iter()
        .map(|h| Window {
            pid: h.host.pid,
            bounds: h.bounds,
        })
        .collect();
    front_for_press(&windows, at) == Front::AfterRelease
}

/// Whether keys posted to app `pid` now stay off the user's screens: its
/// focused window, as Accessibility names it, is on an Agent Display.
/// Without a known focused window the keys may reach one of the user's,
/// as one on another Space or minimized, which the window list misses.
pub fn keys_stay_on_agent_displays(pid: i32) -> bool {
    super::ax::focused_window_id(pid).is_some_and(on_agent_display)
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
/// that too, as the app the user is clicking. The Viewer is its key window
/// by then (`viewer_input::pressed_viewer`): AppKit takes this for an
/// activation by a click, and without a key window it would wait for the
/// press, which the Viewer's monitor keeps, to make one.
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

/// WindowServer's front process, -1 if it can't say.
pub fn server_front() -> i32 {
    unsafe { SSFrontPid() }
}

/// Ask WindowServer to make Agent Screen its front process with its window
/// number `window`, key already, as the key window: the front hold's start.
/// Nothing counts Agent Screen as the front app yet: the hold does once
/// `server_front` confirms it.
pub fn take_front(window: u32) -> bool {
    unsafe { SSMakeSelfFront(window) }
}

/// Make `pid` WindowServer's front process with its window `window` key,
/// as the user's click on that window would: the front hold's hand-back.
pub fn give_front(pid: i32, window: u32) -> bool {
    unsafe { SSSetFrontProcess(pid, window) }
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

/// For a document window that draws its own title bar, a point 10 pt
/// outside its top-left corner. AppKit routes a click posted there to the
/// window by number and makes it key, but no view is there, and it is
/// beyond the 4 pt ring where a press resizes. Without it, AppKit drops the
/// next press as the window's first mouse, and every drag event after it
/// (custom-title-bar-first-press.md). Not for panels, lists and pop-ups:
/// they may close on a press outside them.
fn beside(target: WindowTarget, bounds: Rect) -> Option<(f64, f64)> {
    let document = super::windows::find(target.window_id).is_some_and(|w| w.layer == 0);
    (document && super::ax::is_standard_window(target))
        .then_some((bounds.x - 10.0, bounds.y - 10.0))
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
mod probe_tests {
    use super::*;

    #[test]
    fn the_press_stands_in_until_the_target_reports_its_caret() {
        let mut probe = Probe::default();
        assert!(probe.rect().is_none());
        probe.press((500.0, 300.0));
        let rect = probe.rect().unwrap();
        assert_eq!((rect.x, rect.y + rect.height / 2.0), (500.0, 300.0));
        let reported = Rect::new(120.0, 80.0, 1.0, 22.0);
        let ticket = probe.ask();
        assert!(probe.answer(ticket, Focus::Text, Some(reported)));
        assert_eq!(probe.rect(), Some(reported));
        assert_eq!(probe.focus, Focus::Text);
    }

    #[test]
    fn the_latest_lookup_wins_and_a_press_voids_older_ones() {
        let mut probe = Probe::default();
        let (first, second) = (probe.ask(), probe.ask());
        assert!(probe.answer(second, Focus::Secure, None));
        // The first came back late: it is out of date.
        assert!(!probe.answer(first, Focus::Text, None));
        assert_eq!(probe.focus, Focus::Secure);
        // A press after a lookup was asked for: its answer is out of date.
        let third = probe.ask();
        probe.press((1.0, 1.0));
        assert!(!probe.answer(third, Focus::Text, None));
        assert_eq!(probe.focus, Focus::Other);
        let fourth = probe.ask();
        assert!(probe.answer(fourth, Focus::Text, None));
    }

    #[test]
    fn the_focus_is_unknown_until_the_latest_lookup_answers() {
        let mut probe = Probe::default();
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        let (first, second) = (probe.ask(), probe.ask());
        // An older lookup's answer doesn't say where the focus is now.
        assert!(probe.answer(first, Focus::Text, None));
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        assert!(probe.answer(second, Focus::Secure, None));
        assert_eq!(probe.secrecy(), Secrecy::Secure);
        probe.press((1.0, 1.0));
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        // After a press, one look doesn't settle it; two that agree on the
        // field clicked do.
        let clicked = Seen {
            focus: Focus::Text,
            element: None,
            frame: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
        };
        let third = probe.ask();
        assert_eq!(probe.answer_look(third, clicked, None), Answered::Again);
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        let fourth = probe.ask();
        assert_eq!(probe.answer_look(fourth, clicked, None), Answered::Kept);
        assert_eq!(probe.secrecy(), Secrecy::Open);
    }

    #[test]
    fn the_second_look_runs_100_ms_after_the_key_at_the_soonest() {
        let ms = Duration::from_millis;
        assert_eq!(look_again_in(Some(ms(30))), ms(70));
        assert_eq!(look_again_in(Some(ms(80))), ms(40));
        assert_eq!(look_again_in(Some(ms(300))), ms(40));
        assert_eq!(look_again_in(None), ms(40));
    }

    #[test]
    fn after_a_click_the_old_field_away_from_it_must_wait() {
        let away = Seen {
            focus: Focus::Text,
            element: Some(1),
            frame: Some(Rect::new(0.0, 0.0, 100.0, 20.0)),
        };
        let mut probe = Probe::default();
        let first = probe.ask();
        probe.answer_look(first, away, None);
        // A click below the field: an app that moves its focus late still
        // shows the old field to both looks.
        probe.press((50.0, 200.0));
        for _ in 0..3 {
            let ticket = probe.ask_again();
            assert_eq!(probe.answer_look(ticket, away, None), Answered::Again);
        }
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        // It moves: settled.
        let ticket = probe.ask_again();
        let moved = Seen {
            element: Some(2),
            ..away
        };
        assert_eq!(probe.answer_look(ticket, moved, None), Answered::Kept);
        assert_eq!(probe.secrecy(), Secrecy::Open);
        // A click on a button leaves the focus: settled in the end.
        probe.press((50.0, 200.0));
        probe.moved_at = std::time::Instant::now().checked_sub(Duration::from_millis(
            agent_screen_model::viewer_keys::SETTLE_AWAY_MS,
        ));
        let ticket = probe.ask_again();
        assert_eq!(probe.answer_look(ticket, moved, None), Answered::Again);
        let ticket = probe.ask_again();
        assert_eq!(probe.answer_look(ticket, moved, None), Answered::Kept);
        assert_eq!(probe.secrecy(), Secrecy::Open);
    }

    #[test]
    fn an_unanswered_look_looks_again_until_keys_stop_waiting() {
        let mut probe = Probe::default();
        let ticket = probe.ask();
        assert!(probe.answer(ticket, Focus::Unanswered, None));
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        let ticket = probe.ask();
        assert_eq!(
            probe.answer_look(
                ticket,
                Seen {
                    focus: Focus::Unanswered,
                    ..Seen::default()
                },
                None
            ),
            Answered::Again
        );
        // Keys gave up waiting: no more looks until the next key or click.
        probe.chain_at = std::time::Instant::now().checked_sub(Duration::from_millis(
            agent_screen_model::viewer_keys::HOLD_MS,
        ));
        let ticket = probe.ask_again();
        assert_eq!(
            probe.answer_look(
                ticket,
                Seen {
                    focus: Focus::Unanswered,
                    ..Seen::default()
                },
                None
            ),
            Answered::Kept
        );
    }

    #[test]
    fn after_a_key_that_moves_the_focus_the_old_field_must_be_seen_twice() {
        let look = |focus, element| Seen {
            focus,
            element,
            frame: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
        };
        let mut probe = Probe::default();
        let first = probe.ask();
        probe.answer_look(first, look(Focus::Text, Some(1)), None);
        assert_eq!(probe.secrecy(), Secrecy::Open);
        // Tab: the app may not have moved its focus yet.
        probe.moved();
        let second = probe.ask();
        assert_eq!(
            probe.answer_look(second, look(Focus::Text, Some(1)), None),
            Answered::Again
        );
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
        // The look again finds the password field it moved to.
        let third = probe.ask();
        probe.answer_look(third, look(Focus::Secure, Some(2)), None);
        assert_eq!(probe.secrecy(), Secrecy::Secure);
        // A click into another field settles at once.
        probe.press((1.0, 1.0));
        let fourth = probe.ask();
        assert_eq!(
            probe.answer_look(fourth, look(Focus::Text, Some(3)), None),
            Answered::Kept
        );
        assert_eq!(probe.secrecy(), Secrecy::Open);
        // No answer leaves it unknown.
        let fifth = probe.ask();
        probe.answer_look(fifth, look(Focus::Unanswered, None), None);
        assert_eq!(probe.secrecy(), Secrecy::Unknown);
    }

    #[test]
    fn a_lookup_is_pending_until_it_answers_or_a_press_voids_it() {
        let mut probe = Probe::default();
        assert!(!probe.pending());
        let first = probe.ask();
        assert!(probe.pending());
        assert!(probe.answer(first, Focus::Text, None));
        assert!(!probe.pending());
        // A press voids the lookup in flight: its answer won't count, so
        // the next key asks again.
        probe.ask();
        probe.press((1.0, 1.0));
        assert!(!probe.pending());
    }
}

#[cfg(test)]
mod click_diagnostics_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn real_hit_observer_records_only_routing_metadata() {
        let target = WindowTarget {
            pid: 12,
            window_id: 34,
        };
        let hit = Hit {
            target,
            host: target,
            bounds: Rect::new(4680.0, -1981.0, 497.0, 182.0),
            layer: 8,
            content: None,
        };
        let rows = diagnostics::tests::capture(|| {
            diagnostics::within(Click::when(false), Phase::Down, || record_hit(Some(hit)));
            diagnostics::within(Click::when(true), Phase::Down, || record_hit(Some(hit)));
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["target"], json!({"pid": 12, "window_id": 34}));
        assert_eq!(rows[0]["layer"], 8);
        assert_eq!(rows[0]["bounds"], json!([4680.0, -1981.0, 497.0, 182.0]));
        assert_eq!(rows[0].as_object().unwrap().len(), 8);
        for key in ["text", "value", "title", "owner_name", "keys", "screenshot"] {
            assert!(rows[0].get(key).is_none());
        }
    }
}

#[cfg(test)]
mod remote_panel_tests {
    use super::*;
    use objc2_core_graphics::CGWindowListOption;
    use serde_json::Value;

    #[test]
    fn captured_remote_panel_keeps_window_local_coordinates() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/remote-panel-click.json"))
                .unwrap();
        let target = |name: &str| WindowTarget {
            pid: fixture[name]["pid"].as_i64().unwrap() as i32,
            window_id: fixture[name]["window_id"].as_u64().unwrap() as u32,
        };
        let b = fixture["bounds"].as_array().unwrap();
        let bounds = Rect::new(
            b[0].as_f64().unwrap(),
            b[1].as_f64().unwrap(),
            b[2].as_f64().unwrap(),
            b[3].as_f64().unwrap(),
        );
        let at = (
            fixture["global"][0].as_f64().unwrap(),
            fixture["global"][1].as_f64().unwrap(),
        );
        let window = |target| cg::WindowInfo {
            target,
            owner_name: "Fixture".into(),
            title: String::new(),
            bounds,
            layer: 0,
            alpha: 1.0,
        };
        let remote = window(target("remote"));
        let hits = hit_list(
            vec![window(target("host"))],
            vec![remote.clone()],
            24,
            25,
            101,
            |_| false,
        );
        let hit = hits.into_iter().find(|h| h.bounds.contains(at)).unwrap();
        assert_eq!(hit.host, target("host"));
        assert_eq!(hit.target, target("remote"));
        let mut fields = Vec::new();
        let mut written_local = None;
        let (resolved, local) = stamp_routing_using(
            at,
            hit.target,
            |id| {
                cg::window_bounds_using(id, Some(hit.target.pid), |option, relative| {
                    if option == CGWindowListOption::OptionAll {
                        vec![remote.clone()]
                    } else {
                        assert_eq!(option, CGWindowListOption::OptionIncludingWindow);
                        assert_eq!(relative, remote.target.window_id);
                        assert!(fixture["targeted_lookup_bounds"].is_null());
                        Vec::new()
                    }
                })
            },
            |field, value| fields.push((field, value)),
            |local| written_local = Some(local),
        );
        assert_eq!(fields, [(40, 6587), (51, 8440), (91, 8440), (92, 8440)]);
        assert_eq!(written_local, Some(local));
        assert_eq!(
            resolved,
            Some(bounds),
            "a matched remote panel must retain its geometry"
        );
        let expected = &fixture["expected_local"];
        assert!((local.0 - expected[0].as_f64().unwrap()).abs() < 0.000001);
        assert!((local.1 - expected[1].as_f64().unwrap()).abs() < 0.000001);
    }
}

#[cfg(test)]
mod popover_routing_tests {
    use super::*;
    use objc2_core_graphics::CGWindowListOption;

    const FINDER: WindowTarget = WindowTarget {
        pid: 100,
        window_id: 50,
    };
    const SHARING: WindowTarget = WindowTarget {
        pid: 200,
        window_id: 60,
    };
    const POPOVER: Rect = Rect::new(1000.0, 200.0, 320.0, 380.0);
    // The arrow takes 13 pt above the remote content.
    const CONTENT: Rect = Rect::new(1000.0, 213.0, 320.0, 367.0);

    fn window(target: WindowTarget, bounds: Rect, layer: i64) -> cg::WindowInfo {
        cg::WindowInfo {
            target,
            owner_name: "Fixture".into(),
            title: String::new(),
            bounds,
            layer,
            alpha: 1.0,
        }
    }

    fn popover_hit(layer: i64, is_remote: impl Fn(i32) -> bool) -> Hit {
        let list = hit_list(
            vec![window(FINDER, POPOVER, layer)],
            vec![window(SHARING, CONTENT, 3)],
            24,
            25,
            101,
            is_remote,
        );
        assert_eq!(list.len(), 1);
        list[0]
    }

    #[test]
    fn share_popover_click_goes_to_the_remote_view() {
        for layer in [0, 3] {
            let hit = popover_hit(layer, |p| p == SHARING.pid);
            assert_eq!(hit.host, FINDER);
            assert_eq!(hit.target, SHARING);
            assert_eq!(hit.bounds, POPOVER);
            assert_eq!(hit.content, Some(CONTENT));
        }
        let hit = popover_hit(0, |p| p == SHARING.pid);
        let at = (1100.0, 300.0);
        let mut fields = Vec::new();
        let (resolved, local) = stamp_routing_using(
            at,
            hit.target,
            |id| {
                cg::window_bounds_using(id, Some(hit.target.pid), |option, relative| {
                    assert_eq!(option, CGWindowListOption::OptionIncludingWindow);
                    assert_eq!(relative, SHARING.window_id);
                    vec![window(SHARING, CONTENT, 3)]
                })
            },
            |field, value| fields.push((field, value)),
            |_| {},
        );
        assert!(fields.contains(&(40, SHARING.pid as i64)));
        assert!(fields.contains(&(51, SHARING.window_id as i64)));
        assert_eq!(resolved, Some(CONTENT));
        assert!((local.0 - 100.0).abs() < 0.000001);
        assert!((local.1 - 87.0).abs() < 0.000001);
    }

    #[test]
    fn popup_menu_layer_host_is_not_retargeted_by_inset() {
        let hit = popover_hit(101, |p| p == SHARING.pid);
        assert_eq!(hit.target, FINDER);
    }

    #[test]
    fn remote_service_window_is_never_retargeted() {
        let hit = popover_hit(0, |p| p == FINDER.pid || p == SHARING.pid);
        assert_eq!(hit.target, FINDER);
    }

    #[test]
    fn retargeted_popover_is_not_an_app_window() {
        let hit = popover_hit(0, |p| p == SHARING.pid);
        let owners = std::collections::BTreeMap::new();
        assert_eq!(
            click_check_in(&[hit], 8, (1100.0, 300.0), &owners),
            Ok(None)
        );
    }

    #[test]
    fn the_arrow_is_finders_window_for_the_click_guard() {
        let list = [popover_hit(0, |p| p == SHARING.pid)];
        let owners = std::collections::BTreeMap::new();
        // On the arrow the guard sees Finder's window, as the press goes
        // there; on the content the click takes the forwarding path.
        assert_eq!(
            click_check_in(&list, 8, (1100.0, 205.0), &owners),
            Ok(Some((FINDER, POPOVER)))
        );
        assert_eq!(click_check_in(&list, 8, (1100.0, 213.5), &owners), Ok(None));
        assert_eq!(
            topmost_hit_in(&list, (1100.0, 213.5)).map(|h| h.target),
            Some(SHARING)
        );
    }

    #[test]
    fn real_click_reads_the_process_that_draws_the_point() {
        let list = [popover_hit(0, |p| p == SHARING.pid)];
        let at = |y| resolve_real_click(&list, &[FINDER], 101, |p| p == SHARING.pid, (1100.0, y));
        for (y, pid) in [
            (205.0, FINDER.pid),
            (212.5, FINDER.pid),
            (213.5, SHARING.pid),
        ] {
            let hit = at(y).unwrap();
            assert_eq!(hit.ax_pid, pid, "at y = {y}");
            assert_eq!(hit.window, FINDER);
            assert_eq!(hit.window_frame, POPOVER);
            assert_eq!(hit.menu, None);
        }
    }

    #[test]
    fn a_press_on_the_arrow_goes_to_finder_and_one_on_the_content_to_sharing() {
        let list = [popover_hit(0, |p| p == SHARING.pid)];
        let goes_to = |at| topmost_hit_in(&list, at).map(|h| h.target);
        // On the arrow, above the content's top edge at y = 213.
        assert_eq!(goes_to((1100.0, 205.0)), Some(FINDER));
        assert_eq!(goes_to((1100.0, 212.5)), Some(FINDER));
        assert_eq!(goes_to((1100.0, 213.5)), Some(SHARING));
        assert_eq!(goes_to((1100.0, 300.0)), Some(SHARING));
        assert!(
            topmost_hit_in(&list, (1100.0, 205.0))
                .unwrap()
                .content
                .is_none()
        );
    }

    #[test]
    fn a_click_on_a_window_covering_the_popover_stays_on_that_window() {
        const COVER: WindowTarget = WindowTarget {
            pid: 300,
            window_id: 70,
        };
        // Slightly larger than the popover, above it, and inside the inset
        // rule's 40 pt of the Share content.
        let cover = Rect::new(990.0, 190.0, 340.0, 400.0);
        let list = hit_list(
            vec![window(COVER, cover, 0), window(FINDER, POPOVER, 0)],
            vec![window(SHARING, CONTENT, 3)],
            24,
            25,
            101,
            |p| p == SHARING.pid,
        );
        assert_eq!(list[0].target, COVER);
        assert_eq!(list[0].content, None);
        assert_eq!(list[1].target, SHARING);
        assert_eq!(list[1].content, Some(CONTENT));
        assert_eq!(
            topmost_hit_in(&list, (1100.0, 300.0)).map(|h| h.target),
            Some(COVER)
        );
        // A file panel's exact window still gets its panel.
        let list = hit_list(
            vec![window(COVER, cover, 0), window(FINDER, POPOVER, 0)],
            vec![window(SHARING, POPOVER, 0)],
            24,
            25,
            101,
            |p| p == SHARING.pid,
        );
        assert_eq!(list[0].target, COVER);
        assert_eq!(list[1].target, SHARING);
        assert_eq!(list[1].content, None);
    }

    #[test]
    fn keys_after_a_click_on_the_arrow_go_to_finder() {
        let list = [popover_hit(0, |p| p == SHARING.pid)];
        // The click focuses what it hit, and the popover stays open.
        let clicked = topmost_hit_in(&list, (1100.0, 205.0)).unwrap().target;
        assert_eq!(clicked, FINDER);
        let keys = focused_hit(&list, Some(clicked)).unwrap();
        assert_eq!(keys.target, FINDER);
        assert_eq!(keys.host, FINDER);
        assert!(!keys.needs_wake(&[]));
    }

    #[test]
    fn keys_after_a_click_on_the_content_go_to_sharing() {
        let list = [popover_hit(0, |p| p == SHARING.pid)];
        let clicked = topmost_hit_in(&list, (1100.0, 300.0)).unwrap().target;
        assert_eq!(clicked, SHARING);
        let keys = focused_hit(&list, Some(clicked)).unwrap();
        assert_eq!(keys.target, SHARING);
        assert_eq!(keys.host, FINDER);
        assert_eq!(keys.content, Some(CONTENT));
        assert!(!keys.needs_wake(&[]));
    }

    #[test]
    fn keys_after_focusing_a_file_panels_app_window_go_to_the_panel() {
        let list = hit_list(
            vec![window(FINDER, POPOVER, 0)],
            vec![window(SHARING, POPOVER, 0)],
            24,
            25,
            101,
            |p| p == SHARING.pid,
        );
        assert_eq!(focused_hit(&list, Some(FINDER)).unwrap().target, SHARING);
        assert!(focused_hit(&list, None).is_none());
    }

    #[test]
    fn keys_on_an_unclicked_popover_send_no_wake_click() {
        let popover = popover_hit(0, |p| p == SHARING.pid);
        assert!(!popover.needs_wake(&[]));
        // A file panel at the app window's frame still gets its wake click,
        // once.
        let list = hit_list(
            vec![window(FINDER, POPOVER, 0)],
            vec![window(SHARING, POPOVER, 0)],
            24,
            25,
            101,
            |p| p == SHARING.pid,
        );
        let panel = list[0];
        assert_eq!(panel.target, SHARING);
        assert!(panel.content.is_none());
        assert!(panel.needs_wake(&[]));
        assert!(!panel.needs_wake(&[SHARING.window_id]));
    }

    #[test]
    fn a_scroll_near_the_content_bottom_lands_inside_it() {
        let list = [popover_hit(0, |p| p == SHARING.pid)];
        // 5 pt above the bottom edge, which the popover and its content share.
        let at = (1100.0, 575.0);
        let hit = topmost_hit_in(&list, at).unwrap();
        assert_eq!(hit.target, SHARING);
        for routed in [Some(CONTENT), None] {
            let local = hit.scroll_location(at, routed);
            assert!((local.0 - 100.0).abs() < 0.000001);
            assert!((local.1 - 362.0).abs() < 0.000001);
            assert!(local.1 < CONTENT.height);
        }
        // An ordinary window keeps its window-relative point.
        let finder = Hit {
            target: FINDER,
            host: FINDER,
            bounds: POPOVER,
            layer: 0,
            content: None,
        };
        assert_eq!(finder.scroll_location(at, None), (100.0, 375.0));
        assert_eq!(finder.scroll_location(at, Some(POPOVER)), (100.0, 375.0));
    }
}

#[cfg(test)]
mod routing_bounds_tests {
    use super::*;
    use objc2_core_graphics::CGWindowListOption;

    fn window(target: WindowTarget, bounds: Rect) -> cg::WindowInfo {
        cg::WindowInfo {
            target,
            bounds,
            layer: 0,
            alpha: 1.0,
            owner_name: "Fixture".into(),
            title: String::new(),
        }
    }

    #[test]
    fn ordinary_hit_and_mismatched_remote_keep_their_original_target() {
        let host = window(
            WindowTarget {
                pid: 12,
                window_id: 34,
            },
            Rect::new(4000.0, -2000.0, 497.0, 182.0),
        );
        let mut remote = window(
            WindowTarget {
                pid: 13,
                window_id: 35,
            },
            host.bounds,
        );
        remote.bounds.width += 2.0;
        for panels in [Vec::new(), vec![remote]] {
            // An ordinary/non-XPC process is not in the remote-panel list.
            let list = hit_list(vec![host.clone()], panels, 24, 25, 101, |_| false);
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].target, host.target);
            assert_eq!(list[0].bounds, host.bounds);
        }
    }

    #[test]
    fn targeted_frame_wins_and_does_not_scan_the_full_list() {
        let target = WindowTarget {
            pid: 13,
            window_id: 35,
        };
        let expected = Rect::new(4000.0, -2000.0, 497.0, 182.0);
        let mut calls = 0;
        let got = cg::window_bounds_using(target.window_id, Some(target.pid), |option, id| {
            calls += 1;
            assert_eq!(option, CGWindowListOption::OptionIncludingWindow);
            assert_eq!(id, target.window_id);
            vec![window(target, expected)]
        });
        assert_eq!(got, Some(expected));
        assert_eq!(calls, 1);
    }

    #[test]
    fn moved_remote_uses_its_current_frame_not_the_host_or_an_old_frame() {
        let target = WindowTarget {
            pid: 13,
            window_id: 35,
        };
        let moved = Rect::new(4200.0, -1800.0, 497.0, 182.0);
        let host = window(
            WindowTarget {
                pid: 12,
                window_id: 34,
            },
            Rect::new(4000.0, -2000.0, 497.0, 182.0),
        );
        let (bounds, local) = stamp_routing_using(
            (4645.0, -1639.0),
            target,
            |id| {
                cg::window_bounds_using(id, Some(target.pid), |option, relative| {
                    if option == CGWindowListOption::OptionIncludingWindow {
                        Vec::new()
                    } else {
                        assert_eq!(relative, 0);
                        vec![host.clone(), window(target, moved)]
                    }
                })
            },
            |_, _| {},
            |_| {},
        );
        assert_eq!(bounds, Some(moved));
        assert_eq!(local, (445.0, 161.0));
    }

    #[test]
    fn missing_remote_does_not_borrow_a_host_or_reused_window_id() {
        let target = WindowTarget {
            pid: 13,
            window_id: 35,
        };
        let other = window(
            WindowTarget {
                pid: 14,
                window_id: 35,
            },
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        let host = window(
            WindowTarget {
                pid: 12,
                window_id: 34,
            },
            Rect::new(4000.0, -2000.0, 497.0, 182.0),
        );
        let at = (4445.0, -1839.0);
        let (bounds, local) = stamp_routing_using(
            at,
            target,
            |id| {
                cg::window_bounds_using(id, Some(target.pid), |option, _| {
                    if option == CGWindowListOption::OptionIncludingWindow {
                        vec![other.clone()]
                    } else {
                        vec![host.clone(), other.clone()]
                    }
                })
            },
            |_, _| {},
            |_| {},
        );
        assert_eq!(bounds, None);
        // This is the pre-existing missing-window behavior, not a claim
        // that sending global-as-local to a vanished target is safe.
        assert_eq!(local, at);
    }

    #[test]
    fn ordinary_bounds_queries_do_not_scan_offscreen_windows() {
        let mut queries = Vec::new();
        assert_eq!(
            cg::window_bounds_using(8440, None, |option, relative| {
                queries.push((option, relative));
                Vec::new()
            }),
            None
        );
        assert_eq!(queries, [(CGWindowListOption::OptionIncludingWindow, 8440)]);
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    #[test]
    fn reported_scroll_amounts_reach_unposted_event_fields_without_clamping() {
        for amount in [3, 60, 200, 400, 800] {
            let event = scroll_event(
                None,
                (100.0, 100.0),
                (0.0, -f64::from(amount)),
                false,
                0,
                0,
                CGEventFlags::empty(),
            )
            .unwrap();
            let read = |field| CGEvent::integer_value_field(Some(&event), field);
            assert_eq!(
                read(CGEventField::ScrollWheelEventDeltaAxis1),
                -i64::from(amount)
            );
            assert_eq!(
                read(CGEventField::ScrollWheelEventPointDeltaAxis1),
                -i64::from(amount) * 10
            );
            assert_eq!(
                read(CGEventField::ScrollWheelEventFixedPtDeltaAxis1),
                -i64::from(amount) * 65536
            );
            assert_eq!(read(CGEventField::ScrollWheelEventDeltaAxis2), 0);
            assert_eq!(read(CGEventField::ScrollWheelEventIsContinuous), 0);
        }
    }
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn a_takeover_ends_the_typeahead_wait_at_once() {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let takeover = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            flag.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        assert!(!wait_typeahead(&stopped));
        takeover.join().unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn the_typeahead_wait_outlasts_a_search() {
        let start = Instant::now();
        assert!(wait_typeahead(&AtomicBool::new(false)));
        assert!(start.elapsed() >= Duration::from_millis(1200));
    }

    #[test]
    fn drag_and_real_click_accept_session_popup_menus_and_sheets_only_on_the_display() {
        let area = Rect::new(4288.0, -800.0, 1280.0, 800.0);
        let sheet = WindowTarget {
            pid: 100,
            window_id: 201,
        };
        let appex_menu = WindowTarget {
            pid: 300,
            window_id: 401,
        };
        let stranger_menu = WindowTarget {
            pid: 999,
            window_id: 902,
        };
        let sheet_rect = Rect::new(4500.0, -700.0, 600.0, 400.0);
        let menu_rect = Rect::new(4600.0, -600.0, 200.0, 180.0);
        let hits = vec![
            Hit {
                host: appex_menu,
                target: appex_menu,
                bounds: menu_rect,
                layer: 101,
                content: None,
            },
            Hit {
                host: sheet,
                target: WindowTarget {
                    pid: 300,
                    window_id: 301,
                },
                bounds: sheet_rect,
                layer: 8,
                content: None,
            },
        ];
        let at_menu = (4650.0, -520.0);
        let at_sheet = (4900.0, -520.0);

        // #205 Expected #3: drag opening a pop-up menu (same pid or `.appex`
        // remote service) keeps seeing `drop` on the display.
        assert_eq!(
            resolve_drag_under(
                &hits,
                (sheet, sheet),
                area,
                (500, 101),
                |p| p == 300,
                at_menu
            ),
            Some(sheet)
        );
        assert_eq!(
            resolve_drag_under(&hits, (sheet, sheet), area, (500, 101), |_| false, at_sheet),
            Some(sheet)
        );
        // An unrelated non-remote app's menu does not pass.
        let stranger_hits = vec![
            Hit {
                host: stranger_menu,
                target: stranger_menu,
                bounds: menu_rect,
                layer: 101,
                content: None,
            },
            hits[1],
        ];
        assert_eq!(
            resolve_drag_under(
                &stranger_hits,
                (sheet, sheet),
                area,
                (500, 101),
                |_| false,
                at_menu
            ),
            None
        );

        // #205 Expected #2: real_click on an open pop-up menu resolves to the
        // owning display window + menu frame, and rejects unrelated menus.
        assert_eq!(
            resolve_real_click(&hits, &[sheet], 101, |p| p == 300, at_menu),
            Some(RealClickHit {
                window: sheet,
                window_frame: sheet_rect,
                menu: Some((appex_menu, menu_rect)),
                ax_pid: 300,
            })
        );
        assert_eq!(
            resolve_real_click(&stranger_hits, &[sheet], 101, |_| false, at_menu),
            None
        );

        // #207 + L7: native menus only probe their own PIDs, and `.appex`
        // remote-service menus add only the frontmost adjacent host window PID,
        // skipping unrelated side windows (`999`) and background windows (`888`)
        // behind the host window.
        let behind_host = Hit {
            host: WindowTarget {
                pid: 888,
                window_id: 888,
            },
            target: WindowTarget {
                pid: 888,
                window_id: 888,
            },
            bounds: sheet_rect,
            layer: 0,
            content: None,
        };
        let bystander = Hit {
            host: WindowTarget {
                pid: 999,
                window_id: 999,
            },
            target: WindowTarget {
                pid: 999,
                window_id: 999,
            },
            bounds: Rect::new(4700.0, -500.0, 200.0, 200.0),
            layer: 0,
            content: None,
        };
        let native_menu = Hit {
            host: sheet,
            target: sheet,
            bounds: menu_rect,
            layer: 101,
            content: None,
        };
        let all_windows = vec![hits[0], bystander, hits[1], behind_host];
        assert_eq!(
            resolve_menu_pids(&native_menu, &all_windows, Some(area), |_| false),
            vec![100]
        );
        assert_eq!(
            resolve_menu_pids(&hits[0], &all_windows, Some(area), |p| p == 300),
            vec![300, 100]
        );
    }

    #[test]
    fn hit_routing_follows_visual_z_order_and_agent_check_refuses_foreign_top_window() {
        let pt = (4100.0, -500.0);
        let foreign_top = Hit {
            host: WindowTarget {
                pid: 10,
                window_id: 101,
            },
            target: WindowTarget {
                pid: 10,
                window_id: 101,
            },
            bounds: Rect::new(4000.0, -600.0, 400.0, 300.0),
            layer: 0,
            content: None,
        };
        let own_below = Hit {
            host: WindowTarget {
                pid: 20,
                window_id: 202,
            },
            target: WindowTarget {
                pid: 20,
                window_id: 202,
            },
            bounds: Rect::new(3950.0, -650.0, 500.0, 400.0),
            layer: 0,
            content: None,
        };
        let mut owners = std::collections::BTreeMap::new();
        owners.insert(101, 7);
        owners.insert(202, 8);

        // 1. Foreign window 101 (recorded on 7) visibly covers 202 (recorded on 8) on display 8:
        //    - Viewer `topmost_hit_in` delivers to what the user sees on top (101), never clicking through to 202!
        //    - Agent `foreign_recorded_top_window_in` refuses on display 8 naming owner 7!
        let stack_foreign_on_top = [foreign_top, own_below];
        assert_eq!(
            topmost_hit_in(&stack_foreign_on_top, pt).map(|h| h.target.window_id),
            Some(101)
        );
        assert_eq!(
            foreign_recorded_top_window_in(&stack_foreign_on_top, 8, pt, &owners),
            Some((foreign_top.host, 7))
        );

        // 2. When display 8's own window 202 is visibly on top of 101, agent input on 8 is allowed!
        let stack_own_on_top = [own_below, foreign_top];
        assert_eq!(
            topmost_hit_in(&stack_own_on_top, pt).map(|h| h.target.window_id),
            Some(202)
        );
        assert_eq!(
            foreign_recorded_top_window_in(&stack_own_on_top, 8, pt, &owners),
            None
        );
    }

    #[test]
    fn viewer_drag_guard_scopes_pause_to_watched_window_only() {
        assert!(!viewer_watching_window(777_001));
        assert!(!viewer_watching_window(777_002));
        {
            let _g1 = ViewerDragGuard::new(777_001);
            assert!(viewer_watching_window(777_001));
            assert!(!viewer_watching_window(777_002));
            {
                let _g2 = ViewerDragGuard::new(777_001);
                assert!(viewer_watching_window(777_001));
            }
            assert!(viewer_watching_window(777_001));
        }
        assert!(!viewer_watching_window(777_001));

        // Leaked guards auto-expire after VIEWER_DRAG_GUARD_TTL (L7).
        let t0 = Instant::now();
        let _leaked = ViewerDragGuard::new_at(777_003, t0);
        assert!(viewer_watching_window_at(
            777_003,
            t0 + Duration::from_secs(9)
        ));
        assert!(!viewer_watching_window_at(
            777_003,
            t0 + VIEWER_DRAG_GUARD_TTL
        ));
    }

    #[test]
    fn panel_cache_invalidates_on_layout_change_or_ttl() {
        let host = Hit {
            host: WindowTarget {
                pid: 10,
                window_id: 100,
            },
            target: WindowTarget {
                pid: 10,
                window_id: 100,
            },
            bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
            layer: 0,
            content: None,
        };
        let moved = Hit {
            bounds: Rect::new(10.0, 0.0, 800.0, 600.0),
            ..host
        };
        let t0 = Instant::now();
        let entry: PanelCacheEntry = (panel_keys(&[host]), t0, Vec::new());
        assert!(panel_cache_valid(
            &entry,
            &panel_keys(&[host]),
            t0 + Duration::from_millis(100)
        ));
        assert!(!panel_cache_valid(
            &entry,
            &panel_keys(&[moved]),
            t0 + Duration::from_millis(10)
        ));
        assert!(!panel_cache_valid(
            &entry,
            &panel_keys(&[host]),
            t0 + PANEL_CACHE_TTL
        ));
    }

    #[test]
    fn click_check_in_refuses_foreign_and_returns_owned_top_window() {
        let pt = (150.0, 150.0);
        let foreign = Hit {
            host: WindowTarget {
                pid: 11,
                window_id: 101,
            },
            target: WindowTarget {
                pid: 11,
                window_id: 101,
            },
            bounds: Rect::new(100.0, 100.0, 300.0, 200.0),
            layer: 0,
            content: None,
        };
        let own = Hit {
            host: WindowTarget {
                pid: 22,
                window_id: 202,
            },
            target: WindowTarget {
                pid: 22,
                window_id: 202,
            },
            bounds: Rect::new(100.0, 100.0, 300.0, 200.0),
            layer: 0,
            content: None,
        };
        let mut owners = std::collections::BTreeMap::new();
        owners.insert(101, 7);
        owners.insert(202, 8);

        assert_eq!(
            click_check_in(&[foreign, own], 8, pt, &owners),
            Err((foreign.host, 7))
        );
        assert_eq!(
            click_check_in(&[own, foreign], 8, pt, &owners),
            Ok(Some((own.target, own.bounds)))
        );
    }
}
