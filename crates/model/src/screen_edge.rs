//! The Input Grant's red edge around a screen: the outline of the screen's
//! visible glass, square on an external display and on built-in panels
//! nobody has measured, and the measured shape on a notched MacBook Pro:
//! rounded top corners, square bottom corners, and a path around the
//! camera notch. Points are in points from the screen's top-left corner,
//! y down. No OS calls.
//!
//! The app strokes the outline at twice the edge's width and clips the
//! stroke to the outline, so the visible line lies wholly on the glass, its
//! outer side on the glass's edge, and keeps its width through every curve.
//! An inset path can't do that exactly: a curve offset by half a stroke is
//! no longer a cubic Bézier.
//!
//! macOS has no API for the glass's shape: NSScreen answers no corner
//! selector, and neither IORegistry nor SkyLight names one. The rounding
//! comes from fitting Apple's MacBook Pro bezel art (Apple Design
//! Resources), whose screen opening is the glass, pixel for pixel.

pub type Point = (f64, f64);

/// How far a continuous corner reaches along each of its edges, in radii.
pub const CONTINUOUS_EXTENT: f64 = 1.528665;

/// Apple's continuous ("squircle") corner, the curve of CALayer's
/// `cornerCurve = .continuous` and SwiftUI's `RoundedRectangle(style:
/// .continuous)`, as the three cubic Béziers SwiftUI's path is made of, in
/// radii. A point is (u, v): u along the edge the corner comes from, v
/// along the edge it goes to, both from the corner's vertex. It starts at
/// (CONTINUOUS_EXTENT, 0) and ends at (0, CONTINUOUS_EXTENT).
const CONTINUOUS: [[Point; 3]; 3] = [
    [(1.088490, 0.0), (0.868407, 0.0), (0.631494, 0.074911)],
    [
        (0.372824, 0.169060),
        (0.169060, 0.372824),
        (0.074911, 0.631494),
    ],
    [(0.0, 0.868407), (0.0, 1.088490), (0.0, CONTINUOUS_EXTENT)],
];

/// A built-in panel's rounding, as continuous-corner radii in the panel's
/// own pixels, so it holds in every display mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezel {
    /// The glass's top corners.
    pub top: f64,
    /// The glass's bottom corners.
    pub bottom: f64,
    /// The notch's lower corners.
    pub notch_corner: f64,
    /// Where the top edge turns down into the notch.
    pub notch_fillet: f64,
}

impl Bezel {
    /// No rounding: an external display, and the fallback for a built-in
    /// panel without measured data. A guess larger than the real rounding
    /// would draw the line off the glass's edge where the panel is square;
    /// square corners on a rounded panel only lose the corner's tip under
    /// the bezel.
    pub const SQUARE: Bezel = Bezel {
        top: 0.0,
        bottom: 0.0,
        notch_corner: 0.0,
        notch_fillet: 0.0,
    };

    /// The 16-inch MacBook Pro (M1 Pro and later, 3456 × 2234 panel),
    /// fitted to its bezel art.
    pub const MACBOOK_PRO_16: Bezel = Bezel {
        top: 39.75,
        bottom: 0.0,
        notch_corner: 20.5,
        notch_fillet: 9.75,
    };

    /// The 14-inch MacBook Pro (M1 Pro and later, 3024 × 1964 panel),
    /// fitted to its bezel art.
    pub const MACBOOK_PRO_14: Bezel = Bezel {
        top: 38.75,
        bottom: 0.0,
        notch_corner: 20.5,
        notch_fillet: 9.25,
    };

    /// The rounding of a built-in panel with `pixels` native pixels, wide
    /// and tall, on a screen that reports a notch or not. Keyed on the
    /// panel, which is what has the shape, rather than on `hw.model`, which
    /// changes every generation while the panels stay. A notched MacBook
    /// Pro in a mode that ends below the notch reports none: the screen's
    /// top then runs below the rounded corners, so it is square too.
    pub fn of_builtin(pixels: (u32, u32), notch: bool) -> Bezel {
        match (pixels, notch) {
            ((3456, 2234), true) => Bezel::MACBOOK_PRO_16,
            ((3024, 1964), true) => Bezel::MACBOOK_PRO_14,
            _ => Bezel::SQUARE,
        }
    }
}

