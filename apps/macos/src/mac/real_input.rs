//! The Input Queue's executor (ADR 0013): one complete drag with the
//! user's own pointer, during an Input Grant the user allowed. Events go to
//! the HID system, so the pointer really moves and the session's left
//! button really reads down, as Chromium needs. Each event carries `TAG`,
//! which tells it from the user's own input: while armed, any untagged
//! mouse or key event is the user taking control back.

use super::native::{self, Native};
use crate::model::WindowTarget;
use objc2_app_kit::NSEvent;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, CGPoint, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMouseButton,
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

/// Serialize background input with a real-input loan into the same
/// process. Other processes keep their own independent gates.
pub fn process_gate(pid: i32) -> std::sync::Arc<std::sync::RwLock<()>> {
    static GATES: std::sync::LazyLock<
        std::sync::Mutex<std::collections::BTreeMap<i32, std::sync::Weak<std::sync::RwLock<()>>>>,
    > = std::sync::LazyLock::new(Default::default);
    let mut gates = GATES.lock().unwrap_or_else(|e| e.into_inner());
    gates.retain(|_, gate| gate.strong_count() != 0);
    if let Some(gate) = gates.get(&pid).and_then(std::sync::Weak::upgrade) {
        return gate;
    }
    let gate = std::sync::Arc::new(std::sync::RwLock::new(()));
    gates.insert(pid, std::sync::Arc::downgrade(&gate));
    gate
}

/// Keyboard Borrow and a real-input loan use the same session input and
/// cannot overlap, even if their target processes differ.
pub static SESSION_INPUT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// "ASQ!" in the event's source user data.
pub const TAG: i64 = 0x4153_5121;
/// Points per drag step: small enough for apps that start a drag only after
/// a few points of movement.
const STEP: f64 = 4.0;

static ARMED: AtomicBool = AtomicBool::new(false);
static TOUCHED: AtomicBool = AtomicBool::new(false);
/// A missing or disabled observer prevents real input.
static WATCHING: AtomicBool = AtomicBool::new(false);
/// A physical press of the user's takes precedence over releasing our
/// synthetic button. Never synthesize an up over a physical down.
static USER_LEFT_DOWN: AtomicBool = AtomicBool::new(false);
/// The app a loan is pressing in, 0 for none. The guard that gives the
/// front back to the user's app and the reminder leave it alone.
static HOLDING: AtomicI32 = AtomicI32::new(0);

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
    fn CGEventSourceButtonState(state: CGEventSourceStateID, button: u32) -> bool;
    fn CGEventSourceFlagsState(state: CGEventSourceStateID) -> CGEventFlags;
}

/// Watch for the user's own input from now on, or stop. Either clears a
/// touch from an earlier grant.
pub fn arm(on: bool) {
    TOUCHED.store(false, Ordering::SeqCst);
    ARMED.store(on, Ordering::SeqCst);
}

pub fn watching() -> bool {
    WATCHING.load(Ordering::SeqCst)
}

pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// The user touched the mouse or keyboard while armed.
pub fn touched() -> bool {
    TOUCHED.load(Ordering::SeqCst) || !watching()
}

/// Whether the executor may move the pointer now: during a grant it
/// watches, or while a loan presses.
pub fn in_use() -> bool {
    armed() || HOLDING.load(Ordering::Relaxed) != 0
}

pub fn holding(pid: i32) -> bool {
    pid != 0 && HOLDING.load(Ordering::Relaxed) == pid
}

/// An event this module posted.
pub fn is_ours(event: &NSEvent) -> bool {
    event.CGEvent().is_some_and(|e| {
        CGEvent::integer_value_field(Some(&e), CGEventField::EventSourceUserData) == TAG
    })
}

thread_local! {
    // Keep the listen-only tap and its main-run-loop source alive.
    static WATCH: std::cell::RefCell<Option<(CFRetained<CFMachPort>, CFRetained<CFRunLoopSource>)>> = const { std::cell::RefCell::new(None) };
}

