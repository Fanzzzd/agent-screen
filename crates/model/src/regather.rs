//! Putting an Agent Display's windows back after the displays change (#291).
//!
//! When a display comes or goes, macOS moves the others and may drop their
//! windows elsewhere: on the user's screen, on a spare or a unit twin Agent
//! Screen is making, or off every display. A regather looks at each window
//! recorded on a live Agent Display a few times while the displays settle
//! and puts back each one that is off.
//!
//! It used to look twice and believe the move's answer. A window whose move
//! failed once stayed off its display. One that landed on a spare or a
//! unit twin counted as on an Agent Display and stayed there: no session
//! shows a spare, and its recorded owner (ADR 0018) kept the window listed
//! on its own display all the same. A window that vanished left no log
//! line. Now each pass reads where the window really
//! is, a window that stays off after every try goes to the user's screen
//! with its record dropped, and a window gone from two passes in a row is
//! logged and forgotten.

use crate::model::{Rect, WindowId};
use std::collections::HashMap;

/// When each pass runs, in milliseconds after the one before; the first
/// counts from the display change. A newer change starts over.
pub const PASSES_MS: [u64; 5] = [500, 1500, 3000, 6000, 1000];

/// Where a recorded window is during a regather.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In no window list.
    Gone,
    /// Its center is on its own display or on another of the app's Agent
    /// Displays, live or in review: a session may move a window between
    /// its displays.
    Home,
    /// Anywhere else: the user's screen, a spare, a unit twin, or no
    /// display at all.
    Off,
}

/// Where the window with `frame` is, given its own display's frame and
/// the frames of the app's other Agent Displays, spares not among them.
pub fn place(frame: Option<Rect>, own: Rect, others: &[Rect]) -> Place {
    let Some(frame) = frame else {
        return Place::Gone;
    };
    let mid = frame.mid();
    if own.contains(mid) || others.iter().any(|d| d.contains(mid)) {
        Place::Home
    } else {
        Place::Off
    }
}

/// What a pass does with one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing: it is home, or it was missing from one pass only.
    Stay,
    /// It was off and is home again: say so.
    Back,
    /// Move it onto its own display.
    PutBack,
    /// It was in no window list two passes in a row: its app closed it.
    /// Say so and forget its record.
    Vanished,
    /// It is still off after the last try: return it to the user's screen
    /// and forget its record, so it isn't left on a spare or tiled into
    /// the user's workspace as the agent's.
    HandToUser,
}

/// One regather: what it saw of each window in earlier passes.
#[derive(Debug, Default)]
pub struct Regather {
    tries: HashMap<WindowId, u32>,
    missing: HashMap<WindowId, u32>,
}

impl Regather {
    pub fn new() -> Self {
        Self::default()
    }

    /// The step for `window`, seen at `place` in pass `pass` (from 0) of
    /// `PASSES_MS`. The last pass only checks the moves before it.
    pub fn step(&mut self, window: WindowId, place: Place, pass: usize) -> Step {
        let last = pass + 1 >= PASSES_MS.len();
        if place != Place::Gone {
            self.missing.remove(&window);
        }
        match place {
            Place::Gone => {
                let seen = self.missing.entry(window).or_default();
                *seen += 1;
                // Once: a forgotten record isn't looked at again.
                if *seen == 2 {
                    Step::Vanished
                } else {
                    Step::Stay
                }
            }
            Place::Home => {
                if self.tries.remove(&window).is_some() {
                    Step::Back
                } else {
                    Step::Stay
                }
            }
            Place::Off if last => {
                self.tries.remove(&window);
                Step::HandToUser
            }
            Place::Off => {
                *self.tries.entry(window).or_default() += 1;
                Step::PutBack
            }
        }
    }

