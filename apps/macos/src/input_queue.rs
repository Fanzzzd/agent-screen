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

/// A request's times on the wall clock, in milliseconds since 1970.
#[derive(Clone, Copy, Debug, Default)]
struct Walls {
    asked: Option<u64>,
    allowed: Option<u64>,
    started: Option<u64>,
    ended: Option<u64>,
}

fn wall_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub struct InputQueue {
    pub queue: Queue,
    clock: Instant,
    /// When each request was asked, allowed, started and ended, read from
    /// the wall clock as it happened, in milliseconds since 1970. `clock`
    /// stops while the Mac sleeps, so it gives only the durations.
    walls: BTreeMap<RequestId, Walls>,
    /// Requests the user already allowed whose grant ended with nothing
    /// executed. Presentation only: they stay queued for the agent, and
    /// only a new request or a revocation asks again.
    pub(crate) approved: BTreeSet<RequestId>,
    /// What each runnable request does. A request without one, made with
    /// input_queue's request action, only asks for approval.
    plans: BTreeMap<RequestId, Plan>,
    /// Why the executor could not carry a loan out.
    failures: BTreeMap<RequestId, String>,
    /// Per session's display, the last click refused because what was
    /// under its point changed, and how many times in a row.
    changed: BTreeMap<Owner, (String, u32)>,
    /// Whether the active loan may send its next event. The executor's
    /// thread reads it before every event.
    pub loan_ok: Arc<AtomicBool>,
    /// The loans the current grant started, in order, with their display
    /// and what each did, so the card can number the steps and draw the
    /// finished ones after their requests are gone.
    ran: Vec<(RequestId, DisplayId, Plan)>,
    /// The grant `ran` belongs to.
    ran_grant: Option<u64>,
}

/// What one loan does.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Drag(Drag),
    MenuItem(MenuItem),
    Click(Click),
}

/// One complete drag, in global points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// The window under `from` when the agent asked, and its frame.
    pub target: WindowTarget,
    pub frame: Rect,
    /// The window under `to` then, and its frame: `target` and `frame` for
    /// a drag within one window (ADR 0013, "Drags between windows").
    pub drop: WindowTarget,
    pub drop_frame: Rect,
    pub area: Rect,
    pub from: (f64, f64),
    pub to: (f64, f64),
    /// CGEventFlags bits.
    pub flags: u64,
}

/// An item of an Electron app's menu bar, chosen with a real click on the
/// Agent Display's menu bar (ADR 0013, "Menu items of Electron apps").
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// The agent's window, which comes forward so its app's menus show.
    pub target: WindowTarget,
    pub area: Rect,
    /// The app's name, for the card.
    pub app: String,
    /// Index among its siblings and title of each element from the menu
    /// bar title down to the item.
    pub path: Vec<(usize, String)>,
}

/// One complete click with the user's pointer, in global points (ADR 0013,
/// "Clicks").
#[derive(Clone, Debug, PartialEq)]
pub struct Click {
    pub target: WindowTarget,
    pub frame: Rect,
    /// When clicking an open pop-up menu (level 101) owned by an app on the
    /// display, the menu window and its frame (#205).
    pub menu: Option<(WindowTarget, Rect)>,
    /// Process Accessibility hit-tests at `at` (differs from `target.pid` for
    /// hosted `.xpc`/`.appex` views and menus).
    pub ax_pid: i32,
    pub area: Rect,
    pub at: (f64, f64),
    pub click: agent_screen_model::real_click::Click,
    /// CGEventFlags bits.
    pub flags: u64,
    /// The app's name and the window's title, for the card.
    pub app: String,
    pub title: Option<String>,
    /// The display-local region `real_click::look_region` chose, its
    /// samples when the agent asked, and what accessibility named at the
    /// point. The loan clicks only if both still look the same.
    pub region: Rect,
    pub look: Vec<u8>,
    pub element: Option<Box<agent_screen_model::real_click::Identity>>,
    /// When the agent first asked, by the queue's clock.
    pub asked_ms: u64,
}

impl Plan {
    /// Whether the loan acts on window `window`: a drag's start or drop,
    /// a menu item's window, a click's window.
    pub fn touches(&self, window: u32) -> bool {
        match self {
            Plan::Drag(d) => d.target.window_id == window || d.drop.window_id == window,
            Plan::MenuItem(m) => m.target.window_id == window,
            Plan::Click(c) => {
                c.target.window_id == window || c.menu.is_some_and(|(m, _)| m.window_id == window)
            }
        }
    }
}