/// Observe HID events before AppKit manufactures pointer moves when a
/// window opens under the cursor. ListenOnly neither suppresses nor
/// changes an event. Failure to create the tap leaves execution disabled.
pub fn watch() {
    unsafe extern "C-unwind" fn seen(
        _proxy: CGEventTapProxy,
        kind: CGEventType,
        event: NonNull<CGEvent>,
        _context: *mut std::ffi::c_void,
    ) -> *mut CGEvent {
        let disabled = matches!(
            kind,
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
        );
        if disabled {
            WATCHING.store(false, Ordering::SeqCst);
        }
        let ours = CGEvent::integer_value_field(
            Some(unsafe { event.as_ref() }),
            CGEventField::EventSourceUserData,
        ) == TAG;
        if !ours {
            if kind == CGEventType::LeftMouseDown {
                USER_LEFT_DOWN.store(true, Ordering::SeqCst);
            } else if kind == CGEventType::LeftMouseUp {
                USER_LEFT_DOWN.store(false, Ordering::SeqCst);
            }
            if ARMED.load(Ordering::SeqCst) && !TOUCHED.swap(true, Ordering::SeqCst) {
                native::send(Native::UserTookInput);
            }
        }
        // A listen-only callback never replaces the original event.
        event.as_ptr()
    }
    let kinds = [
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
        CGEventType::ScrollWheel,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ];
    let mask = kinds.iter().fold(0, |mask, kind| mask | (1_u64 << kind.0));
    let Some(tap) = (unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::HIDEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            mask,
            Some(seen),
            std::ptr::null_mut(),
        )
    }) else {
        log::warn!("input queue: input observer unavailable; real input is disabled");
        return;
    };
    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
        return;
    };
    let Some(run_loop) = CFRunLoop::main() else {
        return;
    };
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    CGEvent::tap_enable(&tap, true);
    WATCH.with(|watch| *watch.borrow_mut() = Some((tap, source)));
    WATCHING.store(true, Ordering::SeqCst);
}

/// The pointer and front app at grant start, to put back when it ends
/// normally.
#[derive(Clone, Copy, Debug)]
pub struct Saved {
    pub pointer: (f64, f64),
    pub front: Option<i32>,
}

pub fn save() -> Saved {
    let pointer = CGEvent::new(None)
        .map(|e| CGEvent::location(Some(&e)))
        .map_or((0.0, 0.0), |p| (p.x, p.y));
    Saved {
        pointer,
        front: super::app::frontmost_pid(),
    }
}

/// Put the pointer and front app back. Not after the user took control:
/// that would undo what they just did.
pub fn restore(saved: Saved) {
    log::info!("input queue: restoring the pointer and front app");
    if touched() {
        return;
    }
    unsafe {
        CGWarpMouseCursorPosition(CGPoint::new(saved.pointer.0, saved.pointer.1));
        CGAssociateMouseAndMouseCursorPosition(true);
    }
    if !touched()
        && let Some(front) = saved.front.filter(|&p| p != std::process::id() as i32)
    {
        // Do not wait and activate again: the user may act during a wait.
        super::ax::set_attribute(
            &super::ax::application(front),
            "AXFrontmost",
            objc2_core_foundation::CFBoolean::new(true),
        );
    }
}

/// While AeroSpace runs: how long after a loan's AX writes it keeps
/// touching the claim file, and how long the window must then keep its
/// frame before anything is pressed.
const TOUCHING: Duration = Duration::from_millis(500);
const SETTLED: Duration = Duration::from_millis(300);
/// How long a loan waits for all that before it gives up.
const SETTLE_LIMIT: Duration = Duration::from_millis(1500);

fn write_main(target: WindowTarget) {
    let key = objc2_core_foundation::CFString::from_static_str("AXMain");
    let yes = objc2_core_foundation::CFBoolean::new(true);
    if let Some(window) = super::ax::window_element(target) {
        unsafe { window.set_attribute_value(&key, yes) };
    }
}

fn write_front(pid: i32) {
    let key = objc2_core_foundation::CFString::from_static_str("AXFrontmost");
    let yes = objc2_core_foundation::CFBoolean::new(true);
    unsafe { super::ax::application(pid).set_attribute_value(&key, yes) };
}

