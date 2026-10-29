//! Dismissing an Agent Display: every window on it is either returned to the
//! user or closed, then the display goes away.
//!
//! The agent's Disposition decides each window by default. Without one, the
//! windows the agent opened close and everything else returns. A Handover
//! always returns, whatever the agent said. The user sees the plan and can
//! change any window before confirming.

use crate::i18n::t;
use crate::mac::aerospace::AeroSpace;
use crate::mac::display::VirtualDisplay;
use crate::mac::{ax, cg, windows};
use crate::model::{Display, Disposition, Fate, Home, Origin, Rect, WindowTarget, default_fate};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct Item {
    pub target: WindowTarget,
    pub app: String,
    pub title: String,
    pub fate: Fate,
    /// A Handover: the user's own window, never closed.
    pub locked: bool,
    pub home: Option<Home>,
    /// Why the default is what it is, shown under the window's name.
    pub reason: String,
}

impl Item {
    /// "Safari · Flights", or just the app when the title adds nothing.
    pub fn name(&self) -> String {
        if self.title.is_empty() || self.title == self.app {
            self.app.clone()
        } else {
            format!("{} · {}", self.app, self.title)
        }
    }
}

pub fn plan(display: &Display<VirtualDisplay>) -> Vec<Item> {
    let launched: HashSet<i32> = display
        .records
        .values()
        .filter(|r| r.origin == Origin::Launched)
        .map(|r| r.target.pid)
        .collect();
    windows::on_display(display.id)
        .into_iter()
        .map(|window| {
            let record = display.records.get(&window.target.window_id);
            let origin = record.map(|r| r.origin);
            let disposition = record.and_then(|r| r.disposition);
            let of_launched_app = launched.contains(&window.target.pid);
            Item {
                target: window.target,
                app: window.app,
                title: window.title,
                fate: default_fate(origin, disposition, of_launched_app),
                locked: origin == Some(Origin::HandedOver),
                home: record.and_then(|r| r.home.clone()),
                reason: fate_reason(origin, disposition, of_launched_app, display.agent_name()),
            }
        })
        .collect()
}

/// Why the default is what it is, shown under the window's name.
fn fate_reason(
    origin: Option<Origin>,
    disposition: Option<Disposition>,
    of_launched_app: bool,
    agent: &str,
) -> String {
    match (origin, disposition) {
        (Some(Origin::HandedOver), _) => t("You handed this over", "你交出的窗口").into(),
        (_, Some(Disposition::Keep)) => tf!("{agent} suggests keeping it", "{agent} 建议保留"),
        (_, Some(Disposition::Close)) => tf!("{agent} suggests closing it", "{agent} 建议关闭"),
        (Some(Origin::Launched), None) => tf!(
            "Opened by {agent}, no suggestion",
            "{agent} 打开的，未指定去向"
        ),
        (Some(Origin::Placed | Origin::Found), None) => tf!(
            "Moved here by {agent}, no suggestion",
            "{agent} 移进来的，未指定去向"
        ),
        (None, None) => {
            if of_launched_app {
                tf!("From an app {agent} opened", "{agent} 打开的 app 的窗口")
            } else {
                tf!("Not placed here by {agent}", "不是 {agent} 放进来的")
            }
        }
    }
}

/// Whether to ask before dismissing. An empty display nobody uses just goes.
pub fn needs_confirmation(items: &[Item], display: &Display<VirtualDisplay>) -> bool {
    !items.is_empty() || display.is_live()
}

