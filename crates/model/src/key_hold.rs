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

/// Held events past which the hold ends, as no one types that much in
/// `MAX_HOLD_MS`. Events keep being held until it has ended, and none is
/// dropped for the count.
pub const MAX_EVENTS: usize = 512;

/// What happens to one event the hold sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// On its way, as without a hold.
    Pass,
    /// Taken into the hold.
    Hold,
    /// Taken into the hold, which is full: end it now.
    HoldAndEnd,
}

/// Why a hold ended, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Focus is back in the user's window.
    HandedBack,
    /// The user pressed a mouse button: the hold ends at once, and the
    /// press goes out after the keys before it.
    Pressed,
    /// `MAX_EVENTS` were held.
    Full,
    /// `MAX_HOLD_MS` went by first.
    TimedOut,
    /// The new window never came: the press failed or the catch gave up.
    NoWindow,
    /// The launch ended or failed before the hand-back.
    Abandoned,
}

/// What kind of input an event is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    KeyDown,
    KeyUp,
    /// A modifier key's change.
    Modifiers,
    MouseDown,
    MouseUp,
    MouseDragged,
    /// Anything else, which passes.
    Other,
}

impl Kind {
    fn mouse(self) -> bool {
        matches!(self, Kind::MouseDown | Kind::MouseUp | Kind::MouseDragged)
    }
}

/// What the hold sees of one event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The front app when the event came, the one it goes to.
    pub front: i32,
    pub kind: Kind,
    /// The key code or mouse button, to pair a release with its press.
    pub code: i64,
    /// One Agent Screen sent itself, from a hold or an Input Loan.
    pub ours: bool,
}

/// Where held input goes when a hold ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// Sent on to the app's key window.
    Send,
    /// Key downs dropped and counted; the rest sent on (`sent_on_discard`).
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

/// Whether a held event of `kind` still goes out when the hold's input is
/// discarded. Only a key down types anything. A mouse event goes to the
/// window under the pointer, on the user's screen, never to an Agent
/// Display's; a key up or modifier change types nothing, and dropping one
/// whose press went out would leave the key held in the app.
pub fn sent_on_discard(kind: Kind) -> bool {
    kind != Kind::KeyDown
}

/// Input held for app `app`. `E` is the event, kept as it came.
#[derive(Debug)]
pub struct KeyHold<E> {
    app: i32,
    /// Another app whose input is held too: Agent Screen during a front
    /// hold, while it stands in front of `app`.
    also: Option<i32>,
    held: Vec<(Kind, E)>,
    /// Presses held, by kind and code, whose releases are held with them.
    pressed: Vec<(bool, i64)>,
    open: bool,
    ended: Option<End>,
}

impl<E> KeyHold<E> {
    pub fn new(app: i32) -> Self {
        KeyHold {
            app,
            also: None,
            held: Vec::new(),
            pressed: Vec::new(),
            open: true,
            ended: None,
        }
    }

    /// Hold the input bound for `other` too.
    pub fn also(mut self, other: i32) -> Self {
        self.also = Some(other);
        self
    }

    pub fn app(&self) -> i32 {
        self.app
    }

