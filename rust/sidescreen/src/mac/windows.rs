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
    let Some(aerospace) = AeroSpace::current() else {
        return true;
    };
    let Some(monitor) = screen_name.and_then(|name| aerospace.monitor_id(name).ok().flatten())
    else {
        return true;
    };
    match (
        aerospace.workspace_of(target.window_id),
        aerospace.visible_workspace(&monitor),
    ) {
        (Ok(Some(workspace)), Ok(Some(visible))) => workspace == visible,
        // Not managed by AeroSpace, such as a panel: its bounds decide.
        (Ok(None), _) => true,
        _ => false,
    }
}

pub fn place(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
) -> Result<(), String> {
    if let Some(aerospace) = AeroSpace::current()
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
