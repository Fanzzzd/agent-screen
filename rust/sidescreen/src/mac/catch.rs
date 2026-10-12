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

/// Until when each display counts as just driven: by its agent's input,
/// element actions and launches, or by the user through its viewer. An
/// entry with a pid counts only for that app's windows, as for a cold
/// start's first window.
static ACTED: std::sync::Mutex<Vec<(DisplayId, Option<i32>, std::time::Instant)>> =
    std::sync::Mutex::new(Vec::new());

/// The user's clicks and key presses outside Sidescreen in the last few
/// seconds, and where each went. Events Sidescreen posts to an app's pid
/// never reach the global monitors that note these.
static USER_INPUT: std::sync::Mutex<Vec<(std::time::Instant, Towards)>> =
    std::sync::Mutex::new(Vec::new());

/// Where a press of the user's went.
#[derive(Clone, Copy, Debug)]
pub enum Towards {
    /// A key press, to the app with keyboard focus.
    App(i32),
    /// A click on the window under the pointer, with the frontmost app at
    /// the time for a window gone since, such as a menu. The owner is read
    /// when a new window needs it, not on every click.
    Window(u32, Option<i32>),
    Unknown,
}

/// How long after an action a new window counts as its result.
const ACTION_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// Note that the display was just driven.
pub fn agent_acted(display: DisplayId) {
    drive(display, None, ACTION_WINDOW);
}

/// Count `pid`'s new windows on the display as the agent's for `span`, as
/// while an app it launched is still starting.
pub fn agent_acted_for(display: DisplayId, pid: i32, span: std::time::Duration) {
    drive(display, Some(pid), span);
}

fn drive(display: DisplayId, pid: Option<i32>, span: std::time::Duration) {
    let now = std::time::Instant::now();
    let mut acted = ACTED.lock().unwrap_or_else(|e| e.into_inner());
    acted.retain(|(_, _, until)| *until > now);
    let until = now + span;
    match acted
        .iter_mut()
        .find(|(d, p, _)| *d == display && *p == pid)
    {
        Some(at) => at.2 = at.2.max(until),
        None => acted.push((display, pid, until)),
    }
}

/// Note a click or key press of the user's outside Sidescreen.
pub fn user_acted(towards: Towards) {
    let now = std::time::Instant::now();
    let mut input = USER_INPUT.lock().unwrap_or_else(|e| e.into_inner());
    input.retain(|(at, _)| now.duration_since(*at) < ACTION_WINDOW);
    // Typing fast fills this with the same app; one entry per place will do.
    if !matches!(input.last(), Some((_, Towards::App(p))) if matches!(towards, Towards::App(q) if q == *p))
    {
        input.push((now, towards));
    } else if let Some(last) = input.last_mut() {
        last.0 = now;
    }
}

/// Note a key press of the user's: to the frontmost app at once, and to
/// the app with keyboard focus once Accessibility says which. A launcher
/// such as Spotlight takes keys without coming to the front.
pub fn user_pressed_key() {
    static FOCUS: std::sync::OnceLock<std::sync::mpsc::Sender<()>> = std::sync::OnceLock::new();
    if let Some(pid) = super::app::frontmost_pid() {
        user_acted(Towards::App(pid));
    }
    let focus = FOCUS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            while rx.recv().is_ok() {
                // A burst of keys needs one lookup.
                while rx.try_recv().is_ok() {}
                if let Some(pid) = ax::focused_app_pid() {
                    user_acted(Towards::App(pid));
                }
            }
        });
        tx
    });
    let _ = focus.send(());
}

fn acted_recently(display: DisplayId, pid: i32) -> bool {
    let now = std::time::Instant::now();
    ACTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|(d, p, until)| *d == display && p.is_none_or(|p| p == pid) && now < *until)
}

/// Whether the user just pressed something that could make `pid` open a
/// window: a key or click in that app, or in Finder, the Dock or a
/// launcher such as Spotlight, which open files and apps. Typing in
/// another app doesn't count.
pub fn user_acted_recently(pid: i32) -> bool {
    let input: Vec<Towards> = USER_INPUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(at, _)| at.elapsed() < ACTION_WINDOW)
        .map(|(_, towards)| *towards)
        .collect();
    if input.is_empty() {
        return false;
    }
    let shown = super::cg::on_screen_windows();
    input.into_iter().any(|towards| {
        let to = match towards {
            Towards::App(p) => Some(p),
            Towards::Window(id, front) => shown
                .iter()
                .find(|w| w.target.window_id == id)
                .map(|w| w.target.pid)
                .or(front),
            Towards::Unknown => None,
        };
        to.is_none_or(|p| p == pid || super::app::opens_things(p))
    })
}

/// Whose a new window in an app the agent started is.
#[derive(Debug, PartialEq)]
enum Owner {
    Agent,
    User,
    /// Both acted, or neither: leave the window where it is, for the agent
    /// to place if it's its own.
    Unknown,
}

fn owner(agent: bool, user: bool) -> Owner {
    match (agent, user) {
        (true, false) => Owner::Agent,
        (false, true) => Owner::User,
        _ => Owner::Unknown,
    }
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
    /// Windows left alone because the user made them.
    skipped: std::cell::RefCell<Vec<u32>>,
    /// Windows left where they opened because both the agent and the user
    /// had just acted, or neither: the agent may place them.
    undecided: std::cell::RefCell<Vec<u32>>,
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
            undecided: std::cell::RefCell::new(Vec::new()),
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

    /// From now on, take only windows the agent's actions make.
    pub fn keep(&self) {
        unsafe { (*self.target).kept.set(true) }
    }

    /// Windows the user made in the app.
    pub fn skipped(&self) -> Vec<u32> {
        unsafe { (*self.target).skipped.borrow().clone() }
    }

    /// Windows left where they opened, nobody's for sure.
    pub fn undecided(&self) -> Vec<u32> {
        unsafe { (*self.target).undecided.borrow().clone() }
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
    observer: Observer,
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
    // A kept catch takes a window only right after its display was driven,
    // and records one the user made so nothing moves it later.
    if target.kept.get() {
        match owner(
            acted_recently(target.display, target.pid),
            user_acted_recently(target.pid),
        ) {
            Owner::Agent => {}
            Owner::User => {
                if let Some(id) = ax::window_id(window) {
                    target.skipped.borrow_mut().push(id);
                }
                return;
            }
            Owner::Unknown => {
                if let Some(id) = ax::window_id(window) {
                    target.undecided.borrow_mut().push(id);
                }
                return;
            }
        }
    }
    // A window that doesn't name its document yet may be the file's: look
    // again when its title changes, as it does once the file loads.
    if let Some(document) = &target.document {
        if target.taken.get().is_some() {
            return;
        }
        match ax::document_path(window) {
            Some(shown) if shown == *document => {}
            Some(_) => return,
            None => {
                let name = CFString::from_static_str("AXTitleChanged");
                unsafe { AXObserverAddNotification(observer, window, &*name, refcon) };
                return;
            }
        }
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
    use super::{Owner, owner, take};

    #[test]
    fn a_window_is_the_agents_only_when_the_user_did_nothing() {
        assert_eq!(owner(true, false), Owner::Agent);
        assert_eq!(owner(false, true), Owner::User);
        assert_eq!(
            owner(true, true),
            Owner::Unknown,
            "the user's own click may have made it"
        );
        assert_eq!(
            owner(false, false),
            Owner::Unknown,
            "a slow window stays placeable"
        );
    }

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
