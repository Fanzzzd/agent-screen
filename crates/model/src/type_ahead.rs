//! Pacing of type-ahead into a closed pop-up button (#219). AppKit's type
//! select (CarbonCore's `UCTypeSelect`) and a Chromium select both end a
//! search about a second after its last key, measured between the keys'
//! event times, and Agent Screen stamps each key as it goes out. Between
//! two keys the app gets a moment to take the last one (the settle), then
//! each key is checked against the window it would reach. Under load the
//! window list behind that check alone took 300 ms (as-xnq), so the settle
//! gives way to it: it ends early when what is left before the limit would
//! no longer cover the slowest check seen so far. A key that would still go
//! out past the limit is not sent.
//!
//! Times are readings of one monotonic clock, as durations since any fixed
//! start, so tests can drive the pacing with a clock of their own.

use std::time::Duration;

/// The longest gap between two keys of a title, from one key going out to
/// the next. `UCTypeSelect` keeps a search going 1 s after a key and ends it
/// by 1.2 s, and Blink's type-ahead ends it 1 s after; the rest of the
/// second covers the moment between a key's check and its post.
pub const KEY_GAP: Duration = Duration::from_millis(700);

/// The most a settle waits: for the pop-up to show another item, then a
/// little longer for what the new choice triggers.
pub const SETTLE: Duration = Duration::from_millis(170);

/// What a settle leaves before the slowest check, for its last poll step
/// running over.
const MARGIN: Duration = Duration::from_millis(20);

/// One title's keys, from the first key's check to the last key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pacer {
    /// When the last key went out.
    last_key: Option<Duration>,
    /// The longest a check before a key has taken, from the end of the
    /// settle to the key.
    slowest_check: Duration,
}

