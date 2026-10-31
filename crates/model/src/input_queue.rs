//! The user's real input is shared by every Agent Session. A grant admits
//! new requests too; a loan runs one complete operation at a time.
//! The platform must acknowledge release of a cancelled loan before another
//! loan can begin. Time here is monotonic milliseconds supplied by the caller.

use crate::model::DisplayId;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const IDLE_MS: u64 = 30_000;
pub const MAX_REQUESTS: usize = 256;
/// One Agent Session's share of MAX_REQUESTS, kept results included, so a
/// session that never says done cannot shut out the others.
pub const MAX_SESSION_REQUESTS: usize = 64;
pub type RequestId = u64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Owner {
    pub session: String,
    pub connection: u64,
    pub display: DisplayId,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    User,
    Disconnected,
    DisplayEnded,
    /// The same session asked for something else before this one ran.
    Replaced,
    Takeover,
    /// The owning session cancelled it.
    Withdrawn,
    /// The platform could not run it: a window left the display, or secure
    /// input was on.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    /// The platform still owns input until it acknowledges cleanup.
    Stopping(Stop),
    Stopped(Stop),
    Completed,
    Declined,
}

impl Status {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Stopped(_) | Self::Completed | Self::Declined)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub owner: Owner,
    /// The caller keeps this token across waits and retries.
    pub operation: String,
    pub reason: String,
    pub status: Status,
    /// Whether the agent's call is waiting to perform it now. A call that
    /// gave up waiting leaves its request queued but not runnable.
    pub attached: bool,
}

#[derive(Clone, Debug)]
pub struct Grant {
    pub id: u64,
    pub since_ms: u64,
    pub idle_since_ms: Option<u64>,
    participants: BTreeSet<Owner>,
    done: BTreeSet<Owner>,
}

impl Grant {
    /// The user's approval covers this owner's requests: it was waiting
    /// outside a Takeover at approval, or arrived during the grant.
    pub fn admits(&self, owner: &Owner) -> bool {
        self.participants.contains(owner)
    }
}

/// Kept until the platform has finished restoration or user-revocation
/// cleanup. A subsequent user touch changes restore to false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantEnd {
    pub id: u64,
    pub restore: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueError {
    Full,
    /// This session holds MAX_SESSION_REQUESTS records; done releases them.
    SessionFull,
    Invalid,
    Closed,
}

/// An input_queue call, checked before anything changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Request,
    Wait(RequestId),
    Status(Option<RequestId>),
    Withdraw(RequestId),
    Done,
}

impl Action {
    pub fn parse(name: &str, request: Option<RequestId>) -> Result<Self, &'static str> {
        match (name, request) {
            ("request", _) => Ok(Self::Request),
            ("wait", Some(id)) => Ok(Self::Wait(id)),
            ("status", id) => Ok(Self::Status(id)),
            ("withdraw", Some(id)) => Ok(Self::Withdraw(id)),
            ("wait" | "withdraw", None) => Err("wait and withdraw need a request id"),
            // done forgets the session's results: a request id with it
            // would name a request that is gone once done has succeeded.
            ("done", None) => Ok(Self::Done),
            ("done", Some(_)) => Err(
                "done covers all of this session's requests and clears their results; call it without a request id",
            ),
            _ => Err("action is request, wait, status, withdraw or done"),
        }
    }
}

#[derive(Default, Debug)]
pub struct Queue {
    next: RequestId,
    approval: u64,
    next_owner: u64,
    owners: BTreeMap<DisplayId, Owner>,
    closed: BTreeSet<u64>,
    pending_end: Option<GrantEnd>,
    requests: BTreeMap<RequestId, Request>,
    waiting: VecDeque<RequestId>,
    active: Option<RequestId>,
    grant: Option<Grant>,
    takeovers: BTreeSet<DisplayId>,
}

impl Queue {
    /// Called only by authoritative display creation, never by an input
    /// request. A reused physical display gets a fresh lifetime identity.
    pub fn register(
        &mut self,
        session: String,
        connection: u64,
        display: DisplayId,
    ) -> Option<Owner> {
        if self.closed.contains(&connection) || self.next_owner == u64::MAX {
            return None;
        }
        if let Some(owner) = self.owners.get(&display) {
            return (owner.connection == connection && owner.session == session)
                .then(|| owner.clone());
        }
        self.next_owner += 1;
        let owner = Owner {
            session,
            connection,
            display,
            generation: self.next_owner,
        };
        self.owners.insert(display, owner.clone());
        Some(owner)
    }

    pub fn pending_end(&self) -> Option<GrantEnd> {
        self.pending_end
    }

    /// Called after held-input cleanup and any normal restoration. Until
    /// then a second grant cannot overwrite the saved physical state.
    pub fn acknowledge_end(&mut self, id: u64) -> bool {
        if self.active.is_some() || !self.pending_end.is_some_and(|end| end.id == id) {
            return false;
        }
        self.pending_end = None;
        true
    }

    /// A card carries this revision so its delayed answer cannot approve
    /// again after a revocation or decline.
    pub fn approval(&self) -> u64 {
        self.approval
    }

    pub fn grant(&self) -> Option<&Grant> {
        self.grant.as_ref()
    }

    pub fn request(&self, id: RequestId) -> Option<&Request> {
        self.requests.get(&id)
    }

    pub fn active(&self) -> Option<&Request> {
        self.active.and_then(|id| self.request(id))
    }

