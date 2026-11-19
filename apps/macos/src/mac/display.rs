//! A CGVirtualDisplay, created through the private CoreGraphics classes
//! (the same interface DeskPad and BetterDisplay use).

use super::cg;
use agent_screen_model::arrangement;
use agent_screen_model::connect_policy::PolicyHold;
use agent_screen_model::landing::{
    self, BadUnits, CreationGuard, Landing, Now, OnLanding, Outcome, RecentMirrors, StoredSets,
    UserBefore,
};
use agent_screen_model::model::SerialSizes;
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
    fn CGDisplayModeGetPixelHeight(mode: *const c_void) -> usize;
    fn CGDisplayModeGetIOFlags(mode: *const c_void) -> u32;
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
    fn CGConfigureDisplayMirrorOfDisplay(config: *mut c_void, display: u32, master: u32) -> i32;
}

/// `CGCompleteDisplayConfiguration` options.
const FOR_SESSION: u32 = 1;
const PERMANENTLY: u32 = 2;

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
        super::claims::remove_display(self.id);
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

/// A serial held for one display (`lowest_free_serial` and `SerialSizes`
/// say why), and the first of its instance's range.
pub struct Serial(u32, u32);

/// `~/Library/Application Support/Agent Screen/serials.json`, or
/// `serials-<instance>.json`: the size each of the instance's serials was
/// last leased for (`SerialSizes`).
fn serial_sizes_path() -> std::path::PathBuf {
    bad_units_path().with_file_name(match agent_screen_mcp::endpoint::instance() {
        Some(instance) => format!("serials-{instance}.json"),
        None => "serials.json".into(),
    })
}

