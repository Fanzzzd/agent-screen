//! Which new process is the one `launch_app` started. Every checkout of an
//! Electron project runs the same Electron.app, so a new process of the
//! bundle may be another tool's, started from a shell while the launch
//! waits for its own (issue #35). `open` hands the launch to launchd, the
//! parent of every app LaunchServices starts, with exactly the arguments
//! asked for.

use std::path::{Path, PathBuf};

use crate::model::Rect;

/// launchd's pid, the parent of an app `open` started.
pub const LAUNCHD: i32 = 1;

/// Whether a new instance quit, as an app that allows one instance does.
/// A process Agent Screen started itself isn't listed among the bundle's
/// running apps until it registers, a few hundred milliseconds in, so it
/// has quit only once it no longer runs at all. Taking "not listed" for
/// "quit" left a starting browser without its catch or hide, and its
/// window came onto the user's screen (#222).
pub fn has_quit(started_here: bool, runs: bool, listed: bool) -> bool {
    if started_here { !runs } else { !listed }
}

/// Whether a new window of the launched app answers the launch. A window
/// the agents' own browser instance opened for a page asked through its
/// pipe (ADR 0022) answers only when it is that page's window: the instance
/// is shared by every session, and another session's page may open a
/// window meanwhile. A starting app's windows are all its. A running app's
/// is only the one the catch took, or the one showing the file asked for,
/// which is asked of the app only when nothing else decides.
pub fn answers_launch(
    was_running: bool,
    pipe_page: Option<bool>,
    caught: bool,
    shows_file: impl FnOnce() -> bool,
) -> bool {
    match pipe_page {
        Some(is_page_window) => is_page_window,
        None => !was_running || caught || shows_file(),
    }
}

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

/// What `launch_app` asks macOS while it finds the app an agent named.
pub trait AppLookup {
    /// The app LaunchServices has for a bundle id.
    fn by_bundle_id(&self, id: &str) -> Option<PathBuf>;
    /// The app bundle at `path`, spelled as on disk: the file system
    /// ignores case, and "finder" should name Finder.app.
    fn bundle_at(&self, path: &Path) -> Option<PathBuf>;
    /// The app LaunchServices finds by name, wherever it is installed.
    fn by_name(&self, name: &str) -> Option<PathBuf>;
}

/// The folders searched for `<name>.app` before LaunchServices is asked by
/// name, in order. LaunchServices reports some apps at a path other than the
/// one the user knows, as Safari in its cryptex, so the familiar folders
/// come first.
pub fn app_folders(home: &str) -> Vec<PathBuf> {
    let system = [
        "/System/Applications",
        "/System/Applications/Utilities",
        "/System/Library/CoreServices",
        "/System/Library/CoreServices/Applications",
    ];
    ["/Applications", "/Applications/Utilities"]
        .iter()
        .map(PathBuf::from)
        .chain([Path::new(home).join("Applications")])
        .chain(system.iter().map(PathBuf::from))
        .collect()
}

/// The app bundle `launch_app` opens for `app`: an .app path (absolute or
/// from `~/`), a bundle id, or a name, with or without `.app`, found in
/// [`app_folders`] or by LaunchServices. Finder lives in
/// /System/Library/CoreServices, which the folders used to leave out
/// (#168).
pub fn find_app(app: &str, home: &str, lookup: &impl AppLookup) -> Option<PathBuf> {
    let app = app.trim();
    if app.is_empty() {
        return None;
    }
    let by_path = match app.strip_prefix("~/") {
        Some(rest) => Some(Path::new(home).join(rest)),
        None => app.starts_with('/').then(|| PathBuf::from(app)),
    };
    if let Some(path) = by_path {
        return lookup.bundle_at(&path);
    }
    if let Some(path) = lookup.by_bundle_id(app) {
        return Some(path);
    }
    let name = app.strip_suffix(".app").unwrap_or(app);
    // A name with a slash isn't a name; Path::join would read it as a path.
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let bundle = format!("{name}.app");
    app_folders(home)
        .into_iter()
        .find_map(|folder| lookup.bundle_at(&folder.join(&bundle)))
        .or_else(|| lookup.by_name(name))
}

