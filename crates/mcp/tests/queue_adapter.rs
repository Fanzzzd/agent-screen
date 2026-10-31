//! Compile the app's actual OS-free queue coordinator into this isolated
//! harness. It exercises lifecycle and projection without linking GPUI,
//! creating a display, or sending input.

use agent_screen_model::model;

#[path = "../../../apps/macos/src/input_queue.rs"]
mod app_input_queue;

use app_input_queue::{InputQueue, Plan};
use std::sync::atomic::Ordering;

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
        .submit(a.clone(), "a".into(), "Drag A".into(), None)
        .unwrap();
    assert!(input.approval_needed());
    let old_card = input.queue.approval();
    input.decline(old_card);
    assert_eq!(
        input.status(&a, Some(a_id), false)["requests"][0]["state"],
        "declined"
    );
    let b_id = input
        .submit(b.clone(), "b".into(), "Drag B".into(), None)
        .unwrap();
    assert!(!input.approve(old_card));
    assert!(input.approve(input.queue.approval()));
    assert_eq!(input.elapsed_seconds(), Some(0));
    assert!(input.withdraw(&a, b_id).is_err());
    input.done(&a);
    assert!(input.queue.grant().is_some());
    input.revoke();
    assert!(input.queue.grant().is_none());
    settle(&mut input);
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
fn idle_out(input: &mut InputQueue) {
    input
        .queue
        .tick(input.now() + agent_screen_model::input_queue::IDLE_MS);
    input.tick();
    assert!(input.queue.grant().is_none());
}

/// The executor's cleanup after a grant ends, as the shell does it.
fn settle(input: &mut InputQueue) {
    if let Some(end) = input.ending() {
        input.ended(end);
    }
}

fn expire(input: &mut InputQueue) {
    idle_out(input);
    settle(input);
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
        .submit(a.clone(), "a".into(), "Drag A".into(), None)
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
    // The agent sees that nothing ran and that no card will ask again.
    let state = input.status(&a, Some(a_id), false);
    assert_eq!(state["requests"][0]["state"], "approval_lapsed");
    assert_eq!(state["executorAvailable"], true);
    // A new request asks again, for its whole batch.
    input
        .submit(b.clone(), "b".into(), "Drag B".into(), None)
        .unwrap();
    assert!(input.approval_needed());
    assert_eq!(input.card_display(&d), Some(2));
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    assert!(!input.approval_needed());
    // A revocation takes the approval back.
    input
        .submit(a.clone(), "a2".into(), "Drag A again".into(), None)
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
    input.submit(a, "a".into(), "Drag A".into(), None).unwrap();
    input.takeover(2, true);
    input.submit(b, "b".into(), "Drag B".into(), None).unwrap();
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    input.takeover(2, false);
    // The grant never covered display 2, which was in a Takeover.
    assert!(input.approval_needed());
    assert_eq!(input.card_display(&displays(&[1, 2])), Some(2));
}

fn state(input: &InputQueue, owner: &agent_screen_model::input_queue::Owner, id: u64) -> String {
    input.status(owner, Some(id), false)["requests"][0]["state"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A request the user allowed whose grant ended with nothing run is not
/// asked about again, so the agent must not be told to keep waiting.
#[test]
fn a_lapsed_approval_is_its_own_state_and_ends_the_wait() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    let a = input.owner(1, 1).unwrap();
    let id = input
        .submit(a.clone(), "drag".into(), "Drag".into(), None)
        .unwrap();
    assert_eq!(state(&input, &a, id), "queued");
    assert!(input.awaits(id, false));
    assert!(input.approve(input.queue.approval()));
    assert_eq!(state(&input, &a, id), "approved");
    expire(&mut input);
    let lapsed = input.status(&a, Some(id), false);
    assert_eq!(lapsed["requests"][0]["state"], "approval_lapsed");
    assert!(lapsed["requests"][0]["next"].is_string());
    assert!(!input.awaits(id, false));
    // The same token is the same request: no new prompt, no wait.
    let retry = input
        .submit(a.clone(), "drag".into(), "Drag".into(), None)
        .unwrap();
    assert_eq!(retry, id);
    assert_eq!(state(&input, &a, id), "approval_lapsed");
    assert!(!input.approval_needed());
    // A new token replaces it and is asked about.
    let other = input
        .submit(a.clone(), "other".into(), "Drag".into(), None)
        .unwrap();
    assert_ne!(other, id);
    assert_eq!(state(&input, &a, id), "stopped");
    assert_eq!(state(&input, &a, other), "queued");
    assert!(input.approval_needed());
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    assert_eq!(state(&input, &a, other), "approval_lapsed");
    // Withdrawing and asking again starts a fresh request the card asks about.
    input.withdraw(&a, other).unwrap();
    let again = input
        .submit(a.clone(), "other".into(), "Drag".into(), None)
        .unwrap();
    assert_ne!(again, other);
    assert_eq!(state(&input, &a, again), "queued");
    assert!(input.approval_needed());
    // A revocation takes the approval back: the card asks again.
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    assert_eq!(state(&input, &a, again), "approval_lapsed");
    input.revoke();
    assert_eq!(state(&input, &a, again), "queued");
    assert!(input.approval_needed());
}

#[test]
fn ended_or_withdrawn_approved_requests_leave_the_approved_set() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let a = input.owner(1, 1).unwrap();
    let b = input.owner(2, 2).unwrap();
    let a_id = input
        .submit(a.clone(), "a".into(), "Drag".into(), None)
        .unwrap();
    let b_id = input
        .submit(b.clone(), "b".into(), "Drag".into(), None)
        .unwrap();
    assert!(input.approve(input.queue.approval()));
    input.end_display(1);
    input.withdraw(&b, b_id).unwrap();
    expire(&mut input);
    assert!(!input.approved.contains(&a_id));
    assert!(!input.approved.contains(&b_id));
    input.register(1, &owner(3));
    let c = input.owner(1, 3).unwrap();
    let c_id = input
        .submit(c.clone(), "a".into(), "Drag".into(), None)
        .unwrap();
    assert_eq!(state(&input, &c, c_id), "queued");
    assert!(input.approval_needed());
}

