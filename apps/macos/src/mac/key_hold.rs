//! The user's keys held while a new window of the agent's has their front
//! app's key focus (#110; the rules are in `agent_screen_model::key_hold`).
//! An active tap at the HID level, enabled only during a hold, takes the
//! key events bound for that app and keeps them. When the user's window
//! has its focus back, or at the latest after `MAX_HOLD_MS`, they go out
//! again, in order, marked so the tap lets them through, and reach the
//! user's window.

use agent_screen_model::key_hold::{End, KeyHold, MAX_HOLD_MS, Seen, Verdict};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType,
};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Marks the keys this module sends on: "ASKH".
const TAG: i64 = 0x4153_4b48;

/// After a hold, keys still on their way to the tap are sent on behind
/// the held ones for this long, so none passes ahead of them.
const DRAIN: Duration = Duration::from_millis(300);

struct Held {
    event: CFRetained<CGEvent>,
    down: bool,
}
// A CGEvent is a CF object, used from one thread at a time here.
unsafe impl Send for Held {}

enum State {
    Idle,
    Holding {
        id: u64,
        hold: KeyHold<Held>,
        started: Instant,
    },
    /// Sending on keys that reach the tap after the hold, until `until`.
    Draining {
        until: Instant,
    },
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static NEXT: AtomicU64 = AtomicU64::new(1);

/// What the last hold did, for `debugKeyHold`.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub events: usize,
    pub key_downs: usize,
    pub ms: u64,
    pub end: End,
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

/// A hold in progress. Dropping it ends the hold and sends its keys on.
pub struct Hold {
    id: u64,
}

impl Hold {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Its id, for a caller that ends it with `finish` itself.
    pub fn detach(self) -> u64 {
        let id = self.id;
        std::mem::forget(self);
        id
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        finish(self.id, End::Abandoned);
    }
}

/// Hold the keys bound for `app`, the user's front app. None when the tap
/// can't be installed, or another hold is going on: the keys then go as
/// they would without one.
pub fn start(app: i32) -> Option<Hold> {
    let tap = TAP.get_or_init(install).as_ref()?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, State::Idle) {
            return None;
        }
        *state = State::Holding {
            id,
            hold: KeyHold::new(app),
            started: Instant::now(),
        };
    }
    CGEvent::tap_enable(&tap.0, true);
    log::debug!("key hold: holding keys for {app}");
    // However the launch goes, the hold ends in time.
    let _ = std::thread::Builder::new()
        .name("key-hold-limit".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(MAX_HOLD_MS));
            finish(id, End::TimedOut);
        });
    Some(Hold { id })
}

/// End hold `id`, if it is still the one going on, and send its keys on.
pub fn finish(id: u64, why: End) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let State::Holding { id: current, .. } = &*state else {
        return;
    };
    if *current != id {
        return;
    }
    let State::Holding {
        mut hold, started, ..
    } = std::mem::replace(
        &mut *state,
        State::Draining {
            until: Instant::now() + DRAIN,
        },
    )
    else {
        return;
    };
    let held = hold.release(why);
    send_on(held, hold.ended().unwrap_or(why), started);
    drop(state);
    end_drain_later();
}

/// Send `held` on, in order, and note what the hold did. Called with the
/// state locked, so a key the tap sees meanwhile waits and goes after.
fn send_on(held: Vec<Held>, why: End, started: Instant) {
    let stats = Stats {
        events: held.len(),
        key_downs: held.iter().filter(|h| h.down).count(),
        ms: started.elapsed().as_millis() as u64,
        end: why,
    };
    for h in &held {
        CGEvent::set_integer_value_field(Some(&h.event), CGEventField::EventSourceUserData, TAG);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&h.event));
    }
    match why {
        End::HandedBack => log::info!("key hold: {stats:?}"),
        _ => log::warn!("key hold: ended before the hand-back: {stats:?}"),
    }
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(stats);
}

fn end_drain_later() {
    let _ = std::thread::Builder::new()
        .name("key-hold-drain".into())
        .spawn(|| {
            std::thread::sleep(DRAIN);
            end_drain();
        });
}

fn end_drain() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(*state, State::Draining { until } if Instant::now() >= until) {
        *state = State::Idle;
        if let Some(Some(tap)) = TAP.get() {
            CGEvent::tap_enable(&tap.0, false);
        }
    }
}

/// The tap, on a thread of its own, disabled until a hold starts.
fn install() -> Option<Tap> {
    let mask = [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ]
    .iter()
    .fold(0_u64, |mask, kind| mask | (1_u64 << kind.0));
    let (ready, made) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("key-hold".into())
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
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
            CGEvent::tap_enable(&tap, false);
            let _ = ready.send(Some(Tap(tap)));
            CFRunLoop::run();
        });
    if spawned.is_err() {
        return None;
    }
    let tap = made.recv_timeout(Duration::from_secs(1)).ok().flatten();
    if tap.is_none() {
        log::warn!("key hold: no tap; keys typed during a launch go as they are");
    }
    tap
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
        let holding = !matches!(
            *STATE.lock().unwrap_or_else(|e| e.into_inner()),
            State::Idle
        );
        if holding && let Some(Some(tap)) = TAP.get() {
            CGEvent::tap_enable(&tap.0, true);
        }
        return event.as_ptr();
    }
    let cg = unsafe { event.as_ref() };
    let mark = CGEvent::integer_value_field(Some(cg), CGEventField::EventSourceUserData);
    let ours = mark == TAG || mark == super::real_input::TAG;
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    match &mut *state {
        State::Idle => event.as_ptr(),
        State::Draining { until } => {
            if ours || Instant::now() >= *until {
                return event.as_ptr();
            }
            // Behind the held keys, which went out before it.
            let Some(copy) = CGEvent::new_copy(Some(cg)) else {
                return event.as_ptr();
            };
            CGEvent::set_integer_value_field(Some(&copy), CGEventField::EventSourceUserData, TAG);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&copy));
            std::ptr::null_mut()
        }
        State::Holding { hold, started, .. } => {
            let Some(copy) = CGEvent::new_copy(Some(cg)) else {
                return event.as_ptr();
            };
            let seen = Seen {
                at_ms: started.elapsed().as_millis() as u64,
                front: super::input::front().unwrap_or(0),
                key: true,
                ours,
            };
            let held = Held {
                event: copy,
                down: kind == CGEventType::KeyDown,
            };
            match hold.see(seen, held) {
                (Verdict::Pass, _) => event.as_ptr(),
                (Verdict::Hold, _) => std::ptr::null_mut(),
                (Verdict::Flush, _) => {
                    let started = *started;
                    let why = hold.ended().unwrap_or(End::Abandoned);
                    let held = hold.release(why);
                    *state = State::Draining {
                        until: Instant::now() + DRAIN,
                    };
                    send_on(held, why, started);
                    drop(state);
                    end_drain_later();
                    std::ptr::null_mut()
                }
            }
        }
    }
}
