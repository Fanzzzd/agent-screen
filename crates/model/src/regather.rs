//! Putting an Agent Display's windows back after the displays change (#291).
//!
//! When a display comes or goes, macOS moves the others and may drop their
//! windows elsewhere: on the user's screen, on a spare or a unit twin Agent
//! Screen is making, on another session's display, or off every display. A
//! regather looks at each window recorded on a live Agent Display a few
//! times while the displays settle and puts back each one that is off.
//!
//! It used to look twice and believe the move's answer. A window whose move
//! failed once stayed off its display. One that landed on a spare or a
//! unit twin counted as on an Agent Display and stayed there: no session
//! shows a spare, and its recorded owner (ADR 0018) kept the window listed
//! on its own display all the same. A window that vanished left no log
//! line.
//!
//! Now each pass reads where the window really is. Home is only the display
//! the window is recorded on: a move between displays that the agent asks
//! for moves the record too (ADR 0018), so a window on any other display
//! was dropped there. Each window gets `MAX_TRIES` put backs of its own,
//! however late in the regather it is first seen off, and the regather runs
//! extra passes until those are spent. One still off after them goes to the
//! user's screen, and its record is dropped only once that return is
//! confirmed. A window missing from two passes in a row is logged and
//! forgotten.

use crate::model::{DisplayId, Rect, WindowId};
use std::collections::{HashMap, HashSet};

/// When the first passes run, in milliseconds after the one before; the
/// first counts from the display change. A newer change starts over.
pub const PASSES_MS: [u64; 5] = [500, 1500, 3000, 6000, 1000];
/// The wait before each pass past `PASSES_MS`, run while a window still
/// has tries left or waits to be confirmed gone.
pub const EXTRA_PASS_MS: u64 = 2000;
/// Put backs per window before it goes to the user's screen.
pub const MAX_TRIES: u32 = 4;
/// Passes in all, at most, whatever windows come and go meanwhile.
pub const MAX_PASSES: usize = 20;

/// The wait before pass `pass`, from 0.
pub fn wait_before(pass: usize) -> u64 {
    PASSES_MS.get(pass).copied().unwrap_or(EXTRA_PASS_MS)
}

/// Where a recorded window is during a regather.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In no window list.
    Gone,
    /// Its center is on the display it is recorded on.
    Home,
    /// Anywhere else: the user's screen, a spare, a unit twin, another
    /// session's display, or no display at all.
    Off,
}

/// Where the window with `frame` is, given the frame of the display it is
/// recorded on.
pub fn place(frame: Option<Rect>, own: Rect) -> Place {
    match frame {
        None => Place::Gone,
        Some(frame) if own.contains(frame.mid()) => Place::Home,
        Some(_) => Place::Off,
    }
}

/// When a hand-back looks for stranded windows (#319), in milliseconds
/// after the one before; the first counts from the hand-back.
pub const HAND_BACK_PASSES_MS: [u64; 3] = [300, 1000, 2500];

/// A window shows too little to use when what of it is on the online
/// displays together is narrower than this, in points,
pub const MIN_VISIBLE_WIDTH: f64 = 100.0;
/// or lower than this,
pub const MIN_VISIBLE_HEIGHT: f64 = 40.0;
/// or this share of its frame or less.
pub const MIN_VISIBLE_SHARE: f64 = 0.1;

/// Whether a window at `frame` is stranded: so little of it is on the
/// online `displays`, all of them together, that neither the user nor the
/// agent can use it. The agents' Chrome window left its Agent Display
/// during a Takeover for a spot above the user's screen with 30 points
/// showing (#319). A window the user parked at a screen's edge with a
/// good part showing is not, nor one split across two displays.
pub fn stranded(frame: Rect, displays: &[Rect]) -> bool {
    let pieces: Vec<Rect> = displays
        .iter()
        .filter_map(|d| intersection(frame, *d))
        .collect();
    let Some(first) = pieces.first() else {
        return true;
    };
    let span = pieces.iter().fold(*first, |all, p| all.union(p));
    let area: f64 = pieces.iter().map(|p| p.width * p.height).sum();
    span.width < MIN_VISIBLE_WIDTH
        || span.height < MIN_VISIBLE_HEIGHT
        || area <= frame.width * frame.height * MIN_VISIBLE_SHARE
}

