//! A CGVirtualDisplay, created through the private CoreGraphics classes the
//! Swift app uses, and parked to the right of the user's screens.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_foundation::{NSArray, NSSize as CGSize, NSString};
use std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGMainDisplayID() -> u32;
    fn CGBeginDisplayConfiguration(config: *mut *mut c_void) -> i32;
    fn CGConfigureDisplayOrigin(config: *mut c_void, display: u32, x: i32, y: i32) -> i32;
    fn CGCompleteDisplayConfiguration(config: *mut c_void, option: u32) -> i32;
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CGRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub struct VirtualDisplay {
    pub id: u32,
    /// Keeps the display alive; macOS removes it when this is released.
    _handle: Retained<AnyObject>,
}

impl VirtualDisplay {
    /// A HiDPI display of `width` × `height` points.
    pub fn create(name: &str, width: u32, height: u32, serial: u32) -> Option<Self> {
        unsafe {
            let descriptor: Retained<AnyObject> =
                msg_send![class("CGVirtualDisplayDescriptor")?, new];
            let main_queue = &raw const _dispatch_main_q;
            let _: () = msg_send![&descriptor, setQueue: main_queue];
            let _: () = msg_send![&descriptor, setName: &*NSString::from_str(name)];
            let _: () = msg_send![&descriptor, setMaxPixelsWide: width * 2];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: height * 2];
            // A ~110 ppi panel, so macOS picks sensible default scaling.
            let mm = 25.4 / 110.0;
            let physical = CGSize::new(width as f64 * mm, height as f64 * mm);
            let _: () = msg_send![&descriptor, setSizeInMillimeters: physical];
            let _: () = msg_send![&descriptor, setVendorID: 0x5353u32];
            let _: () = msg_send![&descriptor, setProductID: 0x0002u32];
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

    /// Move the display to the right of every other screen, level with the
    /// bottom of the physical ones, so the user's cursor does not wander in.
    pub fn park(&self) {
        unsafe {
            let mut ids = [0u32; 32];
            let mut count = 0u32;
            CGGetActiveDisplayList(32, ids.as_mut_ptr(), &mut count);
            let others: Vec<u32> = ids[..count as usize]
                .iter()
                .copied()
                .filter(|&d| d != self.id)
                .collect();
            let bounds: Vec<CGRect> = if others.is_empty() {
                vec![CGDisplayBounds(CGMainDisplayID())]
            } else {
                others.iter().map(|&d| CGDisplayBounds(d)).collect()
            };
            let max_x = bounds
                .iter()
                .map(|b| b.x + b.width)
                .fold(f64::MIN, f64::max);
            let max_y = bounds
                .iter()
                .map(|b| b.y + b.height)
                .fold(f64::MIN, f64::max);
            let mut config = std::ptr::null_mut();
            if CGBeginDisplayConfiguration(&mut config) == 0 {
                CGConfigureDisplayOrigin(config, self.id, max_x as i32, max_y as i32);
                // kCGConfigureForSession
                CGCompleteDisplayConfiguration(config, 1);
            }
        }
    }

    pub fn bounds(&self) -> CGRect {
        unsafe { CGDisplayBounds(self.id) }
    }
}

fn class(name: &str) -> Option<&'static AnyClass> {
    let name = std::ffi::CString::new(name).ok()?;
    AnyClass::get(&name)
}
