//! Executing approved Input Loans with the user's pointer (ADR 0013).
//!
//! Runs off the UI thread after `Shell::drive_queue` hands out a `Plan`,
//! re-checking display bounds, window geometry, and containment before every
//! real input event.

use crate::input_queue::{Click, Drag, MenuItem, Plan};
use crate::mac::real_input::{self, Outcome};
use crate::mac::{ax, cg, input, screenshot, windows};
use crate::model::{DisplayId, Rect, WindowTarget};
use agent_screen_model::{real_click, real_drag};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn containment_ok(
    display: DisplayId,
    area: Rect,
    ok: &AtomicBool,
    check: impl FnOnce() -> Option<&'static str>,
) -> bool {
    let why = if !ok.load(Ordering::SeqCst) {
        Some("the loan was cancelled")
    } else if cg::display_bounds(display) != area {
        Some("the display geometry changed")
    } else {
        check()
    };
    if let Some(why) = why {
        log::debug!("input queue containment: {why}");
    }
    why.is_none()
}

pub fn execute(display: DisplayId, plan: Plan, ok: &AtomicBool) -> Outcome {
    let area = cg::display_bounds(display);
    match plan {
        Plan::Click(click) => execute_click(display, area, click, ok),
        Plan::MenuItem(item) => execute_menu_item(display, area, item, ok),
        Plan::Drag(drag) => execute_drag(display, area, drag, ok),
    }
}

fn execute_click(display: DisplayId, area: Rect, click: Click, ok: &AtomicBool) -> Outcome {
    let Some(window) = windows::on_display(display)
        .into_iter()
        .find(|w| w.target == click.target)
    else {
        return Outcome::Refused("the window is no longer on the display");
    };
    let frame = window.bounds.offset(area.x, area.y);
    let hit_frame = click.menu.map_or(frame, |(_, mf)| mf);
    if let Some((menu_target, menu_frame)) = click.menu
        && cg::window_bounds(menu_target.window_id) != Some(menu_frame)
    {
        return Outcome::Refused("the menu closed before the click");
    }
    if area != click.area
        || frame != click.frame
        || !real_click::contained(click.at, area, hit_frame)
    {
        return Outcome::Refused("the click is outside its authorized window or display");
    }
    // Before the window comes forward, which can redraw it: the point must
    // still show what the agent aimed at. Read only when the request named an
    // element, and within 300 ms: the user's pointer waits.
    let element_kept = |when: &str| {
        if click.element.is_none() {
            return true;
        }
        let (element, _) = ax::target_at(click.ax_pid, click.at, Duration::from_millis(300), false);
        let same = real_click::same_element(click.element.as_deref(), element.as_ref());
        if !same {
            log::info!(
                "input queue: the element at the click changed {when}: {:?} -> {element:?}",
                click.element
            );
        }
        same
    };
    if !element_kept("while the request waited") {
        return Outcome::Refused(real_click::CONTENT_CHANGED);
    }
    match screenshot::samples(display, click.region) {
        Ok(now) if real_click::looks_same(&click.look, &now) => {}
        Ok(now) => {
            // Diagnostics tell a false refusal from a real change by how much
            // changed.
            let (changed, all) = real_click::changed_samples(&click.look, &now).unwrap_or((0, 0));
            log::info!(
                "input queue: the region {:?} at the click changed: {changed} of {all} samples ({:.1}%, the limit is {}%)",
                click.region,
                changed as f64 * 100.0 / all.max(1) as f64,
                real_click::LOOK_PERCENT
            );
            return Outcome::Refused(real_click::CONTENT_CHANGED);
        }
        Err(why) => {
            log::info!("input queue: no picture before the click: {why}");
            return Outcome::Refused(
                "what is under the point could not be captured before the click",
            );
        }
    }
    let menu_target = click.menu.map(|(m, _)| m);
    let menu_frame = click.menu.map(|(_, mf)| mf);
    real_input::click_at(
        click.target,
        click.at,
        click.click,
        objc2_core_graphics::CGEventFlags(click.flags),
        |at| {
            // As for a drag: a moved or covered window must not get the click.
            containment_ok(display, area, ok, || {
                if !area.contains(at) {
                    Some("the display geometry changed")
                } else if cg::window_bounds(click.target.window_id) != Some(frame) {
                    Some("the window geometry changed")
                } else if let Some((mt, mf)) = menu_target.zip(menu_frame)
                    && cg::window_bounds(mt.window_id) != Some(mf)
                {
                    Some("the menu closed or moved")
                } else if !input::real_click_still_under(at, click.target, menu_target) {
                    Some("the window was covered or left the screen")
                } else {
                    None
                }
            })
        },
        || element_kept("as its window came forward"),
    )
}