/// A file URL for an absolute path, as a browser takes it in its address
/// field: everything but unreserved characters and `/` percent-encoded.
pub fn file_url(path: &str) -> String {
    let mut url = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// Whether a page's address `shown` is the URL `asked` for: the same
/// scheme, host and path, letter case aside for the scheme and host, a
/// trailing slash, the query and the fragment aside. A browser adds a
/// slash to a bare host and may drop or add a query, so the comparison is
/// loose; a redirect to another page isn't the same page.
pub fn same_page(asked: &str, shown: &str) -> bool {
    fn parts(url: &str) -> Option<(String, String, String)> {
        let (scheme, rest) = url.trim().split_once("://")?;
        let rest = rest.split(['?', '#']).next().unwrap_or_default();
        let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let path = path.trim_end_matches('/');
        Some((
            scheme.to_ascii_lowercase(),
            host.to_ascii_lowercase(),
            path.to_string(),
        ))
    }
    matches!((parts(asked), parts(shown)), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_pages_window_answers_a_launch_through_the_pipe() {
        use super::answers_launch;
        let asked = || panic!("the file is asked of the app only when nothing else decides");
        // The user's running app: only the window the launch caught.
        assert!(!answers_launch(true, None, false, || false));
        assert!(answers_launch(true, None, true, asked));
        assert!(answers_launch(true, None, false, || true));
        // The agents' instance, asked through its pipe: the page's window
        // only, even one the catch took.
        assert!(answers_launch(true, Some(true), false, asked));
        assert!(!answers_launch(true, Some(false), true, asked));
        // A starting app: any new window.
        assert!(answers_launch(false, None, false, asked));
    }

    #[test]
    fn a_process_started_here_has_quit_only_once_it_ends() {
        use super::has_quit;
        // Started here, running, not registered yet: still starting.
        assert!(!has_quit(true, true, false));
        assert!(has_quit(true, false, false));
        // Started by `open`: its listing decides.
        assert!(has_quit(false, true, false));
        assert!(!has_quit(false, true, true));
    }

    use super::*;

    /// A Mac with these app bundles, and these in LaunchServices.
    struct Mac {
        bundles: Vec<&'static str>,
        ids: Vec<(&'static str, &'static str)>,
        names: Vec<(&'static str, &'static str)>,
    }

    impl AppLookup for Mac {
        fn by_bundle_id(&self, id: &str) -> Option<PathBuf> {
            self.ids
                .iter()
                .find(|(i, _)| i.eq_ignore_ascii_case(id))
                .map(|(_, p)| PathBuf::from(p))
        }
        fn bundle_at(&self, path: &Path) -> Option<PathBuf> {
            self.bundles
                .iter()
                .find(|b| {
                    Path::new(b)
                        .as_os_str()
                        .eq_ignore_ascii_case(path.as_os_str())
                })
                .map(PathBuf::from)
        }
        fn by_name(&self, name: &str) -> Option<PathBuf> {
            self.names
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, p)| PathBuf::from(p))
        }
    }

    const FINDER: &str = "/System/Library/CoreServices/Finder.app";
    const SAFARI: &str = "/Applications/Safari.app";
    const CRYPTEX_SAFARI: &str =
        "/System/Volumes/Preboot/Cryptexes/App/System/Applications/Safari.app";

    fn mac() -> Mac {
        Mac {
            bundles: vec![
                FINDER,
                SAFARI,
                CRYPTEX_SAFARI,
                "/System/Library/CoreServices/Applications/Archive Utility.app",
                "/Users/me/Applications/Tool.app",
                "/Users/me/Applications/Dock.app",
                "/System/Library/CoreServices/Dock.app",
                "/tmp/build/MyApp.app",
                "/Volumes/Work/Odd.app",
            ],
            ids: vec![("com.apple.finder", FINDER), ("com.apple.Safari", SAFARI)],
            names: vec![
                ("Finder", FINDER),
                ("Safari", CRYPTEX_SAFARI),
                ("Odd", "/Volumes/Work/Odd.app"),
            ],
        }
    }

    fn find(app: &str) -> Option<PathBuf> {
        find_app(app, "/Users/me", &mac())
    }

    #[test]
    fn finder_is_found_by_its_name() {
        for name in ["Finder", "finder", "Finder.app", " Finder "] {
            assert_eq!(find(name), Some(PathBuf::from(FINDER)), "{name}");
        }
        assert_eq!(find("com.apple.finder"), Some(PathBuf::from(FINDER)));
    }

    #[test]
    fn core_services_apps_are_found_by_name() {
        assert_eq!(
            find("Archive Utility"),
            Some(PathBuf::from(
                "/System/Library/CoreServices/Applications/Archive Utility.app"
            ))
        );
    }

    #[test]
    fn the_applications_folders_come_before_launch_services() {
        assert_eq!(find("Safari"), Some(PathBuf::from(SAFARI)));
        assert_eq!(
            find("Tool"),
            Some(PathBuf::from("/Users/me/Applications/Tool.app"))
        );
    }

    #[test]
    fn the_users_apps_come_before_system_agents() {
        assert_eq!(
            find("Dock"),
            Some(PathBuf::from("/Users/me/Applications/Dock.app"))
        );
    }

    #[test]
    fn launch_services_finds_an_app_outside_the_folders() {
        assert_eq!(find("Odd"), Some(PathBuf::from("/Volumes/Work/Odd.app")));
    }

    #[test]
    fn a_path_is_only_that_path() {
        assert_eq!(
            find("/tmp/build/MyApp.app"),
            Some(PathBuf::from("/tmp/build/MyApp.app"))
        );
        assert_eq!(
            find("~/Applications/Tool.app"),
            Some(PathBuf::from("/Users/me/Applications/Tool.app"))
        );
        assert_eq!(find("/tmp/build/Finder.app"), None);
    }

    #[test]
    fn nothing_else_matches() {
        for app in ["", "  ", ".app", "NoSuchApp", "Work/Odd", "../Finder"] {
            assert_eq!(find(app), None, "{app:?}");
        }
    }

    #[test]
    fn a_file_path_becomes_a_file_url() {
        assert_eq!(file_url("/tmp/page.html"), "file:///tmp/page.html");
        assert_eq!(
            file_url("/tmp/my page#1?.html"),
            "file:///tmp/my%20page%231%3F.html"
        );
        assert_eq!(file_url("/tmp/页.html"), "file:///tmp/%E9%A1%B5.html");
    }

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

    #[test]
    fn a_page_is_the_url_asked_for_by_scheme_host_and_path() {
        let asked = "http://127.0.0.1:5000/agent";
        assert!(same_page(asked, "http://127.0.0.1:5000/agent"));
        assert!(same_page(asked, "HTTP://127.0.0.1:5000/agent/?x=1#top"));
        assert!(same_page("https://Example.com", "https://example.com/"));
        assert!(!same_page(asked, "http://127.0.0.1:5000/next"));
        assert!(!same_page(asked, "https://127.0.0.1:5000/agent"));
        assert!(!same_page(asked, "http://127.0.0.1:5001/agent"));
        assert!(!same_page(asked, "about:blank"));
        assert!(!same_page(asked, ""));
    }
}
