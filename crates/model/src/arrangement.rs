//! Where Agent Screen's displays go among the user's, in CoreGraphics
//! global coordinates (top-left origin, y down).
//!
//! The user's displays never move. Agent Screen's own sit in a compact grid
//! off the top-right corner of the rightmost user display: the first one's
//! bottom-left corner meets that corner, and each further row stacks on the
//! one below. macOS won't leave a display floating unattached, so the grid
//! grows from a corner a user display actually has, not from the corner of
//! their union, which is empty when the screens' tops don't line up (#155).
//! A row is as wide as the user's displays, so the whole arrangement stays
//! close to square and System Settings shows the user's displays at a
//! usable size. Unparked displays go in serial order; displays already
//! parked safely stay put so windows on them keep their session (#204).
//!
//! An Agent Display is safe where it overlaps no user display and touches
//! one at most at a single corner point, never along an edge, so the real
//! cursor can't slide onto it (#132). It must also sit right of every user
//! display and no lower than the top of the rightmost one: never between
//! them, and not in a bottom corner, where AeroSpace hides the windows of
//! other workspaces and a display would show them to agents.

use crate::model::{DisplayId, Rect, WindowTarget};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    /// One of the user's displays.
    User,
    /// A display Agent Screen doesn't place but must keep clear of: another
    /// instance's, or AeroSpace's closet.
    Fixed,
    /// Agent Screen's own, placed in this order.
    Ours { order: u64 },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Screen {
    pub id: u32,
    pub rect: Rect,
    pub role: Role,
}

/// The union of the user's displays.
pub fn user_bounds(screens: &[Screen]) -> Option<Rect> {
    screens
        .iter()
        .filter(|s| s.role == Role::User)
        .map(|s| s.rect)
        .reduce(|a, b| a.union(&b))
}

/// The corner Agent Screen's displays grow from: the top-right corner of
/// the rightmost user display, the topmost one when several end at the
/// same x. Every user display ends at or left of it, and none of those
/// ending there reaches above it.
fn corner(screens: &[Screen]) -> Option<(f64, f64)> {
    screens
        .iter()
        .filter(|s| s.role == Role::User)
        .map(|s| (s.rect.max_x(), s.rect.y))
        .reduce(|a, b| {
            if b.0 > a.0 || (b.0 == a.0 && b.1 < a.1) {
                b
            } else {
                a
            }
        })
}