fn execute_menu_item(display: DisplayId, area: Rect, item: MenuItem, ok: &AtomicBool) -> Outcome {
    if area != item.area
        || !windows::on_display(display)
            .iter()
            .any(|w| w.target == item.target)
    {
        return Outcome::Refused("the window is no longer on its Agent Display");
    }
    real_input::pick_menu_item(item.target, area, &item.path, |at| {
        containment_ok(display, area, ok, || {
            (!area.contains(at)).then_some("the display geometry changed")
        })
    })
}

fn execute_drag(display: DisplayId, area: Rect, plan: Drag, ok: &AtomicBool) -> Outcome {
    // A drag may go from one window of its display to another (ADR 0013,
    // "Drags between windows").
    let on_display = windows::on_display(display);
    let frame_of = |target: WindowTarget| {
        on_display
            .iter()
            .find(|w| w.target == target)
            .map(|w| w.bounds.offset(area.x, area.y))
    };
    let Some(frame) = frame_of(plan.target) else {
        return Outcome::Refused("the window is no longer on the display");
    };
    let Some(drop_frame) = frame_of(plan.drop) else {
        return Outcome::Refused("the drop window is no longer on the display");
    };
    if area != plan.area
        || frame != plan.frame
        || !frame.contains(plan.from)
        || ![plan.from, plan.to].iter().all(|p| area.contains(*p))
    {
        return Outcome::Refused("the drag is outside its authorized window or display");
    }
    if drop_frame != plan.drop_frame
        || cg::window_bounds(plan.drop.window_id) != Some(plan.drop_frame)
        || !drop_frame.contains(plan.to)
    {
        return Outcome::Refused(
            "the drop window moved or changed size, so the drag would end elsewhere",
        );
    }
    let path = real_drag::Path {
        area,
        from: plan.from,
        to: plan.to,
        source: plan.target,
        drop: plan.drop,
    };
    let windows: Vec<WindowTarget> = on_display.iter().map(|w| w.target).collect();
    let sight = input::DragSight::default();
    real_input::drag(
        plan.target,
        plan.drop,
        drop_frame,
        plan.from,
        plan.to,
        objc2_core_graphics::CGEventFlags(plan.flags),
        |at| {
            // Recheck geometry and the window under the point before every
            // event. A moved or covered window must not receive input at its
            // old coordinates. One read of the on-screen windows gives both
            // frames and what lies under every point the check looks at.
            let scene = sight.look(plan.target, plan.drop, area);
            containment_ok(display, area, ok, || {
                if scene.frame(plan.target.window_id) != Some(frame) {
                    Some("the window geometry changed")
                } else if scene.frame(plan.drop.window_id) != Some(drop_frame) {
                    Some("the drop window's geometry changed")
                } else {
                    let under = |p| scene.under(p);
                    real_drag::refusal(&path, at, under(at), &windows).or_else(|| {
                        // At the press, the end must lie uncovered too: the
                        // source coming forward may cover it.
                        (at == plan.from && plan.to != plan.from)
                            .then(|| real_drag::refusal(&path, plan.to, under(plan.to), &windows))
                            .flatten()
                    })
                }
            })
        },
    )
}
