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

/// How `launch_app` starts a browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// As the agent asked: not a Chromium browser, a profile the agent
    /// named, or a browser no one runs yet.
    AsAsked,
    /// Start the agents' instance with these switches ahead of the agent's
    /// arguments: the user runs the browser and the agents' instance
    /// doesn't run.
    Start(Vec<String>),
    /// The agents' instance runs as this pid: open the new window there.
    Reuse(i32),
    /// The browser refuses a second instance, so the window opens in the
    /// user's process as before, and shares its focus (#110).
    Shared,
}

/// `running`: the browser's processes, each with its command line.
/// `refuses`: the browser is known to refuse a second instance.
pub fn plan(
    chromium_browser: bool,
    arguments: &[String],
    running: &[(i32, Vec<String>)],
    profile: &Path,
    refuses: bool,
) -> Plan {
    if !chromium_browser || names_profile(arguments) {
        return Plan::AsAsked;
    }
    if let Some((pid, _)) = running.iter().find(|(_, argv)| is_agents(argv, profile)) {
        return Plan::Reuse(*pid);
    }
    if running.is_empty() {
        return Plan::AsAsked;
    }
    if refuses {
        return Plan::Shared;
    }
    Plan::Start(switches(profile))
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
        let plan = plan(true, &[], &[user], &profile(), false);
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
            plan(true, &[], &[user, agents], &profile(), false),
            Plan::Reuse(200)
        );
    }

    #[test]
    fn the_agents_instance_is_reused_when_the_user_quit_theirs() {
        let mut argv = s(&["Dia"]);
        argv.extend(switches(&profile()));
        assert_eq!(
            plan(true, &[], &[(200, argv)], &profile(), false),
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
                plan(true, &named, std::slice::from_ref(&user), &profile(), false),
                Plan::AsAsked
            );
        }
    }

    #[test]
    fn a_browser_no_one_runs_or_another_app_starts_as_before() {
        assert_eq!(plan(true, &[], &[], &profile(), false), Plan::AsAsked);
        assert_eq!(
            plan(false, &[], &[(100, s(&["Safari"]))], &profile(), false),
            Plan::AsAsked
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
            plan(true, &[], std::slice::from_ref(&user), &profile(), true),
            Plan::Shared
        );
        // No one runs it: it starts as before, on its own profile.
        assert_eq!(plan(true, &[], &[], &profile(), true), Plan::AsAsked);
        // A profile the agent named still wins.
        let named = s(&["--user-data-dir=/tmp/p"]);
        assert_eq!(plan(true, &named, &[user], &profile(), true), Plan::AsAsked);
    }

    #[test]
    fn an_update_replaces_the_refusal_of_an_older_version() {
        let dia = "company.thebrowser.dia";
        let record = with_refusal("com.other 9\n", dia, "1.0");
        let record = with_refusal(&record, dia, "2.0");
        assert_eq!(record, "com.other 9\ncompany.thebrowser.dia 2.0\n");
    }
}