/// The camera housing, from the screen's top-left corner: its left and
/// right sides and how far down it reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Notch {
    pub left: f64,
    pub right: f64,
    pub depth: f64,
}

impl Notch {
    /// The notch between the screen's top-left area, which ends at
    /// `left`, and its top-right area, which starts at `right`, on a
    /// screen `width` points wide. NSScreen reports the areas in whole
    /// points, so a 185-point notch on a 1728-point screen ends the left
    /// area at 771 and starts the right one at 956, half a point left of
    /// the centre, while the bezel art of both MacBook Pros has the
    /// housing centred on the panel to a hundredth of a pixel. A notch
    /// within a point of the centre is the centred housing, as wide as
    /// reported.
    pub fn from_areas(width: f64, left: f64, right: f64, depth: f64) -> Notch {
        let half = (right - left) / 2.0;
        let centre = width / 2.0;
        if ((left + right) / 2.0 - centre).abs() <= 1.0 {
            Notch {
                left: centre - half,
                right: centre + half,
                depth,
            }
        } else {
            Notch { left, right, depth }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    /// Panel pixels per point: the panel's native pixel width over the
    /// screen's width in points. 2 in a built-in display's default mode,
    /// 1.92 at 1800 points on a 3456-pixel panel.
    pub scale: f64,
    pub bezel: Bezel,
    pub notch: Option<Notch>,
}

/// One piece of an outline, from where the last one ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Line(Point),
    /// A cubic Bézier: two control points, then its end.
    Curve(Point, Point, Point),
}

/// A closed outline: `start`, then each segment in turn, the last ending
/// at `start`. Clockwise on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub start: Point,
    pub segments: Vec<Segment>,
}

impl Outline {
    /// Every point that defines the outline: its start, and each segment's
    /// control points and end. Each curve lies within the hull of its
    /// points, so a region holding them all holds the outline.
    pub fn points(&self) -> impl Iterator<Item = Point> + '_ {
        std::iter::once(self.start).chain(self.segments.iter().flat_map(|s| match *s {
            Segment::Line(p) => vec![p],
            Segment::Curve(a, b, p) => vec![a, b, p],
        }))
    }
}

/// The edge of `screen`'s visible glass. Stroke it at twice the edge's
/// width, clipped to itself.
pub fn outline(screen: Screen) -> Outline {
    let (w, h) = (screen.width, screen.height);
    let reach = |px: f64| px / screen.scale * CONTINUOUS_EXTENT;
    // A corner reaches at most half the shorter side.
    let fit = |px: f64| reach(px).min(w.min(h) / 2.0) / CONTINUOUS_EXTENT;
    let (top, bottom) = (fit(screen.bezel.top), fit(screen.bezel.bottom));
    let top_reach = top * CONTINUOUS_EXTENT;
    let mut path = Path::new((top_reach, 0.0));
    if let Some((n, fillet, lower)) = notch(&screen, top_reach, &reach) {
        path.corner((n.left, 0.0), (-1.0, 0.0), (0.0, 1.0), fillet);
        path.corner((n.left, n.depth), (0.0, -1.0), (1.0, 0.0), lower);
        path.corner((n.right, n.depth), (-1.0, 0.0), (0.0, -1.0), lower);
        path.corner((n.right, 0.0), (0.0, 1.0), (1.0, 0.0), fillet);
    }
    path.corner((w, 0.0), (-1.0, 0.0), (0.0, 1.0), top);
    path.corner((w, h), (0.0, -1.0), (-1.0, 0.0), bottom);
    path.corner((0.0, h), (1.0, 0.0), (0.0, -1.0), bottom);
    path.corner((0.0, 0.0), (0.0, 1.0), (1.0, 0.0), top);
    path.outline
}

/// The notch with its fillet and lower-corner radii in points, when it
/// lies within the top edge, clear of the top corners. Radii that don't
/// fit the housing shrink together.
fn notch(screen: &Screen, top_reach: f64, reach: &dyn Fn(f64) -> f64) -> Option<(Notch, f64, f64)> {
    let n = screen.notch?;
    let (fillet, lower) = (
        reach(screen.bezel.notch_fillet),
        reach(screen.bezel.notch_corner),
    );
    let fits = n.depth > 0.0
        && n.left < n.right
        && n.left - fillet >= top_reach
        && n.right + fillet <= screen.width - top_reach
        && n.depth < screen.height / 2.0;
    if !fits {
        return None;
    }
    let shrink = (n.depth / (fillet + lower)).min(1.0);
    let lower = (lower * shrink).min((n.right - n.left) / 2.0);
    Some((
        n,
        fillet * shrink / CONTINUOUS_EXTENT,
        lower / CONTINUOUS_EXTENT,
    ))
}

