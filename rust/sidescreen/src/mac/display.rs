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
    fn CGDisplayCopyAllDisplayModes(display: u32, options: *const c_void) -> *const c_void;
    static kCGDisplayShowDuplicateLowResolutionModes: core_foundation::string::CFStringRef;
    fn CGDisplayCopyDisplayMode(display: u32) -> *const c_void;
    fn CGDisplayModeGetWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeGetHeight(mode: *const c_void) -> usize;
    fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeRelease(mode: *const c_void);
    fn CGConfigureDisplayWithDisplayMode(
        config: *mut c_void,
        display: u32,
        mode: *const c_void,
        options: *const c_void,
    ) -> i32;
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
    handle: Option<Retained<AnyObject>>,
    /// Its serial, free again once the display has gone.
    serial: Option<Serial>,
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        drop(self.handle.take());
        // WindowServer removes the display a moment later. Its serial waits
        // for that: a display with it now would be refused, or share its
        // identity with the one going.
        let Some(serial) = self.serial.take() else {
            return;
        };
        let id = self.id;
        std::thread::spawn(move || {
            let mut wait = std::time::Duration::from_millis(100);
            while cg::is_online(id) {
                std::thread::sleep(wait);
                wait = (wait * 2).min(std::time::Duration::from_secs(5));
            }
            drop(serial);
        });
    }
}

/// Serials of this app's displays, spares included, and of displays being
/// made.
static SERIALS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// A serial held for one display (`lowest_free_serial` says why), and the
/// first of its instance's range.
pub struct Serial(u32, u32);

impl Serial {
    /// The lowest serial from `first` no display of this app holds.
    pub fn lease(first: u32) -> Serial {
        let mut taken = SERIALS.lock().unwrap_or_else(|e| e.into_inner());
        let serial = sidescreen_model::model::lowest_free_serial(first, &taken);
        taken.insert(serial);
        Serial(serial, first)
    }

    pub fn get(&self) -> u32 {
        self.0
    }

    /// Its place in its instance's range, from 1.
    pub fn slot(&self) -> u32 {
        self.0 + 1 - self.1
    }
}

impl Drop for Serial {
    fn drop(&mut self) {
        SERIALS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

// Made on a background thread and then only held, never messaged, on the
// main thread, until released there.
unsafe impl Send for VirtualDisplay {}

/// Displays that got a taken unit number, kept online while Sidescreen has
/// displays. WindowServer numbers a new display by how many are online, so
/// once a unit is gone from the middle, as when the user unplugs a monitor
/// that came before AeroSpace's closet, every new display gets a taken one.
/// Making it again and releasing the twin each time is two display changes
/// more, and each change moves the other displays and may drop their
/// windows on the user's screen. A kept twin fills the gap.
static FILLERS: std::sync::Mutex<Vec<VirtualDisplay>> = std::sync::Mutex::new(Vec::new());
const MAX_FILLERS: usize = 2;

/// Ids of the kept twins.
pub fn filler_ids() -> Vec<u32> {
    FILLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|d| d.id)
        .collect()
}

/// Release the kept twins, once Sidescreen has no other display.
pub fn release_fillers() {
    let fillers = std::mem::take(&mut *FILLERS.lock().unwrap_or_else(|e| e.into_inner()));
    for filler in fillers {
        crate::lifecycle::note("released", filler.id, "filler");
    }
}

impl VirtualDisplay {
    /// A HiDPI display of `width` × `height` points, with a unit number no
    /// other display has (`cg::unit_twin`). A display that gets a taken one
    /// stays until another comes, so WindowServer hands that one the next
    /// unit; up to three tries, each with the next free serial, so a retry
    /// leaves no identity that never comes back.
    pub fn create(name: &str, width: u32, height: u32, serial: Serial) -> Option<Self> {
        let first = serial.1;
        let mut serial = Some(serial);
        let mut twinned: Vec<Self> = Vec::new();
        for _ in 0..3u32 {
            let lease = serial.take().unwrap_or_else(|| Serial::lease(first));
            let number = lease.get();
            let mut display = Self::create_once(name, width, height, number)?;
            display.serial = Some(lease);
            crate::lifecycle::note("created", display.id, &format!("serial={number}"));
            let Some(twin) = cg::unit_twin(display.id) else {
                let mut fillers = FILLERS.lock().unwrap_or_else(|e| e.into_inner());
                for twinned in twinned {
                    if fillers.len() < MAX_FILLERS {
                        crate::lifecycle::note("kept", twinned.id, "filler");
                        fillers.push(twinned);
                    } else {
                        crate::lifecycle::note("released", twinned.id, "unit twin");
                    }
                }
                return Some(display);
            };
            log::warn!(
                "display {} got the unit number of display {twin}; making another",
                display.id
            );
            twinned.push(display);
        }
        // Still twinned: capture refuses it, rather than show the other.
        twinned.pop()
    }

    /// Its serial, and the serial's place in its instance's range, from 1.
    pub fn serial(&self) -> Option<(u32, u32)> {
        self.serial.as_ref().map(|s| (s.get(), s.slot()))
    }

