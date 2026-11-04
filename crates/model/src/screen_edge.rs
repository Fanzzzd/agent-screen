//! The Input Grant's red edge around a screen, as a closed outline that
//! follows the screen's shape: square on an external display, rounded on a
//! built-in one, and around the camera notch where there is one. Points
//! are in points from the screen's top-left corner, y down. No OS calls.

pub type Point = (f64, f64);

/// The corner radius of a built-in MacBook display, in points. macOS has
/// no API for it (NSScreen answers no corner selector, and neither
/// IORegistry nor SkyLight names one), so this is an estimate a little
/// above the panel's rounding: an edge drawn tighter than the glass is cut
/// off at the corners, one drawn a little wider stays whole.
pub const BUILTIN_CORNER: f64 = 12.0;

/// Where the edge bends from the top of the screen down into the notch.
pub const NOTCH_FLARE: f64 = 4.0;
/// The rounding of the notch's lower corners.
pub const NOTCH_CORNER: f64 = 8.0;

/// The camera housing, from the screen's top-left corner: its left and
/// right sides and how far down it reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Notch {
    pub left: f64,
    pub right: f64,
    pub depth: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    /// 0 for square corners.
    pub corner: f64,
    pub notch: Option<Notch>,
}

impl Screen {
    /// A notch only counts when it lies within the top edge, clear of the
    /// rounded corners.
    fn notch(&self) -> Option<Notch> {
        self.notch.filter(|n| {
            n.depth > 0.0
                && n.left > self.corner + NOTCH_FLARE
                && n.right < self.width - self.corner - NOTCH_FLARE
                && n.left < n.right
                && n.depth < self.height / 2.0
        })
    }
}

/// Segments per quarter circle: about 1.5 pt each at the built-in radius.
const STEPS: usize = 8;

/// The centre line of a stroke `width` wide that lies wholly on the
/// visible screen: inset from its edges and outset from its notch by half
/// the stroke. Clockwise from the top-left corner's end; closed, the last
/// point joining the first.
pub fn outline(screen: Screen, width: f64) -> Vec<Point> {
    let i = width / 2.0;
    let (w, h) = (screen.width, screen.height);
    // Concentric with the glass's corner.
    let r = (screen.corner - i).max(0.0);
    let mut path = Vec::new();
    path.push((i + r, i));
    if let Some(n) = screen.notch() {
        let (left, right, depth) = (n.left - i, n.right + i, n.depth + i);
        let flare = NOTCH_FLARE;
        let lower = NOTCH_CORNER
            .min((right - left) / 2.0)
            .min(depth - i - flare);
        arc(&mut path, (left - flare, i + flare), flare, -90.0, 0.0);
        arc(&mut path, (left + lower, depth - lower), lower, 180.0, 90.0);
        arc(&mut path, (right - lower, depth - lower), lower, 90.0, 0.0);
        arc(&mut path, (right + flare, i + flare), flare, 180.0, 270.0);
    }
    arc(&mut path, (w - i - r, i + r), r, -90.0, 0.0);
    arc(&mut path, (w - i - r, h - i - r), r, 0.0, 90.0);
    arc(&mut path, (i + r, h - i - r), r, 90.0, 180.0);
    arc(&mut path, (i + r, i + r), r, 180.0, 270.0);
    // The last corner ends where the outline began.
    if path.len() > 1 && path.first() == path.last() {
        path.pop();
    }
    path
}

