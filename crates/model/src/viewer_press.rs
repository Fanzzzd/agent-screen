//! When a press the user makes in the Viewer makes Agent Screen the front
//! process (ADR 0008, ADR 0014). Taking the front makes the app on the
//! display lose active, if it had it, and some apps close a list on that:
//! a Qt tool window hides with its app, and WeCom's search results closed
//! before the press reached them, which then picked the chat under the
//! list. An app that made itself the front process, as aTrust does, drops
//! every press Agent Screen posts to its main window until Agent Screen
//! takes the front (issue #41).
//!
//! So a press on a list or panel reaches it first, and Agent Screen takes
//! the front once the release is delivered. Any other press keeps #41's
//! order: the front first, then the press.

use crate::model::Rect;

/// A window a press can reach, as the window list reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub pid: i32,
    pub layer: i64,
    pub bounds: Rect,
}

/// When Agent Screen takes the front for a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    /// Before the press goes out.
    BeforePress,
    /// After the release is delivered.
    AfterRelease,
}

/// When to take the front for a press at `at`, given the windows a press
/// can reach, front to back. The window the press hits is a list or panel
/// when it floats above the normal layer, or when it lies over another
/// window of its own app at that point.
pub fn front_for_press(windows: &[Window], at: (f64, f64)) -> Front {
    let mut under = windows.iter().filter(|w| w.bounds.contains(at));
    let Some(hit) = under.next() else {
        return Front::BeforePress;
    };
    if hit.layer != 0 || under.any(|w| w.pid == hit.pid) {
        Front::AfterRelease
    } else {
        Front::BeforePress
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(pid: i32, layer: i64, x: f64, y: f64, w: f64, h: f64) -> Window {
        Window {
            pid,
            layer,
            bounds: Rect::new(x, y, w, h),
        }
    }

    #[test]
    fn a_press_on_an_app_window_takes_the_front_first() {
        let windows = [window(10, 0, 0.0, 0.0, 600.0, 500.0)];
        assert_eq!(
            front_for_press(&windows, (100.0, 100.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn a_press_on_a_list_over_its_own_app_reaches_it_first() {
        // WeCom's search results: a borderless window over the main one.
        let windows = [
            window(10, 0, 20.0, 70.0, 560.0, 206.0),
            window(10, 0, 0.0, 0.0, 600.0, 500.0),
        ];
        assert_eq!(
            front_for_press(&windows, (100.0, 120.0)),
            Front::AfterRelease
        );
        // Beside the list, the press hits the main window.
        assert_eq!(
            front_for_press(&windows, (100.0, 400.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn a_press_on_a_floating_window_reaches_it_first() {
        // A Qt tool window or a pop-up menu, above the normal layer.
        for layer in [3, 101] {
            let windows = [window(10, layer, 20.0, 70.0, 560.0, 206.0)];
            assert_eq!(
                front_for_press(&windows, (100.0, 120.0)),
                Front::AfterRelease
            );
        }
    }

    #[test]
    fn a_window_over_another_apps_window_is_an_app_window() {
        let windows = [
            window(10, 0, 0.0, 0.0, 600.0, 500.0),
            window(20, 0, 0.0, 0.0, 900.0, 700.0),
        ];
        assert_eq!(
            front_for_press(&windows, (100.0, 100.0)),
            Front::BeforePress
        );
    }

    #[test]
    fn a_press_on_no_window_takes_the_front_first() {
        assert_eq!(front_for_press(&[], (5.0, 5.0)), Front::BeforePress);
    }
}
