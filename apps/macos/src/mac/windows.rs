//! App windows as the agent sees them, and moving them between displays.

use super::aerospace::AeroSpace;
use super::{ax, cg};
use crate::model::{DisplayId, Rect, WindowTarget};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct Window {
    pub target: WindowTarget,
    pub app: String,
    pub title: String,
    /// Display-relative bounds for windows on a display, global otherwise.
    pub bounds: Rect,
    /// Window level: 0 for documents, 3 for floating panels, 8 for modal
    /// panels and alerts.
    pub layer: i64,
    /// True when the window belongs to an Agent Display via an active record
    /// while its global frame currently sits outside that display's viewport.
    pub off_display: bool,
}

impl Window {
    fn kind_str(&self) -> &'static str {
        match self.layer {
            0 => "window",
            l if l >= 8 => "dialog",
            _ => "panel",
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "windowId": self.target.window_id,
            "app": self.app,
            "title": self.title,
            "x": self.bounds.x as i64,
            "y": self.bounds.y as i64,
            "width": self.bounds.width as i64,
            "height": self.bounds.height as i64,
            "kind": self.kind_str(),
        })
    }

    /// JSON serialized for `listWindows` on an Agent Display (`M2`): omits
    /// misleading coordinates and sets `offDisplay: true` when a recorded
    /// window sits outside its display's viewport.
    pub fn display_json(&self, scale: f64) -> Value {
        if self.off_display {
            json!({
                "windowId": self.target.window_id,
                "app": self.app,
                "title": self.title,
                "kind": self.kind_str(),
                "offDisplay": true,
            })
        } else {
            json!({
                "windowId": self.target.window_id,
                "app": self.app,
                "title": self.title,
                "x": (self.bounds.x * scale).round() as i64,
                "y": (self.bounds.y * scale).round() as i64,
                "width": (self.bounds.width * scale).round() as i64,
                "height": (self.bounds.height * scale).round() as i64,
                "kind": self.kind_str(),
            })
        }
    }
}

/// A window held by its title bar: a drag moves it, inside its display.
/// Posted events can't move a window. WindowServer takes a title-bar drag
/// to the real pointer, and while the user's button is down that is on
/// their own screen.
#[derive(Clone, Copy, Debug)]
pub struct Grab {
    pub target: WindowTarget,
    frame: Rect,
    from: (f64, f64),
    display: DisplayId,
    /// The display's global bounds at the press: refreshed from `display` if
    /// the display moves mid-drag.
    area: Rect,
}

impl Grab {
    /// The window on display `id` whose title bar is at a global point,
    /// held there. A user window that only overhangs the display is never
    /// taken.
    pub fn on_display(id: DisplayId, at: (f64, f64)) -> Option<Grab> {
        let area = cg::display_bounds(id);
        let window = on_display(id)
            .into_iter()
            .map(|w| Window {
                bounds: w.bounds.offset(area.x, area.y),
                ..w
            })
            .find(|w| w.bounds.contains(at))?;
        Grab::title_bar(window.target, window.bounds, at, id)
    }

    /// `target`, at global `bounds` on `display`, if `at` is on its title
    /// bar.
    pub fn title_bar(
        target: WindowTarget,
        bounds: Rect,
        at: (f64, f64),
        display: DisplayId,
    ) -> Option<Grab> {
        let is_sheet = ax::host_window_target(target) != target;
        let min_or_full = ax::is_minimized_or_fullscreen(target);
        Grab::held_checked(target, bounds, at, display, min_or_full, false, is_sheet)
            .filter(|_| ax::on_title_bar(target, bounds, at))
    }

    /// `target`, at global `bounds` belonging to `display`, pressed at `at`
    /// inside `display` wherever that is on the window (including a tall or
    /// wide window hanging past `display`'s edges).
    pub fn held(
        target: WindowTarget,
        bounds: Rect,
        at: (f64, f64),
        display: DisplayId,
    ) -> Option<Grab> {
        let min_or_full = ax::is_minimized_or_fullscreen(target);
        Grab::held_checked(target, bounds, at, display, min_or_full, false, false)
    }

