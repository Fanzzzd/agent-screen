//! Peek presentation intent, with monotonic deadlines supplied by the caller.
//! Visibility, hover dwell and explicit activation are separate decisions.

/// Candidate dwell/grace values informed by web UX research, not macOS
/// settings or measured presentation latency. See docs/research/hover-click-intent.md.
pub const OPEN_DELAY_MS: u64 = 300;
pub const CLOSE_DELAY_MS: u64 = 500;
/// A reminder of where an app's windows are keeps Peek up this long, or
/// until the pointer, having come onto it, leaves.
pub const REMIND_MS: u64 = 5000;

/// Explicit Dock clicks express a new user request. Automatic activations
/// are rate-limited so an app cannot repeatedly pull Peek open.
pub fn reminder_due(elapsed_ms: Option<u64>, docked: bool) -> bool {
    docked || elapsed_ms.is_none_or(|elapsed| elapsed >= 60_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub open_ms: u64,
    pub close_ms: u64,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            open_ms: OPEN_DELAY_MS,
            close_ms: CLOSE_DELAY_MS,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Closed,
    Hover,
    Pinned,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointer {
    pub icon: bool,
    pub peek: bool,
}
impl Pointer {
    pub fn inside(self) -> bool {
        self.icon || self.peek
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Show,
    Hide,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Open,
    Close,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    pub due_ms: u64,
    generation: u64,
    kind: Kind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Click {
    Pin,
    Close,
}
#[derive(Clone, Copy, Debug)]
struct Press {
    id: u64,
    action: Option<Click>,
}

#[derive(Default, Debug)]
pub struct Intent {
    mode: Mode,
    pointer: Pointer,
    suppressed: bool,
    stopped: bool,
    generation: u64,
    timer: Option<Timer>,
    press: Option<Press>,
}
impl Intent {
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn stopped(&self) -> bool {
        self.stopped
    }
    pub fn pending(&self) -> Option<Timer> {
        self.timer
    }
    pub fn cancel_timer(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.timer = None;
    }
    fn schedule(&mut self, kind: Kind, now: u64, delay: u64) {
        if self.timer.is_some_and(|t| t.kind == kind) {
            return;
        }
        self.cancel_timer();
        self.timer = Some(Timer {
            kind,
            due_ms: now.saturating_add(delay),
            generation: self.generation,
        });
    }
    pub fn pointer(&mut self, pointer: Pointer, now: u64, timing: Timing) {
        self.pointer = pointer;
        if !pointer.icon {
            self.suppressed = false;
        }
        if self.stopped || self.press.is_some_and(|p| p.action.is_some()) {
            return;
        }
        match self.mode {
            Mode::Closed if pointer.icon && !self.suppressed => {
                self.schedule(Kind::Open, now, timing.open_ms)
            }
            Mode::Hover if !pointer.inside() => self.schedule(Kind::Close, now, timing.close_ms),
            _ => self.cancel_timer(),
        }
    }
    pub fn fire(&mut self, timer: Timer, now: u64) -> Effect {
        if self.stopped || self.timer != Some(timer) || now < timer.due_ms {
            return Effect::None;
        }
        self.cancel_timer();
        match timer.kind {
            Kind::Open if self.mode == Mode::Closed && self.pointer.icon && !self.suppressed => {
                self.mode = Mode::Hover;
                Effect::Show
            }
            Kind::Close if self.mode == Mode::Hover && !self.pointer.inside() => {
                self.mode = Mode::Closed;
                Effect::Hide
            }
            _ => Effect::None,
        }
    }
    fn click_intent(&self) -> Click {
        // A visible hover is transient. Only an already pinned Peek closes.
        if self.mode == Mode::Pinned {
            Click::Close
        } else {
            Click::Pin
        }
    }
    pub fn press(&mut self, id: u64) {
        if self.stopped || self.press.is_some_and(|p| p.id == id) {
            return;
        }
        let action = self.click_intent();
        self.cancel_timer();
        self.press = Some(Press {
            id,
            action: Some(action),
        });
    }
    pub fn release(&mut self, valid: bool) -> Effect {
        if self.stopped {
            return Effect::None;
        }
        let action = self.press.as_mut().and_then(|p| p.action.take());
        if !valid {
            self.suppressed |= self.mode == Mode::Closed && self.pointer.icon;
            return Effect::None;
        }
        action.map_or(Effect::None, |action| self.apply_click(action))
    }
    fn apply_click(&mut self, action: Click) -> Effect {
        self.cancel_timer();
        match action {
            Click::Pin => {
                self.mode = Mode::Pinned;
                self.suppressed = false;
                Effect::Show
            }
            Click::Close => {
                self.dismiss();
                self.suppressed = true;
                Effect::Hide
            }
        }
    }
    /// Keyboard/accessibility/debug activation is not a second mouse phase.
    pub fn activate(&mut self) -> Effect {
        if self.stopped {
            return Effect::None;
        }
        self.cancel_press();
        self.apply_click(self.click_intent())
    }
    /// Shown without the pointer, to say where an app's windows are: it
    /// goes after `hold_ms` like a hover Peek the pointer left, unless the
    /// pointer comes onto it. A repeated reminder refreshes a hover Peek's
    /// close deadline; a pinned Peek stays pinned.
    pub fn remind(&mut self, now: u64, hold_ms: u64) -> Effect {
        if self.stopped || self.mode == Mode::Pinned {
            return Effect::None;
        }
        let effect = if self.mode == Mode::Closed {
            Effect::Show
        } else {
            Effect::None
        };
        self.cancel_press();
        self.mode = Mode::Hover;
        self.cancel_timer();
        if !self.pointer.inside() {
            self.schedule(Kind::Close, now, hold_ms);
        }
        effect
    }
    pub fn pin(&mut self) -> Effect {
        if self.stopped {
            return Effect::None;
        }
        self.cancel_press();
        self.apply_click(Click::Pin)
    }
    fn cancel_press(&mut self) {
        if let Some(press) = self.press.as_mut() {
            press.action = None;
        }
    }
    pub fn outside_press(&mut self, pointer: Pointer) -> Effect {
        if pointer.inside() {
            return Effect::None;
        }
        let visible = self.mode != Mode::Closed;
        self.dismiss();
        if visible { Effect::Hide } else { Effect::None }
    }
    pub fn dismiss(&mut self) {
        self.cancel_timer();
        self.cancel_press();
        self.mode = Mode::Closed;
        self.pointer.peek = false;
        self.suppressed |= self.pointer.icon;
    }
    pub fn stop(&mut self) {
        self.dismiss();
        self.stopped = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ICON: Pointer = Pointer {
        icon: true,
        peek: false,
    };
    const OUT: Pointer = Pointer {
        icon: false,
        peek: false,
    };
    const PEEK: Pointer = Pointer {
        icon: false,
        peek: true,
    };
    fn hovered() -> Intent {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let timer = intent.pending().unwrap();
        assert_eq!(intent.fire(timer, OPEN_DELAY_MS), Effect::Show);
        intent
    }
    #[test]
    fn dock_clicks_remind_each_time_but_automatic_activations_still_wait() {
        for elapsed in [0, 1000, 4999, 59_999, 60_000] {
            assert!(reminder_due(Some(elapsed), true));
        }
        assert!(reminder_due(None, false));
        assert!(!reminder_due(Some(59_999), false));
        assert!(reminder_due(Some(60_000), false));
    }

    #[test]
    fn a_repeated_reminder_refreshes_its_deadline_and_invalidates_the_old_timer() {
        let mut intent = Intent::default();
        assert_eq!(intent.remind(0, REMIND_MS), Effect::Show);
        let first = intent.pending().unwrap();
        assert_eq!(intent.remind(4000, REMIND_MS), Effect::None);
        let repeated = intent.pending().unwrap();
        assert_eq!(repeated.due_ms, 9000);
        assert_eq!(intent.fire(first, 5000), Effect::None);
        assert_eq!(intent.fire(repeated, 9000), Effect::Hide);
    }

    #[test]
    fn a_repeated_reminder_does_not_close_while_the_pointer_is_inside() {
        let mut intent = Intent::default();
        intent.remind(0, REMIND_MS);
        intent.pointer(PEEK, 100, Timing::default());
        intent.remind(1000, REMIND_MS);
        assert!(intent.pending().is_none());
        intent.pointer(OUT, 2000, Timing::default());
        let timer = intent.pending().unwrap();
        assert_eq!(timer.due_ms, 2000 + CLOSE_DELAY_MS);
    }

    #[test]
    fn a_reminder_goes_by_itself_unless_the_pointer_comes() {
        let mut intent = Intent::default();
        assert_eq!(intent.remind(0, REMIND_MS), Effect::Show);
        // The pointer moving elsewhere doesn't shorten it.
        intent.pointer(OUT, 100, Timing::default());
        let timer = intent.pending().unwrap();
        assert_eq!(timer.due_ms, REMIND_MS);
        assert_eq!(intent.fire(timer, REMIND_MS), Effect::Hide);

        let mut intent = Intent::default();
        intent.remind(0, REMIND_MS);
        let early = intent.pending().unwrap();
        intent.pointer(PEEK, 1000, Timing::default());
        assert_eq!(intent.fire(early, REMIND_MS), Effect::None);
        assert_eq!(intent.mode(), Mode::Hover);
        intent.pointer(OUT, 6000, Timing::default());
        let late = intent.pending().unwrap();
        assert_eq!(intent.fire(late, 6000 + CLOSE_DELAY_MS), Effect::Hide);
    }
    #[test]
    fn a_reminder_leaves_a_pinned_peek_alone() {
        let mut intent = Intent::default();
        intent.activate();
        assert_eq!(intent.remind(0, REMIND_MS), Effect::None);
        assert_eq!(intent.mode(), Mode::Pinned);
        assert!(intent.pending().is_none());
    }
    #[test]
    fn hover_then_immediate_click_pins() {
        let mut intent = hovered();
        intent.press(1);
        assert_eq!(intent.release(true), Effect::Show);
        assert_eq!(intent.mode(), Mode::Pinned);
    }
    #[test]
    fn hover_age_does_not_turn_first_click_into_close() {
        let mut intent = hovered();
        intent.pointer(ICON, 1_000_000, Timing::default());
        assert_eq!(intent.activate(), Effect::Show);
        assert_eq!(intent.mode(), Mode::Pinned);
    }
    #[test]
    fn click_closed_then_independent_click_closes() {
        let mut intent = Intent::default();
        assert_eq!(intent.activate(), Effect::Show);
        assert_eq!(intent.activate(), Effect::Hide);
        intent.pointer(OUT, 1, Timing::default());
    }
    #[test]
    fn press_before_open_deadline_freezes_intent_until_release_once() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let old = intent.pending().unwrap();
        assert_eq!(intent.fire(old, OPEN_DELAY_MS - 1), Effect::None);
        intent.press(10);
        assert!(intent.pending().is_none());
        assert_eq!(intent.fire(old, OPEN_DELAY_MS + 1), Effect::None);
        assert_eq!(intent.mode(), Mode::Closed);
        assert_eq!(intent.release(true), Effect::Show);
        assert_eq!(intent.mode(), Mode::Pinned);
        assert_eq!(intent.release(true), Effect::None);
        intent.press(10); // Duplicate down from the consumed gesture.
        assert_eq!(intent.release(true), Effect::None);
        intent.press(11);
        assert_eq!(intent.release(true), Effect::Hide);
    }

    #[test]
    fn duplicate_down_and_outside_observation_cannot_change_close_intent() {
        let mut intent = hovered();
        intent.activate();
        intent.press(5);
        assert_eq!(intent.outside_press(ICON), Effect::None);
        intent.press(5);
        intent.pointer(OUT, 1000, Timing::default());
        assert!(intent.pending().is_none());
        assert_eq!(intent.release(true), Effect::Hide);
        assert_eq!(intent.release(true), Effect::None);
    }

    #[test]
    fn explicit_dismiss_during_a_press_prevents_release_reopening() {
        for pinned in [false, true] {
            let mut intent = hovered();
            if pinned {
                intent.activate();
            }
            intent.press(1);
            intent.dismiss();
            assert_eq!(intent.release(true), Effect::None);
            intent.pointer(ICON, 1000, Timing::default());
            assert!(intent.pending().is_none());
            assert_eq!(intent.mode(), Mode::Closed);
        }
    }

    #[test]
    fn cancelled_drag_out_does_not_activate_and_next_gesture_is_independent() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        intent.press(1);
        intent.pointer(OUT, 20, Timing::default());
        assert_eq!(intent.release(false), Effect::None);
        assert_eq!(intent.mode(), Mode::Closed);
        intent.press(2);
        assert_eq!(intent.release(true), Effect::Show);
    }

    #[test]
    fn deadlines_have_exact_boundaries_and_union_crossing_keeps_hover_open() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 10, Timing::default());
        let open = intent.pending().unwrap();
        assert_eq!(intent.fire(open, open.due_ms - 1), Effect::None);
        assert_eq!(intent.fire(open, open.due_ms), Effect::Show);
        assert_eq!(intent.fire(open, open.due_ms + 1), Effect::None);
        intent.pointer(OUT, 400, Timing::default());
        let crossing = intent.pending().unwrap();
        intent.pointer(
            Pointer {
                icon: false,
                peek: true,
            },
            450,
            Timing::default(),
        );
        assert_eq!(intent.fire(crossing, 1000), Effect::None);
        assert_eq!(intent.mode(), Mode::Hover);
        intent.pointer(OUT, 1100, Timing::default());
        let close = intent.pending().unwrap();
        assert_eq!(intent.fire(close, close.due_ms - 1), Effect::None);
        assert_eq!(intent.fire(close, close.due_ms), Effect::Hide);
        assert_eq!(intent.fire(close, close.due_ms + 1), Effect::None);
    }

    #[test]
    fn pinned_ignores_leaving_and_stale_open_or_close_callbacks() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let open = intent.pending().unwrap();
        intent.activate();
        assert_eq!(intent.fire(open, 1000), Effect::None);
        intent.pointer(OUT, 1000, Timing::default());
        assert!(intent.pending().is_none());
        assert_eq!(intent.mode(), Mode::Pinned);
        let mut intent = hovered();
        intent.pointer(OUT, 1000, Timing::default());
        let close = intent.pending().unwrap();
        intent.activate();
        assert_eq!(intent.fire(close, 2000), Effect::None);
        assert_eq!(intent.mode(), Mode::Pinned);
    }

    #[test]
    fn explicit_close_requires_exit_reentry_and_quit_is_terminal() {
        let mut intent = hovered();
        intent.activate();
        intent.activate();
        intent.pointer(ICON, 1000, Timing::default());
        assert!(intent.pending().is_none());
        intent.pointer(OUT, 1010, Timing::default());
        intent.pointer(ICON, 1020, Timing::default());
        let open = intent.pending().unwrap();
        intent.stop();
        assert_eq!(intent.fire(open, 2000), Effect::None);
        assert_eq!(intent.activate(), Effect::None);
        assert_eq!(intent.pin(), Effect::None);
        assert!(intent.pending().is_none());
    }

    #[test]
    fn pause_or_explicit_window_close_invalidates_queued_hover() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let old = intent.pending().unwrap();
        intent.cancel_timer();
        assert_eq!(intent.fire(old, 1000), Effect::None);
        intent.pointer(ICON, 1100, Timing::default());
        let fresh = intent.pending().unwrap();
        intent.dismiss();
        assert_eq!(intent.fire(fresh, 2000), Effect::None);
    }
}
