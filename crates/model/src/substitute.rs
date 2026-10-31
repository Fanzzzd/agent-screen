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

/// Geometry tolerance for matching a thumb position to its current value.
/// This alone is not sufficient to confirm a write: use `confirmed`.
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
/// midpoint both directions fit; leave that drag to another route. The
/// value moves from `current` by the drag's distance along the track, held
/// to the ends: a grab beside the thumb's centre still moves the way the
/// drag goes, and a drag that goes nowhere changes nothing.
pub fn drag_value(slider: Slider, from: (f64, f64), to: (f64, f64), current: f64) -> Option<f64> {
    let at_start = slider_value(slider, from)?;
    let delta = slider_value(slider, to)? - at_start;
    let reversed_start = slider.min + (slider.max - at_start);
    let delta = match (
        landed(slider, at_start, current),
        landed(slider, reversed_start, current),
    ) {
        (true, false) => delta,
        (false, true) => -delta,
        _ => return None,
    };
    Some((current + delta).clamp(slider.min, slider.max))
}

/// Confirmation seam for a value write, separate from source-geometry
/// matching. The caller supplies the value read before the write too.
pub fn confirmed(slider: Slider, before: f64, wanted: f64, read: f64) -> bool {
    if !before.is_finite() || !landed(slider, wanted, read) {
        return false;
    }
    if wanted == before {
        return read == wanted;
    }
    // A short drag's old value can already be inside the geometry
    // tolerance. Require actual progress closer to the requested value.
    (read - wanted).abs() < (read - before).abs()
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
    fn an_unchanged_value_does_not_confirm_a_short_drag() {
        let s = slider(false);
        let wanted = drag_value(s, (150.0, 210.0), (158.0, 210.0), 25.0).unwrap();
        assert!((wanted - 29.0).abs() < 1e-9);
        assert!(!confirmed(s, 25.0, wanted, 25.0));
        assert!(confirmed(s, 25.0, wanted, 28.0));
        assert!(!confirmed(s, 75.0, 71.0, 75.0));
    }

    fn near(got: Option<f64>, want: f64) -> bool {
        got.is_some_and(|v| (v - want).abs() < 1e-9)
    }

    #[test]
    fn an_off_centre_grab_moves_by_the_drag_not_to_its_end() {
        let s = slider(false);
        // The thumb at x 150 reads 25; the value is 22, inside tolerance.
        let left = drag_value(s, (150.0, 210.0), (148.0, 210.0), 22.0);
        assert!(near(left, 21.0), "a left drag must not raise it: {left:?}");
        let right = drag_value(s, (150.0, 210.0), (158.0, 210.0), 22.0);
        assert!(near(right, 26.0), "{right:?}");
        let still = drag_value(s, (150.0, 210.0), (150.0, 210.0), 22.0);
        assert!(
            near(still, 22.0),
            "a zero drag must not change it: {still:?}"
        );
    }

    #[test]
    fn a_reversed_or_vertical_slider_moves_by_the_signed_drag() {
        let s = slider(false);
        // Reversed: the thumb at x 150 reads 75.
        let right = drag_value(s, (150.0, 210.0), (158.0, 210.0), 77.0);
        assert!(near(right, 73.0), "{right:?}");
        let left = drag_value(s, (150.0, 210.0), (148.0, 210.0), 77.0);
        assert!(near(left, 78.0), "{left:?}");
        let v = Slider {
            frame: Rect::new(0.0, 0.0, 20.0, 100.0),
            ..slider(true)
        };
        let up = drag_value(v, (10.0, 75.0), (10.0, 70.0), 23.0);
        assert!(near(up, 28.0), "{up:?}");
        let down = drag_value(v, (10.0, 75.0), (10.0, 76.0), 23.0);
        assert!(
            near(down, 22.0),
            "a downward drag must not raise it: {down:?}"
        );
    }

    #[test]
    fn a_drag_past_the_track_is_held_to_its_ends() {
        let s = slider(false);
        assert!(near(
            drag_value(s, (150.0, 210.0), (900.0, 210.0), 27.0),
            100.0
        ));
        assert!(near(
            drag_value(s, (150.0, 210.0), (40.0, 210.0), 22.0),
            0.0
        ));
        assert!(near(
            drag_value(s, (150.0, 210.0), (900.0, 210.0), 73.0),
            0.0
        ));
    }

    #[test]
    fn confirming_a_noop_does_not_accept_an_unexpected_change() {
        assert!(confirmed(slider(false), 25.0, 25.0, 25.0));
        assert!(!confirmed(slider(false), 25.0, 25.0, 26.0));
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
