//! The Agent Pointer over a display image: a rounded arrow that arcs to
//! each point the agent acts on, leaning into the move, presses on clicks
//! with a ripple, and sways a little while the agent thinks. It lives in
//! the Viewer and Peek only, never on the display, so agent screenshots
//! never contain it.
//!
//! The original cubic outline has a soft cap, shoulder and heel. Constant
//! screen-space light and dark strokes keep its accent fill readable.
//! Existing glide, lean, press, ripple and display-shared motion are reused.

use crate::model::{AgentPointer, DisplayId, Rect};
use agent_screen_model::pointer_geometry as geometry;
use gpui::{
    App, Bounds, ContentMask, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled, Window,
    canvas, linear_color_stop, linear_gradient, point, px,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::time::{Duration, Instant};

/// A move takes longer the farther it goes, within these bounds: short
/// enough that the arrow lands before the click it leads to shows.
const GLIDE_MIN: f64 = 0.09;
const GLIDE_MAX: f64 = 0.22;
/// A move that starts before the last one landed takes this share of the
/// time, so a run of quick clicks doesn't leave the arrow behind.
const CATCH_UP: f64 = 0.65;
/// How far, as a share of the move, the arc bows out from a straight line.
const BOW: f64 = 0.14;
/// The arrow turns with a move by up to this much, in radians, as if its
/// tail trailed behind the tip.
const LEAN: f32 = 0.30;
/// The speed, in display points per second, at which it turns fully.
const LEAN_SPEED: f64 = 2400.0;
/// The turn follows a spring of this frequency, in Hz, and damping ratio,
/// so it settles with a small wobble when the arrow stops.
const SPRING_HZ: f32 = 5.0;
const SPRING_DAMPING: f32 = 0.5;
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
    /// The turn's spring: angle, its rate, and when it was last stepped.
    lean: f32,
    lean_rate: f32,
    stepped: Instant,
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

/// Fast off the mark, soft on landing.
fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
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
    Duration::from_secs_f64((0.06 + distance.sqrt() * 0.006).clamp(GLIDE_MIN, GLIDE_MAX))
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
                    lean: 0.0,
                    lean_rate: 0.0,
                    stepped: now,
                });
                glide.fade(true, now);
                if glide.seq != pointer.seq {
                    let moving = glide.progress(now) < 1.0;
                    let here = glide.at(ease_out(glide.progress(now)));
                    glide.seq = pointer.seq;
                    glide.from = here;
                    glide.to = target;
                    glide.start = now;
                    glide.duration = glide_time(here, target);
                    if moving {
                        glide.duration = glide.duration.mul_f64(CATCH_UP);
                    }
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
        let mut at = glide.at(ease_out(t));
        // Velocity, from a step along the arc.
        let step = 0.004;
        let ahead = glide.at(ease_out((t + step / glide.duration.as_secs_f64()).min(1.0)));
        let (vx, vy) = if t < 1.0 {
            ((ahead.0 - at.0) / step, (ahead.1 - at.1) / step)
        } else {
            (0.0, 0.0)
        };
        let speed = vx.hypot(vy);
        let pace = (speed / LEAN_SPEED).min(1.0) as f32;
        // The tail swings toward where the arrow came from: the turn is
        // the sine of the angle between the arrow's axis and the trail.
        let target = if speed > 1.0 {
            let axis = AXIS.1.atan2(AXIS.0);
            let trail = (-vy as f32).atan2(-vx as f32);
            LEAN * (trail - axis).sin() * pace
        } else {
            0.0
        };
        let dt = (now - glide.stepped).as_secs_f32().min(0.05);
        glide.stepped = now;
        let omega = 2.0 * PI * SPRING_HZ;
        // Semi-implicit Euler in small steps keeps the spring stable.
        let steps = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let accel = omega * omega * (target - glide.lean)
                - 2.0 * SPRING_DAMPING * omega * glide.lean_rate;
            glide.lean_rate += accel * h;
            glide.lean += glide.lean_rate * h;
        }
        let mut lean = glide.lean;
        let settling = glide.lean.abs() > 0.002 || glide.lean_rate.abs() > 0.02;
        let stretch = 1.0 + 0.08 * pace;

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
            changing: !landed || fading || swaying || settling || since < RIPPLE.max(PRESS),
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
            // The same aspect-fit geometry is tested without WindowServer.
            let container = Rect::new(
                f32::from(bounds.origin.x) as f64,
                f32::from(bounds.origin.y) as f64,
                f32::from(bounds.size.width) as f64,
                f32::from(bounds.size.height) as f64,
            );
            let Some(fitted) = geometry::fit_image(container, size) else {
                return;
            };
            let Some(clip) = geometry::clip_bounds(fitted, container) else {
                return;
            };
            let at = geometry::image_point(fitted, size, frame.at);
            let tip = point(px(at.0 as f32), px(at.1 as f32));
            // Keep the existing Viewer/thumbnail size policy.
            let scale = (fitted.width / size.0 * 1.6).clamp(0.55, 1.0) as f32;
            let mask = ContentMask {
                bounds: Bounds::new(
                    point(px(clip.x as f32), px(clip.y as f32)),
                    gpui::size(px(clip.width as f32), px(clip.height as f32)),
                ),
            };
            window.with_content_mask(Some(mask), |window| {
                if let Some(progress) = frame.ripple {
                    draw_ripple(window, tip, scale, progress, accent, frame.opacity);
                }
                draw_arrow(window, tip, scale, &frame, accent);
            });
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