/// The part of `a` inside `b`, if any.
fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    let (max_x, max_y) = (a.max_x().min(b.max_x()), a.max_y().min(b.max_y()));
    (max_x > x && max_y > y).then(|| Rect::new(x, y, max_x - x, max_y - y))
}

/// What a pass does with one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing: it is home, it was missing from one pass only, or its
    /// return to the user's screen failed and it stays as it is.
    Stay,
    /// It was off and is home again: say so.
    Back,
    /// Move it onto its own display.
    PutBack,
    /// It was in no window list two passes in a row: its app closed it.
    /// Say so and forget its record.
    Vanished,
    /// It is still off after its last try: return it to the user's screen,
    /// so it isn't left on a spare or tiled into the user's workspace as
    /// the agent's. Its record goes only if `handed` says so.
    HandToUser,
}

/// A window as recorded on a display: a window whose record moves to
/// another display starts over there, with every try.
pub type Key = (WindowId, DisplayId);

/// One regather: what it saw of each window in earlier passes.
#[derive(Debug, Default)]
pub struct Regather {
    tries: HashMap<Key, u32>,
    missing: HashMap<Key, u32>,
    /// Windows whose return to the user's screen wasn't confirmed: left
    /// where they are, record and all, for the rest of this regather.
    stuck: HashSet<Key>,
}

impl Regather {
    pub fn new() -> Self {
        Self::default()
    }