    fn held_checked(
        target: WindowTarget,
        bounds: Rect,
        at: (f64, f64),
        display: DisplayId,
        minimized: bool,
        fullscreen: bool,
        is_sheet: bool,
    ) -> Option<Grab> {
        let area = cg::display_bounds(display);
        let others = other_displays(display);
        agent_screen_model::viewer_press::can_grab_window(
            bounds, at, area, &others, minimized, fullscreen, is_sheet,
        )
        .then_some(Grab {
            target,
            frame: bounds,
            from: at,
            display,
            area,
        })
    }

    fn rebased(&self, to: (f64, f64)) -> (Rect, (f64, f64), (f64, f64), Rect) {
        agent_screen_model::viewer_press::rebase_grab_for_display(
            self.frame,
            self.from,
            to,
            self.area,
            cg::display_bounds(self.display),
        )
    }

    /// Whether the window has left its display, carried off by something
    /// other than this grab. Uses single-window `cg::window_bounds` instead of
    /// enumerating the global window list (M3). Blocking.
    pub fn left_display(&self) -> bool {
        let (_, _, _, area) = self.rebased(self.from);
        let others = other_displays(self.display);
        cg::window_bounds(self.target.window_id).is_some_and(|bounds| {
            !agent_screen_model::arrangement::window_belongs_to_area(&bounds, &area, &others)
        })
    }

    /// Where it was pressed.
    pub fn from(&self) -> (f64, f64) {
        self.rebased(self.from).1
    }

    /// Whether the pointer has gone far enough from the press to count as
    /// a drag rather than a click.
    pub fn moved(&self, to: (f64, f64)) -> bool {
        let (_, from, to, _) = self.rebased(to);
        (to.0 - from.0).hypot(to.1 - from.1) >= 3.0
    }

    /// Move the window's origin as far as the pointer moved from the press,
    /// without mutating its size. Blocking.
    pub fn drag_to(&self, to: (f64, f64)) -> bool {
        let (frame, from, to, area) = self.rebased(to);
        let (x, y) = agent_screen_model::viewer_press::dragged_window_origin(frame, from, to, area);
        ax::set_window_position(self.target, x, y).is_ok()
    }

    /// Put a window that escaped onto the user's screen back at its dragged
    /// frame on the display, restoring its size as well in case the user's
    /// window manager tiled it (`set_frame_asked` also tolerates Chromium's
    /// spurious AX error codes, #167). Blocking.
    pub fn restore_to(&self, to: (f64, f64)) -> bool {
        let (frame, from, to, area) = self.rebased(to);
        let (x, y) = agent_screen_model::viewer_press::dragged_window_origin(frame, from, to, area);
        let _ = ax::set_frame_asked(self.target, Rect::new(x, y, frame.width, frame.height));
        !self.left_display()
    }
}

/// Other apps' windows a user can act on, front to back, global bounds:
/// documents, floating panels such as inspectors, and modal panels and
/// alerts (levels 0 to 8). Menus, the Dock and status items sit higher.
pub fn all() -> Vec<Window> {
    let own = std::process::id() as i32;
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| {
            (0..=8).contains(&w.layer)
                && w.target.pid != own
                && w.bounds.width > 40.0
                && w.bounds.height > 40.0
        })
        .map(|w| Window {
            target: w.target,
            app: w.owner_name,
            title: w.title,
            bounds: w.bounds,
            layer: w.layer,
            off_display: false,
        })
        .collect()
}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, RwLock};

use agent_screen_model::placement::PutAway;

/// Other apps' document windows that exist but aren't on screen, that
/// `keep` takes, with why each is off screen (#221). Global bounds: where
/// each shows when it comes back. Blocking: a few Accessibility reads for
/// each window `keep` takes.
pub fn put_away(mut keep: impl FnMut(&Window) -> bool) -> Vec<(Window, PutAway)> {
    let own = std::process::id() as i32;
    let on = cg::on_screen_ids();
    cg::all_windows()
        .into_iter()
        .filter(|w| w.target.pid != own && !on.contains(&w.target.window_id))
        .map(|w| Window {
            target: w.target,
            app: w.owner_name,
            title: w.title,
            bounds: w.bounds,
            layer: w.layer,
            off_display: false,
        })
        .filter(|w| keep(w))
        .map(|w| {
            let why = why_away(w.target);
            (w, why)
        })
        .collect()
}