impl MenuItem {
    pub fn titles(&self) -> Vec<String> {
        self.path.iter().map(|(_, title)| title.clone()).collect()
    }
}

impl Default for InputQueue {
    fn default() -> Self {
        Self {
            queue: Queue::default(),
            clock: Instant::now(),
            walls: BTreeMap::new(),
            approved: BTreeSet::new(),
            plans: BTreeMap::new(),
            failures: BTreeMap::new(),
            changed: BTreeMap::new(),
            loan_ok: Arc::default(),
            ran: Vec::new(),
            ran_grant: None,
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

    /// A real click `operation` of `owner` was refused because what was
    /// under its point changed: how many times in a row that click has
    /// been. Another click starts over; `None`, for a click that ended any
    /// other way, forgets it.
    pub fn content_changed(&mut self, owner: &Owner, operation: Option<&str>) -> u32 {
        let Some(operation) = operation else {
            self.changed.remove(owner);
            return 0;
        };
        let entry = self
            .changed
            .entry(owner.clone())
            .or_insert_with(|| (operation.to_string(), 0));
        if entry.0 != operation {
            *entry = (operation.to_string(), 0);
        }
        entry.1 += 1;
        entry.1
    }

    /// What a request does, if it can run.
    pub fn plan(&self, id: RequestId) -> Option<&Plan> {
        self.plans.get(&id)
    }

    /// The loan `id` of `owner` ran, and its call read how it ended. The
    /// result goes, so the same press made again is a new request, not
    /// this one's answer. The session declares itself done unless it has
    /// more waiting, so its grant ends and the pointer and front app go
    /// back without the 30 seconds' wait.
    pub fn loan_done(&mut self, owner: &Owner, id: RequestId) {
        if self.queue.request(id).is_some_and(|r| {
            &r.owner == owner && matches!(r.status, Status::Completed | Status::Stopped(_))
        }) {
            self.queue.forget(id);
        }
        // Done once nothing of the session's waits or runs, on any of its
        // displays.
        if !self.queue.waiting().any(|r| r.owner.same_session(owner))
            && !self
                .queue
                .active()
                .is_some_and(|r| r.owner.same_session(owner))
        {
            let now = self.now();
            self.queue.done(owner, now);
        }
        self.tick();
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
        let plan = self.plans.get(&id).cloned();
        self.follow_grant();
        if let (Some(plan), Some(request)) = (&plan, self.queue.request(id)) {
            self.ran.push((id, request.owner.display, plan.clone()));
        }
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
        self.queue.decline(approval, self.now());
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

    /// The session is done with real input, on every display it has.
    pub fn done(&mut self, owner: &Owner) {
        let pending: Vec<_> = self
            .queue
            .waiting()
            .filter(|r| r.owner.same_session(owner))
            .map(|r| (r.id, r.owner.clone()))
            .collect();
        for (id, request_owner) in pending {
            self.queue.withdraw(id, &request_owner, self.now());
        }
        self.queue.done(owner, self.now());
        self.forget_terminal(|request_owner| request_owner.same_session(owner));
        self.tick();
    }

    /// The user gave `window` from `display` to another Agent Session
    /// (ADR 0017): that display's requests aimed at it stop, a queued one
    /// at once and a running one once its loan lets go. Returns how many.
    pub fn window_given(&mut self, display: DisplayId, window: u32) -> usize {
        let aimed: Vec<(RequestId, Owner)> = self
            .queue
            .waiting()
            .chain(self.queue.active())
            .filter(|r| r.owner.display == display)
            .filter(|r| self.plans.get(&r.id).is_some_and(|p| p.touches(window)))
            .map(|r| (r.id, r.owner.clone()))
            .collect();
        let now = self.now();
        let mut stopped = 0;
        for (id, owner) in aimed {
            if self.queue.withdraw(id, &owner, now) {
                self.failures.insert(
                    id,
                    "the user gave this window to another agent session".into(),
                );
                stopped += 1;
            }
        }
        self.tick();
        stopped
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
        self.changed
            .retain(|owner, _| requests.owner(owner.display) == Some(owner));
        self.walls.retain(|id, _| requests.request(*id).is_some());
        // Every change to the queue ends here, so what the queue noted
        // just now gets the wall clock's time now.
        let now = wall_now();
        for r in requests.requests() {
            let walls = self.walls.entry(r.id).or_default();
            let stamp = |wall: &mut Option<u64>, noted: Option<u64>| {
                if noted.is_some() && wall.is_none() {
                    *wall = Some(now);
                }
            };
            stamp(&mut walls.asked, Some(r.times.asked));
            stamp(&mut walls.allowed, r.times.allowed);
            stamp(&mut walls.started, r.times.started);
            stamp(&mut walls.ended, r.times.ended);
        }
        let ok = self
            .queue
            .active()
            .is_some_and(|r| self.queue.may_send(r.id, &r.owner));
        self.loan_ok.store(ok, Ordering::Relaxed);
        self.follow_grant();
        was.is_some() || self.queue.grant().is_some()
    }

    /// A new grant, or none, starts with no loans run.
    fn follow_grant(&mut self) {
        let grant = self.queue.grant().map(|grant| grant.id);
        if grant != self.ran_grant {
            self.ran.clear();
            self.ran_grant = grant;
        }
    }

    /// The loans this grant started, oldest first: the last one may still
    /// be running.
    pub fn ran(&self) -> &[(RequestId, DisplayId, Plan)] {
        &self.ran
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
    /// request still to ask about, or during a grant the running loan's
    /// display, the first waiting request's, the last loan's while it is
    /// live, or any live display.
    pub fn card_display<H>(&self, displays: &Displays<H>) -> Option<DisplayId> {
        let id = if self.queue.grant().is_some() {
            self.queue
                .active()
                .or_else(|| self.queue.waiting().next())
                .map(|r| r.owner.display)
                .or_else(|| {
                    self.ran
                        .last()
                        .map(|(_, display, _)| *display)
                        .filter(|id| displays.get(*id).is_some_and(|d| d.is_live()))
                })
                .or_else(|| displays.list.iter().find(|d| d.is_live()).map(|d| d.id))
        } else {
            self.queue
                .waiting()
                .find(|r| self.asks(r.id))
                .map(|r| r.owner.display)
        };
        id.filter(|id| displays.get(*id).is_some())
    }

    /// When a request was asked, allowed, started and ended, in
    /// milliseconds since 1970, and the waits between, in milliseconds.
    fn timing(&self, id: RequestId, times: &agent_screen_model::input_queue::Times) -> Value {
        let walls = self.walls.get(&id).copied().unwrap_or_default();
        json!({
            "askedAt": walls.asked,
            "allowedAt": walls.allowed,
            "startedAt": walls.started,
            "endedAt": walls.ended,
            "approvalMs": times.approval_ms(),
            "turnMs": times.turn_ms(),
            "runMs": times.run_ms(),
        })
    }

    pub fn elapsed_seconds(&self) -> Option<u64> {
        self.queue
            .grant()
            .map(|grant| self.now().saturating_sub(grant.since_ms) / 1000)
    }

    /// How long the grant has had nothing to run, which ends it at
    /// IDLE_MS. None while a request runs or waits to run.
    pub fn idle_seconds(&self) -> Option<u64> {
        self.queue
            .grant()
            .and_then(|grant| grant.idle_since_ms)
            .map(|idle| self.now().saturating_sub(idle) / 1000)
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
                    "elapsedSeconds": seconds(request.times.elapsed_ms(self.now())),
                    "timing": self.timing(request.id, &request.times),
                })
            })
            .collect();
        json!({
            "executorAvailable": EXECUTOR_AVAILABLE,
            "grantActive": self.queue.grant().is_some(),
            "elapsedSeconds": self.elapsed_seconds(),
            "waiting": self.queue.waiting().count(),
            "requests": requests,
            "note": "Only completed means the action ran, with the user's pointer. approved means allowed and waiting its turn; a request made with input_queue's request action is then answered: retry the drag or click with real_input while the approval lasts. elapsedSeconds at the top is how long the current approval has lasted, null with none; a request's elapsedSeconds runs from your ask to its end, or to now. timing gives when it was asked, allowed, started and ended, in milliseconds since 1970: approvalMs is the user's time, turnMs the wait for its turn, runMs the run with the user's pointer.",
        })
    }
}

