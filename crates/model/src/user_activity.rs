//! The User Activity Monitor's rules (ADR 0025): which of the user's
//! windows are In Use, and whether their hands are At Rest. The platform
//! samples the OS and feeds `Sample`s; nothing here reads it.
//!
//! A user window is In Use when the user clicked, typed into, scrolled or
//! pointed at it within `WINDOW_QUIET_MS`, when its app holds a
//! display-sleep assertion (video, a call), or while the user holds a
//! button on it. The hands are At Rest after `REST_MS` without input, with
//! no button or key held, no composition, Secure Input off, the screen
//! unlocked and no system prompt in front.

use crate::autonomy::Busy;
use crate::model::WindowId;
use std::collections::{BTreeMap, BTreeSet};

/// A window is In Use until this long after its last use.
pub const WINDOW_QUIET_MS: u64 = 120_000;
/// Idle time before the hands are At Rest.
pub const REST_MS: u64 = 5_000;
/// The same, after the user interrupted a mode-opened grant or hold, until
/// one completes untouched.
pub const REST_AFTER_INTERRUPT_MS: u64 = 30_000;
/// In Auto the Red Glow shows Armed this long before the first event of a
/// mode-opened grant. A touch in that time cancels it.
pub const LEAD_MS: u64 = 400;
/// ADR 0008's quiet time, unchanged.
pub const BORROW_REST_MS: u64 = 2_000;
/// How long an Auto Keyboard Borrow waits for that quiet.
pub const BORROW_WAIT_MS: u64 = 10_000;
/// In YOLO, how long a running operation may go on after the first user
/// key or press is held.
pub const HELD_GRACE_MS: u64 = 1_500;
/// After a hold ends, the time the user's input flows freely before
/// another may start.
pub const HOLD_GAP_MS: u64 = 1_000;
/// The monitor's period.
pub const SAMPLE_MS: u64 = 1_000;
/// Its period while a crossing waits on it.
pub const WAITING_SAMPLE_MS: u64 = 250;
/// After a waiting call answered, how long the Auto line waits before it
/// shows "Waiting for your mouse".
pub const HANDS_LINE_MS: u64 = 30_000;
/// The most windows the tracker keeps.
pub const MAX_WINDOWS: usize = 512;
/// A window not used for this long is forgotten.
pub const FORGET_MS: u64 = 600_000;

const _: () = assert!(REST_MS < REST_AFTER_INTERRUPT_MS && BORROW_REST_MS < REST_MS);
const _: () = assert!(WAITING_SAMPLE_MS < SAMPLE_MS && SAMPLE_MS < REST_MS);

/// One look at the user's input and screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    pub now_ms: u64,
    /// Since the user's last input, Agent Screen's own posts discounted.
    pub idle_ms: u64,
    /// The front app's focused window.
    pub active_window: Option<WindowId>,
    /// The window under the pointer.
    pub pointer_window: Option<WindowId>,
    pub button_down: bool,
    pub key_down: bool,
    pub composing: bool,
    pub secure_input: bool,
    pub locked: bool,
    pub system_prompt: bool,
    /// On-screen windows of apps holding a display-sleep assertion.
    pub media_windows: Vec<WindowId>,
}

impl Sample {
    /// Whether the user gave input since the last tick.
    fn active(&self) -> bool {
        self.idle_ms < SAMPLE_MS + 200
    }
}

/// The hands as the last sample saw them, with the interrupt and hold
/// marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hands {
    pub idle_ms: u64,
    pub button_down: bool,
    pub key_down: bool,
    pub composing: bool,
    pub secure_input: bool,
    pub locked: bool,
    pub system_prompt: bool,
    /// The user interrupted a mode-opened grant or hold, and none has
    /// completed untouched since.
    pub interrupted: bool,
    /// Since the last hold ended; None when none has.
    pub since_hold_ms: Option<u64>,
}

impl Hands {
    /// The conditions every mode respects, YOLO included.
    fn hard(&self) -> Result<(), Busy> {
        if self.locked {
            return Err(Busy::Locked);
        }
        if self.secure_input {
            return Err(Busy::SecureInput);
        }
        if self.system_prompt {
            return Err(Busy::SystemPrompt);
        }
        if self.button_down || self.key_down {
            return Err(Busy::ButtonHeld);
        }
        if self.since_hold_ms.is_some_and(|ms| ms < HOLD_GAP_MS) {
            return Err(Busy::HoldGap);
        }
        Ok(())
    }