#[test]
fn a_takeover_during_the_grant_keeps_the_request_allowed() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    let a = input.owner(1, 1).unwrap();
    let id = input
        .submit(a.clone(), "a".into(), "Drag".into(), None)
        .unwrap();
    assert!(input.approve(input.queue.approval()));
    input.takeover(1, true);
    assert_eq!(
        input.status(&a, Some(id), true)["requests"][0]["state"],
        "takeover"
    );
    expire(&mut input);
    input.takeover(1, false);
    assert_eq!(state(&input, &a, id), "approval_lapsed");
    assert!(!input.approval_needed());
}

#[test]
fn a_help_request_outranks_an_open_grant_card() {
    use agent_screen_model::card::{self, Card};
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let mut d = displays(&[1, 2]);
    let b = input.owner(2, 2).unwrap();
    input.submit(b, "b".into(), "Drag".into(), None).unwrap();
    assert!(input.approve(input.queue.approval()));
    let pick = |d: &model::Displays<()>, input: &InputQueue| {
        card::select(d, None, |_, _| false, input.card_display(d), false)
    };
    assert_eq!(pick(&d, &input), Some(Card::Input(2)));
    d.get_mut(1).unwrap().help_request = Some("Sign in".into());
    assert_eq!(pick(&d, &input), Some(Card::Help(1)));
    d.get_mut(2).unwrap().help_request = Some("Code".into());
    d.get_mut(1).unwrap().help_request = None;
    assert_eq!(pick(&d, &input), Some(Card::Help(2)));
    d.get_mut(2).unwrap().help_request = None;
    assert_eq!(pick(&d, &input), Some(Card::Input(2)));
}

/// The card a new request brings lists every waiting request, a lapsed one
/// included, so Not Now answers the whole batch the user saw.
#[test]
fn not_now_on_a_later_card_declines_a_lapsed_request_it_lists() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let a = input.owner(1, 1).unwrap();
    let b = input.owner(2, 2).unwrap();
    let a_id = input
        .submit(a.clone(), "a".into(), "Drag".into(), None)
        .unwrap();
    assert!(input.approve(input.queue.approval()));
    expire(&mut input);
    assert_eq!(state(&input, &a, a_id), "approval_lapsed");
    let b_id = input
        .submit(b.clone(), "b".into(), "Drag".into(), None)
        .unwrap();
    assert!(input.approval_needed());
    let listed: Vec<_> = input.queue.waiting().map(|r| r.id).collect();
    assert_eq!(listed, vec![a_id, b_id]);
    input.decline(input.queue.approval());
    assert_eq!(state(&input, &a, a_id), "declined");
    assert_eq!(state(&input, &b, b_id), "declined");
}

fn plan(x: f64) -> Plan {
    Plan {
        target: model::WindowTarget {
            pid: 1,
            window_id: 7,
        },
        frame: model::Rect::new(0.0, 0.0, 800.0, 600.0),
        area: model::Rect::new(0.0, 0.0, 800.0, 600.0),
        from: (x, 10.0),
        to: (x + 50.0, 10.0),
        flags: 0,
    }
}

