//! Wheel scroll chunking for background scroll events (#104).
//!
//! Large scroll amounts are split into events of at most `WHEEL_LINES` lines
//! on each axis so virtualized lists advance by every requested click instead
//! of clamping to one page.

/// Lines in one wheel event, at most: 200 px in Chrome, under a page of
/// any list worth scrolling.
pub const WHEEL_LINES: i32 = 5;

/// Whole wheel steps, at least one for any movement.
pub fn wheel_steps(delta: f64) -> i32 {
    if delta == 0.0 {
        0
    } else if delta < 0.0 {
        delta.round().min(-1.0) as i32
    } else {
        delta.round().max(1.0) as i32
    }
}

/// A scroll of `delta` lines as events of at most `WHEEL_LINES` lines on
/// each axis, adding up to the same clicks.
pub fn wheel_chunks(delta: (f64, f64)) -> Vec<(f64, f64)> {
    let (x, y) = (wheel_steps(delta.0), wheel_steps(delta.1));
    let count = (x.abs().max(y.abs()) + WHEEL_LINES - 1) / WHEEL_LINES;
    let share = |total: i32, i: i32| total * (i + 1) / count.max(1) - total * i / count.max(1);
    (0..count.max(1))
        .map(|i| (share(x, i) as f64, share(y, i) as f64))
        .filter(|&(x, y)| x != 0.0 || y != 0.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_wheel_clicks_go_as_events_of_a_few_lines_adding_up_to_them() {
        let chunks = wheel_chunks((0.0, -800.0));
        assert_eq!(chunks.len(), 160);
        assert!(
            chunks
                .iter()
                .all(|&(x, y)| x == 0.0 && (-5.0..=-1.0).contains(&y))
        );
        assert_eq!(chunks.iter().map(|c| c.1).sum::<f64>(), -800.0);
        // Both axes at once share the events.
        let both = wheel_chunks((7.0, -12.0));
        assert_eq!(both.len(), 3);
        assert_eq!(both.iter().map(|c| c.0).sum::<f64>(), 7.0);
        assert_eq!(both.iter().map(|c| c.1).sum::<f64>(), -12.0);
        // A few clicks stay one event, as before.
        assert_eq!(wheel_chunks((0.0, 3.0)), vec![(0.0, 3.0)]);
        assert!(wheel_chunks((0.0, 0.0)).is_empty());
    }
}
