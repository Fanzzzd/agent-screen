//! Retries that wait longer after each failure instead of giving up.
//!
//! Re-parking a display and giving it its mode back run when the displays
//! change. WindowServer ignores configurations while it is still
//! reconfiguring, so a burst of changes, as when a dock is unplugged, can
//! fail several tries in a row. A cap on tries left a display where macOS
//! put it, maybe next to the user's screen, until another change happened
//! to fix it (#286). A `Backoff` keeps trying, later each time, up to
//! `MAX_DELAY_MS` apart. Each try moves displays, which every app on the
//! Mac handles, so one that keeps failing tries only every few minutes.

use std::collections::HashMap;
use std::hash::Hash;

/// The wait after the first failure; it doubles after each next one.
pub const FIRST_DELAY_MS: u64 = 500;
/// The longest wait between two tries.
pub const MAX_DELAY_MS: u64 = 300_000;

#[derive(Clone, Debug)]
pub struct Backoff<K> {
    /// Failures in a row and when the next try is due, by key.
    failed: HashMap<K, (u32, u64)>,
}

impl<K> Default for Backoff<K> {
    fn default() -> Self {
        Self {
            failed: HashMap::new(),
        }
    }
}

/// The wait after `failures` failures in a row.
pub fn delay_ms(failures: u32) -> u64 {
    let doublings = failures.saturating_sub(1).min(16);
    (FIRST_DELAY_MS << doublings).min(MAX_DELAY_MS)
}

impl<K: Eq + Hash + Clone> Backoff<K> {
    /// Whether `key` may be tried at `now_ms`: it never failed, or its wait
    /// is over.
    pub fn due(&self, key: &K, now_ms: u64) -> bool {
        self.failed.get(key).is_none_or(|&(_, at)| now_ms >= at)
    }

    /// A try of `key` failed at `now_ms`.
    pub fn failed(&mut self, key: K, now_ms: u64) {
        let entry = self.failed.entry(key).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = now_ms + delay_ms(entry.0);
    }

    /// A try of `key` worked, or there is nothing to fix: the next failure
    /// starts from the first wait again.
    pub fn succeeded(&mut self, key: &K) {
        self.failed.remove(key);
    }

    /// Failures in a row of `key`.
    pub fn failures(&self, key: &K) -> u32 {
        self.failed.get(key).map_or(0, |&(n, _)| n)
    }

    /// Keep only the keys `keep` says still exist, as after displays went.
    pub fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.failed.retain(|k, _| keep(k));
    }

    /// How long after `now_ms` the soonest waiting retry is due; None when
    /// nothing waits.
    pub fn next_in(&self, now_ms: u64) -> Option<u64> {
        self.failed
            .values()
            .map(|&(_, at)| at.saturating_sub(now_ms))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_up_to_the_most() {
        assert_eq!(delay_ms(1), 500);
        assert_eq!(delay_ms(2), 1_000);
        assert_eq!(delay_ms(4), 4_000);
        assert_eq!(delay_ms(7), 32_000);
        assert_eq!(delay_ms(10), 256_000);
        assert_eq!(delay_ms(11), 300_000);
        assert_eq!(delay_ms(u32::MAX), 300_000);
    }

    /// Three failures in a row used to stop the retries for good.
    #[test]
    fn a_display_is_tried_again_after_any_number_of_failures() {
        let mut b = Backoff::default();
        let mut now = 0;
        for failures in 1..=20 {
            assert!(b.due(&7, now), "try {failures} is due");
            b.failed(7, now);
            assert!(!b.due(&7, now), "not again at once");
            let wait = b.next_in(now).unwrap();
            assert_eq!(wait, delay_ms(failures));
            now += wait;
        }
        assert_eq!(b.failures(&7), 20);
        assert!(b.due(&7, now));
    }

    #[test]
    fn success_starts_the_waits_over_and_other_keys_are_apart() {
        let mut b = Backoff::default();
        b.failed(7, 0);
        b.failed(7, 500);
        assert!(b.due(&8, 0));
        b.succeeded(&7);
        assert!(b.due(&7, 0));
        assert_eq!(b.next_in(0), None);
        b.failed(7, 0);
        assert_eq!(b.next_in(0), Some(500));
        b.retain(|&k| k != 7);
        assert_eq!(b.failures(&7), 0);
    }
}
