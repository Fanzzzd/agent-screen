//! Original Agent Pointer geometry and image-space placement. No OS calls,
//! clocks or display identity. The outline is authored here from cubic curves;
//! it does not contain an imported cursor asset or a traced third-party path.

use crate::model::Rect;

pub type Vertex = (f64, f64);
pub const TIP: Vertex = (0.0, 0.0);
pub const DESIGN_SIZE: f64 = 28.0;
pub const AXIS: Vertex = (7.0, 15.0);
pub const INNER_STROKE: f64 = 1.6;
pub const MIN_INNER_STROKE: f64 = 1.0;
pub const DARK_EDGE: f64 = 0.9;
pub const MIN_DARK_EDGE: f64 = 0.8;

#[derive(Clone, Copy, Debug)]
pub struct Cubic {
    pub control_a: Vertex,
    pub control_b: Vertex,
    pub to: Vertex,
}

// A soft leading cap, right shoulder, shallow concave neck, rounded heel
// and near-vertical left spine. Adjacent controls share tangent directions.
// The final cap returns to TIP; the tip itself is never rounded away.
pub const OUTLINE: [Cubic; 8] = [
    Cubic {
        control_a: (0.9, -0.42),
        control_b: (1.8, 0.3),
        to: (2.65, 1.1),
    },
    Cubic {
        control_a: (6.9, 5.1),
        control_b: (16.8, 14.36),
        to: (18.0, 15.5),
    },
    Cubic {
        control_a: (19.2, 16.64),
        control_b: (19.0, 18.1),
        to: (17.4, 18.25),
    },
    Cubic {
        control_a: (14.2, 18.55),
        control_b: (11.2, 18.4),
        to: (9.0, 19.2),
    },
    Cubic {
        control_a: (6.8, 20.0),
        control_b: (5.65, 23.4),
        to: (4.3, 24.0),
    },
    Cubic {
        control_a: (2.95, 24.6),
        control_b: (1.6, 24.0),
        to: (1.6, 22.8),
    },
    Cubic {
        control_a: (1.6, 17.2),
        control_b: (0.5, 6.0),
        to: (-0.1, 2.3),
    },
    Cubic {
        control_a: (-0.22, 1.56),
        control_b: (-0.9, 0.42),
        to: TIP,
    },
];

/// Constant screen-space widths after the curve has been transformed.
/// The outer dark edge and inner light rim keep the accent fill readable
/// on both light and dark content. Small sizes retain a one-pixel rim.
pub fn stroke_widths(size: f64) -> (f64, f64) {
    let inner = (INNER_STROKE * size / DESIGN_SIZE).max(MIN_INNER_STROKE);
    let outer = inner + (DARK_EDGE * size / DESIGN_SIZE).max(MIN_DARK_EDGE);
    (inner, outer)
}

/// The drawing caller supplies a finite hotspot/lean and positive finite
/// size/stretch, as the existing shared motion does. This transform does
/// not sanitize arbitrary input; invalid-image tests cover fit_image only.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub hotspot: Vertex,
    pub size: f64,
    pub stretch: f64,
    pub lean: f64,
}
impl Transform {
    /// The existing motion can lean and stretch the body, but its origin
    /// stays exactly at the animated hotspot. Press is included in size.
    pub fn map(self, (x, y): Vertex) -> Vertex {
        let axis = AXIS.1.atan2(AXIS.0);
        let (sin_a, cos_a) = axis.sin_cos();
        let (sin_t, cos_t) = (axis + self.lean).sin_cos();
        let scale = self.size / DESIGN_SIZE;
        let along = (x * cos_a + y * sin_a) * self.stretch * scale;
        let across = (-x * sin_a + y * cos_a) / self.stretch.sqrt() * scale;
        (
            self.hotspot.0 + along * cos_t - across * sin_t,
            self.hotspot.1 + along * sin_t + across * cos_t,
        )
    }