impl Serial {
    /// The serial from `first` for a display of `size`, in points, that no
    /// display of this app holds: the one last leased at that size where
    /// it can (`SerialSizes::serial_for`).
    pub fn lease(first: u32, size: (u32, u32)) -> Serial {
        let mut taken = SERIALS.lock().unwrap_or_else(|e| e.into_inner());
        let serial = with_shared(
            &serial_sizes_path(),
            SerialSizes::from_json,
            SerialSizes::to_json,
            |mut sizes| {
                let serial = sizes.serial_for(first, &taken, size);
                sizes.leased(serial, size, wall());
                (serial, Some(sizes))
            },
        );
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

/// Displays that got a taken unit number, kept online while Agent Screen has
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

/// Whether `id` shares its unit number with a display of Agent Screen's,
/// any instance's. Capture refuses both of a pair, so a filler like that
/// would leave a display in use with no picture for as long as it stays.
fn twins_ours(id: u32) -> bool {
    cg::unit_twin(id).is_some_and(cg::is_agent_screen)
}

/// Release kept twins that share a unit number with a display of Agent
/// Screen's: the displays changed, and a filler that filled a gap may now
/// pair with one in use.
pub fn release_fillers_twinning_ours() {
    let mut fillers = FILLERS.lock().unwrap_or_else(|e| e.into_inner());
    let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut *fillers)
        .into_iter()
        .partition(|f| twins_ours(f.id));
    *fillers = kept;
    drop(fillers);
    for filler in gone {
        log::warn!(
            "released filler {}: it shared a unit number with an Agent Display",
            filler.id
        );
        crate::lifecycle::note("released", filler.id, "filler twinning an agent display");
    }
}

/// Release the kept twins, once Agent Screen has no other display.
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
    ///
    /// A display that lands on the user's screen (`landing::landing`, from
    /// the displays `before` it among `others`, Agent Screen's) has its
    /// mirror turned off and its arrangement stored (`unmirror`): macOS
    /// mirrored it with the user's display, which runs at its size
    /// meanwhile (#220). Where that isn't allowed or doesn't hold, it goes
    /// at once, its twins with it, and no other is made.
    pub fn create(
        name: &str,
        width: u32,
        height: u32,
        serial: Serial,
        others: &[u32],
        before: &[arrangement::Screen],
    ) -> Result<Self, NotMade> {
        // No mirror to flash the user's screen: WindowServer extends onto
        // a new display with no stored arrangement until this returns.
        let _extend = extend_new_displays();
        let first = serial.1;
        let mut serial = Some(serial);
        let mut twinned: Vec<Self> = Vec::new();
        for _ in 0..3u32 {
            // The next display, a unit-twin retry's included, would get a
            // unit number macOS put on the user's screen before.
            if let Some(why) = creation_refusal() {
                for twinned in twinned {
                    crate::lifecycle::note("released", twinned.id, "unit twin");
                }
                return Err(NotMade::Refused(why));
            }
            let lease = serial
                .take()
                .unwrap_or_else(|| Serial::lease(first, (width, height)));
            let number = lease.get();
            let mut display =
                Self::create_once(name, width, height, number).ok_or(NotMade::MacRefused)?;
            display.serial = Some(lease);
            crate::lifecycle::note("created", display.id, &format!("serial={number}"));
            twinned.push(display);
            // Every display of this call: macOS may mirror an earlier one
            // once a later one comes, as it did display 65 (#220).
            if let Some(unit) = release_if_on_user(&mut twinned, others, before) {
                return Err(NotMade::OnUser { unit });
            }
            let display = twinned.pop().expect("just pushed");
            // macOS keeps the resolution last used for a display's serial,
            // and serials repeat across launches: a display that once ran
            // at another size comes back at it, maybe at 1x, and shows
            // blurry. Ask for this size at 2x, once it is clear of the
            // user's displays: in a mirror set the user's display would
            // take the size too.
            settle_mode(display.id, width, height);
            let Some(twin) = cg::unit_twin(display.id) else {
                twinned.push(display);
                if let Some(unit) = release_if_on_user(&mut twinned, others, before) {
                    return Err(NotMade::OnUser { unit });
                }
                let display = twinned.pop().expect("just pushed");
                let mut fillers = FILLERS.lock().unwrap_or_else(|e| e.into_inner());
                for twinned in twinned {
                    if fillers.len() < MAX_FILLERS && !twins_ours(twinned.id) {
                        crate::lifecycle::note("kept", twinned.id, "filler");
                        fillers.push(twinned);
                    } else {
                        crate::lifecycle::note("released", twinned.id, "unit twin");
                    }
                }
                return Ok(display);
            };
            log::warn!(
                "display {} got the unit number of display {twin}; making another",
                display.id
            );
            twinned.push(display);
        }
        // Still twinned: capture refuses it, rather than show the other.
        twinned.pop().ok_or(NotMade::MacRefused)
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
            // The display's own callbacks run on a queue of Agent Screen's,
            // not the main one, which draws the menu bar icon and Viewer.
            let _: () = msg_send![&descriptor, setQueue: callback_queue()];
            let _: () = msg_send![&descriptor, setName: &*NSString::from_str(name)];
            // Room for the largest size an agent may ask for, so a spare
            // can take any size later: a mode change, not a new display.
            let largest = if knob("AGENTSCREEN_MAX_PIXELS").as_deref() == Some("fit") {
                (width, height)
            } else {
                LARGEST
            };
            let _: () = msg_send![&descriptor, setMaxPixelsWide: largest.0.max(width) * scale];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: largest.1.max(height) * scale];
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
            super::claims::add_display(id);
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
        let queue = dispatch2::DispatchQueue::new("app.agentscreen.virtual-displays", None);
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
        if scale == 1 || use_mode(id, width, height, 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Switch display `id` back to `width` × `height`, after macOS picked
/// another of its modes. True once it runs at that size.
pub fn restore_mode(id: u32, width: u32, height: u32) -> bool {
    let (_, scale) = mode_settings();
    for _ in 0..25 {
        if use_mode(id, width, height, scale) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let now = cg::display_bounds(id);
    (now.width as u32, now.height as u32) == (width, height)
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
            .name("agent-screen-displays".into())
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

/// Why no display came of a creation.
#[derive(Clone, PartialEq, Debug)]
pub enum NotMade {
    /// WindowServer refused the display: nothing changed on screen.
    MacRefused,
    /// It got unit number `unit`, landed on the user's screen and went at
    /// once.
    OnUser { unit: u32 },
    /// Parking never verified a safe position, and it went.
    Unparked,
    /// None was made, for the reason given to the agent
    /// (`creation_refusal`).
    Refused(String),
}

/// SkyLight's calls for WindowServer's default with a new display
/// (`connect_policy`), looked up once. SkyLight has no public header:
/// `SLSSetTVConnectDefaultPolicy` writes the `TVConnectPolicy` preference
/// and sends it to WindowServer, which reads it in
/// `apply_default_configuration` when a set of displays has no stored
/// arrangement.
struct PolicyCalls {
    main_connection: unsafe extern "C" fn() -> i32,
    get: unsafe extern "C" fn(i32, *mut i32) -> i32,
    set: unsafe extern "C" fn(i32, i32) -> i32,
}

fn policy_calls() -> Option<&'static PolicyCalls> {
    static CALLS: std::sync::OnceLock<Option<PolicyCalls>> = std::sync::OnceLock::new();
    CALLS
        .get_or_init(|| unsafe {
            let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
            let lib = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY);
            if lib.is_null() {
                return None;
            }
            let find = |name: &std::ffi::CStr| {
                let f = libc::dlsym(lib, name.as_ptr());
                (!f.is_null()).then_some(f)
            };
            Some(PolicyCalls {
                main_connection: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> i32>(
                    find(c"SLSMainConnectionID")?,
                ),
                get: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(i32, *mut i32) -> i32>(
                    find(c"SLSGetTVConnectDefaultPolicy")?,
                ),
                set: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(i32, i32) -> i32>(
                    find(c"SLSSetTVConnectDefaultPolicy")?,
                ),
            })
        })
        .as_ref()
}

/// The policy now. None when SkyLight doesn't say.
fn read_policy() -> Option<i32> {
    let calls = policy_calls()?;
    let mut policy = -1;
    let error = unsafe { (calls.get)((calls.main_connection)(), &mut policy) };
    (error == 0 && policy >= 0).then_some(policy)
}

/// Set the policy; true when SkyLight took it and it reads back.
fn write_policy(policy: i32) -> bool {
    let Some(calls) = policy_calls() else {
        return false;
    };
    let error = unsafe { (calls.set)((calls.main_connection)(), policy) };
    error == 0 && read_policy() == Some(policy)
}

/// `~/Library/Application Support/Agent Screen/connect-policy.json`, one
/// for every instance: they share WindowServer's policy.
fn connect_policy_path() -> std::path::PathBuf {
    stored_sets_path().with_file_name("connect-policy.json")
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    unsafe { libc::kill(pid, 0) == 0 || *libc::__error() == libc::EPERM }
}

/// Run `job` on the shared `PolicyHold` with the policy now. Nothing when
/// SkyLight can't read the policy.
fn with_policy_hold<T: Default>(job: impl FnOnce(&mut PolicyHold, i32) -> T) -> T {
    with_shared(
        &connect_policy_path(),
        PolicyHold::from_json,
        PolicyHold::to_json,
        |mut hold| match read_policy() {
            Some(current) => {
                let out = job(&mut hold, current);
                (out, Some(hold))
            }
            None => (T::default(), None),
        },
    )
}

/// Sets the policy back when dropped (`extend_new_displays`).
pub struct Extending(bool);

/// Have WindowServer extend the desktop onto a new display, not mirror the
/// user's displays onto it, until the returned value drops (#220). The
/// user's policy comes back then, unless they changed it meanwhile.
/// `AGENTSCREEN_KEEP_CONNECT_POLICY` leaves it, for a test of the unmirror.
pub fn extend_new_displays() -> Extending {
    if std::env::var_os("AGENTSCREEN_KEEP_CONNECT_POLICY").is_some() {
        return Extending(false);
    }
    let me = std::process::id();
    let held = with_policy_hold(|hold, current| match hold.begin(current, me, alive) {
        Some(policy) if write_policy(policy) => {
            crate::lifecycle::line(&format!(
                "new displays extend: TV connect policy {policy}, was {current}"
            ));
            true
        }
        Some(policy) => {
            log::warn!("couldn't set the TV connect policy to {policy}");
            // Nothing to restore: end() finds the value unchanged.
            hold.end(current, Some(me), alive);
            false
        }
        None => !hold.is_empty(),
    });
    Extending(held)
}

impl Drop for Extending {
    fn drop(&mut self) {
        if self.0 {
            end_policy_hold(Some(std::process::id()));
        }
    }
}

/// Set back the policy a process that died while holding it left
/// (`connect_policy`). At start-up.
pub fn restore_connect_policy() {
    if connect_policy_path().exists() {
        end_policy_hold(None);
    }
}

fn end_policy_hold(me: Option<u32>) {
    with_policy_hold(|hold, current| {
        if let Some(policy) = hold.end(current, me, alive) {
            if write_policy(policy) {
                crate::lifecycle::line(&format!("TV connect policy back to {policy}"));
            } else {
                log::warn!("couldn't set the TV connect policy back to {policy}");
            }
        }
    });
}

/// How this process's last new displays ended. Mirrors this instance
/// recorded before a restart come from its file at each decision.
static GUARD: std::sync::Mutex<CreationGuard> = std::sync::Mutex::new(CreationGuard::new());

/// `~/Library/Application Support/Agent Screen/bad-units.json`, or
/// `bad-units-<instance>.json`: one per instance, so a test instance's
/// refusal doesn't refuse the user's agents. Another instance's landing is
/// unmirrored before it could refuse anything (`unmirror`).
fn bad_units_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let name = match agent_screen_mcp::endpoint::instance() {
        Some(instance) => format!("bad-units-{instance}.json"),
        None => "bad-units.json".into(),
    };
    std::path::Path::new(&home)
        .join("Library/Application Support/Agent Screen")
        .join(name)
}

/// Run `job` on the instance's file's record, holding an exclusive lock
/// on its `.lock` sibling. A record
/// `job` returns replaces the file: written beside it, then renamed in.
fn with_bad_units<T>(job: impl FnOnce(BadUnits) -> (T, Option<BadUnits>)) -> T {
    with_shared(
        &bad_units_path(),
        BadUnits::from_json,
        BadUnits::to_json,
        job,
    )
}

/// `~/Library/Application Support/Agent Screen/arrangements.json`, one for
/// every instance: the arrangements it records are WindowServer's.
fn stored_sets_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join("Library/Application Support/Agent Screen/arrangements.json")
}

