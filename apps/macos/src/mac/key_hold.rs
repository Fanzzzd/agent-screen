//! The user's input held while a new window of the agent's has their front
//! app's key focus (#110; the rules are in `agent_screen_model::key_hold`).
//! An active tap at the HID level, enabled only during a hold, takes the
//! key and mouse button events bound for that app and keeps them. When
//! the hold ends, at the latest after `MAX_HOLD_MS`, the user's window is
//! made key again and confirmed; only then does the input go out again, in
//! order, marked so the tap lets it through. Where that window can't be
//! made key by `MAX_HOLD_MS`, the input goes out only if the app's key
//! window is off the Agent Displays; otherwise its key downs are discarded
//! and the rest goes out, as mouse events go to the user's screen and key
//! ups keep the app's key state paired.

use crate::model::WindowTarget;
use agent_screen_model::key_hold::{
    Destination, End, KeyHold, Kind, MAX_HOLD_MS, Seen, Verdict, destination, sent_on_discard,
};
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

struct Held(CFRetained<CGEvent>);
// A CGEvent is a CF object, used from one thread at a time here.
unsafe impl Send for Held {}

enum State {
    Idle,
    Holding {
        id: u64,
        hold: KeyHold<Held>,
        started: Instant,
        /// The user's window, to make key again before anything goes out.
        user: WindowTarget,
        /// A press or a full hold has asked for the end already.
        ending: bool,
        /// Its owner ends it (`start_with`): a press or a full hold only
        /// asks, in `asked`, and the owner calls `finish` once the user's
        /// app is in front again.
        owned: bool,
        asked: Option<End>,
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
    /// Whether the held input went out or was dropped.
    pub sent: bool,
    /// Key downs dropped, when not sent.
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

/// What a hold holds (`Hold::status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Still holding: `typed`, a key the user typed; `asked`, why the
    /// user's input asks it to end, for an owned hold.
    Holding { typed: bool, asked: Option<End> },
    /// It ended, at its own limit or by `finish`.
    Ended,
}

/// A hold in progress. Dropping it ends the hold and sends its keys on.
pub struct Hold {
    id: u64,
}

impl Hold {
    /// Whether it is still holding input, not yet ended by a press, a full
    /// hold or the time limit.
    pub fn holding(&self) -> bool {
        matches!(
            &*STATE.lock().unwrap_or_else(|e| e.into_inner()),
            State::Holding { id, .. } if *id == self.id
        )
    }