    pub fn waiting(&self) -> impl Iterator<Item = &Request> {
        self.waiting.iter().filter_map(|id| self.request(*id))
    }

    /// Every request not yet forgotten, terminal ones included.
    pub fn requests(&self) -> impl Iterator<Item = &Request> {
        self.requests.values()
    }

    pub fn owner(&self, display: DisplayId) -> Option<&Owner> {
        self.owners.get(&display)
    }

    /// The owning session cancels its own request. A queued one stops at
    /// once; a running one keeps its loan until the platform releases held
    /// input and calls finish. Other sessions' grant share is untouched.
    pub fn withdraw(&mut self, id: RequestId, owner: &Owner, now: u64) -> bool {
        let Some(r) = self.requests.get_mut(&id) else {
            return false;
        };
        if r.owner != *owner {
            return false;
        }
        match r.status {
            Status::Queued => {
                r.status = Status::Stopped(Stop::Withdrawn);
                self.waiting.retain(|queued| *queued != id);
                if self.grant.is_none() {
                    self.approval = self.approval.saturating_add(1);
                }
            }
            Status::Running if self.active == Some(id) => {
                r.status = Status::Stopping(Stop::Withdrawn);
            }
            _ => return false,
        }
        self.refresh_idle(now);
        self.tick(now);
        true
    }

    /// A retry returns the newest request with the same token, including a
    /// declined or completed result, so a decline is never silently
    /// reapproved. The one exception: a token whose newest request the same
    /// session replaced or withdrew starts a fresh request. The old one keeps
    /// its stopped result for any call still waiting on its id. The
    /// platform forgets requests only after delivery.
    pub fn enqueue(
        &mut self,
        owner: Owner,
        operation: String,
        reason: String,
        now: u64,
    ) -> Result<RequestId, EnqueueError> {
        if self.owners.get(&owner.display) != Some(&owner) {
            return Err(EnqueueError::Closed);
        }
        if let Some(r) = self
            .requests
            .values()
            .rev()
            .find(|r| r.owner == owner && r.operation == operation)
            .filter(|r| !matches!(r.status, Status::Stopped(Stop::Replaced | Stop::Withdrawn)))
        {
            let id = r.id;
            // Expire first: a retry at or after the idle deadline must not
            // make the old grant busy again.
            self.tick(now);
            if let Some(r) = self.requests.get_mut(&id) {
                r.attached = true;
            }
            self.refresh_idle(now);
            return Ok(id);
        }
        if operation.trim().is_empty() || reason.trim().is_empty() {
            return Err(EnqueueError::Invalid);
        }
        // Expire before admitting a late arrival to an old grant.
        self.tick(now);
        // The session's own cap first, so the session that fills it is told
        // what to clear.
        let held = self
            .requests
            .values()
            .filter(|r| r.owner.connection == owner.connection)
            .count();
        if held >= MAX_SESSION_REQUESTS {
            return Err(EnqueueError::SessionFull);
        }
        if self.requests.len() >= MAX_REQUESTS || self.next == u64::MAX {
            return Err(EnqueueError::Full);
        }
        self.next += 1;
        let id = self.next;
        // An agent waits on one action at a time: an older queued action
        // of the same session would otherwise run later, out of context.
        let requests = &mut self.requests;
        self.waiting.retain(|queued| {
            let Some(r) = requests.get_mut(queued) else {
                return false;
            };
            if r.owner == owner {
                r.status = Status::Stopped(Stop::Replaced);
                false
            } else {
                true
            }
        });
        if !self.takeovers.contains(&owner.display)
            && let Some(grant) = self.grant.as_mut()
        {
            grant.participants.insert(owner.clone());
            grant.done.remove(&owner);
        }
        self.requests.insert(
            id,
            Request {
                id,
                owner,
                operation,
                reason,
                status: Status::Queued,
                attached: true,
            },
        );
        self.waiting.push_back(id);
        self.refresh_idle(now);
        Ok(id)
    }

    /// Only the user's approval calls this. Cleanup of an interrupted loan
    /// must finish first, even if the user immediately approves again.
    pub fn allow(&mut self, approval: u64, now: u64) -> bool {
        if approval != self.approval
            || self.approval == u64::MAX
            || self.grant.is_some()
            || self.pending_end.is_some()
            || self.active.is_some()
            || !self.has_ready()
        {
            return false;
        }
        let participants = self
            .waiting()
            .filter(|r| !self.takeovers.contains(&r.owner.display))
            .map(|r| r.owner.clone())
            .collect();
        self.approval += 1;
        self.grant = Some(Grant {
            id: self.approval,
            since_ms: now,
            idle_since_ms: None,
            participants,
            done: BTreeSet::new(),
        });
        true
    }

    /// FIFO among eligible requests. A Takeover cannot block other sessions.
    pub fn start(&mut self, now: u64) -> Option<RequestId> {
        self.tick(now);
        if self.grant.is_none() || self.active.is_some() {
            return None;
        }
        let index = self
            .waiting
            .iter()
            .position(|id| self.requests.get(id).is_some_and(|r| self.runnable(r)))?;
        let id = self.waiting.remove(index)?;
        let r = self.requests.get_mut(&id)?;
        r.status = Status::Running;
        let grant = self.grant.as_mut()?;
        grant.participants.insert(r.owner.clone());
        grant.done.remove(&r.owner);
        grant.idle_since_ms = None;
        self.active = Some(id);
        Some(id)
    }