/// `~/Library/Application Support/Agent Screen/mirrors.json`, or
/// `mirrors-<instance>.json`: when this instance's displays landed on the
/// user's screen lately (`RecentMirrors`).
fn recent_mirrors_path() -> std::path::PathBuf {
    bad_units_path().with_file_name(match agent_screen_mcp::endpoint::instance() {
        Some(instance) => format!("mirrors-{instance}.json"),
        None => "mirrors.json".into(),
    })
}

/// `with_bad_units` for the sets Agent Screen stored an arrangement for.
fn with_stored_sets<T>(job: impl FnOnce(StoredSets) -> (T, Option<StoredSets>)) -> T {
    with_shared(
        &stored_sets_path(),
        StoredSets::from_json,
        StoredSets::to_json,
        job,
    )
}

/// Run `job` on the record in the JSON file at `path`, holding an
/// exclusive lock on the file's `.lock` sibling. A record `job` returns
/// replaces the file: written beside it, then renamed in.
fn with_shared<R: Default, T>(
    path: &std::path::Path,
    parse: fn(&serde_json::Value) -> Option<R>,
    to_json: fn(&R) -> serde_json::Value,
    job: impl FnOnce(R) -> (T, Option<R>),
) -> T {
    use std::os::fd::AsRawFd;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"));
    let locked = lock
        .as_ref()
        .is_ok_and(|f| unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0);
    if !locked {
        log::warn!("couldn't lock {}", path.with_extension("lock").display());
    }
    let record = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .and_then(|v| parse(&v))
        .unwrap_or_default();
    let (out, write) = job(record);
    if let Some(record) = write {
        let temp = path.with_extension(format!("json.{}", std::process::id()));
        let written = std::fs::write(&temp, to_json(&record).to_string())
            .and_then(|()| std::fs::rename(&temp, path));
        if let Err(e) = written {
            log::warn!("couldn't write {}: {e}", path.display());
        }
    }
    // Dropping the file releases the lock.
    drop(lock);
    out
}

/// The login session's unique id, which a logout or restart changes along
/// with WindowServer. Empty when CoreGraphics doesn't say: then no record
/// matches.
fn session() -> &'static str {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION.get_or_init(cg::session_uuid)
}

/// Seconds since this process first asked.
fn secs() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// The Mac as a decision to make a display reads it now.
fn now() -> Now<'static> {
    let units: Vec<u32> = cg::online().into_iter().map(cg::unit_number).collect();
    Now {
        next_unit: landing::next_unit(&units),
        online: units.len(),
        session: session(),
        secs: secs(),
        wall: wall(),
    }
}

/// Seconds since 1970.
fn wall() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether new displays are paused because one landed on the user's
/// screen, for the menu and the "+" button to say so.
pub fn paused_by_mirror() -> bool {
    let now = now();
    with_bad_units(|file| {
        let guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        (guard.paused_by_mirror(&file, &now), None)
    })
}

/// The user's Allow New Displays: forget where displays landed on their
/// screen, in this instance.
pub fn allow_new_displays() {
    GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear_bad_units();
    with_bad_units(|_| ((), Some(BadUnits::new())));
    with_shared(
        &recent_mirrors_path(),
        RecentMirrors::from_json,
        RecentMirrors::to_json,
        |_| ((), Some(RecentMirrors::new())),
    );
    crate::lifecycle::line("new displays allowed again by the user");
}

/// A record that a display landed on the user's screen now, as a test
/// would see one, in this instance only (`debugShow displays-paused`).
#[cfg(debug_assertions)]
pub fn debug_pause_new_displays() {
    let now = now();
    GUARD.lock().unwrap_or_else(|e| e.into_inner()).record(
        Outcome::Mirrored {
            unit: now.next_unit,
            online: now.online,
            at: now.wall,
        },
        session(),
    );
}

