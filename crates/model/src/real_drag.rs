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

    #[test]
    fn only_a_drag_between_two_windows_asks_for_the_pointer() {
        assert!(between_windows(Some(SOURCE), Some(DROP)));
        assert!(!between_windows(Some(SOURCE), Some(SOURCE)));
        assert!(!between_windows(Some(SOURCE), None));
        assert!(!between_windows(None, Some(DROP)));
    }
}