/// Why a window that exists isn't on screen. Blocking.
pub fn why_away(target: WindowTarget) -> PutAway {
    let element = ax::window_element(target);
    let minimized = element.as_ref().is_some_and(|w| ax::is_minimized(w));
    PutAway::of(
        element.is_some(),
        minimized,
        super::app::is_hidden(target.pid),
        cg::space_count(target.window_id),
    )
}

/// Bring a window the user put away out onto `display`, without drawing
/// it on their screen (#221). A hidden app's window moves while the app is
/// hidden, as launch_app moves a starting app's, and shows once the app
/// does. A minimized window would play the Dock's restore on the user's
/// screen, so its app hides first, the window leaves the Dock and moves
/// while hidden, and the app shows again. That needs an app in the
/// background with no other window on screen, which hiding would take off
/// the user's screen for a moment. A window that leaves the Dock while its
/// app is hidden doesn't come back when the app shows; a raise puts it on
/// the display, and activates nothing. Blocking.
pub fn bring_out(target: WindowTarget, display: DisplayId) -> Result<(), String> {
    let bounds = cg::display_bounds(display);
    let pid = target.pid;
    let window = ax::window_element(target).ok_or(
        "its window is on another macOS Space, or closed, where Accessibility can't reach it",
    )?;
    let hidden = super::app::is_hidden(pid);
    let minimized = ax::is_minimized(&window);
    log::info!(
        "bringing out window {} of pid {pid}: minimized {minimized}, app hidden {hidden}",
        target.window_id
    );
    if minimized && !hidden {
        if super::app::is_active(pid) {
            return Err(
                "it is minimized and its app is in front, where taking it out of the \
                 Dock would show on the user's screen; ask the user to click it in the Dock"
                    .into(),
            );
        }
        if all().iter().any(|w| w.target.pid == pid) {
            return Err(
                "it is minimized and its app has other windows on the user's \
                 screen, which taking it out of the Dock unseen would hide for a moment; \
                 ask the user to click it in the Dock"
                    .into(),
            );
        }
        super::app::hide(pid);
        if !wait_for(|| super::app::is_hidden(pid)) {
            log::info!("bring_out: pid {pid} didn't hide; unhiding it");
            super::app::unhide(pid);
            return Err("its app didn't hide, so it would show on the user's screen".into());
        }
    }
    if minimized && !ax::set_minimized(&window, false) {
        log::info!(
            "bring_out: AXMinimized false refused for window {}",
            target.window_id
        );
    }
    if let Err(e) = ax::move_element_into(&window, bounds) {
        // Still hidden, it showed nowhere: put it back as it was.
        if minimized {
            ax::set_minimized(&window, true);
        }
        if !hidden {
            super::app::unhide(pid);
        }
        return Err(e.to_string());
    }
    super::app::unhide(pid);
    let on_screen = || cg::on_screen_ids().contains(&target.window_id);
    if minimized && !wait_for_ms(300, on_screen) {
        ax::perform(&window, "AXRaise");
    }
    if !wait_for(on_screen) {
        return Err("it didn't come back on screen".into());
    }
    Ok(())
}

/// Whether `done` came true within 2 s, checked every 20 ms. Blocking.
fn wait_for(done: impl FnMut() -> bool) -> bool {
    wait_for_ms(2000, done)
}

/// Whether `done` came true within `ms`, checked every 20 ms. Blocking.
fn wait_for_ms(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(ms) {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    done()
}

/// Active window-to-display ownership snapshot bound directly to
/// `Core::displays` (`Displays::bind_recorded_owners`, L3), so every
/// `windows::on_display`, `counts`, dismissal, spare, and input-routing check
/// resolves ownership with one rule (`effective_agent_display`, ADR 0018).
static RECORDED_OWNERS: LazyLock<Arc<RwLock<BTreeMap<u32, DisplayId>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(BTreeMap::new())));

pub fn recorded_owners_handle() -> Arc<RwLock<BTreeMap<u32, DisplayId>>> {
    RECORDED_OWNERS.clone()
}