/// Why Agent Screen makes no new display now, for the agent: the last new
/// display of this instance landed on the user's screen with these
/// displays online or this unit next, or parking failed twice. None while
/// it may.
pub fn creation_refusal() -> Option<String> {
    let now = now();
    with_bad_units(|file| {
        let guard = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        (guard.refusal(&file, &now), None)
    })
}

fn record(outcome: Outcome) {
    GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .record(outcome, session());
    if let Outcome::Mirrored { unit, online, at } = outcome {
        crate::lifecycle::line(&format!(
            "landed on the user's screen: unit {unit} with {online} displays online; no display is made while either repeats, for 30 min"
        ));
        // A test's injected landing stays in this instance.
        if !landing_refused() {
            with_bad_units(|mut file| {
                file.add(session(), unit, online, at);
                ((), Some(file))
            });
        }
    }
}

/// What to tell the agent about a creation that made no display; None for
/// the plain failure message.
fn refusal_after(not_made: &NotMade) -> Option<String> {
    match not_made {
        NotMade::Refused(why) => Some(why.clone()),
        NotMade::OnUser { .. } | NotMade::Unparked | NotMade::MacRefused => creation_refusal(),
    }
}

/// Once one display in `made` has landed on the user's screen, turn the
/// mirror off and keep them all (`unmirror`). Where that isn't allowed or
/// doesn't hold, release every display in `made`: that one's unit number.
fn release_if_on_user(
    made: &mut Vec<VirtualDisplay>,
    others: &[u32],
    before: &[arrangement::Screen],
) -> Option<u32> {
    let ids: Vec<u32> = made.iter().map(|d| d.id).collect();
    let (at, landed) = made.iter().enumerate().find_map(|(i, d)| {
        let mut also = others.to_vec();
        also.extend(ids.iter().copied().filter(|&o| o != d.id));
        let landed = landed(d.id, &also, before);
        landed.takes_user_display().then_some((i, landed))
    })?;
    if unmirror(&ids, others, before) {
        return None;
    }
    let on_user = made.remove(at);
    for other in made.drain(..) {
        crate::lifecycle::note("released", other.id, "unit twin");
    }
    Some(release_on_user(on_user, landed, before))
}

/// The displays online as WindowServer keys a stored arrangement
/// (`landing::arrangement_key`), with each display in `before` at the size
/// it had then: a user display mirroring a new one runs at the new one's
/// size meanwhile. None when a display has no identity.
fn arrangement_key(before: &[arrangement::Screen]) -> Option<String> {
    let now = cg::online()
        .into_iter()
        .map(|d| {
            let b = cg::display_bounds(d);
            let uuid = super::colorsync::uuid_of(d)?;
            Some((d, uuid, (b.width as u32, b.height as u32)))
        })
        .collect::<Option<Vec<_>>>()?;
    let sizes: Vec<(u32, (u32, u32))> = before
        .iter()
        .map(|s| (s.id, (s.rect.width as u32, s.rect.height as u32)))
        .collect();
    Some(landing::arrangement_key_before(&now, &sizes))
}

/// A display mode CoreGraphics gave, released with this.
struct Mode(*const c_void);

// Only the display worker touches it; CoreGraphics modes are immutable.
unsafe impl Send for Mode {}

impl Drop for Mode {
    fn drop(&mut self) {
        unsafe { CGDisplayModeRelease(self.0) };
    }
}

/// The user's displays before the display worker's current creation or
/// reuse: origin, the display each mirrored, and mode
/// (`snapshot_users`). `unmirror` puts them back as they were.
static USERS_BEFORE: std::sync::Mutex<Vec<(UserBefore, Option<Mode>)>> =
    std::sync::Mutex::new(Vec::new());

/// Take `USERS_BEFORE` from the user displays among `before`.
fn snapshot_users(before: &[arrangement::Screen]) {
    let users = before
        .iter()
        .filter(|s| s.role == arrangement::Role::User)
        .map(|s| {
            let mode = unsafe { CGDisplayCopyDisplayMode(s.id) };
            let user = UserBefore {
                id: s.id,
                origin: (s.rect.x, s.rect.y),
                mirrors: cg::mirrors(s.id),
            };
            (user, (!mode.is_null()).then_some(Mode(mode)))
        })
        .collect();
    *USERS_BEFORE.lock().unwrap_or_else(|e| e.into_inner()) = users;
}