    /// Whether `start` would hand out a loan now.
    pub fn ready(&self) -> bool {
        self.grant.is_some()
            && self.active.is_none()
            && self.pending_end.is_none()
            && self.waiting().any(|r| self.runnable(r))
    }

    /// Checked before each real event, not just at the beginning of a drag.
    pub fn may_send(&self, id: RequestId, owner: &Owner) -> bool {
        self.grant.is_some()
            && self.owners.get(&owner.display) == Some(owner)
            && self.active == Some(id)
            && self.request(id).is_some_and(|r| {
                r.owner == *owner
                    && r.status == Status::Running
                    && !self.takeovers.contains(&owner.display)
            })
    }

    /// Called after the platform has released the operation's held inputs.
    /// Stale completions cannot release another session's loan.
    pub fn finish(&mut self, id: RequestId, now: u64) -> bool {
        if self.active != Some(id) {
            return false;
        }
        let Some(r) = self.requests.get_mut(&id) else {
            return false;
        };
        r.status = match r.status {
            Status::Stopping(why) => Status::Stopped(why),
            Status::Running => Status::Completed,
            _ => return false,
        };
        self.active = None;
        self.refresh_idle(now);
        self.tick(now);
        true
    }

    /// Physical input revokes the whole grant but leaves the other requests
    /// queued for a new approval. The platform must not restore the pointer
    /// or front app after this user-initiated revocation.
    pub fn user_input(&mut self) {
        self.approval = self.approval.saturating_add(1);
        self.end_grant(false);
        if let Some(end) = self.pending_end.as_mut() {
            end.restore = false;
        }
        self.stop_active(Stop::User);
    }

    /// Not Now declines the waiting batch; it is not a timeout.
    pub fn decline(&mut self, approval: u64) {
        if approval != self.approval || self.grant.is_some() {
            return;
        }
        self.approval = self.approval.saturating_add(1);
        for id in self.waiting.drain(..) {
            if let Some(r) = self.requests.get_mut(&id) {
                r.status = Status::Declined;
            }
        }
    }

    pub fn done(&mut self, owner: &Owner, now: u64) {
        if let Some(grant) = self.grant.as_mut()
            && grant.participants.contains(owner)
        {
            grant.done.insert(owner.clone());
        }
        self.tick(now);
    }

    pub fn takeover(&mut self, display: DisplayId, user: bool, now: u64) {
        if user {
            self.takeovers.insert(display);
            if self.active().is_some_and(|r| r.owner.display == display) {
                self.stop_active(Stop::Takeover);
            }
        } else {
            // Resuming after the idle deadline doesn't revive the grant.
            self.tick(now);
            self.takeovers.remove(&display);
        }
        self.refresh_idle(now);
        self.tick(now);
    }

    pub fn disconnect(&mut self, connection: u64, now: u64) {
        self.closed.insert(connection);
        self.owners
            .retain(|_, owner| owner.connection != connection);
        self.remove_owners(
            |owner| owner.connection == connection,
            Stop::Disconnected,
            now,
        );
    }

    pub fn end_display(&mut self, display: DisplayId, now: u64) {
        self.owners.remove(&display);
        self.remove_owners(|owner| owner.display == display, Stop::DisplayEnded, now);
        self.takeovers.remove(&display);
    }

    /// Returns true when this call ends the grant normally. The durable
    /// pending_end carries the restoration obligation even when another
    /// queue method called tick internally.
    pub fn tick(&mut self, now: u64) -> bool {
        self.refresh_idle(now);
        let expired = self.grant.as_ref().is_some_and(|grant| {
            grant.idle_since_ms.is_some_and(|idle| {
                now.saturating_sub(idle) >= IDLE_MS || grant.participants.is_subset(&grant.done)
            })
        });
        if expired {
            self.end_grant(true);
        }
        expired
    }

    pub fn forget(&mut self, id: RequestId) -> bool {
        if !self.request(id).is_some_and(|r| r.status.terminal()) {
            return false;
        }
        self.requests.remove(&id);
        true
    }

    fn end_grant(&mut self, restore: bool) {
        if let Some(grant) = self.grant.take() {
            self.pending_end = Some(GrantEnd {
                id: grant.id,
                restore,
            });
        }
    }

    fn has_ready(&self) -> bool {
        self.waiting()
            .any(|r| !self.takeovers.contains(&r.owner.display))
    }

    /// The call that asked for it is waiting, and its display is not in
    /// a Takeover.
    fn runnable(&self, r: &Request) -> bool {
        r.status == Status::Queued && r.attached && !self.takeovers.contains(&r.owner.display)
    }

    /// The agent's call waits for it again.
    pub fn attach(&mut self, id: RequestId, now: u64) {
        if let Some(r) = self.requests.get_mut(&id)
            && r.status == Status::Queued
        {
            r.attached = true;
        }
        self.refresh_idle(now);
    }

    /// The running loan could not be carried out. `finish` then reports it
    /// stopped, not completed.
    pub fn fail(&mut self, id: RequestId) {
        if self.active == Some(id) {
            self.stop_active(Stop::Failed);
        }
    }

    /// The agent's call stopped waiting; the request keeps its place.
    pub fn detach(&mut self, id: RequestId, now: u64) {
        if let Some(r) = self.requests.get_mut(&id)
            && r.status == Status::Queued
        {
            r.attached = false;
        }
        self.refresh_idle(now);
    }

