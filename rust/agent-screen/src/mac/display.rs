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

pub struct VirtualDisplay {
    pub id: u32,
    /// Keeps the display alive; macOS removes it when this is released.
    handle: Option<Retained<AnyObject>>,
    /// The size it runs at, in points.
    size: (u32, u32),
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
        let serial = agent_screen_model::model::lowest_free_serial(first, &taken);
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

    /// Run at `width` × `height` points from now on: one display change,
    /// where a new display would be one more display online for every
    /// later change to cost. False when it can't, as for a display made
    /// with less room.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        if !unsafe { apply_mode(handle, width, height) } {
            return false;
        }
        settle_mode(self.id, width, height);
        let now = cg::display_bounds(self.id);
        self.size = (width, height);
        (now.width as u32, now.height as u32) == (width, height)
    }

    fn create_once(name: &str, width: u32, height: u32, serial: u32) -> Option<Self> {
        let (_, scale) = mode_settings();
        unsafe {
            let descriptor: Retained<AnyObject> =
                msg_send![class("CGVirtualDisplayDescriptor")?, new];
            // The display's own callbacks run on a queue of Sidescreen's,
            // not the main one, which draws the menu bar icon and Viewer.
            let _: () = msg_send![&descriptor, setQueue: callback_queue()];
            let _: () = msg_send![&descriptor, setName: &*NSString::from_str(name)];
            // Room for the largest size an agent may ask for, so a spare
            // can take any size later: a mode change, not a new display.
            let _: () = msg_send![&descriptor, setMaxPixelsWide: LARGEST.0.max(width) * scale];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: LARGEST.1.max(height) * scale];
            // A ~110 ppi panel, so macOS picks sensible default scaling.
            let mm = 25.4 / 110.0;
            let physical = CGSize::new(width as f64 * mm, height as f64 * mm);
            let _: () = msg_send![&descriptor, setSizeInMillimeters: physical];
            let _: () = msg_send![&descriptor, setVendorID: 0x5353u32];
            let _: () = msg_send![&descriptor, setProductID: 0x0001u32];
            let _: () = msg_send![&descriptor, setSerialNum: serial];

            let handle: Option<Retained<AnyObject>> = msg_send![msg_send![class("CGVirtualDisplay")?, alloc], initWithDescriptor: &*descriptor];
            let handle = handle?;

            if !apply_mode(&handle, width, height) {
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
            // blurry. Ask for this size at 2x.
            settle_mode(id, width, height);
            Some(Self {
                id,
                handle: Some(handle),
                size: (width, height),
                serial: None,
            })
        }
    }
}

/// The largest size create_display gives, in points.
const LARGEST: (u32, u32) = (2560, 1600);

/// The serial queue virtual displays call back on.
fn callback_queue() -> &'static AnyObject {
    static QUEUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let queue = *QUEUE.get_or_init(|| {
        let queue = dispatch2::DispatchQueue::new("dev.sidescreen.virtual-displays", None);
        // Kept for the app's life.
        dispatch2::DispatchRetained::into_raw(queue).as_ptr() as usize
    });
    unsafe { &*(queue as *const AnyObject) }
}

/// Give the display one mode, `width` × `height` points at the build's
/// refresh rate and scale.
unsafe fn apply_mode(handle: &AnyObject, width: u32, height: u32) -> bool {
    let (hz, scale) = mode_settings();
    let Some((mode_class, settings_class)) =
        class("CGVirtualDisplayMode").zip(class("CGVirtualDisplaySettings"))
    else {
        return false;
    };
    unsafe {
        let mode: Retained<AnyObject> = msg_send![
            msg_send![mode_class, alloc],
            initWithWidth: width, height: height, refreshRate: hz
        ];
        let settings: Retained<AnyObject> = msg_send![settings_class, new];
        let _: () = msg_send![&settings, setHiDPI: u32::from(scale == 2)];
        let modes = NSArray::from_retained_slice(&[mode]);
        let _: () = msg_send![&settings, setModes: &*modes];
        let applied: Bool = msg_send![handle, applySettings: &*settings];
        applied.as_bool()
    }
}

