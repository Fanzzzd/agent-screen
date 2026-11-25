//! Where a window an Agent Display holds is now, for the tools that name
//! it: wait_for_change, get_window_state and the other tools that take a
//! window id, list_windows, and Take Back. A window off screen isn't
//! closed: its app hid or minimized it, or it is on a macOS Space the
//! display isn't showing, as after the user switches desktops while
//! "Displays have separate Spaces" is off. Each tool says which, so the
//! agent can tell the user what to do; none of them switches a Space,
//! shows an app or raises a window (ADR 0001).

use crate::placement::PutAway;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// On the display, on screen.
    Here,
    /// On screen, off the display.
    Elsewhere,
    /// Open, on a macOS Space no display shows now.
    OtherSpace,
    /// Minimized to the Dock.
    Minimized,
    /// Its app is hidden.
    Hidden,
    /// Gone, or kept out of sight by its app on no Space, or off screen
    /// for a reason macOS doesn't give.
    Closed,
}

impl Presence {
    /// Where a window is, from the on-screen window list: on the display
    /// (`here`) or on screen anywhere (`on_screen`). Off screen, `exists`
    /// says whether the window server still lists it, and `away` why it is
    /// off screen; each is asked only when the answer before it leaves the
    /// question open, since `away` costs a SkyLight call and an
    /// Accessibility read.
    pub fn of(
        here: bool,
        on_screen: bool,
        exists: impl FnOnce() -> bool,
        away: impl FnOnce() -> PutAway,
    ) -> Presence {
        if here {
            return Presence::Here;
        }
        if on_screen {
            return Presence::Elsewhere;
        }
        if !exists() {
            return Presence::Closed;
        }
        match away() {
            PutAway::Minimized => Presence::Minimized,
            PutAway::Hidden => Presence::Hidden,
            PutAway::OtherSpace => Presence::OtherSpace,
            PutAway::Closed | PutAway::Unknown => Presence::Closed,
        }
    }

    /// list_windows' `offScreen` for a window the display holds that shows
    /// nowhere but is still open. None on screen or closed.
    pub fn off_screen(self) -> Option<&'static str> {
        match self {
            Presence::OtherSpace => Some("otherSpace"),
            Presence::Minimized => Some("minimized"),
            Presence::Hidden => Some("hidden"),
            Presence::Here | Presence::Elsewhere | Presence::Closed => None,
        }
    }

    /// wait_for_change's outcome when the window stops being on the
    /// display. None while it is.
    pub fn outcome(self) -> Option<&'static str> {
        match self {
            Presence::Here => None,
            Presence::Elsewhere => Some("windowLeft"),
            Presence::OtherSpace => Some("windowOnOtherSpace"),
            Presence::Minimized => Some("windowMinimized"),
            Presence::Hidden => Some("windowHidden"),
            Presence::Closed => Some("windowClosed"),
        }
    }

    /// The presence a wait_for_change outcome names, for its text.
    pub fn of_outcome(outcome: &str) -> Option<Presence> {
        [
            Presence::Elsewhere,
            Presence::OtherSpace,
            Presence::Minimized,
            Presence::Hidden,
            Presence::Closed,
        ]
        .into_iter()
        .find(|p| p.outcome() == Some(outcome))
    }

    /// What is true of window `window` held by display `display` that shows
    /// nowhere, and what the agent can do about it: one sentence or two,
    /// for an error and for wait_for_change. None on screen or closed.
    pub fn advice(self, window: i64, display: u32) -> Option<String> {
        match self {
            Presence::OtherSpace => Some(format!(
                "window {window} is still open, on a macOS Space (desktop) that display {display} isn't showing now, so Agent Screen can't see or reach it, and it doesn't switch Spaces. Ask the user to switch back to the desktop the window is on, or to move the window to the desktop shown now, then call list_windows"
            )),
            Presence::Minimized => Some(format!(
                "window {window} is minimized to the Dock. Call place_window with window_id {window}: it brings a minimized window back onto display {display} when it can, and says what to ask the user otherwise"
            )),
            Presence::Hidden => Some(format!(
                "window {window}'s app is hidden, so its windows show nowhere. Agent Screen doesn't show it, since that would bring back the app's windows on the user's screen too. Ask the user to show the app, then call list_windows"
            )),
            Presence::Here | Presence::Elsewhere | Presence::Closed => None,
        }
    }
}

/// The windows list_windows found closed while a record still held them:
/// an app that closes a window but keeps it can leave it in the records
/// for good, and each look at it costs a SkyLight call and an
/// Accessibility read. Such a window is skipped until it shows on screen
/// again, when its app brought it back.
#[derive(Clone, Debug, Default)]
pub struct ClosedMarks(std::collections::HashSet<u32>);

impl ClosedMarks {
    /// Bounds what a long session marks.
    const MAX: usize = 4096;

    /// Whether to skip `window` without asking macOS: it was found closed
    /// and isn't on screen now. On screen, its mark goes.
    pub fn skip(&mut self, window: u32, on_screen: bool) -> bool {
        if on_screen {
            self.0.remove(&window);
            return false;
        }
        self.0.contains(&window)
    }