    fn create_once(name: &str, width: u32, height: u32, serial: u32) -> Option<Self> {
        let (hz, scale) = mode_settings();
        unsafe {
            let descriptor: Retained<AnyObject> =
                msg_send![class("CGVirtualDisplayDescriptor")?, new];
            // The main dispatch queue is an object; pass it as one.
            let main_queue = &*(&raw const _dispatch_main_q).cast::<AnyObject>();
            let _: () = msg_send![&descriptor, setQueue: main_queue];
            let _: () = msg_send![&descriptor, setName: &*NSString::from_str(name)];
            let _: () = msg_send![&descriptor, setMaxPixelsWide: width * scale];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: height * scale];
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
                initWithWidth: width, height: height, refreshRate: hz
            ];
            let settings: Retained<AnyObject> = msg_send![class("CGVirtualDisplaySettings")?, new];
            let _: () = msg_send![&settings, setHiDPI: u32::from(scale == 2)];
            let modes = NSArray::from_retained_slice(&[mode]);
            let _: () = msg_send![&settings, setModes: &*modes];
            let applied: Bool = msg_send![&handle, applySettings: &*settings];
            if !applied.as_bool() {
                return None;
            }
            let id: u32 = msg_send![&handle, displayID];
            // WindowServer refused it, as it does a display whose identity
            // another online display has.
            if id == 0 {
                return None;
            }
            // macOS keeps the resolution last used for a display's serial,
            // and serials repeat across launches: a display that once ran
            // at another size comes back at it, maybe at 1x, and shows
            // blurry. Ask for this size at 2x. A display WindowServer has
            // only just added may ignore it, so try for a moment.
            for _ in 0..25 {
                if scale == 1 || use_hidpi_mode(id, width, height) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Some(Self {
                id,
                handle: Some(handle),
                serial: None,
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
    let Some(anchor) = physical_bounds(id, &others, virtual_ids) else {
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

/// The union of the user's own displays, or of the main display when all
/// are virtual.
fn physical_bounds(id: u32, others: &[u32], virtual_ids: &[u32]) -> Option<crate::model::Rect> {
    let physical: Vec<u32> = others
        .iter()
        .copied()
        .filter(|&d| d != id && !virtual_ids.contains(&d) && !cg::is_sidescreen(d))
        .collect();
    let anchor_ids = if physical.is_empty() {
        vec![cg::main_display()]
    } else {
        physical
    };
    anchor_ids
        .iter()
        .map(|&d| cg::display_bounds(d))
        .reduce(|a, b| a.union(&b))
}

/// Whether display `id` still sits where `tuck_away` parks displays: above
/// and right of the user's displays, out of the cursor's way. The user may
/// have dragged it elsewhere in System Settings.
pub fn is_tucked(id: u32, virtual_ids: &[u32]) -> bool {
    let others = cg::active_displays();
    let Some(anchor) = physical_bounds(id, &others, virtual_ids) else {
        return true;
    };
    let bounds = cg::display_bounds(id);
    bounds.max_y() <= anchor.y && bounds.x >= anchor.max_x()
}

/// Refresh rate and backing scale of new displays. Test builds read them
/// from SIDESCREEN_DISPLAY_HZ and SIDESCREEN_DISPLAY_SCALE, to measure
/// what each costs.
fn mode_settings() -> (f64, u32) {
    let read = |name: &str| {
        cfg!(debug_assertions)
            .then(|| std::env::var(name).ok())
            .flatten()
    };
    let hz = read("SIDESCREEN_DISPLAY_HZ")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60.0);
    let scale = read("SIDESCREEN_DISPLAY_SCALE")
        .and_then(|v| v.parse().ok())
        .filter(|s| *s == 1 || *s == 2)
        .unwrap_or(2);
    (hz, scale)
}

/// Switch display `id` to `width` × `height` points at 2x. True once it
/// runs that way.
fn use_hidpi_mode(id: u32, width: u32, height: u32) -> bool {
    let (width, height) = (width as usize, height as usize);
    let is_wanted = |mode: *const c_void| unsafe {
        CGDisplayModeGetWidth(mode) == width
            && CGDisplayModeGetHeight(mode) == height
            && CGDisplayModeGetPixelWidth(mode) == width * 2
    };
    unsafe {
        let current = CGDisplayCopyDisplayMode(id);
        if !current.is_null() {
            let done = is_wanted(current);
            CGDisplayModeRelease(current);
            if done {
                return true;
            }
        }
        use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
        use core_foundation::base::CFRelease;
        use core_foundation::base::TCFType;
        // HiDPI modes only show among the "duplicate" ones.
        let options = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(
            core_foundation::string::CFString::wrap_under_get_rule(
                kCGDisplayShowDuplicateLowResolutionModes,
            ),
            core_foundation::boolean::CFBoolean::true_value(),
        )]);
        let modes = CGDisplayCopyAllDisplayModes(id, options.as_concrete_TypeRef() as *const c_void)
            as CFArrayRef;
        if modes.is_null() {
            return false;
        }
        let wanted = (0..CFArrayGetCount(modes))
            .map(|i| CFArrayGetValueAtIndex(modes, i))
            .find(|&mode| is_wanted(mode));
        log::debug!(
            "display {id}: {} modes, {width}×{height} at 2x among them: {}",
            CFArrayGetCount(modes),
            wanted.is_some()
        );
        if let Some(mode) = wanted {
            let mut config = std::ptr::null_mut();
            if CGBeginDisplayConfiguration(&mut config) == 0 {
                let set = CGConfigureDisplayWithDisplayMode(config, id, mode, std::ptr::null());
                // kCGConfigurePermanently: macOS remembers it for the
                // serial, so the next display with it starts right.
                let done = CGCompleteDisplayConfiguration(config, 2);
                log::debug!("display {id}: mode set {set}, completed {done}");
            }
        }
        CFRelease(modes as *const c_void);
    }
    false
}

fn class(name: &str) -> Option<&'static AnyClass> {
    let name = std::ffi::CString::new(name).ok()?;
    AnyClass::get(&name)
}
