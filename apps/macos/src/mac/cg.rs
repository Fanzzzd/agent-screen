//! CoreGraphics lookups: displays and the on-screen window list.

use crate::model::{DisplayId, Rect, WindowTarget};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{
    CGDisplayBounds, CGGetActiveDisplayList, CGMainDisplayID, CGWindowListCopyWindowInfo,
    CGWindowListCreate, CGWindowListOption,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

pub fn rect(r: CGRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

unsafe extern "C" {
    fn CGDisplayVendorNumber(display: u32) -> u32;
    fn CGDisplayModelNumber(display: u32) -> u32;
    fn CGDisplayUnitNumber(display: u32) -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayRegisterReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut std::ffi::c_void),
        user_info: *mut std::ffi::c_void,
    ) -> i32;
}

/// When a display last began or finished changing, in ms since `SINCE`.
static CHANGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SINCE: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

extern "C" fn display_changed(_: u32, _: u32, _: *mut std::ffi::c_void) {
    let now = SINCE.elapsed().as_millis() as u64;
    CHANGED.store(now.max(1), std::sync::atomic::Ordering::Relaxed);
}

/// Start noting display changes, once. Call on the main thread, whose run
/// loop delivers them.
pub fn watch_display_changes() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = *SINCE;
        unsafe { CGDisplayRegisterReconfigurationCallback(display_changed, std::ptr::null_mut()) };
    });
}

/// Whether a display began or finished changing within `span`. Meanwhile
/// display bounds and the window list disagree: a display has moved and
/// its windows not yet, or the other way round.
pub fn displays_changed_within(span: std::time::Duration) -> bool {
    let at = CHANGED.load(std::sync::atomic::Ordering::Relaxed);
    at != 0 && SINCE.elapsed().as_millis() as u64 <= at + span.as_millis() as u64
}

/// The display's unit number, which ScreenCaptureKit tells displays apart by.
pub fn unit_number(id: DisplayId) -> u32 {
    unsafe { CGDisplayUnitNumber(id) }
}

/// Another online display with the same unit number as `id`, the closet
/// included. ScreenCaptureKit tells displays apart by unit number: streams
/// and screenshots of either show the same one, maybe the other's.
/// WindowServer can hand a new virtual display a unit that AeroSpace's
/// closet already has.
pub fn unit_twin(id: DisplayId) -> Option<DisplayId> {
    let mut ids = [0u32; 32];
    let mut count = 0u32;
    unsafe { CGGetOnlineDisplayList(32, ids.as_mut_ptr(), &mut count) };
    let unit = unsafe { CGDisplayUnitNumber(id) };
    ids[..count as usize]
        .iter()
        .copied()
        .find(|&other| other != id && unsafe { CGDisplayUnitNumber(other) } == unit)
}

/// Whether `id` is among the online displays.
pub fn is_online(id: DisplayId) -> bool {
    online().contains(&id)
}

/// The online displays, the closet included.
pub fn online() -> Vec<DisplayId> {
    let mut ids = [0u32; 64];
    let mut count = 0u32;
    unsafe { CGGetOnlineDisplayList(64, ids.as_mut_ptr(), &mut count) };
    ids[..count as usize].to_vec()
}

/// Whether a display is one of Agent Screen's, from this app or another.
pub fn is_agent_screen(id: DisplayId) -> bool {
    unsafe { CGDisplayVendorNumber(id) == 0x5353 }
}

/// Ownership classification only. Layout, capture and the independent e2e
/// watcher keep using the actual hardware identity.
pub fn window_display_role(id: DisplayId) -> crate::model::DisplayRole {
    #[cfg(debug_assertions)]
    let simulated_user = DEBUG_USER_STAGE
        .lock()
        .unwrap()
        .is_some_and(|(stage, _)| stage == id);
    #[cfg(not(debug_assertions))]
    let simulated_user = false;
    unsafe {
        crate::model::DisplayRole::from_hardware(
            CGDisplayVendorNumber(id),
            CGDisplayModelNumber(id),
            simulated_user,
        )
    }
}

