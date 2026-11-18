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
//!
//! Every corner is a smoothed corner, the construction Figma calls corner
//! smoothing: a circular arc of the corner's radius, shortened at both
//! ends, with a cubic Bézier from each end onto its edge. More smoothing
//! starts the curve farther along the edge and eases it in. Apple's
//! continuous corner, SwiftUI's `RoundedRectangle(style: .continuous)`, is
//! close to 0.6. The bezel art's corners are not that curve: at the radius
//! that fits a top corner's middle, Apple's curve reaches the side sooner
//! than the glass does and runs 0.5 px past the glass's edge there.

pub type Point = (f64, f64);

/// One corner's rounding, in the panel's own pixels, so it holds in every
/// display mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    /// The radius of the corner's circular arc.
    pub radius: f64,
    /// 0 is a circular corner; 1 has no arc left, only the two easing
    /// curves. The curve reaches `(1 + smoothing) × radius` along each
    /// edge.
    pub smoothing: f64,
}

impl Corner {
    pub const SQUARE: Corner = Corner {
        radius: 0.0,
        smoothing: 0.0,
    };

    const fn new(radius: f64, smoothing: f64) -> Corner {
        Corner { radius, smoothing }
    }

    /// How far the curve reaches along each edge, per unit of radius.
    fn extent(self) -> f64 {
        1.0 + self.smoothing.clamp(0.0, 1.0)
    }

    /// The corner's cubic Béziers for a radius `radius`, as points (u, v):
    /// u along the edge the corner comes from, v along the edge it goes
    /// to, both from the corner's vertex. They start at (extent × radius,
    /// 0) and end at (0, extent × radius), mirror images about the
    /// diagonal.
    fn cubics(self, radius: f64) -> Vec<[Point; 3]> {
        let s = self.smoothing.clamp(0.0, 1.0);
        let r = radius;
        let p = (1.0 + s) * r;
        // The arc's sweep, and its chord along each axis.
        let arc = std::f64::consts::FRAC_PI_2 * (1.0 - s);
        let chord = (arc / 2.0).sin() * r * std::f64::consts::SQRT_2;
        // The easing curve's last leg, which meets the arc on its tangent.
        let alpha = (std::f64::consts::FRAC_PI_2 - arc) / 2.0;
        let beta = std::f64::consts::FRAC_PI_4 * s;
        let c = r * (alpha / 2.0).tan() * beta.cos();
        let d = c * beta.tan();
        let b = (p - chord - c - d) / 3.0;
        let a = 2.0 * b;
        // Built along the incoming edge, x from where the curve leaves it
        // and y towards the outgoing edge, then turned into (u, v).
        let uv = |(x, y): Point| (p - x, y);
        let arc_start = (a + b + c, d);
        let arc_end = (arc_start.0 + chord, arc_start.1 + chord);
        let mut out = Vec::new();
        if a + b + c > 1e-12 {
            out.push([(a, 0.0), (a + b, 0.0), arc_start].map(uv));
        }
        if arc > 1e-12 {
            // One cubic per arc of at most 90°: its tangents' lengths are
            // 4/3 × tan(sweep / 4) radii.
            let centre = (p - r, r);
            let tangent = |(x, y): Point| (-(y - centre.1) / r, (x - centre.0) / r);
            let k = 4.0 / 3.0 * (arc / 4.0).tan() * r;
            let (t0, t1) = (tangent(arc_start), tangent(arc_end));
            out.push(
                [
                    (arc_start.0 + k * t0.0, arc_start.1 + k * t0.1),
                    (arc_end.0 - k * t1.0, arc_end.1 - k * t1.1),
                    arc_end,
                ]
                .map(uv),
            );
        }
        if a + b + c > 1e-12 {
            let (x, y) = arc_end;
            out.push([(x + d, y + c), (x + d, y + b + c), (x + d, y + a + b + c)].map(uv));
        }
        out
    }
}

/// A built-in panel's rounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezel {
    /// The glass's top corners.
    pub top: Corner,
    /// The glass's bottom corners.
    pub bottom: Corner,
    /// The notch's lower corners.
    pub notch_corner: Corner,
    /// Where the top edge turns down into the notch.
    pub notch_fillet: Corner,
}

