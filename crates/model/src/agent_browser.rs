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

/// The browsers whose agents' instance has been verified on a display
/// (ADR 0022): it starts beside the user's without taking the front, its
/// windows come through the pipe onto the display, posted input reaches its
/// pages, and it quits with nothing on the user's screen. Google Chrome
/// passed `agentbrowser` on 2026-11-19. Dia did not yet: its windows made
/// through the pipe never show, and it moves a window it closes to the
/// middle of the main screen. Any other Chromium browser keeps the
/// behavior from before ADR 0022 until it is verified.
pub const VERIFIED: [&str; 1] = ["com.google.Chrome"];

/// Whether the browser `bundle_id` gets the agents' own instance.
pub fn verified(bundle_id: &str) -> bool {
    VERIFIED.contains(&bundle_id)
}

/// `verified`: the browser is on [`VERIFIED`]. One that isn't opens the
/// agents' windows in the user's process when the user runs it, as before
/// ADR 0022, and starts as asked when no one does.
/// `running`: the browser's processes, each with its command line.
/// `piped`: the pids Agent Screen holds a DevTools pipe to.
/// `refuses`: the browser is known to refuse a second instance.
pub fn plan(
    chromium_browser: bool,
    verified: bool,
    arguments: &[String],
    running: &[(i32, Vec<String>)],
    piped: &[i32],
    profile: &Path,
    refuses: bool,
) -> Plan {
    if !chromium_browser || names_profile(arguments) {
        return Plan::AsAsked;
    }
    if !verified {
        return if running.is_empty() {
            Plan::AsAsked
        } else {
            Plan::Shared
        };
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

/// What Agent Screen does when the agents' instance comes to the front by
/// something other than Agent Screen (ADR 0022).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentsActivated {
    /// The user is using its window, in the Viewer: theirs to use.
    Leave,
    /// Give the front back, so the agents' profile never takes the user's
    /// typing. Without positive evidence of a user's launch, as when the
    /// instance brings itself forward for a popup, nothing more.
    GiveBack,
    /// Give the front back and start the user's own browser: the user just
    /// pressed the Dock (Launchpad included), Spotlight, Finder or another
    /// launcher, which is how they open an app, and macOS handed that to
    /// the agents' instance because the user's own isn't running.
    GiveBackAndStartUsers,
}

/// Where one of the user's presses went, as the agents' instance comes to
/// the front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// To one of the instance's own windows, in the Viewer.
    ItsWindow,
    /// To an app that opens apps: the Dock (Launchpad included),
    /// Spotlight, Finder, Raycast or Alfred.
    Opener,
    /// Anywhere else, or nowhere known.
    Other,
}

/// `recent`: the user's presses just before, oldest first. Only the last
/// one counts: a Dock click followed by typing elsewhere was no launch.
/// `users_running`: the user's own instance runs, which macOS would have
/// picked for their launch.
pub fn agents_activated(recent: &[Press], users_running: bool) -> AgentsActivated {
    match recent.last() {
        Some(Press::ItsWindow) => AgentsActivated::Leave,
        Some(Press::Opener) if !users_running => AgentsActivated::GiveBackAndStartUsers,
        _ => AgentsActivated::GiveBack,
    }
}

/// The app to give the front back to when `pid` came to the front by
/// itself after `previous`, if any. Agent Screen is `own`. An app an agent
/// started that comes forward over Agent Screen, as the user works in the
/// Viewer, stays (#41). The agents' browser instance never does: the
/// user's next keys would go to the agents' profile, so Agent Screen takes
/// the front back for itself too.
pub fn front_to_return(
    previous: Option<i32>,
    own: i32,
    pid: i32,
    agents_instance: bool,
) -> Option<i32> {
    previous.filter(|&p| p != pid && (p != own || agents_instance))
}

/// What Agent Screen does with a page that appears in the agents' instance
/// (#235).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewPage {
    /// The agent's page, or nothing to move.
    Keep,
    /// No address yet: decide once it has one.
    Wait,
    /// A link macOS handed the agents' instance, as the user clicked it in
    /// another app: it opens in the user's own browser instead.
    Forward,
}

/// What is known as a page appears in the agents' instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageOrigin {
    /// A page opened it, as a link with a target or `window.open`.
    pub opener: bool,
    /// Agent Screen was opening a page through its pipe.
    pub through_pipe: bool,
    /// An agent drove the instance in the last `LINK_SECS`: input or an
    /// element action on a display that shows its window, as Command-T or
    /// a Command-click.
    pub agent_acted: bool,
    /// The user clicked or pressed a key outside Agent Screen in the last
    /// `LINK_SECS`, as the global monitors record. Agent Screen's posted
    /// events never reach them.
    pub user_acted: bool,
}

