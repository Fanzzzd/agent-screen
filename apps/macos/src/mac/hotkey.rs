//! The Stop chord, ⌃⌥⌘. (ADR 0025): registered with `RegisterEventHotKey`
//! while the mode is Auto or YOLO, and unregistered in Ask so no other app
//! loses the chord. During an armed hold or a key hold that tap sees the
//! chord first (`armed_hold`, `key_hold`); each sends `Native::StopChord`.
//! Main thread.

use agent_screen_model::autonomy::{Mode, PERIOD_KEYCODE};
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

type Handler = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        count: u32,
        list: *const EventTypeSpec,
        user: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
}

/// 'keyb', kEventHotKeyPressed.
const CLASS_KEYBOARD: u32 = 0x6b65_7962;
const HOT_KEY_PRESSED: u32 = 5;
/// Carbon's modifier bits: cmdKey, optionKey, controlKey.
const CMD: u32 = 1 << 8;
const OPTION: u32 = 1 << 11;
const CONTROL: u32 = 1 << 12;
/// 'ASst'.
const SIGNATURE: u32 = 0x4153_7374;

thread_local! {
    static HOTKEY: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
    static HANDLER: Cell<bool> = const { Cell::new(false) };
}

static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Whether the chord is registered, for `debugAutonomy`.
pub fn registered() -> bool {
    REGISTERED.load(Ordering::Acquire)
}

unsafe extern "C" fn pressed(_next: *mut c_void, _event: *mut c_void, _user: *mut c_void) -> i32 {
    log::info!("autonomy: the Stop chord");
    super::native::send(super::native::Native::StopChord);
    0
}

/// Register the chord in Auto and YOLO, unregister it in Ask.
pub fn sync(mode: Mode) {
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let want = mode.acts();
    let current = HOTKEY.with(Cell::get);
    if want == !current.is_null() {
        return;
    }
    if !want {
        unsafe { UnregisterEventHotKey(current) };
        HOTKEY.with(|h| h.set(std::ptr::null_mut()));
        REGISTERED.store(false, Ordering::Release);
        log::info!("autonomy: the Stop chord is unregistered");
        return;
    }
    let target = unsafe { GetApplicationEventTarget() };
    if !HANDLER.with(Cell::get) {
        let spec = EventTypeSpec {
            class: CLASS_KEYBOARD,
            kind: HOT_KEY_PRESSED,
        };
        let status = unsafe {
            InstallEventHandler(
                target,
                pressed,
                1,
                &spec,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            log::warn!("autonomy: the Stop chord's handler failed: {status}");
            return;
        }
        HANDLER.with(|h| h.set(true));
    }
    let mut hotkey = std::ptr::null_mut();
    let status = unsafe {
        RegisterEventHotKey(
            u32::from(PERIOD_KEYCODE),
            CMD | OPTION | CONTROL,
            EventHotKeyId {
                signature: SIGNATURE,
                id: 1,
            },
            target,
            0,
            &mut hotkey,
        )
    };
    if status != 0 || hotkey.is_null() {
        log::warn!("autonomy: the Stop chord could not be registered: {status}");
        return;
    }
    HOTKEY.with(|h| h.set(hotkey));
    REGISTERED.store(true, Ordering::Release);
    log::info!("autonomy: the Stop chord is registered");
}
