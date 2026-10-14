//! The Agent Pointer over a display image: a rounded arrow that arcs to
//! each point the agent acts on, leaning into the move, presses on clicks
//! with a ripple, and sways a little while the agent thinks. It lives in
//! the Viewer and Peek only, never on the display, so agent screenshots
//! never contain it.
//!
//! The look and the motion follow Codex's computer-use cursor.

use crate::model::{AgentPointer, DisplayId};
use gpui::{
    App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled, Window, canvas,
    linear_color_stop, linear_gradient, point, px,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::time::{Duration, Instant};

/// A move takes longer the farther it goes, within these bounds.
const GLIDE_MIN: f64 = 0.16;
const GLIDE_MAX: f64 = 0.40;
/// How far, as a share of the move, the arc bows out from a straight line.
const BOW: f64 = 0.14;
/// The arrow leans into a move by up to this much, in radians.
const LEAN: f32 = 0.24;
/// A click presses the arrow in, then lets it spring back.
const PRESS: Duration = Duration::from_millis(260);
const RIPPLE: Duration = Duration::from_millis(480);
/// After a move the arrow sways while the agent thinks, then rests, so an
/// idle Viewer doesn't redraw for nothing.
const SWAY: Duration = Duration::from_secs(5);

/// The arrow fades in when an agent's pointer appears and out when it
/// goes, as on a Takeover or at the end.
const FADE: Duration = Duration::from_millis(200);

/// Where the arrow is heading, in display points. Shared by every view of
/// a display so Peek and the Viewer agree.
struct Glide {
    seq: u64,
    from: (f64, f64),
    to: (f64, f64),
    start: Instant,
    duration: Duration,
    /// Which side the arc bows to: moves alternate, so a run of clicks
    /// doesn't trace the same curve.
    bow: f64,
    click: bool,
    /// Fading toward visible, from `fade_from` opacity at `fade_start`.
    shown: bool,
    fade_from: f32,
    fade_start: Instant,
}

impl Glide {
    fn opacity(&self, now: Instant) -> f32 {
        let t = ((now - self.fade_start).as_secs_f32() / FADE.as_secs_f32()).min(1.0);
        let to = if self.shown { 1.0 } else { 0.0 };
        self.fade_from + (to - self.fade_from) * t
    }

    fn fade(&mut self, shown: bool, now: Instant) {
        if self.shown != shown {
            self.fade_from = self.opacity(now);
            self.fade_start = now;
            self.shown = shown;
        }
    }

    /// The arc's control point.
    fn control(&self) -> (f64, f64) {
        let (dx, dy) = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        (
            (self.from.0 + self.to.0) / 2.0 - dy * BOW * self.bow,
            (self.from.1 + self.to.1) / 2.0 + dx * BOW * self.bow,
        )
    }

    /// Position on the arc at eased progress `e`.
    fn at(&self, e: f64) -> (f64, f64) {
        let c = self.control();
        let u = 1.0 - e;
        (
            u * u * self.from.0 + 2.0 * u * e * c.0 + e * e * self.to.0,
            u * u * self.from.1 + 2.0 * u * e * c.1 + e * e * self.to.1,
        )
    }

    fn progress(&self, now: Instant) -> f64 {
        ((now - self.start).as_secs_f64() / self.duration.as_secs_f64()).min(1.0)
    }
}

thread_local! {
    static GLIDES: RefCell<HashMap<DisplayId, Glide>> = RefCell::new(HashMap::new());
}

fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

fn glide_time(from: (f64, f64), to: (f64, f64)) -> Duration {
    let distance = (to.0 - from.0).hypot(to.1 - from.1);
    Duration::from_secs_f64((0.18 + distance / 1200.0).clamp(GLIDE_MIN, GLIDE_MAX))
}

/// The arrow now. None once it has faded out.
struct Frame {
    at: (f64, f64),
    /// Extra rotation, radians, clockwise.
    lean: f32,
    /// Stretch along the arrow's length; it narrows to match.
    stretch: f32,
    /// Overall size, below 1 while a click presses it.
    press: f32,
    ripple: Option<f32>,
    opacity: f32,
    changing: bool,
}

fn advance(display: DisplayId, pointer: Option<AgentPointer>) -> Option<Frame> {
    GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        let now = Instant::now();
        let glide = match pointer {
            Some(pointer) => {
                let target = (pointer.x, pointer.y);
                let glide = glides.entry(display).or_insert(Glide {
                    seq: pointer.seq,
                    from: target,
                    to: target,
                    start: now - Duration::from_secs(60),
                    duration: Duration::from_secs_f64(GLIDE_MIN),
                    bow: 1.0,
                    click: false,
                    shown: true,
                    fade_from: 0.0,
                    fade_start: now,
                });
                glide.fade(true, now);
                if glide.seq != pointer.seq {
                    let here = glide.at(ease_in_out(glide.progress(now)));
                    glide.seq = pointer.seq;
                    glide.from = here;
                    glide.to = target;
                    glide.start = now;
                    glide.duration = glide_time(here, target);
                    glide.bow = -glide.bow;
                    glide.click = pointer.click;
                }
                glide
            }
            // Gone: fade out where it last was.
            None => {
                let glide = glides.get_mut(&display)?;
                glide.fade(false, now);
                glide
            }
        };
        let opacity = glide.opacity(now);
        let fading = now - glide.fade_start < FADE;
        if opacity <= 0.0 && !fading {
            return None;
        }
        let t = glide.progress(now);
        let mut at = glide.at(ease_in_out(t));
        // Leaning and stretching peak mid-move and settle on arrival.
        let swing = (PI * t as f32).sin();
        let (dx, dy) = (glide.to.0 - glide.from.0, glide.to.1 - glide.from.1);
        let across = (dx / (dx.abs() + dy.abs()).max(1.0)) as f32;
        let mut lean = LEAN * across * swing;
        let stretch = 1.0 + 0.10 * swing * ((dx.hypot(dy) / 120.0).min(1.0) as f32);

        let arrived = glide.start + glide.duration;
        let since = now.saturating_duration_since(arrived);
        let landed = t >= 1.0;
        let press = if landed && glide.click && since < PRESS {
            let p = since.as_secs_f32() / PRESS.as_secs_f32();
            // In to 84% over the first third, then a small overshoot back.
            if p < 0.3 {
                1.0 - 0.16 * (p / 0.3)
            } else {
                let q = (p - 0.3) / 0.7;
                0.84 + 0.16 * q + 0.05 * (PI * q).sin()
            }
        } else {
            1.0
        };
        let ripple = (landed && glide.click && since < RIPPLE)
            .then(|| since.as_secs_f32() / RIPPLE.as_secs_f32());
        let swaying = landed && since < SWAY && glide.shown;
        if swaying {
            // A slow drift with a nod every couple of seconds, fading in
            // so it starts from rest.
            let phase = since.as_secs_f32() * 2.4;
            let ramp = (since.as_secs_f32() / 0.6).min(1.0)
                * (1.0 - ((since.as_secs_f32() - 4.0) / 1.0).clamp(0.0, 1.0));
            let cycle = phase % 5.2;
            let nod = if (2.9..3.45).contains(&cycle) {
                ease_in_out(((cycle - 2.9) / 0.55) as f64) as f32
            } else if (3.45..3.95).contains(&cycle) {
                1.0
            } else if (3.95..4.55).contains(&cycle) {
                1.0 - ease_in_out(((cycle - 3.95) / 0.6) as f64) as f32
            } else {
                0.0
            };
            at.0 += ((phase * 0.35).cos() * 1.1 + nod * 0.9) as f64 * ramp as f64;
            at.1 += ((phase * 0.48).sin() * 0.45 - nod * 0.35) as f64 * ramp as f64;
            lean += ((phase * 0.42).sin() * 0.03 + nod * 0.22) * ramp;
        }
        Some(Frame {
            at,
            lean,
            stretch,
            press,
            ripple,
            opacity,
            changing: !landed || fading || swaying || since < RIPPLE.max(PRESS),
        })
    })
}