pub fn recorded_owners_snapshot() -> BTreeMap<u32, DisplayId> {
    RECORDED_OWNERS
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn recorded_owner(window_id: u32) -> Option<DisplayId> {
    RECORDED_OWNERS
        .read()
        .ok()
        .and_then(|g| g.get(&window_id).copied())
}

pub fn online_display_areas() -> Vec<(DisplayId, Rect, bool)> {
    cg::active_displays()
        .into_iter()
        .map(|id| {
            (
                id,
                cg::display_bounds(id),
                cg::window_display_role(id) == crate::model::DisplayRole::Agent,
            )
        })
        .collect()
}

/// Pure ownership filter used by `on_display`, `counts`, and unit tests.
pub fn filter_on_display(
    display: DisplayId,
    windows: &[Window],
    online_displays: &[(DisplayId, Rect, bool)],
    recorded_owners: &BTreeMap<u32, DisplayId>,
) -> Vec<Window> {
    let Some(&(_, area, is_agent)) = online_displays.iter().find(|&&(id, _, _)| id == display)
    else {
        return Vec::new();
    };
    if !is_agent {
        let others: Vec<Rect> = online_displays
            .iter()
            .filter(|&&(id, _, _)| id != display)
            .map(|&(_, r, _)| r)
            .collect();
        return windows
            .iter()
            .filter(|w| belongs(&w.bounds, &area, &others))
            .map(|w| Window {
                bounds: w.bounds.offset(-area.x, -area.y),
                off_display: false,
                ..w.clone()
            })
            .collect();
    }
    windows
        .iter()
        .filter_map(|w| {
            let (owner, _) = agent_screen_model::arrangement::effective_agent_display(
                &w.bounds,
                online_displays,
                recorded_owners.get(&w.target.window_id).copied(),
            )?;
            (owner == display).then(|| Window {
                bounds: w.bounds.offset(-area.x, -area.y),
                off_display: !w.bounds.intersects(&area),
                ..w.clone()
            })
        })
        .collect()
}

/// Pure check returning every display in `ids` whose online bounds are
/// physically covered by any window in `windows` (center on the display or more
/// than half of the window's area on it, `M1`), regardless of recorded
/// ownership (`L4`). Prevents `pick_spare` from handing out a spare covered by
/// another display's unshifted recorded window without flagging narrow edge
/// overhangs from an adjacent display.
pub fn physically_occupied_displays(
    ids: &[DisplayId],
    windows: &[Window],
    online_displays: &[(DisplayId, Rect, bool)],
) -> HashSet<DisplayId> {
    ids.iter()
        .copied()
        .filter(|&id| {
            online_displays
                .iter()
                .find(|&&(oid, _, _)| oid == id)
                .is_some_and(|&(_, area, _)| {
                    windows
                        .iter()
                        .any(|w| agent_screen_model::spares::window_covers_spare(&w.bounds, &area))
                })
        })
        .collect()
}

/// Whether a window with `bounds` is on the display covering `area`: it
/// overlaps it, and no other display holds its center instead.
fn belongs(bounds: &Rect, area: &Rect, others: &[Rect]) -> bool {
    agent_screen_model::arrangement::window_belongs_to_area(bounds, area, others)
}

fn other_displays(display: DisplayId) -> Vec<Rect> {
    cg::active_displays()
        .into_iter()
        .filter(|&d| d != display)
        .map(cg::display_bounds)
        .collect()
}

/// How many windows each display has, plus which displays have any window
/// physically intersecting their viewport (`L4`).
pub fn counts_and_physical(
    displays: &[DisplayId],
) -> (HashMap<DisplayId, usize>, HashSet<DisplayId>) {
    if displays.is_empty() {
        return (HashMap::new(), HashSet::new());
    }
    let windows = all();
    let online = online_display_areas();
    let owners = recorded_owners_snapshot();
    let counts = displays
        .iter()
        .map(|&id| (id, filter_on_display(id, &windows, &online, &owners).len()))
        .collect();
    let physical = physically_occupied_displays(displays, &windows, &online);
    (counts, physical)
}

/// How many windows each display has, from one window list and one display
/// layout snapshot, honoring `RECORDED_OWNERS` (ADR 0018).
pub fn counts(displays: &[DisplayId]) -> HashMap<DisplayId, usize> {
    counts_and_physical(displays).0
}

/// Windows on a display, with display-relative bounds, resolved through
/// `effective_agent_display` and `RECORDED_OWNERS` (ADR 0018).
pub fn on_display(display: DisplayId) -> Vec<Window> {
    let windows = all();
    let online = online_display_areas();
    let owners = recorded_owners_snapshot();
    filter_on_display(display, &windows, &online, &owners)
}

/// Menus open on a display: an app's pop-up menus, as a web page's select
/// shows, at the pop-up menu level (kCGPopUpMenuWindowLevel).
pub fn menus_on(display: DisplayId) -> Vec<u32> {
    let own = std::process::id() as i32;
    let area = cg::display_bounds(display);
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| w.layer == 101 && w.target.pid != own && w.bounds.intersects(&area))
        .map(|w| w.target.window_id)
        .collect()
}

