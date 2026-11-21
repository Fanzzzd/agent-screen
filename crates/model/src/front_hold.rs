//! The front hold (ADR 0013, "Let the agent continue"): actions waiting in
//! the Input Queue for the user to leave their front app run at once when
//! the user presses the button for them. Agent Screen takes the front from
//! that app for the moment the actions need, so they go in through the
//! background route, then gives the front back with the user's window key.
//! The user's keys typed meanwhile are held and go to their window after.

use crate::key_hold;
use crate::model::DisplayId;

/// How long Agent Screen waits to become the front process, from the
/// hold's start.
pub const TAKE_MS: u64 = 200;

/// When the actions must have run, from the hold's start: what is left
/// waits again.
pub const RUN_END_MS: u64 = 1200;

/// After the run, how long actions already sending may take.
pub const SETTLE_MS: u64 = 300;

/// The longest giving the front back to the user's app may take.
pub const GIVE_BACK_MS: u64 = 700;

/// What the key hold needs after that to make the user's window key and
/// send their keys on: one of its 300 ms asks, and room to send.
pub const KEYS_BACK_MS: u64 = 600;

/// The whole hold, at the latest. It ends inside the key hold's limit, so
/// the user's keys reach their window instead of timing out while Agent
/// Screen is in front, where they would be dropped.
pub const BUDGET_MS: u64 = RUN_END_MS + SETTLE_MS + GIVE_BACK_MS + KEYS_BACK_MS;
const _: () = assert!(TAKE_MS < RUN_END_MS && BUDGET_MS <= key_hold::MAX_HOLD_MS);

/// A hold older than this is left over, as from a panic partway: a new
/// press may start in its place.
pub const STALE_MS: u64 = 10_000;
const _: () = assert!(STALE_MS > key_hold::MAX_HOLD_MS);

/// How a hold starts, for an action waiting on app `waits_on`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The app is in front: Agent Screen takes the front from it.
    Take,
    /// Agent Screen is in front already, as the press on its button made
    /// it: the actions run, then the app gets the front back.
    InFront,
    /// Another app is in front: the actions run by themselves.
    NotNeeded,
}

pub fn start(front: Option<i32>, own: i32, waits_on: i32) -> Start {
    match front {
        Some(pid) if pid == waits_on => Start::Take,
        Some(pid) if pid == own => Start::InFront,
        _ => Start::NotNeeded,
    }
}

/// The app display `display`'s front requests wait on, from `fronts`, each
/// request's display and app: the hold takes the front from it. Only that
/// display's, and only a real app: a preview's has none.
pub fn waits_on(
    display: DisplayId,
    fronts: impl IntoIterator<Item = (DisplayId, i32)>,
) -> Option<i32> {
    fronts
        .into_iter()
        .find(|&(d, pid)| d == display && pid > 0)
        .map(|(_, pid)| pid)
}

/// A front hold going on: Agent Screen stands in front of app `app` while
/// display `display`'s actions waiting on it run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold {
    pub display: DisplayId,
    pub app: i32,
    /// It ends: no more actions start, those running finish.
    pub ending: bool,
    /// When it started, in the queue's ms.
    pub started: u64,
}

impl Hold {
    pub fn new(display: DisplayId, app: i32, now: u64) -> Self {
        Hold {
            display,
            app,
            ending: false,
            started: now,
        }
    }
}

/// Whether a new hold may start beside `current`: none goes on, or it is
/// left over (`STALE_MS`).
pub fn can_start(current: Option<&Hold>, now: u64) -> bool {
    current.is_none_or(|h| now.saturating_sub(h.started) >= STALE_MS)
}

/// Whether a front request of display `display`, waiting on app `app`, may
/// start now. During a hold of `app`, only the held display's, and not once
/// it ends: the user pressed for that display's actions only.
pub fn may_start(hold: Option<&Hold>, display: DisplayId, app: i32) -> bool {
    match hold {
        Some(h) if h.app == app => h.display == display && !h.ending,
        _ => true,
    }
}

