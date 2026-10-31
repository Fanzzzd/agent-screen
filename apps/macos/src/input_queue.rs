//! App-wide admission, scheduling and presentation for Input Requests. No
//! input is sent here: the shell runs each loan this hands out through the
//! executor, and reports back how it ended.

use crate::model::{DisplayId, Displays, Owner as DisplayOwner, Rect, WindowTarget};
use agent_screen_model::input_queue::{EnqueueError, GrantEnd, Owner, Queue, RequestId, Status};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub const EXECUTOR_AVAILABLE: bool = true;
const REASON_CHARS: usize = 300;
const OPERATION_CHARS: usize = 2048;
const LAPSED_NEXT: &str = "The user allowed this request, but the approval ended before it ran and they will not be asked about it again. Waiting or retrying the same operation returns at once. Make a new request to ask again, use a background alternative or request_help, then withdraw this one.";

pub struct InputQueue {
    pub queue: Queue,
    clock: Instant,
    /// Requests the user already allowed whose grant ended with nothing
    /// executed. Presentation only: they stay queued for the agent, and
    /// only a new request or a revocation asks again.
    pub(crate) approved: BTreeSet<RequestId>,
    /// What each runnable request does. A request without one, made with
    /// input_queue's request action, only asks for approval.
    plans: BTreeMap<RequestId, Plan>,
    /// Why the executor could not carry a loan out.
    failures: BTreeMap<RequestId, String>,
    /// Whether the active loan may send its next event. The executor's
    /// thread reads it before every event.
    pub loan_ok: Arc<AtomicBool>,
}

/// One complete drag, in global points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub target: WindowTarget,
    pub frame: Rect,
    pub area: Rect,
    pub from: (f64, f64),
    pub to: (f64, f64),
    /// CGEventFlags bits.
    pub flags: u64,
}

impl Default for InputQueue {
    fn default() -> Self {
        Self {
            queue: Queue::default(),
            clock: Instant::now(),
            approved: BTreeSet::new(),
            plans: BTreeMap::new(),
            failures: BTreeMap::new(),
            loan_ok: Arc::default(),
        }
    }
}

impl InputQueue {
    pub fn now(&self) -> u64 {
        self.clock.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    /// Called by display adoption, not by a tool that happens to name a
    /// display. A physical display reused as a spare gets a new owner token.
    pub fn register(&mut self, display: DisplayId, owner: &DisplayOwner) {
        let _ = self
            .queue
            .register(owner.session_key.clone(), owner.connection, display);
    }

    pub fn owner(&self, display: DisplayId, connection: u64) -> Result<Owner, String> {
        self.queue
            .owner(display)
            .filter(|owner| owner.connection == connection)
            .cloned()
            .ok_or_else(|| "this connection does not own the queue's display lifetime".into())
    }

    pub fn submit(
        &mut self,
        owner: Owner,
        operation: String,
        reason: String,
        plan: Option<Plan>,
    ) -> Result<RequestId, String> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > REASON_CHARS {
            return Err(format!(
                "reason must contain 1 to {REASON_CHARS} characters"
            ));
        }
        if operation.trim().is_empty() || operation.chars().count() > OPERATION_CHARS {
            return Err(format!(
                "operation must contain 1 to {OPERATION_CHARS} characters"
            ));
        }
        let now = self.now();
        let id = self.queue.enqueue(owner, operation, reason.into(), now).map_err(|why| {
            match why {
                EnqueueError::Full => "the input queue is full; use input_queue done when your session no longer needs its requests, or wait for other sessions to clear theirs",
                EnqueueError::SessionFull => "this connection holds 64 input requests across its displays, including finished ones it has not cleared; use input_queue done on each display once it has read their results, then request again",
                EnqueueError::Invalid => "operation and reason are required",
                EnqueueError::Closed => "the queue's display lifetime has ended",
            }.to_string()
        })?;
        // The caller waits for a request it can run. One without a plan
        // only asks, and must not hold a grant.
        match plan {
            Some(plan) => {
                self.plans.entry(id).or_insert(plan);
            }
            None if !self.plans.contains_key(&id) => self.queue.detach(id, now),
            None => {}
        }
        self.tick();
        Ok(id)
    }

