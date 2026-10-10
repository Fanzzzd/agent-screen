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

/// Whether a display is one of Sidescreen's, from this app or another.
pub fn is_sidescreen(id: DisplayId) -> bool {
    unsafe extern "C" {
        fn CGDisplayVendorNumber(display: u32) -> u32;
    }
    unsafe { CGDisplayVendorNumber(id) == 0x5353 }
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
    ids[..count as usize].to_vec()
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
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, 0) else {
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
