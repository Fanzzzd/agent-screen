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

use crate::model::Rect;

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
/// Displays of ours that are already `out_of_the_way` and overlap neither a
/// fixed display nor an earlier display of ours keep their origins: moving a
/// live display in CoreGraphics moves only its viewport while WindowServer
/// leaves windows at their global coordinates, which would hand one display's
/// windows to whichever display took its old slot (#204). Only unparked or
/// overlapping displays of ours are placed into free slots of the grid.
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
    let mut obstacles = fixed;
    let mut ours: Vec<(u64, u32, Rect)> = screens
        .iter()
        .filter_map(|s| match s.role {
            Role::Ours { order } => Some((order, s.id, s.rect)),
            _ => None,
        })
        .collect();
    ours.sort_by_key(|&(order, id, _)| (order, id));
    let mut placed: Vec<(u64, u32, (f64, f64))> = Vec::with_capacity(ours.len());
    let mut unparked: Vec<(u64, u32, Rect)> = Vec::new();
    for (order, id, rect) in ours {
        if out_of_the_way(&rect, screens) && obstacles.iter().all(|o| !rect.intersects(o)) {
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
    /// Every planned display is safe, and none overlaps another.
    fn assert_plan_safe(s: &[Screen]) {
        let placed = rects(s, &plan(s));
        assert!(!placed.is_empty());
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

        // And when the first slot was vacated, the new display fills that gap
        // while still leaving the parked displays untouched.
        s[2].rect = Rect::new(5568., -800., 1280., 800.);
        s[3].rect = Rect::new(6848., -800., 1280., 800.);
        let planned = plan(&s);
        assert!(planned.contains(&(638, (4288., -800.))), "{planned:?}");
        assert!(planned.contains(&(379, (5568., -800.))), "{planned:?}");
        assert!(planned.contains(&(381, (6848., -800.))), "{planned:?}");
        assert_plan_safe(&s);
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
}
