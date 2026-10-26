//! Which new process is the one `launch_app` started. Every checkout of an
//! Electron project runs the same Electron.app, so a new process of the
//! bundle may be another tool's, started from a shell while the launch
//! waits for its own (issue #35). `open` hands the launch to launchd, the
//! parent of every app LaunchServices starts, with exactly the arguments
//! asked for.

/// launchd's pid, the parent of an app `open` started.
pub const LAUNCHD: i32 = 1;

/// Whether a process with this parent and `argv` is the instance `open`
/// started with `asked` after `--args`.
pub fn is_opened(parent: i32, argv: &[String], asked: &[String]) -> bool {
    parent == LAUNCHD && argv.get(1..) == Some(asked)
}

#[cfg(test)]
mod tests {
    use super::*;

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
