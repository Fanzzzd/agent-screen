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
            "height": self.bounds.height as i64
        })
    }
}

/// Normal app windows (layer 0) of other apps, front to back, global bounds.
pub fn all() -> Vec<Window> {
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| w.layer == 0 && w.bounds.width > 40.0 && w.bounds.height > 40.0)
        .map(|w| Window {
            target: w.target,
            app: w.owner_name,
            title: w.title,
            bounds: w.bounds,
        })
        .collect()
}

/// Windows on a display, with display-relative bounds. A window counts if it
/// touches the display and its center is on no other display. A window
/// hanging off the bottom edge still counts, so dismissing the display does
/// not strand it off-screen.
pub fn on_display(display: DisplayId) -> Vec<Window> {
    let area = cg::display_bounds(display);
    let others: Vec<Rect> = cg::active_displays()
        .into_iter()
        .filter(|&d| d != display)
        .map(cg::display_bounds)
        .collect();
    all()
        .into_iter()
        .filter(|w| {
            let center = w.bounds.mid();
            w.bounds.intersects(&area)
                && (area.contains(center) || !others.iter().any(|o| o.contains(center)))
        })
        .map(|w| Window {
            bounds: w.bounds.offset(-area.x, -area.y),
            ..w
        })
        .collect()
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
pub fn place(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
) -> Result<(), String> {
    if let Some(aerospace) = AeroSpace::current()
        && let Some(name) = screen_name
        && let Ok(Some(monitor)) = aerospace.monitor_id(name)
    {
        return aerospace
            .move_to_monitor(target.window_id, &monitor)
            .map_err(|e| format!("AeroSpace refused: {e}"));
    }
    ax::move_into(target, cg::display_bounds(display))
        .map_err(|e| format!("Accessibility refused: {e}"))
}