    fn refresh_idle(&mut self, now: u64) {
        let busy = self.active.is_some() || self.waiting().any(|r| self.runnable(r));
        if let Some(grant) = self.grant.as_mut() {
            if busy {
                grant.idle_since_ms = None;
            } else {
                grant.idle_since_ms.get_or_insert(now);
            }
        }
    }

    fn stop_active(&mut self, why: Stop) {
        if let Some(id) = self.active
            && let Some(r) = self.requests.get_mut(&id)
            && (r.status == Status::Running
                || (why == Stop::User && matches!(r.status, Status::Stopping(_))))
        {
            r.status = Status::Stopping(why);
        }
    }

    fn remove_owners(&mut self, matches: impl Fn(&Owner) -> bool, why: Stop, now: u64) {
        if self.active().is_some_and(|r| matches(&r.owner)) {
            self.stop_active(why);
        }
        let waiting = self.waiting.len();
        self.waiting.retain(|id| {
            let Some(r) = self.requests.get_mut(id) else {
                return false;
            };
            if matches(&r.owner) {
                r.status = Status::Stopped(why);
                false
            } else {
                true
            }
        });
        if self.grant.is_none() && self.waiting.len() != waiting {
            self.approval = self.approval.saturating_add(1);
        }
        if let Some(grant) = self.grant.as_mut() {
            grant.participants.retain(|owner| !matches(owner));
            grant.done.retain(|owner| !matches(owner));
        }
        self.tick(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(session: &str, display: DisplayId) -> Owner {
        Owner {
            session: session.into(),
            connection: session.bytes().map(u64::from).sum(),
            display,
            generation: display.into(),
        }
    }

    fn queue() -> Queue {
        let mut q = Queue::default();
        for (session, display) in [("a", 1), ("b", 2), ("c", 3)] {
            assert_eq!(
                q.register(session.into(), owner(session, display).connection, display),
                Some(owner(session, display))
            );
        }
        q
    }

    fn restored(q: &mut Queue) {
        let end = q.pending_end().unwrap();
        assert!(q.acknowledge_end(end.id));
    }

    fn add(q: &mut Queue, session: &str, display: DisplayId, op: &str, now: u64) -> RequestId {
        q.enqueue(
            owner(session, display),
            op.into(),
            "Drag the divider".into(),
            now,
        )
        .unwrap()
    }

    #[test]
    fn sessions_share_one_fifo_and_new_arrivals_share_the_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        assert!(!q.may_send(a, &owner("a", 1)));
        assert_eq!(q.start(0), None);
        assert!(q.allow(q.approval(), 1));
        assert_eq!(q.start(2), Some(a));
        let c = add(&mut q, "c", 3, "drag", 3);
        assert_eq!(q.start(3), None);
        assert!(!q.may_send(a, &owner("b", 2)));
        assert!(q.finish(a, 4));
        assert_eq!(q.start(4), Some(b));
        assert!(!q.finish(a, 5));
        assert!(q.may_send(b, &owner("b", 2)));
        assert!(q.finish(b, 6));
        assert_eq!(q.start(6), Some(c));
    }

    #[test]
    fn idle_grant_waits_thirty_seconds_and_late_arrivals_need_approval() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.finish(a, 10);
        assert!(!q.tick(30_009));
        assert!(q.grant().is_some());
        let b = add(&mut q, "b", 2, "drag", 30_010);
        assert!(q.grant().is_none());
        assert_eq!(q.start(30_010), None);
        restored(&mut q);
        assert!(q.allow(q.approval(), 30_011));
        assert_eq!(q.start(30_011), Some(b));
    }

