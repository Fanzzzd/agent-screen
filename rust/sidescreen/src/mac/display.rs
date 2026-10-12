//! A CGVirtualDisplay, created through the private CoreGraphics classes
//! (the same interface DeskPad and BetterDisplay use).

use super::cg;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_foundation::{NSArray, NSSize as CGSize, NSString};
use std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBeginDisplayConfiguration(config: *mut *mut c_void) -> i32;
    fn CGConfigureDisplayOrigin(config: *mut c_void, display: u32, x: i32, y: i32) -> i32;
    fn CGCompleteDisplayConfiguration(config: *mut c_void, option: u32) -> i32;
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
}

pub struct VirtualDisplay {
    pub id: u32,
    /// Keeps the display alive; macOS removes it when this is released.
    _handle: Retained<AnyObject>,
}

// Made on a background thread and then only held, never messaged, on the
// main thread, until released there.
unsafe impl Send for VirtualDisplay {}

impl VirtualDisplay {
    /// A HiDPI display of `width` × `height` points.
    pub fn create(name: &str, width: u32, height: u32, serial: u32) -> Option<Self> {
        unsafe {
            let descriptor: Retained<AnyObject> =
                msg_send![class("CGVirtualDisplayDescriptor")?, new];
            // The main dispatch queue is an object; pass it as one.
            let main_queue = &*(&raw const _dispatch_main_q).cast::<AnyObject>();
            let _: () = msg_send![&descriptor, setQueue: main_queue];
            let _: () = msg_send![&descriptor, setName: &*NSString::from_str(name)];
            let _: () = msg_send![&descriptor, setMaxPixelsWide: width * 2];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: height * 2];
            // A ~110 ppi panel, so macOS picks sensible default scaling.
            let mm = 25.4 / 110.0;
            let physical = CGSize::new(width as f64 * mm, height as f64 * mm);
            let _: () = msg_send![&descriptor, setSizeInMillimeters: physical];
            let _: () = msg_send![&descriptor, setVendorID: 0x5353u32];
            let _: () = msg_send![&descriptor, setProductID: 0x0001u32];
            let _: () = msg_send![&descriptor, setSerialNum: serial];

            let handle: Option<Retained<AnyObject>> = msg_send![msg_send![class("CGVirtualDisplay")?, alloc], initWithDescriptor: &*descriptor];
            let handle = handle?;

            let mode: Retained<AnyObject> = msg_send![
                msg_send![class("CGVirtualDisplayMode")?, alloc],
                initWithWidth: width, height: height, refreshRate: 60.0f64
            ];
            let settings: Retained<AnyObject> = msg_send![class("CGVirtualDisplaySettings")?, new];
            let _: () = msg_send![&settings, setHiDPI: 1u32];
            let modes = NSArray::from_retained_slice(&[mode]);
            let _: () = msg_send![&settings, setModes: &*modes];
            let applied: Bool = msg_send![&handle, applySettings: &*settings];
            if !applied.as_bool() {
                return None;
            }
            let id: u32 = msg_send![&handle, displayID];
            Some(Self {
                id,
                _handle: handle,
            })
        }
    }
}

/// Park a virtual display diagonally off the top-right corner of the
/// physical displays, so the real cursor rarely wanders onto it. Further
/// virtual displays stack to the right of earlier ones.
///
/// Not the bottom corners: AeroSpace hides the windows of other workspaces
/// in a monitor's bottom corner, hanging off screen, and a display there
/// would show the user's hidden windows to agents.
///
/// True once the display sits there. A display WindowServer has only just
/// added ignores the configuration, so callers try again.
pub fn tuck_away(id: u32, virtual_ids: &[u32]) -> bool {
    let others: Vec<u32> = cg::active_displays()
        .into_iter()
        .filter(|&d| d != id)
        .collect();
    let physical: Vec<u32> = others
        .iter()
        .copied()
        .filter(|&d| !virtual_ids.contains(&d) && !cg::is_sidescreen(d))
        .collect();
    let anchor_ids = if physical.is_empty() {
        vec![cg::main_display()]
    } else {
        physical
    };
    let union = |ids: &[u32]| {
        ids.iter()
            .map(|&d| cg::display_bounds(d))
            .reduce(|a, b| a.union(&b))
    };
    let Some(anchor) = union(&anchor_ids) else {
        return false;
    };
    let occupied = others
        .iter()
        .filter(|&&d| virtual_ids.contains(&d) || cg::is_sidescreen(d))
        .map(|&d| cg::display_bounds(d))
        .fold(anchor, |a, b| a.union(&b));
    let height = cg::display_bounds(id).height;
    let origin = (occupied.max_x() as i32, (anchor.y - height) as i32);
    unsafe {
        let mut config = std::ptr::null_mut();
        if CGBeginDisplayConfiguration(&mut config) == 0 {
            CGConfigureDisplayOrigin(config, id, origin.0, origin.1);
            // kCGConfigureForSession
            CGCompleteDisplayConfiguration(config, 1);
        }
    }
    let now = cg::display_bounds(id);
    (now.x as i32, now.y as i32) == origin
}

fn class(name: &str) -> Option<&'static AnyClass> {
    let name = std::ffi::CString::new(name).ok()?;
    AnyClass::get(&name)
}