/// New displays `new` of which one landed on the user's screen, with
/// Agent Screen's displays `others` and the displays `before` them: take
/// the new displays out of their mirror set, keep every mirror the user
/// chose, give the user's displays back their origins and modes
/// (`USERS_BEFORE`, `landing::unmirror_plan`) and park `new`, in one change
/// that WindowServer stores for this set of displays
/// (`landing::OnLanding::Unmirror`). True once every display in `new` is
/// clear of the user's screen. False for a set Agent Screen stored before
/// and macOS mirrored anyway, a change WindowServer refused, or a mirror
/// still on after 8 s: the caller releases.
fn unmirror(new: &[u32], others: &[u32], before: &[arrangement::Screen]) -> bool {
    // A test's injected landing has no mirror to turn off: it takes the
    // release path.
    if landing_refused() {
        return false;
    }
    let Some(key) = arrangement_key(before) else {
        log::warn!("displays {new:?}: a display has no identity; releasing");
        return false;
    };
    let now = wall();
    let set = with_stored_sets(|stored| match stored.on_landing(&key, now) {
        OnLanding::Release => {
            let mut stored = stored;
            stored.forget(&key);
            (OnLanding::Release, Some(stored))
        }
        OnLanding::Unmirror => (OnLanding::Unmirror, None),
    });
    if set == OnLanding::Release {
        crate::lifecycle::line(&format!(
            "displays {new:?} landed on the user's screen although Agent Screen stored this arrangement less than 30 min ago, or unmirrored this set twice in 30 min; releasing"
        ));
    }
    // Every landing counts, a released one too: each flashed the screen.
    let decision = with_shared(
        &recent_mirrors_path(),
        RecentMirrors::from_json,
        RecentMirrors::to_json,
        |mut mirrors| {
            let decision = mirrors.on_landing(set, now);
            mirrors.landed(now);
            (decision, Some(mirrors))
        },
    );
    if decision == OnLanding::Release {
        if set == OnLanding::Unmirror {
            crate::lifecycle::line(&format!(
                "displays {new:?} landed on the user's screen, the third of this instance's in 30 min; releasing"
            ));
        }
        return false;
    }
    let started = std::time::Instant::now();
    let online = cg::online();
    // The user's displays where they were, Agent Screen's others too; the
    // new ones wherever macOS put them, for the plan to park.
    let mut screens: Vec<arrangement::Screen> = before
        .iter()
        .filter(|s| online.contains(&s.id))
        .cloned()
        .collect();
    for &d in new {
        screens.retain(|s| s.id != d);
        screens.push(arrangement::Screen {
            id: d,
            rect: cg::display_bounds(d),
            role: arrangement::Role::Ours {
                order: u64::from(cg::serial(d)),
            },
        });
    }
    let parked = arrangement::plan(&screens);
    let mirrors_now: Vec<(u32, u32)> = online
        .iter()
        .filter_map(|&d| cg::mirrors(d).map(|m| (d, m)))
        .collect();
    let users = USERS_BEFORE.lock().unwrap_or_else(|e| e.into_inner());
    let plan = landing::unmirror_plan(
        &users.iter().map(|(u, _)| *u).collect::<Vec<_>>(),
        &mirrors_now,
        new,
    );
    log::info!("displays {new:?}: unmirroring, mirrors {mirrors_now:?}, plan {plan:?}");
    // One change: the new displays out of their mirror set, the user's own
    // mirrors kept, the user's displays back at their origins and modes,
    // the new ones parked, stored for this set of displays. The next time
    // the set comes, at these sizes, WindowServer applies it, not its
    // default, which mirrors (#220). WindowServer had already stored the
    // mirror for the set when it added the display; a change for the
    // session only would leave that. Split in two, the user's screen came
    // back no sooner (1.4 s for the session part alone).
    let completed = unsafe {
        let mut config = std::ptr::null_mut();
        if CGBeginDisplayConfiguration(&mut config) != 0 {
            return false;
        }
        for &d in &plan.unmirror {
            CGConfigureDisplayMirrorOfDisplay(config, d, 0);
        }
        for &(d, master) in &plan.remirror {
            CGConfigureDisplayMirrorOfDisplay(config, d, master);
        }
        for &d in &plan.modes {
            if let Some((_, Some(mode))) = users.iter().find(|(u, _)| u.id == d) {
                CGConfigureDisplayWithDisplayMode(config, d, mode.0, std::ptr::null());
            }
        }
        for &(d, (x, y)) in plan.origins.iter().chain(&parked) {
            CGConfigureDisplayOrigin(config, d, x as i32, y as i32);
        }
        CGCompleteDisplayConfiguration(config, PERMANENTLY)
    };
    drop(users);
    // A refused change leaves the mirror on: release at once, as 0.1.39.
    if completed != 0 {
        log::warn!("displays {new:?}: unmirroring refused ({completed}); releasing");
        return false;
    }
    let stored_ms = started.elapsed().as_millis();
    let clear = || {
        new.iter().all(|&d| {
            let mut also = others.to_vec();
            also.extend(new.iter().copied().filter(|&o| o != d));
            !landed(d, &also, before).takes_user_display()
        })
    };
    // The app reads the change late while WindowServer reconfigures: the
    // user's screen was back 0.8 s after creation on 2026-11-18, and the
    // app saw it 4 s after.
    while !clear() && started.elapsed().as_secs() < 8 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ms = started.elapsed().as_millis();
    if !clear() {
        log::warn!("displays {new:?}: still on the user's screen {ms} ms after unmirroring");
        return false;
    }
    let stored = arrangement_key(&[]).unwrap_or(key);
    with_stored_sets(|mut sets| {
        sets.stored(&stored, wall());
        ((), Some(sets))
    });
    for &d in new {
        crate::lifecycle::note(
            "unmirrored",
            d,
            &format!("mirror off and arrangement stored in {stored_ms} ms, seen clear in {ms} ms"),
        );
    }
    true
}

/// The online displays but the closet, as the arrangement sees them. A
/// display in a mirror set may be missing from the active list.
fn online_screens(virtual_ids: &[u32]) -> Vec<arrangement::Screen> {
    let online: Vec<u32> = cg::online()
        .into_iter()
        .filter(|&d| !cg::is_closet(d))
        .collect();
    screens(&online, virtual_ids, &[])
}

/// Where Agent Screen's display `id` landed among the displays `before`
/// it (`landing::landing`).
fn landed(id: u32, others: &[u32], before: &[arrangement::Screen]) -> Landing {
    let mut ids = others.to_vec();
    ids.push(id);
    let online: Vec<u32> = cg::online();
    let mirrored: Vec<u32> = online
        .iter()
        .copied()
        .filter(|&d| cg::in_mirror_set(d))
        .collect();
    let landed = landing::landing(id, before, &online_screens(&ids), &mirrored);
    if landed.takes_user_display() {
        return landed;
    }
    if landing_refused() {
        return Landing::OnUser {
            user: cg::main_display(),
        };
    }
    Landing::Clear
}