    /// Whether it holds any key down.
    pub fn typed(&self) -> bool {
        self.held.iter().any(|(kind, _)| *kind == Kind::KeyDown)
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn ended(&self) -> Option<End> {
        self.ended
    }

    /// What to do with `event`. On `Hold` and `HoldAndEnd` it is kept; on
    /// `Pass` it comes back to go on its way.
    pub fn see(&mut self, seen: Seen, event: E) -> (Verdict, Option<E>) {
        // Input for another app goes there, as the user means it to.
        let bound = seen.front == self.app || Some(seen.front) == self.also;
        if !self.open || seen.ours || seen.kind == Kind::Other || !bound {
            return (Verdict::Pass, Some(event));
        }
        let press = (seen.kind.mouse(), seen.code);
        match seen.kind {
            Kind::KeyDown | Kind::MouseDown => {
                if !self.pressed.contains(&press) {
                    self.pressed.push(press);
                }
            }
            // A release or drag whose press went out before the hold goes
            // out too, so no key or button is left held.
            Kind::KeyUp | Kind::MouseUp | Kind::MouseDragged if !self.pressed.contains(&press) => {
                return (Verdict::Pass, Some(event));
            }
            Kind::KeyUp | Kind::MouseUp => self.pressed.retain(|p| *p != press),
            _ => {}
        }
        self.held.push((seen.kind, event));
        match self.held.len() >= MAX_EVENTS {
            true => (Verdict::HoldAndEnd, None),
            false => (Verdict::Hold, None),
        }
    }

    /// End the hold, if it is still open, and take what it held, in the
    /// order it came, with each event's kind, for `destination` to decide.
    pub fn release(&mut self, why: End) -> Vec<(Kind, E)> {
        if self.open {
            self.open = false;
            self.ended = Some(why);
        }
        self.pressed.clear();
        std::mem::take(&mut self.held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(kind: Kind, code: i64) -> Seen {
        Seen {
            front: 7,
            kind,
            code,
            ours: false,
        }
    }

    fn kinds<E>(held: Vec<(Kind, E)>) -> Vec<E> {
        held.into_iter().map(|(_, e)| e).collect()
    }

    #[test]
    fn input_for_the_app_is_held_in_order_and_released_whole() {
        let mut hold = KeyHold::new(7);
        assert_eq!(
            hold.see(seen(Kind::KeyDown, 0), "a down"),
            (Verdict::Hold, None)
        );
        assert_eq!(
            hold.see(seen(Kind::MouseDown, 0), "press"),
            (Verdict::Hold, None)
        );
        assert_eq!(
            hold.see(seen(Kind::MouseUp, 0), "release"),
            (Verdict::Hold, None)
        );
        assert_eq!(
            hold.see(seen(Kind::KeyUp, 0), "a up"),
            (Verdict::Hold, None)
        );
        assert_eq!(
            kinds(hold.release(End::HandedBack)),
            vec!["a down", "press", "release", "a up"]
        );
        assert_eq!(hold.ended(), Some(End::HandedBack));
        assert_eq!(
            hold.see(seen(Kind::KeyUp, 1), "b up"),
            (Verdict::Pass, Some("b up"))
        );
    }

    #[test]
    fn other_apps_input_moves_and_our_own_pass() {
        let mut hold = KeyHold::new(7);
        let other = Seen {
            front: 8,
            ..seen(Kind::KeyDown, 0)
        };
        assert_eq!(hold.see(other, "other"), (Verdict::Pass, Some("other")));
        assert_eq!(
            hold.see(seen(Kind::Other, 0), "move"),
            (Verdict::Pass, Some("move"))
        );
        let ours = Seen {
            ours: true,
            ..seen(Kind::KeyDown, 0)
        };
        assert_eq!(
            hold.see(ours, "replayed"),
            (Verdict::Pass, Some("replayed"))
        );
        assert!(hold.is_open());
    }

    #[test]
    fn a_front_hold_holds_keys_for_agent_screen_in_front_of_the_app() {
        let mut hold = KeyHold::new(7).also(8);
        let to_agent_screen = Seen {
            front: 8,
            ..seen(Kind::KeyDown, 1)
        };
        assert!(!hold.typed());
        assert_eq!(hold.see(to_agent_screen, "a"), (Verdict::Hold, None));
        assert_eq!(hold.see(seen(Kind::KeyDown, 2), "b"), (Verdict::Hold, None));
        let elsewhere = Seen {
            front: 9,
            ..seen(Kind::KeyDown, 3)
        };
        assert_eq!(hold.see(elsewhere, "c"), (Verdict::Pass, Some("c")));
        assert!(hold.typed());
        assert_eq!(kinds(hold.release(End::HandedBack)), vec!["a", "b"]);
    }

    #[test]
    fn a_release_or_drag_whose_press_went_out_before_passes() {
        let mut hold = KeyHold::new(7);
        assert_eq!(
            hold.see(seen(Kind::KeyUp, 4), "up"),
            (Verdict::Pass, Some("up"))
        );
        assert_eq!(
            hold.see(seen(Kind::MouseDragged, 0), "drag"),
            (Verdict::Pass, Some("drag"))
        );
        assert_eq!(
            hold.see(seen(Kind::MouseUp, 0), "mup"),
            (Verdict::Pass, Some("mup"))
        );
        // A key's code and a button's number don't pair with each other.
        hold.see(seen(Kind::KeyDown, 0), "a down");
        assert_eq!(
            hold.see(seen(Kind::MouseUp, 0), "m"),
            (Verdict::Pass, Some("m"))
        );
        assert_eq!(
            hold.see(seen(Kind::KeyUp, 0), "a up"),
            (Verdict::Hold, None)
        );
    }

    #[test]
    fn a_full_hold_ends_and_drops_nothing() {
        let mut hold = KeyHold::new(7);
        for i in 0..MAX_EVENTS - 1 {
            assert_eq!(
                hold.see(seen(Kind::Modifiers, 56), i),
                (Verdict::Hold, None)
            );
        }
        assert_eq!(
            hold.see(seen(Kind::Modifiers, 56), MAX_EVENTS - 1),
            (Verdict::HoldAndEnd, None)
        );
        // Until it has ended, input is still held.
        assert_eq!(
            hold.see(seen(Kind::Modifiers, 56), MAX_EVENTS),
            (Verdict::HoldAndEnd, None)
        );
        assert_eq!(hold.release(End::Full).len(), MAX_EVENTS + 1);
    }

    #[test]
    fn held_input_goes_only_to_the_users_key_window() {
        use Destination::*;
        assert_eq!(destination(true, true, Some(true)), Send);
        assert_eq!(destination(true, false, Some(false)), Send);
        assert_eq!(destination(true, false, Some(true)), Discard);
        assert_eq!(destination(true, false, None), Discard);
        assert_eq!(destination(false, true, Some(false)), Discard);
    }

    #[test]
    fn a_discard_drops_only_key_downs() {
        // Mouse events go to the window under the pointer, on the user's
        // screen; key ups and modifier changes type nothing and keep the
        // app's key state paired.
        assert!(!sent_on_discard(Kind::KeyDown));
        for kind in [
            Kind::KeyUp,
            Kind::Modifiers,
            Kind::MouseDown,
            Kind::MouseUp,
            Kind::MouseDragged,
        ] {
            assert!(sent_on_discard(kind), "{kind:?}");
        }
    }

    #[test]
    fn a_failed_press_ends_the_hold_with_the_users_input_intact() {
        let mut hold = KeyHold::new(7);
        hold.see(seen(Kind::KeyDown, 0), "a down");
        hold.see(seen(Kind::KeyUp, 0), "a up");
        assert_eq!(kinds(hold.release(End::NoWindow)), vec!["a down", "a up"]);
        assert_eq!(hold.ended(), Some(End::NoWindow));
        assert_eq!(destination(true, true, Some(false)), Destination::Send);
    }
}
