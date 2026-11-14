//! Which spare displays to reuse, keep or retire so WindowServer unit numbers
//! stay contiguous from the bottom up.
//!
//! WindowServer assigns a new `CGVirtualDisplay` the unit number
//! `base + active_virtual_count` rather than filling a gap below an online
//! virtual display. Releasing a spare with unit 8 while unit 9 stays online
//! makes every later display creation collide with unit 9 ("unit twin"),
//! forcing a throwaway filler display and extra WindowServer reconfigurations.

/// Facts about one spare display, gathered on the caller's side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpareInfo {
    pub size: (f64, f64),
    pub unit: Option<u32>,
    pub expired: bool,
    pub has_windows: bool,
}

fn unit_key(unit: Option<u32>, index: usize) -> (u32, usize) {
    (unit.unwrap_or(u32::MAX), index)
}

/// Index of the spare to reuse for `wanted`: a matching size with the lowest
/// unit number, or when none matches `wanted`, the spare with the lowest unit
/// number overall so the caller can resize it.
pub fn pick_spare(spares: &[SpareInfo], wanted: (f64, f64)) -> Option<usize> {
    spares
        .iter()
        .enumerate()
        .filter(|(_, s)| s.size == wanted)
        .min_by_key(|&(i, s)| unit_key(s.unit, i))
        .or_else(|| {
            spares
                .iter()
                .enumerate()
                .min_by_key(|&(i, s)| unit_key(s.unit, i))
        })
        .map(|(i, _)| i)
}

/// When `spares` is full (`spares.len() >= max_spares`) and an empty display
/// with `incoming_unit` ends, return the index of a windowless spare with a
/// higher unit number to retire in its place so the lower unit stays in the
/// pool.
pub fn retire_instead(
    spares: &[SpareInfo],
    max_spares: usize,
    incoming_unit: Option<u32>,
) -> Option<usize> {
    if spares.len() < max_spares {
        return None;
    }
    let incoming = incoming_unit?;
    spares
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.has_windows && s.unit.is_none_or(|u| u > incoming))
        .max_by_key(|&(i, s)| unit_key(s.unit, i))
        .map(|(i, _)| i)
}

/// Which spares stay in the pool (`true`) vs release (`false`) during idle
/// tidy.
///
/// - Any spare that somehow has a window on it stays so windows are never
///   evacuated unexpectedly by background expiry.
/// - Unexpired spares stay.
/// - While `warm`, up to `ready` spares stay, preferring the lowest unit
///   numbers so higher units release from the top down.
/// - Within `max_spares`, an expired spare whose unit lies below an online
///   live display or kept spare stays rather than opening a unit gap below it.
pub fn keep_spares(
    spares: &[SpareInfo],
    warm: bool,
    ready: usize,
    max_spares: usize,
    live_units: &[u32],
) -> Vec<bool> {
    let mut keep: Vec<bool> = spares.iter().map(|s| !s.expired || s.has_windows).collect();
    let mut kept_count = keep.iter().filter(|&&k| k).count();

    let mut expired_low_first: Vec<usize> = (0..spares.len()).filter(|&i| !keep[i]).collect();
    expired_low_first.sort_by_key(|&i| unit_key(spares[i].unit, i));

    if warm {
        for &i in &expired_low_first {
            if kept_count >= ready {
                break;
            }
            keep[i] = true;
            kept_count += 1;
        }
    }

    // Recompute highest active unit as low-unit spares below it are retained.
    loop {
        if kept_count >= max_spares {
            break;
        }
        let highest = live_units
            .iter()
            .copied()
            .chain(
                spares
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| keep[i])
                    .filter_map(|(_, s)| s.unit),
            )
            .max();
        let Some(highest) = highest else {
            break;
        };
        let Some(&next) = expired_low_first
            .iter()
            .find(|&&i| !keep[i] && spares[i].unit.is_some_and(|u| u < highest))
        else {
            break;
        };
        keep[next] = true;
        kept_count += 1;
    }

    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: (f64, f64) = (1280.0, 800.0);
    const WIDE: (f64, f64) = (1728.0, 1117.0);

    #[test]
    fn pick_spare_prefers_matching_size_with_lowest_unit_then_lowest_unit_to_resize() {
        let spares = [
            SpareInfo {
                size: DEFAULT,
                unit: Some(9),
                expired: false,
                has_windows: false,
            },
            SpareInfo {
                size: DEFAULT,
                unit: Some(8),
                expired: false,
                has_windows: false,
            },
            SpareInfo {
                size: WIDE,
                unit: Some(7),
                expired: false,
                has_windows: false,
            },
        ];
        assert_eq!(pick_spare(&spares, DEFAULT), Some(1));
        assert_eq!(pick_spare(&spares, WIDE), Some(2));
        assert_eq!(pick_spare(&spares, (1024.0, 768.0)), Some(2));
    }

    #[test]
    fn tidy_keeps_lowest_units_and_never_opens_a_gap_below_a_live_display() {
        // Reproduction of displays.log unit-twin chain: spares at unit 8 and
        // unit 9 both aged past SPARE_LIFE while READY = 1; keeping unit 9 and
        // releasing unit 8 made every later create collide with unit 9.
        let spares = [
            SpareInfo {
                size: DEFAULT,
                unit: Some(8),
                expired: true,
                has_windows: false,
            },
            SpareInfo {
                size: DEFAULT,
                unit: Some(9),
                expired: true,
                has_windows: false,
            },
        ];
        assert_eq!(keep_spares(&spares, true, 1, 3, &[]), vec![true, false]);

        // Even when cold (warm = false), an expired spare at unit 8 stays
        // while a live display at unit 9 is online, whereas unit 10 above it
        // releases.
        let spares_around_live = [
            SpareInfo {
                size: DEFAULT,
                unit: Some(8),
                expired: true,
                has_windows: false,
            },
            SpareInfo {
                size: DEFAULT,
                unit: Some(10),
                expired: true,
                has_windows: false,
            },
        ];
        assert_eq!(
            keep_spares(&spares_around_live, false, 2, 3, &[9]),
            vec![true, false]
        );
    }

    #[test]
    fn a_spare_with_windows_is_never_released_nor_retired() {
        let spares = [
            SpareInfo {
                size: DEFAULT,
                unit: Some(10),
                expired: true,
                has_windows: true,
            },
            SpareInfo {
                size: DEFAULT,
                unit: Some(9),
                expired: true,
                has_windows: false,
            },
        ];
        // Unit 10 has a window, so it stays, which in turn keeps unit 9 below
        // it within max_spares = 3.
        assert_eq!(keep_spares(&spares, false, 0, 3, &[]), vec![true, true]);
        // Only unit 9 (windowless) can be retired for incoming unit 8 when
        // max_spares = 2.
        assert_eq!(retire_instead(&spares, 2, Some(8)), Some(1));
    }
}