/// A page goes to the user's own browser only as a link the user just
/// followed: the user clicked or pressed a key, no agent drove the
/// instance, and nothing in the instance opened it. Anything else stays:
/// an agent's `open <url>` from its shell, a native app on an agent's
/// display opening a sign-in page, or a tab Chrome or an extension opens
/// by itself has no user input behind it.
pub fn new_page(origin: PageOrigin, url: &str) -> NewPage {
    let PageOrigin {
        opener,
        through_pipe,
        agent_acted,
        user_acted,
    } = origin;
    if opener || through_pipe || agent_acted || !user_acted {
        return NewPage::Keep;
    }
    let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        None | Some("about") if url.is_empty() || url == "about:blank" => NewPage::Wait,
        Some("http" | "https" | "file") => NewPage::Forward,
        _ => NewPage::Keep,
    }
}

/// How long a page with no address yet may still be a link.
pub const LINK_SECS: u64 = 3;

/// Who may use the agents' instance: launches that open a window in it,
/// and the quit once it is idle. A quit never starts while a launch holds
/// the instance, and a launch waits for a quit that started to end, so a
/// window being opened is never closed with the instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentsUse {
    holds: usize,
    quitting: usize,
}

impl AgentsUse {
    pub const fn new() -> Self {
        AgentsUse {
            holds: 0,
            quitting: 0,
        }
    }

    /// A launch takes the instance. False while a quit runs: wait for it.
    pub fn hold(&mut self) -> bool {
        if self.quitting > 0 {
            return false;
        }
        self.holds += 1;
        true
    }

    /// A launch takes the instance even while a quit runs, after waiting
    /// long enough for it.
    pub fn force_hold(&mut self) {
        self.holds += 1;
    }

    pub fn release(&mut self) {
        self.holds = self.holds.saturating_sub(1);
    }

    /// A quit starts, unless a launch holds the instance. `force`, as
    /// Agent Screen quits, starts it anyway.
    pub fn begin_quit(&mut self, force: bool) -> bool {
        if self.holds > 0 && !force {
            return false;
        }
        self.quitting += 1;
        true
    }

    pub fn end_quit(&mut self) {
        self.quitting = self.quitting.saturating_sub(1);
    }
}

/// A starting instance has no window yet for this long: it isn't idle.
pub const START_SECS: u64 = 30;

/// Whether the agents' instance, `age_secs` old, with `pages` page targets
/// as its pipe counts them (None when it doesn't answer), has nothing left
/// and quits. Counted through the pipe, a window the user minimized, hid
/// or moved to another Space still counts.
pub fn idle(age_secs: u64, pages: Option<usize>) -> bool {
    age_secs > START_SECS && pages == Some(0)
}

/// The switch that starts Chromium without its startup window.
pub const NO_STARTUP_WINDOW: &str = "--no-startup-window";

/// Browsers that keep their pages in windows of their own, which Chromium
/// gives no frame: measured on 2026-11-19, Dia answers a new window through
/// the pipe with one of no size that never shows, and opens its startup
/// window without taking the front.
const OWN_WINDOWS: [&str; 1] = ["company.thebrowser.dia"];

