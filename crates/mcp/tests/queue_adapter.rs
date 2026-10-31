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

fn displays(ids: &[u32]) -> model::Displays<()> {
    let mut d = model::Displays::default();
    for &id in ids {
        d.add(
            id,
            format!("d{id}"),
            (800.0, 600.0),
            Some(owner(id.into())),
            (),
        );
    }
    d
}

/// The grant ends on its own after its idle time; the model's clock is
/// moved instead of sleeping 30 seconds.
fn expire(input: &mut InputQueue) {
    input
        .queue
        .tick(input.now() + agent_screen_model::input_queue::IDLE_MS);
    input.tick();
    assert!(input.queue.grant().is_none());
}

#[test]
fn an_approved_batch_is_not_asked_again_when_its_unexecuted_grant_idles_out() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let d = displays(&[1, 2]);
    let a = input.owner(1, 1).unwrap();
    let b = input.owner(2, 2).unwrap();
    let a_id = input
        .submit(a.clone(), "a".into(), "Drag A".into())
        .unwrap();
    assert!(input.approval_needed());
    assert_eq!(input.card_display(&d), Some(1));
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    assert!(
        !input.approval_needed(),
        "the user already allowed this request; asking again repeats the card and notification"
    );
    assert_eq!(input.card_display(&d), None);
    // The agent still sees an honest queued state: nothing ran.
    let state = input.status(&a, Some(a_id), false);
    assert_eq!(state["requests"][0]["state"], "queued");
    assert_eq!(state["executorAvailable"], false);
    // A new request asks again, for its whole batch.
    input
        .submit(b.clone(), "b".into(), "Drag B".into())
        .unwrap();
    assert!(input.approval_needed());
    assert_eq!(input.card_display(&d), Some(2));
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    assert!(!input.approval_needed());
    // A revocation takes the approval back.
    input
        .submit(a.clone(), "a2".into(), "Drag A again".into())
        .unwrap();
    assert!(input.approve(input.queue.approval()));
    input.revoke();
    assert!(input.approval_needed());
}

#[test]
fn a_request_from_a_takeover_display_is_still_asked_after_the_grant() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let a = input.owner(1, 1).unwrap();
    let b = input.owner(2, 2).unwrap();
    input.submit(a, "a".into(), "Drag A".into()).unwrap();
    input.takeover(2, true);
    input.submit(b, "b".into(), "Drag B".into()).unwrap();
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    input.takeover(2, false);
    // The grant never covered display 2, which was in a Takeover.
    assert!(input.approval_needed());
    assert_eq!(input.card_display(&displays(&[1, 2])), Some(2));
}
