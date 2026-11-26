//! The armed hold of a grant the Autonomy Mode opened (ADR 0025; the rules
//! are `agent_screen_model::key_hold::Armed`). An active tap at the HID
//! level, enabled from the grant's start until the pointer, front app and
//! focused window are back. It holds nothing while nothing of the user's
//! arrives. Their first key, modifier change or press is held, and ends
//! the grant in Auto; once the user's window is confirmed key again, what
//! it held goes out in order, a press at the pointer's saved position.
//! Where that window can't be confirmed key, the key downs are dropped:
//! losing what the user typed beats sending it to an agent's window.
//!
//! Agent Screen's own events carry `real_input::TAG`, `key_hold`'s tag or
//! this module's, and pass. Grants the user opened with Allow keep the
//! listen-only observer and no hold.

use agent_screen_model::autonomy::{Mode, stop_chord};
use agent_screen_model::key_hold::{Armed, ArmedEvent, ArmedVerdict, Kind, sent_on_discard};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, CGPoint, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType,
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Marks the user's events this module sends on: "ASAH".
pub const TAG: i64 = 0x4153_4148;

/// The key-hold tag, which passes too.
const KEY_HOLD_TAG: i64 = 0x4153_4b48;

struct Held(CFRetained<CGEvent>);
// A CGEvent is a CF object, used from one thread at a time here.
unsafe impl Send for Held {}

enum State {
    Idle,
    /// `finish` is sending on what was held: the user's new input queues
    /// behind it, so nothing overtakes a held event and no release passes
    /// before its press.
    Draining {
        held: Vec<(Kind, Held)>,
    },
    Armed {
        id: u64,
        mode: Mode,
        armed: Armed,
        started: Instant,
        held: Vec<(Kind, Held)>,
    },
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static NEXT: AtomicU64 = AtomicU64::new(1);

/// What the last hold did, for `debugAutonomy`.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub events: usize,
    pub key_downs: usize,
    /// From the first held event to the release.
    pub ms: u64,
    pub sent: bool,
    pub discarded: usize,
}

static LAST: Mutex<Option<Stats>> = Mutex::new(None);

pub fn last() -> Option<Stats> {
    *LAST.lock().unwrap_or_else(|e| e.into_inner())
}

struct Tap(CFRetained<CFMachPort>);
// A mach port is a kernel object; enabling a tap works from any thread.
unsafe impl Send for Tap {}
unsafe impl Sync for Tap {}
static TAP: OnceLock<Option<Tap>> = OnceLock::new();

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Arm the hold for a grant `mode` opened. False when the tap can't be
/// installed: a mode then opens no grant, as nothing would hold the
/// user's keys.
pub fn start(mode: Mode) -> bool {
    let Some(tap) = TAP.get_or_init(install).as_ref() else {
        return false;
    };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    *state() = State::Armed {
        id,
        mode,
        armed: Armed::new(),
        started: Instant::now(),
        held: Vec::new(),
    };
    CGEvent::tap_enable(&tap.0, true);
    log::info!("armed hold: on ({})", mode.as_str());
    true
}

/// Whether a hold is armed.
pub fn armed() -> bool {
    matches!(*state(), State::Armed { .. })
}

/// Whether it holds anything: no new loan starts then.
pub fn holding() -> bool {
    matches!(&*state(), State::Armed { armed, .. } if armed.first().is_some())
}

/// In YOLO, whether `HELD_GRACE_MS` passed since the first held event; in
/// Auto, whether anything is held.
pub fn grace_over() -> bool {
    match &*state() {
        State::Armed {
            armed,
            mode,
            started,
            ..
        } => armed
            .deadline(*mode)
            .is_some_and(|deadline| millis(*started) >= deadline),
        _ => false,
    }
}

/// How many events it holds.
pub fn held() -> usize {
    match &*state() {
        State::Armed { held, .. } => held.len(),
        _ => 0,
    }
}

