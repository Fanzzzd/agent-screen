//! App-wide admission and presentation for Input Requests. No input is sent
//! here. This build has no real-input executor: approval remains a distinct
//! state and must never be reported as a completed action.

use crate::model::{DisplayId, Owner as DisplayOwner};
use agent_screen_model::input_queue::{EnqueueError, Owner, Queue, RequestId, Status};
use serde_json::{Value, json};
use std::time::Instant;

pub const EXECUTOR_AVAILABLE: bool = false;
const REASON_CHARS: usize = 300;
const OPERATION_CHARS: usize = 2048;

pub struct InputQueue {
    pub queue: Queue,
    clock: Instant,
}

impl Default for InputQueue {
    fn default() -> Self {
        Self {
            queue: Queue::default(),
            clock: Instant::now(),
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
                EnqueueError::Invalid => "operation and reason are required",
                EnqueueError::Closed => "the queue's display lifetime has ended",
            }.to_string()
        })?;
        // A future executor attaches only while it is ready to run the
        // operation. An unavailable executor must not hold a grant forever.
        self.queue.detach(id, now);
        self.tick();
        Ok(id)
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
        self.queue.tick(self.now());
        // No physical state was acquired in this build. A future executor
        // must own this acknowledgment after cleanup/restoration instead.
        if !EXECUTOR_AVAILABLE && let Some(end) = self.queue.pending_end() {
            self.queue.acknowledge_end(end.id);
        }
        was.is_some() || self.queue.grant().is_some()
    }

    pub fn approval_needed(&self) -> bool {
        self.queue.grant().is_none() && self.queue.waiting().next().is_some()
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
                    Status::Queued if self.queue.grant().is_some() => "approved_waiting_executor",
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
                    "stopReason": match request.status {
                        Status::Stopped(why) | Status::Stopping(why) => Some(format!("{why:?}")),
                        _ => None,
                    },
                })
            })
            .collect();
        json!({
            "executorAvailable": EXECUTOR_AVAILABLE,
            "grantActive": self.queue.grant().is_some(),
            "elapsedSeconds": self.elapsed_seconds(),
            "waiting": self.queue.waiting().count(),
            "requests": requests,
            "note": "Queue approval is not action completion. This build has no real-input executor; no pointer, key or drag is sent by this queue. Do not retry the action assuming it ran.",
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
            .submit(owner.clone(), "drag".into(), "Move a card".into())
            .unwrap();
        assert!(input.approve(input.queue.approval()));
        let state = input.status(&owner, Some(id), false);
        assert_eq!(state["executorAvailable"], false);
        assert_eq!(state["requests"][0]["state"], "approved_waiting_executor");
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
            .submit(a.clone(), "drag".into(), "Private reason".into())
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
        assert!(input.submit(old, "late".into(), "Drag".into()).is_err());
        assert!(input.owner(1, 2).is_ok());
    }

    #[test]
    fn done_and_closed_owners_do_not_permanently_fill_the_queue() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        let owner = input.owner(1, 1).unwrap();
        for i in 0..agent_screen_model::input_queue::MAX_REQUESTS {
            input
                .submit(owner.clone(), format!("drag{i}"), "Drag".into())
                .unwrap();
        }
        assert!(
            input
                .submit(owner.clone(), "overflow".into(), "Drag".into())
                .is_err()
        );
        input.done(&owner);
        assert_eq!(input.queue.requests().count(), 0);
        input.submit(owner, "new".into(), "Drag".into()).unwrap();
        input.disconnect(1);
        assert_eq!(input.queue.requests().count(), 0);
        input.register(1, &display_owner(2));
        let owner = input.owner(1, 2).unwrap();
        input.submit(owner, "other".into(), "Drag".into()).unwrap();
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
        let id = input.submit(a.clone(), "a".into(), "Drag".into()).unwrap();
        input.submit(b, "b".into(), "Drag".into()).unwrap();
        input.takeover(1, true);
        assert!(input.approve(input.queue.approval()));
        assert_eq!(
            input.status(&a, Some(id), true)["requests"][0]["state"],
            "takeover"
        );
    }
}
