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

/// How long after the launch the watch keeps waiting for a busy app to
/// say what a window it saw in the cold start is. A window still
/// unanswered then is left where it is, and the agent is told.
pub const RESOLVE_BY: Duration = Duration::from_secs(60);

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

    /// Whether this window ends the cold start's 30 s for its app: the
    /// user acted as it opened, in the app or in Finder, the Dock or a
    /// launcher, which open apps' windows (`user`), so they may use it too.
    /// Its later windows count as the agent's only right after the agent
    /// acts, as any app's do, or the link they click in Mail would open on
    /// the Agent Display.
    pub fn ends_cold_start(self) -> bool {
        self.user
    }

    /// The facts from when a window first showed, with whether the user
    /// has acted since: a window they clicked into while the app was too
    /// busy to describe it isn't the agent's.
    pub fn with_user_now(self, user: bool) -> Seen {
        Seen {
            user: self.user || user,
            ..self
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
    /// The app doesn't answer what the window is yet: look again on the
    /// next tick, with the facts from when it first showed.
    Later,
}

/// What Accessibility says a new window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// An AXWindow, as the catch takes.
    Window,
    /// A sheet, a panel the app doesn't list, or anything else the catch
    /// leaves where the app puts it.
    Other,
    /// The app didn't answer in time, as one too busy building the window
    /// doesn't: ask again.
    Unanswered,
    /// The app answered with an error, and asking again won't help.
    Unreadable,
}

/// The app's answer for its AXWindows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listing<'a> {
    /// No answer within the messaging timeout (kAXErrorCannotComplete).
    TimedOut,
    /// Another error, such as an app that doesn't implement Accessibility.
    Failed,
    /// The window's role if the list holds it, `None` for the role when
    /// the app didn't answer for that.
    Answered(Option<Option<&'a str>>),
}

/// What the window is, from the app's answer for its AXWindows.
pub fn role(listing: Listing) -> Role {
    match listing {
        Listing::TimedOut | Listing::Answered(Some(None)) => Role::Unanswered,
        Listing::Failed => Role::Unreadable,
        // An app that answers and doesn't list it: a sheet, or a window it
        // keeps out of its list.
        Listing::Answered(None) => Role::Other,
        Listing::Answered(Some(Some("AXWindow"))) => Role::Window,
        Listing::Answered(Some(Some(_))) => Role::Other,
    }
}

/// `decided`: the catch left the window to the user or left it undecided,
/// or the user took the app back.
pub fn watch(on: On, decided: bool, role: Role, seen: Seen) -> Watch {
    match (on, decided, role) {
        (_, true, _)
        | (On::OtherDisplay, ..)
        | (_, _, Role::Other)
        | (On::ThisDisplay, _, Role::Unreadable) => Watch::Leave,
        // It can't be moved, and may be anything: the agent is told.
        (On::Elsewhere, false, Role::Unreadable) => Watch::Undecided,
        (_, false, Role::Unanswered) => Watch::Later,
        (On::ThisDisplay, false, Role::Window) => Watch::Record,
        (On::Elsewhere, false, Role::Window) => match seen.owner() {
            Owner::Agent => Watch::Place,
            Owner::User => Watch::Theirs,
            Owner::Unknown => Watch::Undecided,
        },
    }
}

/// What the launch's watch does at the start of a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The display ended, or the app is no longer one it started, as once
    /// the user takes a window of it back: every window of the app is the
    /// user's, and the watch ends. Or the cold start is over, with no
    /// window waiting.
    Stop,
    /// The app finished starting hidden, as an Electron development build
    /// does after the launch showed it, so its windows are listed nowhere:
    /// move them onto the display and show it.
    ShowHidden,
    /// Hidden after it showed a window, as by the user's Command-H, or
    /// with a window the user made: that window may be theirs, so nothing
    /// moves.
    Skip,
    /// Look for new windows, and at those waiting.
    Look,
    /// Past the cold start: look only at the windows still waiting for
    /// the app to say what they are.
    Resolve,
    /// Past `RESOLVE_BY`: leave the windows still waiting where they are,
    /// and tell the agent.
    GiveUp,
}