struct Path {
    outline: Outline,
    at: Point,
}

impl Path {
    fn new(start: Point) -> Path {
        Path {
            outline: Outline {
                start,
                segments: Vec::new(),
            },
            at: start,
        }
    }

    fn line(&mut self, p: Point) {
        if (p.0 - self.at.0).abs() > 1e-9 || (p.1 - self.at.1).abs() > 1e-9 {
            self.outline.segments.push(Segment::Line(p));
            self.at = p;
        }
    }

    /// A continuous corner of `radius` at `vertex`, coming in along the
    /// edge that runs from the vertex towards `from`, and leaving along the
    /// one towards `to`, both unit vectors. A zero radius is the vertex.
    fn corner(&mut self, vertex: Point, from: Point, to: Point, radius: f64) {
        let at = |(u, v): Point| {
            (
                vertex.0 + radius * (u * from.0 + v * to.0),
                vertex.1 + radius * (u * from.1 + v * to.1),
            )
        };
        if radius <= 0.0 {
            self.line(vertex);
            return;
        }
        self.line(at((CONTINUOUS_EXTENT, 0.0)));
        for [a, b, p] in CONTINUOUS {
            let p = at(p);
            self.outline.segments.push(Segment::Curve(at(a), at(b), p));
            self.at = p;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This Mac's built-in display at its default 1728 × 1117 points, as
    /// NSScreen reports its auxiliary areas: the left one ends at 771, the
    /// right one starts at 956, with a 32 pt safe-area top.
    const MACBOOK: Screen = Screen {
        width: 1728.0,
        height: 1117.0,
        scale: 2.0,
        bezel: Bezel::MACBOOK_PRO_16,
        notch: Some(Notch {
            left: 771.0,
            right: 956.0,
            depth: 32.0,
        }),
    };

    fn near(a: Point, b: Point) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    fn ends(o: &Outline) -> Vec<Point> {
        o.segments
            .iter()
            .map(|s| match *s {
                Segment::Line(p) | Segment::Curve(_, _, p) => p,
            })
            .collect()
    }

    /// Points along the outline, a few per curve.
    fn walk(o: &Outline) -> Vec<Point> {
        let mut at = o.start;
        let mut out = vec![at];
        for s in &o.segments {
            match *s {
                Segment::Line(p) => out.push(p),
                Segment::Curve(a, b, p) => {
                    for i in 1..=16 {
                        let t = i as f64 / 16.0;
                        let m = 1.0 - t;
                        let f = |q0: f64, q1: f64, q2: f64, q3: f64| {
                            m * m * m * q0
                                + 3.0 * m * m * t * q1
                                + 3.0 * m * t * t * q2
                                + t * t * t * q3
                        };
                        out.push((f(at.0, a.0, b.0, p.0), f(at.1, a.1, b.1, p.1)));
                    }
                }
            }
            at = ends(&Outline {
                start: at,
                segments: vec![*s],
            })[0];
        }
        out
    }

    #[test]
    fn an_external_display_is_its_frame() {
        let o = outline(Screen {
            width: 2560.0,
            height: 1440.0,
            scale: 2.0,
            bezel: Bezel::SQUARE,
            notch: None,
        });
        assert_eq!(o.start, (0.0, 0.0));
        assert_eq!(
            o.segments,
            [(2560.0, 0.0), (2560.0, 1440.0), (0.0, 1440.0), (0.0, 0.0)].map(Segment::Line)
        );
    }

    #[test]
    fn the_outline_closes_where_it_starts() {
        for screen in [
            MACBOOK,
            Screen {
                notch: None,
                ..MACBOOK
            },
        ] {
            let o = outline(screen);
            assert!(near(*ends(&o).last().unwrap(), o.start));
        }
    }

    #[test]
    fn top_corners_are_continuous_and_bottom_corners_square() {
        let o = outline(Screen {
            notch: None,
            ..MACBOOK
        });
        let reach = 39.75 / 2.0 * CONTINUOUS_EXTENT;
        let (w, h) = (MACBOOK.width, MACBOOK.height);
        assert!(near(o.start, (reach, 0.0)));
        let e = ends(&o);
        // Along the top to the top-right corner, its curve down onto the
        // right side, straight to the square bottom corners, and up the
        // left side into the top-left corner's curve.
        assert!(near(e[0], (w - reach, 0.0)));
        assert!(near(e[3], (w, reach)));
        assert!(near(e[4], (w, h)));
        assert!(near(e[5], (0.0, h)));
        assert!(near(e[6], (0.0, reach)));
        assert!(near(e[9], (reach, 0.0)));
        assert_eq!(e.len(), 10);
        assert!(matches!(o.segments[4], Segment::Line(_)));
        assert!(matches!(o.segments[5], Segment::Line(_)));
    }

    #[test]
    fn a_continuous_corner_is_apples_curve() {
        // The top-right corner: three cubics that leave the top edge along
        // it, reach the side along it, and mirror about the corner's
        // diagonal.
        let o = outline(Screen {
            notch: None,
            ..MACBOOK
        });
        let r = 39.75 / 2.0;
        let w = MACBOOK.width;
        let mut at = ends(&o)[0];
        let mut cubics = Vec::new();
        for s in &o.segments[1..4] {
            let Segment::Curve(a, b, p) = *s else {
                panic!("{s:?}")
            };
            cubics.push([at, a, b, p]);
            at = p;
        }
        assert!(cubics[0][1].1.abs() < 1e-9);
        assert!((cubics[2][2].0 - w).abs() < 1e-9);
        let flip = |q: Point| (w - q.1, w - q.0);
        let [s, a, b, p] = cubics[1];
        assert!(near(flip(s), p));
        assert!(near(flip(a), b));
        assert!(near(flip(cubics[0][0]), cubics[2][3]));
        // Its middle sits 0.2915 radii in from both edges, where SwiftUI's
        // continuous corner puts it; a circular one would be at 0.2929.
        let mid = (
            (s.0 + 3.0 * a.0 + 3.0 * b.0 + p.0) / 8.0,
            (s.1 + 3.0 * a.1 + 3.0 * b.1 + p.1) / 8.0,
        );
        assert!(((w - mid.0) / r - 0.291507).abs() < 1e-6);
        assert!((mid.1 / r - 0.291507).abs() < 1e-6);
    }

    #[test]
    fn radii_scale_with_the_panel_not_the_backing_store() {
        // 1800 points on the 3456-pixel panel: 1.92 panel pixels a point.
        let o = outline(Screen {
            width: 1800.0,
            height: 1169.0,
            scale: 3456.0 / 1800.0,
            notch: None,
            ..MACBOOK
        });
        let reach = 39.75 / 1.92 * CONTINUOUS_EXTENT;
        assert!(near(o.start, (reach, 0.0)));
    }

    #[test]
    fn the_notch_is_traced_down_its_sides_and_along_its_bottom() {
        let n = Notch::from_areas(1728.0, 771.0, 956.0, 32.0);
        let o = outline(Screen {
            notch: Some(n),
            ..MACBOOK
        });
        let fillet = 9.75 / 2.0 * CONTINUOUS_EXTENT;
        let lower = 20.5 / 2.0 * CONTINUOUS_EXTENT;
        let lines: Vec<(Point, Point)> = {
            let mut at = o.start;
            let mut v = Vec::new();
            for s in &o.segments {
                match *s {
                    Segment::Line(p) => {
                        v.push((at, p));
                        at = p;
                    }
                    Segment::Curve(_, _, p) => at = p,
                }
            }
            v
        };
        let has = |a: Point, b: Point| lines.iter().any(|&(p, q)| near(p, a) && near(q, b));
        // The top edge runs to the fillet, which turns down the notch's
        // side; the side runs straight to the lower corner, the bottom
        // straight between the lower corners, and back up the other side.
        let top_reach = 39.75 / 2.0 * CONTINUOUS_EXTENT;
        assert!(has((top_reach, 0.0), (n.left - fillet, 0.0)));
        assert!(has((n.left, fillet), (n.left, n.depth - lower)));
        assert!(has((n.left + lower, n.depth), (n.right - lower, n.depth)));
        assert!(has((n.right, n.depth - lower), (n.right, fillet)));
        assert!(has(
            (n.right + fillet, 0.0),
            (MACBOOK.width - top_reach, 0.0)
        ));
    }

    #[test]
    fn a_notch_rounded_to_whole_points_is_centred() {
        // This Mac's areas, half a point left of the centre.
        let n = Notch::from_areas(1728.0, 771.0, 956.0, 32.0);
        assert_eq!((n.left, n.right, n.depth), (771.5, 956.5, 32.0));
        // Already centred, unchanged.
        let n = Notch::from_areas(1512.0, 663.0, 849.0, 32.0);
        assert_eq!((n.left, n.right), (663.0, 849.0));
        // Far off centre: as reported.
        let n = Notch::from_areas(1728.0, 700.0, 900.0, 32.0);
        assert_eq!((n.left, n.right), (700.0, 900.0));
    }

    #[test]
    fn the_outline_is_symmetric_about_the_centre() {
        let screen = Screen {
            notch: Some(Notch::from_areas(1728.0, 771.0, 956.0, 32.0)),
            ..MACBOOK
        };
        // Every defining point has its mirror image among them.
        let points: Vec<_> = outline(screen).points().collect();
        for &(x, y) in &points {
            let mirror = (screen.width - x, y);
            assert!(points.iter().any(|&q| near(q, mirror)), "{mirror:?}");
        }
    }

    #[test]
    fn the_outline_stays_on_the_glass() {
        let n = Notch::from_areas(1728.0, 771.0, 956.0, 32.0);
        for screen in [
            MACBOOK,
            Screen {
                notch: Some(n),
                ..MACBOOK
            },
            Screen {
                bezel: Bezel::SQUARE,
                ..MACBOOK
            },
        ] {
            let o = outline(screen);
            for (x, y) in o.points().chain(walk(&o)) {
                assert!((-1e-9..=screen.width + 1e-9).contains(&x), "{x}");
                assert!((-1e-9..=screen.height + 1e-9).contains(&y), "{y}");
                // Off the housing: the glass reaches into the notch's
                // rectangle only at its rounded lower corners.
                let n = screen.notch.unwrap();
                let lower = screen.bezel.notch_corner / screen.scale * CONTINUOUS_EXTENT;
                let inside = |lo: f64, hi: f64, v: f64| v > lo + 1e-9 && v < hi - 1e-9;
                let under = inside(n.left, n.right, x) && inside(0.0, n.depth - lower, y)
                    || inside(n.left + lower, n.right - lower, x) && inside(0.0, n.depth, y);
                assert!(!under, "({x}, {y}) is under the notch");
            }
        }
    }

    #[test]
    fn only_measured_panels_are_rounded() {
        assert_eq!(Bezel::of_builtin((3456, 2234), true), Bezel::MACBOOK_PRO_16);
        assert_eq!(Bezel::of_builtin((3024, 1964), true), Bezel::MACBOOK_PRO_14);
        // The same panels in a mode that ends below the notch.
        assert_eq!(Bezel::of_builtin((3456, 2234), false), Bezel::SQUARE);
        // A notched MacBook Air, and an older MacBook Pro without a notch.
        assert_eq!(Bezel::of_builtin((2560, 1664), true), Bezel::SQUARE);
        assert_eq!(Bezel::of_builtin((2880, 1800), false), Bezel::SQUARE);
    }

    #[test]
    fn an_unmeasured_notch_is_a_square_cut() {
        let o = outline(Screen {
            bezel: Bezel::SQUARE,
            ..MACBOOK
        });
        assert_eq!(o.start, (0.0, 0.0));
        assert_eq!(
            ends(&o),
            [
                (771.0, 0.0),
                (771.0, 32.0),
                (956.0, 32.0),
                (956.0, 0.0),
                (1728.0, 0.0),
                (1728.0, 1117.0),
                (0.0, 1117.0),
                (0.0, 0.0),
            ]
        );
    }

    #[test]
    fn a_notch_that_does_not_fit_is_left_out() {
        let screen = Screen {
            notch: Some(Notch {
                left: 20.0,
                right: 60.0,
                depth: 32.0,
            }),
            ..MACBOOK
        };
        let plain = Screen {
            notch: None,
            ..MACBOOK
        };
        assert_eq!(outline(screen), outline(plain));
    }
}