#[cfg(debug_assertions)]
static DEBUG_USER_STAGE: std::sync::Mutex<Option<(DisplayId, u64)>> = std::sync::Mutex::new(None);

#[cfg(debug_assertions)]
pub fn debug_user_stage(id: DisplayId, connection: u64) {
    *DEBUG_USER_STAGE.lock().unwrap() = Some((id, connection));
}

#[cfg(debug_assertions)]
pub fn clear_user_stage(id: DisplayId) {
    let mut stage = DEBUG_USER_STAGE.lock().unwrap();
    if stage.is_some_and(|(held, _)| held == id) {
        *stage = None;
    }
}

#[cfg(debug_assertions)]
pub fn clear_user_stage_connection(connection: u64) {
    let mut stage = DEBUG_USER_STAGE.lock().unwrap();
    if stage.is_some_and(|(_, owner)| owner == connection) {
        *stage = None;
    }
}

/// Whether a display is AeroSpace's closet, where it keeps the windows of
/// invisible workspaces. Nobody looks at it: Agent Screen treats it as if it
/// weren't there.
pub fn is_closet(id: DisplayId) -> bool {
    unsafe { CGDisplayVendorNumber(id) == 0x4145 && CGDisplayModelNumber(id) == 1 }
}

/// A display's size in pixels, None when it isn't online.
pub fn pixel_size(id: DisplayId) -> Option<(u32, u32)> {
    let size = unsafe { (CGDisplayPixelsWide(id), CGDisplayPixelsHigh(id)) };
    (size.0 > 0 && size.1 > 0).then_some((size.0 as u32, size.1 as u32))
}

pub fn display_bounds(id: DisplayId) -> Rect {
    rect(CGDisplayBounds(id))
}

pub fn main_display() -> DisplayId {
    CGMainDisplayID()
}

pub fn active_displays() -> Vec<DisplayId> {
    let mut ids = [0u32; 32];
    let mut count = 0u32;
    unsafe { CGGetActiveDisplayList(32, ids.as_mut_ptr(), &mut count) };
    ids[..count as usize]
        .iter()
        .copied()
        .filter(|&id| !is_closet(id))
        .collect()
}

/// Ids of every window, hidden, minimized and on other Spaces included,
/// so one that only comes back into view isn't taken for new.
pub fn all_ids() -> std::collections::HashSet<u32> {
    window_ids(CGWindowListOption::OptionAll)
}

/// Ids of the on-screen windows, without their descriptions: a fraction of
/// the cost of `on_screen_windows`, for telling new windows from old.
pub fn on_screen_ids() -> std::collections::HashSet<u32> {
    window_ids(CGWindowListOption::OptionOnScreenOnly)
}

fn window_ids(option: CGWindowListOption) -> std::collections::HashSet<u32> {
    let Some(list) = CGWindowListCreate(option, 0) else {
        return Default::default();
    };
    // The array holds the CGWindowIDs themselves, not CFNumbers.
    let count = unsafe { cf::CFArrayGetCount(&list) };
    (0..count)
        .map(|i| unsafe { cf::CFArrayGetValueAtIndex(&list, i) } as usize as u32)
        .collect()
}

/// One entry of WindowServer's on-screen window list.
#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub target: WindowTarget,
    pub owner_name: String,
    pub title: String,
    /// Global bounds, top-left origin, points.
    pub bounds: Rect,
    pub layer: i64,
    pub alpha: f64,
}

/// On-screen windows of other processes, front to back, desktop elements left out.
pub fn on_screen_windows() -> Vec<WindowInfo> {
    window_list(CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements)
}

/// Every normal window of other processes, hidden, minimized and on other
/// Spaces included.
pub fn all_windows() -> Vec<WindowInfo> {
    // Each app also owns a 30-point menu bar strip on every display.
    window_list_where(
        CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
        0,
        |_, layer, b| layer == 0 && b.width > 40.0 && b.height > 40.0,
    )
}