    /// The step for `window`, recorded on `display`, seen at `place` in
    /// this pass.
    pub fn step(&mut self, window: WindowId, display: DisplayId, place: Place) -> Step {
        let window = (window, display);
        if self.stuck.contains(&window) {
            return Step::Stay;
        }
        if place != Place::Gone {
            self.missing.remove(&window);
        }
        match place {
            Place::Gone => {
                let seen = self.missing.entry(window).or_default();
                *seen += 1;
                // Once: a forgotten record isn't looked at again.
                if *seen == 2 {
                    self.tries.remove(&window);
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
            Place::Off => {
                let tries = self.tries.entry(window).or_default();
                if *tries < MAX_TRIES {
                    *tries += 1;
                    Step::PutBack
                } else {
                    self.tries.remove(&window);
                    Step::HandToUser
                }
            }
        }
    }

    /// `window`'s return to the user's screen ended, `returned` when the
    /// window is confirmed off every Agent Display. Whether to forget its
    /// record. One that didn't leave keeps its record and the place to
    /// return it to, and this regather leaves it alone.
    pub fn handed(&mut self, window: WindowId, display: DisplayId, returned: bool) -> bool {
        let window = (window, display);
        if !returned {
            self.stuck.insert(window);
        }
        returned
    }

    /// Keep what this regather knows only of `recorded`, the windows still
    /// recorded on live displays: one whose record went meanwhile, by a
    /// Take Back or the agent's own move, keeps no regather running.
    pub fn retain(&mut self, recorded: &[Key]) {
        self.tries.retain(|w, _| recorded.contains(w));
        self.missing.retain(|w, _| recorded.contains(w));
        self.stuck.retain(|w| recorded.contains(w));
    }

    /// How many times `window` was put back on `display` in this regather
    /// so far.
    pub fn tries(&self, window: WindowId, display: DisplayId) -> u32 {
        self.tries.get(&(window, display)).copied().unwrap_or(0)
    }

    /// Whether pass `next` runs: every pass of `PASSES_MS`, then more while
    /// a window waits for a check after a put back or for a second look
    /// after it was missing, up to `MAX_PASSES`.
    pub fn runs(&self, next: usize) -> bool {
        next < PASSES_MS.len()
            || (next < MAX_PASSES && (!self.tries.is_empty() || !self.missing.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: DisplayId = 1;
    const OWN: Rect = Rect::new(8128.0, -2240.0, 1280.0, 800.0);

    fn at(x: f64, y: f64) -> Option<Rect> {
        Some(Rect::new(x, y, 600.0, 400.0))
    }

    const MAIN: Rect = Rect::new(0.0, 0.0, 2560.0, 1440.0);

    /// A display left of the main one and one above it.
    const LEFT: Rect = Rect::new(-1280.0, 200.0, 1280.0, 800.0);
    const ABOVE: Rect = Rect::new(-500.0, -1117.0, 1728.0, 1117.0);

    #[test]
    fn a_window_with_30_points_showing_is_stranded() {
        // The report: 1200 by 720 at y=-690, its bottom 30 points over
        // the main display's top edge.
        let frame = Rect::new(681.0, -690.0, 1200.0, 720.0);
        assert!(stranded(frame, &[MAIN, OWN]));
    }

    #[test]
    fn a_window_half_showing_at_a_right_edge_is_not_stranded() {
        // Parked by the user: 500 of 1200 points still on the screen.
        let frame = Rect::new(2060.0, 300.0, 1200.0, 720.0);
        assert!(!stranded(frame, &[MAIN, OWN]));
        // On its own display, and wholly on the user's screen.
        assert!(!stranded(
            Rect::new(8168.0, -2200.0, 1200.0, 720.0),
            &[MAIN, OWN]
        ));
        assert!(!stranded(
            Rect::new(400.0, 300.0, 1200.0, 720.0),
            &[MAIN, OWN]
        ));
    }

    #[test]
    fn a_window_off_every_display_is_stranded() {
        assert!(stranded(
            Rect::new(3000.0, -900.0, 1200.0, 720.0),
            &[MAIN, OWN]
        ));
        // Touching an edge shows nothing.
        assert!(stranded(Rect::new(2560.0, 0.0, 1200.0, 720.0), &[MAIN]));
    }

    #[test]
    fn what_shows_on_two_displays_counts_together() {
        // 60 points on each side of the edge between the left display and
        // the main one: 120 wide in all.
        let frame = Rect::new(-60.0, 300.0, 120.0, 400.0);
        assert!(!stranded(frame, &[LEFT, MAIN]));
        assert!(stranded(frame, &[MAIN]));
    }

    #[test]
    fn displays_left_of_and_above_the_main_one_count() {
        // Wholly on the display above, at negative coordinates.
        assert!(!stranded(
            Rect::new(-400.0, -1000.0, 1200.0, 720.0),
            &[MAIN, ABOVE]
        ));
        // Past the left display's left edge with 30 points showing.
        assert!(stranded(
            Rect::new(-2450.0, 300.0, 1200.0, 720.0),
            &[MAIN, LEFT]
        ));
        // The display above shows a good part of a window over the main
        // display's top edge, which alone shows 30 points.
        let frame = Rect::new(681.0, -690.0, 1200.0, 720.0);
        assert!(!stranded(frame, &[MAIN, ABOVE]));
    }

    #[test]
    fn with_no_display_online_every_window_is_stranded() {
        assert!(stranded(Rect::new(0.0, 0.0, 100.0, 100.0), &[]));
    }

    #[test]
    fn a_window_on_its_own_display_is_home() {
        assert_eq!(place(at(8300.0, -2100.0), OWN), Place::Home);
    }

    #[test]
    fn a_window_on_another_display_of_the_app_is_off() {
        // Another session's display, a spare or a twin, right of its own:
        // the agent moving a window there would have moved its record.
        assert_eq!(place(at(9500.0, -2100.0), OWN), Place::Off);
    }

    #[test]
    fn a_window_on_the_users_screen_or_nowhere_is_off() {
        assert_eq!(place(at(400.0, 300.0), OWN), Place::Off);
        assert_eq!(place(at(-90000.0, 0.0), OWN), Place::Off);
    }

    #[test]
    fn a_window_in_no_list_is_gone() {
        assert_eq!(place(None, OWN), Place::Gone);
    }

    #[test]
    fn a_window_that_stays_home_is_left_alone() {
        let mut r = Regather::new();
        for pass in 0..PASSES_MS.len() {
            assert_eq!(r.step(1, D, Place::Home), Step::Stay);
            assert!(r.runs(pass + 1) || pass + 1 == PASSES_MS.len());
        }
        assert!(!r.runs(PASSES_MS.len()));
    }

    /// The run of #291 before the fix: the one move failed, and the
    /// window stayed off. Now it is tried `MAX_TRIES` times and checked
    /// once more before it goes to the user's screen.
    #[test]
    fn a_put_back_that_fails_is_tried_again_each_pass() {
        let mut r = Regather::new();
        for n in 1..=MAX_TRIES {
            assert_eq!(r.step(7, D, Place::Off), Step::PutBack);
            assert_eq!(r.tries(7, D), n);
        }
        assert_eq!(r.step(7, D, Place::Off), Step::HandToUser);
        assert_eq!(r.tries(7, D), 0);
    }

    #[test]
    fn a_window_first_seen_off_in_the_last_pass_is_put_back_not_handed_over() {
        let mut r = Regather::new();
        let last = PASSES_MS.len() - 1;
        for _ in 0..last {
            assert_eq!(r.step(7, D, Place::Home), Step::Stay);
        }
        assert_eq!(r.step(7, D, Place::Off), Step::PutBack);
        // The regather goes on past `PASSES_MS` for its remaining tries.
        let mut pass = last + 1;
        while r.runs(pass) {
            match r.step(7, D, Place::Off) {
                Step::PutBack => {}
                Step::HandToUser => break,
                other => panic!("pass {pass}: {other:?}"),
            }
            pass += 1;
        }
        assert!(pass < MAX_PASSES, "it ran out of passes before its tries");
        assert!(!r.runs(pass + 1));
    }

    #[test]
    fn a_window_recorded_mid_regather_gets_every_try() {
        let mut r = Regather::new();
        for pass in 0..3 {
            r.step(1, D, Place::Home);
            assert!(r.runs(pass + 1));
        }
        let mut steps = Vec::new();
        let mut pass = 3;
        while r.runs(pass) {
            steps.push(r.step(2, D, Place::Off));
            if steps.last() == Some(&Step::HandToUser) {
                break;
            }
            pass += 1;
        }
        let tries = steps.iter().filter(|s| **s == Step::PutBack).count();
        assert_eq!(tries as u32, MAX_TRIES);
        assert_eq!(steps.last(), Some(&Step::HandToUser));
    }

    #[test]
    fn a_window_put_back_is_noted_once_it_is_home() {
        let mut r = Regather::new();
        assert_eq!(r.step(7, D, Place::Off), Step::PutBack);
        assert_eq!(r.step(7, D, Place::Off), Step::PutBack);
        assert_eq!(r.step(7, D, Place::Home), Step::Back);
        assert_eq!(r.step(7, D, Place::Home), Step::Stay);
        assert!(!r.runs(PASSES_MS.len()));
    }

    #[test]
    fn a_window_that_isnt_confirmed_returned_keeps_its_record_and_is_left_alone() {
        let mut r = Regather::new();
        for _ in 0..MAX_TRIES {
            r.step(7, D, Place::Off);
        }
        assert_eq!(r.step(7, D, Place::Off), Step::HandToUser);
        assert!(!r.handed(7, D, false), "the record stays");
        assert_eq!(r.step(7, D, Place::Off), Step::Stay);
        assert_eq!(r.step(7, D, Place::Gone), Step::Stay);
        assert!(!r.runs(PASSES_MS.len()));
    }

    #[test]
    fn a_window_confirmed_returned_loses_its_record() {
        let mut r = Regather::new();
        assert!(r.handed(7, D, true));
    }

    #[test]
    fn a_window_missing_from_one_pass_only_is_kept() {
        // WindowServer's list may skip a window mid-reconfiguration.
        let mut r = Regather::new();
        assert_eq!(r.step(3, D, Place::Gone), Step::Stay);
        assert!(r.runs(MAX_PASSES - 1), "a second look is due");
        assert_eq!(r.step(3, D, Place::Home), Step::Stay);
        assert_eq!(r.step(3, D, Place::Gone), Step::Stay);
        assert_eq!(r.step(3, D, Place::Home), Step::Stay);
    }

    #[test]
    fn a_window_missing_two_passes_in_a_row_vanished_once() {
        let mut r = Regather::new();
        assert_eq!(r.step(3, D, Place::Gone), Step::Stay);
        assert_eq!(r.step(3, D, Place::Gone), Step::Vanished);
        assert_eq!(r.step(3, D, Place::Gone), Step::Stay);
    }

    #[test]
    fn a_window_that_vanished_after_falling_off_is_not_handed_over() {
        let mut r = Regather::new();
        assert_eq!(r.step(5, D, Place::Off), Step::PutBack);
        assert_eq!(r.step(5, D, Place::Gone), Step::Stay);
        assert_eq!(r.step(5, D, Place::Gone), Step::Vanished);
        assert_eq!(r.tries(5, D), 0);
        assert_eq!(r.step(5, D, Place::Gone), Step::Stay);
    }

    #[test]
    fn windows_are_judged_apart() {
        let mut r = Regather::new();
        assert_eq!(r.step(1, D, Place::Off), Step::PutBack);
        assert_eq!(r.step(2, D, Place::Home), Step::Stay);
        assert_eq!(r.step(1, D, Place::Home), Step::Back);
        assert_eq!(r.step(2, D, Place::Home), Step::Stay);
    }

    #[test]
    fn a_window_no_longer_recorded_keeps_no_pass_running() {
        let mut r = Regather::new();
        r.step(1, D, Place::Off);
        r.step(2, D, Place::Gone);
        assert!(r.runs(PASSES_MS.len()));
        r.retain(&[(3, D)]);
        assert!(!r.runs(PASSES_MS.len()));
        assert_eq!(r.tries(1, D), 0);
    }

    /// Its record moved to display 2 meanwhile, as the agent moving it
    /// does: it gets every try there, not a hand-over with none.
    #[test]
    fn a_window_whose_record_moved_gets_fresh_tries_on_its_new_display() {
        let mut r = Regather::new();
        for _ in 0..MAX_TRIES {
            assert_eq!(r.step(7, D, Place::Off), Step::PutBack);
        }
        r.retain(&[(7, 2)]);
        assert_eq!(r.tries(7, D), 0);
        assert_eq!(r.step(7, 2, Place::Off), Step::PutBack);
        assert_eq!(r.tries(7, 2), 1);
    }

    #[test]
    fn a_record_moved_without_a_retain_between_still_starts_over() {
        let mut r = Regather::new();
        for _ in 0..MAX_TRIES {
            r.step(7, D, Place::Off);
        }
        assert_eq!(r.step(7, 2, Place::Off), Step::PutBack);
    }

    #[test]
    fn passes_stop_at_the_cap() {
        let mut r = Regather::new();
        r.step(1, D, Place::Off);
        assert!(r.runs(MAX_PASSES - 1));
        assert!(!r.runs(MAX_PASSES));
        assert_eq!(wait_before(0), PASSES_MS[0]);
        assert_eq!(wait_before(PASSES_MS.len()), EXTRA_PASS_MS);
    }
}