/// Where the hold is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Agent Screen asked to be the front process.
    Taking,
    /// It is: the waiting actions run.
    Running,
}

/// What the key hold says of the user's input during the hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    /// Holding, with nothing typed.
    Quiet,
    /// Holding keys the user typed for their window.
    Typed,
    /// Holding, and the user pressed a mouse button: the press goes where
    /// they meant it, after the keys before it.
    Pressed,
    /// The key hold ended by itself, at its own limit.
    Ended,
}

/// Why the hold ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Every waiting action ran.
    Ran,
    /// Agent Screen never became the front process.
    NoFront,
    /// Another app took the front during the hold.
    FrontLost,
    /// The user typed or pressed: what is left waits again.
    UserActed(Keys),
    /// The key hold ended first: what is left waits again.
    KeysEnded,
    /// `RUN_END_MS` went by first: what is left waits again.
    TimedOut,
}

/// What the hold does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Wait,
    /// Agent Screen is the front process: let the actions run.
    Run,
    /// The hold ends.
    End(End),
}

/// The step after one look at the hold, `elapsed` ms after it started.
/// `own_front`: Agent Screen is WindowServer's front process. `pending`:
/// actions still waiting or running. The user's input ends it first.
pub fn next(phase: Phase, elapsed: u64, own_front: bool, pending: usize, keys: Keys) -> Next {
    match keys {
        Keys::Quiet => {}
        Keys::Ended => return Next::End(End::KeysEnded),
        acted => return Next::End(End::UserActed(acted)),
    }
    match phase {
        Phase::Taking if own_front => Next::Run,
        Phase::Taking if elapsed >= TAKE_MS => Next::End(End::NoFront),
        Phase::Taking => Next::Wait,
        Phase::Running if pending == 0 => Next::End(End::Ran),
        Phase::Running if !own_front => Next::End(End::FrontLost),
        Phase::Running if elapsed >= RUN_END_MS => Next::End(End::TimedOut),
        Phase::Running => Next::Wait,
    }
}

/// Whether the front goes back to the user's app when the hold ends: not
/// when another app took it, which is where the user went. After a press
/// it does, so the keys typed before it reach their window first.
pub fn gives_back(end: End) -> bool {
    end != End::FrontLost
}

