//! A drag with the user's real pointer (ADR 0013, "Drags between
//! windows"). A drag may run from one window to another on the same Agent
//! Display, as a file from Finder onto a web page does (issue #166). A
//! system drag session follows only the real pointer, so such a drag can't
//! run in the background; it asks for the user's pointer instead.

use crate::model::{Rect, WindowTarget};

/// Why a drag between two windows enters the Input Queue.
pub const BETWEEN_WINDOWS: &str = "A drag from one window to another, such as a file onto a web page, needs a system drag session, which follows only the user's pointer, so it can't run in the background.";

/// A drag the user allowed, in global points: its display, its ends, and
/// the windows under them when the agent asked. Both are the same window
/// for a drag within one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Path {
    pub area: Rect,
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub source: WindowTarget,
    pub drop: WindowTarget,
}

/// Whether a background drag from a window to another must ask for the
/// user's pointer: both ends lie on windows, and not the same one.
pub fn between_windows(start: Option<WindowTarget>, end: Option<WindowTarget>) -> bool {
    matches!((start, end), (Some(a), Some(b)) if a != b)
}

/// Why the drag may not send its event at `at`, or None. `under` is the
/// topmost ordinary window there; `on_display`, the windows on the drag's
/// Agent Display when its loan began. The press must land on the source
/// window and the release on the drop window. On the way the pointer may
/// cross any window of that display, or none, but never leave it.
pub fn refusal(
    path: &Path,
    at: (f64, f64),
    under: Option<WindowTarget>,
    on_display: &[WindowTarget],
) -> Option<&'static str> {
    if !path.area.contains(at) {
        return Some("the drag would leave its Agent Display");
    }
    if at == path.from {
        return (under != Some(path.source)).then_some("the window was covered or left the screen");
    }
    if at == path.to {
        return (under != Some(path.drop))
            .then_some("the drop window was covered or left the screen");
    }
    under
        .is_some_and(|w| !on_display.contains(&w))
        .then_some("the drag would cross a window that isn't on its Agent Display")
}

/// A drag's travel never takes fewer events than this, so a short drag
/// still reaches the drop window as several moves, or more than this, so
/// a long one doesn't take longer than a person's.
pub const MIN_STEPS: usize = 12;
pub const MAX_STEPS: usize = 24;
/// Points of travel per event between the two limits.
const SPACING: f64 = 24.0;
/// How far from the press the source must see the pointer before the drag
/// waits for its system drag session: past AppKit's few points of drag
/// threshold, which Finder's icons and the filedrag fixture use.
pub const SESSION_AFTER: f64 = 6.0;

/// How many events carry a drag of `distance` points from the press to
/// the release.
pub fn steps(distance: f64) -> usize {
    if distance <= 0.0 || !distance.is_finite() {
        return 1;
    }
    ((distance / SPACING).ceil() as usize).clamp(MIN_STEPS, MAX_STEPS)
}

/// The points of a drag's travel after the press at `from`, the last
/// exactly `to`. They ease in and out, as a person's hand does: the first
/// moves are a few points, so the source sees the drag start where it was
/// pressed, and the last ones land close to `to`, so the drop window sees
/// several moves over it before the release.
pub fn travel(from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let n = steps(dx.hypot(dy));
    (1..=n)
        .map(|i| {
            if i == n {
                return to;
            }
            let t = i as f64 / n as f64;
            let eased = t * t * (3.0 - 2.0 * t);
            (from.0 + dx * eased, from.1 + dy * eased)
        })
        .collect()
}

