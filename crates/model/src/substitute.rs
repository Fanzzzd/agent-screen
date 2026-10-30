//! Accessibility stand-ins for pointer actions: a drag on a slider sets the
//! slider's value, so it needs neither the user's pointer nor a pipe. Only
//! where the element says enough to know what the drag means: its range,
//! its orientation and its frame.

use crate::model::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slider {
    pub frame: Rect,
    pub min: f64,
    pub max: f64,
    pub vertical: bool,
}

/// The value a drag that ends at `to` gives the slider: where `to` falls
/// along its track, held to the ends. A vertical slider's minimum is at the
/// bottom, as AppKit's and the web's are. None when the element doesn't say
/// enough.
pub fn slider_value(slider: Slider, to: (f64, f64)) -> Option<f64> {
    let Slider {
        frame,
        min,
        max,
        vertical,
    } = slider;
    let finite = [
        frame.x,
        frame.y,
        frame.width,
        frame.height,
        min,
        max,
        to.0,
        to.1,
    ]
    .iter()
    .all(|v| v.is_finite());
    if !finite
        || !(max - min).is_finite()
        || max <= min
        || frame.width <= 0.0
        || frame.height <= 0.0
    {
        return None;
    }
    let fraction = if vertical {
        (frame.y + frame.height - to.1) / frame.height
    } else {
        (to.0 - frame.x) / frame.width
    };
    Some(min + fraction.clamp(0.0, 1.0) * (max - min))
}

/// Whether the value the slider reads back after setting `wanted` is it,
/// give or take a step: sliders snap to their own steps.
pub fn landed(slider: Slider, wanted: f64, read: f64) -> bool {
    let span = slider.max - slider.min;
    span.is_finite()
        && span > 0.0
        && wanted.is_finite()
        && read.is_finite()
        && (read - wanted).abs() <= span * 0.05
}

/// Infer direction from the current thumb position and value, rather than
/// assuming that a horizontal or vertical control is not reversed. At the
/// midpoint both directions fit; leave that drag to another route.
pub fn drag_value(slider: Slider, from: (f64, f64), to: (f64, f64), current: f64) -> Option<f64> {
    let at_start = slider_value(slider, from)?;
    let at_end = slider_value(slider, to)?;
    let reversed_start = slider.min + (slider.max - at_start);
    match (
        landed(slider, at_start, current),
        landed(slider, reversed_start, current),
    ) {
        (true, false) => Some(at_end),
        (false, true) => Some(slider.min + (slider.max - at_end)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slider(vertical: bool) -> Slider {
        Slider {
            frame: Rect::new(100.0, 200.0, 200.0, 20.0),
            min: 0.0,
            max: 100.0,
            vertical,
        }
    }

    #[test]
    fn a_drag_along_a_horizontal_slider_picks_the_value_under_its_end() {
        assert_eq!(slider_value(slider(false), (150.0, 210.0)), Some(25.0));
        assert_eq!(slider_value(slider(false), (40.0, 900.0)), Some(0.0));
        assert_eq!(slider_value(slider(false), (900.0, 0.0)), Some(100.0));
    }

    #[test]
    fn a_vertical_slider_grows_upward() {
        let s = Slider {
            frame: Rect::new(0.0, 0.0, 20.0, 100.0),
            ..slider(true)
        };
        assert_eq!(slider_value(s, (10.0, 75.0)), Some(25.0));
        assert_eq!(slider_value(s, (10.0, -5.0)), Some(100.0));
    }

    #[test]
    fn an_element_that_says_too_little_gets_no_value() {
        let mut s = slider(false);
        s.max = s.min;
        assert_eq!(slider_value(s, (150.0, 210.0)), None);
        let mut s = slider(false);
        s.frame.width = 0.0;
        assert_eq!(slider_value(s, (150.0, 210.0)), None);
        assert_eq!(slider_value(slider(false), (f64::NAN, 0.0)), None);
    }

    #[test]
    fn a_drag_requires_one_unambiguous_direction_and_a_matching_thumb() {
        assert_eq!(
            drag_value(slider(false), (150.0, 210.0), (250.0, 210.0), 25.0),
            Some(75.0)
        );
        assert_eq!(
            drag_value(slider(false), (150.0, 210.0), (250.0, 210.0), 75.0),
            Some(25.0)
        );
        assert_eq!(
            drag_value(slider(false), (200.0, 210.0), (250.0, 210.0), 50.0),
            None
        );
        assert_eq!(
            drag_value(slider(false), (150.0, 210.0), (250.0, 210.0), 40.0),
            None
        );
    }

    #[test]
    fn an_overflowing_range_or_nonfinite_value_has_no_substitution() {
        let s = Slider {
            min: -f64::MAX,
            max: f64::MAX,
            ..slider(false)
        };
        assert_eq!(slider_value(s, (150.0, 210.0)), None);
        assert!(!landed(s, 1.0, 2.0));
        assert!(!landed(slider(false), f64::NAN, 25.0));
    }

    #[test]
    fn a_snapped_value_counts_as_landed_but_an_unmoved_one_does_not() {
        assert!(landed(slider(false), 25.0, 24.0));
        assert!(!landed(slider(false), 25.0, 60.0));
        assert!(!landed(slider(false), 25.0, f64::NAN));
    }
}