/// Wait for the display to run at its size at 2x, asking for it if macOS
/// picked another mode. A display WindowServer has only just added may
/// ignore the request, so try for a moment.
fn settle_mode(id: u32, width: u32, height: u32) {
    let (_, scale) = mode_settings();
    for _ in 0..25 {
        if scale == 1 || use_hidpi_mode(id, width, height) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Display configuration runs on a thread of its own. Each change waits
/// on WindowServer for 100 ms or more, which on the main thread stalls the
/// menu bar, Peek and the Viewer. One at a time, since each picks a free
/// spot from where the others are.
pub async fn configure<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> T {
    type Job = Box<dyn FnOnce() + Send>;
    static WORKER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> =
        std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("sidescreen-displays".into())
            .spawn(move || {
                for job in receive {
                    job();
                }
            })
            .expect("display thread");
        std::sync::Mutex::new(send)
    });
    let (done, result) = futures::channel::oneshot::channel();
    let job: Job = Box::new(move || {
        let _ = done.send(job());
    });
    let _ = worker.lock().unwrap_or_else(|e| e.into_inner()).send(job);
    result.await.expect("display thread ended")
}

/// A new display, parked from here as soon as macOS has made it, with the
/// displays macOS moved for it put back. `others` are the app's other
/// virtual displays. The main thread isn't involved: each new display
/// stalls it for a while, and parking after that would wait it out.
pub async fn create_parked(
    name: String,
    width: u32,
    height: u32,
    serial: Serial,
    others: Vec<u32>,
) -> Option<VirtualDisplay> {
    let before = origins();
    let made = VirtualDisplay::create(&name, width, height, serial)?;
    let id = made.id;
    let mut ids = others;
    ids.push(id);
    configure(move || {
        crate::core::park_fillers(&ids);
        tuck_away_restoring(id, &ids, &before)
    })
    .await;
    Some(made)
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
    tuck_away_restoring(id, virtual_ids, &[])
}

/// Where every display is now, to put back with `tuck_away_restoring`.
pub fn origins() -> Vec<(u32, (f64, f64))> {
    cg::active_displays()
        .into_iter()
        .map(|d| {
            let b = cg::display_bounds(d);
            (d, (b.x, b.y))
        })
        .collect()
}

/// `tuck_away` for a display just made, putting the other displays back
/// where `before` had them in the same change. macOS adds a display beside
/// the main one and moves a display already there out of its way, then
/// remembers that arrangement for this set of displays: without this, the
/// user's monitor would stay moved, and move again each time.
pub fn tuck_away_restoring(id: u32, virtual_ids: &[u32], before: &[(u32, (f64, f64))]) -> bool {
    let online = cg::active_displays();
    let restore: Vec<(u32, (f64, f64))> = before
        .iter()
        .copied()
        .filter(|&(d, (x, y))| {
            let now = cg::display_bounds(d);
            d != id && online.contains(&d) && (now.x, now.y) != (x, y)
        })
        .collect();
    let bounds_of = |d: u32| {
        let now = cg::display_bounds(d);
        match restore.iter().find(|(r, _)| *r == d) {
            Some(&(_, (x, y))) => crate::model::Rect::new(x, y, now.width, now.height),
            None => now,
        }
    };
    let others: Vec<u32> = online.into_iter().filter(|&d| d != id).collect();
    let anchor = others
        .iter()
        .copied()
        .filter(|&d| !virtual_ids.contains(&d) && !cg::is_sidescreen(d))
        .map(bounds_of)
        .reduce(|a, b| a.union(&b))
        .unwrap_or_else(|| cg::display_bounds(cg::main_display()));
    let occupied = others
        .iter()
        .filter(|&&d| virtual_ids.contains(&d) || cg::is_sidescreen(d))
        .map(|&d| bounds_of(d))
        .fold(anchor, |a, b| a.union(&b));
    let height = cg::display_bounds(id).height;
    let origin = (occupied.max_x() as i32, (anchor.y - height) as i32);
    if !restore.is_empty() {
        log::info!("display {id}: putting back displays macOS moved for it: {restore:?}");
    }
    unsafe {
        let mut config = std::ptr::null_mut();
        if CGBeginDisplayConfiguration(&mut config) == 0 {
            CGConfigureDisplayOrigin(config, id, origin.0, origin.1);
            for &(d, (x, y)) in &restore {
                CGConfigureDisplayOrigin(config, d, x as i32, y as i32);
            }
            // For this session only. Made permanent, macOS took a second
            // longer to finish, 2.3 s against 1.3 s, and still showed the
            // next display with the serial at its default spot first.
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
