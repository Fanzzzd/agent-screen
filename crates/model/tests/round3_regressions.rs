use agent_screen_model::arrangement::ReparkState;
use agent_screen_model::input_queue::Queue;
use agent_screen_model::model::{Displays, Home, Origin, Owner, Rect, WindowTarget};
use agent_screen_model::viewer_press::dragged_window_origin;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

fn owner(key: &str, connection: u64) -> Owner {
    Owner {
        session_key: key.into(),
        client: "Claude Code".into(),
        title: "Round 3".into(),
        connection,
    }
}

#[test]
fn r3_input_queue_owner_preserved_after_rebind_and_overlapping_fallback() {
    let mut d = Displays::default();
    let mut q = Queue::default();
    d.add(
        7,
        "Agent Screen 1".into(),
        (1280.0, 800.0),
        Some(owner("s1", 1)),
        (),
    );
    assert!(q.register("s1".into(), 1, 7).is_some());

    // 1. Grace-period rebind: connection 1 closes, connection 11 rebinds within grace.
    q.disconnect(1, 10_000);
    let (marked, migrated) = d.connection_disconnected_with_migrated(1, 10_000);
    d.register_queue_owners(&mut q, &migrated);
    assert_eq!(marked, 1);
    assert!(migrated.is_empty());

    let rebound = d.rebind_disconnected("s1", 11, 10_500);
    d.register_queue_owners(&mut q, &rebound);
    assert_eq!(rebound, vec![7]);
    let owner_11 = q
        .owner(7)
        .expect("rebound display 7 must have Queue owner")
        .clone();
    assert_eq!(owner_11.connection, 11);
    assert!(
        q.enqueue(owner_11, "click:1".into(), "Retry click".into(), 10_600)
            .is_ok()
    );

    // 2. Overlapping fallback: connection 15 speaks for s1 BEFORE 11 closes.
    assert!(d.rebind_disconnected("s1", 15, 11_000).is_empty());
    q.disconnect(11, 11_200);
    let (marked2, migrated2) = d.connection_disconnected_with_migrated(11, 11_200);
    d.register_queue_owners(&mut q, &migrated2);
    assert_eq!(marked2, 0);
    assert_eq!(migrated2, vec![7]);
    let owner_15 = q
        .owner(7)
        .expect("fallback-migrated display 7 must have Queue owner")
        .clone();
    assert_eq!(owner_15.connection, 15);
    assert!(
        q.enqueue(owner_15, "click:2".into(), "Second click".into(), 11_300)
            .is_ok()
    );
}

#[test]
fn r3_rebind_to_closed_connection_leaks_display_forever() {
    let mut d = Displays::default();
    d.add(
        7,
        "Agent Screen 1".into(),
        (1280.0, 800.0),
        Some(owner("s1", 9)),
        (),
    );
    d.claim_on(
        7,
        WindowTarget {
            pid: 100,
            window_id: 1,
        },
        Origin::Placed,
        None,
    );

    // Event::Closed runs inline first for connection 9.
    let _ = d.connection_disconnected(9, 10_000);
    // Spawned request from connection 9 runs right after close.
    let rebound = d.rebind_disconnected("s1", 9, 10_050);
    assert!(
        rebound.is_empty(),
        "closed connection 9 must not rebind its display"
    );
    assert_eq!(d.expire_disconnected(13_000, 2_000), vec![7]);
}

#[test]
fn r3_stale_open_connection_absorbs_last_close() {
    let mut d = Displays::default();
    d.add(
        7,
        "Agent Screen 1".into(),
        (1280.0, 800.0),
        Some(owner("s1", 9)),
        (),
    );
    // Overlapping connection 11 speaks before 9 closes.
    let _ = d.rebind_disconnected("s1", 11, 9_500);
    // Connection 9 closes (migrates 7 -> 11).
    let _ = d.connection_disconnected(9, 10_000);
    // Late spawned task from closed connection 9 calls rebind_disconnected.
    let _ = d.rebind_disconnected("s1", 9, 10_050);

    // Now connection 11 (the last truly open connection!) closes.
    let _ = d.connection_disconnected(11, 20_000);
    assert_eq!(
        d.expire_disconnected(60_000, 2_000),
        vec![7],
        "stale closed connection 9 must not absorb connection 11's close"
    );
}

#[test]
fn r3_very_tall_window_center_leaves_display() {
    // Second-row 1280x800 display above a display below it (`y = -800..0`).
    let area = Rect::new(2560.0, -1600.0, 1280.0, 800.0);
    let below = Rect::new(2560.0, -800.0, 1280.0, 800.0);
    let frame = Rect::new(2700.0, -1600.0, 500.0, 1700.0);
    let (nx, ny) = dragged_window_origin(frame, (2800.0, -1580.0), (2800.0, -1200.0), area);
    let moved = Rect::new(nx, ny, frame.width, frame.height);
    assert!(
        area.contains(moved.mid()),
        "1700 pt window center {:?} must stay on its 800 pt display {:?}, not neighbor {:?}",
        moved.mid(),
        area,
        below
    );
    assert!(!below.contains(moved.mid()));
}

