//! The Input Queue's executor (ADR 0013): one complete drag, click or
//! menu item with the user's own pointer, during an Input Grant the user
//! allowed. Events go to the HID system, so the pointer really moves and
//! the session's button really reads down, as Chromium needs. Each event
//! carries `TAG`, which tells it from the user's own input: while armed,
//! any untagged mouse or key event is the user taking control back.

use super::native::{self, Native};
use crate::model::{Rect, WindowTarget};
use agent_screen_model::menu_press;
use agent_screen_model::real_click::{self, Event};
use objc2_app_kit::NSEvent;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, CGPoint, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMouseButton,
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
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
/// Half the points per move of the pointer's glide to a menu: small enough
/// for menu tracking to follow it.
const STEP: f64 = 4.0;

static ARMED: AtomicBool = AtomicBool::new(false);
static TOUCHED: AtomicBool = AtomicBool::new(false);
/// A missing or disabled observer prevents real input.
static WATCHING: AtomicBool = AtomicBool::new(false);
/// A physical press of the user's takes precedence over releasing our
/// synthetic button. Never synthesize an up over a physical down. One bit
/// per button: left, right, other.
static USER_DOWN: AtomicU8 = AtomicU8::new(0);
const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const OTHER: u8 = 4;

fn user_holds(bits: u8) -> bool {
    USER_DOWN.load(Ordering::SeqCst) & bits != 0
}
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

fn observer_disabled() {
    WATCHING.store(false, Ordering::SeqCst);
    if ARMED.load(Ordering::SeqCst) && !TOUCHED.swap(true, Ordering::SeqCst) {
        native::send(Native::UserTookInput);
    }
}

/// Actual tap health, read on the main thread where the tap is retained.
pub fn observer_enabled() -> bool {
    WATCH.with(|watch| {
        watch
            .borrow()
            .as_ref()
            .is_some_and(|(tap, _)| CGEvent::tap_is_enabled(tap))
    })
}

/// Recover only after the shell has revoked the old grant and finished
/// releasing its inputs. This restores observation, never permission.
pub fn recover_observer() {
    if watching() {
        return;
    }
    let enabled = WATCH.with(|watch| {
        let watch = watch.borrow();
        let Some((tap, _)) = watch.as_ref() else {
            return false;
        };
        CGEvent::tap_enable(tap, true);
        CGEvent::tap_is_enabled(tap)
    });
    WATCHING.store(enabled, Ordering::SeqCst);
    if enabled {
        log::info!("input queue: input observer recovered; a new approval is required");
    }
}

/// E2E-only control hook: disable the real tap and take its timeout path.
/// The control handler gates this behind debug_methods and the e2e instance.
pub fn debug_observer_timeout() {
    WATCH.with(|watch| {
        if let Some((tap, _)) = watch.borrow().as_ref() {
            CGEvent::tap_enable(tap, false);
        }
    });
    observer_disabled();
}

/// Note one event of the user's: the buttons they hold, and that they took
/// input while a grant is armed. The Viewer's private press calls it for
/// the presses it takes before this observer could see them.
pub fn user_event(kind: CGEventType) {
    let (down, bit) = match kind {
        CGEventType::LeftMouseDown => (Some(true), LEFT),
        CGEventType::LeftMouseUp => (Some(false), LEFT),
        CGEventType::RightMouseDown => (Some(true), RIGHT),
        CGEventType::RightMouseUp => (Some(false), RIGHT),
        CGEventType::OtherMouseDown => (Some(true), OTHER),
        CGEventType::OtherMouseUp => (Some(false), OTHER),
        _ => (None, 0),
    };
    match down {
        Some(true) => {
            USER_DOWN.fetch_or(bit, Ordering::SeqCst);
        }
        Some(false) => {
            USER_DOWN.fetch_and(!bit, Ordering::SeqCst);
        }
        None => {}
    }
    if ARMED.load(Ordering::SeqCst) && !TOUCHED.swap(true, Ordering::SeqCst) {
        native::send(Native::UserTookInput);
    }
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
            observer_disabled();
            return event.as_ptr();
        }
        let ours = CGEvent::integer_value_field(
            Some(unsafe { event.as_ref() }),
            CGEventField::EventSourceUserData,
        ) == TAG;
        if !ours {
            user_event(kind);
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
    let enabled = CGEvent::tap_is_enabled(&tap);
    WATCH.with(|watch| *watch.borrow_mut() = Some((tap, source)));
    WATCHING.store(enabled, Ordering::SeqCst);
}