/// Make the loan's window main and its app frontmost, and keep the window
/// where it was. The caller holds the process's input gate, so the writes
/// here skip it. `go` says the loan may go on.
///
/// The AX writes focus this window only: NSRunningApplication activation
/// can make a window manager bring the app onto the user's workspace. Even
/// so, AeroSpace refreshes when an app activates, and that refresh lays
/// out the user's workspaces before it reads the claim file. An app it has
/// never seen in front, such as an accessory app, gets its focused window
/// filed in the user's focused workspace there, and laid out on their
/// screen before AeroSpace lets go of it as claimed (issue #121). Touching
/// the claim file starts a refresh that reads the claims first, and that
/// one replaces the refresh for the activation. If the window moves all
/// the same, it goes back to where it was before anything is pressed;
/// claimed, AeroSpace leaves it alone from then on.
pub fn come_forward(target: WindowTarget, go: impl Fn() -> bool) -> Result<(), &'static str> {
    let home = super::cg::window_bounds(target.window_id);
    let aerospace = super::app::is_running("bobko.aerospace");
    let element = super::ax::window_element(target);
    write_main(target);
    write_front(target.pid);
    let asked = std::time::Instant::now();
    let mut still = asked;
    loop {
        if !go() {
            return Err("the approval ended before the drag began");
        }
        if aerospace && asked.elapsed() < TOUCHING {
            super::claims::touch();
        }
        if super::app::frontmost_pid() != Some(target.pid) {
            if asked.elapsed() > Duration::from_secs(1) {
                return Err("the app didn't come to the front");
            }
        } else if !aerospace {
            return Ok(());
        } else if let Some(home) = home
            && super::cg::window_bounds(target.window_id) != Some(home)
        {
            log::info!(
                "input queue: window {} moved as its app came forward; putting it back",
                target.window_id
            );
            if !element
                .as_ref()
                .is_some_and(|w| super::ax::set_frame_holding_gate(w, home))
            {
                return Err("the window moved off the Agent Display and couldn't be put back");
            }
            still = std::time::Instant::now();
        } else if asked.elapsed() >= TOUCHING && still.elapsed() >= SETTLED {
            return Ok(());
        }
        if asked.elapsed() > SETTLE_LIMIT {
            return Err("a window manager kept moving the window off the Agent Display");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// A loan's focus step alone, for `debugLoanFocus`: whether the window
/// moved while its app came forward, whether it was back once the step
/// returned and stayed for `watch`, and whether other windows moved. The
/// app that was in front comes back afterwards.
pub fn debug_forward(target: WindowTarget, watch: Duration) -> serde_json::Value {
    let gate = process_gate(target.pid);
    let _process = gate.write().unwrap_or_else(|e| e.into_inner());
    let before = super::cg::window_bounds(target.window_id);
    let others = |frames: Vec<(u32, i64, crate::model::Rect)>| {
        frames
            .into_iter()
            .filter(|(id, layer, _)| *layer == 0 && *id != target.window_id)
            .map(|(id, _, frame)| (id, frame))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let others_before = others(super::cg::on_screen_frames());
    let front = super::app::frontmost_pid();
    let start = std::time::Instant::now();
    let moved = std::sync::Mutex::new((None::<u64>, 0_u32));
    let watcher = std::sync::atomic::AtomicBool::new(true);
    let (came, ready_at, moved_after) = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut away = false;
            while watcher.load(Ordering::Relaxed) {
                let off = super::cg::window_bounds(target.window_id) != before;
                if off && !away {
                    let mut m = moved.lock().unwrap();
                    m.0.get_or_insert(start.elapsed().as_millis() as u64);
                    m.1 += 1;
                }
                away = off;
                thread::sleep(Duration::from_millis(2));
            }
        });
        let came = come_forward(target, || true);
        let ready_at = start.elapsed().as_millis() as u64;
        let mut moved_after = None;
        while start.elapsed() < watch {
            if moved_after.is_none() && super::cg::window_bounds(target.window_id) != before {
                moved_after = Some(start.elapsed().as_millis() as u64);
            }
            thread::sleep(Duration::from_millis(5));
        }
        watcher.store(false, Ordering::Relaxed);
        (came, ready_at, moved_after)
    });
    let after = super::cg::window_bounds(target.window_id);
    let mut others_after = others(super::cg::on_screen_frames());
    let changed: Vec<u32> = others_before
        .iter()
        .filter(|(id, frame)| others_after.remove(*id).is_some_and(|f| f != **frame))
        .map(|(id, _)| *id)
        .collect();
    if let Some(front) = front.filter(|&p| p != target.pid) {
        write_front(front);
    }
    let (moved_at, trips) = *moved.lock().unwrap();
    serde_json::json!({
        "before": before.map(|r| [r.x, r.y, r.width, r.height]),
        "after": after.map(|r| [r.x, r.y, r.width, r.height]),
        "came": came.err(),
        "readyAtMs": ready_at,
        "movedAtMs": moved_at,
        "trips": trips,
        "movedAfterReadyMs": moved_after,
        "othersChanged": changed,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Nothing was pressed.
    Refused(&'static str),
    /// Stopped partway; the button the loan pressed was let go.
    Stopped(&'static str),
}

struct Poster {
    source: Option<objc2_core_foundation::CFRetained<CGEventSource>>,
    flags: CGEventFlags,
}

impl Poster {
    fn event(&self, kind: CGEventType, at: (f64, f64)) -> Option<CFRetained<CGEvent>> {
        let event = CGEvent::new_mouse_event(
            self.source.as_deref(),
            kind,
            CGPoint::new(at.0, at.1),
            CGMouseButton::Left,
        )?;
        CGEvent::set_flags(Some(&event), self.flags);
        CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUserData, TAG);
        if matches!(kind, CGEventType::LeftMouseDown | CGEventType::LeftMouseUp) {
            CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventClickState, 1);
        }
        Some(event)
    }

    fn post(&self, kind: CGEventType, at: (f64, f64)) -> bool {
        let Some(event) = self.event(kind, at) else {
            return false;
        };
        super::ime::note_posted();
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        true
    }
}

/// Allocate the release before pressing, and keep it armed through unwinds.
/// A physical button the user just pressed is theirs to release.
struct Release {
    event: CFRetained<CGEvent>,
    at: (f64, f64),
    pressed: bool,
}

impl Drop for Release {
    fn drop(&mut self) {
        if self.pressed && !USER_LEFT_DOWN.load(Ordering::SeqCst) {
            let at = if touched() {
                CGEvent::new(None)
                    .map(|e| CGEvent::location(Some(&e)))
                    .map_or(self.at, |p| (p.x, p.y))
            } else {
                self.at
            };
            CGEvent::set_location(Some(&self.event), CGPoint::new(at.0, at.1));
            super::ime::note_posted();
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&self.event));
        }
        HOLDING.store(0, Ordering::Relaxed);
    }
}