/// Release a display that landed on the user's screen, now, and note
/// whether the user's display got back the size it had `before`. macOS
/// restores it; Agent Screen doesn't touch the user's display mode. The
/// display's unit number.
fn release_on_user(
    display: VirtualDisplay,
    landed: Landing,
    before: &[arrangement::Screen],
) -> u32 {
    let id = display.id;
    let unit = cg::unit_number(id);
    log::warn!("display {id} landed on the user's screen ({landed:?}); releasing it at once");
    crate::lifecycle::note("released", id, &format!("on the user's screen: {landed:?}"));
    drop(display);
    // WindowServer removes it a moment later; until then the displays
    // online, which the next decision reads, still count it.
    let going = std::time::Instant::now();
    while cg::is_online(id) && going.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let user = match landed {
        Landing::OnUser { user } | Landing::UserResized { user, .. } => user,
        Landing::Clear => return unit,
    };
    let Some(was) = before.iter().find(|s| s.id == user).map(|s| s.rect) else {
        return unit;
    };
    let size = || {
        let now = cg::display_bounds(user);
        (now.width, now.height)
    };
    let started = std::time::Instant::now();
    while size() != (was.width, was.height) && started.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (w, h) = size();
    let back = (w, h) == (was.width, was.height);
    let state = if back { "back at" } else { "not back at" };
    // A smaller user display moved the displays right of it; put back
    // those still off where they were. Another instance's may be parking
    // meanwhile, and parks its own.
    if back {
        let moved: Vec<(u32, (f64, f64))> = before
            .iter()
            .filter(|s| s.id != id && s.role != arrangement::Role::Fixed && cg::is_online(s.id))
            .filter(|s| {
                let now = cg::display_bounds(s.id);
                (now.x, now.y) != (s.rect.x, s.rect.y)
            })
            .map(|s| (s.id, (s.rect.x, s.rect.y)))
            .collect();
        if !moved.is_empty() {
            log::info!("display {id}: putting back displays macOS moved for it: {moved:?}");
            put_back(&moved);
        }
    }
    crate::lifecycle::line(&format!(
        "user display {user} {state} {}x{} {} ms after display {id} went, now {w}x{h}",
        was.width,
        was.height,
        started.elapsed().as_millis()
    ));
    unit
}

/// Move each display to its origin, in one change for this session.
fn put_back(origins: &[(u32, (f64, f64))]) {
    unsafe {
        let mut config = std::ptr::null_mut();
        if CGBeginDisplayConfiguration(&mut config) == 0 {
            for &(d, (x, y)) in origins {
                CGConfigureDisplayOrigin(config, d, x as i32, y as i32);
            }
            CGCompleteDisplayConfiguration(config, FOR_SESSION);
        }
    }
}

/// Report a display on the user's screen without one there, in an
/// isolated debug instance, so a test can take the release path without
/// resizing the user's screen.
fn landing_refused() -> bool {
    knob("AGENTSCREEN_TEST_LANDED_ON_USER").is_some()
        && std::env::var("AGENTSCREEN_INSTANCE").as_deref() == Ok("e2e")
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
) -> Result<VirtualDisplay, Option<String>> {
    // Creation and parking share the display worker. Otherwise two new
    // displays can choose positions from an incomplete set of origins.
    configure(move || {
        if let Some(why) = creation_refusal() {
            log::warn!("not making {name}: {why}");
            return Err(Some(why));
        }
        let before = origins();
        let before_screens = online_screens(&others);
        snapshot_users(&before_screens);
        let online = cg::online().len();
        let previous_fillers = filler_ids();
        let made = VirtualDisplay::create(&name, width, height, serial, &others, &before_screens);
        let parked = made.and_then(|made| park_owned(made, others, &before, &before_screens));
        match &parked {
            Ok(_) => record(Outcome::Parked),
            Err(NotMade::OnUser { unit }) => record(Outcome::Mirrored {
                unit: *unit,
                online,
                at: wall(),
            }),
            Err(NotMade::Unparked) => record(Outcome::Failed {
                online,
                secs: secs(),
            }),
            Err(NotMade::MacRefused | NotMade::Refused(_)) => {}
        }
        if parked.is_err() {
            // A unit-twin retry can retain fillers before it returns its
            // usable target. Failed creation owns those handles too.
            let mut fillers = FILLERS.lock().unwrap_or_else(|e| e.into_inner());
            let (kept, released): (Vec<_>, Vec<_>) = std::mem::take(&mut *fillers)
                .into_iter()
                .partition(|d| previous_fillers.contains(&d.id));
            *fillers = kept;
            drop(fillers);
            for filler in released {
                crate::lifecycle::note("released", filler.id, "filler from failed creation");
            }
        }
        parked.map_err(|not_made| refusal_after(&not_made))
    })
    .await
}

/// Reuse a spare without publishing it while a larger mode overlaps the
/// user's screens. Its old origin is not necessarily safe at its new size.
pub async fn reuse_parked(
    mut handle: VirtualDisplay,
    width: u32,
    height: u32,
    resize: bool,
    others: Vec<u32>,
) -> Option<VirtualDisplay> {
    configure(move || {
        let before = origins();
        let before_screens = online_screens(&others);
        snapshot_users(&before_screens);
        if resize && !handle.resize(width, height) {
            crate::lifecycle::note("released", handle.id, "spare could not resize");
            return None;
        }
        let mut ids = others;
        ids.push(handle.id);
        // With the spare gone, one display fewer is online.
        let online = cg::online().len().saturating_sub(1);
        let landed = landed(handle.id, &others_of(&ids, handle.id), &before_screens);
        if landed.takes_user_display()
            && !unmirror(&[handle.id], &others_of(&ids, handle.id), &before_screens)
        {
            let unit = release_on_user(handle, landed, &before_screens);
            record(Outcome::Mirrored {
                unit,
                online,
                at: wall(),
            });
            return None;
        }
        if is_tucked(handle.id, &ids) {
            record(Outcome::Reused);
            return Some(handle);
        }
        let parked = park_owned(handle, ids, &before, &before_screens);
        match &parked {
            Ok(_) => record(Outcome::Reused),
            Err(NotMade::OnUser { unit }) => record(Outcome::Mirrored {
                unit: *unit,
                online,
                at: wall(),
            }),
            Err(_) => {}
        }
        parked.ok()
    })
    .await
}

/// `ids` without `id`.
fn others_of(ids: &[u32], id: u32) -> Vec<u32> {
    ids.iter().copied().filter(|&d| d != id).collect()
}