/// The pointer, front app and its focused window at grant start, to put
/// back when it ends normally.
#[derive(Clone, Copy, Debug)]
pub struct Saved {
    pub pointer: (f64, f64),
    pub front: Option<i32>,
    /// The front app's focused window. A loan in the same app, as the
    /// agent's Chrome window beside the user's, makes its own window main,
    /// and the user's typing would go there.
    pub window: Option<u32>,
    /// Its main window, which differs when a panel has focus: a panel
    /// can't become main, and the document window it serves must.
    pub main: Option<u32>,
}

pub fn save() -> Saved {
    let pointer = CGEvent::new(None)
        .map(|e| CGEvent::location(Some(&e)))
        .map_or((0.0, 0.0), |p| (p.x, p.y));
    let front = super::app::frontmost_pid();
    let theirs = front.filter(|&p| p != std::process::id() as i32);
    let window = theirs.and_then(super::ax::focused_window_id);
    let main = theirs.and_then(|pid| {
        let app = super::ax::application(pid);
        unsafe { app.set_messaging_timeout(0.1) };
        super::ax::window_id(&*super::ax::element_attribute(&app, "AXMainWindow")?)
    });
    Saved {
        pointer,
        front,
        window,
        main,
    }
}

/// Put the pointer and front app back. Not after the user took control:
/// that would undo what they just did.
pub fn restore(saved: Saved) {
    let now = CGEvent::new(None).map(|e| CGEvent::location(Some(&e)));
    log::info!(
        "input queue: restoring the pointer and front app; pointer at {:?}",
        now.map(|p| (p.x, p.y))
    );
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
        // Their windows first, so the app comes forward with them main
        // and focused, and their keys go back there. The main window, then
        // a focused panel apart from it, which can't become main but can
        // take focus.
        let target = |window_id| WindowTarget {
            pid: front,
            window_id,
        };
        if let Some(main) = saved.main
            && !super::ax::make_main(target(main))
        {
            log::info!("input queue: window {main} could not be made main again");
        }
        if let Some(focused) = saved.window.filter(|&w| Some(w) != saved.main) {
            let focused_again = super::ax::window_element(target(focused)).is_some_and(|w| {
                super::ax::set_attribute(
                    &w,
                    "AXFocused",
                    objc2_core_foundation::CFBoolean::new(true),
                ) || super::ax::set_attribute(
                    &w,
                    "AXMain",
                    objc2_core_foundation::CFBoolean::new(true),
                )
            });
            if !focused_again {
                log::info!("input queue: window {focused} could not be focused again");
            }
        }
        // Some apps take AXFrontmost and stay behind, as Dia did behind
        // the loan's app; the user's touch stops the wait.
        super::ime::return_front_unless(front, touched);
    }
}

/// While AeroSpace runs: how long after a loan's AX writes it keeps
/// touching the claim file, and how long the window must then keep its
/// frame before anything is pressed.
const TOUCHING: Duration = Duration::from_millis(500);
const SETTLED: Duration = Duration::from_millis(300);
/// How long a loan waits for all that before it gives up.
const SETTLE_LIMIT: Duration = Duration::from_millis(1500);