    /// Conservative control-hull bounds, including the full outer stroke.
    /// Cubic curves stay within this hull; stroke width is applied after
    /// the transform rather than stretched with the body.
    pub fn bounds(self) -> Rect {
        let mut min = self.map(TIP);
        let mut max = min;
        for segment in OUTLINE {
            for p in [segment.control_a, segment.control_b, segment.to] {
                let p = self.map(p);
                min = (min.0.min(p.0), min.1.min(p.1));
                max = (max.0.max(p.0), max.1.max(p.1));
            }
        }
        let half = stroke_widths(self.size).1 / 2.0;
        Rect::new(
            min.0 - half,
            min.1 - half,
            max.0 - min.0 + 2.0 * half,
            max.1 - min.1 + 2.0 * half,
        )
    }
}

/// The display image within a letterboxed Viewer or thumbnail.
pub fn fit_image(container: Rect, image: (f64, f64)) -> Option<Rect> {
    if ![
        container.x,
        container.y,
        container.width,
        container.height,
        image.0,
        image.1,
    ]
    .iter()
    .all(|n| n.is_finite())
        || container.width <= 0.0
        || container.height <= 0.0
        || image.0 <= 0.0
        || image.1 <= 0.0
    {
        return None;
    }
    let scale = (container.width / image.0).min(container.height / image.1);
    let (w, h) = (image.0 * scale, image.1 * scale);
    Some(Rect::new(
        container.x + (container.width - w) / 2.0,
        container.y + (container.height - h) / 2.0,
        w,
        h,
    ))
}

/// Do not move the hotspot to keep the body on screen. At an edge the
/// image mask clips the body, while the tip keeps the requested position.
pub fn image_point(fitted: Rect, image: (f64, f64), point: Vertex) -> Vertex {
    (
        fitted.x + point.0 * fitted.width / image.0,
        fitted.y + point.1 * fitted.height / image.1,
    )
}

