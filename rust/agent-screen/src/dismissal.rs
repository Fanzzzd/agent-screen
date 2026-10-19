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

/// Close and return windows. Blocking. Returns how many windows refused to
/// close, for example because they ask about unsaved changes; macOS moves
/// those to the main display when the virtual one disappears.
///
/// `main_name` is the main screen's name, read on the main thread.
pub fn execute(items: &[Item], main_name: Option<&str>) -> usize {
    let mut stubborn = 0;
    for item in items.iter().filter(|i| i.fate == Fate::Close) {
        if ax::close(item.target).is_err() {
            stubborn += 1;
        }
    }
    for item in items.iter().filter(|i| i.fate == Fate::GiveBack) {
        give_back(item.target, item.home.as_ref(), main_name);
    }
    if stubborn > 0 {
        log::warn!("{stubborn} window(s) did not close on dismissal");
    }
    stubborn
}

/// A window returns to where it came from when we know that, otherwise to
/// the main display. Blocking.
pub fn give_back(target: WindowTarget, home: Option<&Home>, main_name: Option<&str>) {
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

/// Where a window is now, for a Handover: its frame and AeroSpace
/// workspace. Blocking.
pub fn home_of(target: WindowTarget, frame: Option<Rect>) -> Option<Home> {
    let frame = frame.or_else(|| windows::find(target.window_id).map(|w| w.bounds))?;
    let workspace =
        AeroSpace::current().and_then(|a| a.workspace_of(target.window_id).ok().flatten());
    Some(Home { frame, workspace })
}