impl Bezel {
    /// No rounding: an external display, and the fallback for a built-in
    /// panel without measured data. A guess larger than the real rounding
    /// would draw the line off the glass's edge where the panel is square;
    /// square corners on a rounded panel only lose the corner's tip under
    /// the bezel.
    pub const SQUARE: Bezel = Bezel {
        top: Corner::SQUARE,
        bottom: Corner::SQUARE,
        notch_corner: Corner::SQUARE,
        notch_fillet: Corner::SQUARE,
    };

    /// The 16-inch MacBook Pro (M1 Pro and later, 3456 × 2234 panel),
    /// fitted to its bezel art so the outline stays on the glass.
    pub const MACBOOK_PRO_16: Bezel = Bezel {
        top: Corner::new(39.2, 0.92),
        bottom: Corner::SQUARE,
        notch_corner: Corner::new(20.5, 0.5),
        notch_fillet: Corner::new(10.25, 0.6),
    };

    /// The 14-inch MacBook Pro (M1 Pro and later, 3024 × 1964 panel),
    /// fitted to its bezel art so the outline stays on the glass.
    pub const MACBOOK_PRO_14: Bezel = Bezel {
        top: Corner::new(39.2, 0.78),
        bottom: Corner::SQUARE,
        notch_corner: Corner::new(21.0, 0.6),
        notch_fillet: Corner::new(9.25, 0.6),
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
    // Radii in points; a corner reaches at most half the shorter side.
    let fit = |c: Corner| Sized {
        shape: c,
        radius: (c.radius.max(0.0) / screen.scale).min(w.min(h) / 2.0 / c.extent()),
    };
    let (top, bottom) = (fit(screen.bezel.top), fit(screen.bezel.bottom));
    let mut path = Path::new((top.reach(), 0.0));
    if let Some((n, fillet, lower)) = notch(&screen, top.reach()) {
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

/// A corner's shape with its radius in points.
#[derive(Clone, Copy)]
struct Sized {
    shape: Corner,
    radius: f64,
}

impl Sized {
    fn reach(self) -> f64 {
        self.radius * self.shape.extent()
    }
}

/// The notch with its fillets and lower corners in points, when it lies
/// within the top edge, clear of the top corners. Corners that don't fit
/// the housing shrink together.
fn notch(screen: &Screen, top_reach: f64) -> Option<(Notch, Sized, Sized)> {
    let n = screen.notch?;
    let sized = |c: Corner| Sized {
        shape: c,
        radius: c.radius.max(0.0) / screen.scale,
    };
    let (fillet, lower) = (
        sized(screen.bezel.notch_fillet),
        sized(screen.bezel.notch_corner),
    );
    let fits = n.depth > 0.0
        && n.left < n.right
        && n.left - fillet.reach() >= top_reach
        && n.right + fillet.reach() <= screen.width - top_reach
        && n.depth < screen.height / 2.0;
    if !fits {
        return None;
    }
    let shrink = (n.depth / (fillet.reach() + lower.reach())).min(1.0);
    let lower_reach = (lower.reach() * shrink).min((n.right - n.left) / 2.0);
    Some((
        n,
        Sized {
            radius: fillet.radius * shrink,
            ..fillet
        },
        Sized {
            radius: lower_reach / lower.shape.extent(),
            ..lower
        },
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

    /// A smoothed corner at `vertex`, coming in along the edge that runs
    /// from the vertex towards `from`, and leaving along the one towards
    /// `to`, both unit vectors. A zero radius is the vertex.
    fn corner(&mut self, vertex: Point, from: Point, to: Point, corner: Sized) {
        let at = |(u, v): Point| {
            (
                vertex.0 + u * from.0 + v * to.0,
                vertex.1 + u * from.1 + v * to.1,
            )
        };
        if corner.radius <= 0.0 {
            self.line(vertex);
            return;
        }
        self.line(at((corner.reach(), 0.0)));
        for [a, b, p] in corner.shape.cubics(corner.radius) {
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

    /// How far a corner reaches along its edges, in points.
    fn reach_of(c: Corner, scale: f64) -> f64 {
        c.radius / scale * c.extent()
    }

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
    fn top_corners_are_rounded_and_bottom_corners_square() {
        let o = outline(Screen {
            notch: None,
            ..MACBOOK
        });
        let reach = reach_of(Bezel::MACBOOK_PRO_16.top, 2.0);
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
    fn a_top_corner_leaves_and_meets_its_edges_along_them() {
        // The top-right corner: three cubics that leave the top edge along
        // it, reach the side along it, and mirror about the corner's
        // diagonal.
        let o = outline(Screen {
            notch: None,
            ..MACBOOK
        });
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
    }

    /// Each smoothing's cubics, from the start of the curve to its end.
    fn corner_cubics(smoothing: f64) -> Vec<[Point; 4]> {
        let c = Corner::new(10.0, smoothing);
        let mut at = (c.extent() * 10.0, 0.0);
        c.cubics(10.0)
            .into_iter()
            .map(|[a, b, p]| {
                let cubic = [at, a, b, p];
                at = p;
                cubic
            })
            .collect()
    }

    #[test]
    fn no_smoothing_is_a_circular_corner() {
        let cubics = corner_cubics(0.0);
        assert_eq!(cubics.len(), 1);
        let [s, a, b, p] = cubics[0];
        assert!(near(s, (10.0, 0.0)) && near(p, (0.0, 10.0)));
        // Its middle is on the circle about (10, 10), to the 0.03% one
        // cubic per quarter circle allows.
        let mid = (
            (s.0 + 3.0 * a.0 + 3.0 * b.0 + p.0) / 8.0,
            (s.1 + 3.0 * a.1 + 3.0 * b.1 + p.1) / 8.0,
        );
        assert!(((mid.0 - 10.0).hypot(mid.1 - 10.0) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_smoothed_corner_is_an_arc_eased_onto_both_edges() {
        for smoothing in [0.5, 0.6, 0.78, 0.92] {
            let cubics = corner_cubics(smoothing);
            assert_eq!(cubics.len(), 3);
            let reach = 10.0 * (1.0 + smoothing);
            assert!(near(cubics[0][0], (reach, 0.0)));
            assert!(near(cubics[2][3], (0.0, reach)));
            // The arc's ends are on the corner's circle.
            for q in [cubics[1][0], cubics[1][3]] {
                assert!(
                    ((q.0 - 10.0).hypot(q.1 - 10.0) - 10.0).abs() < 1e-9,
                    "{q:?}"
                );
            }
            // No kink where one cubic meets the next.
            for pair in cubics.windows(2) {
                let (into, out) = (pair[0], pair[1]);
                let d0 = (into[3].0 - into[2].0, into[3].1 - into[2].1);
                let d1 = (out[1].0 - out[0].0, out[1].1 - out[0].1);
                let cross = d0.0 * d1.1 - d0.1 * d1.0;
                assert!(cross.abs() < 1e-9 * d0.0.hypot(d0.1) * d1.0.hypot(d1.1));
                assert!(d0.0 * d1.0 + d0.1 * d1.1 > 0.0);
            }
        }
        // All smoothing: the two easing curves meet with no arc between.
        assert_eq!(corner_cubics(1.0).len(), 2);
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
        let reach = reach_of(Bezel::MACBOOK_PRO_16.top, 1.92);
        assert!(near(o.start, (reach, 0.0)));
    }

    #[test]
    fn the_notch_is_traced_down_its_sides_and_along_its_bottom() {
        let n = Notch::from_areas(1728.0, 771.0, 956.0, 32.0);
        let o = outline(Screen {
            notch: Some(n),
            ..MACBOOK
        });
        let fillet = reach_of(Bezel::MACBOOK_PRO_16.notch_fillet, 2.0);
        let lower = reach_of(Bezel::MACBOOK_PRO_16.notch_corner, 2.0);
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
        let top_reach = reach_of(Bezel::MACBOOK_PRO_16.top, 2.0);
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
                let lower = reach_of(screen.bezel.notch_corner, screen.scale);
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

    /// The outline as a polygon, 64 chords a curve.
    fn polygon(o: &Outline) -> Vec<Point> {
        let mut at = o.start;
        let mut out = vec![at];
        for s in &o.segments {
            match *s {
                Segment::Line(p) => at = p,
                Segment::Curve(a, b, p) => {
                    for i in 1..64 {
                        let t = i as f64 / 64.0;
                        let m = 1.0 - t;
                        let f = |q0: f64, q1: f64, q2: f64, q3: f64| {
                            m * m * m * q0
                                + 3.0 * m * m * t * q1
                                + 3.0 * m * t * t * q2
                                + t * t * t * q3
                        };
                        out.push((f(at.0, a.0, b.0, p.0), f(at.1, a.1, b.1, p.1)));
                    }
                    at = p;
                }
            }
            out.push(at);
        }
        out
    }

    /// How far the outline runs past the glass at a point on the glass's
    /// edge: positive when the point is inside the outline, so the outline
    /// lies beyond it, over the bezel; negative for a gap.
    fn past(p: Point, polygon: &[Point]) -> f64 {
        let mut distance = f64::INFINITY;
        let mut inside = false;
        for pair in polygon.windows(2) {
            let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
            let (dx, dy) = (bx - ax, by - ay);
            let length = dx * dx + dy * dy;
            let t = if length > 0.0 {
                (((p.0 - ax) * dx + (p.1 - ay) * dy) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            distance = distance.min((p.0 - ax - t * dx).hypot(p.1 - ay - t * dy));
            if (ay > p.1) != (by > p.1) && p.0 < ax + (p.1 - ay) / dy * dx {
                inside = !inside;
            }
        }
        if inside { distance } else { -distance }
    }

    #[test]
    fn the_outline_stays_on_the_bezel_arts_glass() {
        // Points on the glass's edge in Apple's bezel art, near the top
        // corners and the notch, against the outline at one point per
        // panel pixel, with the notch NSScreen reports. The outline may
        // cross the glass's edge by no more than 0.1 px, and leave no more
        // than 0.5 px of it bare: 0.7 px on the 14-inch panel, whose notch
        // in the art is 0.36 px shallower than NSScreen's 32 points.
        let panels = [
            (
                include_str!("../testdata/bezel-16.txt"),
                (3456.0, 2234.0),
                (1542.0, 1912.0),
                0.5,
            ),
            (
                include_str!("../testdata/bezel-14.txt"),
                (3024.0, 1964.0),
                (1326.0, 1696.0),
                0.7,
            ),
        ];
        for (art, (width, height), (left, right), most_gap) in panels {
            let screen = Screen {
                width,
                height,
                scale: 1.0,
                bezel: Bezel::of_builtin((width as u32, height as u32), true),
                notch: Some(Notch::from_areas(width, left, right, 64.0)),
            };
            let polygon = polygon(&outline(screen));
            let edge: Vec<Point> = art
                .lines()
                .filter(|l| !l.starts_with('#'))
                .map(|l| {
                    let mut xy = l.split(' ').map(|v| v.parse::<f64>().unwrap());
                    (xy.next().unwrap(), xy.next().unwrap())
                })
                .collect();
            assert!(edge.len() > 150);
            let (mut over, mut gap) = ((0.0, (0.0, 0.0)), (0.0, (0.0, 0.0)));
            for p in edge {
                let d = past(p, &polygon);
                if d > over.0 {
                    over = (d, p);
                }
                if -d > gap.0 {
                    gap = (-d, p);
                }
            }
            assert!(over.0 <= 0.1, "{width}: {over:?} past the glass");
            assert!(gap.0 <= most_gap, "{width}: {gap:?} of glass bare");
        }
    }

    #[test]
    fn a_less_smoothed_top_corner_runs_past_the_glass() {
        // Smoothing 0.6, close to Apple's continuous corner, which the edge
        // used before, at the 39.75 px that fits the 16-inch top corner's
        // middle: the art's glass reaches the side later, so the outline
        // crosses its edge there.
        let screen = Screen {
            width: 3456.0,
            height: 2234.0,
            scale: 1.0,
            bezel: Bezel {
                top: Corner::new(39.75, 0.6),
                ..Bezel::SQUARE
            },
            notch: None,
        };
        let polygon = polygon(&outline(screen));
        let over = include_str!("../testdata/bezel-16.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                let mut xy = l.split(' ').map(|v| v.parse::<f64>().unwrap());
                (xy.next().unwrap(), xy.next().unwrap())
            })
            .filter(|p| p.0 < 100.0)
            .map(|p| past(p, &polygon))
            .fold(0.0, f64::max);
        assert!(over > 0.3, "{over}");
    }
}