/// Whether the browser `bundle_id` opens a real window when its pipe asks:
/// then the agents' instance starts with no window and gets its first one
/// through the pipe, since Chromium shows its startup window with
/// activation.
pub fn pipe_makes_windows(bundle_id: &str) -> bool {
    !OWN_WINDOWS.contains(&bundle_id)
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
        let plan = plan(true, true, &[], &[user], &[], &profile(), false);
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
            plan(true, true, &[], &[user, agents], &[], &profile(), false),
            Plan::Replace(200)
        );
    }

    #[test]
    fn the_agents_instance_is_reused_when_the_user_quit_theirs() {
        let mut argv = s(&["Dia"]);
        argv.extend(switches(&profile()));
        assert_eq!(
            plan(true, true, &[], &[(200, argv)], &[200], &profile(), false),
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
            plan(true, true, &[], &[], &[], &profile(), false),
            Plan::Start(switches(&profile()))
        );
        assert_eq!(
            plan(
                false,
                true,
                &[],
                &[(100, s(&["Safari"]))],
                &[],
                &profile(),
                false
            ),
            Plan::AsAsked
        );
    }

    #[test]
    fn dia_keeps_its_own_windows_and_chrome_takes_them_through_the_pipe() {
        assert!(!pipe_makes_windows("company.thebrowser.dia"));
        assert!(pipe_makes_windows("com.google.Chrome"));
    }

    #[test]
    fn a_launch_from_an_opener_that_reached_the_agents_instance_starts_the_users() {
        // A Dock, Spotlight or Finder press just before, the user's own
        // browser not running: their intent was to open the browser.
        assert_eq!(
            agents_activated(&[Press::Opener], false),
            AgentsActivated::GiveBackAndStartUsers
        );
    }

    #[test]
    fn a_self_activation_only_gives_the_front_back() {
        // No input, as for a popup the agents' instance shows by
        // itself: the user's browser never starts unasked.
        assert_eq!(agents_activated(&[], false), AgentsActivated::GiveBack);
        // The user's own runs: macOS would have picked it for their launch.
        assert_eq!(
            agents_activated(&[Press::Opener], true),
            AgentsActivated::GiveBack
        );
        // The user clicked the agents' window, in the Viewer.
        assert_eq!(
            agents_activated(&[Press::ItsWindow], false),
            AgentsActivated::Leave
        );
    }

    #[test]
    fn only_the_users_last_press_decides_the_agents_activation() {
        use AgentsActivated::*;
        use Press::*;
        assert_eq!(
            agents_activated(&[Other, Opener], false),
            GiveBackAndStartUsers
        );
        assert_eq!(agents_activated(&[Opener, Other], false), GiveBack);
        assert_eq!(agents_activated(&[Opener, ItsWindow], false), Leave);
        assert_eq!(
            agents_activated(&[ItsWindow, Opener], false),
            GiveBackAndStartUsers
        );
    }

    #[test]
    fn the_agents_instance_over_agent_screen_gives_the_front_back_to_it() {
        let (own, pid, user) = (10, 20, 30);
        assert_eq!(front_to_return(Some(own), own, pid, true), Some(own));
        assert_eq!(front_to_return(Some(own), own, pid, false), None);
        assert_eq!(front_to_return(Some(user), own, pid, false), Some(user));
        assert_eq!(front_to_return(Some(pid), own, pid, true), None);
        assert_eq!(front_to_return(None, own, pid, true), None);
    }

    #[test]
    fn a_launch_holding_the_agents_instance_keeps_the_idle_quit_off() {
        let mut using = AgentsUse::default();
        assert!(using.hold());
        assert!(!using.begin_quit(false));
        assert!(using.begin_quit(true));
        using.end_quit();
        using.release();
        assert!(using.begin_quit(false));
        assert!(!using.hold());
        using.end_quit();
        assert!(using.hold());
    }

    #[test]
    fn only_a_link_the_user_just_followed_is_forwarded() {
        use NewPage::*;
        let user = PageOrigin {
            user_acted: true,
            ..PageOrigin::default()
        };
        assert_eq!(new_page(user, "https://example.com/a"), Forward);
        assert_eq!(new_page(user, "file:///Users/x/a.html"), Forward);
        assert_eq!(new_page(user, ""), Wait);
        assert_eq!(new_page(user, "about:blank"), Wait);
        // The agent's own pages: a target link, a page Agent Screen opens,
        // and Command-T or a Command-click, even as the user types too.
        for agents in [
            PageOrigin {
                opener: true,
                ..user
            },
            PageOrigin {
                through_pipe: true,
                ..user
            },
            PageOrigin {
                agent_acted: true,
                ..user
            },
        ] {
            assert_eq!(new_page(agents, "https://example.com"), Keep);
        }
        // A new tab page, settings or an extension's page isn't a link.
        assert_eq!(new_page(user, "chrome://newtab/"), Keep);
        assert_eq!(new_page(user, "chrome-extension://abc/x.html"), Keep);
    }

    #[test]
    fn a_page_no_user_input_explains_stays_in_the_agents_instance() {
        // An agent runs `open https://...` from its shell: no input at all.
        assert_eq!(
            new_page(PageOrigin::default(), "https://example.com/a"),
            NewPage::Keep
        );
        // A native app on an agent's display opens its sign-in page: the
        // agent clicked that app, not the browser, and the user did nothing.
        assert_eq!(
            new_page(PageOrigin::default(), "https://slack.com/signin"),
            NewPage::Keep
        );
        // Chrome or an extension opens a tab by itself, as after an update
        // or an install, with no page as its opener.
        assert_eq!(
            new_page(
                PageOrigin::default(),
                "https://www.google.com/chrome/whats-new/"
            ),
            NewPage::Keep
        );
        // A page with no address yet and no user input isn't waited on.
        assert_eq!(new_page(PageOrigin::default(), ""), NewPage::Keep);
    }

    #[test]
    fn the_agents_instance_is_idle_once_its_pipe_counts_no_page() {
        assert!(idle(31, Some(0)));
        assert!(!idle(31, Some(1)));
        assert!(!idle(5, Some(0)));
        assert!(!idle(31, None));
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
        assert_eq!(
            plan(true, true, &[], &[], &[], &profile(), true),
            Plan::AsAsked
        );
        // A profile the agent named still wins.
        let named = s(&["--user-data-dir=/tmp/p"]);
        assert_eq!(
            plan(true, true, &named, &[user], &[], &profile(), true),
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