/// Whether `target`'s app is in front with `target` its main window.
fn already_forward(target: WindowTarget) -> bool {
    if super::app::frontmost_pid() != Some(target.pid) {
        return false;
    }
    let app = super::ax::application(target.pid);
    unsafe { app.set_messaging_timeout(0.1) };
    super::ax::element_attribute(&app, "AXMainWindow").and_then(|w| super::ax::window_id(&w))
        == Some(target.window_id)
}

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
///
/// A window already main in the app in front, as after an earlier loan of
/// the same grant, needs none of this: nothing activates, so no window
/// manager lays anything out.
pub fn come_forward(target: WindowTarget, go: impl Fn() -> bool) -> Result<(), &'static str> {
    if already_forward(target) {
        return if go() {
            Ok(())
        } else {
            Err("the approval ended before anything was pressed")
        };
    }
    let home = super::cg::window_bounds(target.window_id);
    let aerospace = super::app::is_running("bobko.aerospace");
    let element = super::ax::window_element(target);
    write_main(target);
    write_front(target.pid);
    let asked = std::time::Instant::now();
    let mut still = asked;
    loop {
        if !go() {
            return Err("the approval ended before anything was pressed");
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
        self.mouse(kind, at, CGMouseButton::Left, 1)
    }

    /// A mouse event of `button`; a press or release carries click state
    /// `state`, 2 for the second press of a double click.
    fn mouse(
        &self,
        kind: CGEventType,
        at: (f64, f64),
        button: CGMouseButton,
        state: i64,
    ) -> Option<CFRetained<CGEvent>> {
        let event = CGEvent::new_mouse_event(
            self.source.as_deref(),
            kind,
            CGPoint::new(at.0, at.1),
            button,
        )?;
        CGEvent::set_flags(Some(&event), self.flags);
        CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUserData, TAG);
        if matches!(
            kind,
            CGEventType::LeftMouseDown
                | CGEventType::LeftMouseUp
                | CGEventType::RightMouseDown
                | CGEventType::RightMouseUp
                | CGEventType::OtherMouseDown
                | CGEventType::OtherMouseUp
        ) {
            CGEvent::set_integer_value_field(
                Some(&event),
                CGEventField::MouseEventClickState,
                state,
            );
        }
        Some(event)
    }

    fn post(&self, kind: CGEventType, at: (f64, f64)) -> bool {
        self.send(self.event(kind, at))
    }

    fn send(&self, event: Option<CFRetained<CGEvent>>) -> bool {
        let Some(event) = event else {
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
    /// The button's bit in USER_DOWN.
    button: u8,
}

impl Drop for Release {
    fn drop(&mut self) {
        if self.pressed && !user_holds(self.button) {
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
    }
}

/// While a loan works in an app: the guard that gives the front back to
/// the user's app, and the one that keeps the cursor off Agent Displays,
/// leave it alone until this drops.
struct Holding;

impl Holding {
    fn new(pid: i32) -> Self {
        HOLDING.store(pid, Ordering::Relaxed);
        Holding
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        HOLDING.store(0, Ordering::Relaxed);
    }
}

/// Input the user holds, or secure input: a loan may not start.
fn inputs_busy() -> Option<&'static str> {
    if unsafe { IsSecureEventInputEnabled() } {
        return Some("secure input is on, as for a password field");
    }
    let held = user_holds(LEFT | RIGHT | OTHER)
        || (0..3).any(|button| unsafe {
            CGEventSourceButtonState(CGEventSourceStateID::CombinedSessionState, button)
        })
        || unsafe { CGEventSourceFlagsState(CGEventSourceStateID::CombinedSessionState) }.0
            & (CGEventFlags::MaskShift
                | CGEventFlags::MaskControl
                | CGEventFlags::MaskAlternate
                | CGEventFlags::MaskCommand)
                .0
            != 0;
    held.then_some("a mouse button or modifier is already held")
}

/// The pace of a drag with the user's pointer, after its window came
/// forward. The pointer rests on the press point for a frame, so the app
/// there sees it arrive before the press.
const HOVER: Duration = Duration::from_millis(16);
/// The button stays down this long before the first move: apps that read
/// the session's button state, as Chromium does, see it held.
const PRESS_HOLD: Duration = Duration::from_millis(30);
/// Between two moves: about two moves per frame, which the app coalesces.
const STEP_GAP: Duration = Duration::from_millis(8);
/// After the release, before the loan ends and anything else may move the
/// front app, so the drop reaches its window first.
const AFTER_RELEASE: Duration = Duration::from_millis(40);
/// How long a drag between windows waits, once past the drag threshold,
/// for the source to start its system drag session. The session follows
/// the real pointer, so moves made before it starts carry nothing.
const SESSION_WAIT: Duration = Duration::from_millis(500);

/// How many drags the system drag pasteboard has carried: a source that
/// starts a system drag session writes its items there first. The loan's
/// thread has no autorelease pool of its own, so the read gets one.
fn drag_board() -> isize {
    objc2::rc::autoreleasepool(|_| {
        unsafe {
            objc2_app_kit::NSPasteboard::pasteboardWithName(objc2_app_kit::NSPasteboardNameDrag)
        }
        .changeCount()
    })
}

/// Drag from `from` to `to`, global points, with the user's pointer, from
/// `target`'s window to `drop`, framed `drop_frame`, which may be a window
/// of another app. The pointer is over the drop window for
/// `real_drag::OVER_DROP_MS` before the release, so a web page there has
/// heard the drag and answered. `go`
/// is checked before every event, with the user's touch: a loan the queue
/// stopped, or a window that left the display, ends it. It gets exactly
/// `from` at the press and exactly `to` at the release. The button goes up
/// before this returns, however it ends.
pub fn drag(
    target: WindowTarget,
    drop: WindowTarget,
    drop_frame: Rect,
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
    // Background input into the drop app waits too.
    let drop_gate = (drop.pid != target.pid).then(|| process_gate(drop.pid));
    let _drop_process = match &drop_gate {
        Some(gate) => match gate.try_write() {
            Ok(held) => Some(held),
            Err(_) => {
                return Outcome::Refused(
                    "background input into the drop window's app is still running",
                );
            }
        },
        None => None,
    };
    if let Some(why) = inputs_busy() {
        return Outcome::Refused(why);
    }
    let Some(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return Outcome::Refused("the input event source could not be created");
    };
    let poster = Poster {
        source: Some(source),
        flags,
    };
    let began = std::time::Instant::now();
    let checks = std::cell::Cell::new((0u32, Duration::ZERO));
    let allowed = |at| {
        let t = std::time::Instant::now();
        let ok = go(at) && !touched() && !unsafe { IsSecureEventInputEnabled() };
        let (n, spent) = checks.get();
        checks.set((n + 1, spent + t.elapsed()));
        ok
    };
    const BEFORE_PRESS: &str = "nothing was pressed: before the press, the approval ended, the user used their pointer or keyboard, secure input came on, or a window moved or now covers the drag's start or end";
    if !allowed(from) {
        return Outcome::Refused(BEFORE_PRESS);
    }
    // A click on an app in the background only brings it forward, and the
    // page never sees the press: the app comes to the front first. The
    // grant's end puts the user's app back.
    let _holding = Holding::new(target.pid);
    if let Err(why) = come_forward(target, || allowed(from)) {
        return Outcome::Refused(why);
    }
    let forward = began.elapsed();
    if !poster.post(CGEventType::MouseMoved, from) {
        return Outcome::Refused("the mouse event could not be created");
    }
    thread::sleep(HOVER);
    if !allowed(from) || super::app::frontmost_pid() != Some(target.pid) {
        return Outcome::Refused(BEFORE_PRESS);
    }
    let Some(up) = poster.event(CGEventType::LeftMouseUp, from) else {
        return Outcome::Refused("the release event could not be created before pressing");
    };
    let mut release = Release {
        event: up,
        at: from,
        pressed: false,
        button: LEFT,
    };
    // Only a drag between windows needs a system drag session.
    let between = drop != target;
    let board = between.then(drag_board);
    if !poster.post(CGEventType::LeftMouseDown, from) {
        return Outcome::Refused("the press could not be created");
    }
    release.pressed = true;
    let pressed = began.elapsed();
    thread::sleep(PRESS_HOLD);
    let points = agent_screen_model::real_drag::travel(from, to, drop_frame);
    let mut over: Option<std::time::Instant> = None;
    let wait_after = between
        .then(|| agent_screen_model::real_drag::session_step(from, &points))
        .flatten();
    let mut session: Option<Duration> = None;
    let mut at = from;
    let mut stopped = None;
    for (i, &next) in points.iter().enumerate() {
        if super::app::frontmost_pid() != Some(target.pid) {
            stopped = Some("the foreground app changed");
            break;
        }
        if !allowed(next) {
            stopped = Some(if touched() {
                "the user moved the mouse or pressed a key"
            } else {
                "the approval or target geometry changed"
            });
            break;
        }
        if !poster.post(CGEventType::LeftMouseDragged, next) {
            stopped = Some("a drag event could not be created");
            break;
        }
        at = next;
        release.at = at;
        if over.is_none() && drop_frame.contains(at) {
            over = Some(std::time::Instant::now());
        }
        thread::sleep(STEP_GAP);
        if session.is_none()
            && let Some(board) = board
        {
            // Past the threshold, the rest of the travel waits for the
            // session, which may take the source a moment to start.
            let limit = if wait_after == Some(i) {
                SESSION_WAIT
            } else {
                Duration::ZERO
            };
            let waiting = std::time::Instant::now();
            loop {
                if drag_board() != board {
                    session = Some(began.elapsed() - pressed);
                    break;
                }
                if waiting.elapsed() >= limit || touched() {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
    let travelled = began.elapsed();
    if stopped.is_none() {
        let so_far = over.map_or(0, |t| t.elapsed().as_millis() as u64);
        let rest = agent_screen_model::real_drag::rest_before_release(so_far);
        thread::sleep(Duration::from_millis(rest));
    }
    if stopped.is_none() && (!allowed(at) || super::app::frontmost_pid() != Some(target.pid)) {
        stopped = Some("the approval ended before release");
    }
    std::mem::drop(release);
    let released = began.elapsed();
    let over_ms = over.map_or(0, |t| t.elapsed().as_millis());
    thread::sleep(AFTER_RELEASE);
    let (count, spent) = checks.get();
    let ms = |d: Duration| d.as_millis();
    log::info!(
        "input queue: drag of {:.0} pt took {} ms: window forward {} ms, press at {} ms, {} moves until {} ms, release at {} ms after {over_ms} ms over the drop window; {count} checks took {} ms; {}",
        (to.0 - from.0).hypot(to.1 - from.1),
        ms(began.elapsed()),
        ms(forward),
        ms(pressed),
        points.len(),
        ms(travelled),
        ms(released),
        ms(spent),
        match (between, session) {
            (false, _) => "within one window".to_string(),
            (true, Some(after)) => format!("drag session began {} ms after the press", ms(after)),
            (true, None) => "no drag session seen".to_string(),
        }
    );
    match stopped {
        Some(why) => Outcome::Stopped(why),
        None => Outcome::Done,
    }
}

/// One complete click at `at`, global points, with the user's pointer
/// (ADR 0013, "Clicks"): the window comes forward as for a drag, the
/// pointer goes to the point, and each press is released before the next.
/// `go` is checked before every event, with the user's touch, secure input
/// and the app still in front; `still`, that the element at the point is
/// the one the agent aimed at, once the window is forward. Every button
/// pressed goes up before this returns, however it ends.
pub fn click_at(
    target: WindowTarget,
    at: (f64, f64),
    click: real_click::Click,
    flags: CGEventFlags,
    go: impl Fn((f64, f64)) -> bool,
    still: impl Fn() -> bool,
) -> Outcome {
    let Ok(_session) = SESSION_INPUT.try_lock() else {
        return Outcome::Refused("another operation holds the session's keyboard or pointer");
    };
    let gate = process_gate(target.pid);
    let Ok(_process) = gate.try_write() else {
        return Outcome::Refused("background input into this process is still running");
    };
    if let Some(why) = inputs_busy() {
        return Outcome::Refused(why);
    }
    let Some(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return Outcome::Refused("the input event source could not be created");
    };
    let poster = Poster {
        source: Some(source),
        flags,
    };
    let (button, bit, down, up) = match click.button {
        real_click::Button::Left => (
            CGMouseButton::Left,
            LEFT,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
        ),
        real_click::Button::Right => (
            CGMouseButton::Right,
            RIGHT,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
        ),
        real_click::Button::Middle => (
            CGMouseButton::Center,
            OTHER,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
        ),
    };
    let pid = target.pid;
    let front = || super::app::frontmost_pid() == Some(pid);
    let allowed = |at| go(at) && !touched() && !unsafe { IsSecureEventInputEnabled() };
    if !allowed(at) {
        return Outcome::Refused("the approval ended before the click");
    }
    // An app in the background takes a click as a click on its window to
    // bring it forward: the app comes to the front first. The grant's end
    // puts the user's app back.
    let _holding = Holding::new(pid);
    if let Err(why) = come_forward(target, || allowed(at)) {
        return Outcome::Refused(why);
    }
    // Under AeroSpace the window takes a second to settle: a list could
    // reorder meanwhile. The element at the point is checked once more.
    if !still() {
        return Outcome::Refused(real_click::CONTENT_CHANGED);
    }
    let mut release: Option<Release> = None;
    let mut pressed = false;
    for event in click.events() {
        if !allowed(at) || !front() {
            let why = if touched() {
                "the user moved the mouse or pressed a key"
            } else if !front() {
                "the foreground app changed"
            } else {
                "the approval or target geometry changed"
            };
            drop(release.take());
            return if pressed {
                Outcome::Stopped(why)
            } else {
                Outcome::Refused(why)
            };
        }
        match event {
            Event::Move => {
                if !poster.post(CGEventType::MouseMoved, at) {
                    return Outcome::Refused("the mouse event could not be created");
                }
                thread::sleep(Duration::from_millis(60));
            }
            Event::Down(state) => {
                let Some(event) = poster.mouse(up, at, button, state) else {
                    let why = "the release event could not be created before pressing";
                    return if pressed {
                        Outcome::Stopped(why)
                    } else {
                        Outcome::Refused(why)
                    };
                };
                let held = release.insert(Release {
                    event,
                    at,
                    pressed: false,
                    button: bit,
                });
                if !poster.send(poster.mouse(down, at, button, state)) {
                    return if pressed {
                        Outcome::Stopped("the press could not be created")
                    } else {
                        Outcome::Refused("the press could not be created")
                    };
                }
                held.pressed = true;
                pressed = true;
                thread::sleep(Duration::from_millis(real_click::HOLD_MS));
            }
            Event::Up(_) => {
                drop(release.take());
                thread::sleep(Duration::from_millis(real_click::GAP_MS));
            }
        }
    }
    Outcome::Done
}

/// How long a menu gets to open, or a submenu to show, after the click or
/// hover that opens it.
const MENU_OPENS: Duration = Duration::from_millis(1000);

/// The frame of an open menu. A closed one reports none, or no size.
fn open_frame(menu: &super::ax::Element) -> Option<Rect> {
    super::ax::frame(menu).filter(|f| f.width > 0.0 && f.height > 0.0)
}

/// The frame of a menu open on the Agent Display. A closed submenu of an
/// Electron app reports a 10×10 frame at the main screen's bottom left,
/// so a size alone doesn't say it opened (#39).
fn open_on(menu: &super::ax::Element, area: Rect) -> Option<Rect> {
    open_frame(menu).filter(|f| menu_press::on_display(*f, area))
}

/// Close whatever the loan opened, with accessibility: never a key.
fn close_menu(title: &super::ax::Element) {
    let Some(menu) = super::ax::submenu(title) else {
        return;
    };
    if open_frame(&menu).is_none() {
        return;
    }
    let cancel = objc2_core_foundation::CFString::from_static_str("AXCancel");
    // The loan holds the process's input gate, so this skips it.
    let error = unsafe { menu.perform_action(&cancel) };
    log::info!("input queue: closed the menu with AXCancel ({error:?})");
}

/// Where the Agent Display's menu bar shows `title`, read from where
/// accessibility places it on its own screen.
fn title_on_display(title: &super::ax::Element, area: Rect) -> Option<(f64, f64)> {
    let frame = super::ax::frame(title)?;
    let screen = super::cg::active_displays()
        .into_iter()
        .map(super::cg::display_bounds)
        .find(|b| b.contains(frame.mid()))?;
    let at = menu_press::title_point(frame, screen, area);
    log::info!(
        "input queue: menu bar title at {frame:?} on screen {screen:?}; on the display at {at:?}"
    );
    at
}

/// Whether `pid`'s menu bar title `name` is what the screen shows at `at`.
fn title_there(pid: i32, name: &str, at: (f64, f64)) -> bool {
    let seen = super::ax::on_screen_at(at);
    let ok = seen.as_ref().is_some_and(|(owner, element)| {
        *owner == pid
            && super::ax::string_attribute(element, "AXRole").as_deref() == Some("AXMenuBarItem")
            && super::ax::string_attribute(element, "AXTitle").as_deref() == Some(name)
    });
    if !ok {
        log::debug!(
            "input queue: at {at:?} the screen shows {:?}",
            seen.map(|(owner, e)| (
                owner,
                super::ax::string_attribute(&e, "AXRole"),
                super::ax::string_attribute(&e, "AXTitle")
            ))
        );
    }
    ok
}

/// Choose an item of `target`'s app's menu bar with the user's pointer
/// (ADR 0013, "Menu items of Electron apps"): a real click on the title on
/// the Agent Display's own menu bar, then on the item in the menu that
/// opens there. Electron then sees a mouse event, not a shortcut (#39).
/// `path` runs from the menu bar title down to the item, as
/// `ax::menu_bar_path` gives it. `go` is checked before every event, with
/// the user's touch, the app still in front and the menu still open. No
/// key is ever sent; a menu left open on any stop closes with AXCancel.
pub fn pick_menu_item(
    target: WindowTarget,
    area: Rect,
    path: &[(usize, String)],
    go: impl Fn((f64, f64)) -> bool,
) -> Outcome {
    let Ok(_session) = SESSION_INPUT.try_lock() else {
        return Outcome::Refused("another operation holds the session's keyboard or pointer");
    };
    let gate = process_gate(target.pid);
    let Ok(_process) = gate.try_write() else {
        return Outcome::Refused("background input into this process is still running");
    };
    if let Some(why) = inputs_busy() {
        return Outcome::Refused(why);
    }
    if path.len() < 2 {
        return Outcome::Refused("the item is not in a menu of the app's menu bar");
    }
    let Some(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return Outcome::Refused("the input event source could not be created");
    };
    let poster = Poster {
        source: Some(source),
        flags: CGEventFlags(0),
    };
    let pid = target.pid;
    let front = || super::app::frontmost_pid() == Some(pid);
    let allowed = |at| go(at) && !touched() && !unsafe { IsSecureEventInputEnabled() };
    let why_not = |at| {
        if touched() {
            "the user moved the mouse or pressed a key"
        } else if !front() {
            "the foreground app changed"
        } else if !allowed(at) {
            "the approval or the display changed"
        } else {
            "the menu closed"
        }
    };
    if !allowed(area.mid()) {
        return Outcome::Refused("the approval ended before the click");
    }
    let _holding = Holding::new(pid);
    if let Err(why) = come_forward(target, || allowed(area.mid())) {
        return Outcome::Refused(why);
    }
    let Some(items) = super::ax::menu_bar_elements(pid, path) else {
        return Outcome::Refused("the menu item is no longer where it was in the app's menu bar");
    };
    let title = &items[0];
    let Some(at) = title_on_display(title, area) else {
        return Outcome::Refused("the menu bar title has no place on the Agent Display's menu bar");
    };
    // The menu bar shows the app in front. It may also hide until the
    // pointer reaches the top of the screen: wait for the title to show.
    if !poster.post(CGEventType::MouseMoved, at) {
        return Outcome::Refused("the mouse event could not be created");
    }
    let asked = std::time::Instant::now();
    loop {
        if !allowed(at) || !front() {
            return Outcome::Refused(why_not(at));
        }
        if title_there(pid, &path[0].1, at) {
            break;
        }
        if asked.elapsed() > MENU_OPENS {
            return Outcome::Refused(
                "the Agent Display's menu bar didn't show the app's menus, as when Displays have separate Spaces is off",
            );
        }
        thread::sleep(Duration::from_millis(30));
    }
    let mut pointer = at;
    let stop = |why: &'static str| {
        close_menu(title);
        Outcome::Stopped(why)
    };
    if let Err(why) = click(&poster, at, &allowed, &front) {
        return stop(why);
    }
    // Each level: the menu opens on the display, then the pointer goes to
    // the next item. Into a submenu it moves across first, so it doesn't
    // cross its menu's other items, which would close the submenu.
    let mut menu_of = title.clone();
    for (depth, item) in items.iter().enumerate().skip(1) {
        let Some(menu) = super::ax::submenu(&menu_of) else {
            return stop("the menu has no items");
        };
        let opened = std::time::Instant::now();
        let frame = loop {
            if !allowed(pointer) || !front() {
                return stop(why_not(pointer));
            }
            if let Some(frame) = open_on(&menu, area) {
                break frame;
            }
            if opened.elapsed() > MENU_OPENS {
                let seen = open_frame(&menu);
                log::info!("input queue: menu {depth} not open on {area:?}; it reports {seen:?}");
                return stop(if seen.is_some() {
                    "the menu opened outside the Agent Display"
                } else {
                    "the menu didn't open"
                });
            }
            thread::sleep(Duration::from_millis(20));
        };
        log::info!("input queue: menu {depth} open at {frame:?}, display {area:?}");
        let Some(spot) = super::ax::frame(item).filter(|f| menu_press::on_display(*f, area)) else {
            return stop("the menu item is not on the Agent Display");
        };
        let to = spot.mid();
        let corner = if depth == 1 {
            (pointer.0, to.1)
        } else {
            (to.0, pointer.1)
        };
        for leg in [corner, to] {
            if let Err(why) = glide(&poster, pointer, leg, |at| {
                allowed(at) && front() && open_on(&menu, area).is_some()
            }) {
                return stop(if why {
                    why_not(pointer)
                } else {
                    "a mouse event could not be created"
                });
            }
            pointer = leg;
        }
        if depth + 1 < items.len() {
            menu_of = item.clone();
            continue;
        }
        if open_on(&menu, area).is_none() {
            return stop("the menu closed");
        }
        if let Err(why) = click(&poster, to, &allowed, &front) {
            return stop(why);
        }
    }
    // The chosen item blinks before its menu goes away.
    let clicked = std::time::Instant::now();
    while super::ax::submenu(title).is_some_and(|m| open_frame(&m).is_some()) {
        if clicked.elapsed() > MENU_OPENS {
            return stop("the menu stayed open after the click, so the item didn't take it");
        }
        thread::sleep(Duration::from_millis(30));
    }
    Outcome::Done
}

/// One click at `at`. The button goes up however this ends.
fn click(
    poster: &Poster,
    at: (f64, f64),
    allowed: &dyn Fn((f64, f64)) -> bool,
    front: &dyn Fn() -> bool,
) -> Result<(), &'static str> {
    if !allowed(at) || !front() {
        return Err(if touched() {
            "the user moved the mouse or pressed a key"
        } else {
            "the approval, the display or the front app changed"
        });
    }
    let Some(up) = poster.event(CGEventType::LeftMouseUp, at) else {
        return Err("the release event could not be created before pressing");
    };
    let mut release = Release {
        event: up,
        at,
        pressed: false,
        button: LEFT,
    };
    if !poster.post(CGEventType::LeftMouseDown, at) {
        return Err("the press could not be created");
    }
    release.pressed = true;
    thread::sleep(Duration::from_millis(50));
    drop(release);
    thread::sleep(Duration::from_millis(30));
    Ok(())
}

/// Move the pointer from `from` to `to` in small steps, checking `ok`
/// before each. Err(true) when `ok` stopped it, Err(false) when an event
/// could not be made.
fn glide(
    poster: &Poster,
    from: (f64, f64),
    to: (f64, f64),
    ok: impl Fn((f64, f64)) -> bool,
) -> Result<(), bool> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let steps = ((dx.hypot(dy) / (STEP * 2.0)).ceil() as usize).clamp(1, 200);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let at = (from.0 + dx * t, from.1 + dy * t);
        if !ok(at) {
            return Err(true);
        }
        if !poster.post(CGEventType::MouseMoved, at) {
            return Err(false);
        }
        thread::sleep(Duration::from_millis(8));
    }
    thread::sleep(Duration::from_millis(60));
    Ok(())
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