    /// How many times `window` was put back in this regather so far.
    pub fn tries(&self, window: WindowId) -> u32 {
        self.tries.get(&window).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: Rect = Rect::new(8128.0, -2240.0, 1280.0, 800.0);
    const OTHER: Rect = Rect::new(9408.0, -2240.0, 1280.0, 800.0);

    fn at(x: f64, y: f64) -> Option<Rect> {
        Some(Rect::new(x, y, 600.0, 400.0))
    }

    #[test]
    fn a_window_on_its_own_or_another_live_display_is_home() {
        assert_eq!(place(at(8300.0, -2100.0), OWN, &[]), Place::Home);
        assert_eq!(place(at(9500.0, -2100.0), OWN, &[OTHER]), Place::Home);
    }

    #[test]
    fn a_window_on_a_spare_the_users_screen_or_nowhere_is_off() {
        // On a spare: not in the live list.
        assert_eq!(place(at(9500.0, -2100.0), OWN, &[]), Place::Off);
        // On the user's main screen.
        assert_eq!(place(at(400.0, 300.0), OWN, &[OTHER]), Place::Off);
        // Off every display.
        assert_eq!(place(at(-90000.0, 0.0), OWN, &[]), Place::Off);
    }

    #[test]
    fn a_window_in_no_list_is_gone() {
        assert_eq!(place(None, OWN, &[OTHER]), Place::Gone);
    }

    #[test]
    fn a_window_that_stays_home_is_left_alone() {
        let mut r = Regather::new();
        for pass in 0..PASSES_MS.len() {
            assert_eq!(r.step(1, Place::Home, pass), Step::Stay);
        }
    }

    /// The run of #291 before the fix: the one move failed, and the
    /// window stayed off. Now every pass tries again.
    #[test]
    fn a_put_back_that_fails_is_tried_again_each_pass() {
        let mut r = Regather::new();
        let last = PASSES_MS.len() - 1;
        for pass in 0..last {
            assert_eq!(r.step(7, Place::Off, pass), Step::PutBack);
            assert_eq!(r.tries(7), pass as u32 + 1);
        }
        assert_eq!(r.step(7, Place::Off, last), Step::HandToUser);
        assert_eq!(r.tries(7), 0);
    }

    #[test]
    fn a_window_put_back_is_noted_once_it_is_home() {
        let mut r = Regather::new();
        assert_eq!(r.step(7, Place::Off, 0), Step::PutBack);
        assert_eq!(r.step(7, Place::Off, 1), Step::PutBack);
        assert_eq!(r.step(7, Place::Home, 2), Step::Back);
        assert_eq!(r.step(7, Place::Home, 3), Step::Stay);
        assert_eq!(r.step(7, Place::Home, 4), Step::Stay);
    }

    #[test]
    fn a_window_home_only_by_the_last_pass_is_back_not_handed_over() {
        let mut r = Regather::new();
        let last = PASSES_MS.len() - 1;
        for pass in 0..last {
            r.step(7, Place::Off, pass);
        }
        assert_eq!(r.step(7, Place::Home, last), Step::Back);
    }

    #[test]
    fn a_window_missing_from_one_pass_only_is_kept() {
        // WindowServer's list may skip a window mid-reconfiguration.
        let mut r = Regather::new();
        assert_eq!(r.step(3, Place::Gone, 0), Step::Stay);
        assert_eq!(r.step(3, Place::Home, 1), Step::Stay);
        assert_eq!(r.step(3, Place::Gone, 2), Step::Stay);
        assert_eq!(r.step(3, Place::Home, 3), Step::Stay);
    }

    #[test]
    fn a_window_missing_two_passes_in_a_row_vanished_once() {
        let mut r = Regather::new();
        assert_eq!(r.step(3, Place::Gone, 0), Step::Stay);
        assert_eq!(r.step(3, Place::Gone, 1), Step::Vanished);
        assert_eq!(r.step(3, Place::Gone, 2), Step::Stay);
    }

    #[test]
    fn a_window_that_vanished_after_falling_off_is_not_handed_over() {
        let mut r = Regather::new();
        let last = PASSES_MS.len() - 1;
        assert_eq!(r.step(5, Place::Off, 0), Step::PutBack);
        for pass in 1..last {
            r.step(5, Place::Gone, pass);
        }
        assert_eq!(r.step(5, Place::Gone, last), Step::Stay);
    }

    #[test]
    fn windows_are_judged_apart() {
        let mut r = Regather::new();
        assert_eq!(r.step(1, Place::Off, 0), Step::PutBack);
        assert_eq!(r.step(2, Place::Home, 0), Step::Stay);
        assert_eq!(r.step(1, Place::Home, 1), Step::Back);
        assert_eq!(r.step(2, Place::Home, 1), Step::Stay);
    }
}
