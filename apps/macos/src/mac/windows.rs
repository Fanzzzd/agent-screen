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
}

impl Window {
    pub fn json(&self) -> Value {
        json!({
            "windowId": self.target.window_id,
            "app": self.app,
            "title": self.title,
            "x": self.bounds.x as i64,
            "y": self.bounds.y as i64,
            "width": self.bounds.width as i64,
            "height": self.bounds.height as i64,
            "kind": match self.layer {
                0 => "window",
                l if l >= 8 => "dialog",
                _ => "panel",
            }
        })
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
    /// The display's global bounds: the window stays inside them.
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
        Grab::title_bar(window.target, window.bounds, at, area)
    }

    /// `target`, at global `bounds` inside `area`, if `at` is on its title
    /// bar.
    pub fn title_bar(
        target: WindowTarget,
        bounds: Rect,
        at: (f64, f64),
        area: Rect,
    ) -> Option<Grab> {
        Grab::held(target, bounds, at, area).filter(|_| ax::on_title_bar(target, bounds, at))
    }

    /// `target`, at global `bounds` inside `area`, pressed at `at`
    /// wherever that is on it.
    pub fn held(target: WindowTarget, bounds: Rect, at: (f64, f64), area: Rect) -> Option<Grab> {
        area.contains(bounds.mid()).then_some(Grab {
            target,
            frame: bounds,
            from: at,
            area,
        })
    }

    /// Whether the window has left its display, carried off by something
    /// other than this grab. Blocking.
    pub fn left_display(&self) -> bool {
        find(self.target.window_id).is_some_and(|w| !self.area.contains(w.bounds.mid()))
    }

    /// Where it was pressed.
    pub fn from(&self) -> (f64, f64) {
        self.from
    }

    /// Whether the pointer has gone far enough from the press to count as
    /// a drag rather than a click.
    pub fn moved(&self, to: (f64, f64)) -> bool {
        (to.0 - self.from.0).hypot(to.1 - self.from.1) >= 3.0
    }

    /// Move the window as far as the pointer moved from the press.
    /// Blocking.
    pub fn drag_to(&self, to: (f64, f64)) -> bool {
        let frame = self
            .frame
            .offset(to.0 - self.from.0, to.1 - self.from.1)
            .kept_inside(&self.area);
        ax::set_frame(self.target, frame).is_ok()
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
        })
        .collect()
}

/// Whether a window with `bounds` is on the display covering `area`: it
/// overlaps it, and no other display holds its center instead.
fn belongs(bounds: &Rect, area: &Rect, others: &[Rect]) -> bool {
    let center = bounds.mid();
    bounds.intersects(area) && (area.contains(center) || !others.iter().any(|o| o.contains(center)))
}

fn other_displays(display: DisplayId) -> Vec<Rect> {
    cg::active_displays()
        .into_iter()
        .filter(|&d| d != display)
        .map(cg::display_bounds)
        .collect()
}

/// How many windows each display has, from one window list.
pub fn counts(displays: &[DisplayId]) -> std::collections::HashMap<DisplayId, usize> {
    let windows = all();
    displays
        .iter()
        .map(|&id| {
            let (area, others) = (cg::display_bounds(id), other_displays(id));
            let n = windows
                .iter()
                .filter(|w| belongs(&w.bounds, &area, &others))
                .count();
            (id, n)
        })
        .collect()
}

/// Windows on a display, with display-relative bounds. A window counts if it
/// touches the display and its center is on no other display. A window
/// hanging off the bottom edge still counts, so dismissing the display does
/// not strand it off-screen.
pub fn on_display(display: DisplayId) -> Vec<Window> {
    let area = cg::display_bounds(display);
    let others = other_displays(display);
    all()
        .into_iter()
        .filter(|w| belongs(&w.bounds, &area, &others))
        .map(|w| Window {
            bounds: w.bounds.offset(-area.x, -area.y),
            ..w
        })
        .collect()
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
/// Calculator switching to Scientific, back inside. True when one moved.
pub fn keep_inside(display: DisplayId) -> bool {
    let area = cg::display_bounds(display);
    let local = Rect::new(0.0, 0.0, area.width, area.height);
    let mut moved = false;
    for window in on_display(display).into_iter().filter(|w| w.layer == 0) {
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