/// Milliseconds as seconds with one decimal.
fn seconds(ms: u64) -> f64 {
    (ms / 100) as f64 / 10.0
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

    fn drag(x: f64) -> Plan {
        let target = WindowTarget {
            pid: 1,
            window_id: 1,
        };
        let area = Rect::new(0.0, 0.0, 1280.0, 800.0);
        Plan::Drag(Drag {
            target,
            frame: area,
            drop: target,
            drop_frame: area,
            area,
            from: (x, 100.0),
            to: (x + 50.0, 200.0),
            flags: 0,
        })
    }

    #[test]
    fn giving_a_window_away_stops_the_requests_aimed_at_it() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        let owner = input.owner(1, 1).unwrap();
        let on_it = input
            .submit(owner, "a".into(), "Drag".into(), Some(drag(10.0)))
            .unwrap();
        // Another window, or the same window id on another display, is
        // left alone.
        assert_eq!(input.window_given(1, 2), 0);
        assert_eq!(input.window_given(2, 1), 0);
        assert_eq!(
            input.queue.request(on_it).map(|r| r.status),
            Some(Status::Queued)
        );
        assert_eq!(input.window_given(1, 1), 1);
        assert_eq!(
            input.queue.request(on_it).map(|r| r.status),
            Some(Status::Stopped(
                agent_screen_model::input_queue::Stop::Withdrawn
            ))
        );
    }

    #[test]
    fn a_grant_keeps_the_loans_it_ran_in_order_until_it_ends() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        input.register(2, &display_owner(2));
        let first = input.owner(1, 1).unwrap();
        let second = input.owner(2, 2).unwrap();
        let a = input
            .submit(first.clone(), "a".into(), "Drag".into(), Some(drag(10.0)))
            .unwrap();
        let b = input
            .submit(second, "b".into(), "Drag".into(), Some(drag(20.0)))
            .unwrap();
        assert!(input.ran().is_empty());
        assert!(input.approve(input.queue.approval()));
        assert_eq!(input.start_loan().map(|(id, _)| id), Some(a));
        input.finish_loan(a, None);
        // The agent read its result: the request goes, its step stays.
        input.loan_done(&first, a);
        assert!(input.queue.request(a).is_none());
        assert_eq!(input.start_loan().map(|(id, _)| id), Some(b));
        let ran: Vec<_> = input
            .ran()
            .iter()
            .map(|(id, d, p)| (*id, *d, p.clone()))
            .collect();
        assert_eq!(ran, vec![(a, 1, drag(10.0)), (b, 2, drag(20.0))]);
        input.finish_loan(b, None);
        input.revoke();
        assert!(input.queue.grant().is_none());
        assert!(input.ran().is_empty());
    }

    #[test]
    fn a_finished_request_reports_when_it_was_asked_allowed_started_and_ended() {
        let mut input = InputQueue::default();
        input.register(1, &display_owner(1));
        let owner = input.owner(1, 1).unwrap();
        let id = input
            .submit(owner.clone(), "a".into(), "Drag".into(), Some(drag(10.0)))
            .unwrap();
        let asked = &input.status(&owner, Some(id), false)["requests"][0];
        let before = wall_now();
        assert!(asked["timing"]["askedAt"].as_u64().unwrap() <= before);
        assert!(asked["timing"]["askedAt"].as_u64().unwrap() + 60_000 > before);
        assert!(asked["timing"]["allowedAt"].is_null());
        assert!(asked["elapsedSeconds"].as_f64().is_some());
        assert!(input.approve(input.queue.approval()));
        assert_eq!(input.start_loan().map(|(id, _)| id), Some(id));
        input.finish_loan(id, None);
        let state = input.status(&owner, Some(id), false);
        let r = &state["requests"][0];
        assert_eq!(r["state"], "completed");
        let timing = &r["timing"];
        for key in ["askedAt", "allowedAt", "startedAt", "endedAt"] {
            assert!(timing[key].as_u64().is_some(), "{key}: {timing}");
        }
        for key in ["approvalMs", "turnMs", "runMs"] {
            assert!(timing[key].as_u64().is_some(), "{key}: {timing}");
        }
        assert!(
            timing["endedAt"].as_u64() >= timing["startedAt"].as_u64()
                && timing["startedAt"].as_u64() >= timing["allowedAt"].as_u64()
                && timing["allowedAt"].as_u64() >= timing["askedAt"].as_u64()
        );
        assert_eq!(seconds(1_250), 1.2);
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