    fn quiet(&self, rest_ms: u64) -> Result<(), Busy> {
        self.hard()?;
        if self.composing {
            return Err(Busy::Composing);
        }
        if self.idle_ms < rest_ms {
            return Err(Busy::Active);
        }
        Ok(())
    }

    /// The rest time now: longer after an interrupt.
    pub fn rest_ms(&self) -> u64 {
        if self.interrupted {
            REST_AFTER_INTERRUPT_MS
        } else {
            REST_MS
        }
    }

    /// At Rest, for Auto's grants and holds.
    pub fn at_rest(&self) -> Result<(), Busy> {
        self.quiet(self.rest_ms())
    }

    /// ADR 0008's 2 s quiet, with no composition, for Auto's Keyboard
    /// Borrow.
    pub fn quiet_for_borrow(&self) -> Result<(), Busy> {
        self.quiet(BORROW_REST_MS)
    }

    /// What YOLO waits for: no button or key held, Secure Input off, no
    /// system prompt, the screen unlocked, and the gap after a hold.
    pub fn yolo_may_start(&self) -> Result<(), Busy> {
        self.hard()
    }
}

/// When each of the user's windows was last used.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    used: BTreeMap<WindowId, u64>,
    last: Sample,
    interrupted: bool,
    hold_ended: Option<u64>,
}

impl Tracker {
    /// The monitor starts: the active window and the window under the
    /// pointer are used now, and the presses the catch still holds, each
    /// at its time. Every other window starts as not In Use.
    pub fn seed(&mut self, sample: &Sample, presses: impl IntoIterator<Item = (WindowId, u64)>) {
        for (window, at) in presses {
            self.mark(window, at.min(sample.now_ms));
        }
        for window in [sample.active_window, sample.pointer_window]
            .into_iter()
            .flatten()
        {
            self.mark(window, sample.now_ms);
        }
        self.take(sample);
    }

    /// One tick. An idle tick marks nothing; an active one marks the active
    /// window and the one under the pointer. Media windows are used on
    /// each look.
    pub fn observe(&mut self, sample: &Sample) {
        if sample.active() && !sample.locked {
            for window in [sample.active_window, sample.pointer_window]
                .into_iter()
                .flatten()
            {
                self.mark(window, sample.now_ms);
            }
        }
        self.take(sample);
    }

    fn take(&mut self, sample: &Sample) {
        for &window in &sample.media_windows {
            self.mark(window, sample.now_ms);
        }
        self.last = sample.clone();
        self.trim(sample.now_ms);
    }

    /// A press the global monitor saw, on `window`.
    pub fn pressed(&mut self, window: WindowId, now_ms: u64) {
        self.mark(window, now_ms);
        self.trim(now_ms);
    }

    fn mark(&mut self, window: WindowId, at: u64) {
        let entry = self.used.entry(window).or_insert(at);
        *entry = (*entry).max(at);
    }

    fn trim(&mut self, now_ms: u64) {
        self.used
            .retain(|_, at| now_ms.saturating_sub(*at) < FORGET_MS);
        while self.used.len() > MAX_WINDOWS {
            let Some((&oldest, _)) = self.used.iter().min_by_key(|(_, at)| **at) else {
                break;
            };
            self.used.remove(&oldest);
        }
    }

    pub fn in_use(&self, window: WindowId, now_ms: u64) -> bool {
        if self.last.locked {
            return false;
        }
        if self.last.media_windows.contains(&window) {
            return true;
        }
        if self.last.button_down && self.last.pointer_window == Some(window) {
            return true;
        }
        self.unused_ms(window, now_ms)
            .is_some_and(|ms| ms < WINDOW_QUIET_MS)
    }

    /// Whether the user holds a button with the pointer on `window`.
    pub fn pressed_on(&self, window: WindowId) -> bool {
        self.last.button_down && self.last.pointer_window == Some(window)
    }

    /// Since the window was last used; None when the tracker doesn't know
    /// it.
    pub fn unused_ms(&self, window: WindowId, now_ms: u64) -> Option<u64> {
        self.used.get(&window).map(|at| now_ms.saturating_sub(*at))
    }