pub fn forget(display: DisplayId) {
    GLIDES.with(|g| g.borrow_mut().remove(&display));
}

/// A layer the size of the display image that draws the pointer.
pub fn layer(
    display: DisplayId,
    pointer: Option<AgentPointer>,
    size: (f64, f64),
    accent: Hsla,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut App| {
            let Some(frame) = advance(display, pointer) else {
                return;
            };
            // The image is aspect-fit inside the layer.
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let fit = (w / size.0 as f32).min(h / size.1 as f32);
            let bounds = Bounds::new(
                point(
                    bounds.origin.x + px((w - size.0 as f32 * fit) / 2.),
                    bounds.origin.y + px((h - size.1 as f32 * fit) / 2.),
                ),
                gpui::size(px(size.0 as f32 * fit), px(size.1 as f32 * fit)),
            );
            let at = frame.at;
            let width = f32::from(bounds.size.width) as f64;
            let tip = point(
                bounds.origin.x + px((at.0 / size.0 * width) as f32),
                bounds.origin.y + px((at.1 / size.1 * f32::from(bounds.size.height) as f64) as f32),
            );
            // Full size in the Viewer, smaller on thumbnails.
            let scale = (width / size.0 * 1.6).clamp(0.55, 1.0) as f32;
            if let Some(progress) = frame.ripple {
                draw_ripple(window, tip, scale, progress, accent, frame.opacity);
            }
            draw_arrow(window, tip, scale, &frame, accent);
            if frame.changing {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The arrow's corners with the tip at (0, 0), y down, and each corner's
/// rounding radius: a tip, a wing, a notch between the wings, a wing.
const SHAPE: [((f32, f32), f32); 4] = [
    ((0.0, 0.0), 1.4),
    ((26.4, 7.6), 5.0),
    ((13.4, 12.0), 3.0),
    ((10.2, 24.8), 5.0),
];
/// The arrow's length runs from the tip toward the notch.
const AXIS: (f32, f32) = (13.4, 12.0);

/// The rounded outline, mapped through `map`.
fn outline(builder: &mut PathBuilder, map: &impl Fn((f32, f32)) -> Point<Pixels>) {
    let n = SHAPE.len();
    let toward = |a: (f32, f32), b: (f32, f32), r: f32| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(0.001);
        (a.0 + dx / len * r, a.1 + dy / len * r)
    };
    for i in 0..n {
        let (corner, r) = SHAPE[i];
        let prev = SHAPE[(i + n - 1) % n].0;
        let next = SHAPE[(i + 1) % n].0;
        let enter = toward(corner, prev, r);
        let leave = toward(corner, next, r);
        if i == 0 {
            builder.move_to(map(enter));
        } else {
            builder.line_to(map(enter));
        }
        builder.curve_to(map(leave), map(corner));
    }
    builder.close();
}

fn draw_arrow(window: &mut Window, tip: Point<Pixels>, scale: f32, frame: &Frame, accent: Hsla) {
    let opacity = frame.opacity;
    let axis = AXIS.1.atan2(AXIS.0);
    let (along, across) = (frame.stretch, 1.0 / frame.stretch.sqrt());
    let size = scale * frame.press;
    let (sin_a, cos_a) = axis.sin_cos();
    let (sin_t, cos_t) = (axis + frame.lean).sin_cos();
    // Stretch along the axis, then turn by the lean, about the tip.
    let map = |(x, y): (f32, f32), dy: f32| {
        let u = (x * cos_a + y * sin_a) * along * size;
        let v = (-x * sin_a + y * cos_a) * across * size;
        point(
            tip.x + px(u * cos_t - v * sin_t),
            tip.y + px(u * sin_t + v * cos_t + dy),
        )
    };

    // A soft shadow: the shape below the arrow, then widening halos.
    for (width, alpha) in [(0.0, 0.16), (2.0, 0.08), (4.0, 0.05), (6.5, 0.03)] {
        let mut shadow = if width == 0.0 {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(px(width * scale))
        };
        outline(&mut shadow, &|p| map(p, 1.6 * scale));
        if let Ok(path) = shadow.build() {
            window.paint_path(path, gpui::black().opacity(alpha * opacity));
        }
    }

    // Deep accent at the tip, washing toward a pale, warmer tint at the
    // wings, as Codex's cursor runs from cyan to a pink white.
    let deep = Hsla {
        l: accent.l * 0.92,
        ..accent
    };
    let pale = Hsla {
        h: (accent.h + 0.1) % 1.0,
        s: accent.s * 0.85,
        l: 0.9,
        ..accent
    };
    let mid = Hsla {
        l: accent.l + (1.0 - accent.l) * 0.3,
        ..accent
    };
    let heading = (axis + frame.lean).to_degrees() + 90.0;
    let mut fill = PathBuilder::fill();
    outline(&mut fill, &|p| map(p, 0.0));
    if let Ok(path) = fill.build() {
        window.paint_path(
            path,
            linear_gradient(
                heading,
                linear_color_stop(deep.opacity(opacity), 0.1),
                linear_color_stop(pale.opacity(opacity), 0.95),
            ),
        );
    }
    // A brighter sheen across the middle gives it depth.
    let mut sheen = PathBuilder::fill();
    outline(&mut sheen, &|(x, y)| {
        map((x * 0.6 + 4.0, y * 0.6 + 3.4), 0.0)
    });
    if let Ok(path) = sheen.build() {
        window.paint_path(
            path,
            linear_gradient(
                heading,
                linear_color_stop(mid.opacity(0.0), 0.0),
                linear_color_stop(gpui::white().opacity(0.45 * opacity), 1.0),
            ),
        );
    }
    let mut rim = PathBuilder::stroke(px(1.6 * scale));
    outline(&mut rim, &|p| map(p, 0.0));
    if let Ok(path) = rim.build() {
        window.paint_path(path, gpui::white().opacity(0.95 * opacity));
    }
}

/// A ring that spreads from the tip.
fn draw_ripple(
    window: &mut Window,
    tip: Point<Pixels>,
    scale: f32,
    progress: f32,
    accent: Hsla,
    opacity: f32,
) {
    let eased = 1.0 - (1.0 - progress).powi(3);
    let radius = 20.0 * scale * (0.15 + 0.85 * eased);
    let alpha = opacity * (1.0 - progress).powf(1.4);
    let circle = |builder: &mut PathBuilder| {
        let r = px(radius);
        builder.move_to(point(tip.x + r, tip.y));
        builder.arc_to(point(r, r), px(0.), false, true, point(tip.x - r, tip.y));
        builder.arc_to(point(r, r), px(0.), false, true, point(tip.x + r, tip.y));
        builder.close();
    };
    let mut fill = PathBuilder::fill();
    circle(&mut fill);
    if let Ok(path) = fill.build() {
        window.paint_path(path, accent.opacity(0.16 * alpha));
    }
    let mut ring = PathBuilder::stroke(px(1.8 * scale));
    circle(&mut ring);
    if let Ok(path) = ring.build() {
        window.paint_path(path, accent.opacity(0.9 * alpha));
    }
}