    /// What it holds, for an owner that ends it itself.
    pub fn status(&self) -> Status {
        match &*STATE.lock().unwrap_or_else(|e| e.into_inner()) {
            State::Holding {
                id, hold, asked, ..
            } if *id == self.id => Status::Holding {
                typed: hold.typed(),
                asked: *asked,
            },
            _ => Status::Ended,
        }
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
        // Making the user's window key waits on the app: not on the
        // caller's thread, which may be the main thread.
        let id = self.id;
        let _ = std::thread::Builder::new()
            .name("key-hold-end".into())
            .spawn(move || finish(id, End::Abandoned));
    }
}

/// Hold the input bound for `user`'s app, the user's front app, whose
/// window `user` has its key focus. None when the tap can't be installed,
/// or another hold is going on: input then goes as it would without one.
/// Input still draining from an ended hold is held by the new one, behind
/// what went out before it.
pub fn start(user: WindowTarget) -> Option<Hold> {
    start_with(user, None)
}

/// As `start`, holding the input bound for `also` too: Agent Screen, while
/// it stands in front of the user's app during a front hold. With `also`,
/// the caller ends the hold: a press or a full hold only asks, as the
/// keys before them can reach the user's window only once their app is in
/// front again. The time limit still ends it.
pub fn start_with(user: WindowTarget, also: Option<i32>) -> Option<Hold> {
    let app = user.pid;
    let tap = TAP.get_or_init(install).as_ref()?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*state, State::Holding { .. }) {
            return None;
        }
        *state = State::Holding {
            id,
            hold: match also {
                Some(other) => KeyHold::new(app).also(other),
                None => KeyHold::new(app),
            },
            started: Instant::now(),
            user,
            ending: false,
            owned: also.is_some(),
            asked: None,
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

/// End hold `id`, if it is still the one going on: make the user's window
/// key again, then send what it held on, or drop its key downs where they
/// would reach the agent's window. Keeps trying to make the window key
/// until `MAX_HOLD_MS` from the hold's start, blocking meanwhile. False
/// when the hold had ended already, or another call ended it first.
pub fn finish(id: u64, why: End) -> bool {
    let (user, started) = match &*STATE.lock().unwrap_or_else(|e| e.into_inner()) {
        State::Holding {
            id: current,
            user,
            started,
            ..
        } if *current == id => (*user, *started),
        _ => return false,
    };
    // Input keeps being held meanwhile.
    let user_key = make_key(user, started + Duration::from_millis(MAX_HOLD_MS));
    let key_on_agent_display = match user_key {
        true => Some(false),
        false => super::ax::focused_window_id(user.pid).and_then(|w| {
            super::windows::find(w).map(|w| super::input::on_agent_display_at(w.bounds))
        }),
    };
    let front_is_app = super::input::front() == Some(user.pid);
    let to = destination(front_is_app, user_key, key_on_agent_display);
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(&*state, State::Holding { id: current, .. } if *current == id) {
        return false;
    }
    let next = match to {
        Destination::Send => State::Draining {
            until: Instant::now() + DRAIN,
        },
        Destination::Discard => State::Idle,
    };
    let State::Holding {
        mut hold, started, ..
    } = std::mem::replace(&mut *state, next)
    else {
        return false;
    };
    let held = hold.release(why);
    let why = hold.ended().unwrap_or(why);
    send_on(held, why, started, to);
    if to == Destination::Discard
        && let Some(Some(tap)) = TAP.get()
    {
        CGEvent::tap_enable(&tap.0, false);
    }
    drop(state);
    if to == Destination::Send {
        end_drain_later();
    }
    true
}

/// Make `user` its app's key window, as a click on it would, and wait for
/// accessibility to say it is, asking again every 300 ms until `until`,
/// as a busy app can take a while. Asks at least once.
fn make_key(user: WindowTarget, until: Instant) -> bool {
    let key = || super::ax::focused_window_id(user.pid) == Some(user.window_id);
    loop {
        if key() {
            return true;
        }
        super::ax::make_main(user);
        let asked = Instant::now();
        while asked.elapsed() < Duration::from_millis(300) {
            std::thread::sleep(Duration::from_millis(20));
            if key() {
                return true;
            }
        }
        if Instant::now() >= until {
            return false;
        }
    }
}

/// Send `held` on, in order, all of it or all but its key downs, and note
/// what the hold did. Called with the state locked, so input the tap sees
/// meanwhile waits and goes after.
fn send_on(held: Vec<(Kind, Held)>, why: End, started: Instant, to: Destination) {
    let sent = to == Destination::Send;
    let key_downs = held.iter().filter(|(k, _)| *k == Kind::KeyDown).count();
    let stats = Stats {
        events: held.len(),
        key_downs,
        ms: started.elapsed().as_millis() as u64,
        end: why,
        sent,
        discarded: if sent { 0 } else { key_downs },
    };
    for (kind, Held(event)) in &held {
        if sent || sent_on_discard(*kind) {
            CGEvent::set_integer_value_field(Some(event), CGEventField::EventSourceUserData, TAG);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
        }
    }
    match (why, sent) {
        (End::HandedBack, true) => log::info!("key hold: {stats:?}"),
        (_, true) => log::warn!("key hold: ended before the hand-back: {stats:?}"),
        (_, false) => log::warn!(
            "key hold: the user's window couldn't be made key, so the held key downs were dropped rather than sent to the agent's window: {stats:?}"
        ),
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
        // The Stop chord while the mode acts: this tap would keep it from
        // the hot key, so it stops here, as in the armed hold.
        State::Holding { .. }
            if kind == CGEventType::KeyDown
                && !ours
                && super::hotkey::registered()
                && agent_screen_model::autonomy::stop_chord(
                    CGEvent::integer_value_field(Some(cg), CGEventField::KeyboardEventKeycode)
                        as u16,
                    CGEvent::flags(Some(cg)).0,
                ) =>
        {
            drop(state);
            log::info!("autonomy: the Stop chord, during a key hold");
            dispatch2::DispatchQueue::main()
                .exec_async(|| super::native::send(super::native::Native::StopChord));
            std::ptr::null_mut()
        }
        State::Holding {
            id,
            hold,
            ending,
            owned,
            asked,
            ..
        } => {
            let Some(copy) = CGEvent::new_copy(Some(cg)) else {
                return event.as_ptr();
            };
            let field = |f| CGEvent::integer_value_field(Some(cg), f);
            let (kind, code) = match kind {
                CGEventType::KeyDown => (Kind::KeyDown, field(CGEventField::KeyboardEventKeycode)),
                CGEventType::KeyUp => (Kind::KeyUp, field(CGEventField::KeyboardEventKeycode)),
                CGEventType::FlagsChanged => {
                    (Kind::Modifiers, field(CGEventField::KeyboardEventKeycode))
                }
                CGEventType::LeftMouseDown
                | CGEventType::RightMouseDown
                | CGEventType::OtherMouseDown => {
                    (Kind::MouseDown, field(CGEventField::MouseEventButtonNumber))
                }
                CGEventType::LeftMouseUp
                | CGEventType::RightMouseUp
                | CGEventType::OtherMouseUp => {
                    (Kind::MouseUp, field(CGEventField::MouseEventButtonNumber))
                }
                CGEventType::LeftMouseDragged
                | CGEventType::RightMouseDragged
                | CGEventType::OtherMouseDragged => (
                    Kind::MouseDragged,
                    field(CGEventField::MouseEventButtonNumber),
                ),
                _ => (Kind::Other, 0),
            };
            let seen = Seen {
                front: super::input::front().unwrap_or(0),
                kind,
                code,
                ours,
            };
            let why = match hold.see(seen, Held(copy)) {
                (Verdict::Pass, _) => return event.as_ptr(),
                (Verdict::Hold, _) if kind != Kind::MouseDown => return std::ptr::null_mut(),
                // A press means the user moved on: end the hold now, and
                // the press goes out after the keys before it.
                (Verdict::Hold, _) => End::Pressed,
                (Verdict::HoldAndEnd, _) => End::Full,
            };
            if *owned {
                asked.get_or_insert(why);
            } else if !*ending {
                *ending = true;
                let id = *id;
                let _ = std::thread::Builder::new()
                    .name("key-hold-end".into())
                    .spawn(move || finish(id, why));
            }
            std::ptr::null_mut()
        }
    }
}