/// Windows that draw another app's panel out of process, as the file Open
/// and Save panels do. The app owns an empty window in front; WindowServer
/// sends input to this one, which it lists as off screen, at the same
/// place.
pub fn remote_panels() -> Vec<WindowInfo> {
    // The full list runs to a couple of thousand windows: the pid, layer and
    // size sort out all but a few before anything else is read.
    window_list_where(CGWindowListOption::OptionAll, 0, |pid, layer, b| {
        // Normal windows, and modal panels at their own level, as Firefox's.
        (0..1000).contains(&layer) && b.width > 100.0 && b.height > 100.0 && is_xpc_service(pid)
    })
}

/// Whether a process runs from an XPC service bundle. A process's path
/// stays for its life, so each answer is kept while the pid lives on.
fn is_xpc_service(pid: i32) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static KNOWN: Mutex<Option<HashMap<i32, bool>>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let known = known.get_or_insert_with(HashMap::new);
    if let Some(&xpc) = known.get(&pid) {
        return xpc;
    }
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if n <= 0 {
        // Gone, or not ours to read: ask again next time.
        return false;
    }
    let xpc = String::from_utf8_lossy(&path[..n as usize]).contains(".xpc/Contents/MacOS/");
    // A reused pid is a new process: forget the lot now and then.
    if known.len() > 4096 {
        known.clear();
    }
    known.insert(pid, xpc);
    xpc
}

/// A pre-show gate cannot infer ownership from a caller-supplied pid.
#[cfg(debug_assertions)]
pub fn hidden_window_owner(window: u32) -> Option<i32> {
    window_list_where(
        CGWindowListOption::OptionIncludingWindow,
        window,
        |_, _, _| true,
    )
    .into_iter()
    .find(|w| w.target.window_id == window)
    .map(|w| w.target.pid)
}

/// A window's global bounds, while it exists.
pub fn window_bounds(window_id: u32) -> Option<Rect> {
    window_list_where(
        CGWindowListOption::OptionIncludingWindow,
        window_id,
        |_, _, _| true,
    )
    .first()
    .map(|w| w.bounds)
}

fn window_list(options: CGWindowListOption) -> Vec<WindowInfo> {
    window_list_where(options, 0, |_, _, _| true)
}

/// Id, layer and bounds of every on-screen window, this app's included.
pub fn on_screen_frames() -> Vec<(u32, i64, Rect)> {
    let mut out = Vec::new();
    list_raw(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
        |_, window_id, layer, bounds, _| out.push((window_id, layer, bounds)),
    );
    out
}

mod cf {
    use std::ffi::c_void;
    pub type Ref = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFArrayGetCount(array: &objc2_core_foundation::CFArray) -> isize;
        pub fn CFArrayGetValueAtIndex(array: &objc2_core_foundation::CFArray, index: isize) -> Ref;
        pub fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
        pub fn CFNumberGetValue(number: Ref, kind: isize, out: *mut c_void) -> bool;
        pub fn CFStringGetCStringPtr(string: Ref, encoding: u32) -> *const std::ffi::c_char;
        pub fn CFStringGetCString(
            string: Ref,
            buffer: *mut std::ffi::c_char,
            size: isize,
            encoding: u32,
        ) -> bool;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub static kCGWindowNumber: Ref;
        pub static kCGWindowOwnerPID: Ref;
        pub static kCGWindowOwnerName: Ref;
        pub static kCGWindowName: Ref;
        pub static kCGWindowBounds: Ref;
        pub static kCGWindowLayer: Ref;
        pub static kCGWindowAlpha: Ref;
        pub fn CGRectMakeWithDictionaryRepresentation(
            dict: Ref,
            rect: *mut objc2_core_foundation::CGRect,
        ) -> bool;
    }
    const SINT64: isize = 4;
    const FLOAT64: isize = 6;
    const UTF8: u32 = 0x0800_0100;

    pub unsafe fn int(dict: Ref, key: Ref) -> Option<i64> {
        let value = unsafe { CFDictionaryGetValue(dict, key) };
        let mut out = 0i64;
        (!value.is_null() && unsafe { CFNumberGetValue(value, SINT64, (&raw mut out).cast()) })
            .then_some(out)
    }

    pub unsafe fn float(dict: Ref, key: Ref) -> Option<f64> {
        let value = unsafe { CFDictionaryGetValue(dict, key) };
        let mut out = 0f64;
        (!value.is_null() && unsafe { CFNumberGetValue(value, FLOAT64, (&raw mut out).cast()) })
            .then_some(out)
    }

    pub unsafe fn string(dict: Ref, key: Ref) -> String {
        let value = unsafe { CFDictionaryGetValue(dict, key) };
        if value.is_null() {
            return String::new();
        }
        let fast = unsafe { CFStringGetCStringPtr(value, UTF8) };
        if !fast.is_null() {
            return unsafe { std::ffi::CStr::from_ptr(fast) }
                .to_string_lossy()
                .into_owned();
        }
        let mut buffer = [0 as std::ffi::c_char; 1024];
        if unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) } {
            unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        }
    }
}