/// How the key hold ends once the front is back, None when it ended by
/// itself. A press goes out after the keys; with the front lost, the keys
/// are not the new app's, and the key hold drops them.
pub fn keys_end(end: End) -> Option<key_hold::End> {
    match end {
        End::KeysEnded => None,
        End::UserActed(Keys::Pressed) => Some(key_hold::End::Pressed),
        End::FrontLost => Some(key_hold::End::Abandoned),
        _ => Some(key_hold::End::HandedBack),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: i32 = 10;
    const DIA: i32 = 20;

    #[test]
    fn agent_screen_takes_the_front_only_from_the_app_an_action_waits_on() {
        assert_eq!(start(Some(DIA), OWN, DIA), Start::Take);
        assert_eq!(start(Some(OWN), OWN, DIA), Start::InFront);
        assert_eq!(start(Some(30), OWN, DIA), Start::NotNeeded);
        assert_eq!(start(None, OWN, DIA), Start::NotNeeded);
    }

    #[test]
    fn the_actions_run_once_agent_screen_is_in_front() {
        assert_eq!(next(Phase::Taking, 0, false, 2, Keys::Quiet), Next::Wait);
        assert_eq!(next(Phase::Taking, 30, true, 2, Keys::Quiet), Next::Run);
        assert_eq!(
            next(Phase::Taking, TAKE_MS, false, 2, Keys::Quiet),
            Next::End(End::NoFront)
        );
    }

    #[test]
    fn the_hold_ends_when_every_action_ran() {
        assert_eq!(next(Phase::Running, 50, true, 1, Keys::Quiet), Next::Wait);
        assert_eq!(
            next(Phase::Running, 90, true, 0, Keys::Quiet),
            Next::End(End::Ran)
        );
    }

    #[test]
    fn the_users_input_ends_the_hold_first() {
        for phase in [Phase::Taking, Phase::Running] {
            for keys in [Keys::Typed, Keys::Pressed] {
                assert_eq!(
                    next(phase, 10, true, 0, keys),
                    Next::End(End::UserActed(keys))
                );
            }
        }
    }

    #[test]
    fn a_lost_front_or_the_time_limit_ends_it() {
        assert_eq!(
            next(Phase::Running, 10, false, 1, Keys::Quiet),
            Next::End(End::FrontLost)
        );
        assert_eq!(
            next(Phase::Running, RUN_END_MS, true, 1, Keys::Quiet),
            Next::End(End::TimedOut)
        );
    }

    #[test]
    fn the_whole_hold_fits_inside_the_key_holds_limit() {
        // The worst case: the take uses all its time, the run ends at its
        // limit, the settle and the hand-back use theirs, and the key hold
        // still has room to make the user's window key and send.
        let worst = RUN_END_MS + SETTLE_MS + GIVE_BACK_MS;
        assert!(worst + KEYS_BACK_MS <= key_hold::MAX_HOLD_MS);
        assert_eq!(BUDGET_MS, worst + KEYS_BACK_MS);
        // The run's limit counts from the hold's start, so a slow take
        // doesn't add to it.
        assert_eq!(
            next(Phase::Running, RUN_END_MS, true, 1, Keys::Quiet),
            Next::End(End::TimedOut)
        );
    }

    #[test]
    fn a_key_hold_timing_out_is_not_a_press_and_the_front_still_goes_back() {
        for phase in [Phase::Taking, Phase::Running] {
            assert_eq!(
                next(phase, 10, true, 1, Keys::Ended),
                Next::End(End::KeysEnded)
            );
        }
        assert!(gives_back(End::KeysEnded));
        assert_eq!(keys_end(End::KeysEnded), None);
    }

    #[test]
    fn after_a_press_the_front_goes_back_and_the_keys_go_before_the_press() {
        let pressed = End::UserActed(Keys::Pressed);
        assert!(gives_back(pressed));
        assert_eq!(keys_end(pressed), Some(key_hold::End::Pressed));
        assert_eq!(
            keys_end(End::UserActed(Keys::Typed)),
            Some(key_hold::End::HandedBack)
        );
        assert_eq!(keys_end(End::Ran), Some(key_hold::End::HandedBack));
    }

    #[test]
    fn the_front_goes_back_unless_the_user_went_elsewhere() {
        for end in [End::Ran, End::TimedOut, End::NoFront, End::KeysEnded] {
            assert!(gives_back(end));
        }
        assert!(!gives_back(End::FrontLost));
        assert_eq!(keys_end(End::FrontLost), Some(key_hold::End::Abandoned));
    }

    #[test]
    fn a_hold_is_for_one_displays_actions() {
        // A press on display 1 takes the front from Dia for display 1's
        // actions only, never another display's, nor a preview's.
        assert_eq!(waits_on(1, [(2, DIA), (1, DIA)]), Some(DIA));
        assert_eq!(waits_on(1, [(2, DIA)]), None);
        assert_eq!(waits_on(1, [(1, 0)]), None);
        let hold = Hold::new(1, DIA, 0);
        assert!(may_start(Some(&hold), 1, DIA));
        assert!(!may_start(Some(&hold), 2, DIA));
        // An action waiting on another app isn't the hold's business.
        assert!(may_start(Some(&hold), 2, 30));
        let ending = Hold {
            ending: true,
            ..hold
        };
        assert!(!may_start(Some(&ending), 1, DIA));
        assert!(may_start(None, 2, DIA));
    }

    #[test]
    fn a_left_over_hold_doesnt_block_the_next_press() {
        let hold = Hold::new(1, DIA, 100);
        assert!(can_start(None, 0));
        assert!(!can_start(Some(&hold), 100 + STALE_MS - 1));
        assert!(can_start(Some(&hold), 100 + STALE_MS));
    }
}
