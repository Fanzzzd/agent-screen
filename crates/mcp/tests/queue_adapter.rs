//! Compile the app's actual OS-free queue coordinator into this isolated
//! harness. It exercises lifecycle and projection without linking GPUI,
//! creating a display, or sending input.

use agent_screen_model::model;

#[path = "../../../apps/macos/src/input_queue.rs"]
mod app_input_queue;

use app_input_queue::InputQueue;

fn owner(connection: u64) -> model::Owner {
    model::Owner {
        session_key: format!("session{connection}"),
        client: "Test".into(),
        title: "Queue".into(),
        connection,
    }
}

#[test]
fn approval_decline_withdraw_done_and_revoke_use_the_app_coordinator() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let a = input.owner(1, 1).unwrap();
    let b = input.owner(2, 2).unwrap();
    let a_id = input
        .submit(a.clone(), "a".into(), "Drag A".into())
        .unwrap();
    assert!(input.approval_needed());
    let old_card = input.queue.approval();
    input.decline(old_card);
    assert_eq!(
        input.status(&a, Some(a_id), false)["requests"][0]["state"],
        "declined"
    );
    let b_id = input
        .submit(b.clone(), "b".into(), "Drag B".into())
        .unwrap();
    assert!(!input.approve(old_card));
    assert!(input.approve(input.queue.approval()));
    assert_eq!(input.elapsed_seconds(), Some(0));
    assert!(input.withdraw(&a, b_id).is_err());
    input.done(&a);
    assert!(input.queue.grant().is_some());
    input.revoke();
    assert!(input.queue.grant().is_none());
    assert!(input.approve(input.queue.approval()));
    input.done(&b);
    assert!(input.queue.grant().is_none());
    assert!(!input.approval_needed());
    input.end_display(1);
    assert!(input.owner(1, 1).is_err());
    assert!(input.owner(2, 2).is_ok());
    input.tick();
}
