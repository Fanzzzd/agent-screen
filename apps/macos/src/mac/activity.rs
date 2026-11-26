//! The User Activity Monitor (ADR 0025): what the user is using, for Auto.
//! A thread samples the OS once a second, four times a second while a
//! crossing waits on it, and feeds `user_activity::Tracker`, which decides
//! which windows are In Use and whether the hands are At Rest. It runs only
//! while the mode is Auto; Ask and YOLO read one sample at decision time.
//!
//! Also here: the lock, sleep and session observers that end YOLO, and the
//! e2e instance's override of the sample (`debugActivity`).
// The crossings that read it arrive in the next commits.
#![allow(dead_code)]

use agent_screen_model::autonomy::YoloEnd;
use agent_screen_model::user_activity::{Hands, SAMPLE_MS, Sample, Tracker, WAITING_SAMPLE_MS};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The monitor's clock: ms since the app started.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

pub fn now_ms() -> u64 {
    EPOCH.elapsed().as_millis() as u64
}

struct State {
    tracker: Tracker,
    /// The sampler thread runs.
    running: bool,
    /// Bumped on each start and stop, so a thread from before a stop ends.
    generation: u64,
    /// The e2e override: fields set here replace the OS's.
    over: Option<serde_json::Map<String, Value>>,
    /// The last media read, and its windows.
    media: (Option<Instant>, Vec<u32>),
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        tracker: Tracker::default(),
        running: false,
        generation: 0,
        over: None,
        media: (None, Vec::new()),
    })
});

/// Crossings waiting on the monitor; while any does it samples every 250 ms.
static WAITING: AtomicU64 = AtomicU64::new(0);
/// The sampler sleeps on this, so a start or a waiting crossing wakes it.
static WAKE: LazyLock<(Mutex<()>, std::sync::Condvar)> =
    LazyLock::new(|| (Mutex::new(()), std::sync::Condvar::new()));
/// Set when the override changed, so decisions read it at once.
static DIRTY: AtomicBool = AtomicBool::new(false);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start the monitor: Auto was chosen, or the app launched in Auto. It
/// marks the active window and the window under the pointer as used now,
/// and takes the presses the catch still holds.
pub fn start() {
    let generation = {
        let mut s = state();
        if s.running {
            return;
        }
        s.running = true;
        s.generation += 1;
        s.generation
    };
    let _ = std::thread::Builder::new()
        .name("agent-screen-activity".into())
        .spawn(move || {
            let first = sample();
            let presses = super::catch::recent_windows()
                .into_iter()
                .map(|(window, ago)| (window, first.now_ms.saturating_sub(ago)));
            state().tracker.seed(&first, presses);
            log::info!("autonomy: the activity monitor started");
            loop {
                let period = if WAITING.load(Ordering::Acquire) > 0 {
                    WAITING_SAMPLE_MS
                } else {
                    SAMPLE_MS
                };
                {
                    let (lock, wake) = &*WAKE;
                    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = wake.wait_timeout(guard, Duration::from_millis(period));
                }
                if state().generation != generation {
                    break;
                }
                let next = sample();
                let mut s = state();
                if s.generation != generation {
                    break;
                }
                s.tracker.observe(&next);
            }
            log::info!("autonomy: the activity monitor stopped");
        });
}

/// Stop it: the mode left Auto. What it learned is kept for diagnostics.
pub fn stop() {
    let mut s = state();
    if s.running {
        s.running = false;
        s.generation += 1;
    }
    drop(s);
    WAKE.1.notify_all();
}

pub fn running() -> bool {
    state().running
}

/// A crossing waits on the monitor until the guard drops: it samples every
/// 250 ms meanwhile.
pub struct Waiting(());

