//! Input ordering for an in-call menu focus guard. The OS adapter owns
//! taps, events and focus checks. This policy alone never authorizes opening
//! a menu. ADR 0011 records the runtime requirements, still to be implemented.

use std::collections::VecDeque;
use std::time::Duration;

pub const DEADLINE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key,
    Modifiers,
    Button,
    Scroll,
    PointerMove,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Hardware,
    Posted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Opening,
    FocusReleased,
    Draining,
    Done,
}

/// A replay is complete only when the adapter sees it return through its
/// tap. Posting it is not completion: live input could otherwise overtake it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayId(u64);

pub struct Queue<T> {
    phase: Phase,
    held: VecDeque<(Event, T)>,
    pending: VecDeque<(ReplayId, T)>,
    failure: Option<CancelReason>,
    next: u64,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            phase: Phase::Opening,
            held: VecDeque::new(),
            pending: VecDeque::new(),
            failure: None,
            next: 0,
        }
    }
}

impl<T> Queue<T> {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn should_hold(&self, event: Event, origin: Origin) -> bool {
        if origin == Origin::Posted || matches!(event, Event::PointerMove | Event::Other) {
            return false;
        }
        if self.failure.is_some() {
            return true;
        }
        match self.phase {
            Phase::Opening | Phase::Draining => true,
            Phase::FocusReleased => {
                !self.held.is_empty()
                    || !self.pending.is_empty()
                    || matches!(event, Event::Button | Event::Scroll)
            }
            Phase::Done => false,
        }
    }

    /// Return the event to pass it through. None means this queue owns it.
    /// The adapter must allocate its retained event copy before calling this.
    pub fn offer(&mut self, kind: Event, origin: Origin, event: T) -> Option<T> {
        if self.should_hold(kind, origin) {
            self.held.push_back((kind, event));
            None
        } else {
            Some(event)
        }
    }

    /// Call only after verifying that key focus returned to the user.
    pub fn focus_released(&mut self) {
        if self.phase == Phase::Opening {
            self.phase = Phase::FocusReleased;
        }
    }

    /// Both observations are required. A deadline alone permits no replay.
    pub fn confirm_closed(&mut self, menu_closed: bool, focus_returned: bool) -> bool {
        if !menu_closed || !focus_returned {
            return false;
        }
        if self.phase != Phase::Done {
            self.phase = Phase::Draining;
        }
        self.complete_if_drained();
        true
    }

    /// Inspect without transferring custody. The adapter can refuse an
    /// unsafe focus transition before it creates an in-flight replay.
    pub fn replay_front(&self) -> Option<&T> {
        if self.failure.is_some() {
            return None;
        }
        let (kind, payload) = self.held.front()?;
        match self.phase {
            Phase::Opening | Phase::Done => None,
            Phase::FocusReleased if matches!(kind, Event::Button | Event::Scroll) => None,
            _ => Some(payload),
        }
    }

    /// Keys may replay once focus returns, until a mouse event precedes them.
    /// Mouse events wait for menu closure. All held event kinds share this FIFO.
    pub fn next_replay(&mut self) -> Option<(ReplayId, T)>
    where
        T: Clone,
    {
        self.replay_front()?;
        // Retain the original until acknowledgement. Allocate and clone
        // before removing it, so a panic cannot orphan a consumed event.
        self.pending.reserve(1);
        let copy = self.held.front()?.1.clone();
        let (_, original) = self.held.pop_front()?;
        let id = ReplayId(self.next);
        self.next = self.next.wrapping_add(1);
        self.pending.push_back((id, original));
        Some((id, copy))
    }

    /// A replay marker must also be authenticated by the adapter's poster
    /// pid and a per-transaction nonce. Reject out-of-order acknowledgements.
    pub fn acknowledge(&mut self, id: ReplayId) -> bool {
        if self.pending.front().map(|(pending, _)| *pending) != Some(id) {
            return false;
        }
        self.pending.pop_front();
        self.complete_if_drained();
        true
    }

    /// The guard keeps custody while cleanup is incomplete. Posting a copy
    /// does not transfer custody to the OS until an authenticated ACK arrives.
    pub fn retained(&self) -> usize {
        self.held.len() + self.pending.len()
    }