    /// The windows the tracker knows, with when each was last used.
    pub fn known(&self) -> impl Iterator<Item = (WindowId, u64)> + '_ {
        self.used.iter().map(|(w, at)| (*w, *at))
    }

    pub fn hands(&self, now_ms: u64) -> Hands {
        let s = &self.last;
        Hands {
            idle_ms: s.idle_ms,
            button_down: s.button_down,
            key_down: s.key_down,
            composing: s.composing,
            secure_input: s.secure_input,
            locked: s.locked,
            system_prompt: s.system_prompt,
            interrupted: self.interrupted,
            since_hold_ms: self.hold_ended.map(|at| now_ms.saturating_sub(at)),
        }
    }

    /// The user interrupted a mode-opened grant or hold: the next needs
    /// `REST_AFTER_INTERRUPT_MS`.
    pub fn interrupted(&mut self, _now_ms: u64) {
        self.interrupted = true;
    }

    /// One completed untouched: the rest is `REST_MS` again.
    pub fn completed_untouched(&mut self) {
        self.interrupted = false;
    }

    /// A hold ended: none starts for `HOLD_GAP_MS`.
    pub fn hold_ended(&mut self, now_ms: u64) {
        self.hold_ended = Some(now_ms);
    }

    /// Forget the windows that closed.
    pub fn forget_closed(&mut self, open: &BTreeSet<WindowId>) {
        self.used.retain(|w, _| open.contains(w));
    }

    /// The last sample.
    pub fn last(&self) -> &Sample {
        &self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now_ms: u64, idle_ms: u64) -> Sample {
        Sample {
            now_ms,
            idle_ms,
            ..Sample::default()
        }
    }

    #[test]
    fn a_window_is_in_use_for_exactly_120_s_after_its_last_use() {
        let mut t = Tracker::default();
        t.pressed(7, 1_000);
        assert!(t.in_use(7, 1_000));
        assert!(t.in_use(7, 1_000 + WINDOW_QUIET_MS - 1));
        assert!(!t.in_use(7, 1_000 + WINDOW_QUIET_MS));
        assert_eq!(t.unused_ms(7, 5_000), Some(4_000));
        assert_eq!(t.unused_ms(8, 5_000), None);
        assert!(!t.in_use(8, 5_000));
    }

    #[test]
    fn an_idle_tick_marks_nothing() {
        let mut t = Tracker::default();
        t.observe(&Sample {
            active_window: Some(7),
            pointer_window: Some(8),
            ..at(10_000, SAMPLE_MS + 200)
        });
        assert!(!t.in_use(7, 10_000));
        assert!(!t.in_use(8, 10_000));
    }

    #[test]
    fn an_active_tick_marks_the_active_window_and_the_one_under_the_pointer() {
        let mut t = Tracker::default();
        t.observe(&Sample {
            active_window: Some(7),
            pointer_window: Some(8),
            ..at(10_000, 100)
        });
        assert!(t.in_use(7, 10_000));
        assert!(t.in_use(8, 10_000));
        assert!(!t.in_use(9, 10_000));
    }

    #[test]
    fn a_media_window_stays_in_use_with_no_input() {
        let mut t = Tracker::default();
        let media = Sample {
            media_windows: vec![7],
            ..at(10_000, 600_000)
        };
        t.observe(&media);
        assert!(t.in_use(7, 10_000 + WINDOW_QUIET_MS * 3));
        // The video stopped: 120 s after its last look it is free.
        t.observe(&at(20_000, 600_000));
        assert!(t.in_use(7, 20_000));
        assert!(!t.in_use(7, 10_000 + WINDOW_QUIET_MS));
    }

    #[test]
    fn a_locked_screen_frees_every_window() {
        let mut t = Tracker::default();
        t.pressed(7, 10_000);
        t.observe(&Sample {
            locked: true,
            media_windows: vec![8],
            ..at(10_500, 0)
        });
        assert!(!t.in_use(7, 10_500));
        assert!(!t.in_use(8, 10_500));
    }

    #[test]
    fn a_held_button_keeps_the_window_under_the_pointer_in_use() {
        let mut t = Tracker::default();
        t.observe(&Sample {
            button_down: true,
            pointer_window: Some(7),
            ..at(500_000, 900_000)
        });
        assert!(t.in_use(7, 500_000));
        assert!(t.pressed_on(7));
        assert!(!t.pressed_on(8));
    }

    #[test]
    fn rest_needs_5_s_and_30_s_after_an_interrupt_and_a_completed_grant_clears_it() {
        let mut t = Tracker::default();
        t.observe(&at(100_000, REST_MS - 1));
        assert_eq!(t.hands(100_000).at_rest(), Err(Busy::Active));
        t.observe(&at(100_001, REST_MS));
        assert_eq!(t.hands(100_001).at_rest(), Ok(()));
        t.interrupted(100_001);
        assert_eq!(t.hands(100_001).at_rest(), Err(Busy::Active));
        t.observe(&at(200_000, REST_AFTER_INTERRUPT_MS));
        assert_eq!(t.hands(200_000).at_rest(), Ok(()));
        t.completed_untouched();
        t.observe(&at(200_001, REST_MS));
        assert_eq!(t.hands(200_001).at_rest(), Ok(()));
    }

    #[test]
    fn each_busy_reason_is_its_own() {
        let rested = at(100_000, 60_000);
        for (sample, busy) in [
            (
                Sample {
                    button_down: true,
                    ..rested.clone()
                },
                Busy::ButtonHeld,
            ),
            (
                Sample {
                    key_down: true,
                    ..rested.clone()
                },
                Busy::ButtonHeld,
            ),
            (
                Sample {
                    composing: true,
                    ..rested.clone()
                },
                Busy::Composing,
            ),
            (
                Sample {
                    secure_input: true,
                    ..rested.clone()
                },
                Busy::SecureInput,
            ),
            (
                Sample {
                    system_prompt: true,
                    ..rested.clone()
                },
                Busy::SystemPrompt,
            ),
            (
                Sample {
                    locked: true,
                    ..rested.clone()
                },
                Busy::Locked,
            ),
        ] {
            let mut t = Tracker::default();
            t.observe(&sample);
            assert_eq!(t.hands(100_000).at_rest(), Err(busy), "{sample:?}");
        }
    }

    #[test]
    fn the_gap_after_a_hold() {
        let mut t = Tracker::default();
        t.observe(&at(100_000, 60_000));
        t.hold_ended(99_500);
        assert_eq!(t.hands(100_000).at_rest(), Err(Busy::HoldGap));
        assert_eq!(t.hands(100_000).yolo_may_start(), Err(Busy::HoldGap));
        assert_eq!(t.hands(99_500 + HOLD_GAP_MS).at_rest(), Ok(()));
        assert_eq!(t.hands(99_500 + HOLD_GAP_MS).yolo_may_start(), Ok(()));
    }

    #[test]
    fn the_tracker_holds_512_windows_at_most_and_forgets_after_10_minutes() {
        let mut t = Tracker::default();
        for w in 0..(MAX_WINDOWS as u32 + 10) {
            t.pressed(w, 1_000 + u64::from(w));
        }
        assert_eq!(t.known().count(), MAX_WINDOWS);
        // The oldest went first.
        assert_eq!(t.unused_ms(0, 2_000), None);
        assert!(t.unused_ms(MAX_WINDOWS as u32 + 9, 2_000).is_some());
        t.observe(&at(1_000 + FORGET_MS + 600, 900_000));
        assert_eq!(t.known().count(), 0);
    }

    #[test]
    fn seeding_takes_old_presses() {
        let mut t = Tracker::default();
        let start = Sample {
            active_window: Some(1),
            pointer_window: Some(2),
            ..at(300_000, 90_000)
        };
        t.seed(&start, [(3, 250_000), (4, 100_000)]);
        assert!(t.in_use(1, 300_000));
        assert!(t.in_use(2, 300_000));
        assert!(t.in_use(3, 300_000));
        assert!(!t.in_use(4, 300_000));
        assert_eq!(t.unused_ms(3, 300_000), Some(50_000));
        assert!(!t.in_use(5, 300_000));
    }

    #[test]
    fn closed_windows_are_forgotten() {
        let mut t = Tracker::default();
        t.pressed(1, 10);
        t.pressed(2, 10);
        t.forget_closed(&[2].into());
        assert_eq!(t.unused_ms(1, 20), None);
        assert_eq!(t.unused_ms(2, 20), Some(10));
    }
}
