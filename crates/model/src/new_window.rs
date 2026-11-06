//! Whose a new window of an app an Agent Session started is (ADR 0007).
//! The catch decides as macOS reports the window. The launch's watch
//! decides, from the same facts, a window of an app the display started
//! cold that the catch couldn't: one an app makes while its main thread is
//! too busy to answer Accessibility, as WeCom's main window is, or before
//! the app takes observers.

use std::time::Duration;

/// How long after an action a new window counts as its result.
pub const ACTION_WINDOW: Duration = Duration::from_secs(3);

/// How long a cold start counts as driving the display, for that app
/// alone, whether `launch_app` has answered with a window or not: a slow
/// app shows its first window late, and one with a loading window shows
/// its main window seconds after.
pub const COLD_START: Duration = Duration::from_secs(30);

/// What is known as a new window of the app appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The display was driven in the last `ACTION_WINDOW`, by its agent or
    /// through its Viewer, or started this app cold in the last
    /// `COLD_START`; and no other display that shows the app was driven
    /// meanwhile.
    pub agent: bool,
    /// The user pressed something in the last `ACTION_WINDOW` that could
    /// make the app open a window: a key or click in it, or in Finder, the
    /// Dock or a launcher.
    pub user: bool,
    /// None of the app's other windows is off the display, so the user
    /// isn't using it.
    pub only_here: bool,
}

/// Whose a new window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Agent,
    User,
    /// Both acted, or neither while the app has a window elsewhere: the
    /// window stays where it opened, for the agent to place if it's its own.
    Unknown,
}

impl Seen {
    pub fn owner(self) -> Owner {
        match (self.agent, self.user) {
            (true, false) => Owner::Agent,
            (false, true) => Owner::User,
            // A window the app makes by itself while nobody acted, as a VM
            // viewer does when its guest reboots (#46).
            (false, false) if self.only_here => Owner::Agent,
            _ => Owner::Unknown,
        }
    }
}

/// Where a new window is when the launch's watch finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    /// The display that started the app.
    ThisDisplay,
    /// Another Agent Display, which holds it.
    OtherDisplay,
    /// The user's screen.
    Elsewhere,
}

/// What the launch's watch does with a new window of an app the display
/// started cold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// On another Agent Display, decided by the catch, or of an app the
    /// user took back: nothing to do.
    Leave,
    /// On the display already, as the catch put it: settle it there, as
    /// `launch_app` does its window.
    Record,
    /// The agent's: onto the display.
    Place,
    /// The user's: nothing moves it later.
    Theirs,
    /// Nobody's for sure: it stays, and the agent's next screenshot names it.
    Undecided,
}

/// `decided`: the catch left the window to the user or left it undecided,
/// or the user took the app back.
pub fn watch(on: On, decided: bool, seen: Seen) -> Watch {
    match (on, decided) {
        (_, true) | (On::OtherDisplay, _) => Watch::Leave,
        (On::ThisDisplay, false) => Watch::Record,
        (On::Elsewhere, false) => match seen.owner() {
            Owner::Agent => Watch::Place,
            Owner::User => Watch::Theirs,
            Owner::Unknown => Watch::Undecided,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(agent: bool, user: bool, only_here: bool) -> Seen {
        Seen {
            agent,
            user,
            only_here,
        }
    }

    #[test]
    fn a_window_is_the_agents_only_when_the_user_did_nothing() {
        assert_eq!(seen(true, false, false).owner(), Owner::Agent);
        assert_eq!(seen(false, true, false).owner(), Owner::User);
        assert_eq!(seen(false, true, true).owner(), Owner::User);
        assert_eq!(
            seen(true, true, true).owner(),
            Owner::Unknown,
            "the user's own click may have made it"
        );
        assert_eq!(
            seen(false, false, false).owner(),
            Owner::Unknown,
            "a slow window, with others of the app on the user's screen, stays placeable"
        );
        assert_eq!(
            seen(false, false, true).owner(),
            Owner::Agent,
            "an app on the display alone makes a window by itself"
        );
    }

    #[test]
    fn a_cold_start_counts_past_the_action_window() {
        // WeCom's main window, 3.4 s after its loading window, which
        // launch_app had answered with, while it had no window elsewhere.
        assert_eq!(
            watch(On::Elsewhere, false, seen(true, false, true)),
            Watch::Place
        );
        // The same with a window of it on the user's screen.
        assert_eq!(
            watch(On::Elsewhere, false, seen(true, false, false)),
            Watch::Place
        );
    }

    #[test]
    fn the_watch_leaves_the_users_windows_to_them() {
        assert_eq!(
            watch(On::Elsewhere, false, seen(false, true, true)),
            Watch::Theirs,
            "a window the user opened in the app is theirs"
        );
        assert_eq!(
            watch(On::Elsewhere, false, seen(true, true, true)),
            Watch::Undecided,
            "a window that opens while both act is moved by no one"
        );
        assert_eq!(
            watch(On::Elsewhere, false, seen(false, false, false)),
            Watch::Undecided
        );
    }

    #[test]
    fn the_watch_leaves_what_is_placed_or_decided() {
        let agents = seen(true, false, true);
        assert_eq!(
            watch(On::ThisDisplay, false, agents),
            Watch::Record,
            "caught already, recorded as launched"
        );
        assert_eq!(watch(On::OtherDisplay, false, agents), Watch::Leave);
        for on in [On::ThisDisplay, On::Elsewhere] {
            assert_eq!(
                watch(on, true, agents),
                Watch::Leave,
                "the catch decided it, or the user took the app back"
            );
        }
    }
}
