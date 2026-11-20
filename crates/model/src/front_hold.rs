//! The front hold (ADR 0013, "Let the agent continue"): actions waiting in
//! the Input Queue for the user to leave their front app run at once when
//! the user presses the button for them. Agent Screen takes the front from
//! that app for the moment the actions need, so they go in through the
//! background route, then gives the front back with the user's window key.
//! The user's keys typed meanwhile are held and go to their window after.

/// How long Agent Screen waits to become the front process.
pub const TAKE_MS: u64 = 200;

/// The longest the actions may run while Agent Screen holds the front.
/// Well under `key_hold::MAX_HOLD_MS`, so the user's held keys still go to
/// their window.
pub const RUN_MS: u64 = 2000;

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

/// Where the hold is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Agent Screen asked to be the front process.
    Taking,
    /// It is: the waiting actions run.
    Running,
}

/// The user's input during the hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserInput {
    None,
    /// Keys, held for their window.
    Typed,
    /// A mouse button: the press goes where the user meant it.
    Pressed,
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
    UserActed(UserInput),
    /// `RUN_MS` went by first: what is left waits again.
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

/// The step after one look at the hold, `elapsed` ms into its phase.
/// `own_front`: Agent Screen is WindowServer's front process. `pending`:
/// actions still waiting or running. The user's input ends it first.
pub fn next(phase: Phase, elapsed: u64, own_front: bool, pending: usize, input: UserInput) -> Next {
    if input != UserInput::None {
        return Next::End(End::UserActed(input));
    }
    match phase {
        Phase::Taking if own_front => Next::Run,
        Phase::Taking if elapsed >= TAKE_MS => Next::End(End::NoFront),
        Phase::Taking => Next::Wait,
        Phase::Running if pending == 0 => Next::End(End::Ran),
        Phase::Running if !own_front => Next::End(End::FrontLost),
        Phase::Running if elapsed >= RUN_MS => Next::End(End::TimedOut),
        Phase::Running => Next::Wait,
    }
}

/// Whether the front goes back to the user's app when the hold ends. Not
/// after a press, nor when another app took the front: that is where the
/// user went.
pub fn gives_back(end: End) -> bool {
    !matches!(end, End::FrontLost | End::UserActed(UserInput::Pressed))
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
        assert_eq!(
            next(Phase::Taking, 0, false, 2, UserInput::None),
            Next::Wait
        );
        assert_eq!(next(Phase::Taking, 30, true, 2, UserInput::None), Next::Run);
        assert_eq!(
            next(Phase::Taking, TAKE_MS, false, 2, UserInput::None),
            Next::End(End::NoFront)
        );
    }

    #[test]
    fn the_hold_ends_when_every_action_ran() {
        assert_eq!(
            next(Phase::Running, 50, true, 1, UserInput::None),
            Next::Wait
        );
        assert_eq!(
            next(Phase::Running, 90, true, 0, UserInput::None),
            Next::End(End::Ran)
        );
    }

    #[test]
    fn the_users_input_ends_the_hold_first() {
        for phase in [Phase::Taking, Phase::Running] {
            for input in [UserInput::Typed, UserInput::Pressed] {
                assert_eq!(
                    next(phase, 10, true, 0, input),
                    Next::End(End::UserActed(input))
                );
            }
        }
    }

    #[test]
    fn a_lost_front_or_the_time_limit_ends_it() {
        assert_eq!(
            next(Phase::Running, 10, false, 1, UserInput::None),
            Next::End(End::FrontLost)
        );
        assert_eq!(
            next(Phase::Running, RUN_MS, true, 1, UserInput::None),
            Next::End(End::TimedOut)
        );
        const { assert!(RUN_MS < crate::key_hold::MAX_HOLD_MS) };
    }

    #[test]
    fn the_front_goes_back_unless_the_user_went_elsewhere() {
        assert!(gives_back(End::Ran));
        assert!(gives_back(End::TimedOut));
        assert!(gives_back(End::NoFront));
        assert!(gives_back(End::UserActed(UserInput::Typed)));
        assert!(!gives_back(End::UserActed(UserInput::Pressed)));
        assert!(!gives_back(End::FrontLost));
    }
}