/// What is known at the start of a watch tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Since the launch.
    pub elapsed: Duration,
    /// The display takes windows and still counts the app as one it
    /// started.
    pub started: bool,
    pub hidden: bool,
    /// `launch_app` answered with a window of the app, or the watch has
    /// seen one.
    pub shown: bool,
    /// A window of the hidden app is the user's: they made it, took it
    /// back, or it was left with them undecided.
    pub theirs: bool,
    /// Windows wait for the app to say what they are.
    pub waiting: bool,
}

pub fn step(t: Tick) -> Step {
    let past = t.elapsed >= COLD_START;
    if !t.started {
        Step::Stop
    } else if t.elapsed >= RESOLVE_BY {
        if t.waiting { Step::GiveUp } else { Step::Stop }
    } else if past && !t.waiting {
        Step::Stop
    } else if t.hidden {
        if t.shown || t.theirs || past {
            Step::Skip
        } else {
            Step::ShowHidden
        }
    } else if past {
        Step::Resolve
    } else {
        Step::Look
    }
}

/// Whether the watch's tick `tick`, one per 150 ms, tries again for a
/// catch on an app that has none. Asking a busy app can hold the catch
/// thread, which every display's catches share, for 1 s, so the tries
/// thin out: about 1, 2, 4, 8 and 17 s in.
pub fn retries_catch(tick: u32) -> bool {
    tick == 0 || tick.is_multiple_of(7) && (tick / 7).is_power_of_two()
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
            watch(On::Elsewhere, false, Role::Window, seen(true, false, true)),
            Watch::Place
        );
        // The same with a window of it on the user's screen.
        assert_eq!(
            watch(On::Elsewhere, false, Role::Window, seen(true, false, false)),
            Watch::Place
        );
    }

    #[test]
    fn the_watch_leaves_the_users_windows_to_them() {
        assert_eq!(
            watch(On::Elsewhere, false, Role::Window, seen(false, true, true)),
            Watch::Theirs,
            "a window the user opened in the app is theirs"
        );
        assert_eq!(
            watch(On::Elsewhere, false, Role::Window, seen(true, true, true)),
            Watch::Undecided,
            "a window that opens while both act is moved by no one"
        );
        assert_eq!(
            watch(
                On::Elsewhere,
                false,
                Role::Window,
                seen(false, false, false)
            ),
            Watch::Undecided
        );
    }

    #[test]
    fn the_watch_leaves_what_is_placed_or_decided() {
        let agents = seen(true, false, true);
        assert_eq!(
            watch(On::ThisDisplay, false, Role::Window, agents),
            Watch::Record,
            "caught already, recorded as launched"
        );
        assert_eq!(
            watch(On::OtherDisplay, false, Role::Window, agents),
            Watch::Leave
        );
        for on in [On::ThisDisplay, On::Elsewhere] {
            assert_eq!(
                watch(on, true, Role::Window, agents),
                Watch::Leave,
                "the catch decided it, or the user took the app back"
            );
        }
    }

    #[test]
    fn the_watch_settles_only_what_the_catch_would_take() {
        let agents = seen(true, false, true);
        for on in [On::ThisDisplay, On::Elsewhere] {
            assert_eq!(
                watch(on, false, Role::Other, agents),
                Watch::Leave,
                "a sheet or panel stays where the app puts it"
            );
            assert_eq!(
                watch(on, false, Role::Unanswered, agents),
                Watch::Later,
                "a busy app's window is decided once it answers"
            );
            assert_eq!(watch(on, true, Role::Unanswered, agents), Watch::Leave);
        }
        assert_eq!(
            watch(On::OtherDisplay, false, Role::Unanswered, agents),
            Watch::Leave
        );
    }

    #[test]
    fn the_user_acting_on_the_app_ends_its_cold_start() {
        assert!(
            seen(true, true, false).ends_cold_start(),
            "a window they opened from the Dock as the app started"
        );
        assert!(seen(false, true, true).ends_cold_start());
        assert!(!seen(true, false, true).ends_cold_start());
        assert!(!seen(false, false, true).ends_cold_start());
    }

    fn tick(elapsed: u64, hidden: bool, shown: bool) -> Tick {
        Tick {
            elapsed: Duration::from_secs(elapsed),
            started: true,
            hidden,
            shown,
            ..Tick::default()
        }
    }

    #[test]
    fn the_watch_moves_a_hidden_app_only_before_it_shows_a_window() {
        assert_eq!(step(tick(5, true, false)), Step::ShowHidden);
        assert_eq!(
            step(tick(5, true, true)),
            Step::Skip,
            "hidden by the user after a window showed, which may be theirs"
        );
        assert_eq!(
            step(Tick {
                theirs: true,
                ..tick(5, true, false)
            }),
            Step::Skip,
            "a window the user made, which the watch hadn't seen yet"
        );
        assert_eq!(step(tick(5, false, false)), Step::Look);
        assert_eq!(step(tick(5, false, true)), Step::Look);
        for hidden in [false, true] {
            for shown in [false, true] {
                for elapsed in [5, 40, 70] {
                    assert_eq!(
                        step(Tick {
                            started: false,
                            waiting: true,
                            ..tick(elapsed, hidden, shown)
                        }),
                        Step::Stop,
                        "taken back, or the display ended: all its windows are the user's"
                    );
                }
            }
        }
    }

    #[test]
    fn a_window_still_waiting_is_resolved_or_reported() {
        let waiting = |elapsed| Tick {
            waiting: true,
            ..tick(elapsed, false, true)
        };
        assert_eq!(step(tick(31, false, true)), Step::Stop);
        assert_eq!(
            step(waiting(31)),
            Step::Resolve,
            "past the cold start, a window seen in it is still decided"
        );
        assert_eq!(
            step(Tick {
                hidden: true,
                ..waiting(31)
            }),
            Step::Skip
        );
        assert_eq!(
            step(waiting(60)),
            Step::GiveUp,
            "left where it is, and named to the agent"
        );
        assert_eq!(step(tick(60, false, true)), Step::Stop);
    }

    #[test]
    fn a_window_the_app_doesnt_answer_for_is_unanswered() {
        assert_eq!(
            role(Listing::TimedOut),
            Role::Unanswered,
            "AXWindows timed out: not a sheet, a busy app's window"
        );
        assert_eq!(role(Listing::Answered(Some(None))), Role::Unanswered);
        assert_eq!(role(Listing::Failed), Role::Unreadable);
        assert_eq!(role(Listing::Answered(None)), Role::Other);
        assert_eq!(
            role(Listing::Answered(Some(Some("AXWindow")))),
            Role::Window
        );
        assert_eq!(role(Listing::Answered(Some(Some("AXSheet")))), Role::Other);
    }

    #[test]
    fn an_unreadable_window_is_reported_not_moved() {
        let agents = seen(true, false, true);
        assert_eq!(
            watch(On::Elsewhere, false, Role::Unreadable, agents),
            Watch::Undecided
        );
        assert_eq!(
            watch(On::ThisDisplay, false, Role::Unreadable, agents),
            Watch::Leave
        );
    }

    #[test]
    fn the_users_input_while_a_window_waits_counts() {
        let first = seen(true, false, true);
        assert_eq!(
            watch(
                On::Elsewhere,
                false,
                Role::Window,
                first.with_user_now(true)
            ),
            Watch::Undecided,
            "clicked into while the app was busy: not moved from under the pointer"
        );
        assert_eq!(first.with_user_now(false), first);
        let users = seen(false, true, false);
        assert_eq!(users.with_user_now(false), users);
    }

    #[test]
    fn retries_for_a_catch_thin_out() {
        let tries: Vec<u32> = (0..200).filter(|&t| retries_catch(t)).collect();
        assert_eq!(tries, [0, 7, 14, 28, 56, 112]);
    }
}