    /// The agent's call waits for `id` again, if it can run.
    pub fn attach(&mut self, id: RequestId) {
        if self.plans.contains_key(&id) {
            let now = self.now();
            self.queue.attach(id, now);
            self.tick();
        }
    }

    /// The agent's call gave up waiting; the request keeps its place.
    pub fn detach(&mut self, id: RequestId) {
        let now = self.now();
        self.queue.detach(id, now);
        self.tick();
    }

    /// The next loan to run, if the grant allows one now.
    pub fn start_loan(&mut self) -> Option<(RequestId, Plan)> {
        if self.queue.pending_end().is_some() {
            return None;
        }
        let now = self.now();
        let id = self.queue.start(now)?;
        let plan = self.plans.get(&id).copied();
        if plan.is_none() {
            // Not reachable: a request without a plan is never attached.
            self.queue.fail(id);
            self.queue.finish(id, now);
        }
        self.tick();
        plan.map(|plan| (id, plan))
    }

    /// The executor let go of everything the loan held. `failure` says why
    /// it could not carry the loan out, if it couldn't.
    pub fn finish_loan(&mut self, id: RequestId, failure: Option<String>) {
        if let Some(why) = failure {
            self.queue.fail(id);
            self.failures.insert(id, why);
        }
        let now = self.now();
        self.queue.finish(id, now);
        if self
            .queue
            .request(id)
            .is_some_and(|r| self.queue.owner(r.owner.display) != Some(&r.owner))
        {
            // End/disconnect retained the active record only until release.
            self.queue.forget(id);
        }
        self.tick();
    }

    /// A grant that ended and waits for the executor's cleanup, once no
    /// loan runs. Call `ended` after restoring.
    pub fn ending(&self) -> Option<GrantEnd> {
        self.queue
            .pending_end()
            .filter(|_| self.queue.active().is_none())
    }

    pub fn ended(&mut self, end: GrantEnd) {
        self.queue.acknowledge_end(end.id);
        self.tick();
    }

    pub fn request_for(&self, owner: &Owner, id: RequestId) -> Result<(), String> {
        self.queue
            .request(id)
            .filter(|request| &request.owner == owner)
            .map(|_| ())
            .ok_or_else(|| "no input request with that id belongs to this display lifetime".into())
    }

    pub fn approve(&mut self, approval: u64) -> bool {
        let changed = self.queue.allow(approval, self.now());
        self.tick();
        changed
    }

    pub fn decline(&mut self, approval: u64) {
        self.queue.decline(approval);
        self.tick();
    }

    pub fn revoke(&mut self) {
        self.queue.user_input();
        self.approved.clear();
        self.tick();
    }

    pub fn withdraw(&mut self, owner: &Owner, id: RequestId) -> Result<bool, String> {
        self.request_for(owner, id)?;
        let changed = self.queue.withdraw(id, owner, self.now());
        self.tick();
        Ok(changed)
    }

    pub fn done(&mut self, owner: &Owner) {
        let pending: Vec<_> = self
            .queue
            .waiting()
            .filter(|r| &r.owner == owner)
            .map(|r| r.id)
            .collect();
        for id in pending {
            self.queue.withdraw(id, owner, self.now());
        }
        self.queue.done(owner, self.now());
        self.forget_terminal(|request_owner| request_owner == owner);
        self.tick();
    }

    pub fn end_display(&mut self, display: DisplayId) {
        self.queue.end_display(display, self.now());
        self.forget_terminal(|owner| owner.display == display);
        self.tick();
    }

    pub fn disconnect(&mut self, connection: u64) {
        self.queue.disconnect(connection, self.now());
        self.forget_terminal(|owner| owner.connection == connection);
        self.tick();
    }

    fn forget_terminal(&mut self, matches: impl Fn(&Owner) -> bool) {
        let terminal: Vec<_> = self
            .queue
            .requests()
            .filter(|r| r.status.terminal() && matches(&r.owner))
            .map(|r| r.id)
            .collect();
        for id in terminal {
            self.queue.forget(id);
        }
    }

    pub fn takeover(&mut self, display: DisplayId, user: bool) {
        self.queue.takeover(display, user, self.now());
        self.tick();
    }

