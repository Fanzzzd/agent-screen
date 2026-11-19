//! A Chromium browser the user already runs gets a separate instance for
//! agents (#222, ADR 0022). The agents' windows used to open in the user's
//! own process. While that process was the user's front app with their
//! window focused, Agent Screen could post nothing to it (#110), and a
//! browser's web elements take no accessibility press, so every agent's
//! browser work stopped. A second process with its own `--user-data-dir`
//! is never the user's front app, and Agent Screen starts it with its
//! private DevTools pipe (ADR 0010). Its profile persists, so a sign-in the
//! user does there once carries over to later sessions.

use std::path::{Path, PathBuf};

/// The folder in Agent Screen's support folder that holds the agents'
/// browser profiles, one per browser.
pub const PROFILES: &str = "Browser Profiles";

/// The switch that names a Chromium profile folder.
const USER_DATA_DIR: &str = "--user-data-dir";

/// Where the agents' profile for the browser `bundle_id` lives. A test
/// instance of Agent Screen keeps its own, so a test never signs in or out
/// of the agents' real profile.
pub fn profile_dir(support: &Path, instance: Option<&str>, bundle_id: &str) -> PathBuf {
    let folder = match instance {
        Some(instance) => format!("{PROFILES} ({instance})"),
        None => PROFILES.to_string(),
    };
    support.join(folder).join(bundle_id)
}

/// Whether the agent's arguments name a profile themselves, which then
/// wins: the agent asked for that instance.
pub fn names_profile(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|a| a == USER_DATA_DIR || a.starts_with(&format!("{USER_DATA_DIR}=")))
}

/// The switches that start the agents' instance on `profile`, ahead of the
/// agent's own arguments. A fresh profile would show a first-run page and
/// ask to become the default browser, which the agent doesn't need.
pub fn switches(profile: &Path) -> Vec<String> {
    vec![
        format!("{USER_DATA_DIR}={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
    ]
}

/// Whether a process with this command line is the agents' instance on
/// `profile`.
pub fn is_agents(argv: &[String], profile: &Path) -> bool {
    let wanted = format!("{USER_DATA_DIR}={}", profile.display());
    argv.contains(&wanted)
}

/// The file beside the agents' profiles that lists the browsers that
/// refuse a second instance, one `<bundle id> <version>` per line. Dia
/// does: its app quits at once when another Dia runs, whatever its
/// profile. An update may change that, so a new version is tried again.
pub const REFUSALS: &str = "refuses own instance";

/// Whether `record`, the refusals file, lists this browser's version.
pub fn refuses(record: &str, bundle_id: &str, version: &str) -> bool {
    record
        .lines()
        .any(|l| l.split_once(' ') == Some((bundle_id, version)))
}

/// `record` with this browser's version listed, in place of any other
/// version of it.
pub fn with_refusal(record: &str, bundle_id: &str, version: &str) -> String {
    record
        .lines()
        .filter(|l| !l.is_empty() && l.split_once(' ').map(|(id, _)| id) != Some(bundle_id))
        .map(String::from)
        .chain([format!("{bundle_id} {version}")])
        .map(|l| l + "\n")
        .collect()
}

/// Whether a window of `size` reaches past a display of `display`, both
/// width and height in points. Dia's first window takes the main screen's
/// size whatever `--window-size` says.
pub fn overflows(size: (f64, f64), display: (f64, f64)) -> bool {
    size.0 > display.0 + 0.5 || size.1 > display.1 + 0.5
}

/// Where a page that the agents' instance put in a window it had open,
/// as a tab, leaves the launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabLanding {
    /// In a window on the launch's display: that window answers.
    ThisDisplay,
    /// In a window elsewhere, such as another agent's display: the tab is
    /// closed and the launch fails.
    Elsewhere,
}

/// Where a tab in a window of frame `window` landed, for a launch on
/// `display`, both in global points.
pub fn tab_landing(window: crate::model::Rect, display: crate::model::Rect) -> TabLanding {
    if display.contains(window.mid()) {
        TabLanding::ThisDisplay
    } else {
        TabLanding::Elsewhere
    }
}

/// Whether a window the system lists has the frame the browser gave for
/// it: the two round differently.
pub fn same_frame(a: crate::model::Rect, b: crate::model::Rect) -> bool {
    (a.x - b.x).abs() <= 2.0
        && (a.y - b.y).abs() <= 2.0
        && (a.width - b.width).abs() <= 2.0
        && (a.height - b.height).abs() <= 2.0
}

