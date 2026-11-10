//! The display budget. Every new virtual display makes WindowServer and
//! ColorSync reconfigure, and about 60 in 11 minutes froze this Mac on
//! 2026-10-20. `suite` reads how many displays the test instance made from
//! its display log, whoever ran the scripts, and holds a script back until
//! the last ten minutes have room for the displays it makes. A run that
//! has made its cap stops.

/// The stretch of time the budget counts over, in seconds.
pub const WINDOW: i64 = 600;

/// How many displays the last ten minutes may hold.
pub const BUDGET: usize = 12;

/// How many displays one run of `suite` may make.
pub const CAP: usize = 60;

/// A display log line's time, in seconds: the line's local date and time
/// read as if it were UTC, which is all a difference of two needs.
pub fn stamp(line: &str) -> Option<i64> {
    let number = |from: usize, to: usize| line.get(from..to)?.parse::<i64>().ok();
    let bytes = line.as_bytes();
    let marks = [(4, b'-'), (7, b'-'), (10, b' '), (13, b':'), (16, b':')];
    if marks.iter().any(|&(at, mark)| bytes.get(at) != Some(&mark)) {
        return None;
    }
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since 1970-01-01, by Howard Hinnant's civil-date arithmetic.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// When the log says a display was made new: its `created display=` lines.
/// A reused spare adds no display and isn't one.
pub fn created(log: &str) -> Vec<i64> {
    log.lines()
        .filter(|line| line.contains(" created display="))
        .filter_map(stamp)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Displays the last `WINDOW` seconds may hold.
    pub budget: usize,
    /// Displays this run may make.
    pub cap: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            budget: BUDGET,
            cap: CAP,
        }
    }
}

/// Whether a script that makes `need` displays may start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Go,
    /// The last ten minutes are too full: there is room in `secs` seconds.
    Wait {
        secs: u64,
        recent: usize,
    },
    /// The script makes more displays than the budget ever has room for.
    TooMany,
    /// The run made `made` displays, and `need` more would pass its cap.
    Capped {
        made: usize,
    },
}

/// Decide at `now`, for a run that began at `began`, from the times
/// displays were made. A time after `now`, as a clock change leaves, counts
/// as just made.
pub fn decide(made: &[i64], now: i64, began: i64, need: usize, limits: Limits) -> Verdict {
    if need == 0 {
        return Verdict::Go;
    }
    let in_run = made.iter().filter(|&&t| t >= began).count();
    if in_run + need > limits.cap {
        return Verdict::Capped { made: in_run };
    }
    if need > limits.budget {
        return Verdict::TooMany;
    }
    let mut recent: Vec<i64> = made
        .iter()
        .map(|&t| t.min(now))
        .filter(|&t| now - t < WINDOW)
        .collect();
    if recent.len() + need <= limits.budget {
        return Verdict::Go;
    }
    // Room comes when enough of the oldest have left the window.
    recent.sort();
    let leaving = recent[recent.len() + need - limits.budget - 1];
    Verdict::Wait {
        secs: (leaving + WINDOW - now).max(1) as u64,
        recent: recent.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
2026-11-10 12:40:49.011 created display=303 unit=9 at=3008,-2234 size=1728x1117 serial=10002
2026-11-10 12:40:50.875 created display=304 unit=10 at=3008,-2834 size=900x600 serial=10003
2026-11-10 12:40:51.000 reused display=304 unit=10 at=3008,-2834 size=900x600
2026-11-10 12:41:07.393 released display=14 unit=11 at=8128,-600 size=900x600 quit
<unknown> created display=9
2026-11-10 12:41:07.393 app quit pid=82569
2026-11-10 12:50:49.000 created display=305 unit=9 at=0,0 size=900x600 serial=10002
";

    fn limits(budget: usize, cap: usize) -> Limits {
        Limits { budget, cap }
    }

    #[test]
    fn a_stamp_is_seconds() {
        assert_eq!(stamp("1970-01-01 00:00:00.000 x"), Some(0));
        assert_eq!(
            stamp("2026-11-10 12:40:49.011 created"),
            Some(1_794_314_449)
        );
        assert_eq!(
            stamp("2026-03-01 00:00:00").unwrap() - stamp("2026-02-28 23:59:59").unwrap(),
            1
        );
        assert_eq!(
            stamp("2028-03-01 00:00:00").unwrap() - stamp("2028-02-28 00:00:00").unwrap(),
            2 * 86_400
        );
        assert_eq!(stamp("<unknown> created display=9"), None);
        assert_eq!(stamp("2026-13-10 12:40:49"), None);
        assert_eq!(stamp(""), None);
    }

    #[test]
    fn only_displays_made_new_count() {
        let made = created(LOG);
        assert_eq!(made.len(), 3);
        assert_eq!(made[1] - made[0], 1);
        assert_eq!(made[2] - made[0], 600);
    }

    #[test]
    fn a_script_goes_while_the_window_has_room() {
        let now = 10_000;
        let made = [now - 700, now - 500, now - 10];
        assert_eq!(decide(&made, now, now, 2, limits(4, 60)), Verdict::Go);
        assert_eq!(decide(&[], now, now, 12, Limits::default()), Verdict::Go);
        // A script that makes none never waits.
        assert_eq!(decide(&[now; 50], now, 0, 0, limits(4, 5)), Verdict::Go);
    }

    #[test]
    fn a_full_window_waits_until_the_oldest_leave() {
        let now = 10_000;
        let made = [now - 700, now - 500, now - 300, now - 10];
        // Three in the window, budget 4, two more wanted: the one made 500 s
        // ago must leave, in 100 s.
        assert_eq!(
            decide(&made, now, now, 2, limits(4, 60)),
            Verdict::Wait {
                secs: 100,
                recent: 3
            }
        );
        // Three wanted: the one made 300 s ago must leave too.
        assert_eq!(
            decide(&made, now, now, 3, limits(4, 60)),
            Verdict::Wait {
                secs: 300,
                recent: 3
            }
        );
        // After the wait there is room.
        assert_eq!(decide(&made, now + 100, now, 2, limits(4, 60)), Verdict::Go);
    }

    #[test]
    fn a_time_ahead_of_the_clock_counts_as_just_made() {
        let now = 10_000;
        assert_eq!(
            decide(&[now + 3600], now, now, 1, limits(1, 60)),
            Verdict::Wait {
                secs: 600,
                recent: 1
            }
        );
    }

    #[test]
    fn a_script_larger_than_the_budget_never_runs() {
        assert_eq!(decide(&[], 0, 0, 13, Limits::default()), Verdict::TooMany);
    }

    #[test]
    fn the_run_stops_at_its_cap() {
        let began = 10_000;
        let now = began + 3000;
        // Two made before the run don't count against it; four since do.
        let made = [
            began - 50,
            began - 20,
            began + 10,
            began + 900,
            began + 1800,
            began + 2700,
        ];
        assert_eq!(decide(&made, now, began, 1, limits(12, 5)), Verdict::Go);
        assert_eq!(
            decide(&made, now, began, 2, limits(12, 5)),
            Verdict::Capped { made: 4 }
        );
        // The cap comes before the wait: a capped run never sleeps first.
        assert_eq!(
            decide(&made, now, began, 2, limits(1, 5)),
            Verdict::Capped { made: 4 }
        );
    }
}