/// The pointer, front app and focused window are back: send what the hold
/// kept, in order, and let go. `pointer` is where a held press goes out;
/// `confirmed`, whether the user's window is key again, without which the
/// key downs are dropped. Blocks only to post.
pub fn finish(pointer: Option<(f64, f64)>, confirmed: bool) {
    let taken = {
        let mut s = state();
        match *s {
            State::Armed { .. } => std::mem::replace(&mut *s, State::Draining { held: Vec::new() }),
            // Another finish is draining: it lets go.
            State::Draining { .. } => return,
            State::Idle => {
                if let Some(Some(tap)) = TAP.get() {
                    CGEvent::tap_enable(&tap.0, false);
                }
                return;
            }
        }
    };
    let State::Armed {
        armed,
        started,
        held,
        ..
    } = taken
    else {
        return;
    };
    let key_downs = held.iter().filter(|(k, _)| *k == Kind::KeyDown).count();
    let mut events = held.len();
    let mut discarded = if confirmed { 0 } else { key_downs };
    let now = millis(started);
    send(&held, pointer, confirmed);
    // What the user did meanwhile goes out after, in order. The tap goes
    // off under the same lock that finds the queue empty.
    loop {
        let more = {
            let mut s = state();
            match &mut *s {
                State::Draining { held } if !held.is_empty() => std::mem::take(held),
                _ => {
                    *s = State::Idle;
                    if let Some(Some(tap)) = TAP.get() {
                        CGEvent::tap_enable(&tap.0, false);
                    }
                    break;
                }
            }
        };
        events += more.len();
        if !confirmed {
            discarded += more.iter().filter(|(k, _)| *k == Kind::KeyDown).count();
        }
        send(&more, pointer, confirmed);
    }
    let stats = Stats {
        events,
        key_downs,
        ms: armed.first().map_or(0, |first| now.saturating_sub(first)),
        sent: confirmed,
        discarded,
    };
    if events == 0 {
        log::info!("armed hold: off, nothing held");
    } else if confirmed {
        log::info!("armed hold: off, sent on {stats:?}");
    } else {
        log::warn!(
            "armed hold: off; the user's window couldn't be confirmed key, so the held key downs were dropped rather than sent to the agent's window: {stats:?}"
        );
    }
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(stats);
}

/// Send on `held` in order, tagged so the tap lets it pass; without
/// `confirmed`, the key downs are dropped.
fn send(held: &[(Kind, Held)], pointer: Option<(f64, f64)>, confirmed: bool) {
    for (kind, Held(event)) in held {
        if !confirmed && !sent_on_discard(*kind) {
            continue;
        }
        if matches!(kind, Kind::MouseDown | Kind::MouseUp | Kind::MouseDragged)
            && let Some((x, y)) = pointer
        {
            CGEvent::set_location(Some(event), CGPoint::new(x, y));
        }
        CGEvent::set_integer_value_field(Some(event), CGEventField::EventSourceUserData, TAG);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }
}

fn millis(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

/// However the restoration goes, the hold lets go `MAX_HOLD_MS` after its
/// first held event, dropping key downs.
fn limit_later(id: u64) {
    let _ = std::thread::Builder::new()
        .name("armed-hold-limit".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(
                agent_screen_model::key_hold::MAX_HOLD_MS,
            ));
            let current = matches!(&*state(), State::Armed { id: now, .. } if *now == id);
            if current {
                log::warn!("armed hold: held for the limit; letting go");
                finish(None, false);
            }
        });
}

/// In YOLO, the operation ends `HELD_GRACE_MS` after the first held event.
fn grace_later(id: u64) {
    let _ = std::thread::Builder::new()
        .name("armed-hold-grace".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(
                agent_screen_model::user_activity::HELD_GRACE_MS,
            ));
            let current = matches!(&*state(), State::Armed { id: now, .. } if *now == id);
            if current {
                log::info!("armed hold: the grace after the user's first held input is over");
                dispatch2::DispatchQueue::main()
                    .exec_async(|| super::native::send(super::native::Native::GraceOver));
            }
        });
}

/// The tap, on a thread of its own, disabled until a hold starts.
fn install() -> Option<Tap> {
    let mask = [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::MouseMoved,
        CGEventType::ScrollWheel,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
    ]
    .iter()
    .fold(0_u64, |mask, kind| mask | (1_u64 << kind.0));
    let (ready, made) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("armed-hold".into())
        .spawn(move || {
            let tap = unsafe {
                CGEvent::tap_create(
                    CGEventTapLocation::HIDEventTap,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::Default,
                    mask,
                    Some(seen),
                    std::ptr::null_mut(),
                )
            };
            let Some(tap) = tap else {
                let _ = ready.send(None);
                return;
            };
            let (Some(source), Some(run_loop)) = (
                CFMachPort::new_run_loop_source(None, Some(&tap), 0),
                CFRunLoop::current(),
            ) else {
                let _ = ready.send(None);
                return;
            };
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            CGEvent::tap_enable(&tap, false);
            let _ = ready.send(Some(Tap(tap)));
            CFRunLoop::run();
        });
    if spawned.is_err() {
        return None;
    }
    let tap = made.recv_timeout(Duration::from_secs(1)).ok().flatten();
    if tap.is_none() {
        log::warn!("armed hold: no tap; no mode opens a grant");
    }
    tap
}