/// The index in `points` after which a drag between two windows waits for
/// the source's system drag session: the first point `SESSION_AFTER` from
/// the press, or the last point of a shorter drag.
pub fn session_step(from: (f64, f64), points: &[(f64, f64)]) -> Option<usize> {
    let last = points.last()?;
    let far = SESSION_AFTER.min((last.0 - from.0).hypot(last.1 - from.1));
    points
        .iter()
        .position(|p| (p.0 - from.0).hypot(p.1 - from.1) >= far)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: WindowTarget = WindowTarget {
        pid: 1,
        window_id: 10,
    };
    const DROP: WindowTarget = WindowTarget {
        pid: 2,
        window_id: 20,
    };
    const OTHER: WindowTarget = WindowTarget {
        pid: 3,
        window_id: 30,
    };
    const USERS: WindowTarget = WindowTarget {
        pid: 4,
        window_id: 40,
    };

    fn path() -> Path {
        Path {
            area: Rect::new(2000.0, 0.0, 1280.0, 800.0),
            from: (2300.0, 300.0),
            to: (2900.0, 300.0),
            source: SOURCE,
            drop: DROP,
        }
    }

    #[test]
    fn a_drag_runs_from_one_window_to_another_on_its_display() {
        let on = [SOURCE, DROP, OTHER];
        let p = path();
        assert_eq!(refusal(&p, p.from, Some(SOURCE), &on), None);
        assert_eq!(refusal(&p, (2500.0, 300.0), Some(SOURCE), &on), None);
        assert_eq!(refusal(&p, (2600.0, 300.0), Some(OTHER), &on), None);
        // Between windows, or under the drag's own image.
        assert_eq!(refusal(&p, (2650.0, 300.0), None, &on), None);
        assert_eq!(refusal(&p, p.to, Some(DROP), &on), None);
    }

    #[test]
    fn the_press_and_the_release_land_only_on_their_windows() {
        let on = [SOURCE, DROP, OTHER];
        let p = path();
        assert!(refusal(&p, p.from, Some(OTHER), &on).is_some());
        assert!(refusal(&p, p.from, None, &on).is_some());
        assert!(refusal(&p, p.to, Some(OTHER), &on).is_some_and(|why| why.contains("drop window")));
        assert!(refusal(&p, p.to, None, &on).is_some());
    }

    #[test]
    fn the_path_never_leaves_its_display_or_crosses_a_window_not_on_it() {
        let on = [SOURCE, DROP];
        let p = path();
        assert!(refusal(&p, (1999.0, 300.0), None, &on).is_some());
        assert!(refusal(&p, (2600.0, 800.0), None, &on).is_some());
        assert!(refusal(&p, (2600.0, 300.0), Some(USERS), &on).is_some());
    }

    #[test]
    fn a_drag_within_one_window_keeps_its_rule() {
        let on = [SOURCE];
        let p = Path {
            to: (2400.0, 300.0),
            drop: SOURCE,
            ..path()
        };
        assert_eq!(refusal(&p, p.to, Some(SOURCE), &on), None);
        assert!(refusal(&p, p.to, None, &on).is_some());
    }

    fn length(a: (f64, f64), b: (f64, f64)) -> f64 {
        (b.0 - a.0).hypot(b.1 - a.1)
    }

    #[test]
    fn the_number_of_moves_follows_the_distance_within_its_limits() {
        assert_eq!(steps(0.0), 1);
        assert_eq!(steps(f64::NAN), 1);
        assert_eq!(steps(5.0), MIN_STEPS);
        assert_eq!(steps(80.0), MIN_STEPS);
        assert_eq!(steps(400.0), 17);
        assert_eq!(steps(580.0), MAX_STEPS);
        assert_eq!(steps(5000.0), MAX_STEPS);
    }

    #[test]
    fn the_travel_eases_from_the_press_to_exactly_the_release() {
        let (from, to) = ((320.0, 268.0), (900.0, 268.0));
        let points = travel(from, to);
        assert_eq!(points.len(), MAX_STEPS);
        assert_eq!(*points.last().unwrap(), to);
        // Each move goes further, none back, and none sits on the press.
        let far: Vec<f64> = points.iter().map(|p| length(from, *p)).collect();
        assert!(far.windows(2).all(|w| w[1] > w[0]));
        assert!(far[0] > 0.0);
        // The first move stays under the drag threshold, as a hand's does.
        assert!(far[0] < 4.0);
        // Several moves land near the drop point before the release.
        let near = points.iter().filter(|p| length(**p, to) < 16.0).count();
        assert!(near >= 3, "{near} points near the end");
        // A zero-length drag releases where it pressed.
        assert_eq!(travel(from, from), vec![from]);
    }

    #[test]
    fn the_session_wait_comes_once_the_drag_is_past_its_threshold() {
        let from = (100.0, 100.0);
        let points = travel(from, (700.0, 100.0));
        let i = session_step(from, &points).unwrap();
        assert!(length(from, points[i]) >= SESSION_AFTER);
        assert!(length(from, points[i - 1]) < SESSION_AFTER);
        assert!(i <= 3, "the session wait comes at move {i}");
        // A drag shorter than the threshold waits after its last move.
        let short = travel(from, (103.0, 100.0));
        assert_eq!(session_step(from, &short), Some(short.len() - 1));
        assert_eq!(session_step(from, &[]), None);
    }

    #[test]
    fn only_a_drag_between_two_windows_asks_for_the_pointer() {
        assert!(between_windows(Some(SOURCE), Some(DROP)));
        assert!(!between_windows(Some(SOURCE), Some(SOURCE)));
        assert!(!between_windows(Some(SOURCE), None));
        assert!(!between_windows(None, Some(DROP)));
    }
}
