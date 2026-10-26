//! Limits on adding virtual displays. Each display added is a
//! reconfiguration every app on the Mac handles, and while it is connected
//! ColorSync answers more requests (ADR 0009). Two rules keep that in
//! bounds: agents get `limit` new displays per `window`, and no display is
//! added once WindowServer's color request would take longer than
//! `BUDGET_MS`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The longest an answer to WindowServer's color request may take, against
/// its 50 ms limit (ADR 0009).
pub const BUDGET_MS: f64 = 40.0;

/// A sliding count of displays made new.
#[derive(Debug)]
pub struct MadeLimit {
    made: VecDeque<Instant>,
    limit: usize,
    window: Duration,
}

impl MadeLimit {
    pub fn new(limit: usize, window: Duration) -> Self {
        MadeLimit {
            made: VecDeque::new(),
            limit,
            window,
        }
    }

    /// Whether one more display may be made at `now`; otherwise how long
    /// until one may. `limit` overrides the limit given at `new`.
    pub fn allows(&mut self, now: Instant, limit: Option<usize>) -> Result<(), Duration> {
        while self
            .made
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > self.window)
        {
            self.made.pop_front();
        }
        if self.made.len() < limit.unwrap_or(self.limit) {
            return Ok(());
        }
        let oldest = self.made.front().copied().unwrap_or(now);
        Err(self
            .window
            .saturating_sub(now.saturating_duration_since(oldest)))
    }

    /// A display was made new at `now`.
    pub fn record(&mut self, now: Instant) {
        self.made.push_back(now);
    }
}

/// What ColorSync keeps, as the last helper run or count found it.
#[derive(Clone, Copy, Debug)]
pub struct ColorState {
    /// Display records ColorSync keeps; `None` till first counted.
    pub records: Option<usize>,
    /// The helper is set up, which makes each record cheaper to look up.
    pub helper_ready: bool,
}

impl ColorState {
    /// How long WindowServer's color request takes with `online` displays,
    /// in ms.
    pub fn request_ms(&self, online: usize) -> f64 {
        let per_record = if self.helper_ready { 0.05 } else { 0.2 };
        let records = self.records.unwrap_or(0) as f64;
        online as f64 * (1.0 + records * per_record)
    }

    /// Whether a display may be added to `online` displays, `spares` of
    /// them spares that can be released.
    pub fn allows(&self, online: usize, spares: usize) -> Budget {
        // Till counted, no guess refuses a display.
        if self.records.is_none() {
            return Budget::Allow;
        }
        let with_one_more = online + 1;
        if self.request_ms(with_one_more) <= BUDGET_MS {
            return Budget::Allow;
        }
        if spares > 0 && self.request_ms(with_one_more.saturating_sub(spares)) <= BUDGET_MS {
            return Budget::ReleaseSpares;
        }
        Budget::Refuse
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Budget {
    Allow,
    /// Allowed once every spare is released.
    ReleaseSpares,
    Refuse,
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(600);

    #[test]
    fn made_limit_allows_up_to_the_limit() {
        let start = Instant::now();
        let mut limit = MadeLimit::new(3, WINDOW);
        for i in 0..3 {
            let at = start + Duration::from_secs(i);
            assert_eq!(limit.allows(at, None), Ok(()));
            limit.record(at);
        }
        let at = start + Duration::from_secs(100);
        assert_eq!(limit.allows(at, None), Err(Duration::from_secs(500)));
    }

    #[test]
    fn made_limit_frees_a_slot_once_the_oldest_leaves_the_window() {
        let start = Instant::now();
        let mut limit = MadeLimit::new(2, WINDOW);
        limit.record(start);
        limit.record(start + Duration::from_secs(300));
        assert!(
            limit
                .allows(start + Duration::from_secs(600), None)
                .is_err()
        );
        assert_eq!(limit.allows(start + Duration::from_secs(601), None), Ok(()));
        limit.record(start + Duration::from_secs(601));
        assert_eq!(
            limit.allows(start + Duration::from_secs(700), None),
            Err(Duration::from_secs(200))
        );
    }

    #[test]
    fn made_limit_takes_an_override() {
        let start = Instant::now();
        let mut limit = MadeLimit::new(1, WINDOW);
        limit.record(start);
        assert!(limit.allows(start, None).is_err());
        assert_eq!(limit.allows(start, Some(12)), Ok(()));
    }

    #[test]
    fn color_budget_allows_before_the_first_count() {
        let state = ColorState {
            records: None,
            helper_ready: false,
        };
        assert_eq!(state.allows(100, 0), Budget::Allow);
    }

    #[test]
    fn color_budget_allows_while_in_time() {
        // 4 displays, 10 records, no helper: 4 * (1 + 2) = 12 ms.
        let state = ColorState {
            records: Some(10),
            helper_ready: false,
        };
        assert_eq!(state.request_ms(4), 12.0);
        assert_eq!(state.allows(3, 0), Budget::Allow);
    }

    #[test]
    fn color_budget_releases_spares_when_that_is_enough() {
        // 100 records, no helper: 21 ms a display, so 1 fits and 2 don't.
        let state = ColorState {
            records: Some(100),
            helper_ready: false,
        };
        assert_eq!(state.allows(1, 0), Budget::Refuse);
        assert_eq!(state.allows(1, 1), Budget::ReleaseSpares);
    }

    #[test]
    fn color_budget_refuses_when_spares_are_not_enough() {
        let state = ColorState {
            records: Some(100),
            helper_ready: false,
        };
        assert_eq!(state.allows(3, 1), Budget::Refuse);
    }

    #[test]
    fn helper_makes_records_cheaper() {
        // 100 records: 21 ms a display without the helper, 6 ms with it.
        let without = ColorState {
            records: Some(100),
            helper_ready: false,
        };
        let with = ColorState {
            helper_ready: true,
            ..without
        };
        assert_eq!(without.allows(2, 0), Budget::Refuse);
        assert_eq!(with.allows(2, 0), Budget::Allow);
    }
}