pub fn clip_bounds(bounds: Rect, clip: Rect) -> Option<Rect> {
    let (x, y) = (bounds.x.max(clip.x), bounds.y.max(clip.y));
    let (right, bottom) = (
        bounds.max_x().min(clip.max_x()),
        bounds.max_y().min(clip.max_y()),
    );
    (right > x && bottom > y).then(|| Rect::new(x, y, right - x, bottom - y))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-8, "{a} != {b}");
    }
    fn at(from: Vertex, curve: Cubic, t: f64) -> Vertex {
        let u = 1.0 - t;
        (
            u * u * u * from.0
                + 3.0 * u * u * t * curve.control_a.0
                + 3.0 * u * t * t * curve.control_b.0
                + t * t * t * curve.to.0,
            u * u * u * from.1
                + 3.0 * u * u * t * curve.control_a.1
                + 3.0 * u * t * t * curve.control_b.1
                + t * t * t * curve.to.1,
        )
    }

    #[test]
    fn original_outline_is_closed_and_has_smooth_tangent_joins() {
        assert_eq!(OUTLINE.last().unwrap().to, TIP);
        for i in 0..OUTLINE.len() {
            let current = OUTLINE[i];
            let next = OUTLINE[(i + 1) % OUTLINE.len()];
            let a = (
                current.to.0 - current.control_b.0,
                current.to.1 - current.control_b.1,
            );
            let b = (
                next.control_a.0 - current.to.0,
                next.control_a.1 - current.to.1,
            );
            let lengths = a.0.hypot(a.1) * b.0.hypot(b.1);
            assert!(lengths > 0.0);
            near((a.0 * b.1 - a.1 * b.0) / lengths, 0.0);
            assert!(a.0 * b.0 + a.1 * b.1 > 0.0, "join must not reverse tangent");
        }
    }

    #[test]
    fn rounded_tip_remains_the_leading_point_not_an_inset_virtual_corner() {
        let mut from = TIP;
        for curve in OUTLINE {
            for i in 0..=100 {
                let p = at(from, curve, i as f64 / 100.0);
                assert!(p.0 * AXIS.0 + p.1 * AXIS.1 >= -1e-8);
            }
            from = curve.to;
        }
    }

    #[test]
    fn sizes_18_24_32_keep_the_hotspot_and_uniform_stroke_metrics() {
        for size in [18.0, 24.0, 32.0] {
            let t = Transform {
                hotspot: (123.5, 77.25),
                size,
                stretch: 1.0,
                lean: 0.0,
            };
            assert_eq!(t.map(TIP), t.hotspot);
            let (inner, outer) = stroke_widths(size);
            assert!(inner >= 1.0 && outer > inner);
            assert!(t.bounds().width <= size && t.bounds().height <= size);
            let mut from = TIP;
            for curve in OUTLINE {
                for i in 0..=20 {
                    let p = t.map(at(from, curve, i as f64 / 20.0));
                    let b = t.bounds();
                    assert!(p.0 >= b.x && p.0 <= b.max_x() && p.1 >= b.y && p.1 <= b.max_y());
                }
                from = curve.to;
            }
        }
    }

    #[test]
    fn existing_press_stretch_and_lean_never_shift_the_tip() {
        for size in [18.0, 24.0, 32.0] {
            for scale in [0.55, 1.0, 2.0] {
                for press in [0.84, 1.0, 1.05] {
                    for lean in [-0.30, 0.0, 0.30] {
                        let t = Transform {
                            hotspot: (100.0, 50.0),
                            size: size * scale * press,
                            stretch: 1.08,
                            lean,
                        };
                        assert_eq!(t.map(TIP), t.hotspot);
                        assert!(t.bounds().width.is_finite() && t.bounds().height.is_finite());
                    }
                }
            }
        }
    }

    #[test]
    fn aspect_fit_maps_the_requested_image_point_not_the_letterbox() {
        let fitted = fit_image(Rect::new(100.0, 50.0, 200.0, 200.0), (1280.0, 800.0)).unwrap();
        assert_eq!(fitted, Rect::new(100.0, 87.5, 200.0, 125.0));
        assert_eq!(image_point(fitted, (1280.0, 800.0), TIP), (100.0, 87.5));
        assert_eq!(
            image_point(fitted, (1280.0, 800.0), (640.0, 400.0)),
            fitted.mid()
        );
        assert_eq!(
            image_point(fitted, (1280.0, 800.0), (1280.0, 800.0)),
            (300.0, 212.5)
        );
    }

    #[test]
    fn viewport_edges_clip_the_body_without_repositioning_the_hotspot() {
        for viewport in [
            Rect::new(0.0, 0.0, 400.0, 200.0),
            Rect::new(25.0, 50.0, 80.0, 160.0),
        ] {
            let fit = fit_image(viewport, (1280.0, 800.0)).unwrap();
            let mask = clip_bounds(fit, viewport).unwrap();
            assert_eq!(mask, fit);
            for p in [TIP, (1279.0, 0.0), (0.0, 799.0), (1279.0, 799.0)] {
                let hotspot = image_point(fit, (1280.0, 800.0), p);
                for size in [18.0, 24.0, 32.0] {
                    let t = Transform {
                        hotspot,
                        size,
                        stretch: 1.08,
                        lean: 0.3,
                    };
                    assert_eq!(t.map(TIP), hotspot);
                    if let Some(visible) = clip_bounds(t.bounds(), mask) {
                        assert!(visible.x >= mask.x && visible.y >= mask.y);
                        assert!(visible.max_x() <= mask.max_x() && visible.max_y() <= mask.max_y());
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_image_sizes_do_not_produce_nan_placement() {
        for size in [(0.0, 800.0), (1280.0, -1.0), (f64::NAN, 800.0)] {
            assert!(fit_image(Rect::new(0.0, 0.0, 200.0, 100.0), size).is_none());
        }
        assert!(
            clip_bounds(
                Rect::new(0.0, 0.0, 10.0, 10.0),
                Rect::new(20.0, 20.0, 10.0, 10.0)
            )
            .is_none()
        );
    }
}