/// A circular arc from angle `from` to `to`, in degrees, y down: 0 points
/// right, 90 down. A zero radius is the corner point alone.
fn arc(path: &mut Vec<Point>, center: Point, radius: f64, from: f64, to: f64) {
    let steps = if radius > 0.0 { STEPS } else { 0 };
    for step in 0..=steps {
        let t = if steps == 0 {
            0.0
        } else {
            step as f64 / steps as f64
        };
        let angle = (from + (to - from) * t).to_radians();
        let p = (
            center.0 + radius * angle.cos(),
            center.1 + radius * angle.sin(),
        );
        if path
            .last()
            .is_none_or(|q| (q.0 - p.0).abs() > 1e-9 || (q.1 - p.1).abs() > 1e-9)
        {
            path.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACBOOK: Screen = Screen {
        width: 1728.0,
        height: 1117.0,
        corner: BUILTIN_CORNER,
        // This Mac's NSScreen: auxiliary areas end at 771 and start at
        // 956, with a 32 pt safe-area top.
        notch: Some(Notch {
            left: 771.0,
            right: 956.0,
            depth: 32.0,
        }),
    };

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn an_external_display_gets_the_inset_rectangle() {
        let path = outline(
            Screen {
                width: 2560.0,
                height: 1440.0,
                corner: 0.0,
                notch: None,
            },
            2.0,
        );
        assert_eq!(
            path,
            vec![(1.0, 1.0), (2559.0, 1.0), (2559.0, 1439.0), (1.0, 1439.0)]
        );
    }

    #[test]
    fn the_whole_stroke_stays_on_the_screen() {
        for screen in [
            MACBOOK,
            Screen {
                notch: None,
                ..MACBOOK
            },
        ] {
            for (x, y) in outline(screen, 2.0) {
                assert!((1.0 - 1e-9..=screen.width - 1.0 + 1e-9).contains(&x), "{x}");
                assert!(
                    (1.0 - 1e-9..=screen.height - 1.0 + 1e-9).contains(&y),
                    "{y}"
                );
            }
        }
    }

    #[test]
    fn built_in_corners_are_rounded_concentric_with_the_glass() {
        let path = outline(
            Screen {
                notch: None,
                ..MACBOOK
            },
            2.0,
        );
        let r = BUILTIN_CORNER - 1.0;
        // No point sits in a corner's square beyond the arc.
        let corners = [
            (1.0 + r, 1.0 + r),
            (MACBOOK.width - 1.0 - r, 1.0 + r),
            (MACBOOK.width - 1.0 - r, MACBOOK.height - 1.0 - r),
            (1.0 + r, MACBOOK.height - 1.0 - r),
        ];
        for c in corners {
            let on_arc: Vec<_> = path
                .iter()
                .filter(|p| (p.0 - c.0).abs() <= r + 1e-6 && (p.1 - c.1).abs() <= r + 1e-6)
                .filter(|p| !near(p.0, c.0) || !near(p.1, c.1))
                .collect();
            assert!(on_arc.len() > STEPS / 2, "{c:?}");
            for p in on_arc {
                let d = ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt();
                // On the arc, or on the straight edge leading into it.
                assert!(near(d, r) || near(p.0, c.0) || near(p.1, c.1), "{p:?} {d}");
            }
        }
    }

    #[test]
    fn the_edge_goes_around_the_notch_never_over_it() {
        let path = outline(MACBOOK, 2.0);
        let n = MACBOOK.notch.unwrap();
        // Nothing inside the housing, with its rounded lower corners,
        // grown by half the stroke.
        let (left, right, depth) = (n.left - 1.0, n.right + 1.0, n.depth + 1.0);
        let r = NOTCH_CORNER;
        let under = |x: f64, y: f64| {
            let e = 1e-6;
            if !(x > left + e && x < right - e && y < depth - e) {
                return false;
            }
            let corner = if x < left + r && y > depth - r {
                Some((left + r, depth - r))
            } else if x > right - r && y > depth - r {
                Some((right - r, depth - r))
            } else {
                None
            };
            corner.is_none_or(|c| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() < r - e)
        };
        for &(x, y) in &path {
            assert!(!under(x, y), "({x}, {y}) is under the notch");
        }
        // It runs down the notch's sides and along its bottom.
        assert!(path.iter().any(|p| near(p.0, left) && p.1 > n.depth / 2.0));
        assert!(path.iter().any(|p| near(p.0, right) && p.1 > n.depth / 2.0));
        let bottom: Vec<_> = path.iter().filter(|p| near(p.1, n.depth + 1.0)).collect();
        assert!(bottom.iter().any(|p| p.0 < n.left + NOTCH_CORNER + 1.0));
        assert!(bottom.iter().any(|p| p.0 > n.right - NOTCH_CORNER - 1.0));
        // And along the top edge on both sides of it.
        assert!(
            path.iter()
                .any(|p| near(p.1, 1.0) && p.0 < n.left - NOTCH_FLARE)
        );
        assert!(
            path.iter()
                .any(|p| near(p.1, 1.0) && p.0 > n.right + NOTCH_FLARE)
        );
    }

    #[test]
    fn curves_take_short_steps_and_straight_runs_stay_straight() {
        let path = outline(MACBOOK, 2.0);
        for w in path.windows(2) {
            let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            let straight = near(dx, 0.0) || near(dy, 0.0);
            assert!(straight || (dx * dx + dy * dy).sqrt() < 2.5, "{w:?}");
        }
    }

    #[test]
    fn a_notch_that_does_not_fit_is_left_out() {
        let screen = Screen {
            notch: Some(Notch {
                left: 2.0,
                right: 40.0,
                depth: 32.0,
            }),
            ..MACBOOK
        };
        let plain = Screen {
            notch: None,
            ..MACBOOK
        };
        assert_eq!(outline(screen, 2.0), outline(plain, 2.0));
    }
}