#[test]
fn r3_failed_shift_retries_and_rebases_across_further_moves() {
    let user = Rect::new(0.0, 0.0, 2560.0, 1440.0);
    let old_7 = Rect::new(2560.0, -800.0, 1280.0, 800.0);
    let mid_7 = Rect::new(3840.0, -800.0, 1280.0, 800.0);
    let fin_7 = Rect::new(5120.0, -800.0, 1280.0, 800.0);
    let win = WindowTarget {
        pid: 400,
        window_id: 55,
    };
    let win_frame = Rect::new(2700.0, -650.0, 500.0, 400.0);

    let mut repark = ReparkState::default();
    repark.note_adopted(7, old_7);

    let (gen1, _) = repark.begin(&[7]).unwrap();
    let (shifts1, _) = repark.plan_shifts(
        gen1,
        &[7],
        &[(7, mid_7)],
        &[user],
        &[(win, win_frame, None)],
    );
    assert_eq!(
        shifts1,
        vec![(win, Rect::new(3980.0, -650.0, 500.0, 400.0))]
    );
    let (rerun1, first_failed1) = repark.finish_with_failures(gen1, &[win]);
    assert!(!rerun1);
    assert_eq!(first_failed1, vec![win]);
    assert!(repark.has_pending_retries(&[7]));

    // Pass 2: display 7 hasn't moved again; failed shift is retried!
    let (gen2, _) = repark.begin(&[7]).unwrap();
    let (shifts2, _) = repark.plan_shifts(
        gen2,
        &[7],
        &[(7, mid_7)],
        &[user],
        &[(win, win_frame, None)],
    );
    assert_eq!(shifts2, shifts1);
    // Second consecutive failure does NOT repeat in `first_failures`.
    let (_, first_failed2) = repark.finish_with_failures(gen2, &[win]);
    assert!(first_failed2.is_empty());

    // Pass 3: display 7 moves AGAIN (`mid_7 -> fin_7`) while `win` is still stuck at `win_frame`.
    // The pending destination rebases directly onto `fin_7`!
    let (gen3, _) = repark.begin(&[7]).unwrap();
    let (shifts3, _) = repark.plan_shifts(
        gen3,
        &[7],
        &[(7, fin_7)],
        &[user],
        &[(win, win_frame, None)],
    );
    assert_eq!(
        shifts3,
        vec![(win, Rect::new(5260.0, -650.0, 500.0, 400.0))]
    );
    // Succeeds on pass 3 -> cleared!
    let (_, _) = repark.finish_with_failures(gen3, &[]);
    assert!(!repark.has_pending_retries(&[7]));
}

#[test]
fn r3_recorded_owners_sink_syncs_automatically_on_return_handover_and_all_mutations() {
    let sink: Arc<RwLock<BTreeMap<u32, u32>>> = Arc::new(RwLock::new(BTreeMap::new()));
    let mut d = Displays::default();
    d.bind_recorded_owners(sink.clone());
    d.add(
        7,
        "Agent Screen 1".into(),
        (1280.0, 800.0),
        Some(owner("s1", 1)),
        (),
    );
    d.add(
        8,
        "Agent Screen 2".into(),
        (1280.0, 800.0),
        Some(owner("s2", 2)),
        (),
    );

    let w = WindowTarget {
        pid: 300,
        window_id: 91,
    };
    let home = Home {
        frame: Rect::new(100.0, 100.0, 600.0, 400.0),
        workspace: None,
    };
    d.claim_on(7, w, Origin::HandedOver, Some(home));
    assert_eq!(sink.read().unwrap().get(&91).copied(), Some(7));

    // Transfer 7 -> 8 updates sink without manual sync.
    let tr = d.transfer(7, 8, w, "Preview").unwrap();
    assert_eq!(sink.read().unwrap().get(&91).copied(), Some(8));

    // Undo transfer restores 7 in sink.
    d.undo_transfer(tr);
    assert_eq!(sink.read().unwrap().get(&91).copied(), Some(7));

    // `return_handover` clears 91 from sink immediately (L3)!
    let given = d.return_handover(7, 91).unwrap();
    assert_eq!(sink.read().unwrap().get(&91), None);

    // Failed give-back restores 7 in sink immediately!
    assert!(d.give_back_failed_on(7, given));
    assert_eq!(sink.read().unwrap().get(&91).copied(), Some(7));
}