/// The window list, read straight from CoreFoundation with the framework's
/// own keys: nothing is allocated for a window `keep` turns down, given its
/// pid, layer and bounds. This app's own windows are left out.
fn window_list_where(
    options: CGWindowListOption,
    relative: u32,
    keep: impl Fn(i32, i64, Rect) -> bool,
) -> Vec<WindowInfo> {
    let me = std::process::id() as i32;
    let mut out = Vec::new();
    list_raw(options, relative, |pid, window_id, layer, bounds, info| {
        if pid == me || !keep(pid, layer, bounds) {
            return;
        }
        out.push(WindowInfo {
            target: WindowTarget { pid, window_id },
            owner_name: unsafe { cf::string(info, cf::kCGWindowOwnerName) },
            title: unsafe { cf::string(info, cf::kCGWindowName) },
            bounds,
            layer,
            alpha: unsafe { cf::float(info, cf::kCGWindowAlpha) }.unwrap_or(1.0),
        });
    });
    out
}

/// Each window's pid, id, layer and bounds, and its description for more.
fn list_raw(
    options: CGWindowListOption,
    relative: u32,
    mut each: impl FnMut(i32, u32, i64, Rect, cf::Ref),
) {
    let Some(list) = CGWindowListCopyWindowInfo(options, relative) else {
        return;
    };
    let count = unsafe { cf::CFArrayGetCount(&list) };
    for i in 0..count {
        unsafe {
            let info = cf::CFArrayGetValueAtIndex(&list, i);
            let Some(pid) = cf::int(info, cf::kCGWindowOwnerPID).map(|p| p as i32) else {
                continue;
            };
            let Some(window_id) = cf::int(info, cf::kCGWindowNumber).map(|n| n as u32) else {
                continue;
            };
            let layer = cf::int(info, cf::kCGWindowLayer).unwrap_or(0);
            let mut r = CGRect::default();
            let dict = cf::CFDictionaryGetValue(info, cf::kCGWindowBounds);
            if !dict.is_null() {
                cf::CGRectMakeWithDictionaryRepresentation(dict, &mut r);
            }
            each(pid, window_id, layer, rect(r), info);
        }
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *mut AnyObject;
}

/// Whether the user's session shows the lock screen. macOS then blocks
/// input to apps and lists no windows, so nothing on a display works.
pub fn screen_locked() -> bool {
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    let Some(session) = (unsafe { Retained::from_raw(raw) }) else {
        return false;
    };
    let session: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { Retained::cast_unchecked(session) };
    session
        .objectForKey(&NSString::from_str("CGSSessionScreenIsLocked"))
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .is_some_and(|n| n.boolValue())
}

#[cfg(all(test, debug_assertions))]
mod stage_tests {
    use super::*;

    #[test]
    fn the_user_stage_role_ends_with_its_display_or_connection() {
        debug_user_stage(7, 11);
        clear_user_stage(8);
        clear_user_stage_connection(12);
        assert_eq!(*DEBUG_USER_STAGE.lock().unwrap(), Some((7, 11)));
        clear_user_stage(7);
        assert_eq!(*DEBUG_USER_STAGE.lock().unwrap(), None);
        debug_user_stage(7, 11);
        clear_user_stage_connection(11);
        assert_eq!(*DEBUG_USER_STAGE.lock().unwrap(), None);
    }
}