    /// What a look at `window` found: a closed one is marked, any other
    /// presence clears its mark.
    pub fn note(&mut self, window: u32, presence: Presence) {
        if presence != Presence::Closed {
            self.0.remove(&window);
            return;
        }
        if self.0.len() >= Self::MAX {
            self.0.clear();
        }
        self.0.insert(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_found_closed_is_skipped_until_it_shows_again() {
        let mut marks = ClosedMarks::default();
        assert!(!marks.skip(7, false));
        marks.note(7, Presence::Closed);
        assert!(marks.skip(7, false));
        assert!(marks.skip(7, false));
        assert!(!marks.skip(8, false));
        // Back on screen: asked about again, and not skipped once off it.
        assert!(!marks.skip(7, true));
        assert!(!marks.skip(7, false));
    }

    #[test]
    fn only_closed_marks() {
        let mut marks = ClosedMarks::default();
        for p in [
            Presence::Here,
            Presence::Elsewhere,
            Presence::OtherSpace,
            Presence::Minimized,
            Presence::Hidden,
        ] {
            marks.note(3, p);
            assert!(!marks.skip(3, false), "{p:?}");
        }
        marks.note(3, Presence::Closed);
        marks.note(3, Presence::Minimized);
        assert!(!marks.skip(3, false));
    }

    #[test]
    fn marks_stay_bounded() {
        let mut marks = ClosedMarks::default();
        for w in 0..(ClosedMarks::MAX as u32 + 10) {
            marks.note(w, Presence::Closed);
        }
        assert!(marks.0.len() <= ClosedMarks::MAX);
        assert!(marks.skip(ClosedMarks::MAX as u32 + 9, false));
    }

    #[test]
    fn on_screen_answers_without_asking_why() {
        for (here, on_screen, want) in [
            (true, true, Presence::Here),
            (true, false, Presence::Here),
            (false, true, Presence::Elsewhere),
        ] {
            let got = Presence::of(
                here,
                on_screen,
                || panic!("asked whether {here}/{on_screen} exists"),
                || panic!("asked why {here}/{on_screen} is away"),
            );
            assert_eq!(got, want);
        }
    }

    #[test]
    fn a_window_the_window_server_forgot_is_closed_without_asking_why() {
        let got = Presence::of(false, false, || false, || panic!("asked why"));
        assert_eq!(got, Presence::Closed);
    }

    #[test]
    fn an_open_window_off_screen_says_why() {
        for (away, want) in [
            (PutAway::Minimized, Presence::Minimized),
            (PutAway::Hidden, Presence::Hidden),
            (PutAway::OtherSpace, Presence::OtherSpace),
            (PutAway::Closed, Presence::Closed),
            (PutAway::Unknown, Presence::Closed),
        ] {
            assert_eq!(
                Presence::of(false, false, || true, || away),
                want,
                "{away:?}"
            );
        }
    }

    /// Every cell from the window server's and Accessibility's facts, as
    /// `PutAway::of` reads them: listed, minimized, app hidden, Spaces.
    #[test]
    fn every_cell_of_the_facts() {
        for listed in [false, true] {
            for minimized in [false, true] {
                for hidden in [false, true] {
                    for spaces in [-1, 0, 1, 2] {
                        let got = Presence::of(
                            false,
                            false,
                            || true,
                            || PutAway::of(listed, minimized, hidden, spaces),
                        );
                        let want = match (listed, spaces) {
                            (true, _) if hidden => Presence::Hidden,
                            (true, _) if minimized => Presence::Minimized,
                            (_, n) if n > 0 => Presence::OtherSpace,
                            _ => Presence::Closed,
                        };
                        assert_eq!(got, want, "{listed} {minimized} {hidden} {spaces}");
                    }
                }
            }
        }
    }

    #[test]
    fn outcomes_name_each_way_a_window_leaves() {
        assert_eq!(Presence::Here.outcome(), None);
        for (p, outcome) in [
            (Presence::Elsewhere, "windowLeft"),
            (Presence::OtherSpace, "windowOnOtherSpace"),
            (Presence::Minimized, "windowMinimized"),
            (Presence::Hidden, "windowHidden"),
            (Presence::Closed, "windowClosed"),
        ] {
            assert_eq!(p.outcome(), Some(outcome));
            assert_eq!(Presence::of_outcome(outcome), Some(p));
        }
        assert_eq!(Presence::of_outcome("fired"), None);
    }

    #[test]
    fn only_open_windows_that_show_nowhere_are_listed_off_screen() {
        assert_eq!(Presence::OtherSpace.off_screen(), Some("otherSpace"));
        assert_eq!(Presence::Minimized.off_screen(), Some("minimized"));
        assert_eq!(Presence::Hidden.off_screen(), Some("hidden"));
        for p in [Presence::Here, Presence::Elsewhere, Presence::Closed] {
            assert_eq!(p.off_screen(), None);
            assert_eq!(p.advice(4, 2), None);
        }
    }

    #[test]
    fn advice_says_what_to_do_and_never_promises_a_space_switch() {
        let space = Presence::OtherSpace.advice(4, 2).unwrap();
        assert!(space.contains("still open") && space.contains("doesn't switch Spaces"));
        assert!(space.contains("Ask the user"), "{space}");
        let minimized = Presence::Minimized.advice(4, 2).unwrap();
        assert!(
            minimized.contains("place_window with window_id 4"),
            "{minimized}"
        );
        let hidden = Presence::Hidden.advice(4, 2).unwrap();
        assert!(hidden.contains("Ask the user to show the app"), "{hidden}");
    }
}
