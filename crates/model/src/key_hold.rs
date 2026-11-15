//! The user's keys held while a new window of the agent's has their app's
//! key focus (#110). `launch_app` asks a running app the user types in,
//! such as a browser, for a new window; the window takes the app's key
//! focus until Agent Screen hands it back to the user's window, about a
//! second later, and the user's keys typed meanwhile reached the agent's
//! window. So keys meant for that app are held from just before the window
//! is asked for until focus is back, then sent on in order, to the user's
//! window. Keys for any other app pass. A held key is never dropped: the
//! hold ends at the latest after `MAX_HOLD_MS`, or when it holds `MAX_KEYS`,
//! and everything held is sent on then.

/// The longest keys are held, in ms, whatever happens to the hand-back.
pub const MAX_HOLD_MS: u64 = 3000;

/// The most keys held at once. A hold that would take more ends there.
pub const MAX_KEYS: usize = 512;

/// What happens to one key event the hold sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// On its way, as without a hold.
    Pass,
    /// Taken into the hold.
    Hold,
    /// The hold ends here: this event joins what is held, and all of it
    /// goes out now, in order, so nothing passes ahead of a held key.
    Flush,
}

/// Why a hold ended, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Focus is back in the user's window.
    HandedBack,
    /// `MAX_HOLD_MS` went by first.
    TimedOut,
    /// `MAX_KEYS` were held.
    Full,
    /// Another app came to the front, where the keys go now.
    FrontChanged,
    /// The launch ended or failed before the hand-back.
    Abandoned,
}

/// One key event as the hold sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Milliseconds since the hold began.
    pub at_ms: u64,
    /// The front app when the event came, the one it goes to.
    pub front: i32,
    /// A key down, key up or modifier change; anything else passes.
    pub key: bool,
    /// One Agent Screen sent on itself, from this hold or an Input Loan.
    pub ours: bool,
}

/// Keys held for app `app`. `E` is the event, kept as it came.
#[derive(Debug)]
pub struct KeyHold<E> {
    app: i32,
    held: Vec<E>,
    open: bool,
    ended: Option<End>,
}

impl<E> KeyHold<E> {
    pub fn new(app: i32) -> Self {
        KeyHold {
            app,
            held: Vec::new(),
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

    /// What to do with `event`. On `Hold` it is kept; on `Flush` it is
    /// kept last and the hold closes: take everything with `release`.
    pub fn see(&mut self, seen: Seen, event: E) -> (Verdict, Option<E>) {
        if !self.open || seen.ours || !seen.key {
            return (Verdict::Pass, Some(event));
        }
        let end = if seen.at_ms >= MAX_HOLD_MS {
            Some(End::TimedOut)
        } else if seen.front != self.app {
            Some(End::FrontChanged)
        } else if self.held.len() + 1 >= MAX_KEYS {
            Some(End::Full)
        } else {
            None
        };
        match end {
            // Nothing held to keep ahead of it: it goes as it is.
            Some(end) if self.held.is_empty() => {
                self.close(end);
                (Verdict::Pass, Some(event))
            }
            Some(end) => {
                self.held.push(event);
                self.close(end);
                (Verdict::Flush, None)
            }
            None => {
                self.held.push(event);
                (Verdict::Hold, None)
            }
        }
    }

    /// End the hold, if it is still open, and take what it held, in the
    /// order it came, to send on.
    pub fn release(&mut self, why: End) -> Vec<E> {
        self.close(why);
        std::mem::take(&mut self.held)
    }

    fn close(&mut self, why: End) {
        if self.open {
            self.open = false;
            self.ended = Some(why);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(at_ms: u64, front: i32) -> Seen {
        Seen {
            at_ms,
            front,
            key: true,
            ours: false,
        }
    }

    #[test]
    fn keys_for_the_app_are_held_in_order_and_released_whole() {
        let mut hold = KeyHold::new(7);
        assert_eq!(hold.see(key(0, 7), "a down"), (Verdict::Hold, None));
        assert_eq!(hold.see(key(30, 7), "a up"), (Verdict::Hold, None));
        assert_eq!(hold.release(End::HandedBack), vec!["a down", "a up"]);
        assert_eq!(hold.ended(), Some(End::HandedBack));
        // After the hold, keys pass.
        assert_eq!(hold.see(key(40, 7), "b"), (Verdict::Pass, Some("b")));
    }

    #[test]
    fn other_apps_keys_mouse_events_and_our_own_pass() {
        let mut hold = KeyHold::new(7);
        assert_eq!(hold.see(key(0, 8), "other"), (Verdict::Pass, Some("other")));
        // An empty hold for an app no longer in front just ends.
        assert!(!hold.is_open());
        let mut hold = KeyHold::new(7);
        let mouse = Seen {
            key: false,
            ..key(0, 7)
        };
        assert_eq!(hold.see(mouse, "click"), (Verdict::Pass, Some("click")));
        let ours = Seen {
            ours: true,
            ..key(0, 7)
        };
        assert_eq!(
            hold.see(ours, "replayed"),
            (Verdict::Pass, Some("replayed"))
        );
        assert!(hold.is_open());
    }

    #[test]
    fn a_hold_ending_early_flushes_with_the_event_that_ended_it() {
        let mut hold = KeyHold::new(7);
        hold.see(key(0, 7), "a down");
        assert_eq!(hold.see(key(10, 8), "b down"), (Verdict::Flush, None));
        assert_eq!(hold.ended(), Some(End::FrontChanged));
        assert_eq!(hold.release(End::Abandoned), vec!["a down", "b down"]);
        // The first reason stays.
        assert_eq!(hold.ended(), Some(End::FrontChanged));

        let mut hold = KeyHold::new(7);
        hold.see(key(0, 7), "a down");
        assert_eq!(
            hold.see(key(MAX_HOLD_MS, 7), "a up"),
            (Verdict::Flush, None)
        );
        assert_eq!(hold.ended(), Some(End::TimedOut));
        assert_eq!(hold.release(End::Abandoned), vec!["a down", "a up"]);
    }

    #[test]
    fn a_full_hold_flushes_and_drops_nothing() {
        let mut hold = KeyHold::new(7);
        for i in 0..MAX_KEYS - 1 {
            assert_eq!(hold.see(key(1, 7), i), (Verdict::Hold, None));
        }
        assert_eq!(hold.see(key(1, 7), MAX_KEYS - 1), (Verdict::Flush, None));
        assert_eq!(hold.ended(), Some(End::Full));
        assert_eq!(
            hold.release(End::Abandoned),
            (0..MAX_KEYS).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_key_up_after_the_hold_still_passes() {
        // A key pressed during the hold goes out down then; its release,
        // after the hold, passes as it comes: a matching pair.
        let mut hold = KeyHold::new(7);
        hold.see(key(0, 7), "a down");
        assert_eq!(hold.release(End::HandedBack), vec!["a down"]);
        assert_eq!(hold.see(key(50, 7), "a up"), (Verdict::Pass, Some("a up")));
    }
}