/// Move document windows that grew past the display's edges, such as
/// Calculator switching to Scientific, or under the strip that holds the
/// user's menu bar, as a zoomed window does, back inside. True when one
/// moved.
pub fn keep_inside(display: DisplayId) -> bool {
    let area = cg::display_bounds(display);
    let local = crate::model::usable_area(
        Rect::new(0.0, 0.0, area.width, area.height),
        super::menubar_height(display),
    );
    let mut moved = false;
    for window in on_display(display)
        .into_iter()
        .filter(|w| w.layer == 0 && !w.off_display)
    {
        let kept = window.bounds.kept_inside(&local);
        if kept != window.bounds {
            moved |= ax::set_frame(window.target, kept.offset(area.x, area.y)).is_ok();
        }
    }
    moved
}

pub fn find(window_id: u32) -> Option<Window> {
    all().into_iter().find(|w| w.target.window_id == window_id)
}

/// Whether the window sits on a virtual display of Agent Screen's, any
/// instance's, rather than on one of the user's screens. Blocking.
pub fn on_virtual_display(window_id: u32) -> bool {
    find(window_id).is_some_and(|w| display_role(&w) == crate::model::DisplayRole::Agent)
}

pub fn display_role(window: &Window) -> crate::model::DisplayRole {
    let mid = window.bounds.mid();
    cg::active_displays()
        .into_iter()
        .find(|&id| cg::display_bounds(id).contains(mid))
        .map(cg::window_display_role)
        .unwrap_or(crate::model::DisplayRole::User)
}

/// Whether `bounds`'s center sits on one of `displays`. Blocking.
pub fn on_any(bounds: &Rect, displays: &[DisplayId]) -> bool {
    let mid = bounds.mid();
    displays
        .iter()
        .any(|&id| cg::display_bounds(id).contains(mid))
}

/// Windows on any of `displays` that AeroSpace leaves there. With
/// `ignored-monitors` it lets go of a window that lands on an Agent Display
/// only once its own list of screens has the display, which comes late
/// after a display appears: a window macOS puts back on a new display that
/// took an old one's place, and AeroSpace still files in the user's
/// workspace, goes back there. A new window it files for a few seconds
/// and then lets go. Blocking.
pub fn kept_on(displays: &[DisplayId]) -> std::collections::HashSet<u32> {
    let mut on: std::collections::HashSet<u32> = displays
        .iter()
        .filter(|&&d| cg::window_display_role(d) == crate::model::DisplayRole::Agent)
        .flat_map(|&d| on_display(d))
        .map(|w| w.target.window_id)
        .collect();
    if super::aerospace::ignores_now()
        && let Some(aerospace) = AeroSpace::current()
        && let Ok(managed) = aerospace.managed_windows()
    {
        on.retain(|w| !managed.contains(w));
    }
    on
}

/// Move a window onto a display in whatever way the user's window manager
/// respects. AeroSpace re-tiles any window moved behind its back, so with
/// AeroSpace running it moves the window to the workspace shown on the
/// display. AeroSpace with `ignored-monitors` doesn't list the display and
/// leaves windows on it alone; then, or without AeroSpace, the Accessibility
/// API moves it.
///
/// Blocking. `screen_name` is the display's localized name, read on the
/// main thread.
/// Whether a window placed on a display stays there. AeroSpace can take a
/// move for a window it hasn't registered yet, then lay the window out in
/// the user's workspace once it does: the window must sit in the workspace
/// AeroSpace shows on the display, not just inside the display's bounds.
pub fn held(target: WindowTarget, display: DisplayId, screen_name: Option<&str>) -> bool {
    if !on_display(display).iter().any(|w| w.target == target) {
        return false;
    }
    if super::aerospace::ignores_now() {
        return true;
    }
    let Some(aerospace) = AeroSpace::current() else {
        return true;
    };
    let Some(name) = screen_name else {
        return true;
    };
    match aerospace.whereabouts(target.window_id) {
        Ok(Some((monitor, visible))) => monitor == name && visible,
        // Not managed by AeroSpace, such as a panel: its bounds decide.
        Ok(None) => true,
        Err(_) => false,
    }
}