    /// Returns whether the presentation needs a refresh. While a grant is
    /// open the once-a-second shell tick updates elapsed time too.
    pub fn tick(&mut self) -> bool {
        let was = self.queue.grant().map(|grant| grant.id);
        if let Some(grant) = self.queue.grant() {
            let admitted: Vec<_> = self
                .queue
                .waiting()
                .filter(|r| grant.admits(&r.owner))
                .map(|r| r.id)
                .collect();
            self.approved.extend(admitted);
        }
        self.queue.tick(self.now());
        let queue = &self.queue;
        self.approved.retain(|id| {
            queue
                .request(*id)
                .is_some_and(|r| r.status == Status::Queued)
        });
        let requests = &self.queue;
        self.plans.retain(|id, _| requests.request(*id).is_some());
        self.failures
            .retain(|id, _| requests.request(*id).is_some());
        let ok = self
            .queue
            .active()
            .is_some_and(|r| self.queue.may_send(r.id, &r.owner));
        self.loan_ok.store(ok, Ordering::Relaxed);
        was.is_some() || self.queue.grant().is_some()
    }

    /// A waiting request the user has not allowed yet. Status stays queued
    /// for the agent either way; this only decides the card and the
    /// notification.
    pub fn asks(&self, id: RequestId) -> bool {
        self.queue.grant().is_none()
            && !self.approved.contains(&id)
            && self
                .queue
                .request(id)
                .is_some_and(|r| r.status == Status::Queued)
    }

    /// Whether a call waiting on `id` should keep waiting: for the user's
    /// answer, or, once allowed, for its turn and its run. A request that
    /// only asked for approval has its answer once allowed.
    pub fn awaits(&self, id: RequestId, paused: bool) -> bool {
        let Some(r) = self.queue.request(id) else {
            return false;
        };
        match r.status {
            Status::Queued if paused => false,
            Status::Queued if self.queue.grant().is_some() => self.plans.contains_key(&id),
            Status::Queued => self.asks(id),
            Status::Running | Status::Stopping(_) => true,
            _ => false,
        }
    }

    pub fn approval_needed(&self) -> bool {
        self.queue.waiting().any(|r| self.asks(r.id))
    }

    /// The display the queue's card shows, while it needs one: the first
    /// request still to ask about, or during a grant the first waiting
    /// request's display or any live display.
    pub fn card_display<H>(&self, displays: &Displays<H>) -> Option<DisplayId> {
        let id = if self.queue.grant().is_some() {
            self.queue
                .waiting()
                .next()
                .map(|r| r.owner.display)
                .or_else(|| displays.list.iter().find(|d| d.is_live()).map(|d| d.id))
        } else {
            self.queue
                .waiting()
                .find(|r| self.asks(r.id))
                .map(|r| r.owner.display)
        };
        id.filter(|id| displays.get(*id).is_some())
    }

    pub fn elapsed_seconds(&self) -> Option<u64> {
        self.queue
            .grant()
            .map(|grant| self.now().saturating_sub(grant.since_ms) / 1000)
    }