impl Pacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// When the settle after the last key must end, read at `now` as it
    /// starts: `SETTLE` from now, or earlier so the slowest check seen so
    /// far still fits before `KEY_GAP`, with a margin. `now` itself when no
    /// time is left, or before the first key.
    pub fn settle_until(&self, now: Duration) -> Duration {
        let Some(last) = self.last_key else {
            return now;
        };
        let check_by = (last + KEY_GAP).saturating_sub(self.slowest_check + MARGIN);
        (now + SETTLE).min(check_by).max(now)
    }

    /// The check before a key, started at `from`, ended at `now`.
    pub fn checked(&mut self, from: Duration, now: Duration) {
        self.slowest_check = self.slowest_check.max(now.saturating_sub(from));
    }

    /// How long after the last key a key at `now` would go out. None for the
    /// first key.
    pub fn gap(&self, now: Duration) -> Option<Duration> {
        self.last_key.map(|last| now.saturating_sub(last))
    }

    /// Whether a key at `now` would go out too late to continue the search.
    pub fn too_late(&self, now: Duration) -> bool {
        self.gap(now).is_some_and(|gap| gap >= KEY_GAP)
    }

    /// A key went out at `now`.
    pub fn sent(&mut self, now: Duration) {
        self.last_key = Some(now);
    }

    /// The longest check seen so far.
    pub fn slowest_check(&self) -> Duration {
        self.slowest_check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_first_key_has_no_gap_and_no_settle() {
        let pacer = Pacer::new();
        assert_eq!(pacer.settle_until(ms(50)), ms(50));
        assert_eq!(pacer.gap(ms(50)), None);
        assert!(!pacer.too_late(ms(5_000)));
    }

    #[test]
    fn a_quick_title_settles_in_full() {
        let mut pacer = Pacer::new();
        pacer.checked(ms(0), ms(20));
        pacer.sent(ms(20));
        assert_eq!(pacer.settle_until(ms(24)), ms(24) + SETTLE);
    }

    #[test]
    fn the_settle_gives_way_to_the_slowest_check() {
        let mut pacer = Pacer::new();
        pacer.checked(ms(0), ms(600));
        pacer.sent(ms(600));
        // 700 ms after the key, less the 600 ms check and the margin.
        assert_eq!(pacer.settle_until(ms(604)), ms(680));
        // Already past that: no settle at all.
        assert_eq!(pacer.settle_until(ms(800)), ms(800));
    }

    #[test]
    fn a_key_at_the_limit_is_too_late() {
        let mut pacer = Pacer::new();
        pacer.sent(ms(1_000));
        assert!(!pacer.too_late(ms(1_699)));
        assert!(pacer.too_late(ms(1_700)));
        assert_eq!(pacer.gap(ms(1_365)), Some(ms(365)));
    }

    /// One key's costs on a given machine: how long the pop-up takes to
    /// show the key's item, a settle poll's step, the check before a key,
    /// and the pause after a post.
    #[derive(Clone, Copy)]
    struct Costs {
        shows: u64,
        poll: u64,
        check: u64,
        after_post: u64,
    }

    /// Type `keys` keys with `costs` on a clock `slow` times slower, as
    /// `choose_closed_popup` does: settle, check, then send unless too
    /// late. The settle polls in steps until the item shows, then waits
    /// out 80 ms, never past its deadline. The number of keys sent, and
    /// each gap a key went out with.
    fn type_keys(keys: usize, costs: Costs, slow: u64) -> (usize, Vec<Duration>) {
        let mut now = ms(0);
        let mut pacer = Pacer::new();
        let mut gaps = Vec::new();
        for typed in 0..keys {
            if typed > 0 {
                let until = pacer.settle_until(now);
                let shown_at = now + ms(costs.shows * slow);
                let mut settled = false;
                while now < until && !settled {
                    // A poll sleeps its step, then reads the pop-up; a busy
                    // Mac oversleeps it.
                    now += ms(costs.poll * slow);
                    settled = now >= shown_at;
                }
                if settled {
                    now = until.min(now + ms(80 * slow));
                }
            }
            let from = now;
            now += ms(costs.check * slow);
            pacer.checked(from, now);
            if let Some(gap) = pacer.gap(now) {
                gaps.push(gap);
            }
            if pacer.too_late(now) {
                return (typed, gaps);
            }
            pacer.sent(now);
            now += ms(costs.after_post * slow);
        }
        (keys, gaps)
    }

    /// The costs measured on the Mac at a load average of 40 to 60 (as-xnq):
    /// the item showed within 45 ms, and the check, mostly the window list,
    /// took up to 135 ms.
    const LOADED: Costs = Costs {
        shows: 45,
        poll: 15,
        check: 135,
        after_post: 4,
    };

    #[test]
    fn a_loaded_mac_types_the_whole_title() {
        let (sent, gaps) = type_keys(11, LOADED, 1);
        assert_eq!(sent, 11);
        assert!(gaps.iter().all(|&g| g < KEY_GAP), "{gaps:?}");
    }

    #[test]
    fn a_three_times_slower_clock_still_types_the_whole_title() {
        let (sent, gaps) = type_keys(11, LOADED, 3);
        assert_eq!(sent, 11, "{gaps:?}");
        assert!(gaps.iter().all(|&g| g < KEY_GAP), "{gaps:?}");
        // The 300 ms limit this replaced would have stopped at the second key.
        assert!(gaps.iter().all(|&g| g > ms(300)), "{gaps:?}");
    }

    #[test]
    fn a_check_near_the_limit_cuts_the_settle_and_still_types() {
        // Twice the 270 ms check: 540 ms, so a full settle would not fit.
        let slow_check = Costs {
            check: 270,
            ..LOADED
        };
        let (sent, gaps) = type_keys(11, slow_check, 2);
        assert_eq!(sent, 11, "{gaps:?}");
        assert!(gaps.iter().all(|&g| g < KEY_GAP), "{gaps:?}");
        assert!(gaps.iter().all(|&g| g < ms(540) + SETTLE), "{gaps:?}");
    }

    #[test]
    fn a_check_slower_than_the_limit_stops_after_the_first_key() {
        let stalled = Costs {
            check: 400,
            ..LOADED
        };
        let (sent, gaps) = type_keys(11, stalled, 2);
        assert_eq!(sent, 1);
        assert!(gaps.iter().all(|&g| g >= KEY_GAP), "{gaps:?}");
    }
}