/// What Agent Screen knows of windows the user took back, and a lane per
/// window for the OS calls that move it. A move for the agent, a frame
/// write and a move home of one window take its lane in turn, so one
/// decided before a Take Back can't put the window on the display again
/// after it went home. The state lock is held only to read or change the
/// state, never across an OS call: the main thread reads it, and windows
/// never wait for each other.
#[derive(Default)]
pub struct Lanes {
    state: std::sync::Mutex<Returns>,
    lanes: std::sync::Mutex<std::collections::HashMap<u32, std::sync::Arc<std::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct Returns {
    /// Taken back and not handed over since.
    returned: std::collections::BTreeSet<u32>,
    /// Moves home so far, by window.
    count: std::collections::HashMap<u32, u64>,
}

impl Lanes {
    fn state(&self) -> std::sync::MutexGuard<'_, Returns> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` in the window's lane, after the window's earlier OS calls.
    fn in_lane<T>(&self, window: u32, f: impl FnOnce() -> T) -> T {
        let lane = self
            .lanes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(window)
            .or_default()
            .clone();
        let held = lane.lock().unwrap_or_else(|e| e.into_inner());
        let out = f();
        drop(held);
        // Nobody else waits on it: let it go.
        let mut lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        if lanes
            .get(&window)
            .is_some_and(|l| std::sync::Arc::strong_count(l) == 2)
        {
            lanes.remove(&window);
        }
        out
    }

    /// How many times a window went home so far. Never waits for an OS call.
    pub fn times_returned(&self, window: u32) -> u64 {
        self.state().count.get(&window).copied().unwrap_or(0)
    }

    /// Move for the agent, unless the user took the window back.
    pub fn agent_move<T>(
        &self,
        window: u32,
        f: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.in_lane(window, || {
            if self.state().returned.contains(&window) {
                return Err("the user took the window back".into());
            }
            f()
        })
    }

    /// Mark the window as the user's, then send it home with `home`. A
    /// window `home` couldn't move off the display is the agent's to move
    /// again.
    pub fn home(&self, window: u32, home: impl FnOnce() -> bool) -> bool {
        self.in_lane(window, || {
            let was_returned = {
                let mut state = self.state();
                *state.count.entry(window).or_default() += 1;
                !state.returned.insert(window)
            };
            let left = home();
            if !left && !was_returned {
                self.state().returned.remove(&window);
            }
            left
        })
    }

