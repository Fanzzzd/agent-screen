//! Keys through the user's input method, the one exception to typing in the
//! background (ADR 0008). An input method composes only for the frontmost
//! app, so for one call the agent's app becomes frontmost, the keys go in
//! as the keyboard's would, and the user's app comes back to the front.
//!
//! While it lasts the user's keyboard belongs to the agent: keys the user
//! types reach the agent's app. So it runs only after the user has left
//! the keyboard and pointer alone for a moment, stops the moment they touch
//! either, and lasts at most `LONGEST`.

use super::{app, ax};
use crate::keychord::KeyChord;
use crate::model::WindowTarget;
use objc2_core_foundation::{CFArray, CFBoolean, CFRetained, CFString};
use objc2_core_graphics::{CGEvent, CGEventSource, CGEventSourceStateID, CGEventTapLocation};
use std::thread;
use std::time::{Duration, Instant};

/// The user must have left the keyboard and pointer alone this long.
pub const QUIET: Duration = Duration::from_secs(2);
/// The longest an agent holds the keyboard in one call.
pub const LONGEST: Duration = Duration::from_secs(10);
/// Per key: down, up, and time for the input method to update.
const PER_KEY: Duration = Duration::from_millis(40);

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: CGEventSourceStateID, kind: u32) -> f64;
    fn TISCopyCurrentKeyboardInputSource() -> *mut std::ffi::c_void;
    fn TISCreateInputSourceList(
        properties: *const std::ffi::c_void,
        include_all: bool,
    ) -> *mut CFArray;
    fn TISSelectInputSource(source: *mut std::ffi::c_void) -> i32;
    fn TISGetInputSourceProperty(
        source: *mut std::ffi::c_void,
        key: *const std::ffi::c_void,
    ) -> *const std::ffi::c_void;
    static kTISPropertyInputSourceID: *const std::ffi::c_void;
    fn CFArrayGetCount(array: &CFArray) -> isize;
    fn CFArrayGetValueAtIndex(array: &CFArray, index: isize) -> *const std::ffi::c_void;
    fn CFRelease(object: *const std::ffi::c_void);
    fn CFRetain(object: *const std::ffi::c_void) -> *const std::ffi::c_void;
    fn IsSecureEventInputEnabled() -> bool;
}

/// Seconds since the user last touched the keyboard, pointer or trackpad.
/// Posted events don't count: they don't pass through the HID system.
pub fn user_idle() -> f64 {
    unsafe { CGEventSourceSecondsSinceLastEventType(CGEventSourceStateID::HIDSystemState, !0) }
}

/// How a borrow ended.
pub enum Typed {
    All,
    /// The user touched the keyboard or pointer after this many keys.
    Stopped(usize),
}

/// An input source, retained.
struct Source(*mut std::ffi::c_void);
unsafe impl Send for Source {}
impl Drop for Source {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

fn source_id(source: *mut std::ffi::c_void) -> String {
    let id = unsafe { TISGetInputSourceProperty(source, kTISPropertyInputSourceID) };
    if id.is_null() {
        return String::new();
    }
    unsafe { &*(id as *const CFString) }.to_string()
}

/// The input source with this id, if it is enabled.
fn find_source(id: &str) -> Option<Source> {
    let list = unsafe { TISCreateInputSourceList(std::ptr::null(), false) };
    if list.is_null() {
        return None;
    }
    let array = unsafe { &*list };
    let found = (0..unsafe { CFArrayGetCount(array) })
        .map(|i| unsafe { CFArrayGetValueAtIndex(array, i) } as *mut std::ffi::c_void)
        .find(|&s| source_id(s) == id)
        .map(|s| Source(unsafe { CFRetain(s) } as *mut _));
    unsafe { CFRelease(list as *const _) };
    found
}

/// Type `keys` into `target` through the input method, `source` if named,
/// else the user's current one. Blocking. An error says why nothing was
/// typed.
pub fn type_keys(
    target: WindowTarget,
    keys: &[KeyChord],
    source: Option<&str>,
) -> Result<Typed, String> {
    if PER_KEY * keys.len() as u32 > LONGEST {
        return Err(format!(
            "at most {} keys go through the input method in one call; split the text",
            LONGEST.as_millis() / PER_KEY.as_millis()
        ));
    }
    // A password field has the keyboard, and keys would land in it.
    if unsafe { IsSecureEventInputEnabled() } {
        return Err(
            "a password field on the user's screen has the keyboard (secure input is on), so keys can't go through the input method now".into(),
        );
    }
    let idle = user_idle();
    if idle < QUIET.as_secs_f64() {
        return Err(format!(
            "the user is using the keyboard or pointer (last input {idle:.1} s ago), and typing through the input method takes their keyboard; try again in a few seconds"
        ));
    }
    let wanted = match source {
        Some(id) => Some(find_source(id).ok_or_else(|| format!("no enabled input source {id}"))?),
        None => None,
    };
    let front = app::frontmost_pid();
    let before = Source(unsafe { TISCopyCurrentKeyboardInputSource() });

    // The agent's app comes to the front, its window key, the pointer
    // where it is.
    let application = ax::application(target.pid);
    ax::set_attribute(&application, "AXFrontmost", CFBoolean::new(true));
    ax::make_main(target);
    if let Some(wanted) = &wanted {
        unsafe { TISSelectInputSource(wanted.0) };
    }
    // Keys go to whichever app has keyboard focus. Unless it is the agent's,
    // send none: they would land in the user's app.
    let asked = Instant::now();
    while ax::focused_app_pid() != Some(target.pid) {
        if asked.elapsed() > Duration::from_secs(1) {
            let holder = ax::focused_app_pid();
            give_back(front, target.pid, wanted.is_some().then_some(&before));
            return Err(format!(
                "the app couldn't take keyboard focus: {} keeps it, so nothing was typed",
                holder
                    .and_then(app::name)
                    .unwrap_or_else(|| "another app".into())
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    let source_state = CGEventSource::new(CGEventSourceStateID::Private);
    let mut sent = 0;
    let mut stopped = false;
    for chord in keys {
        // The user came back: their keys must not reach the agent's app.
        if user_idle() < started.elapsed().as_secs_f64()
            || started.elapsed() > LONGEST
            || ax::focused_app_pid() != Some(target.pid)
            || unsafe { IsSecureEventInputEnabled() }
        {
            stopped = true;
            break;
        }
        for down in [true, false] {
            let Some(event): Option<CFRetained<CGEvent>> =
                CGEvent::new_keyboard_event(source_state.as_deref(), chord.code, down)
            else {
                continue;
            };
            CGEvent::set_flags(Some(&event), super::input::flags(chord.modifiers));
            CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
            thread::sleep(Duration::from_millis(10));
        }
        sent += 1;
        thread::sleep(PER_KEY - Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(100));

    give_back(front, target.pid, wanted.is_some().then_some(&before));
    log::info!(
        "typed {sent} of {} keys through the input method into pid {}; front app back to {front:?}",
        keys.len(),
        target.pid
    );
    Ok(if stopped {
        Typed::Stopped(sent)
    } else {
        Typed::All
    })
}

/// The user's input source, if the call changed it, and their front app.
fn give_back(front: Option<i32>, agent: i32, source: Option<&Source>) {
    if let Some(source) = source {
        unsafe { TISSelectInputSource(source.0) };
    }
    if let Some(front) = front.filter(|&p| p != agent) {
        ax::set_attribute(&ax::application(front), "AXFrontmost", CFBoolean::new(true));
    }
}
