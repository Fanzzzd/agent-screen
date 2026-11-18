//! macOS's own prompts, such as a keychain password request or an Apple
//! Account sign-in alert, belong to the user. macOS can put one on an Agent
//! Display, where the user doesn't see it and an agent could answer it.
//! `send_home` moves them to the user's main display.

use super::cg;
use crate::model::{DisplayId, Rect};

/// Move system alerts off `displays` to the middle of the main display.
/// The ones moved, as "owner window" for the log. Blocking.
pub fn send_home(displays: &[DisplayId]) -> Vec<String> {
    if displays.is_empty() {
        return Vec::new();
    }
    let areas: Vec<Rect> = displays.iter().map(|&id| cg::display_bounds(id)).collect();
    let home = cg::display_bounds(cg::main_display());
    let mut moved = Vec::new();
    for window in cg::on_screen_windows() {
        if !agent_screen_model::alerts::is_system_alert(&window.owner_name) {
            continue;
        }
        let (x, y) = window.bounds.mid();
        if !areas.iter().any(|a| a.contains((x, y))) {
            continue;
        }
        let Some(element) = super::ax::window_element(window.target) else {
            log::warn!(
                "{} window {} is on an agent display and accessibility can't reach it",
                window.owner_name,
                window.target.window_id
            );
            continue;
        };
        let (mid_x, mid_y) = home.mid();
        let at = (
            mid_x - window.bounds.width / 2.0,
            (mid_y - window.bounds.height / 2.0).max(home.y + 40.0),
        );
        if super::ax::set_position(&element, at.0, at.1) {
            moved.push(format!("{} {}", window.owner_name, window.target.window_id));
        }
    }
    moved
}
