//! Take Backs whose return catch is still registering. Registration asks
//! the app and finishes later, so it follows the latest Take Back or
//! Handover, not the order in which registrations finish.

use crate::model::DisplayId;
use std::collections::HashMap;

/// One Take Back of an app's window on a display, to hand back when its
/// return catch has registered.
#[derive(Clone, Copy, Debug)]
pub struct ReturnTicket {
    pub display: DisplayId,
    pub pid: i32,
    serial: u64,
}

#[derive(Default)]
pub struct Returns {
    pending: HashMap<(DisplayId, i32), u64>,
    serial: u64,
}

impl Returns {
    /// A Take Back replaces any earlier one of the app on the display.
    pub fn take_back(&mut self, display: DisplayId, pid: i32) -> ReturnTicket {
        self.serial += 1;
        self.pending.insert((display, pid), self.serial);
        ReturnTicket {
            display,
            pid,
            serial: self.serial,
        }
    }

    /// A Handover makes the app's later windows the agent's again.
    pub fn hand_over(&mut self, display: DisplayId, pid: i32) {
        self.pending.remove(&(display, pid));
    }

    pub fn remove_display(&mut self, display: DisplayId) {
        self.pending.retain(|(d, _), _| *d != display);
    }

    /// Whether this registration is still wanted, consumed once. An
    /// obsolete one leaves the latest Take Back pending.
    pub fn complete(&mut self, ticket: ReturnTicket) -> bool {
        let key = (ticket.display, ticket.pid);
        if self.pending.get(&key) != Some(&ticket.serial) {
            return false;
        }
        self.pending.remove(&key);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handover_cancels_a_pending_take_back() {
        let mut returns = Returns::default();
        let a = returns.take_back(7, 42);
        returns.hand_over(7, 42);
        assert!(!returns.complete(a));
    }

    #[test]
    fn old_registration_does_not_consume_a_later_take_back() {
        let mut returns = Returns::default();
        let a = returns.take_back(7, 42);
        returns.hand_over(7, 42);
        let b = returns.take_back(7, 42);
        assert!(!returns.complete(a), "A is obsolete, not B's registration");
        assert!(returns.complete(b), "B must still be pending");
        assert!(!returns.complete(b), "a registration is consumed once");
    }

    #[test]
    fn later_registration_finishing_first_also_wins() {
        let mut returns = Returns::default();
        let a = returns.take_back(7, 42);
        returns.hand_over(7, 42);
        let b = returns.take_back(7, 42);
        assert!(returns.complete(b));
        assert!(!returns.complete(a));
    }

    #[test]
    fn a_removed_display_cancels_its_take_backs_only() {
        let mut returns = Returns::default();
        let old = returns.take_back(7, 42);
        let other = returns.take_back(8, 42);
        returns.remove_display(7);
        let new = returns.take_back(7, 42);
        assert!(!returns.complete(old));
        assert!(returns.complete(other));
        assert!(returns.complete(new));
    }
}