    /// Run `f`, for a hand-over or a frame write decided when the window had
    /// gone home `seen` times, unless it went home again since. With
    /// `handed_over`, the window is the agent's to move from now on.
    pub fn unless_returned_since<T>(
        &self,
        window: u32,
        seen: u64,
        handed_over: bool,
        f: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.in_lane(window, || {
            if self.state().count.get(&window).copied().unwrap_or(0) != seen {
                return Err("the user took the window back meanwhile".into());
            }
            let result = f()?;
            if handed_over {
                self.state().returned.remove(&window);
            }
            Ok(result)
        })
    }
}

static LANES: std::sync::LazyLock<Lanes> = std::sync::LazyLock::new(Lanes::default);

/// Move a window onto a display for the agent, unless the user took it
/// back. Blocking.
pub fn place_for_agent(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
) -> Result<(), String> {
    LANES.agent_move(target.window_id, || place(target, display, screen_name))
}

/// Mark a window as the user's, after any move for the agent in flight,
/// then send it home with `home`, before any later one. `home` says whether
/// the window left the display, and so does this. Blocking.
pub fn return_home(window: u32, home: impl FnOnce() -> bool) -> bool {
    LANES.home(window, home)
}

/// How many times a window went home so far: read on the main thread when
/// a move or frame write is decided. Doesn't wait for any OS call.
pub fn times_returned(window: u32) -> u64 {
    LANES.times_returned(window)
}

/// Move a window handed over onto a display, unless it went home again
/// since the hand-over was decided (`seen` from `times_returned`). It is
/// the agent's to keep there from now on. Blocking.
pub fn place_handed_over(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
    seen: u64,
) -> Result<(), String> {
    LANES.unless_returned_since(target.window_id, seen, true, || {
        // Minimized, or its app hidden: it comes out onto the display.
        if !cg::on_screen_ids().contains(&target.window_id) {
            bring_out(target, display)?;
        }
        place(target, display, screen_name)
    })
}

/// Write a window's frame for the agent, unless it went home since `seen`.
/// Blocking.
pub fn write_frame_unless_returned<T>(
    window: u32,
    seen: u64,
    write: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    LANES.unless_returned_since(window, seen, false, write)
}

pub fn place(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
) -> Result<(), String> {
    if !super::aerospace::ignores_now()
        && let Some(aerospace) = AeroSpace::current()
        && let Some(name) = screen_name
        && let Ok(Some(monitor)) = aerospace.monitor_id(name)
    {
        // AeroSpace doesn't manage panels, such as an Open panel, and
        // learns of a new window a moment late: move those directly.
        let Err(refused) = aerospace.move_to_monitor(target.window_id, &monitor) else {
            return Ok(());
        };
        return ax::move_into(target, cg::display_bounds(display))
            .map_err(|e| format!("AeroSpace refused ({refused}) and Accessibility refused ({e})"));
    }
    ax::move_into(target, cg::display_bounds(display))
        .map_err(|e| format!("Accessibility refused: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Runs `f` on a thread; whether it finished within 300 ms.
    fn finishes(f: impl FnOnce() + Send + 'static) -> bool {
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            f();
            let _ = done.send(());
        });
        finished.recv_timeout(Duration::from_millis(300)).is_ok()
    }

    #[test]
    fn a_stuck_move_home_blocks_neither_reads_nor_other_windows() {
        let lanes = std::sync::Arc::new(Lanes::default());
        let (release, held) = mpsc::channel::<()>();
        let (entered, inside) = mpsc::channel::<()>();
        let stuck = std::thread::spawn({
            let lanes = lanes.clone();
            move || {
                lanes.home(1, move || {
                    let _ = entered.send(());
                    // An app that doesn't answer Accessibility.
                    let _ = held.recv_timeout(Duration::from_secs(5));
                    true
                })
            }
        });
        inside.recv_timeout(Duration::from_secs(1)).unwrap();
        let read = finishes({
            let lanes = lanes.clone();
            move || {
                lanes.times_returned(1);
            }
        });
        let other_home = finishes({
            let lanes = lanes.clone();
            move || {
                lanes.home(2, || true);
            }
        });
        let other_move = finishes({
            let lanes = lanes.clone();
            move || {
                let _ = lanes.agent_move(3, || Ok(()));
                let _ = lanes.unless_returned_since(4, 0, true, || Ok(()));
            }
        });
        // The same window waits its turn.
        let same = finishes({
            let lanes = lanes.clone();
            move || {
                let _ = lanes.agent_move(1, || Ok(()));
            }
        });
        let _ = release.send(());
        stuck.join().unwrap();
        assert!(read, "the main thread's read waited for an OS call");
        assert!(
            other_home && other_move,
            "another window waited for this one's OS call"
        );
        assert!(!same, "the same window's move ran beside its move home");
        // Once home, the agent's move is refused.
        assert!(lanes.agent_move(1, || Ok(())).is_err());
    }

    #[test]
    fn a_move_decided_before_a_take_back_stays_home() {
        let lanes = Lanes::default();
        let seen = lanes.times_returned(5);
        lanes.home(5, || true);
        let mut moved = false;
        assert!(
            lanes
                .unless_returned_since(5, seen, true, || {
                    moved = true;
                    Ok(())
                })
                .is_err()
        );
        assert!(!moved);
        // Handed over again after it: the agent's to move once more.
        let now = lanes.times_returned(5);
        assert!(lanes.unless_returned_since(5, now, true, || Ok(())).is_ok());
        assert!(lanes.agent_move(5, || Ok(())).is_ok());
    }

    #[test]
    fn a_failed_handover_keeps_the_window_returned() {
        let lanes = Lanes::default();
        lanes.home(6, || true);
        let seen = lanes.times_returned(6);
        assert!(
            lanes
                .unless_returned_since(6, seen, true, || {
                    Err::<(), _>("the app refused the move".into())
                })
                .is_err()
        );
        assert!(lanes.agent_move(6, || Ok(())).is_err());
    }

    #[test]
    fn a_window_that_couldnt_go_home_is_the_agents_to_move_again() {
        let lanes = Lanes::default();
        assert!(!lanes.home(7, || false));
        assert!(lanes.agent_move(7, || Ok(())).is_ok());
        // Still, a move decided before the try doesn't follow it.
        assert!(lanes.unless_returned_since(7, 0, false, || Ok(())).is_err());
        // One the user had taken back before stays theirs.
        assert!(lanes.home(8, || true));
        assert!(!lanes.home(8, || false));
        assert!(lanes.agent_move(8, || Ok(())).is_err());
    }

    #[test]
    fn a_frame_write_after_the_window_went_home_doesnt_run() {
        let seen = times_returned(900_003);
        return_home(900_003, || true);
        let mut wrote = false;
        let result = write_frame_unless_returned(900_003, seen, || {
            wrote = true;
            Ok(())
        });
        assert!(!wrote && result.is_err(), "the writer ran after Take Back");
        // Decided after the return, it writes.
        let now = times_returned(900_003);
        assert!(write_frame_unless_returned(900_003, now, || Ok(())).is_ok());
    }

    #[test]
    fn filter_on_display_uses_recorded_owners_on_virtual_displays_and_geometry_on_user_screens() {
        let online = [
            (1, Rect::new(0.0, 0.0, 2560.0, 1440.0), false),
            (7, Rect::new(2560.0, -800.0, 1280.0, 800.0), true),
            (8, Rect::new(3840.0, -800.0, 1280.0, 800.0), true),
        ];
        let win = |window_id, x, y| Window {
            target: WindowTarget { pid: 42, window_id },
            app: "Test".into(),
            title: format!("w{window_id}"),
            bounds: Rect::new(x, y, 400.0, 300.0),
            layer: 0,
            off_display: false,
        };
        let windows = vec![
            // Recorded on 7, transiently sitting on 8 before AX shift finishes.
            win(10, 3940.0, -700.0),
            // Unrecorded sheet sitting on 8.
            win(11, 4000.0, -650.0),
            // Recorded on 7, returned to user screen 1.
            win(12, 200.0, 200.0),
            // Recorded on 7, actually sitting on 7.
            win(13, 2660.0, -700.0),
        ];
        let mut owners = BTreeMap::new();
        owners.insert(10, 7);
        owners.insert(12, 7);
        owners.insert(13, 7);

        let on_7 = filter_on_display(7, &windows, &online, &owners);
        assert_eq!(on_7.len(), 2);
        assert_eq!(on_7[0].target.window_id, 10);
        assert!(on_7[0].off_display);
        assert_eq!(on_7[0].bounds, Rect::new(1380.0, 100.0, 400.0, 300.0));
        let off_json = on_7[0].display_json(1.0);
        assert_eq!(off_json["offDisplay"], json!(true));
        assert!(off_json.get("x").is_none() && off_json.get("y").is_none());

        assert_eq!(on_7[1].target.window_id, 13);
        assert!(!on_7[1].off_display);
        let on_json = on_7[1].display_json(1.0);
        assert!(on_json.get("offDisplay").is_none());
        assert_eq!(on_json["x"], json!(100));
        assert_eq!(on_json["y"], json!(100));

        // Display 8 sees only its own unrecorded window 11 in `filter_on_display`,
        // yet `physically_occupied_displays` STILL flags 8 as occupied if 8 were
        // a spare covered by 7's unshifted window 10 (L4).
        let on_8 = filter_on_display(8, &windows, &online, &owners);
        assert_eq!(on_8.len(), 1);
        assert_eq!(on_8[0].target.window_id, 11);
        let occupied = physically_occupied_displays(&[8], &[windows[0].clone()], &online);
        assert!(occupied.contains(&8));
        // A narrow overhang from a window centered on display 7 does NOT mark 8 occupied (M1).
        let overhang = win(14, 920.0, 100.0);
        assert!(physically_occupied_displays(&[8], &[overhang], &online).is_empty());

        let on_user = filter_on_display(1, &windows, &online, &owners);
        assert_eq!(on_user.len(), 1);
        assert_eq!(on_user[0].target.window_id, 12);
    }
}