/// Close and return windows. Blocking. Returns the windows still open a
/// moment after their close button was pressed: an app asking whether to
/// save shows a sheet on the window and waits. Removing the display then
/// would make macOS move the question onto the user's screen.
///
/// `main_name` is the main screen's name, read on the main thread.
pub fn execute(items: &[Item], main_name: Option<&str>) -> Vec<WindowTarget> {
    let closing: Vec<WindowTarget> = items
        .iter()
        .filter(|i| i.fate == Fate::Close)
        .map(|i| i.target)
        .filter(|&target| {
            let pressed = ax::close(target).is_ok();
            if !pressed {
                log::warn!("window {} has no close button that works", target.window_id);
            }
            pressed
        })
        .collect();
    for item in items.iter().filter(|i| i.fate == Fate::GiveBack) {
        give_back(item.target, item.home.as_ref(), main_name);
    }
    // Most windows go within a few hundred milliseconds.
    let mut open = closing;
    for _ in 0..15 {
        open.retain(|t| windows::find(t.window_id).is_some());
        if open.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !open.is_empty() {
        log::info!(
            "{} window(s) still open after closing, maybe asking to save",
            open.len()
        );
    }
    open
}

/// A window returns to where it came from when we know that, otherwise to
/// the main display. Blocking.
pub fn give_back(target: WindowTarget, home: Option<&Home>, main_name: Option<&str>) {
    if let Some((stage, home)) = user_stage_home(home) {
        // Debug builds: the claim stays the whole way, and a window that
        // can't go home claimed stays where it is rather than going
        // anywhere AeroSpace or the user's screen could have it.
        if !crate::mac::claims::claim_confirmed(target.window_id, stage) {
            log::warn!(
                "window {} not returned: its claim for the user stand-in stage wasn't published",
                target.window_id
            );
            return;
        }
        if let Err(error) = ax::set_frame(target, home.frame) {
            log::warn!(
                "could not return window {} to the user stand-in stage: {error:?}",
                target.window_id
            );
        }
        return;
    }
    crate::mac::claims::release(target.window_id);
    if let Some(home) = home {
        if let Some(workspace) = &home.workspace
            && let Some(aerospace) = AeroSpace::current()
            && aerospace.move_window(target.window_id, workspace).is_ok()
        {
            return;
        }
        // Its screen may have gone since, as when a monitor is unplugged.
        let screen_there = cg::active_displays()
            .into_iter()
            .any(|d| cg::display_bounds(d).contains(home.frame.mid()));
        if screen_there && ax::set_frame(target, home.frame).is_ok() {
            return;
        }
    }
    if let Err(error) = windows::place(target, cg::main_display(), main_name) {
        log::warn!("could not return window {}: {error}", target.window_id);
    }
}

/// Debug builds: the user stand-in stage and the home when a window's home
/// is on it. The stage is an Agent Display in the claim file, and AeroSpace
/// takes an unclaimed window on one for the user's workspace, onto their
/// real screen; a window going home there stays claimed, as a window on a
/// real screen of the user's stays where it is.
#[cfg(debug_assertions)]
fn user_stage_home(home: Option<&Home>) -> Option<(crate::model::DisplayId, &Home)> {
    let stage = cg::user_stage()?;
    let home = home?;
    stage_home(Some((stage, cg::display_bounds(stage))), Some(home)).map(|stage| (stage, home))
}

#[cfg(not(debug_assertions))]
fn user_stage_home(_: Option<&Home>) -> Option<(crate::model::DisplayId, &Home)> {
    None
}

/// The stage, when the home's middle is on it.
#[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
fn stage_home(
    stage: Option<(crate::model::DisplayId, Rect)>,
    home: Option<&Home>,
) -> Option<crate::model::DisplayId> {
    let (id, bounds) = stage?;
    home.filter(|h| bounds.contains(h.frame.mid())).map(|_| id)
}

/// Where a window is now, for a Handover: its frame and AeroSpace
/// workspace. Blocking.
pub fn home_of(target: WindowTarget, frame: Option<Rect>) -> Option<Home> {
    let frame = frame.or_else(|| windows::find(target.window_id).map(|w| w.bounds))?;
    let workspace =
        AeroSpace::current().and_then(|a| a.workspace_of(target.window_id).ok().flatten());
    Some(Home { frame, workspace })
}

#[cfg(test)]
mod stage_tests {
    use super::*;

    fn home(x: f64, y: f64) -> Home {
        Home {
            frame: Rect::new(x, y, 600.0, 332.0),
            workspace: Some("2".into()),
        }
    }

    #[test]
    fn a_home_on_the_user_stage_returns_there_claimed() {
        let stage = Some((216, Rect::new(4772.0, -1884.0, 1728.0, 1117.0)));
        assert_eq!(stage_home(stage, Some(&home(6132.0, -1804.0))), Some(216));
        // A home on a real screen, none at all, or no stage: the normal way.
        assert_eq!(stage_home(stage, Some(&home(868.0, 38.0))), None);
        assert_eq!(stage_home(stage, None), None);
        assert_eq!(stage_home(None, Some(&home(6132.0, -1804.0))), None);
    }
}
