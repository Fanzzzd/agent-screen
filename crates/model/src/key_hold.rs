//! The user's input held while a new window of the agent's has their app's
//! key focus (#110). `launch_app` asks a running app the user types in,
//! such as a browser, for a new window; the window takes the app's key
//! focus until Agent Screen hands it back to the user's window, about a
//! second later, and the user's keys typed meanwhile reached the agent's
//! window. So key and mouse button events bound for that app are held from
//! just before the window is asked for until focus is back, then sent on in
//! order. Events for any other app pass.
//!
//! Held input goes out only once the user's window is confirmed key, as
//! it may hold a password: where that window can't be made key again, it
//! goes out only if the app's key window is off the Agent Displays, and is
//! otherwise discarded. Losing what the user typed beats sending it to an
//! agent.

/// The longest input is held, in ms, whatever happens to the hand-back.
pub const MAX_HOLD_MS: u64 = 3000;

/// The most events held at once. Past it an event is discarded, not sent
/// to the agent's window; no one types that much in `MAX_HOLD_MS`.
pub const MAX_EVENTS: usize = 512;

/// What happens to one event the hold sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// On its way, as without a hold.
    Pass,
    /// Taken into the hold.
    Hold,
    /// The hold is full: dropped, and counted.
    Discard,
}

/// Why a hold ended, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Focus is back in the user's window.
    HandedBack,
    /// The user pressed a mouse button: the hold ends at once, and the
    /// press goes out after the keys before it.
    Pressed,
    /// `MAX_HOLD_MS` went by first.
    TimedOut,
    /// The new window never came: the press failed or the catch gave up.
    NoWindow,
    /// The launch ended or failed before the hand-back.
    Abandoned,
}

/// What the hold sees of one event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The front app when the event came, the one it goes to.
    pub front: i32,
    /// A key, modifier or mouse button event; anything else passes.
    pub input: bool,
    /// One Agent Screen sent itself, from a hold or an Input Loan.
    pub ours: bool,
}

/// Where held input goes when a hold ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// Sent on to the app's key window.
    Send,
    /// Dropped, and the count logged.
    Discard,
}

/// Where held input goes. `front_is_app`: the held app is still the front
/// app, which posted input reaches. `user_key`: its key window is the
/// user's, confirmed. Otherwise `key_window_on_agent_display` says where
/// its key window is, None when accessibility didn't say.
pub fn destination(
    front_is_app: bool,
    user_key: bool,
    key_window_on_agent_display: Option<bool>,
) -> Destination {
    match (front_is_app, user_key, key_window_on_agent_display) {
        (false, _, _) => Destination::Discard,
        (true, true, _) => Destination::Send,
        (true, false, Some(false)) => Destination::Send,
        (true, false, _) => Destination::Discard,
    }
}

/// Input held for app `app`. `E` is the event, kept as it came.
#[derive(Debug)]
pub struct KeyHold<E> {
    app: i32,
    held: Vec<E>,
    discarded: usize,
    open: bool,
    ended: Option<End>,
}

impl<E> KeyHold<E> {
    pub fn new(app: i32) -> Self {
        KeyHold {
            app,
            held: Vec::new(),
            discarded: 0,
            open: true,
            ended: None,
        }
    }

    pub fn app(&self) -> i32 {
        self.app
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn ended(&self) -> Option<End> {
        self.ended
    }

    /// Events dropped because the hold was full.
    pub fn discarded(&self) -> usize {
        self.discarded
    }

    /// What to do with `event`. On `Hold` it is kept; on `Pass` it comes
    /// back to go on its way.
    pub fn see(&mut self, seen: Seen, event: E) -> (Verdict, Option<E>) {
        // Input for another app goes there, as the user means it to.
        if !self.open || seen.ours || !seen.input || seen.front != self.app {
            return (Verdict::Pass, Some(event));
        }
        if self.held.len() >= MAX_EVENTS {
            self.discarded += 1;
            return (Verdict::Discard, None);
        }
        self.held.push(event);
        (Verdict::Hold, None)
    }

    /// End the hold, if it is still open, and take what it held, in the
    /// order it came, for `destination` to decide.
    pub fn release(&mut self, why: End) -> Vec<E> {
        if self.open {
            self.open = false;
            self.ended = Some(why);
        }
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(front: i32) -> Seen {
        Seen {
            front,
            input: true,
            ours: false,
        }
    }

    #[test]
    fn input_for_the_app_is_held_in_order_and_released_whole() {
        let mut hold = KeyHold::new(7);
        assert_eq!(hold.see(input(7), "a down"), (Verdict::Hold, None));
        assert_eq!(hold.see(input(7), "click"), (Verdict::Hold, None));
        assert_eq!(hold.see(input(7), "a up"), (Verdict::Hold, None));
        assert_eq!(
            hold.release(End::HandedBack),
            vec!["a down", "click", "a up"]
        );
        assert_eq!(hold.ended(), Some(End::HandedBack));
        // After the hold, input passes, as a key's release does.
        assert_eq!(hold.see(input(7), "b up"), (Verdict::Pass, Some("b up")));
    }

    #[test]
    fn other_apps_input_moves_and_our_own_pass() {
        let mut hold = KeyHold::new(7);
        assert_eq!(hold.see(input(8), "other"), (Verdict::Pass, Some("other")));
        let moved = Seen {
            input: false,
            ..input(7)
        };
        assert_eq!(hold.see(moved, "move"), (Verdict::Pass, Some("move")));
        let ours = Seen {
            ours: true,
            ..input(7)
        };
        assert_eq!(
            hold.see(ours, "replayed"),
            (Verdict::Pass, Some("replayed"))
        );
        assert!(hold.is_open());
    }

    #[test]
    fn a_full_hold_discards_and_never_passes_to_the_agent() {
        let mut hold = KeyHold::new(7);
        for i in 0..MAX_EVENTS {
            assert_eq!(hold.see(input(7), i), (Verdict::Hold, None));
        }
        assert_eq!(hold.see(input(7), MAX_EVENTS), (Verdict::Discard, None));
        assert_eq!(hold.discarded(), 1);
        assert_eq!(hold.release(End::TimedOut).len(), MAX_EVENTS);
    }

    #[test]
    fn held_input_goes_only_to_the_users_key_window() {
        use Destination::*;
        // Confirmed: the user's window is key in the front app.
        assert_eq!(destination(true, true, Some(true)), Send);
        // Not confirmed, but the key window is off the Agent Displays.
        assert_eq!(destination(true, false, Some(false)), Send);
        // The agent's window, or unknown: never sent there.
        assert_eq!(destination(true, false, Some(true)), Discard);
        assert_eq!(destination(true, false, None), Discard);
        // Another app in front would get it.
        assert_eq!(destination(false, true, Some(false)), Discard);
    }

    #[test]
    fn a_failed_press_ends_the_hold_with_the_users_input_intact() {
        // The New Window press failed before any window came: focus is
        // still the user's, so what was held goes to their window.
        let mut hold = KeyHold::new(7);
        hold.see(input(7), "a down");
        hold.see(input(7), "a up");
        assert_eq!(hold.release(End::NoWindow), vec!["a down", "a up"]);
        assert_eq!(hold.ended(), Some(End::NoWindow));
        assert_eq!(destination(true, true, Some(false)), Destination::Send);
    }
}