fn kind_of(kind: CGEventType) -> Option<ArmedEvent> {
    Some(match kind {
        CGEventType::MouseMoved | CGEventType::ScrollWheel => ArmedEvent::Moved,
        CGEventType::KeyDown => ArmedEvent::Input(Kind::KeyDown),
        CGEventType::KeyUp => ArmedEvent::Input(Kind::KeyUp),
        CGEventType::FlagsChanged => ArmedEvent::Input(Kind::Modifiers),
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            ArmedEvent::Input(Kind::MouseDown)
        }
        CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp => {
            ArmedEvent::Input(Kind::MouseUp)
        }
        CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => ArmedEvent::Input(Kind::MouseDragged),
        _ => return None,
    })
}

/// Runs on the tap's thread.
unsafe extern "C-unwind" fn seen(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    _context: *mut std::ffi::c_void,
) -> *mut CGEvent {
    if matches!(
        kind,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        if armed()
            && let Some(Some(tap)) = TAP.get()
        {
            CGEvent::tap_enable(&tap.0, true);
        }
        return event.as_ptr();
    }
    let cg = unsafe { event.as_ref() };
    let mark = CGEvent::integer_value_field(Some(cg), CGEventField::EventSourceUserData);
    let ours = mark == super::real_input::TAG || mark == KEY_HOLD_TAG || mark == TAG;
    let chord = kind == CGEventType::KeyDown
        && stop_chord(
            CGEvent::integer_value_field(Some(cg), CGEventField::KeyboardEventKeycode) as u16,
            CGEvent::flags(Some(cg)).0,
        );
    let Some(seen) = (if ours {
        Some(ArmedEvent::Ours)
    } else if chord {
        Some(ArmedEvent::StopChord)
    } else {
        kind_of(kind)
    }) else {
        return event.as_ptr();
    };
    let mut guard = state();
    if let State::Draining { held } = &mut *guard {
        // Behind what `finish` is sending on.
        if let ArmedEvent::Input(input) = seen
            && held.len() < agent_screen_model::key_hold::MAX_EVENTS
            && let Some(copy) = CGEvent::new_copy(Some(cg))
        {
            held.push((input, Held(copy)));
            return std::ptr::null_mut();
        }
        return event.as_ptr();
    }
    let State::Armed {
        id,
        mode,
        armed,
        started,
        held,
    } = &mut *guard
    else {
        return event.as_ptr();
    };
    let was_holding = armed.first().is_some();
    let mode = *mode;
    match armed.see(seen, mode, millis(*started)) {
        ArmedVerdict::Stop => {
            drop(guard);
            dispatch2::DispatchQueue::main()
                .exec_async(|| super::native::send(super::native::Native::StopChord));
            std::ptr::null_mut()
        }
        // In YOLO the mouse is detached for the loan and a move or a
        // scroll bends nothing: it is dropped, so it neither reaches the
        // agent's window under the cursor nor counts as a touch.
        ArmedVerdict::Pass if mode == Mode::Yolo && seen == ArmedEvent::Moved => {
            std::ptr::null_mut()
        }
        ArmedVerdict::Pass | ArmedVerdict::Touched => event.as_ptr(),
        ArmedVerdict::Drop => std::ptr::null_mut(),
        verdict @ (ArmedVerdict::Hold | ArmedVerdict::HoldAndEnd) => {
            let Some(copy) = CGEvent::new_copy(Some(cg)) else {
                return event.as_ptr();
            };
            let ArmedEvent::Input(input) = seen else {
                return event.as_ptr();
            };
            held.push((input, Held(copy)));
            let id = *id;
            drop(guard);
            if !was_holding {
                limit_later(id);
            }
            // The observer never sees a held event: note the buttons the
            // user holds here, and in Auto their touch, which ends the
            // grant. In YOLO the operation runs on for `HELD_GRACE_MS`.
            if mode == Mode::Yolo {
                super::real_input::user_buttons(kind);
                if !was_holding {
                    grace_later(id);
                }
            } else {
                super::real_input::user_event(kind);
            }
            if verdict == ArmedVerdict::HoldAndEnd {
                log::warn!("armed hold: full; the operation ends now");
                dispatch2::DispatchQueue::main()
                    .exec_async(|| super::native::send(super::native::Native::GraceOver));
            }
            std::ptr::null_mut()
        }
    }
}
