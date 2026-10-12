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
    /// How many more windows to take; None for every one.
    left: std::cell::Cell<Option<usize>>,
    /// The last window taken.
    taken: std::cell::Cell<Option<u32>>,
    /// Set while a launch of the same app runs its own catch.
    paused: std::cell::Cell<bool>,
    /// Take only a window that shows this file.
    document: Option<std::path::PathBuf>,
    /// Kept while the display lives, past the launch.
    kept: std::cell::Cell<bool>,
    /// Windows left alone because the user made them with the app in front.
    skipped: std::cell::RefCell<Vec<u32>>,
}

/// Whether to take a new window, and how many are left after it.
fn take(left: Option<usize>) -> (bool, Option<usize>) {
    match left {
        None => (true, None),
        Some(0) => (false, Some(0)),
        Some(n) => (true, Some(n - 1)),
    }
}

impl Catch {
    /// None while the app doesn't take observers yet, as just after launch.
    /// `limit` caps how many windows it takes: an app the user runs too
    /// makes windows of the user's in the meantime, so take only the one
    /// Sidescreen asks for.
    pub fn start(
        pid: i32,
        display: DisplayId,
        screen: Option<String>,
        limit: Option<usize>,
    ) -> Option<Catch> {
        Self::start_with(pid, display, screen, limit, None)
    }

    /// A catch for the one window that shows `document`, for a running app
    /// still opening a file: no window of the user's can match it.
    pub fn for_document(
        pid: i32,
        display: DisplayId,
        screen: Option<String>,
        document: std::path::PathBuf,
    ) -> Option<Catch> {
        Self::start_with(pid, display, screen, Some(1), Some(document))
    }

    fn start_with(
        pid: i32,
        display: DisplayId,
        screen: Option<String>,
        limit: Option<usize>,
        document: Option<std::path::PathBuf>,
    ) -> Option<Catch> {
        let mut observer: Observer = std::ptr::null_mut();
        if unsafe { AXObserverCreate(pid, created, &mut observer) } != 0 || observer.is_null() {
            return None;
        }
        let target = Box::into_raw(Box::new(Target {
            pid,
            display,
            bounds: super::cg::display_bounds(display),
            screen,
            left: std::cell::Cell::new(limit),
            taken: std::cell::Cell::new(None),
            paused: std::cell::Cell::new(false),
            document,
            kept: std::cell::Cell::new(false),
            skipped: std::cell::RefCell::new(Vec::new()),
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

    /// From now on, leave alone windows the user makes with the app in front.
    pub fn keep(&self) {
        unsafe { (*self.target).kept.set(true) }
    }

    /// Windows the user made in the app while it was in front.
    pub fn skipped(&self) -> Vec<u32> {
        unsafe { (*self.target).skipped.borrow().clone() }
    }

    /// Let a launch of the same app take its window alone.
    pub fn pause(&self, paused: bool) {
        unsafe { (*self.target).paused.set(paused) }
    }

    pub fn pid(&self) -> i32 {
        unsafe { (*self.target).pid }
    }

    /// The window id of the last window taken, if any.
    pub fn taken(&self) -> Option<u32> {
        unsafe { (*self.target).taken.get() }
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
    if target.paused.get() {
        return;
    }
    // Panels and sheets stay where the app puts them; Sidescreen places
    // those through the usual path.
    if ax::string_attribute(window, "AXRole").as_deref() != Some("AXWindow") {
        return;
    }
    // A kept catch leaves alone windows the user makes while using the app.
    if target.kept.get() && super::app::frontmost_pid() == Some(target.pid) {
        if let Some(id) = ax::window_id(window) {
            target.skipped.borrow_mut().push(id);
        }
        return;
    }
    // A window that doesn't show the file yet is left to the poll behind it.
    if let Some(document) = &target.document
        && ax::document_path(window).as_ref() != Some(document)
    {
        return;
    }
    let (taken, left) = take(target.left.get());
    target.left.set(left);
    if !taken {
        log::info!(
            "catch: pid {} made another window, {:?}, after the one Sidescreen asked for; left where it is",
            target.pid,
            ax::window_id(window)
        );
        return;
    }
    target.taken.set(ax::window_id(window));
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
/// display's workspace. Polls on a thread of its own, 6 times over 630 ms.
pub fn follow_aerospace(target: WindowTarget, display: DisplayId, screen: String) {
    let id = target.window_id;
    // With ignored-monitors, AeroSpace leaves the window alone.
    if super::aerospace::ignores_now() {
        return;
    }
    let Some(aerospace) = super::aerospace::AeroSpace::current() else {
        return;
    };
    std::thread::spawn(move || {
        for wait in [10, 20, 40, 80, 160, 320] {
            if let Ok(Some((monitor, _))) = aerospace.whereabouts(id) {
                if monitor != screen {
                    let _ = windows::place(target, display, Some(&screen));
                }
                log::debug!("catch: AeroSpace first had window {id} on {monitor}");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(wait));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::take;

    #[test]
    fn takes_only_as_many_windows_as_asked() {
        assert_eq!(take(Some(1)), (true, Some(0)));
        assert_eq!(
            take(Some(0)),
            (false, Some(0)),
            "the user's next window stays"
        );
        assert_eq!(take(None), (true, None));
    }
}
