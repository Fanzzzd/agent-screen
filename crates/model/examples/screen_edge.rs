//! Print a screen's edge outline as SVG path data, for checking it against
//! a bezel image (`tests/e2e/tools/edgefit.swift`).
//!
//! `cargo run -p agent-screen-model --example screen_edge -- WIDTH HEIGHT
//! SCALE PANEL_WIDTH PANEL_HEIGHT [NOTCH_LEFT NOTCH_RIGHT NOTCH_DEPTH]`
//!
//! Sizes in points, SCALE in panel pixels per point; the notch as NSScreen
//! reports it: where the top-left area ends and the top-right one starts.
//! With SCALE 1 and the panel's size, the path is in panel pixels.

use agent_screen_model::screen_edge::{Bezel, Notch, Screen, Segment, outline};

fn main() {
    let args: Vec<f64> = std::env::args()
        .skip(1)
        .map(|a| a.parse().expect("numbers"))
        .collect();
    let [width, height, scale, panel_width, panel_height, ..] = args[..] else {
        panic!("WIDTH HEIGHT SCALE PANEL_WIDTH PANEL_HEIGHT [NOTCH_LEFT NOTCH_RIGHT NOTCH_DEPTH]");
    };
    let notch = match args[5..] {
        [left, right, depth] => Some(Notch::from_areas(width, left, right, depth)),
        _ => None,
    };
    let panel = (panel_width as u32, panel_height as u32);
    let o = outline(Screen {
        width,
        height,
        scale,
        bezel: Bezel::of_builtin(panel, notch.is_some()),
        notch,
    });
    let mut d = format!("M {} {}", o.start.0, o.start.1);
    for s in &o.segments {
        d += &match *s {
            Segment::Line((x, y)) => format!(" L {x} {y}"),
            Segment::Curve(a, b, p) => {
                format!(" C {} {} {} {} {} {}", a.0, a.1, b.0, b.1, p.0, p.1)
            }
        };
    }
    println!("{d} Z");
}