    /// Pending originals precede held originals in the recovery ledger.
    pub fn retained_events(&self) -> impl Iterator<Item = &T> {
        self.pending
            .iter()
            .map(|(_, value)| value)
            .chain(self.held.iter().map(|(_, value)| value))
    }

    pub fn pending(&self) -> impl Iterator<Item = (ReplayId, &T)> {
        self.pending.iter().map(|(id, value)| (*id, value))
    }

    pub fn fail(&mut self, reason: CancelReason) {
        if self.phase != Phase::Done && self.failure.is_none() {
            self.failure = Some(reason);
        }
    }

    pub fn failure(&self) -> Option<CancelReason> {
        self.failure
    }

    /// A failure never authorizes a timeout-based flush. The adapter must
    /// independently verify menu closure, focus return and restored taps.
    pub fn resume_after_failure(
        &mut self,
        menu_closed: bool,
        focus_returned: bool,
        secure_input: bool,
        taps_ready: bool,
    ) -> bool {
        if self.failure.is_none() || !menu_closed || !focus_returned || secure_input || !taps_ready
        {
            return false;
        }
        self.failure = None;
        self.confirm_closed(true, true)
    }

    fn complete_if_drained(&mut self) {
        if self.failure.is_none()
            && self.phase == Phase::Draining
            && self.held.is_empty()
            && self.pending.is_empty()
        {
            self.phase = Phase::Done;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    SecureInput,
    TapUnavailable,
    Deadline,
    OwnerGone,
    Panic,
}

/// Reserve time for cancellation. Expiry means close first, never discard
/// held input or replay into an open menu. The runtime must enforce this on
/// a watchdog independent of AX calls that can block.
pub fn must_close(
    elapsed: Duration,
    close_budget: Duration,
    secure_input: bool,
    taps_ready: bool,
) -> Option<CancelReason> {
    if secure_input {
        Some(CancelReason::SecureInput)
    } else if !taps_ready {
        Some(CancelReason::TapUnavailable)
    } else if elapsed.saturating_add(close_budget) >= DEADLINE {
        Some(CancelReason::Deadline)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posted_payload_remains_owned_until_acknowledgement() {
        let mut q = Queue::default();
        let _ = q.offer(Event::Key, Origin::Hardware, String::from("key"));
        q.focus_released();
        let (id, copy) = q.next_replay().unwrap();
        drop(copy);
        assert_eq!(q.retained(), 1);
        assert_eq!(q.pending().next().unwrap().1, "key");
        assert!(q.acknowledge(id));
        assert_eq!(q.retained(), 0);
    }

    #[test]
    fn failures_suspend_replay_without_losing_the_queue() {
        for reason in [
            CancelReason::SecureInput,
            CancelReason::TapUnavailable,
            CancelReason::Deadline,
            CancelReason::OwnerGone,
            CancelReason::Panic,
        ] {
            let mut q = Queue::default();
            let _ = q.offer(Event::Key, Origin::Hardware, 1);
            q.focus_released();
            q.fail(reason);
            assert_eq!(q.next_replay(), None);
            assert_eq!(q.retained(), 1);
            assert!(!q.resume_after_failure(false, true, false, true));
            assert!(!q.resume_after_failure(true, false, false, true));
            assert!(!q.resume_after_failure(true, true, true, true));
            assert!(!q.resume_after_failure(true, true, false, false));
            assert_eq!(q.retained(), 1);
            assert!(q.resume_after_failure(true, true, false, true));
            let (id, value) = q.next_replay().unwrap();
            assert_eq!(value, 1);
            assert!(q.acknowledge(id));
            assert_eq!(q.phase(), Phase::Done);
        }
    }

    #[test]
    fn a_clone_panic_does_not_remove_the_original() {
        struct Fault;
        impl Clone for Fault {
            fn clone(&self) -> Self {
                panic!("clone fault")
            }
        }
        let mut q = Queue::default();
        let _ = q.offer(Event::Key, Origin::Hardware, Fault);
        q.focus_released();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| q.next_replay()));
        assert!(result.is_err());
        assert_eq!(q.retained(), 1);
    }

    #[test]
    fn only_hardware_keys_modifiers_buttons_and_scrolls_wait() {
        let q = Queue::<u8>::default();
        for event in [Event::Key, Event::Modifiers, Event::Button, Event::Scroll] {
            assert!(q.should_hold(event, Origin::Hardware));
            assert!(!q.should_hold(event, Origin::Posted));
        }
        for event in [Event::PointerMove, Event::Other] {
            assert!(!q.should_hold(event, Origin::Hardware));
        }
    }

    #[test]
    fn opening_holds_input_until_focus_is_verified() {
        let mut q = Queue::default();
        assert_eq!(q.offer(Event::Key, Origin::Hardware, "down"), None);
        assert_eq!(q.next_replay(), None);
        q.focus_released();
        let (id, event) = q.next_replay().unwrap();
        assert_eq!(event, "down");
        assert!(q.acknowledge(id));
        assert_eq!(q.offer(Event::Key, Origin::Hardware, "up"), Some("up"));
        assert_eq!(q.offer(Event::Scroll, Origin::Hardware, "scroll"), None);
    }

    #[test]
    fn mouse_blocks_later_keys_until_menu_closes() {
        let mut q = Queue::default();
        q.focus_released();
        let _ = q.offer(Event::Button, Origin::Hardware, 1);
        let _ = q.offer(Event::Key, Origin::Hardware, 2);
        assert_eq!(q.next_replay(), None);
        assert!(!q.confirm_closed(true, false));
        assert!(!q.confirm_closed(false, true));
        assert_eq!(q.next_replay(), None);
        assert!(q.confirm_closed(true, true));
        let (a, event) = q.next_replay().unwrap();
        assert_eq!(event, 1);
        let (b, event) = q.next_replay().unwrap();
        assert_eq!(event, 2);
        assert!(!q.acknowledge(b));
        assert!(q.acknowledge(a));
        assert!(q.acknowledge(b));
        assert_eq!(q.phase(), Phase::Done);
    }

    #[test]
    fn newly_arriving_hardware_cannot_overtake_a_posted_replay() {
        let mut q = Queue::default();
        let _ = q.offer(Event::Key, Origin::Hardware, "down");
        q.focus_released();
        let (down, _) = q.next_replay().unwrap();
        assert_eq!(q.offer(Event::Key, Origin::Hardware, "up"), None);
        q.confirm_closed(true, true);
        let (up, _) = q.next_replay().unwrap();
        assert_eq!(q.next_replay(), None);
        assert_eq!(q.phase(), Phase::Draining);
        assert!(q.acknowledge(down));
        assert_eq!(q.phase(), Phase::Draining);
        assert!(q.acknowledge(up));
        assert_eq!(q.phase(), Phase::Done);
        assert_eq!(q.offer(Event::Key, Origin::Hardware, "next"), Some("next"));
    }

    #[test]
    fn moves_and_tools_pass_even_while_draining() {
        let mut q = Queue::default();
        let _ = q.offer(Event::Button, Origin::Hardware, 1);
        q.confirm_closed(true, true);
        assert_eq!(q.offer(Event::PointerMove, Origin::Hardware, 2), Some(2));
        assert_eq!(q.offer(Event::Key, Origin::Posted, 3), Some(3));
        assert_eq!(q.next_replay().unwrap().1, 1);
        assert_eq!(q.next_replay(), None);
    }

    #[test]
    fn deadline_reserves_cancellation_time_and_does_not_release_the_queue() {
        let reserve = Duration::from_millis(100);
        assert_eq!(
            must_close(Duration::from_millis(399), reserve, false, true),
            None
        );
        assert_eq!(
            must_close(Duration::from_millis(400), reserve, false, true),
            Some(CancelReason::Deadline)
        );
        assert_eq!(
            must_close(Duration::MAX, reserve, false, true),
            Some(CancelReason::Deadline)
        );
        assert_eq!(
            must_close(Duration::ZERO, reserve, true, true),
            Some(CancelReason::SecureInput)
        );
        assert_eq!(
            must_close(Duration::ZERO, reserve, false, false),
            Some(CancelReason::TapUnavailable)
        );
        let mut q = Queue::default();
        let _ = q.offer(Event::Key, Origin::Hardware, 1);
        assert_eq!(q.next_replay(), None);
    }
}