/// How `launch_app` starts a browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// As the agent asked: not a Chromium browser, a profile the agent
    /// named, or a browser that refuses a second instance and doesn't run.
    AsAsked,
    /// Start the agents' instance with these switches ahead of the agent's
    /// arguments. Also when no one runs the browser: the agents' work then
    /// never touches the user's default profile.
    Start(Vec<String>),
    /// The agents' instance runs as this pid, with Agent Screen's pipe:
    /// open the new window there.
    Reuse(i32),
    /// The agents' instance runs as this pid without Agent Screen's pipe,
    /// left from an earlier run of the app: quit it and start anew, since
    /// only the pipe opens a window there without activating it.
    Replace(i32),
    /// The browser refuses a second instance and the user runs it, so the
    /// window opens in the user's process as before, and shares its focus
    /// (#110).
    Shared,
}

/// `running`: the browser's processes, each with its command line.
/// `piped`: the pids Agent Screen holds a DevTools pipe to.
/// `refuses`: the browser is known to refuse a second instance.
pub fn plan(
    chromium_browser: bool,
    arguments: &[String],
    running: &[(i32, Vec<String>)],
    piped: &[i32],
    profile: &Path,
    refuses: bool,
) -> Plan {
    if !chromium_browser || names_profile(arguments) {
        return Plan::AsAsked;
    }
    if let Some((pid, _)) = running.iter().find(|(_, argv)| is_agents(argv, profile)) {
        return if piped.contains(pid) {
            Plan::Reuse(*pid)
        } else {
            Plan::Replace(*pid)
        };
    }
    match (refuses, running.is_empty()) {
        // Started now, it would refuse the user's own launch later.
        (true, true) => Plan::AsAsked,
        (true, false) => Plan::Shared,
        (false, _) => Plan::Start(switches(profile)),
    }
}

/// Whether a start of the agents' instance was refused, as a browser that
/// allows one instance refuses: the new process quit at once while the
/// user's kept running. Any other failure, such as a timeout or a crash,
/// is not remembered.
pub fn was_refused(quit_at_once: bool, users_still_run: bool) -> bool {
    quit_at_once && users_still_run
}

/// The switches that would open a DevTools port, which any local process
/// could use to drive the agents' signed-in profile.
const DEBUGGING_PORT: [&str; 2] = ["--remote-debugging-port", "--remote-debugging-address"];