impl Waiting {
    pub fn start() -> Self {
        WAITING.fetch_add(1, Ordering::AcqRel);
        WAKE.1.notify_all();
        Waiting(())
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A press of the user's on `window`, from the global monitor.
pub fn pressed(window: u32) {
    let mut s = state();
    if s.running {
        s.tracker.pressed(window, now_ms());
    }
}

/// The user interrupted a grant or hold the mode opened.
pub fn interrupted() {
    state().tracker.interrupted(now_ms());
}

/// One completed untouched.
pub fn completed_untouched() {
    state().tracker.completed_untouched();
}

/// An armed hold ended.
pub fn hold_ended() {
    state().tracker.hold_ended(now_ms());
}

/// The hands now. While the monitor runs, its last sample with the
/// override on top, refreshed when the override just changed; otherwise
/// one fresh sample, as Ask and YOLO read at decision time.
pub fn hands() -> Hands {
    let fresh = !running() || DIRTY.swap(false, Ordering::AcqRel);
    if fresh {
        let next = sample();
        let mut s = state();
        if s.running {
            s.tracker.observe(&next);
        } else {
            // A one-off read: the facts are this sample's, and it marks
            // no window.
            s.tracker.observe(&Sample {
                active_window: None,
                pointer_window: None,
                ..next
            });
        }
    }
    state().tracker.hands(now_ms())
}

/// Whether `window` is In Use. Only the running monitor knows; without it
/// no window is. The override's active and pointer windows count as used
/// now.
pub fn in_use(window: u32) -> bool {
    let _ = hands();
    let s = state();
    s.tracker.in_use(window, now_ms())
}

/// Whether the user holds a button with the pointer on `window`.
pub fn pressed_on(window: u32) -> bool {
    let _ = hands();
    state().tracker.pressed_on(window)
}

/// Since `window` was last used, as far as the monitor knows.
pub fn unused_ms(window: u32) -> Option<u64> {
    state().tracker.unused_ms(window, now_ms())
}

/// Forget the windows that closed.
pub fn forget_closed(open: &BTreeSet<u32>) {
    state().tracker.forget_closed(open);
}

/// `debugActivity`: set fields of the sample, or clear the override with
/// `clear`. Decisions read it instead of the OS. It sends no input.
pub fn set_override(params: &serde_json::Map<String, Value>) {
    let mut s = state();
    if params.get("clear").and_then(Value::as_bool) == Some(true) {
        s.over = None;
    } else {
        let over = s.over.get_or_insert_with(Default::default);
        for key in [
            "idleMs",
            "activeWindow",
            "pointerWindow",
            "buttonDown",
            "keyDown",
            "composing",
            "secureInput",
            "locked",
            "systemPrompt",
            "mediaWindows",
        ] {
            if let Some(value) = params.get(key) {
                over.insert(key.into(), value.clone());
            }
        }
    }
    drop(s);
    DIRTY.store(true, Ordering::Release);
    WAKE.1.notify_all();
}

pub fn overridden() -> bool {
    state().over.is_some()
}

/// `debugAutonomy`'s `tracker`: each known window, and the hands.
pub fn debug_json() -> Value {
    let hands = hands();
    let now = now_ms();
    let s = state();
    let windows: Vec<Value> = s
        .tracker
        .known()
        .map(|(window, at)| {
            json!({
                "windowId": window,
                "unusedMs": now.saturating_sub(at),
                "inUse": s.tracker.in_use(window, now),
            })
        })
        .collect();
    json!({
        "running": s.running,
        "override": s.over.clone().map(Value::Object),
        "windows": windows,
        "hands": {
            "idleMs": hands.idle_ms,
            "atRest": hands.at_rest().is_ok(),
            "busy": hands.at_rest().err().map(|b| format!("{b:?}")),
            "yoloMayStart": hands.yolo_may_start().is_ok(),
            "interrupted": hands.interrupted,
        },
    })
}

/// One look at the OS, with the override on top.
fn sample() -> Sample {
    let over = state().over.clone();
    let now = now_ms();
    let get = |key: &str| over.as_ref().and_then(|o| o.get(key)).cloned();
    let flag = |key: &str| get(key).and_then(|v| v.as_bool());
    let window = |key: &str| get(key).map(|v| v.as_u64().map(|n| n as u32));
    let idle_ms = match get("idleMs").and_then(|v| v.as_u64()) {
        Some(ms) => ms,
        None => (super::ime::user_idle() * 1000.0) as u64,
    };
    let locked = flag("locked").unwrap_or_else(super::cg::screen_locked);
    // An idle tick reads nothing else of the windows.
    let active = idle_ms < SAMPLE_MS + 200;
    let active_window = match window("activeWindow") {
        Some(w) => w,
        None if active && !locked => {
            super::app::frontmost_pid().and_then(super::ax::focused_window_id)
        }
        None => None,
    };
    let pointer_window = match window("pointerWindow") {
        Some(w) => w,
        None if active && !locked => window_under_pointer(),
        None => None,
    };
    let media_windows = match get("mediaWindows").and_then(|v| v.as_array().cloned()) {
        Some(list) => list
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect(),
        None => media_windows(),
    };
    Sample {
        now_ms: now,
        idle_ms,
        active_window,
        pointer_window,
        button_down: flag("buttonDown").unwrap_or_else(super::real_input::user_button_down),
        key_down: flag("keyDown").unwrap_or_else(modifier_held),
        composing: flag("composing").unwrap_or_else(composing),
        secure_input: flag("secureInput").unwrap_or_else(super::ime::secure_input),
        locked,
        system_prompt: flag("systemPrompt").unwrap_or_else(system_prompt),
        media_windows,
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

/// A modifier key is held.
fn modifier_held() -> bool {
    use agent_screen_model::autonomy::{FLAG_COMMAND, FLAG_CONTROL, FLAG_OPTION, FLAG_SHIFT};
    // kCGEventSourceStateHIDSystemState.
    let flags = unsafe { CGEventSourceFlagsState(1) };
    flags & (FLAG_SHIFT | FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND) != 0
}

/// The window under the pointer, of another process, on a user screen.
fn window_under_pointer() -> Option<u32> {
    let event = objc2_core_graphics::CGEvent::new(None)?;
    let at = objc2_core_graphics::CGEvent::location(Some(&event));
    super::cg::on_screen_hit_candidates(24, 25)
        .into_iter()
        .find(|(_, layer, bounds)| *layer == 0 && bounds.contains((at.x, at.y)))
        .map(|(target, _, _)| target.window_id)
}

/// A macOS prompt is on screen.
fn system_prompt() -> bool {
    super::cg::on_screen_windows().iter().any(|w| {
        (0..1000).contains(&w.layer) && agent_screen_model::alerts::is_system_alert(&w.owner_name)
    })
}

/// An input method composes: it is the current source and its process
/// shows a window on a user screen, its candidate window.
fn composing() -> bool {
    let Some(bundle) = super::ime::input_method_bundle() else {
        return false;
    };
    let pids: BTreeSet<i32> = super::app::pids_of_bundle_id(&bundle).into_iter().collect();
    if pids.is_empty() {
        return false;
    }
    super::cg::on_screen_where(|pid, _, bounds| pids.contains(&pid) && on_user_screen(&bounds))
        .iter()
        .any(|w| w.bounds.width > 1.0 && w.bounds.height > 1.0)
}

fn on_user_screen(bounds: &crate::model::Rect) -> bool {
    let mid = bounds.mid();
    super::cg::active_displays()
        .into_iter()
        .find(|&id| super::cg::display_bounds(id).contains(mid))
        .is_some_and(|id| super::cg::window_display_role(id) == crate::model::DisplayRole::User)
}

/// The on-screen windows on user screens of apps holding a display-sleep
/// assertion: video, a call. Read every 5 s.
fn media_windows() -> Vec<u32> {
    {
        let s = state();
        if let (Some(at), windows) = &s.media
            && at.elapsed() < Duration::from_secs(5)
        {
            return windows.clone();
        }
    }
    let pids = display_sleep_pids();
    let windows: Vec<u32> = if pids.is_empty() {
        Vec::new()
    } else {
        super::cg::on_screen_where(|pid, layer, bounds| {
            layer == 0 && pids.contains(&pid) && on_user_screen(&bounds)
        })
        .iter()
        .map(|w| w.target.window_id)
        .collect()
    };
    state().media = (Some(Instant::now()), windows.clone());
    windows
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMCopyAssertionsByProcess(out: *mut *const std::ffi::c_void) -> i32;
}

/// The pids holding `PreventUserIdleDisplaySleep`.
fn display_sleep_pids() -> BTreeSet<i32> {
    use objc2::rc::Retained;
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
    let mut raw: *const std::ffi::c_void = std::ptr::null();
    if unsafe { IOPMCopyAssertionsByProcess(&mut raw) } != 0 || raw.is_null() {
        return BTreeSet::new();
    }
    // SAFETY: a +1 CFDictionary of pid (CFNumber) to an array of assertion
    // dictionaries, toll-free bridged.
    let Some(dict) = (unsafe { Retained::from_raw(raw as *mut NSDictionary<NSNumber, NSArray>) })
    else {
        return BTreeSet::new();
    };
    let key = NSString::from_str("AssertType");
    let mut pids = BTreeSet::new();
    for pid in dict.allKeys().iter() {
        let Some(list) = dict.objectForKey(&pid) else {
            continue;
        };
        let holds = list.iter().any(|a| {
            a.downcast::<NSDictionary>()
                .ok()
                .and_then(|d| d.objectForKey(&key))
                .and_then(|t| t.downcast::<NSString>().ok())
                .is_some_and(|t| {
                    let t = t.to_string();
                    t == "PreventUserIdleDisplaySleep" || t == "NoDisplaySleepAssertion"
                })
        });
        if holds {
            pids.insert(pid.intValue());
        }
    }
    pids
}

/// Observe the screen locking, the Mac sleeping and the user session being
/// switched out: each ends YOLO.
pub fn watch_session() {
    use super::native::{Native, send};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSNotification, NSOperationQueue, NSString};
    use std::ptr::NonNull;
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace().notificationCenter();
    for (name, why) in [
        (
            unsafe { objc2_app_kit::NSWorkspaceWillSleepNotification },
            YoloEnd::Slept,
        ),
        (
            unsafe { objc2_app_kit::NSWorkspaceSessionDidResignActiveNotification },
            YoloEnd::SwitchedOut,
        ),
    ] {
        let block = RcBlock::new(move |_: NonNull<NSNotification>| send(Native::YoloEnds(why)));
        let observer = unsafe {
            workspace.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        let observer: Retained<AnyObject> = observer.into();
        super::native::keep(observer);
    }
    let distributed = objc2_foundation::NSDistributedNotificationCenter::defaultCenter();
    let locked = NSString::from_str("com.apple.screenIsLocked");
    let block = RcBlock::new(|_: NonNull<NSNotification>| send(Native::YoloEnds(YoloEnd::Locked)));
    let observer = unsafe {
        distributed.addObserverForName_object_queue_usingBlock(
            Some(&locked),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    };
    let observer: Retained<AnyObject> = observer.into();
    super::native::keep(observer);
}
