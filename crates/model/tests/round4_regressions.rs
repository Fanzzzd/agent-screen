use agent_screen_model::arrangement::ReparkState;
use agent_screen_model::model::{Rect, WindowTarget};
use agent_screen_model::session::{MAX_CLOSED_CONNECTIONS, Sessions};
use agent_screen_model::spares::{
    SpareInfo, keep_spares, pick_spare, retire_instead, window_covers_spare,
};
use agent_screen_model::viewer_press::dragged_window_origin;

#[test]
fn r4_overhanging_and_covered_spares_cause_zero_tidy_churn_while_pick_spare_skips_covered() {
    let live_area = Rect::new(2560.0, -800.0, 1280.0, 800.0);
    let spare_s1 = Rect::new(3840.0, -800.0, 1280.0, 800.0);

    // 1 pt and 120 pt overhangs from a window centered on `live_area` do NOT cover `spare_s1`.
    let overhang_1pt = Rect::new(3441.0, -700.0, 400.0, 300.0);
    assert!(live_area.contains(overhang_1pt.mid()));
    assert!(overhang_1pt.intersects(&spare_s1));
    assert!(!window_covers_spare(&overhang_1pt, &spare_s1));

    let overhang_120pt = Rect::new(3560.0, -700.0, 400.0, 300.0);
    assert!(live_area.contains(overhang_120pt.mid()));
    assert!(!window_covers_spare(&overhang_120pt, &spare_s1));

    // A window recorded on `live_area` that sits mostly (or centered) on `spare_s1` DOES cover it.
    let stranded_on_s1 = Rect::new(3940.0, -700.0, 400.0, 300.0);
    assert!(window_covers_spare(&stranded_on_s1, &spare_s1));
    let majority_on_s1 = Rect::new(3760.0, -700.0, 400.0, 300.0); // 320 of 400 pt on S1 (80%)
    assert!(window_covers_spare(&majority_on_s1, &spare_s1));

    // Simulate 8 consecutive warm idle `tidy` ticks (both unexpired and expired) for:
    // (a) neighbor overhang (`physically_covered = false`)
    // (b) foreign recorded window stranded on S1 (`physically_covered = true`)
    const READY: usize = 2;
    const MAX_SPARES: usize = 3;
    let mut releases = 0usize;
    let mut creations = 0usize;

    for covered in [false, true] {
        for tick in 0..8 {
            let expired = tick >= 4;
            let pool = [
                SpareInfo {
                    size: (1280.0, 800.0),
                    unit: Some(8),
                    expired,
                    has_windows: false,
                    physically_covered: covered,
                },
                SpareInfo {
                    size: (1280.0, 800.0),
                    unit: Some(9),
                    expired,
                    has_windows: false,
                    physically_covered: false,
                },
            ];
            let keep = keep_spares(&pool, true, READY, MAX_SPARES, &[7]);
            let released_now = keep.iter().filter(|&&stay| !stay).count();
            let remaining = keep.iter().filter(|&&stay| stay).count();
            releases += released_now;
            if remaining < READY {
                creations += 1;
            }
            if covered {
                // `pick_spare` skips covered S1 (index 0) and picks clean S2 (index 1).
                assert_eq!(pick_spare(&pool, (1280.0, 800.0)), Some(1));
                assert_eq!(pick_spare(&pool[..1], (1280.0, 800.0)), None);
                // `retire_instead` refuses to evict a covered spare when only covered spares sit higher.
                let full_with_covered_top = [
                    pool[1],
                    pool[1],
                    SpareInfo {
                        unit: Some(12),
                        ..pool[0]
                    },
                ];
                assert_eq!(retire_instead(&full_with_covered_top, 3, Some(10)), None);
            } else {
                assert_eq!(pick_spare(&pool, (1280.0, 800.0)), Some(0));
            }
        }
    }
    assert_eq!(releases, 0, "idle tidy released spares on overhang/cover");
    assert_eq!(creations, 0, "idle tidy created spares on overhang/cover");
}

#[test]
fn r4_closed_connection_tombstones_are_bounded() {
    let mut sessions = Sessions::default();
    let total = (MAX_CLOSED_CONNECTIONS + 50) as u64;
    for conn in 1..=total {
        sessions.drop_connection(conn);
    }
    // Recent closed connections remain tombstoned and cannot re-open.
    assert!(!sessions.note_connection("s1", total));
    assert!(!sessions.note_connection("s1", 51));
    // Oldest connections beyond MAX_CLOSED_CONNECTIONS were evicted so the set stays bounded.
    assert!(sessions.note_connection("s1", 1));
}