/// Drag from `from` to `to`, global points, with the user's pointer.
/// `go` is checked before every event, with the user's touch: a loan the
/// queue stopped, or a window that left the display, ends it. The button
/// goes up before this returns, however it ends.
pub fn drag(
    target: WindowTarget,
    from: (f64, f64),
    to: (f64, f64),
    flags: CGEventFlags,
    go: impl Fn((f64, f64)) -> bool,
) -> Outcome {
    let Ok(_session) = SESSION_INPUT.try_lock() else {
        return Outcome::Refused("another operation holds the session's keyboard or pointer");
    };
    let gate = process_gate(target.pid);
    let Ok(_process) = gate.try_write() else {
        return Outcome::Refused("background input into this process is still running");
    };
    if unsafe { IsSecureEventInputEnabled() } {
        return Outcome::Refused("secure input is on, as for a password field");
    }
    if USER_LEFT_DOWN.load(Ordering::SeqCst)
        || (0..3).any(|button| unsafe {
            CGEventSourceButtonState(CGEventSourceStateID::CombinedSessionState, button)
        })
        || unsafe { CGEventSourceFlagsState(CGEventSourceStateID::CombinedSessionState) }.0
            & (CGEventFlags::MaskShift
                | CGEventFlags::MaskControl
                | CGEventFlags::MaskAlternate
                | CGEventFlags::MaskCommand)
                .0
            != 0
    {
        return Outcome::Refused("a mouse button or modifier is already held");
    }
    let Some(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return Outcome::Refused("the input event source could not be created");
    };
    let poster = Poster {
        source: Some(source),
        flags,
    };
    let allowed = |at| go(at) && !touched() && !unsafe { IsSecureEventInputEnabled() };
    if !allowed(from) {
        return Outcome::Refused("the approval ended before the drag began");
    }
    // A click on an app in the background only brings it forward, and the
    // page never sees the press: the app comes to the front first. The
    // grant's end puts the user's app back.
    HOLDING.store(target.pid, Ordering::Relaxed);
    if let Err(why) = come_forward(target, || allowed(from)) {
        HOLDING.store(0, Ordering::Relaxed);
        return Outcome::Refused(why);
    }
    if !poster.post(CGEventType::MouseMoved, from) {
        HOLDING.store(0, Ordering::Relaxed);
        return Outcome::Refused("the mouse event could not be created");
    }
    thread::sleep(Duration::from_millis(60));
    if !allowed(from) || super::app::frontmost_pid() != Some(target.pid) {
        HOLDING.store(0, Ordering::Relaxed);
        return Outcome::Refused("the approval ended before the drag began");
    }
    let Some(up) = poster.event(CGEventType::LeftMouseUp, from) else {
        HOLDING.store(0, Ordering::Relaxed);
        return Outcome::Refused("the release event could not be created before pressing");
    };
    let mut release = Release {
        event: up,
        at: from,
        pressed: false,
    };
    if !poster.post(CGEventType::LeftMouseDown, from) {
        HOLDING.store(0, Ordering::Relaxed);
        return Outcome::Refused("the press could not be created");
    }
    release.pressed = true;
    thread::sleep(Duration::from_millis(80));
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let steps = ((dx.hypot(dy) / STEP).ceil() as usize).clamp(8, 400);
    let mut at = from;
    let mut stopped = None;
    for i in 1..=steps {
        if !allowed(at) || super::app::frontmost_pid() != Some(target.pid) {
            stopped = Some(if touched() {
                "the user moved the mouse or pressed a key"
            } else if super::app::frontmost_pid() != Some(target.pid) {
                "the foreground app changed"
            } else {
                "the approval or target geometry changed"
            });
            break;
        }
        let t = i as f64 / steps as f64;
        at = (from.0 + dx * t, from.1 + dy * t);
        release.at = at;
        if !allowed(at) {
            stopped = Some(if touched() {
                "the user moved the mouse or pressed a key"
            } else {
                "the approval or target geometry changed"
            });
            break;
        }
        if !poster.post(CGEventType::LeftMouseDragged, at) {
            stopped = Some("a drag event could not be created");
            break;
        }
        thread::sleep(Duration::from_millis(8));
    }
    if stopped.is_none() {
        thread::sleep(Duration::from_millis(60));
    }
    if stopped.is_none() && (!allowed(at) || super::app::frontmost_pid() != Some(target.pid)) {
        stopped = Some("the approval ended before release");
    }
    drop(release);
    thread::sleep(Duration::from_millis(40));
    match stopped {
        Some(why) => Outcome::Stopped(why),
        None => Outcome::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loan_fences_only_its_own_process() {
        let a = process_gate(101001);
        let peer = process_gate(101001);
        let b = process_gate(101002);
        let loan = a.write().unwrap();
        assert!(peer.try_read().is_err());
        assert!(b.try_read().is_ok());
        drop(loan);
        assert!(peer.try_read().is_ok());
    }

    #[test]
    fn a_background_call_blocks_a_loan_until_it_finishes() {
        let background = process_gate(101003);
        let loan = process_gate(101003);
        let held = background.read().unwrap();
        assert!(loan.try_write().is_err());
        drop(held);
        assert!(loan.try_write().is_ok());
    }
}