    #[test]
    fn arrival_before_deadline_keeps_the_same_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.finish(a, 1);
        let b = add(&mut q, "b", 2, "drag", 20_000);
        assert_eq!(q.grant().unwrap().since_ms, 0);
        assert_eq!(q.start(40_000), Some(b));
    }

    #[test]
    fn user_revokes_and_cleanup_must_finish_before_any_new_loan() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.user_input();
        assert!(!q.may_send(a, &owner("a", 1)));
        assert_eq!(q.request(a).unwrap().status, Status::Stopping(Stop::User));
        assert!(!q.allow(q.approval(), 1));
        assert_eq!(q.start(1), None);
        assert!(q.finish(a, 2));
        assert_eq!(q.request(a).unwrap().status, Status::Stopped(Stop::User));
        assert_eq!(q.start(2), None);
        restored(&mut q);
        assert!(q.allow(q.approval(), 3));
        assert_eq!(q.start(3), Some(b));
    }

    #[test]
    fn completion_by_one_session_does_not_end_anothers_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.finish(a, 1);
        q.done(&owner("a", 1), 2);
        assert!(q.grant().is_some());
        q.start(2);
        q.finish(b, 3);
        q.done(&owner("unrelated", 9), 4);
        assert!(q.grant().is_some());
        q.done(&owner("b", 2), 5);
        assert!(q.grant().is_none());
    }

    #[test]
    fn takeovers_wait_without_blocking_other_sessions() {
        let mut q = queue();
        q.takeover(1, true, 0);
        let a = add(&mut q, "a", 1, "drag", 0);
        assert!(!q.allow(q.approval(), 0));
        let b = add(&mut q, "b", 2, "drag", 0);
        assert!(q.allow(q.approval(), 0));
        assert_eq!(q.start(0), Some(b));
        q.finish(b, 1);
        q.takeover(1, false, 2);
        assert_eq!(q.start(2), Some(a));
        q.takeover(1, true, 3);
        assert!(!q.may_send(a, &owner("a", 1)));
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopping(Stop::Takeover)
        );
    }

    #[test]
    fn disconnect_cancels_only_its_owner_and_fences_the_active_loan() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        let a2 = add(&mut q, "a", 1, "drag2", 0);
        q.disconnect(owner("a", 1).connection, 1);
        assert_eq!(
            q.request(a2).unwrap().status,
            Status::Stopped(Stop::Disconnected)
        );
        assert_eq!(q.start(2), None);
        assert!(!q.may_send(a, &owner("a", 1)));
        q.finish(a, 3);
        assert_eq!(q.start(3), Some(b));
    }

    #[test]
    fn end_display_cancels_queued_work_and_ends_an_empty_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.end_display(1, 1);
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopped(Stop::DisplayEnded)
        );
        assert!(q.grant().is_none());
    }

    #[test]
    fn retries_keep_their_place_and_their_result() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        assert_eq!(add(&mut q, "a", 1, "drag", 60_000), a);
        assert_eq!(q.waiting().count(), 1);
        assert!(!q.forget(a));
        q.decline(q.approval());
        assert_eq!(add(&mut q, "a", 1, "drag", 60_001), a);
        assert_eq!(q.request(a).unwrap().status, Status::Declined);
        assert_eq!(q.waiting().count(), 0);
        assert!(q.forget(a));
    }

    #[test]
    fn a_replaced_or_withdrawn_token_is_requested_afresh_and_keeps_its_history() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "A", 0);
        let b = add(&mut q, "a", 1, "B", 1);
        let a2 = add(&mut q, "a", 1, "A", 2);
        assert_ne!(a2, a, "A->B->A must not return the replaced A");
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopped(Stop::Replaced)
        );
        assert_eq!(
            q.request(b).unwrap().status,
            Status::Stopped(Stop::Replaced)
        );
        assert_eq!(q.waiting().map(|r| r.id).collect::<Vec<_>>(), vec![a2]);
        assert_eq!(
            add(&mut q, "a", 1, "A", 3),
            a2,
            "a retry finds the newest A"
        );
        assert!(q.withdraw(a2, &owner("a", 1), 4));
        let a3 = add(&mut q, "a", 1, "A", 5);
        assert_ne!(a3, a2);
        assert_eq!(
            q.request(a2).unwrap().status,
            Status::Stopped(Stop::Withdrawn)
        );
        assert_eq!(add(&mut q, "a", 1, "A", 6), a3);
        assert_eq!(q.waiting().map(|r| r.id).collect::<Vec<_>>(), vec![a3]);
    }

    #[test]
    fn declined_and_completed_tokens_stay_sticky_after_another_request() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "A", 0);
        q.decline(q.approval());
        add(&mut q, "a", 1, "B", 1);
        assert_eq!(add(&mut q, "a", 1, "A", 2), a);
        assert_eq!(q.request(a).unwrap().status, Status::Declined);
        q.decline(q.approval());
        let c = add(&mut q, "b", 2, "C", 3);
        assert!(q.allow(q.approval(), 3));
        assert_eq!(q.start(3), Some(c));
        assert!(q.finish(c, 4));
        add(&mut q, "b", 2, "D", 5);
        assert_eq!(add(&mut q, "b", 2, "C", 6), c);
        assert_eq!(q.request(c).unwrap().status, Status::Completed);
    }

    #[test]
    fn one_session_reaches_its_own_cap_without_filling_the_queue() {
        let mut q = queue();
        let mut last = 0;
        for n in 0..MAX_SESSION_REQUESTS {
            last = add(&mut q, "a", 1, &format!("drag{n}"), 0);
        }
        assert_eq!(
            q.enqueue(owner("a", 1), "more".into(), "Drag".into(), 0),
            Err(EnqueueError::SessionFull)
        );
        // Retrying a request it still has needs no new room.
        assert_eq!(
            add(
                &mut q,
                "a",
                1,
                &format!("drag{}", MAX_SESSION_REQUESTS - 1),
                0
            ),
            last
        );
        assert!(
            q.enqueue(owner("b", 2), "drag".into(), "Drag".into(), 0)
                .is_ok()
        );
        assert_eq!(
            q.request(1).unwrap().status,
            Status::Stopped(Stop::Replaced)
        );
        // Releasing its kept results makes room again.
        for id in 1..last {
            assert!(q.forget(id));
        }
        assert!(add(&mut q, "a", 1, "after", 1) > last);
    }

    #[test]
    fn done_takes_no_request_id_and_wait_and_withdraw_need_one() {
        // done forgets the session's results: a request id after it would
        // name a request that no longer exists.
        assert!(Action::parse("done", Some(3)).is_err());
        assert_eq!(Action::parse("done", None), Ok(Action::Done));
        assert_eq!(Action::parse("wait", Some(3)), Ok(Action::Wait(3)));
        assert!(Action::parse("wait", None).is_err());
        assert_eq!(Action::parse("withdraw", Some(3)), Ok(Action::Withdraw(3)));
        assert!(Action::parse("withdraw", None).is_err());
        assert_eq!(Action::parse("status", None), Ok(Action::Status(None)));
        assert_eq!(
            Action::parse("status", Some(3)),
            Ok(Action::Status(Some(3)))
        );
        assert_eq!(Action::parse("request", None), Ok(Action::Request));
        assert!(Action::parse("cancel", Some(3)).is_err());
    }

    #[test]
    fn active_operations_have_no_duration_limit() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        assert!(!q.tick(u64::MAX));
        assert!(q.may_send(a, &owner("a", 1)));
    }

    #[test]
    fn done_does_not_cover_a_later_operation_from_the_same_session() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.finish(a, 1);
        q.done(&owner("a", 1), 2);
        let a2 = add(&mut q, "a", 1, "drag2", 3);
        q.start(4);
        q.finish(b, 5);
        q.done(&owner("b", 2), 6);
        assert_eq!(q.start(7), Some(a2));
        q.finish(a2, 8);
        assert!(q.grant().is_some());
        q.done(&owner("a", 1), 9);
        assert!(q.grant().is_none());
    }

    #[test]
    fn a_takeover_hand_back_after_the_deadline_needs_new_approval() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.takeover(1, true, 1);
        q.takeover(1, false, IDLE_MS + 1);
        assert!(q.grant().is_none());
        assert_eq!(q.start(IDLE_MS + 1), None);
        restored(&mut q);
        assert!(q.allow(q.approval(), IDLE_MS + 2));
        assert_eq!(q.start(IDLE_MS + 2), Some(a));
    }

    #[test]
    fn request_limit_does_not_prevent_polling_or_cancellation() {
        let mut q = queue();
        for (session, display) in [("d", 4), ("e", 5)] {
            assert!(
                q.register(session.into(), owner(session, display).connection, display)
                    .is_some()
            );
        }
        for (session, display) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
            for n in 0..MAX_SESSION_REQUESTS {
                add(&mut q, session, display, &format!("drag{n}"), 0);
            }
        }
        assert_eq!(q.requests().count(), MAX_REQUESTS);
        assert_eq!(
            q.enqueue(owner("e", 5), "drag".into(), "Drag".into(), 0),
            Err(EnqueueError::Full)
        );
        assert_eq!(
            q.enqueue(owner("a", 1), "more".into(), "Drag".into(), 0),
            Err(EnqueueError::SessionFull)
        );
        let newest = MAX_SESSION_REQUESTS as u64;
        assert_eq!(
            add(&mut q, "a", 1, &format!("drag{}", newest - 1), 0),
            newest
        );
        q.disconnect(owner("a", 1).connection, 1);
        assert_eq!(q.waiting().count(), 3);
        for id in 1..=newest {
            assert!(q.forget(id));
        }
        assert!(
            q.enqueue(owner("e", 5), "drag".into(), "Drag".into(), 2)
                .is_ok()
        );
    }

    #[test]
    fn interleaved_transitions_never_make_two_loans_or_revive_a_stopped_one() {
        let mut q = queue();
        let mut seed = 73_u64;
        for now in 0..20_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let display = (seed % 2 + 1) as u32;
            let who = q.owners.get(&display).cloned().unwrap_or_else(|| {
                q.register(
                    if seed & 1 == 0 {
                        "a".into()
                    } else {
                        "b".into()
                    },
                    now + 10_000,
                    display,
                )
                .unwrap()
            });
            match (seed >> 32) % 10 {
                0 => {
                    let _ = q.enqueue(who.clone(), format!("op{}", seed % 16), "Drag".into(), now);
                }
                1 => {
                    q.allow(q.approval(), now);
                }
                2 => {
                    q.start(now);
                }
                3 => {
                    if let Some(r) = q.active() {
                        q.finish(r.id, now);
                    }
                }
                4 => q.user_input(),
                5 => q.done(&who, now),
                6 => q.takeover(who.display, seed & 4 != 0, now),
                7 => q.disconnect(who.connection, now),
                8 => {
                    if seed & 8 != 0 {
                        q.decline(q.approval());
                    } else if let Some(id) = q.requests.keys().next().copied() {
                        q.withdraw(id, &who, now);
                    }
                }
                _ => {
                    q.tick(now);
                    if let Some(end) = q.pending_end() {
                        q.acknowledge_end(end.id);
                    }
                }
            }
            let running: Vec<_> = q
                .requests
                .values()
                .filter(|r| matches!(r.status, Status::Running | Status::Stopping(_)))
                .collect();
            assert!(running.len() <= 1);
            assert_eq!(running.first().map(|r| r.id), q.active);
            for r in q.requests.values() {
                assert_eq!(q.waiting.contains(&r.id), r.status == Status::Queued);
                if q.may_send(r.id, &r.owner) {
                    assert_eq!(q.active, Some(r.id));
                    assert_eq!(r.status, Status::Running);
                    assert!(q.grant().is_some());
                }
            }
            let terminal: Vec<_> = q
                .requests
                .values()
                .filter(|r| r.status.terminal())
                .map(|r| r.id)
                .collect();
            for id in terminal {
                q.forget(id);
            }
        }
    }

    #[test]
    fn old_card_answers_cannot_reapprove_or_decline_after_revocation() {
        let mut q = queue();
        add(&mut q, "a", 1, "drag", 0);
        let card = q.approval();
        assert!(q.allow(card, 0));
        q.user_input();
        assert!(!q.allow(card, 1));
        q.decline(card);
        assert_eq!(q.waiting().count(), 1);
        restored(&mut q);
        assert!(q.allow(q.approval(), 2));
    }

    #[test]
    fn a_delayed_allow_after_not_now_cannot_approve_new_work() {
        let mut q = queue();
        add(&mut q, "a", 1, "drag", 0);
        let card = q.approval();
        q.decline(card);
        add(&mut q, "b", 2, "drag", 1);
        assert!(!q.allow(card, 2));
        assert!(q.allow(q.approval(), 2));
    }

    #[test]
    fn reconnecting_a_session_does_not_inherit_its_old_requests() {
        let mut q = queue();
        let before = owner("a", 1);
        let old = add(&mut q, "a", 1, "drag", 0);
        q.disconnect(before.connection, 1);
        let after = q.register("a".into(), before.connection + 100, 1).unwrap();
        let new = q
            .enqueue(after.clone(), "drag".into(), "Drag".into(), 2)
            .unwrap();
        assert_ne!(old, new);
        assert_eq!(
            q.request(old).unwrap().status,
            Status::Stopped(Stop::Disconnected)
        );
        assert_eq!(
            q.enqueue(before.clone(), "late".into(), "Drag".into(), 3),
            Err(EnqueueError::Closed)
        );
        q.allow(q.approval(), 3);
        assert_eq!(q.start(3), Some(new));
        assert!(!q.may_send(new, &before));
        assert!(q.may_send(new, &after));
    }

    #[test]
    fn restoration_is_durable_when_a_late_enqueue_expires_the_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.finish(a, 1);
        add(&mut q, "b", 2, "drag", IDLE_MS + 1);
        assert!(!q.tick(IDLE_MS + 2));
        let end = q.pending_end().unwrap();
        assert!(end.restore);
        assert!(!q.allow(q.approval(), IDLE_MS + 2));
        assert!(!q.acknowledge_end(end.id + 1));
        assert_eq!(q.pending_end(), Some(end));
        assert!(q.acknowledge_end(end.id));
        assert!(q.allow(q.approval(), IDLE_MS + 3));
    }

    #[test]
    fn user_touch_overrides_a_pending_normal_restore_and_existing_cancellation() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.disconnect(owner("a", 1).connection, 1);
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopping(Stop::Disconnected)
        );
        q.user_input();
        let end = q.pending_end().unwrap();
        assert!(!end.restore);
        assert_eq!(q.request(a).unwrap().status, Status::Stopping(Stop::User));
        assert!(!q.acknowledge_end(end.id));
        q.finish(a, 2);
        assert_eq!(q.request(a).unwrap().status, Status::Stopped(Stop::User));
        assert_eq!(q.pending_end(), Some(end));
        restored(&mut q);
        let b = add(&mut q, "b", 2, "drag", 3);
        q.allow(q.approval(), 3);
        q.start(3);
        q.finish(b, 4);
        q.done(&owner("b", 2), 5);
        assert!(q.pending_end().unwrap().restore);
        q.user_input();
        assert!(!q.pending_end().unwrap().restore);
    }

    #[test]
    fn a_reused_display_rejects_late_work_from_its_previous_lifetime() {
        let mut q = queue();
        let before = owner("a", 1);
        q.end_display(1, 0);
        assert_eq!(
            q.enqueue(before.clone(), "late".into(), "Drag".into(), 1),
            Err(EnqueueError::Closed)
        );
        let after = q
            .register(before.session.clone(), before.connection, 1)
            .unwrap();
        assert_ne!(before, after);
        assert_eq!(
            q.enqueue(before.clone(), "later".into(), "Drag".into(), 2),
            Err(EnqueueError::Closed)
        );
        assert!(
            q.enqueue(after.clone(), "new".into(), "Drag".into(), 2)
                .is_ok()
        );
        q.disconnect(after.connection, 3);
        assert!(
            q.register(after.session.clone(), after.connection, 1)
                .is_none()
        );
    }

    #[test]
    fn a_new_action_replaces_the_same_sessions_queued_one() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        let a2 = add(&mut q, "a", 1, "drag2", 1);
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopped(Stop::Replaced)
        );
        assert_eq!(q.waiting().map(|r| r.id).collect::<Vec<_>>(), vec![b, a2]);
    }

    #[test]
    fn a_detached_request_waits_without_running_or_keeping_the_grant_busy() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.detach(a, 1);
        assert!(q.allow(q.approval(), 1));
        assert_eq!(q.start(1), Some(b));
        q.finish(b, 2);
        assert_eq!(q.start(3), None);
        assert!(q.tick(2 + IDLE_MS));
        restored(&mut q);
        assert_eq!(add(&mut q, "a", 1, "drag", 2 + IDLE_MS), a);
        assert!(q.allow(q.approval(), 3 + IDLE_MS));
        assert_eq!(q.start(3 + IDLE_MS), Some(a));
    }

    #[test]
    fn a_card_from_a_withdrawn_batch_cannot_approve_a_later_request() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let card = q.approval();
        assert!(q.withdraw(a, &owner("a", 1), 1));
        add(&mut q, "b", 2, "drag", 2);
        assert!(!q.allow(card, 3));
        assert!(q.allow(q.approval(), 3));
    }

    #[test]
    fn a_card_from_an_ended_owner_cannot_approve_a_later_request() {
        for disconnected in [false, true] {
            let mut q = queue();
            add(&mut q, "a", 1, "drag", 0);
            let card = q.approval();
            if disconnected {
                q.disconnect(owner("a", 1).connection, 1);
            } else {
                q.end_display(1, 1);
            }
            add(&mut q, "b", 2, "drag", 2);
            assert!(!q.allow(card, 3));
        }
    }

    #[test]
    fn owner_and_requests_expose_registration_and_every_request() {
        let mut q = queue();
        assert_eq!(q.owner(1), Some(&owner("a", 1)));
        assert_eq!(q.owner(9), None);
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.decline(q.approval());
        assert_eq!(q.requests().map(|r| r.id).collect::<Vec<_>>(), vec![a, b]);
        q.end_display(1, 1);
        assert_eq!(q.owner(1), None);
    }

    #[test]
    fn withdraw_rejects_another_owner_and_a_previous_lifetime() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        assert!(!q.withdraw(a, &owner("b", 2), 1));
        assert!(!q.withdraw(a + 100, &owner("a", 1), 1));
        q.end_display(1, 1);
        let after = q.register("a".into(), owner("a", 1).connection, 1).unwrap();
        let new = q
            .enqueue(after.clone(), "drag".into(), "Drag".into(), 2)
            .unwrap();
        assert!(!q.withdraw(new, &owner("a", 1), 3));
        assert_eq!(q.request(new).unwrap().status, Status::Queued);
        assert!(q.withdraw(new, &after, 3));
    }

    #[test]
    fn withdrawing_a_queued_request_stops_it_and_keeps_others_queued() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        assert!(q.allow(q.approval(), 0));
        assert!(q.withdraw(a, &owner("a", 1), 1));
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopped(Stop::Withdrawn)
        );
        assert!(!q.withdraw(a, &owner("a", 1), 1));
        assert_eq!(q.waiting().map(|r| r.id).collect::<Vec<_>>(), vec![b]);
        assert!(q.grant().is_some());
        assert_eq!(q.start(2), Some(b));
        assert!(q.forget(a));
    }

    #[test]
    fn withdrawing_a_running_request_keeps_the_cleanup_fence() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        assert_eq!(q.start(0), Some(a));
        assert!(!q.withdraw(a, &owner("b", 2), 1));
        assert!(q.may_send(a, &owner("a", 1)));
        assert!(q.withdraw(a, &owner("a", 1), 1));
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopping(Stop::Withdrawn)
        );
        assert!(!q.may_send(a, &owner("a", 1)));
        assert_eq!(q.active().map(|r| r.id), Some(a));
        assert_eq!(q.start(2), None);
        assert!(!q.forget(a));
        assert!(!q.withdraw(a, &owner("a", 1), 2));
        assert!(q.finish(a, 3));
        assert_eq!(
            q.request(a).unwrap().status,
            Status::Stopped(Stop::Withdrawn)
        );
        // b's share of the grant survives a's withdrawal.
        assert!(q.grant().is_some());
        assert_eq!(q.start(3), Some(b));
    }

    #[test]
    fn withdrawing_the_last_request_leaves_the_grant_to_idle_normally() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.finish(a, 1);
        q.done(&owner("a", 1), 1);
        assert!(q.withdraw(b, &owner("b", 2), 2));
        assert!(q.grant().is_some());
        assert!(!q.tick(2 + IDLE_MS - 1));
        assert!(q.tick(2 + IDLE_MS));
        assert!(q.pending_end().unwrap().restore);
    }

    #[test]
    fn an_attached_retry_at_the_idle_deadline_does_not_revive_the_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.detach(b, 0);
        q.finish(a, 1);
        assert_eq!(add(&mut q, "b", 2, "drag", 1 + IDLE_MS), b);
        assert!(q.grant().is_none());
        assert_eq!(q.start(1 + IDLE_MS), None);
        assert!(q.request(b).unwrap().attached);
        restored(&mut q);
        assert!(q.allow(q.approval(), 2 + IDLE_MS));
        assert_eq!(q.start(2 + IDLE_MS), Some(b));
    }

    #[test]
    fn an_attached_retry_before_the_deadline_keeps_the_grant() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        let b = add(&mut q, "b", 2, "drag", 0);
        q.allow(q.approval(), 0);
        q.start(0);
        q.detach(b, 0);
        q.finish(a, 1);
        assert_eq!(add(&mut q, "b", 2, "drag", IDLE_MS), b);
        assert!(q.grant().is_some());
        assert_eq!(q.start(IDLE_MS + 5), Some(b));
    }

    #[test]
    fn a_busy_grant_cannot_be_declined_by_a_stale_card() {
        let mut q = queue();
        let a = add(&mut q, "a", 1, "drag", 0);
        q.allow(q.approval(), 0);
        q.decline(q.approval());
        assert_eq!(q.start(1), Some(a));
    }

    #[test]
    fn a_failed_loan_is_stopped_not_completed_and_frees_the_slot() {
        let mut q = Queue::default();
        let a = q.register("a".into(), 1, 1).unwrap();
        let id = q
            .enqueue(a.clone(), "drag".into(), "Drag".into(), 0)
            .unwrap();
        q.detach(id, 0);
        assert!(q.allow(q.approval(), 0));
        assert_eq!(q.start(1), None, "a detached request does not run");
        q.attach(id, 1);
        assert_eq!(q.start(1), Some(id));
        q.fail(id);
        assert!(q.finish(id, 2));
        assert_eq!(q.request(id).unwrap().status, Status::Stopped(Stop::Failed));
        assert!(q.active().is_none());
    }
}
