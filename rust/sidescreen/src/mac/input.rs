//! Input for windows on an Agent Display, from the Viewer or an Agent
//! Session. Events go straight to the owning process through SkyLight, so
//! the real cursor stays where the user left it and the user's frontmost app
//! keeps its place in the window stack (ADR 0001).
//!
//! Each display has one worker thread. The focus handshake needs short
//! sleeps, and window list lookups stay off the main thread.

use super::cg;
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

/// One display's input queue. Cheap to clone.
#[derive(Clone)]
pub struct Forwarder {
    jobs: mpsc::Sender<Job>,
}

impl Default for Forwarder {
    fn default() -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("sidescreen-input".into())
            .spawn(move || {
                let mut worker = Worker::new();
                while let Ok(job) = rx.recv() {
                    job(&mut worker);
                }
            })
            .expect("input thread");
        Forwarder { jobs }
    }
}

impl Forwarder {
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
            w.mouse_down(at, button, clicks, flags);
        });
    }

    pub fn mouse_dragged(&self, to: (f64, f64), button: Button, flags: CGEventFlags) {
        self.send(move |w| w.mouse_dragged(to, button, flags));
    }

    pub fn mouse_up(&self, at: (f64, f64), button: Button, clicks: i64, flags: CGEventFlags) {
        self.send(move |w| w.mouse_up(at, button, clicks, flags));
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

    pub fn key(&self, code: u16, down: bool, flags: CGEventFlags) {
        self.send(move |w| w.key(code, down, flags));
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
                hit = w.mouse_down(at, button, index, flags(m));
                hit?;
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
    pub async fn drag(&self, from: (f64, f64), to: (f64, f64), m: Modifiers) -> bool {
        self.run(move |w| {
            if w.mouse_down(from, Button::Left, 1, flags(m)).is_none() {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
            const STEPS: i32 = 20;
            for step in 1..=STEPS {
                let t = step as f64 / STEPS as f64;
                w.mouse_dragged(
                    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t),
                    Button::Left,
                    flags(m),
                );
                thread::sleep(Duration::from_millis(16));
            }
            thread::sleep(Duration::from_millis(100));
            w.mouse_up(to, Button::Left, 1, flags(m));
            true
        })
        .await
        .unwrap_or(false)
    }

    /// Half a click, for drags an agent drives step by step. The window hit
    /// by the press, or None when there is nothing under the point.
    pub async fn button(&self, at: (f64, f64), down: bool, m: Modifiers) -> Option<WindowTarget> {
        self.run(move |w| {
            if down {
                w.mouse_down(at, Button::Left, 1, flags(m))
            } else {
                let target = w.gesture.as_ref().map(|g| g.target);
                w.mouse_up(at, Button::Left, 1, flags(m));
                target
            }
        })
        .await
        .flatten()
    }

    pub async fn move_to(&self, to: (f64, f64)) {
        self.run(move |w| w.mouse_moved(to, CGEventFlags::empty()))
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
    pub async fn type_text(&self, text: String, within: Rect) -> bool {
        self.run(move |w| {
            w.focus_something(within);
            let Some(target) = w.focused else {
                return false;
            };
            for character in text.chars() {
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
            true
        })
        .await
        .unwrap_or(false)
    }

    /// Send keys to `target` from now on, as a click on it would.
    pub async fn focus(&self, target: WindowTarget) {
        self.run(move |w| w.focus(target)).await;
    }

    /// Hold a key chord down for `held`, for apps that act on held keys.
    /// False when no window on the display can take the keys.
    pub async fn hold(&self, chord: KeyChord, held: Duration, within: Rect) -> bool {
        self.run(move |w| {
            w.focus_something(within);
            if w.focused.is_none() {
                return false;
            }
            w.key(chord.code, true, flags(chord.modifiers));
            thread::sleep(held);
            w.key(chord.code, false, flags(chord.modifiers));
            true
        })
        .await
        .unwrap_or(false)
    }

    /// False when no window on the display can take the keys.
    pub async fn press(&self, chord: KeyChord, count: i64, within: Rect) -> bool {
        self.run(move |w| {
            w.focus_something(within);
            if w.focused.is_none() {
                return false;
            }
            for _ in 0..count.max(1) {
                w.key(chord.code, true, flags(chord.modifiers));
                thread::sleep(Duration::from_millis(20));
                w.key(chord.code, false, flags(chord.modifiers));
            }
            true
        })
        .await
        .unwrap_or(false)
    }
}

struct Gesture {
    target: WindowTarget,
    group: i64,
}

struct Worker {
    source: Option<CFRetained<CGEventSource>>,
    focused: Option<WindowTarget>,
    gesture: Option<Gesture>,
    cache: Option<(Vec<Hit>, Instant)>,
    seed: u64,
}

#[derive(Clone, Copy)]
struct Hit {
    target: WindowTarget,
    bounds: Rect,
}

impl Worker {
    fn new() -> Self {
        Worker {
            source: CGEventSource::new(CGEventSourceStateID::HIDSystemState),
            focused: None,
            gesture: None,
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
    fn focus(&mut self, target: WindowTarget) {
        if self.focused != Some(target) {
            // Let AppKit in the target update its key-window routing before
            // events arrive (cua uses the same 50 ms settle).
            if unsafe { SSFocusWithoutRaise(target.pid, target.window_id) } {
                thread::sleep(Duration::from_millis(50));
            }
            self.focused = Some(target);
        }
    }

    fn mouse_down(
        &mut self,
        at: (f64, f64),
        button: Button,
        clicks: i64,
        flags: CGEventFlags,
    ) -> Option<WindowTarget> {
        let Some(hit) = self.hit(at, true) else {
            log::info!("input: no window at {at:?}");
            return None;
        };
        let target = hit.target;
        self.focus(target);
        let group = self.next_group();
        self.gesture = Some(Gesture { target, group });
        self.post(CGEventType::MouseMoved, at, target, button, 0, group, flags);
        self.post(button.down(), at, target, button, clicks, group, flags);
        Some(target)
    }

    fn mouse_dragged(&mut self, to: (f64, f64), button: Button, flags: CGEventFlags) {
        let Some(g) = &self.gesture else { return };
        let (target, group) = (g.target, g.group);
        self.post(button.dragged(), to, target, button, 1, group, flags);
    }

    fn mouse_up(&mut self, at: (f64, f64), button: Button, clicks: i64, flags: CGEventFlags) {
        let Some(g) = self.gesture.take() else { return };
        self.post(button.up(), at, g.target, button, clicks, g.group, flags);
    }

    fn mouse_moved(&mut self, to: (f64, f64), flags: CGEventFlags) {
        // Between an agent's left_mouse_down and left_mouse_up a move is a
        // drag. The Viewer sends its own dragged events and no moves then.
        if self.gesture.is_some() {
            self.mouse_dragged(to, Button::Left, flags);
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

    fn key(&mut self, code: u16, down: bool, flags: CGEventFlags) {
        let Some(target) = self.focused else { return };
        let Some(event) = CGEvent::new_keyboard_event(self.source.as_deref(), code, down) else {
            return;
        };
        CGEvent::set_flags(Some(&event), flags);
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
    fn focus_something(&mut self, within: Rect) {
        let list = hits();
        let still_there = self
            .focused
            .and_then(|t| list.iter().find(|h| h.target == t));
        if still_there.is_some_and(|h| within.intersects(&h.bounds)) {
            return;
        }
        if let Some(top) = list.iter().find(|h| within.contains(h.bounds.mid())) {
            self.focus(top.target);
        }
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
    unsafe {
        SSEventSetIntegerField(event, 40, target.pid as i64); // target pid
        SSEventSetIntegerField(event, 51, window); // window number
        SSEventSetIntegerField(event, 91, window); // window under pointer
        SSEventSetIntegerField(event, 92, window); // window under pointer that can handle it
        SSEventSetWindowLocation(event, CGPoint::new(at.0, at.1));
    }
}

fn deliver(event: &CGEvent, pid: i32, authenticate: bool) {
    if !unsafe { SSPostEventToPid(pid, event, authenticate) } {
        CGEvent::post_to_pid(pid, Some(event));
    }
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
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| {
            w.layer >= 0
                && w.layer != menu
                && w.layer != status
                && w.owner_name != "Window Server"
                && w.owner_name != "Dock"
                && w.alpha > 0.0
        })
        .map(|w| Hit {
            target: w.target,
            bounds: w.bounds,
        })
        .collect()
}

/// A key chord through the public API, for apps that need no focus
/// handshake: asking a running app for a new window.
pub fn post_key_to_pid(pid: i32, chord: KeyChord) {
    for down in [true, false] {
        if let Some(event) = CGEvent::new_keyboard_event(None, chord.code, down) {
            CGEvent::set_flags(Some(&event), flags(chord.modifiers));
            CGEvent::post_to_pid(pid, Some(&event));
        }
    }
}
