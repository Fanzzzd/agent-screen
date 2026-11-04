//! Which new process is the one `launch_app` started. Every checkout of an
//! Electron project runs the same Electron.app, so a new process of the
//! bundle may be another tool's, started from a shell while the launch
//! waits for its own (issue #35). `open` hands the launch to launchd, the
//! parent of every app LaunchServices starts, with exactly the arguments
//! asked for.

use crate::model::Rect;

/// launchd's pid, the parent of an app `open` started.
pub const LAUNCHD: i32 = 1;

/// Whether a process with this parent and `argv` is the instance `open`
/// started with `asked` after `--args`.
pub fn is_opened(parent: i32, argv: &[String], asked: &[String]) -> bool {
    parent == LAUNCHD && argv.get(1..) == Some(asked)
}

/// How far inside the display's edges a launched window sits, as when
/// Agent Screen moves one onto it.
pub const WINDOW_MARGIN: f64 = 40.0;

/// The switches that open a Chromium browser's windows on the display,
/// for a browser Agent Screen starts. Chrome makes its first window on the
/// main screen, clear, and draws it there about 250 ms later, unless it has
/// moved by then; it is busy starting, so a move through Accessibility may
/// come too late, and the user sees the window (#123). Chrome gives every
/// window of the process this frame, and the last of a repeated switch
/// wins, so the agent's own come after these. None for a display too small
/// to hold a window, as a removed one with empty bounds.
pub fn browser_window_switches(display: Rect) -> Vec<String> {
    let area = display.inset(WINDOW_MARGIN, WINDOW_MARGIN);
    if area.width < 1.0 || area.height < 1.0 {
        return Vec::new();
    }
    vec![
        format!(
            "--window-position={},{}",
            area.x.round() as i64,
            area.y.round() as i64
        ),
        format!(
            "--window-size={},{}",
            area.width.round() as i64,
            area.height.round() as i64
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_opens_inside_the_display() {
        let display = Rect::new(4288.0, -710.0, 1100.0, 700.0);
        assert_eq!(
            browser_window_switches(display),
            strings(&["--window-position=4328,-670", "--window-size=1020,620"])
        );
    }

    #[test]
    fn a_removed_display_gives_no_switches() {
        assert!(browser_window_switches(Rect::default()).is_empty());
        assert!(browser_window_switches(Rect::new(0.0, 0.0, 80.0, 600.0)).is_empty());
    }

    fn strings(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_opened_instance_matches() {
        let asked = strings(&["/work/app"]);
        let argv = strings(&["/E.app/Contents/MacOS/Electron", "/work/app"]);
        assert!(is_opened(LAUNCHD, &argv, &asked));
    }

    #[test]
    fn a_shell_started_copy_does_not() {
        let asked = strings(&["/work/app"]);
        // Same arguments, but a shell's child.
        let argv = strings(&["/E.app/Contents/MacOS/Electron", "/work/app"]);
        assert!(!is_opened(4242, &argv, &asked));
    }

    #[test]
    fn another_checkout_does_not() {
        let asked = strings(&["/work/app"]);
        let argv = strings(&["/E.app/Contents/MacOS/Electron", "/other/checkout"]);
        assert!(!is_opened(LAUNCHD, &argv, &asked));
        assert!(!is_opened(LAUNCHD, &strings(&["/E"]), &asked));
        assert!(!is_opened(LAUNCHD, &[], &asked));
    }

    #[test]
    fn no_arguments_matches_none() {
        assert!(is_opened(LAUNCHD, &strings(&["/E"]), &[]));
        assert!(!is_opened(LAUNCHD, &strings(&["/E", "x"]), &[]));
    }
}