    /// Only this owner's requests cross the agent protocol. Other agents'
    /// identities, reasons and operation tokens stay in the user's UI.
    pub fn status(&self, owner: &Owner, request_id: Option<RequestId>, paused: bool) -> Value {
        let requests: Vec<_> = self
            .queue
            .requests()
            .filter(|request| {
                &request.owner == owner && request_id.is_none_or(|id| id == request.id)
            })
            .map(|request| {
                let position = self
                    .queue
                    .waiting()
                    .position(|r| r.id == request.id)
                    .map(|p| p + 1);
                let state = match request.status {
                    Status::Queued if paused => "takeover",
                    Status::Queued
                        if self.queue.grant().is_some_and(|g| g.admits(&request.owner)) =>
                    {
                        "approved"
                    }
                    // Nothing will ask the user again, so nothing is left
                    // to wait for.
                    Status::Queued if self.approved.contains(&request.id) => "approval_lapsed",
                    Status::Queued => "queued",
                    Status::Running => "running",
                    Status::Stopping(_) => "stopping",
                    Status::Stopped(_) => "stopped",
                    Status::Completed => "completed",
                    Status::Declined => "declined",
                };
                json!({
                    "requestId": request.id,
                    "state": state,
                    "reason": request.reason,
                    "position": position,
                    "next": (state == "approval_lapsed").then_some(LAPSED_NEXT),
                    "stopReason": match request.status {
                        Status::Stopped(why) | Status::Stopping(why) => Some(format!("{why:?}")),
                        _ => None,
                    },
                    "failure": self.failures.get(&request.id),
                    "runnable": self.plans.contains_key(&request.id),
                })
            })
            .collect();
        json!({
            "executorAvailable": EXECUTOR_AVAILABLE,
            "grantActive": self.queue.grant().is_some(),
            "elapsedSeconds": self.elapsed_seconds(),
            "waiting": self.queue.waiting().count(),
            "requests": requests,
            "note": "Only completed means the action ran, with the user's pointer. approved means allowed and waiting its turn; a request made with input_queue's request action is then answered: retry the drag with real_input while the approval lasts.",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display_owner(connection: u64) -> DisplayOwner {
        DisplayOwner {
            session_key: format!("session{connection}"),
            client: "Test".into(),
            title: "Queue".into(),
            connection,
        }
    }

    #[test]
    fn approval_never_reports_an_unexecuted_action_as_completed() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        let owner = input.owner(1, 1).unwrap();
        let id = input
            .submit(owner.clone(), "drag".into(), "Move a card".into(), None)
            .unwrap();
        assert!(input.approve(input.queue.approval()));
        let state = input.status(&owner, Some(id), false);
        assert_eq!(state["executorAvailable"], true);
        assert_eq!(state["requests"][0]["state"], "approved");
        assert!(input.queue.active().is_none());
    }

    #[test]
    fn an_agent_cannot_see_or_withdraw_another_sessions_request() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        input.register(2, &display_owner(2));
        let a = input.owner(1, 1).unwrap();
        let b = input.owner(2, 2).unwrap();
        let id = input
            .submit(a.clone(), "drag".into(), "Private reason".into(), None)
            .unwrap();
        assert!(input.withdraw(&b, id).is_err());
        assert!(
            input.status(&b, None, false)["requests"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            !input
                .status(&b, None, false)
                .to_string()
                .contains("Private reason")
        );
        assert!(input.owner(1, 2).is_err());
    }

    #[test]
    fn a_closed_connection_cannot_submit_work_after_a_display_is_reused() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        let old = input.owner(1, 1).unwrap();
        input.disconnect(1);
        input.register(1, &display_owner(2));
        assert!(
            input
                .submit(old, "late".into(), "Drag".into(), None)
                .is_err()
        );
        assert!(input.owner(1, 2).is_ok());
    }

    #[test]
    fn done_and_closed_owners_do_not_permanently_fill_the_queue() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        let owner = input.owner(1, 1).unwrap();
        for i in 0..agent_screen_model::input_queue::MAX_SESSION_REQUESTS {
            input
                .submit(owner.clone(), format!("drag{i}"), "Drag".into(), None)
                .unwrap();
        }
        assert!(
            input
                .submit(owner.clone(), "overflow".into(), "Drag".into(), None)
                .is_err()
        );
        input.done(&owner);
        assert_eq!(input.queue.requests().count(), 0);
        input
            .submit(owner, "new".into(), "Drag".into(), None)
            .unwrap();
        input.disconnect(1);
        assert_eq!(input.queue.requests().count(), 0);
        input.register(1, &display_owner(2));
        let owner = input.owner(1, 2).unwrap();
        input
            .submit(owner, "other".into(), "Drag".into(), None)
            .unwrap();
        input.end_display(1);
        assert_eq!(input.queue.requests().count(), 0);
    }

    #[test]
    fn paused_displays_are_not_presented_as_approved() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        input.register(2, &display_owner(2));
        let a = input.owner(1, 1).unwrap();
        let b = input.owner(2, 2).unwrap();
        let id = input
            .submit(a.clone(), "a".into(), "Drag".into(), None)
            .unwrap();
        input.submit(b, "b".into(), "Drag".into(), None).unwrap();
        input.takeover(1, true);
        assert!(input.approve(input.queue.approval()));
        assert_eq!(
            input.status(&a, Some(id), true)["requests"][0]["state"],
            "takeover"
        );
    }
}