/// Runs only on the display worker, for a new handle or a resized spare.
/// macOS may put a display in a mirror set a moment after making it: one
/// that lands on the user's screen meanwhile is unmirrored once
/// (`unmirror`), and goes at once if that fails or the mirror comes back.
fn park_owned(
    made: VirtualDisplay,
    mut ids: Vec<u32>,
    before: &[(u32, (f64, f64))],
    before_screens: &[arrangement::Screen],
) -> Result<VirtualDisplay, NotMade> {
    let id = made.id;
    ids.push(id);
    // Include fillers retained during this creation, not just the ones
    // known when the UI queued the job.
    let fillers = filler_ids();
    ids.extend(fillers.iter().copied());
    ids.sort_unstable();
    ids.dedup();
    let mut unmirrored = false;
    for attempt in 0..75 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let landed = landed(id, &others_of(&ids, id), before_screens);
        if landed.takes_user_display() {
            // Once: a mirror that comes back goes to the refusal.
            if !unmirrored && unmirror(&[id], &others_of(&ids, id), before_screens) {
                unmirrored = true;
                continue;
            }
            let unit = release_on_user(made, landed, before_screens);
            return Err(NotMade::OnUser { unit });
        }
        crate::core::park_fillers(&ids);
        if tuck_away_restoring(id, &ids, before)
            && is_tucked(id, &ids)
            && fillers
                .iter()
                .all(|&f| !cg::is_online(f) || is_tucked(f, &ids))
        {
            crate::lifecycle::note("parked", id, &format!("tries={}", attempt + 1));
            return Ok(made);
        }
    }
    let why = parking_failure(id, &ids, &fillers, before);
    log::warn!("display {id}: macOS did not verify a safe position ({why}); releasing it");
    crate::lifecycle::note("released", id, &format!("parking failed: {why}"));
    Err(NotMade::Unparked)
}

/// Why the last parking check failed, for the display log: each display
/// not back where it was before, whether `id` and each filler is out of
/// the way, and where every display is.
fn parking_failure(id: u32, ids: &[u32], fillers: &[u32], before: &[(u32, (f64, f64))]) -> String {
    // The closet included, which macOS may move too.
    let online = cg::online();
    let rect = |d: u32| {
        let b = cg::display_bounds(d);
        format!("{},{} {}x{}", b.x, b.y, b.width, b.height)
    };
    let role = |d: u32| {
        if ids.contains(&d) {
            "ours"
        } else if cg::is_agent_screen(d) {
            "other instance"
        } else if cg::is_closet(d) {
            "closet"
        } else {
            "user"
        }
    };
    let moved: Vec<String> = before
        .iter()
        .filter(|&&(d, _)| d != id && online.contains(&d))
        .filter_map(|&(d, (x, y))| {
            let now = cg::display_bounds(d);
            ((now.x, now.y) != (x, y))
                .then(|| format!("{d} ({}) {x},{y} -> {},{}", role(d), now.x, now.y))
        })
        .collect();
    let untucked: Vec<String> = fillers
        .iter()
        .filter(|&&f| cg::is_online(f) && !is_tucked(f, ids))
        .map(|f| f.to_string())
        .collect();
    let all: Vec<String> = online
        .iter()
        .map(|&d| format!("{d} {} {}", role(d), rect(d)))
        .collect();
    format!(
        "tucked={} not restored=[{}] untucked fillers=[{}] online=[{}]",
        is_tucked(id, ids),
        moved.join("; "),
        untucked.join(", "),
        all.join("; ")
    )
}

/// Park Agent Screen's displays where `arrangement::plan` puts them: a
/// compact grid off the top-right corner of the rightmost user display,
/// out of the real cursor's way.
///
/// True once display `id` sits somewhere `arrangement::out_of_the_way`
/// accepts, the plan or wherever macOS attached it instead. A display
/// WindowServer has only just added ignores the configuration, so callers
/// try again.
pub fn tuck_away(id: u32, virtual_ids: &[u32]) -> bool {
    if knob("AGENTSCREEN_NO_PARK").is_some() {
        return true;
    }
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
    if knob("AGENTSCREEN_NO_PARK").is_some() {
        return true;
    }
    let online = cg::active_displays();
    let restore: Vec<(u32, (f64, f64))> = before
        .iter()
        .copied()
        .filter(|&(d, (x, y))| {
            let now = cg::display_bounds(d);
            d != id && online.contains(&d) && (now.x, now.y) != (x, y)
        })
        .collect();
    let screens = screens(&online, virtual_ids, &restore);
    let mut moves: Vec<(u32, (f64, f64))> = arrangement::plan(&screens)
        .into_iter()
        .filter(|&(d, (x, y))| {
            let now = cg::display_bounds(d);
            (now.x, now.y) != (x, y)
        })
        .collect();
    for &(d, pos) in &restore {
        if !moves.iter().any(|&(m, _)| m == d) {
            moves.push((d, pos));
        }
    }
    if !restore.is_empty() {
        log::info!("display {id}: putting back displays macOS moved for it: {restore:?}");
    }
    if !moves.is_empty() {
        unsafe {
            let mut config = std::ptr::null_mut();
            if CGBeginDisplayConfiguration(&mut config) == 0 {
                for &(d, (x, y)) in &moves {
                    CGConfigureDisplayOrigin(config, d, x as i32, y as i32);
                }
                // For this session only. Made permanent, macOS took a second
                // longer to finish, 2.3 s against 1.3 s, and still showed the
                // next display with the serial at its default spot first.
                CGCompleteDisplayConfiguration(config, FOR_SESSION);
            }
        }
    }
    let after = cg::active_displays();
    let restored = restore.iter().all(|&(d, (x, y))| {
        let now = cg::display_bounds(d);
        !after.contains(&d) || (now.x, now.y) == (x, y)
    });
    !parking_refused()
        && restored
        && arrangement::parked_safely(id, &self::screens(&after, virtual_ids, &[]))
}

/// Exercise an unverified parking result without leaving a test display
/// beside the user's screen. The real configuration and restoration still
/// run; only their verification is refused, in an isolated debug instance.
fn parking_refused() -> bool {
    knob("AGENTSCREEN_TEST_PARK_REFUSED").is_some()
        && std::env::var("AGENTSCREEN_INSTANCE").as_deref() == Ok("e2e")
}

