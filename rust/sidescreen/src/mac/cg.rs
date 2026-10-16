//! CoreGraphics lookups: displays and the on-screen window list.

use crate::model::{DisplayId, Rect, WindowTarget};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFArray, CGRect};
use objc2_core_graphics::{
    CGDisplayBounds, CGGetActiveDisplayList, CGMainDisplayID, CGWindowListCopyWindowInfo,
    CGWindowListCreate, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

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

/// Whether a display is one of Sidescreen's, from this app or another.
pub fn is_sidescreen(id: DisplayId) -> bool {
    unsafe { CGDisplayVendorNumber(id) == 0x5353 }
}

/// Whether a display is AeroSpace's closet, where it keeps the windows of
/// invisible workspaces. Nobody looks at it: Sidescreen treats it as if it
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
    unsafe extern "C" {
        fn CFArrayGetCount(array: &CFArray) -> isize;
        fn CFArrayGetValueAtIndex(array: &CFArray, index: isize) -> *const std::ffi::c_void;
    }
    let Some(list) = CGWindowListCreate(option, 0) else {
        return Default::default();
    };
    // The array holds the CGWindowIDs themselves, not CFNumbers.
    let count = unsafe { CFArrayGetCount(&list) };
    (0..count)
        .map(|i| unsafe { CFArrayGetValueAtIndex(&list, i) } as usize as u32)
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
    window_list(CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements)
        .into_iter()
        // Each app also owns a 30-point menu bar strip on every display.
        .filter(|w| w.layer == 0 && w.bounds.width > 40.0 && w.bounds.height > 40.0)
        .collect()
}

/// Windows that draw another app's panel out of process, as the file Open
/// and Save panels do. The app owns an empty window in front; WindowServer
/// sends input to this one, which it lists as off screen, at the same
/// place.
pub fn remote_panels() -> Vec<WindowInfo> {
    let mut services: Vec<(i32, bool)> = Vec::new();
    window_list(CGWindowListOption::OptionAll)
        .into_iter()
        // Normal windows, and modal panels at their own level, as Firefox's.
        .filter(|w| {
            (0..1000).contains(&w.layer) && w.bounds.width > 100.0 && w.bounds.height > 100.0
        })
        .filter(|w| {
            let pid = w.target.pid;
            if let Some(&(_, xpc)) = services.iter().find(|(p, _)| *p == pid) {
                return xpc;
            }
            let xpc = is_xpc_service(pid);
            services.push((pid, xpc));
            xpc
        })
        .collect()
}

fn is_xpc_service(pid: i32) -> bool {
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    n > 0 && String::from_utf8_lossy(&path[..n as usize]).contains(".xpc/Contents/MacOS/")
}

/// A window's global bounds, top-left origin, points.
pub fn window_bounds(window_id: u32) -> Option<Rect> {
    window_list_from(CGWindowListOption::OptionIncludingWindow, window_id)
        .first()
        .map(|w| w.bounds)
}

fn window_list(options: CGWindowListOption) -> Vec<WindowInfo> {
    window_list_from(options, 0)
}

fn window_list_from(options: CGWindowListOption, relative: u32) -> Vec<WindowInfo> {
    let Some(list) = CGWindowListCopyWindowInfo(options, relative) else {
        return Vec::new();
    };
    // CFArray of CFDictionary is toll-free bridged to NSArray of NSDictionary.
    let list: &NSArray<NSDictionary<NSString, AnyObject>> = unsafe {
        &*(&*list as *const CFArray as *const NSArray<NSDictionary<NSString, AnyObject>>)
    };
    let me = std::process::id() as i32;
    list.iter()
        .filter_map(|info| {
            let pid = number(&info, "kCGWindowOwnerPID")?.intValue();
            if pid == me {
                return None;
            }
            let window_id = number(&info, "kCGWindowNumber")?.unsignedIntValue();
            let bounds = info.objectForKey(&NSString::from_str("kCGWindowBounds"))?;
            let bounds: &NSDictionary<NSString, NSNumber> =
                unsafe { &*(&*bounds as *const AnyObject).cast() };
            let get = |key: &str| {
                bounds
                    .objectForKey(&NSString::from_str(key))
                    .map(|n| n.doubleValue())
                    .unwrap_or(0.0)
            };
            Some(WindowInfo {
                target: WindowTarget { pid, window_id },
                owner_name: string(&info, "kCGWindowOwnerName").unwrap_or_default(),
                title: string(&info, "kCGWindowName").unwrap_or_default(),
                bounds: Rect::new(get("X"), get("Y"), get("Width"), get("Height")),
                layer: number(&info, "kCGWindowLayer").map_or(0, |n| n.longLongValue()),
                alpha: number(&info, "kCGWindowAlpha").map_or(1.0, |n| n.doubleValue()),
            })
        })
        .collect()
}

fn number(info: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<Retained<NSNumber>> {
    info.objectForKey(&NSString::from_str(key))?
        .downcast::<NSNumber>()
        .ok()
}

fn string(info: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<String> {
    Some(
        info.objectForKey(&NSString::from_str(key))?
            .downcast::<NSString>()
            .ok()?
            .to_string(),
    )
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