#[test]
fn r4_retry_does_not_undo_a_later_user_move_and_expires_after_five_failures() {
    let mut state = ReparkState::default();
    let d7 = 7;
    let old = Rect::new(2560.0, -800.0, 1280.0, 800.0);
    let now = Rect::new(3840.0, -800.0, 1280.0, 800.0);
    state.note_adopted(d7, old);

    let win = WindowTarget {
        pid: 42,
        window_id: 100,
    };
    let initial = Rect::new(2700.0, -650.0, 400.0, 300.0);

    // Pass 1: display 7 moves from 2560 to 3840, shifting `win` toward (3980, -650),
    // and the AX write fails.
    let (g1, _) = state.begin(&[d7]).unwrap();
    let (s1, _) = state.plan_shifts(g1, &[d7], &[(d7, now)], &[], &[(win, initial, Some(d7))]);
    assert_eq!(s1, vec![(win, Rect::new(3980.0, -650.0, 400.0, 300.0))]);
    let (_, first) = state.finish_with_failures(g1, &[win]);
    assert_eq!(first, vec![win]);
    assert!(state.has_pending_retries(&[d7]));

    // Meanwhile the user drags `win` in the Viewer or `place_window` moves it to (4300, -500).
    // The next pass must drop the stale retry instead of pulling `win` back to (3980, -650).
    let moved_by_user = Rect::new(4300.0, -500.0, 400.0, 300.0);
    let (g2, _) = state.begin(&[d7]).unwrap();
    let (s2, _) = state.plan_shifts(
        g2,
        &[d7],
        &[(d7, now)],
        &[],
        &[(win, moved_by_user, Some(d7))],
    );
    assert!(
        s2.is_empty(),
        "stale pending shift undid later move: {s2:?}"
    );
    state.finish(g2);
    assert!(!state.has_pending_retries(&[d7]));

    // Separate window that stays stuck at its failed frame retries up to 4 times
    // and expires on its 5th failure so a ScreensChanged hours later never retries it.
    let stuck = WindowTarget {
        pid: 99,
        window_id: 200,
    };
    let now2 = Rect::new(5120.0, -800.0, 1280.0, 800.0);
    let stuck_frame = Rect::new(3980.0, -650.0, 400.0, 300.0);
    let expected_dest = Rect::new(5260.0, -650.0, 400.0, 300.0);

    for attempt in 1..=5 {
        let (g, _) = state.begin(&[d7]).unwrap();
        let (shifts, _) = state.plan_shifts(
            g,
            &[d7],
            &[(d7, now2)],
            &[],
            &[(stuck, stuck_frame, Some(d7))],
        );
        assert_eq!(shifts, vec![(stuck, expected_dest)], "attempt {attempt}");
        state.finish_with_failures(g, &[stuck]);
        if attempt < 5 {
            assert!(
                state.has_pending_retries(&[d7]),
                "should still retry after failure {attempt}"
            );
        } else {
            assert!(
                !state.has_pending_retries(&[d7]),
                "pending shift must expire on failure 5"
            );
        }
    }

    let (g6, _) = state.begin(&[d7]).unwrap();
    let (s6, _) = state.plan_shifts(
        g6,
        &[d7],
        &[(d7, now2)],
        &[],
        &[(stuck, stuck_frame, Some(d7))],
    );
    assert!(s6.is_empty(), "expired shift ran after 5 failures: {s6:?}");
    state.finish(g6);
}

#[test]
fn r4_very_tall_window_follows_drag_direction() {
    let area = Rect::new(0.0, 0.0, 900.0, 600.0);
    let frame = Rect::new(340.0, -100.0, 220.0, 1300.0);
    let (x, y) = dragged_window_origin(frame, (450.0, 20.0), (450.0, 60.0), area);
    assert_eq!(x, 340.0);
    assert_eq!(
        y, -60.0,
        "40 pt downward drag of 1300 pt window on 600 pt display should move y from -100 to -60, got {y}"
    );
    assert!(area.contains((x + 110.0, y + 650.0)));

    // Extreme downward drag clamps at max_y = 600 - 1 - 650 = -51, keeping center on [0, 600].
    let (_, y_down) = dragged_window_origin(frame, (450.0, 20.0), (450.0, 420.0), area);
    assert_eq!(y_down, -51.0);
    assert!(area.contains((x + 110.0, y_down + 650.0)));

    // Extreme upward drag clamps at min_y = 0 + 1 - 650 = -649, keeping center on [0, 600].
    let (_, y_up) = dragged_window_origin(frame, (450.0, 500.0), (450.0, -400.0), area);
    assert_eq!(y_up, -649.0);
    assert!(area.contains((x + 110.0, y_up + 650.0)));
}
