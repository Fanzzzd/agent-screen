//! Catch an app's new windows as macOS creates them and move them onto an
//! Agent Display at once. Found by polling instead, a window shows on the
//! user's screen for a moment, and AeroSpace files it in the user's
//! workspace and lays their windows out around it.

use super::ax;
use super::windows;
use crate::model::{DisplayId, Rect, WindowTarget};
use objc2_application_services::AXUIElement;
use objc2_core_foundation::CFString;
use std::ffi::c_void;

type Observer = *mut c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXObserverCreate(
        pid: i32,
        callback: extern "C" fn(Observer, *const AXUIElement, *const CFString, *mut c_void),
        observer: *mut Observer,
    ) -> i32;
    fn AXObserverAddNotification(
        observer: Observer,
        element: *const AXUIElement,
        notification: *const CFString,
        refcon: *mut c_void,
    ) -> i32;
    fn AXObserverGetRunLoopSource(observer: Observer) -> *mut c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const CFString);
    fn CFRunLoopRemoveSource(run_loop: *mut c_void, source: *mut c_void, mode: *const CFString);
    fn CFRelease(object: *const c_void);
    static kCFRunLoopDefaultMode: *const CFString;
}

/// Moves each window `pid` creates onto a display while it lives. Create
/// and drop it on the main thread, whose run loop delivers the notifications.
pub struct Catch {
    observer: Observer,
    target: *mut Target,
}

/// Where caught windows go.
struct Target {
    pid: i32,
    display: DisplayId,
    bounds: Rect,
    /// The display's screen name, for AeroSpace.
    screen: Option<String>,
}

impl Catch {
    /// None while the app doesn't take observers yet, as just after launch.
    pub fn start(pid: i32, display: DisplayId, screen: Option<String>) -> Option<Catch> {
        let mut observer: Observer = std::ptr::null_mut();
        if unsafe { AXObserverCreate(pid, created, &mut observer) } != 0 || observer.is_null() {
            return None;
        }
        let target = Box::into_raw(Box::new(Target {
            pid,
            display,
            bounds: super::cg::display_bounds(display),
            screen,
        }));
        let app = ax::application(pid);
        let name = CFString::from_static_str("AXWindowCreated");
        let added = unsafe { AXObserverAddNotification(observer, &*app, &*name, target.cast()) };
        if added != 0 {
            unsafe {
                CFRelease(observer);
                drop(Box::from_raw(target));
            }
            return None;
        }
        unsafe {
            CFRunLoopAddSource(
                CFRunLoopGetMain(),
                AXObserverGetRunLoopSource(observer),
                kCFRunLoopDefaultMode,
            );
        }
        Some(Catch { observer, target })
    }
}

impl Drop for Catch {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopRemoveSource(
                CFRunLoopGetMain(),
                AXObserverGetRunLoopSource(self.observer),
                kCFRunLoopDefaultMode,
            );
            CFRelease(self.observer);
            drop(Box::from_raw(self.target));
        }
    }
}

extern "C" fn created(
    _observer: Observer,
    element: *const AXUIElement,
    _notification: *const CFString,
    refcon: *mut c_void,
) {
    if element.is_null() || refcon.is_null() {
        return;
    }
    let (window, target) = unsafe { (&*element, &*refcon.cast::<Target>()) };
    // Panels and sheets stay where the app puts them; Sidescreen places
    // those through the usual path.
    if ax::string_attribute(window, "AXRole").as_deref() != Some("AXWindow") {
        return;
    }
    let moved = ax::move_element_into(window, target.bounds).is_ok();
    log::debug!("catch: a new window, moved onto the display: {moved}");
    // AeroSpace registers the window a moment later, in the user's focused
    // workspace, and lays it out there. Move it to the display's workspace
    // as soon as AeroSpace knows it.
    let (Some(id), Some(screen)) = (ax::window_id(window), target.screen.clone()) else {
        return;
    };
    follow_aerospace(
        WindowTarget {
            pid: target.pid,
            window_id: id,
        },
        target.display,
        screen,
    );
}

/// Once AeroSpace registers a new window, which it files in the user's
/// focused workspace whatever the window's position, move it to the
/// display's workspace. Polls on a thread of its own, every 10 ms for 500 ms.
pub fn follow_aerospace(target: WindowTarget, display: DisplayId, screen: String) {
    let id = target.window_id;
    std::thread::spawn(move || {
        let Some(aerospace) = super::aerospace::AeroSpace::current() else {
            return;
        };
        for _ in 0..50 {
            if let Ok(Some((monitor, _))) = aerospace.whereabouts(id) {
                if monitor != screen {
                    let _ = windows::place(target, display, Some(&screen));
                }
                log::debug!("catch: AeroSpace first had window {id} on {monitor}");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}
