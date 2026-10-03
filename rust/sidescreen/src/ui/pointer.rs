//! The Agent Pointer over a display image: an arrow that glides to each
//! point the agent acts on, with a ripple on clicks. It lives in the Viewer
//! and Peek only, never on the display, so agent screenshots never contain
//! it.

use crate::model::{AgentPointer, DisplayId};
use gpui::{
    App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled, Window, canvas, point, px,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const GLIDE: Duration = Duration::from_millis(320);
/// The ripple starts once the arrow has about arrived.
const RIPPLE_DELAY: Duration = Duration::from_millis(180);
const RIPPLE: Duration = Duration::from_millis(500);

/// Where the arrow is heading, in display points. Shared by every view of
/// a display so Peek and the Viewer agree.
struct Glide {
    seq: u64,
    from: (f64, f64),
    to: (f64, f64),
    start: Instant,
    click: bool,
}

thread_local! {
    static GLIDES: RefCell<HashMap<DisplayId, Glide>> = RefCell::new(HashMap::new());
}

fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// Position now, and whether anything is still moving.
fn advance(display: DisplayId, pointer: AgentPointer) -> ((f64, f64), Option<f64>, bool) {
    GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        let now = Instant::now();
        let target = (pointer.x, pointer.y);
        let glide = glides.entry(display).or_insert(Glide {
            seq: pointer.seq,
            from: target,
            to: target,
            start: now - GLIDE - RIPPLE,
            click: false,
        });
        if glide.seq != pointer.seq {
            let t = ease_out((now - glide.start).as_secs_f64() / GLIDE.as_secs_f64()).min(1.0);
            let here = (
                glide.from.0 + (glide.to.0 - glide.from.0) * t,
                glide.from.1 + (glide.to.1 - glide.from.1) * t,
            );
            *glide = Glide {
                seq: pointer.seq,
                from: here,
                to: target,
                start: now,
                click: pointer.click,
            };
        }
        let elapsed = now - glide.start;
        let t = ease_out((elapsed.as_secs_f64() / GLIDE.as_secs_f64()).min(1.0));
        let at = (
            glide.from.0 + (glide.to.0 - glide.from.0) * t,
            glide.from.1 + (glide.to.1 - glide.from.1) * t,
        );
        let ripple = (glide.click && elapsed >= RIPPLE_DELAY && elapsed < RIPPLE_DELAY + RIPPLE)
            .then(|| (elapsed - RIPPLE_DELAY).as_secs_f64() / RIPPLE.as_secs_f64());
        let moving = elapsed < RIPPLE_DELAY + RIPPLE;
        (at, ripple, moving)
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
            let Some(pointer) = pointer else { return };
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
            let (at, ripple, moving) = advance(display, pointer);
            let width = f32::from(bounds.size.width) as f64;
            let tip = point(
                bounds.origin.x + px((at.0 / size.0 * width) as f32),
                bounds.origin.y + px((at.1 / size.1 * f32::from(bounds.size.height) as f64) as f32),
            );
            // Full size in the Viewer, smaller on thumbnails.
            let scale = (width / size.0 * 1.6).clamp(0.55, 1.0) as f32;
            if let Some(progress) = ripple {
                draw_ripple(window, tip, scale, progress as f32, accent);
            }
            draw_arrow(window, tip, scale, accent);
            if moving {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The arrow, tip at `tip`, in the shape of the system cursor.
fn draw_arrow(window: &mut Window, tip: Point<Pixels>, scale: f32, accent: Hsla) {
    // Outline points with the tip at (0, 0), y down.
    const SHAPE: [(f32, f32); 7] = [
        (0., 0.),
        (0., 18.),
        (4.6, 13.6),
        (7.8, 21.),
        (10.6, 19.8),
        (7.5, 12.6),
        (13.5, 12.6),
    ];
    let at = |(x, y): (f32, f32)| point(tip.x + px(x * scale), tip.y + px(y * scale));
    let outline = |builder: &mut PathBuilder| {
        builder.move_to(at(SHAPE[0]));
        for p in &SHAPE[1..] {
            builder.line_to(at(*p));
        }
        builder.close();
    };
    // A soft shadow one point below.
    let mut shadow = PathBuilder::fill();
    let shadow_at = |(x, y): (f32, f32)| point(tip.x + px(x * scale), tip.y + px(y * scale + 1.0));
    shadow.move_to(shadow_at(SHAPE[0]));
    for p in &SHAPE[1..] {
        shadow.line_to(shadow_at(*p));
    }
    shadow.close();
    if let Ok(path) = shadow.build() {
        window.paint_path(path, gpui::black().opacity(0.3));
    }
    let mut fill = PathBuilder::fill();
    outline(&mut fill);
    if let Ok(path) = fill.build() {
        window.paint_path(path, accent);
    }
    let mut stroke = PathBuilder::stroke(px(1.5 * scale));
    outline(&mut stroke);
    if let Ok(path) = stroke.build() {
        window.paint_path(path, gpui::white());
    }
}

/// A ring that spreads from the tip.
fn draw_ripple(window: &mut Window, tip: Point<Pixels>, scale: f32, progress: f32, accent: Hsla) {
    let eased = 1.0 - (1.0 - progress).powi(2);
    let radius = 17.0 * scale * (0.2 + 0.8 * eased);
    let alpha = if progress < 0.4 {
        1.0
    } else {
        1.0 - (progress - 0.4) / 0.6
    };
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
        window.paint_path(path, accent.opacity(0.18 * alpha));
    }
    let mut ring = PathBuilder::stroke(px(2.0));
    circle(&mut ring);
    if let Ok(path) = ring.build() {
        window.paint_path(path, accent.opacity(alpha));
    }
}