/// The agent's arguments for the agents' instance, without switches that
/// open a DevTools port: Agent Screen's own channel is a private pipe.
pub fn without_debugging_port(arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        .filter(|a| {
            !DEBUGGING_PORT
                .iter()
                .any(|s| a.as_str() == *s || a.starts_with(&format!("{s}=")))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    const SUPPORT: &str = "/Users/u/Library/Application Support/Agent Screen";

    fn profile() -> PathBuf {
        profile_dir(Path::new(SUPPORT), None, "company.thebrowser.dia")
    }

    #[test]
    fn each_browser_and_test_instance_has_its_own_profile() {
        assert_eq!(
            profile(),
            Path::new(SUPPORT).join("Browser Profiles/company.thebrowser.dia")
        );
        assert_eq!(
            profile_dir(Path::new(SUPPORT), Some("e2e"), "com.google.Chrome"),
            Path::new(SUPPORT).join("Browser Profiles (e2e)/com.google.Chrome")
        );
    }

    #[test]
    fn a_running_user_browser_gets_a_separate_agents_instance() {
        let user = (100, s(&["/Applications/Dia.app/Contents/MacOS/Dia"]));
        let plan = plan(true, &[], &[user], &[], &profile(), false);
        let Plan::Start(switches) = plan else {
            panic!("{plan:?}")
        };
        assert_eq!(
            switches[0],
            format!("--user-data-dir={}", profile().display())
        );
        assert!(switches.contains(&"--no-first-run".to_string()));
    }

    #[test]
    fn a_running_agents_instance_takes_the_new_window() {
        let user = (100, s(&["Dia"]));
        let mut argv = s(&["Dia", "--remote-debugging-pipe"]);
        argv.extend(switches(&profile()));
        let agents = (200, argv);
        assert_eq!(
            plan(
                true,
                &[],
                &[user.clone(), agents.clone()],
                &[200],
                &profile(),
                false
            ),
            Plan::Reuse(200)
        );
        // Left from an earlier run of Agent Screen, with no pipe to it.
        assert_eq!(
            plan(true, &[], &[user, agents], &[], &profile(), false),
            Plan::Replace(200)
        );
    }

    #[test]
    fn the_agents_instance_is_reused_when_the_user_quit_theirs() {
        let mut argv = s(&["Dia"]);
        argv.extend(switches(&profile()));
        assert_eq!(
            plan(true, &[], &[(200, argv)], &[200], &profile(), false),
            Plan::Reuse(200)
        );
    }

    #[test]
    fn a_profile_the_agent_named_wins() {
        let user = (100, s(&["Dia"]));
        for named in [
            s(&["--user-data-dir=/tmp/p"]),
            s(&["--user-data-dir", "/tmp/p"]),
        ] {
            assert_eq!(
                plan(
                    true,
                    &named,
                    std::slice::from_ref(&user),
                    &[],
                    &profile(),
                    false
                ),
                Plan::AsAsked
            );
        }
    }

    #[test]
    fn a_browser_no_one_runs_starts_on_the_agents_profile() {
        assert_eq!(
            plan(true, &[], &[], &[], &profile(), false),
            Plan::Start(switches(&profile()))
        );
        assert_eq!(
            plan(false, &[], &[(100, s(&["Safari"]))], &[], &profile(), false),
            Plan::AsAsked
        );
    }

    #[test]
    fn only_a_quit_beside_the_users_running_browser_is_a_refusal() {
        assert!(was_refused(true, true));
        assert!(!was_refused(true, false));
        assert!(!was_refused(false, true));
    }

    #[test]
    fn the_agents_instance_opens_no_debugging_port() {
        let given = s(&[
            "--remote-debugging-port=9222",
            "--remote-debugging-address=0.0.0.0",
            "--remote-debugging-port",
            "--lang=en",
            "https://example.com",
        ]);
        assert_eq!(
            without_debugging_port(&given),
            s(&["--lang=en", "https://example.com"])
        );
    }

    #[test]
    fn another_profile_is_not_the_agents() {
        let argv = s(&["Dia", "--user-data-dir=/tmp/other"]);
        assert!(!is_agents(&argv, &profile()));
        // A profile folder that only starts with the agents' path.
        let longer = s(&[&format!("--user-data-dir={}-2", profile().display())]);
        assert!(!is_agents(&longer, &profile()));
    }

    #[test]
    fn a_browser_that_refuses_a_second_instance_shares_the_users() {
        let dia = "company.thebrowser.dia";
        let record = with_refusal("", dia, "1.2.3");
        assert!(refuses(&record, dia, "1.2.3"));
        assert!(!refuses(&record, dia, "1.2.4"));
        assert!(!refuses(&record, "com.google.Chrome", "1.2.3"));
        let user = (100, s(&["Dia"]));
        assert_eq!(
            plan(
                true,
                &[],
                std::slice::from_ref(&user),
                &[],
                &profile(),
                true
            ),
            Plan::Shared
        );
        // No one runs it: it starts as before, on its own profile.
        assert_eq!(plan(true, &[], &[], &[], &profile(), true), Plan::AsAsked);
        // A profile the agent named still wins.
        let named = s(&["--user-data-dir=/tmp/p"]);
        assert_eq!(
            plan(true, &named, &[user], &[], &profile(), true),
            Plan::AsAsked
        );
    }

    #[test]
    fn a_window_the_size_of_the_main_screen_overflows_the_display() {
        assert!(overflows((1728.0, 1084.0), (1280.0, 800.0)));
        assert!(overflows((1200.0, 801.0), (1280.0, 800.0)));
        assert!(!overflows((1200.0, 720.0), (1280.0, 800.0)));
        assert!(!overflows((1280.0, 800.0), (1280.0, 800.0)));
    }

    #[test]
    fn a_tab_answers_only_in_a_window_on_the_launchs_display() {
        use crate::model::Rect;
        let display = Rect::new(3880.0, -760.0, 1280.0, 800.0);
        let on = Rect::new(3904.0, -736.0, 928.0, 544.0);
        let users = Rect::new(400.0, 286.0, 928.0, 544.0);
        assert_eq!(tab_landing(on, display), TabLanding::ThisDisplay);
        assert_eq!(tab_landing(users, display), TabLanding::Elsewhere);
        assert!(same_frame(on, Rect::new(3905.0, -735.5, 927.0, 544.0)));
        assert!(!same_frame(on, users));
    }

    #[test]
    fn an_update_replaces_the_refusal_of_an_older_version() {
        let dia = "company.thebrowser.dia";
        let record = with_refusal("com.other 9\n", dia, "1.0");
        let record = with_refusal(&record, dia, "2.0");
        assert_eq!(record, "com.other 9\ncompany.thebrowser.dia 2.0\n");
    }
}