/// The executor gets each loan in turn, only during a grant and only while
/// the agent's call waits; how it ended is what the agent sees.
#[test]
fn the_executor_runs_allowed_drags_in_order_and_reports_how_they_ended() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    input.register(2, &owner(2));
    let a = input.owner(1, 1).unwrap();
    let b = input.owner(2, 2).unwrap();
    let a_id = input
        .submit(a.clone(), "a".into(), "Drag A".into(), Some(plan(1.0)))
        .unwrap();
    let b_id = input
        .submit(b.clone(), "b".into(), "Drag B".into(), Some(plan(2.0)))
        .unwrap();
    let ask = input
        .submit(a.clone(), "ask".into(), "Ask only".into(), None)
        .unwrap();
    assert_eq!(
        state(&input, &a, a_id),
        "stopped",
        "replaced by its session's newer request"
    );
    assert!(
        input.start_loan().is_none(),
        "nothing runs before the user allows it"
    );
    assert!(input.awaits(b_id, false));
    assert!(input.approve(input.queue.approval()));
    assert!(
        !input.awaits(ask, false),
        "an approval-only request has its answer"
    );
    assert!(input.awaits(b_id, false), "a drag waits for its turn");
    let (id, p) = input.start_loan().unwrap();
    assert_eq!((id, p), (b_id, plan(2.0)));
    assert!(input.loan_ok.load(Ordering::Relaxed));
    assert!(input.start_loan().is_none(), "one loan at a time");
    assert_eq!(state(&input, &b, b_id), "running");
    input.finish_loan(b_id, None);
    assert_eq!(state(&input, &b, b_id), "completed");
    assert!(!input.loan_ok.load(Ordering::Relaxed));
    assert!(!input.awaits(b_id, false));
    assert!(
        input.start_loan().is_none(),
        "the approval-only request never runs"
    );

    // A new drag joins the open grant; the user's touch stops it.
    let c_id = input
        .submit(b.clone(), "c".into(), "Drag C".into(), Some(plan(3.0)))
        .unwrap();
    assert_eq!(input.start_loan().map(|l| l.0), Some(c_id));
    input.revoke();
    assert!(
        !input.loan_ok.load(Ordering::Relaxed),
        "the next event must not go"
    );
    assert_eq!(state(&input, &b, c_id), "stopping");
    assert!(
        input.ending().is_none(),
        "cleanup waits for the executor to let go"
    );
    input.finish_loan(c_id, None);
    assert_eq!(state(&input, &b, c_id), "stopped");
    let end = input.ending().unwrap();
    assert!(
        !end.restore,
        "a user who took control keeps the pointer where it is"
    );
    input.ended(end);
    assert!(input.ending().is_none());
}

#[test]
fn a_detached_drag_does_not_run_until_its_call_waits_again() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    let a = input.owner(1, 1).unwrap();
    let id = input
        .submit(a.clone(), "a".into(), "Drag".into(), Some(plan(1.0)))
        .unwrap();
    input.detach(id);
    assert!(input.approve(input.queue.approval()));
    assert!(input.start_loan().is_none());
    input.attach(id);
    assert_eq!(input.start_loan().map(|l| l.0), Some(id));
    input.finish_loan(id, Some("the window left the display".into()));
    let s = input.status(&a, Some(id), false);
    assert_eq!(s["requests"][0]["state"], "stopped");
    assert_eq!(s["requests"][0]["stopReason"], "Failed");
    assert_eq!(s["requests"][0]["failure"], "the window left the display");
    // Nothing left to run: the grant idles out and ends normally.
    idle_out(&mut input);
    let end = input.ending().unwrap();
    assert!(
        end.restore,
        "a normal end puts the pointer and front app back"
    );
    input.ended(end);
}

#[test]
fn a_takeover_or_an_ended_display_stops_the_running_loan() {
    let mut input = InputQueue::default();
    input.register(1, &owner(1));
    let a = input.owner(1, 1).unwrap();
    let id = input
        .submit(a.clone(), "a".into(), "Drag".into(), Some(plan(1.0)))
        .unwrap();
    assert!(input.approve(input.queue.approval()));
    input.start_loan().unwrap();
    input.takeover(1, true);
    assert!(!input.loan_ok.load(Ordering::Relaxed));
    input.finish_loan(id, None);
    assert_eq!(
        input.status(&a, Some(id), true)["requests"][0]["stopReason"],
        "Takeover"
    );
    input.takeover(1, false);
    let id = input
        .submit(a.clone(), "b".into(), "Drag".into(), Some(plan(2.0)))
        .unwrap();
    input.start_loan().unwrap();
    input.end_display(1);
    assert!(!input.loan_ok.load(Ordering::Relaxed));
    assert!(
        input.queue.request(id).is_some(),
        "kept until the executor lets go"
    );
    input.finish_loan(id, None);
    assert!(
        input.queue.request(id).is_none(),
        "ended owners release their record after cleanup"
    );
}
