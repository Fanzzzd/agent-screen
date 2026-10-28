//! Where Agent Screen's displays go among the user's, in CoreGraphics
//! global coordinates (top-left origin, y down).
//!
//! The user's displays never move. Agent Screen's own sit in a compact grid
//! off the top-right corner of the user's displays: the first row touches
//! them only at that corner, so the real cursor rarely wanders onto one,
//! and each further row stacks on the one below. A row is as wide as the
//! user's displays, so the whole arrangement stays close to square and
//! System Settings shows the user's displays at a usable size. Displays go
//! in serial order, so the same set always lands in the same places.
//!
//! Not the bottom corners: AeroSpace hides the windows of other workspaces
//! in a monitor's bottom corner, and a display there would show them to
//! agents.

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

/// Where each of Agent Screen's displays goes, as origins. Empty when the
/// user has no display of their own to arrange around.
pub fn plan(screens: &[Screen]) -> Vec<(u32, (f64, f64))> {
    let Some(user) = user_bounds(screens) else {
        return Vec::new();
    };
    // Keep clear of fixed displays above the user's: another instance's
    // grid. AeroSpace's closet hangs below, out of the way.
    let start = screens
        .iter()
        .filter(|s| {
            s.role == Role::Fixed && s.rect.max_y() <= user.y && s.rect.max_x() > user.max_x()
        })
        .map(|s| s.rect.max_x())
        .fold(user.max_x(), f64::max);
    let mut ours: Vec<(u64, u32, Rect)> = screens
        .iter()
        .filter_map(|s| match s.role {
            Role::Ours { order } => Some((order, s.id, s.rect)),
            _ => None,
        })
        .collect();
    ours.sort_by_key(|&(order, id, _)| (order, id));
    let mut placed = Vec::with_capacity(ours.len());
    let (mut x, mut bottom, mut top) = (start, user.y, user.y);
    for (_, id, rect) in ours {
        if x > start && x + rect.width > start + user.width {
            x = start;
            bottom = top;
        }
        let y = bottom - rect.height;
        placed.push((id, (x, y)));
        x += rect.width;
        top = top.min(y);
    }
    placed
}

/// Whether `rect` is out of the cursor's way: wholly above and right of
/// the user's displays. One the user dragged elsewhere is not.
pub fn out_of_the_way(rect: &Rect, user: &Rect) -> bool {
    rect.max_y() <= user.y && rect.x >= user.max_x()
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
        let user = user_bounds(&s).unwrap();
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
            assert!(out_of_the_way(r, &user));
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
    fn a_display_dragged_next_to_the_users_is_in_the_way() {
        let user = user_bounds(&desk()).unwrap();
        assert!(out_of_the_way(
            &Rect::new(2923., -1880., 1280., 800.),
            &user
        ));
        assert!(!out_of_the_way(&Rect::new(1728., 0., 1280., 800.), &user));
        assert!(!out_of_the_way(
            &Rect::new(-1280., -800., 1280., 800.),
            &user
        ));
    }
}