/// The displays `online`, as the arrangement sees them, with the user's at
/// the origins in `restore` where it has one.
fn screens(
    online: &[u32],
    virtual_ids: &[u32],
    restore: &[(u32, (f64, f64))],
) -> Vec<arrangement::Screen> {
    online
        .iter()
        .map(|&d| {
            let mut rect = cg::display_bounds(d);
            if let Some(&(_, (x, y))) = restore.iter().find(|(r, _)| *r == d) {
                (rect.x, rect.y) = (x, y);
            }
            let role = if virtual_ids.contains(&d) {
                arrangement::Role::Ours {
                    order: u64::from(cg::serial(d)),
                }
            } else if cg::is_agent_screen(d) {
                arrangement::Role::Fixed
            } else {
                arrangement::Role::User
            };
            arrangement::Screen { id: d, rect, role }
        })
        .collect()
}

/// Whether display `id` is still out of the cursor's way, right of the
/// user's displays and touching them at most at a corner. The user may have
/// dragged it elsewhere in System Settings.
pub fn is_tucked(id: u32, virtual_ids: &[u32]) -> bool {
    if parking_refused() {
        return false;
    }
    // Only the existing debug-only cost comparison may skip parking.
    if knob("AGENTSCREEN_NO_PARK").is_some() {
        return true;
    }
    let online = cg::active_displays();
    if !online.contains(&id) || !virtual_ids.contains(&id) {
        return false;
    }
    arrangement::out_of_the_way(&cg::display_bounds(id), &screens(&online, virtual_ids, &[]))
}

/// Whether every online virtual display of ours in `virtual_ids` reaches a
/// user display through touching displays. When a corner display is released,
/// a sibling to its right may briefly float until `tuck_away` compacts it.
pub fn arrangement_connected(virtual_ids: &[u32]) -> bool {
    if knob("AGENTSCREEN_NO_PARK").is_some() {
        return true;
    }
    let online = cg::active_displays();
    arrangement::connected(&screens(&online, virtual_ids, &[]))
}

/// A setting a test build takes from the environment, to measure what a
/// choice costs (`cargo e2e changecost`). None in a release build.
fn knob(name: &str) -> Option<String> {
    cfg!(debug_assertions)
        .then(|| std::env::var(name).ok())
        .flatten()
}

/// Refresh rate and backing scale of new displays. Test builds read them
/// from AGENTSCREEN_DISPLAY_HZ and AGENTSCREEN_DISPLAY_SCALE, to measure
/// what each costs.
fn mode_settings() -> (f64, u32) {
    let read = |name: &str| {
        cfg!(debug_assertions)
            .then(|| std::env::var(name).ok())
            .flatten()
    };
    let hz = read("AGENTSCREEN_DISPLAY_HZ")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60.0);
    let scale = read("AGENTSCREEN_DISPLAY_SCALE")
        .and_then(|v| v.parse().ok())
        .filter(|s| *s == 1 || *s == 2)
        .unwrap_or(2);
    (hz, scale)
}

/// Switch display `id` to `width` × `height` points at `scale`. True once
/// it runs that way.
fn use_mode(id: u32, width: u32, height: u32, scale: u32) -> bool {
    let (width, height) = (width as usize, height as usize);
    let is_wanted = |mode: *const c_void| unsafe {
        CGDisplayModeGetWidth(mode) == width
            && CGDisplayModeGetHeight(mode) == height
            && CGDisplayModeGetPixelWidth(mode) == width * scale as usize
    };
    // A display in a mirror set shares its mode with the display it
    // mirrors: setting it would resize that one (#220).
    if cg::in_mirror_set(id) {
        return false;
    }
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
            "display {id}: {} modes, {width}×{height} at {scale}x among them: {}",
            CFArrayGetCount(modes),
            wanted.is_some()
        );
        if let Some(mode) = wanted {
            let mut config = std::ptr::null_mut();
            if CGBeginDisplayConfiguration(&mut config) == 0 {
                let set = CGConfigureDisplayWithDisplayMode(config, id, mode, std::ptr::null());
                // kCGConfigureForSession. Made permanent, macOS would store
                // the whole arrangement for this set of displays, a mirror
                // that formed meanwhile included, and apply it again after
                // logout and restart. Each new display asks for its mode
                // anyway (`settle_mode`).
                let done = CGCompleteDisplayConfiguration(config, FOR_SESSION);
                log::debug!("display {id}: mode set {set}, completed {done}");
            }
        }
        CFRelease(modes as *const c_void);
    }
    false
}

/// Display `id`'s panel size in its own pixels: the pixels of its native
/// mode, whatever mode it runs in. A scaled mode's backing store is larger
/// or smaller than the panel (4112 pixels wide for 2056 points on a
/// 3456-pixel panel), so the current mode doesn't say. None when no mode
/// is marked native.
pub fn native_pixels(id: u32) -> Option<(u32, u32)> {
    use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation::base::{CFRelease, TCFType};
    /// IOKit's kDisplayModeNativeFlag.
    const NATIVE: u32 = 0x0200_0000;
    unsafe {
        let options = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(
            core_foundation::string::CFString::wrap_under_get_rule(
                kCGDisplayShowDuplicateLowResolutionModes,
            ),
            core_foundation::boolean::CFBoolean::true_value(),
        )]);
        let modes = CGDisplayCopyAllDisplayModes(id, options.as_concrete_TypeRef() as *const c_void)
            as CFArrayRef;
        if modes.is_null() {
            return None;
        }
        let native = (0..CFArrayGetCount(modes))
            .map(|i| CFArrayGetValueAtIndex(modes, i))
            .filter(|&mode| CGDisplayModeGetIOFlags(mode) & NATIVE != 0)
            .map(|mode| {
                (
                    CGDisplayModeGetPixelWidth(mode) as u32,
                    CGDisplayModeGetPixelHeight(mode) as u32,
                )
            })
            .max();
        CFRelease(modes as *const c_void);
        native
    }
}

fn class(name: &str) -> Option<&'static AnyClass> {
    let name = std::ffi::CString::new(name).ok()?;
    AnyClass::get(&name)
}