/// Motion follows the original geometry's axis, with no new clock or state.
const AXIS: (f32, f32) = (geometry::AXIS.0 as f32, geometry::AXIS.1 as f32);

fn outline(builder: &mut PathBuilder, map: &impl Fn(geometry::Vertex) -> Point<Pixels>) {
    builder.move_to(map(geometry::TIP));
    for curve in geometry::OUTLINE {
        builder.cubic_bezier_to(map(curve.to), map(curve.control_a), map(curve.control_b));
    }
    builder.close();
}

fn draw_arrow(window: &mut Window, tip: Point<Pixels>, scale: f32, frame: &Frame, accent: Hsla) {
    let opacity = frame.opacity;
    let transform = geometry::Transform {
        hotspot: (f32::from(tip.x) as f64, f32::from(tip.y) as f64),
        size: geometry::DESIGN_SIZE * (scale * frame.press) as f64,
        stretch: frame.stretch as f64,
        lean: frame.lean as f64,
    };
    let map = |p: geometry::Vertex, dy: f32| {
        let p = transform.map(p);
        point(px(p.0 as f32), px(p.1 as f32 + dy))
    };
    let (inner, outer) = geometry::stroke_widths(transform.size);
    // Strokes are applied after stretching the curve, keeping their screen
    // width uniform around its cap, concave neck and rounded heel.
    let stroke = |window: &mut Window, width: f64, dy: f32, color: Hsla| {
        let mut path = PathBuilder::stroke(px(width as f32));
        outline(&mut path, &|p| map(p, dy));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    };
    for (extra, alpha) in [(7.0, 0.035), (4.0, 0.06), (1.8, 0.10)] {
        stroke(
            window,
            outer + extra * scale as f64,
            2.4 * scale,
            gpui::black().opacity(alpha * opacity),
        );
    }
    for (extra, alpha) in [(5.0, 0.05), (2.5, 0.08)] {
        stroke(
            window,
            outer + extra * scale as f64,
            0.0,
            accent.opacity(alpha * opacity),
        );
    }
    // A dark outer edge reads on light content; the light rim reads on dark.
    stroke(window, outer, 0.0, gpui::black().opacity(0.82 * opacity));
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
    let heading = (AXIS.1.atan2(AXIS.0) + frame.lean).to_degrees() + 90.0;
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
    stroke(window, inner, 0.0, gpui::white().opacity(0.95 * opacity));
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