/// Where each of Agent Screen's displays goes, as origins. Empty when the
/// user has no display of their own to arrange around.
///
/// Displays of ours that are already `out_of_the_way`, overlap neither a fixed
/// display nor an earlier display of ours, and stay attached to the user's
/// screens keep their origins: moving a live display in CoreGraphics moves only
/// its viewport while WindowServer leaves windows at their global coordinates,
/// which would hand one display's windows to whichever display took its old
/// slot (#204). If a vacated slot would leave a pinned display floating, macOS
/// would refuse the layout and move displays itself, so `plan` unpins one
/// disconnected display at a time from the far end until every display is
/// connected to a user screen.
pub fn plan(screens: &[Screen]) -> Vec<(u32, (f64, f64))> {
    let (Some(user), Some((right, top))) = (user_bounds(screens), corner(screens)) else {
        return Vec::new();
    };
    // Keep clear of fixed displays above the corner: another instance's
    // grid. AeroSpace's closet hangs below, out of the way.
    let fixed: Vec<Rect> = screens
        .iter()
        .filter(|s| s.role == Role::Fixed && s.rect.y < top && s.rect.max_x() > right)
        .map(|s| s.rect)
        .collect();
    let mut start = right;
    while let Some(end) = fixed
        .iter()
        .filter(|f| f.x <= start && f.max_x() > start)
        .map(|f| f.max_x())
        .reduce(f64::max)
    {
        start = end;
    }
    let row_limit = start + user.width;
    let mut ours: Vec<(u64, u32, Rect)> = screens
        .iter()
        .filter_map(|s| match s.role {
            Role::Ours { order } => Some((order, s.id, s.rect)),
            _ => None,
        })
        .collect();
    ours.sort_by_key(|&(order, id, _)| (order, id));

    let mut pinned_allowed: Vec<u32> = ours.iter().map(|&(_, id, _)| id).collect();
    loop {
        let placed = place_grid(
            screens,
            start,
            top,
            row_limit,
            &fixed,
            &ours,
            &pinned_allowed,
        );
        let planned_screens = with_origins(screens, &placed);
        let reached = reachable_from_user(&planned_screens);
        let drop_pin = pinned_allowed
            .iter()
            .copied()
            .filter(|id| !reached.contains(id))
            .max_by(|&a, &b| {
                let ra = ours.iter().find(|&&(_, id, _)| id == a).unwrap().2;
                let rb = ours.iter().find(|&&(_, id, _)| id == b).unwrap().2;
                (-ra.y, ra.x)
                    .partial_cmp(&(-rb.y, rb.x))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        match drop_pin {
            Some(id) => pinned_allowed.retain(|&p| p != id),
            None => return placed,
        }
    }
}

fn place_grid(
    screens: &[Screen],
    start: f64,
    top: f64,
    row_limit: f64,
    fixed: &[Rect],
    ours: &[(u64, u32, Rect)],
    pinned_allowed: &[u32],
) -> Vec<(u32, (f64, f64))> {
    let mut obstacles = fixed.to_vec();
    let mut placed: Vec<(u64, u32, (f64, f64))> = Vec::with_capacity(ours.len());
    let mut unparked: Vec<(u64, u32, Rect)> = Vec::new();
    for &(order, id, rect) in ours {
        if pinned_allowed.contains(&id)
            && out_of_the_way(&rect, screens)
            && obstacles.iter().all(|o| !rect.intersects(o))
        {
            obstacles.push(rect);
            placed.push((order, id, (rect.x, rect.y)));
        } else {
            unparked.push((order, id, rect));
        }
    }
    let (mut x, mut bottom, mut row_top) = (start, top, top);
    for (order, id, rect) in unparked {
        loop {
            if x > start && x + rect.width > row_limit {
                x = start;
                bottom = row_top;
            }
            let y = bottom - rect.height;
            let candidate = Rect::new(x, y, rect.width, rect.height);
            if let Some(hit) = obstacles
                .iter()
                .filter(|o| candidate.intersects(o))
                .max_by(|a, b| a.max_x().total_cmp(&b.max_x()))
            {
                row_top = row_top.min(hit.y);
                x = hit.max_x();
                continue;
            }
            obstacles.push(candidate);
            placed.push((order, id, (x, y)));
            x += rect.width;
            row_top = row_top.min(y);
            break;
        }
    }
    placed.sort_by_key(|&(order, id, _)| (order, id));
    placed.into_iter().map(|(_, id, at)| (id, at)).collect()
}

fn with_origins(screens: &[Screen], origins: &[(u32, (f64, f64))]) -> Vec<Screen> {
    screens
        .iter()
        .map(|&s| {
            if let Some(&(_, (x, y))) = origins.iter().find(|&&(id, _)| id == s.id) {
                Screen {
                    rect: Rect::new(x, y, s.rect.width, s.rect.height),
                    ..s
                }
            } else {
                s
            }
        })
        .collect()
}

/// Whether two display rectangles touch at least at a corner point.
fn touches(a: &Rect, b: &Rect) -> bool {
    a.x <= b.max_x() && b.x <= a.max_x() && a.y <= b.max_y() && b.y <= a.max_y()
}

fn reachable_from_user(screens: &[Screen]) -> Vec<u32> {
    let mut visited = vec![false; screens.len()];
    let mut stack = Vec::new();
    for (i, s) in screens.iter().enumerate() {
        if s.role == Role::User {
            visited[i] = true;
            stack.push(i);
        }
    }
    while let Some(cur) = stack.pop() {
        for (j, other) in screens.iter().enumerate() {
            if !visited[j] && touches(&screens[cur].rect, &other.rect) {
                visited[j] = true;
                stack.push(j);
            }
        }
    }
    screens
        .iter()
        .enumerate()
        .filter(|&(i, _)| visited[i])
        .map(|(_, s)| s.id)
        .collect()
}

/// Whether every `Role::Ours` display in `screens` reaches a `Role::User`
/// display through displays that touch at an edge or corner. macOS refuses an
/// arrangement with a floating display and moves displays itself without
/// moving their windows.
pub fn connected(screens: &[Screen]) -> bool {
    let has_user = screens.iter().any(|s| s.role == Role::User);
    if !has_user {
        return true;
    }
    let reached = reachable_from_user(screens);
    screens
        .iter()
        .filter(|s| matches!(s.role, Role::Ours { .. }))
        .all(|s| reached.contains(&s.id))
}

/// Reconciles geometric display placement with explicit placement/handover
/// records (#204). While a window sits on any virtual display (`on_virtual`),
/// its active `recorded_owner` wins over transient viewport shifts; once the
/// window returns to the user's physical screen (`!on_virtual`), geometry wins
/// so strays and user-returned windows behave normally.
pub fn resolve_display_owner(
    geom_display: Option<DisplayId>,
    on_virtual: bool,
    recorded_owner: Option<DisplayId>,
) -> Option<DisplayId> {
    if !on_virtual {
        return None;
    }
    recorded_owner.or(geom_display)
}

/// Whether a window with global `bounds` belongs to `area` when the other
/// online displays cover `others`: it intersects `area`, and no other display
/// contains its center instead.
pub fn window_belongs_to_area(bounds: &Rect, area: &Rect, others: &[Rect]) -> bool {
    let center = bounds.mid();
    bounds.intersects(area) && (area.contains(center) || !others.iter().any(|o| o.contains(center)))
}

/// Single source of truth for which virtual Agent Display owns a window
/// (#204, ADR 0018), plus the display rect its local coordinates are relative
/// to. `displays` lists every online display as `(id, area, is_agent_virtual)`.
///
/// - A window whose geometry places it on a user screen (`!is_agent_virtual`)
///   belongs to no Agent Display (`None`).
/// - While a window sits geometrically on any virtual Agent Display
///   (`is_agent_virtual`), an active `recorded_owner` wins over geometry across
///   every caller.
pub fn effective_agent_display(
    bounds: &Rect,
    displays: &[(DisplayId, Rect, bool)],
    recorded_owner: Option<DisplayId>,
) -> Option<(DisplayId, Rect)> {
    let center = bounds.mid();
    let mut geom = displays
        .iter()
        .copied()
        .find(|&(_, area, _)| area.contains(center) && bounds.intersects(&area));
    if geom.is_none() {
        for &(id, area, is_agent) in displays {
            let others: Vec<Rect> = displays
                .iter()
                .filter(|&&(oid, _, _)| oid != id)
                .map(|&(_, r, _)| r)
                .collect();
            if window_belongs_to_area(bounds, &area, &others) {
                geom = Some((id, area, is_agent));
                break;
            }
        }
    }
    let (geom_id, geom_area, on_virtual) = geom?;
    let owner = resolve_display_owner(on_virtual.then_some(geom_id), on_virtual, recorded_owner)?;
    let base = if owner == geom_id {
        geom_area
    } else {
        displays
            .iter()
            .find(|&&(id, _, _)| id == owner)
            .map_or(geom_area, |&(_, area, _)| area)
    };
    Some((owner, base))
}

pub type WindowShift = (WindowTarget, Rect);
pub type DisplayMove = (DisplayId, Rect, Rect);

pub const MAX_PENDING_SHIFT_FAILURES: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingShift {
    target: WindowTarget,
    display: DisplayId,
    failed_bounds: Rect,
    dest: Rect,
    failures: u32,
}

/// Serializes window-shift planning and tracks each adopted display's verified
/// `parked_rects` baseline with a monotonic generation counter (H1).
/// Overlapping `ScreensChanged` notifications coalesce into a single follow-up
/// pass (`rerun`) that starts only after in-flight AX moves finish, so
/// unrecorded windows never shift twice and stale runs cannot overwrite newer
/// baselines. Failed AX moves stay in `pending_shifts` (rebased across any
/// further display moves) while the window stays at its failed frame, and
/// expire after [`MAX_PENDING_SHIFT_FAILURES`] failures (`L1`, `L2`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReparkState {
    parked_rects: BTreeMap<DisplayId, Rect>,
    pending_shifts: BTreeMap<u32, PendingShift>,
    planned_in_pass: BTreeMap<u32, (DisplayId, Rect, Rect)>,
    in_flight: bool,
    rerun: bool,
    generation: u64,
}

impl ReparkState {
    pub fn note_adopted(&mut self, id: DisplayId, rect: Rect) {
        if rect.width > 0.0 && rect.height > 0.0 {
            self.parked_rects.insert(id, rect);
        }
    }

    pub fn remove_display(&mut self, id: DisplayId) {
        self.parked_rects.remove(&id);
        self.pending_shifts.retain(|_, ps| ps.display != id);
        self.planned_in_pass.retain(|_, (d, _, _)| *d != id);
    }

    #[cfg(test)]
    fn parked_rect(&self, id: DisplayId) -> Option<Rect> {
        self.parked_rects.get(&id).copied()
    }

    pub fn has_pending_retries(&self, adopted: &[DisplayId]) -> bool {
        self.pending_shifts
            .values()
            .any(|ps| adopted.contains(&ps.display))
    }

    pub fn max_pending_failures(&self, adopted: &[DisplayId]) -> u32 {
        self.pending_shifts
            .values()
            .filter(|ps| adopted.contains(&ps.display))
            .map(|ps| ps.failures)
            .max()
            .unwrap_or(0)
    }

    /// Start a serialized shift pass for `adopted` displays. If a pass is
    /// already running, marks `rerun` and returns `None`.
    pub fn begin(&mut self, adopted: &[DisplayId]) -> Option<(u64, Vec<(DisplayId, Rect)>)> {
        if self.in_flight {
            self.rerun = true;
            return None;
        }
        self.in_flight = true;
        self.rerun = false;
        self.generation += 1;
        self.planned_in_pass.clear();
        let snapshot = adopted
            .iter()
            .filter_map(|&id| self.parked_rects.get(&id).copied().map(|r| (id, r)))
            .collect();
        Some((self.generation, snapshot))
    }

    /// Compute window shifts for `generation` from the current `parked_rects`
    /// baseline to `settled`, advancing `parked_rects` atomically so a later
    /// pass sees the updated origins, and including any previously failed
    /// shifts whose window has not yet reached its destination.
    pub fn plan_shifts(
        &mut self,
        generation: u64,
        adopted: &[DisplayId],
        settled: &[(DisplayId, Rect)],
        stationary: &[Rect],
        windows: &[(WindowTarget, Rect, Option<DisplayId>)],
    ) -> (Vec<WindowShift>, Vec<DisplayMove>) {
        if !self.in_flight || generation != self.generation {
            return (Vec::new(), Vec::new());
        }
        let mut moved = Vec::new();
        for &(id, now) in settled {
            if !adopted.contains(&id) || now.width <= 0.0 || now.height <= 0.0 {
                continue;
            }
            if let Some(&old) = self.parked_rects.get(&id) {
                moved.push((id, old, now));
            }
        }
        let active_moves: Vec<DisplayMove> = moved
            .iter()
            .copied()
            .filter(|&(_, old, now)| (now.x - old.x).abs() > 0.5 || (now.y - old.y).abs() > 0.5)
            .collect();

        // Rebase any pending failed shifts whose display moved again.
        for &(id, old, now) in &active_moves {
            let resized =
                (now.width - old.width).abs() > 0.5 || (now.height - old.height).abs() > 0.5;
            for ps in self.pending_shifts.values_mut() {
                if ps.display == id {
                    let shifted = ps.dest.offset(now.x - old.x, now.y - old.y);
                    ps.dest = if resized {
                        shifted.kept_inside(&now)
                    } else {
                        shifted
                    };
                }
            }
        }

        let owned_shifts = window_shifts_with_owner(&moved, stationary, windows);
        for &(id, now) in settled {
            if adopted.contains(&id) && now.width > 0.0 && now.height > 0.0 {
                self.parked_rects.insert(id, now);
            }
        }

        self.planned_in_pass.clear();
        let mut shifts = Vec::new();
        for (target, owner, dest) in owned_shifts {
            let cur_bounds = windows
                .iter()
                .find(|&&(t, _, _)| t == target)
                .map_or(dest, |&(_, b, _)| b);
            self.planned_in_pass
                .insert(target.window_id, (owner, cur_bounds, dest));
            shifts.push((target, dest));
        }

        // Retry previously failed shifts only while the window still sits at
        // the frame it had when the shift failed (neither moved by the user /
        // `place_window` since, nor already at `dest`, `L1`).
        let same_frame = |a: &Rect, b: &Rect| {
            (a.x - b.x).abs() <= 2.0
                && (a.y - b.y).abs() <= 2.0
                && (a.width - b.width).abs() <= 4.0
                && (a.height - b.height).abs() <= 4.0
        };
        self.pending_shifts.retain(|&wid, ps| {
            if !adopted.contains(&ps.display) {
                return false;
            }
            let Some(&(_, cur_bounds, rec_owner)) =
                windows.iter().find(|&&(t, _, _)| t.window_id == wid)
            else {
                return false;
            };
            if rec_owner.is_some_and(|o| o != ps.display) {
                return false;
            }
            let arrived =
                (cur_bounds.x - ps.dest.x).abs() <= 2.0 && (cur_bounds.y - ps.dest.y).abs() <= 2.0;
            !arrived && same_frame(&cur_bounds, &ps.failed_bounds)
        });
        for ps in self.pending_shifts.values() {
            if let std::collections::btree_map::Entry::Vacant(e) =
                self.planned_in_pass.entry(ps.target.window_id)
            {
                e.insert((ps.display, ps.failed_bounds, ps.dest));
                shifts.push((ps.target, ps.dest));
            }
        }

        (shifts, active_moves)
    }

    /// Complete `generation` when every planned shift succeeded.
    pub fn finish(&mut self, generation: u64) -> bool {
        self.finish_with_failures(generation, &[]).0
    }

    /// Complete `generation`, recording `failed` targets so the next pass
    /// retries them up to [`MAX_PENDING_SHIFT_FAILURES`] failures. Returns
    /// `(rerun, first_failures)` where `first_failures` lists targets failing
    /// for the first time.
    pub fn finish_with_failures(
        &mut self,
        generation: u64,
        failed: &[WindowTarget],
    ) -> (bool, Vec<WindowTarget>) {
        if !self.in_flight || generation != self.generation {
            return (false, Vec::new());
        }
        let planned = std::mem::take(&mut self.planned_in_pass);
        for &wid in planned.keys() {
            if !failed.iter().any(|t| t.window_id == wid) {
                self.pending_shifts.remove(&wid);
            }
        }
        let mut first_failures = Vec::new();
        for &target in failed {
            let wid = target.window_id;
            let resolved = planned.get(&wid).copied().or_else(|| {
                self.pending_shifts
                    .get(&wid)
                    .map(|ps| (ps.display, ps.failed_bounds, ps.dest))
            });
            let Some((display, failed_bounds, dest)) = resolved else {
                continue;
            };
            let prev = self.pending_shifts.get(&wid).map_or(0, |ps| ps.failures);
            let failures = prev.saturating_add(1);
            if failures >= MAX_PENDING_SHIFT_FAILURES {
                self.pending_shifts.remove(&wid);
                continue;
            }
            self.pending_shifts.insert(
                wid,
                PendingShift {
                    target,
                    display,
                    failed_bounds,
                    dest,
                    failures,
                },
            );
            if failures == 1 {
                first_failures.push(target);
            }
        }
        self.in_flight = false;
        (std::mem::take(&mut self.rerun), first_failures)
    }
}

fn window_shifts_with_owner(
    moved: &[(DisplayId, Rect, Rect)],
    stationary_areas: &[Rect],
    windows: &[(WindowTarget, Rect, Option<DisplayId>)],
) -> Vec<(WindowTarget, DisplayId, Rect)> {
    let active_moves: Vec<(DisplayId, Rect, Rect)> = moved
        .iter()
        .copied()
        .filter(|&(_, old, new)| {
            new.width > 0.0
                && new.height > 0.0
                && ((new.x - old.x).abs() > 0.5 || (new.y - old.y).abs() > 0.5)
        })
        .collect();
    if active_moves.is_empty() {
        return Vec::new();
    }
    let all_old: Vec<(Option<DisplayId>, Rect)> = moved
        .iter()
        .map(|&(id, old, _)| (Some(id), old))
        .chain(stationary_areas.iter().map(|&r| (None, r)))
        .collect();
    let mut shifts: Vec<(WindowTarget, DisplayId, Rect)> = Vec::new();
    for &(id, old, new) in &active_moves {
        let others: Vec<Rect> = all_old
            .iter()
            .filter(|&&(oid, _)| oid != Some(id))
            .map(|&(_, r)| r)
            .collect();
        let resized = (new.width - old.width).abs() > 0.5 || (new.height - old.height).abs() > 0.5;
        for &(target, bounds, recorded_owner) in windows {
            if shifts.iter().any(|&(t, _, _)| t == target) {
                continue;
            }
            let belongs = match recorded_owner {
                Some(owner) => owner == id && bounds.intersects(&old),
                None => window_belongs_to_area(&bounds, &old, &others),
            };
            if belongs {
                let shifted = bounds.offset(new.x - old.x, new.y - old.y);
                let next = if resized {
                    shifted.kept_inside(&new)
                } else {
                    shifted
                };
                shifts.push((target, id, next));
            }
        }
    }
    shifts
}

/// A parking attempt succeeds once display `id`, one of Agent Screen's,
/// appears in the current arrangement out of the cursor's way, wherever
/// macOS put it. macOS may move a display off its planned origin, as it
/// attaches one it won't leave floating; only the safety rule counts. An
/// absent display or user screen is unverified, not success.
pub fn parked_safely(id: u32, screens: &[Screen]) -> bool {
    screens
        .iter()
        .find(|s| s.id == id && matches!(s.role, Role::Ours { .. }))
        .is_some_and(|s| out_of_the_way(&s.rect, screens))
}

/// Whether an Agent Display at `rect` is out of the cursor's way: right of
/// every user display, no lower than the top of the rightmost one, over
/// none of them, and touching each at most at a corner point. One the user
/// dragged elsewhere is not. False without a user display.
pub fn out_of_the_way(rect: &Rect, screens: &[Screen]) -> bool {
    let Some((right, top)) = corner(screens) else {
        return false;
    };
    rect.x >= right
        && rect.max_y() <= top
        && screens
            .iter()
            .filter(|s| s.role == Role::User)
            .all(|s| !rect.intersects(&s.rect) && !shares_edge(rect, &s.rect))
}

/// Whether `a` and `b` touch along an edge segment of positive length,
/// where the pointer could cross from one to the other. Meeting at a
/// single corner point is not sharing an edge.
fn shares_edge(a: &Rect, b: &Rect) -> bool {
    let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| a1.min(b1) > a0.max(b0);
    ((a.x == b.max_x() || a.max_x() == b.x) && overlap(a.y, a.max_y(), b.y, b.max_y()))
        || ((a.y == b.max_y() || a.max_y() == b.y) && overlap(a.x, a.max_x(), b.x, b.max_x()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u32, x: f64, y: f64, w: f64, h: f64) -> Screen {
        Screen {
            id,
            rect: Rect::new(x, y, w, h),
            role: Role::User,
        }
    }
    fn ours(id: u32, order: u64, w: f64, h: f64) -> Screen {
        // Wherever macOS put it.
        Screen {
            id,
            rect: Rect::new(0., 1117., w, h),
            role: Role::Ours { order },
        }
    }
    fn rects(screens: &[Screen], plan: &[(u32, (f64, f64))]) -> Vec<Rect> {
        plan.iter()
            .map(|&(id, (x, y))| {
                let r = screens.iter().find(|s| s.id == id).unwrap().rect;
                Rect::new(x, y, r.width, r.height)
            })
            .collect()
    }
    /// This Mac: the built-in display with an external one above it.
    fn desk() -> Vec<Screen> {
        vec![
            user(1, 0., 0., 1728., 1117.),
            user(2, 1003., -1080., 1920., 1080.),
        ]
    }

    #[test]
    fn one_display_touches_the_users_only_at_the_top_right_corner() {
        let mut s = desk();
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (2923., -1880.))]);
    }

    #[test]
    fn displays_fill_rows_as_wide_as_the_users_and_stack_upward() {
        let mut s = desk();
        for i in 0..5 {
            s.push(ours(10 + i, i as u64, 1280., 800.));
        }
        let placed = rects(&s, &plan(&s));
        // Two to a row on a desk 2923 wide: three rows, not a strip 6400 wide.
        assert_eq!(
            placed.iter().map(|r| (r.x, r.y)).collect::<Vec<_>>(),
            vec![
                (2923., -1880.),
                (4203., -1880.),
                (2923., -2680.),
                (4203., -2680.),
                (2923., -3480.),
            ]
        );
        for (i, r) in placed.iter().enumerate() {
            assert!(out_of_the_way(r, &s));
            assert!(s[..2].iter().all(|u| !r.intersects(&u.rect)));
            assert!(placed[i + 1..].iter().all(|o| !r.intersects(o)));
        }
    }

    #[test]
    fn the_same_set_lands_in_the_same_places_whatever_the_order_listed() {
        let mut a = desk();
        a.extend([
            ours(10, 3, 1280., 800.),
            ours(11, 1, 1728., 1117.),
            ours(12, 2, 1280., 800.),
        ]);
        let mut b = a.clone();
        b.reverse();
        let (mut pa, mut pb) = (plan(&a), plan(&b));
        pa.sort_by_key(|p| p.0);
        pb.sort_by_key(|p| p.0);
        assert_eq!(pa, pb);
        // Serial order: 11 first, at the corner.
        assert_eq!(plan(&a)[0], (11, (2923., -2197.)));
    }

    #[test]
    fn a_display_wider_than_a_row_still_gets_a_row_of_its_own() {
        let mut s = vec![user(1, 0., 0., 1000., 800.)];
        s.extend([ours(10, 1, 1728., 1117.), ours(11, 2, 1728., 1117.)]);
        assert_eq!(plan(&s), vec![(10, (1000., -1117.)), (11, (1000., -2234.))]);
    }

    #[test]
    fn another_instances_displays_are_kept_clear_of_and_the_closet_is_not_in_the_way() {
        let mut s = desk();
        s.push(Screen {
            id: 50,
            rect: Rect::new(2923., -1880., 1280., 800.),
            role: Role::Fixed,
        });
        // AeroSpace's closet, off the built-in display's bottom-right corner.
        s.push(Screen {
            id: 82,
            rect: Rect::new(1728., 1117., 1728., 1117.),
            role: Role::Fixed,
        });
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (4203., -1880.))]);
    }

    #[test]
    fn nothing_moves_without_a_user_display_to_arrange_around() {
        assert!(plan(&[ours(10, 1, 1280., 800.)]).is_empty());
    }

    #[test]
    fn parking_requires_the_target_and_user_to_be_enumerated() {
        assert!(!parked_safely(10, &desk()));
        assert!(!parked_safely(10, &[ours(10, 1, 1280., 800.)]));
        assert!(!parked_safely(1, &desk()));
    }

    fn at(id: u32, x: f64, y: f64, w: f64, h: f64) -> Screen {
        Screen {
            id,
            rect: Rect::new(x, y, w, h),
            role: Role::Ours {
                order: u64::from(id),
            },
        }
    }
    /// Issue #155: a built-in display right of the main one, 190 lower.
    fn unaligned() -> Vec<Screen> {
        vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., 190., 1728., 1117.),
        ]
    }
    /// Every planned display is safe, connected to a user display, and
    /// overlaps none of the others.
    fn assert_plan_safe(s: &[Screen]) {
        let planned = plan(s);
        let placed = rects(s, &planned);
        assert!(!placed.is_empty());
        assert!(
            connected(&with_origins(s, &planned)),
            "disconnected plan {planned:?} for {s:?}"
        );
        for (i, r) in placed.iter().enumerate() {
            assert!(out_of_the_way(r, s), "{r:?} is in the way of {s:?}");
            assert!(placed[i + 1..].iter().all(|o| !r.intersects(o)));
        }
    }

    #[test]
    fn where_macos_puts_a_display_on_unaligned_screens_is_parked_and_planned() {
        // macOS attaches the display to the built-in's top-right corner, 190
        // below the union's: displays.log in #155 had it at 4288,-610 for
        // 1280x800 and 4288,-530 for 1280x720, and both were released.
        for (h, y) in [(800., -610.), (720., -530.)] {
            let mut s = unaligned();
            s.push(at(10, 4288., y, 1280., h));
            assert!(parked_safely(10, &s), "{h} tall at 4288,{y}");
            assert_eq!(plan(&s), vec![(10, (4288., y))]);
        }
    }

    #[test]
    fn displays_on_unaligned_screens_stack_up_from_the_rightmost_corner() {
        let mut s = unaligned();
        for i in 0..5 {
            s.push(ours(10 + i, i as u64, 1280., 800.));
        }
        assert_eq!(
            plan(&s).iter().map(|p| p.1).collect::<Vec<_>>(),
            vec![
                (4288., -610.),
                (5568., -610.),
                (6848., -610.),
                (4288., -1410.),
                (5568., -1410.),
            ]
        );
        assert_plan_safe(&s);
    }

    #[test]
    fn aligned_tops_keep_the_union_corner() {
        let mut s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., 0., 1728., 1117.),
        ];
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (4288., -800.))]);
        assert_plan_safe(&s);
    }

    #[test]
    fn the_corner_follows_the_rightmost_screen_taller_shorter_or_lower() {
        // Taller, reaching above the main display: the union's corner.
        let mut s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., -300., 1080., 1920.),
        ];
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (3640., -1100.))]);
        assert_plan_safe(&s);
        // Shorter and lower than the main display's bottom.
        let mut s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., 1000., 1728., 1117.),
        ];
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (4288., 200.))]);
        assert_plan_safe(&s);
    }

    #[test]
    fn screens_left_of_main_and_vertical_stacks_use_the_rightmost_top_corner() {
        // A screen left of the main one, lower: the main one is rightmost.
        let mut s = vec![
            user(1, -1920., 300., 1920., 1080.),
            user(2, 0., 0., 2560., 1440.),
        ];
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (2560., -800.))]);
        assert_plan_safe(&s);
        // A narrower screen above: the one below is rightmost.
        let mut s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 320., -1080., 1920., 1080.),
        ];
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (2560., -800.))]);
        assert_plan_safe(&s);
        // Two of the same width, flush right: the top one's corner.
        let mut s = vec![
            user(1, 0., 0., 1728., 1117.),
            user(2, 0., -1080., 1728., 1080.),
        ];
        s.push(ours(10, 1, 1280., 800.));
        assert_eq!(plan(&s), vec![(10, (1728., -1880.))]);
        assert_plan_safe(&s);
    }

    #[test]
    fn three_screens_use_the_rightmost_ones_corner() {
        let mut s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., -200., 1920., 1080.),
            user(3, 4480., 300., 1728., 1117.),
        ];
        for i in 0..4 {
            s.push(ours(10 + i, i as u64, 1280., 800.));
        }
        assert_eq!(plan(&s)[0], (10, (6208., -500.)));
        assert_plan_safe(&s);
    }

    #[test]
    fn a_display_sharing_an_edge_overlapping_or_between_screens_is_refused() {
        let s = unaligned();
        for (x, y) in [
            // Beside the main display's left edge, where #132's display stayed.
            (-1280., 0.),
            // Down the built-in's right edge, 210 of it shared.
            (4288., -400.),
            // On the built-in's top edge, between it and the main display.
            (2560., -610.),
            // Over the built-in.
            (4000., -500.),
            // Between the screens, meeting the main display at a corner only.
            (2560., -800.),
            // The built-in's bottom-right corner.
            (4288., 1307.),
        ] {
            let r = Rect::new(x, y, 1280., 800.);
            assert!(!out_of_the_way(&r, &s), "{r:?}");
            let mut placed = s.clone();
            placed.push(at(10, x, y, 1280., 800.));
            assert!(!parked_safely(10, &placed), "{r:?}");
        }
        // One pixel more and it shares no edge: macOS's corner placement.
        assert!(out_of_the_way(&Rect::new(4288., -610., 1280., 800.), &s));
        assert!(!out_of_the_way(&Rect::new(4288., -609., 1280., 800.), &s));
    }

    #[test]
    fn a_display_dragged_next_to_the_users_is_in_the_way() {
        let s = desk();
        assert!(out_of_the_way(&Rect::new(2923., -1880., 1280., 800.), &s));
        assert!(!out_of_the_way(&Rect::new(1728., 0., 1280., 800.), &s));
        assert!(!out_of_the_way(&Rect::new(-1280., -800., 1280., 800.), &s));
    }

    #[test]
    fn a_new_display_with_a_lower_serial_leaves_parked_displays_where_they_are() {
        // Issue #204: displays 379 (serial 5) and 381 (serial 6) are already
        // parked with windows on them; new display 638 leases freed serial 1.
        let mut s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., 0., 1728., 1117.),
            Screen {
                id: 379,
                rect: Rect::new(4288., -800., 1280., 800.),
                role: Role::Ours { order: 5 },
            },
            Screen {
                id: 381,
                rect: Rect::new(5568., -800., 1280., 800.),
                role: Role::Ours { order: 6 },
            },
            Screen {
                id: 638,
                rect: Rect::new(-1280., 0., 1280., 800.),
                role: Role::Ours { order: 1 },
            },
        ];
        let planned = plan(&s);
        assert!(planned.contains(&(379, (4288., -800.))), "{planned:?}");
        assert!(planned.contains(&(381, (5568., -800.))), "{planned:?}");
        assert!(planned.contains(&(638, (6848., -800.))), "{planned:?}");
        assert_plan_safe(&s);

        // When a new display arrives right as the corner slot was vacated, the
        // new display fills 4288,-800 so all three end up connected without
        // moving 379 or 381.
        s[2].rect = Rect::new(5568., -800., 1280., 800.);
        s[3].rect = Rect::new(6848., -800., 1280., 800.);
        let planned = plan(&s);
        assert!(planned.contains(&(638, (4288., -800.))), "{planned:?}");
        assert!(planned.contains(&(379, (5568., -800.))), "{planned:?}");
        assert!(planned.contains(&(381, (6848., -800.))), "{planned:?}");
        assert_plan_safe(&s);
    }

    #[test]
    fn vacating_the_corner_with_no_new_display_reconnects_floating_displays() {
        let s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., 0., 1728., 1117.),
            Screen {
                id: 379,
                rect: Rect::new(5568., -800., 1280., 800.),
                role: Role::Ours { order: 5 },
            },
            Screen {
                id: 381,
                rect: Rect::new(6848., -800., 1280., 800.),
                role: Role::Ours { order: 6 },
            },
        ];
        assert!(!connected(&s));
        let planned = plan(&s);
        assert!(planned.contains(&(381, (4288., -800.))), "{planned:?}");
        assert!(planned.contains(&(379, (5568., -800.))), "{planned:?}");
        assert_plan_safe(&s);

        let single = vec![
            user(1, 0., 0., 2560., 1440.),
            Screen {
                id: 379,
                rect: Rect::new(3840., -800., 1280., 800.),
                role: Role::Ours { order: 5 },
            },
        ];
        assert!(!connected(&single));
        assert_eq!(plan(&single), vec![(379, (2560., -800.))]);
        assert_plan_safe(&single);
    }

    #[test]
    fn sequential_creates_and_ends_stay_connected_and_preserve_live_origins() {
        let mut s = vec![user(1, 0., 0., 2560., 1440.)];
        // Create four displays in a row with serials 1..=4.
        for (id, serial) in [(10, 1), (11, 2), (12, 3), (13, 4)] {
            s.push(ours(id, serial, 1280., 800.));
            let planned = plan(&s);
            s = with_origins(&s, &planned);
            assert_plan_safe(&s);
        }
        let d11_before = s.iter().find(|d| d.id == 11).unwrap().rect;
        let d12_before = s.iter().find(|d| d.id == 12).unwrap().rect;

        // End the two middle/far displays (13, then 10 at the corner), then
        // create 14 with recycled serial 1.
        s.retain(|d| d.id != 13);
        let planned = plan(&s);
        s = with_origins(&s, &planned);
        assert_plan_safe(&s);
        assert_eq!(s.iter().find(|d| d.id == 11).unwrap().rect, d11_before);
        assert_eq!(s.iter().find(|d| d.id == 12).unwrap().rect, d12_before);

        // Ending corner display 10 moves only the furthest display (12) into
        // the corner bridge so 11 never moves.
        s.retain(|d| d.id != 10);
        let planned = plan(&s);
        s = with_origins(&s, &planned);
        assert_plan_safe(&s);
        assert_eq!(s.iter().find(|d| d.id == 11).unwrap().rect, d11_before);

        // Creating 14 and 15 with lower serials parks into free slots without
        // disturbing 11 or 12.
        let d12_bridged = s.iter().find(|d| d.id == 12).unwrap().rect;
        for (id, serial) in [(14, 1), (15, 3)] {
            s.push(ours(id, serial, 1280., 800.));
            let planned = plan(&s);
            s = with_origins(&s, &planned);
            assert_plan_safe(&s);
        }
        assert_eq!(s.iter().find(|d| d.id == 11).unwrap().rect, d11_before);
        assert_eq!(s.iter().find(|d| d.id == 12).unwrap().rect, d12_bridged);
    }

    #[test]
    fn physical_display_layout_change_reanchors_displays_and_shifts_all_windows() {
        // User opens MacBook lid (1728x1117 at 0,0) beside 2560x1440 monitor,
        // shifting the rightmost user edge by +1728 pt (2560 -> 4288).
        let old_d1 = Rect::new(2560., -800., 1280., 800.);
        let old_d2 = Rect::new(3840., -800., 1280., 800.);
        let after_hotplug = vec![
            user(1, 0., 0., 1728., 1117.),
            user(2, 1728., 0., 2560., 1440.),
            Screen {
                id: 379,
                rect: old_d1,
                role: Role::Ours { order: 5 },
            },
            Screen {
                id: 381,
                rect: old_d2,
                role: Role::Ours { order: 6 },
            },
        ];
        // Old origins now overlap/lie left of the new rightmost user edge 4288,
        // so `plan` moves both into safe connected slots starting at 4288,-800.
        let planned = plan(&after_hotplug);
        assert_plan_safe(&after_hotplug);
        let settled = with_origins(&after_hotplug, &planned);
        let new_d1 = settled.iter().find(|s| s.id == 379).unwrap().rect;
        let new_d2 = settled.iter().find(|s| s.id == 381).unwrap().rect;
        assert_eq!(new_d1, Rect::new(4288., -800., 1280., 800.));
        assert_eq!(new_d2, Rect::new(5568., -800., 1280., 800.));

        let w1 = (
            WindowTarget {
                pid: 20,
                window_id: 101,
            },
            Rect::new(2660., -700., 800., 600.),
            Some(379),
        );
        let w2 = (
            WindowTarget {
                pid: 30,
                window_id: 102,
            },
            Rect::new(3940., -650., 700., 950.),
            None,
        );
        let user_win = (
            WindowTarget {
                pid: 40,
                window_id: 103,
            },
            Rect::new(1900., 100., 900., 700.),
            None,
        );
        let shifts: Vec<_> = window_shifts_with_owner(
            &[(379, old_d1, new_d1), (381, old_d2, new_d2)],
            &[
                Rect::new(0., 0., 1728., 1117.),
                Rect::new(1728., 0., 2560., 1440.),
            ],
            &[user_win, w1, w2],
        )
        .into_iter()
        .map(|(target, _, next)| (target, next))
        .collect();
        assert_eq!(
            shifts,
            vec![
                (w1.0, Rect::new(4388., -700., 800., 600.)),
                (w2.0, Rect::new(5668., -650., 700., 950.)),
            ]
        );
    }

    #[test]
    fn record_backed_ownership_wins_over_transient_viewport_overlap() {
        // While on a virtual display, an active placement/handover record on
        // 379 keeps ownership with 379 even if 638 transiently occupies the
        // window's coordinates.
        assert_eq!(resolve_display_owner(Some(638), true, Some(379)), Some(379));
        // Unrecorded dialogs fall back to geometric display.
        assert_eq!(resolve_display_owner(Some(638), true, None), Some(638));
        // Once returned to the user's physical screen, virtual ownership is None.
        assert_eq!(resolve_display_owner(None, false, Some(379)), None);
    }

    #[test]
    fn a_fixed_display_to_the_right_does_not_shove_parked_displays_past_it() {
        let s = vec![
            user(1, 0., 0., 2560., 1440.),
            user(2, 2560., 0., 1728., 1117.),
            Screen {
                id: 379,
                rect: Rect::new(4288., -800., 1280., 800.),
                role: Role::Ours { order: 5 },
            },
            Screen {
                id: 381,
                rect: Rect::new(5568., -800., 1280., 800.),
                role: Role::Ours { order: 6 },
            },
            Screen {
                id: 900,
                rect: Rect::new(6848., -800., 1280., 800.),
                role: Role::Fixed,
            },
            ours(638, 1, 1280., 800.),
        ];
        let planned = plan(&s);
        assert!(planned.contains(&(379, (4288., -800.))), "{planned:?}");
        assert!(planned.contains(&(381, (5568., -800.))), "{planned:?}");
        // 6848 is taken by Fixed 900, and 8128 + 1280 > 4288 + 4288, so 638
        // stacks onto the second row at 4288,-1600.
        assert!(planned.contains(&(638, (4288., -1600.))), "{planned:?}");
        assert_plan_safe(&s);
    }

    #[test]
    fn another_instances_corner_display_bridges_our_displays_without_moving_them() {
        // Another Agent Screen instance holds the corner slot (Role::Fixed at
        // 2560,-800); our display 379 sits to its right at 3840,-800 and stays
        // connected through the Fixed bridge when 380 is created at 5120,-800.
        let s = vec![
            user(1, 0., 0., 2560., 1440.),
            Screen {
                id: 900,
                rect: Rect::new(2560., -800., 1280., 800.),
                role: Role::Fixed,
            },
            Screen {
                id: 379,
                rect: Rect::new(3840., -800., 1280., 800.),
                role: Role::Ours { order: 2 },
            },
            ours(380, 1, 1280., 800.),
        ];
        let planned = plan(&s);
        assert!(planned.contains(&(379, (3840., -800.))), "{planned:?}");
        assert_plan_safe(&s);
    }

    #[test]
    fn overlapping_reparks_serialize_and_never_shift_unrecorded_windows_twice() {
        let user_rect = Rect::new(0., 0., 4288., 1440.);
        let r1 = Rect::new(4288., -800., 1280., 800.);
        let r2 = Rect::new(5568., -800., 1280., 800.);
        let r3 = Rect::new(6848., -800., 1280., 800.);

        let mut repark = ReparkState::default();
        repark.note_adopted(10, r1);
        repark.note_adopted(20, r2);

        let unrecorded_a = WindowTarget {
            pid: 1,
            window_id: 101,
        };
        let recorded_a = WindowTarget {
            pid: 1,
            window_id: 102,
        };
        let mut frame_unrec = Rect::new(4388., -700., 500., 400.);
        let mut frame_rec = Rect::new(4450., -650., 500., 400.);

        let (gen1, _) = repark.begin(&[10, 20]).expect("first repark starts");
        // Second ScreensChanged arrives while first is in flight: coalesces into rerun.
        assert_eq!(repark.begin(&[10, 20]), None);

        // Chain move: display 10 moves r1 -> r2, display 20 moves r2 -> r3.
        let settled = [(10, r2), (20, r3)];
        let windows1 = [
            (unrecorded_a, frame_unrec, None),
            (recorded_a, frame_rec, Some(10)),
        ];
        let (shifts1, moved1) =
            repark.plan_shifts(gen1, &[10, 20], &settled, &[user_rect], &windows1);
        assert_eq!(moved1.len(), 2);
        for (target, next) in shifts1 {
            if target == unrecorded_a {
                frame_unrec = next;
            } else if target == recorded_a {
                frame_rec = next;
            }
        }
        assert_eq!(frame_unrec.x, 5668.0);
        assert_eq!(frame_rec.x, 5730.0);

        // Stale generation cannot shift windows or overwrite parked_rects.
        let (stale_shifts, _) = repark.plan_shifts(
            gen1.wrapping_sub(1),
            &[10, 20],
            &[(10, r1), (20, r2)],
            &[user_rect],
            &windows1,
        );
        assert!(stale_shifts.is_empty());
        assert_eq!(repark.parked_rect(10), Some(r2));

        // Finish first pass: rerun was requested, so a second pass runs against
        // the updated baseline and shifts unrecorded_a zero times (stays at 5668,
        // never carried onto display 20 at 6948).
        assert!(repark.finish(gen1));
        let (gen2, _) = repark.begin(&[10, 20]).expect("coalesced rerun starts");
        let windows2 = [
            (unrecorded_a, frame_unrec, None),
            (recorded_a, frame_rec, Some(10)),
        ];
        let (shifts2, moved2) =
            repark.plan_shifts(gen2, &[10, 20], &settled, &[user_rect], &windows2);
        assert!(moved2.is_empty());
        assert!(shifts2.is_empty());
        assert_eq!(frame_unrec.x, 5668.0);
        assert!(!repark.finish(gen2));
    }

    /// A display that goes leaves no baseline and no failed shift behind,
    /// so its id, reused by a later display, never shifts that one's
    /// windows (#289).
    #[test]
    fn a_removed_display_leaves_no_pending_shift_or_baseline() {
        let user_rect = Rect::new(0., 0., 4288., 1440.);
        let old = Rect::new(4288., -800., 1280., 800.);
        let new = Rect::new(5568., -800., 1280., 800.);
        let window = WindowTarget {
            pid: 1,
            window_id: 101,
        };
        let frame = Rect::new(4388., -700., 500., 400.);
        let mut repark = ReparkState::default();
        repark.note_adopted(10, old);
        let (generation, _) = repark.begin(&[10]).unwrap();
        let (shifts, _) = repark.plan_shifts(
            generation,
            &[10],
            &[(10, new)],
            &[user_rect],
            &[(window, frame, None)],
        );
        assert_eq!(shifts.len(), 1);
        // The move failed: it waits to be tried again.
        repark.finish_with_failures(generation, &[window]);
        assert!(repark.has_pending_retries(&[10]));

        repark.remove_display(10);
        assert!(!repark.has_pending_retries(&[10]));
        assert_eq!(repark.parked_rect(10), None);
        let (_, snapshot) = repark.begin(&[10]).unwrap();
        assert!(snapshot.is_empty());
    }

    #[test]
    fn window_shifts_honors_recorded_owner_when_center_straddles_neighbor() {
        // M7 revert-4 guard: window 501 is recorded on display 379 (4288..5568)
        // while overhanging into neighbor 381 (5568..6848) so its center
        // (5250 + 400 = 5650) lies inside 381's old rect. Display 379 moves by
        // +400 in x; display 381 moves by +1280 in x. Without the `recorded_owner`
        // branch, geometry attributes 501 to 381 (+1280) instead of 379 (+400).
        let user_rect = Rect::new(0., 0., 4288., 1440.);
        let old_a = Rect::new(4288., -800., 1280., 800.);
        let new_a = Rect::new(4688., -800., 1280., 800.);
        let old_b = Rect::new(5568., -800., 1280., 800.);
        let new_b = Rect::new(6848., -800., 1280., 800.);
        let w = WindowTarget {
            pid: 9,
            window_id: 501,
        };
        let bounds = Rect::new(5250., -700., 800., 500.);
        assert!(old_b.contains(bounds.mid()));
        let shifts: Vec<_> = window_shifts_with_owner(
            &[(379, old_a, new_a), (381, old_b, new_b)],
            &[user_rect],
            &[(w, bounds, Some(379))],
        )
        .into_iter()
        .map(|(target, _, next)| (target, next))
        .collect();
        assert_eq!(shifts, vec![(w, Rect::new(5650., -700., 800., 500.))]);
    }

    #[test]
    fn effective_agent_display_centralizes_recorded_and_geometric_ownership() {
        let displays = [
            (1, Rect::new(0., 0., 2560., 1440.), false),
            (7, Rect::new(2560., -800., 1280., 800.), true),
            (8, Rect::new(3840., -800., 1280., 800.), true),
        ];
        // Window recorded on 7, transiently sitting on virtual display 8 -> owned by 7,
        // with owner 7's own base rect so display 8's origin never leaks into 7's coordinates (M2).
        let on_8 = Rect::new(3940., -700., 500., 400.);
        assert_eq!(
            effective_agent_display(&on_8, &displays, Some(7)),
            Some((7, Rect::new(2560., -800., 1280., 800.)))
        );
        // Unrecorded window on virtual display 8 -> owned by 8.
        assert_eq!(
            effective_agent_display(&on_8, &displays, None),
            Some((8, Rect::new(3840., -800., 1280., 800.)))
        );
        // Window back on user display 1 -> owned by no Agent Display even with record.
        let on_user = Rect::new(200., 200., 500., 400.);
        assert_eq!(effective_agent_display(&on_user, &displays, Some(7)), None);
    }
}
