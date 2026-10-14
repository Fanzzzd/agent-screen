//! Agent Displays and what Sidescreen knows about them, independent of the
//! platform. The vocabulary follows CONTEXT.md.
//!
//! `H` is the platform's handle that keeps a display alive. Tests use `()`.

use futures::channel::oneshot;
use std::collections::{BTreeMap, HashSet};

pub type DisplayId = u32;
pub type WindowId = u32;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }
    pub fn mid(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.max_x() && y >= self.y && y < self.max_y()
    }
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.max_x()
            && other.x < self.max_x()
            && self.y < other.max_y()
            && other.y < self.max_y()
    }
    pub fn offset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            self.width - 2.0 * dx,
            self.height - 2.0 * dy,
        )
    }
    /// The nearest frame inside `area`: shifted back over the edges it
    /// crosses, and shrunk only where it is larger than `area`.
    pub fn kept_inside(&self, area: &Rect) -> Rect {
        let width = self.width.min(area.width);
        let height = self.height.min(area.height);
        Rect::new(
            self.x.min(area.max_x() - width).max(area.x),
            self.y.min(area.max_y() - height).max(area.y),
            width,
            height,
        )
    }
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.max_x().max(other.max_x()) - x,
            self.max_y().max(other.max_y()) - y,
        )
    }
}

/// One app window, by owner process and window id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WindowTarget {
    pub pid: i32,
    pub window_id: WindowId,
}

/// The Agent Session an Agent Display belongs to.
#[derive(Clone, PartialEq, Debug)]
pub struct Owner {
    /// Stable per client session: Claude Code's session ID, Codex's
    /// `session_id`, or the MCP process as a fallback.
    pub session_key: String,
    /// "Claude Code", "Codex", or whatever the client reports.
    pub client: String,
    /// Task title the agent gave, or the project folder name.
    pub title: String,
    /// The control connection that created the display. When it closes, the
    /// display stops taking input (ADR 0004).
    pub connection: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EndReason {
    /// The agent called end_display.
    Finished,
    /// Its control connection closed.
    Disconnected,
    /// The user closed it while the agent was using it.
    ClosedByUser,
}

/// How a window came onto an Agent Display. Decides what dismissal does with
/// it when the agent left no Disposition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// Opened by the Agent Session: through launch_app, or by an action in
    /// one of its apps, such as New Document.
    Launched,
    /// An existing window the Agent Session placed.
    Placed,
    /// A Handover from the user.
    HandedOver,
    /// Already on the display when the agent gave it a Disposition.
    Found,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Launched => "launched",
            Origin::Placed => "placed",
            Origin::HandedOver => "handedOver",
            Origin::Found => "found",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disposition {
    Keep,
    Close,
}

impl Disposition {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "keep" => Some(Disposition::Keep),
            "close" => Some(Disposition::Close),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Disposition::Keep => "keep",
            Disposition::Close => "close",
        }
    }
}

/// Where a handed-over window lived before, so it can go back there.
#[derive(Clone, PartialEq, Debug)]
pub struct Home {
    /// Global frame, top-left origin.
    pub frame: Rect,
    /// The AeroSpace workspace, when AeroSpace manages the window.
    pub workspace: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Record {
    pub target: WindowTarget,
    pub origin: Origin,
    pub disposition: Option<Disposition>,
    pub home: Option<Home>,
}

/// Where the Agent Session last pointed or clicked, in display points.
/// `seq` grows with every action, so a view can tell a new click from one it
/// already animated.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AgentPointer {
    pub x: f64,
    pub y: f64,
    pub click: bool,
    pub seq: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlWait {
    Returned,
    TimedOut,
    Ended,
}

impl ControlWait {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlWait::Returned => "returned",
            ControlWait::TimedOut => "timedOut",
            ControlWait::Ended => "ended",
        }
    }
}

/// What `wait_for_control` gets right away, or a receiver to await.
pub enum Wait {
    Now(ControlWait),
    Later(oneshot::Receiver<ControlWait>),
}

pub struct Display<H> {
    pub id: DisplayId,
    pub name: String,
    /// Logical size in points.
    pub width: f64,
    pub height: f64,
    /// None for displays the user created from Peek.
    pub owner: Option<Owner>,
    /// Set once the owning Agent Session stopped using the display.
    pub ended: Option<EndReason>,
    /// Windows Sidescreen put on this display, by id.
    pub records: BTreeMap<WindowId, Record>,
    /// Processes launch_app started for this display. Their windows are
    /// the Agent Session's wherever they open; other apps' may be the user's.
    pub started: std::collections::BTreeSet<i32>,
    /// Windows the user took back. They are the user's from then on, even
    /// in an app the agent started.
    pub returned: std::collections::BTreeSet<WindowId>,
    /// Windows an app the agent started made with no agent action just
    /// before: the user's doing.
    pub user_made: std::collections::BTreeSet<WindowId>,
    /// Apps the agent started that the user took back a window of.
    pub released: std::collections::BTreeSet<i32>,
    /// The user is operating the display in the Viewer; agent input is refused.
    pub takeover: bool,
    /// The reason of an open Help Request.
    pub help_request: Option<String>,
    pub pointer: Option<AgentPointer>,
    waiters: Vec<oneshot::Sender<ControlWait>>,
    /// The platform display. Only held: dropping it removes the display.
    #[allow(dead_code)]
    pub handle: H,
}

impl<H> Display<H> {
    pub fn is_ended(&self) -> bool {
        self.ended.is_some()
    }

    /// Display Label: "Claude Code · Book flights".
    pub fn label(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{} · {}", owner.client, owner.title),
            None => self.name.clone(),
        }
    }

    /// The client's name for copy: "Codex", "Claude Code".
    pub fn agent_name(&self) -> &str {
        self.owner
            .as_ref()
            .map(|o| o.client.as_str())
            .unwrap_or("agent")
    }

    /// The agent has to wait for the user before it carries on.
    pub fn awaits_user(&self) -> bool {
        self.takeover || self.help_request.is_some()
    }

    /// A live Agent Display: an Agent Session still uses it.
    pub fn is_live(&self) -> bool {
        self.owner.is_some() && !self.is_ended()
    }

    /// Record a window Sidescreen put here. A Handover stays a Handover even
    /// if the agent places the same window again.
    pub fn claim(&mut self, target: WindowTarget, origin: Origin, home: Option<Home>) {
        if self
            .records
            .get(&target.window_id)
            .is_some_and(|r| r.origin == Origin::HandedOver)
        {
            return;
        }
        // Handed over again after a take-back: the agent's once more.
        self.returned.remove(&target.window_id);
        self.records.insert(
            target.window_id,
            Record {
                target,
                origin,
                disposition: None,
                home,
            },
        );
    }

    pub fn forget(&mut self, window: WindowId) {
        self.records.remove(&window);
    }

    /// The user takes a window back: it leaves the records for good.
    /// Taking one window back makes its app the user's: none of its
    /// windows is the agent's to place any more.
    pub fn give_back(&mut self, target: WindowTarget) {
        self.records.remove(&target.window_id);
        self.returned.insert(target.window_id);
        if self.started.remove(&target.pid) {
            self.released.insert(target.pid);
        }
    }

    /// Whether a window off every display is the agent's to place: one of
    /// an app it started, that no display records, that the user didn't
    /// make while using the app, and never took back.
    pub fn is_stray(&self, target: WindowTarget, recorded_anywhere: bool) -> bool {
        self.started.contains(&target.pid)
            && !recorded_anywhere
            && !self.returned.contains(&target.window_id)
            && !self.user_made.contains(&target.window_id)
    }

    /// The Agent Session's verdict on a window. It can change its mind any time.
    pub fn set_disposition(&mut self, target: WindowTarget, disposition: Disposition) {
        self.records
            .entry(target.window_id)
            .or_insert(Record {
                target,
                origin: Origin::Found,
                disposition: None,
                home: None,
            })
            .disposition = Some(disposition);
    }

    fn resume_waiters(&mut self, outcome: ControlWait) {
        for waiter in self.waiters.drain(..) {
            let _ = waiter.send(outcome);
        }
    }
}

pub struct Displays<H> {
    pub list: Vec<Display<H>>,
    /// Ended displays that are gone. The agent still gets "ended", not
    /// "does not exist", for them.
    pub retired: HashSet<DisplayId>,
    /// Of those, the ones the user closed: the agent should stop, not make
    /// another display.
    pub closed_by_user: HashSet<DisplayId>,
    next_serial: u32,
}

impl<H> Default for Displays<H> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            retired: HashSet::new(),
            closed_by_user: HashSet::new(),
            next_serial: 1,
        }
    }
}

/// Claude scales down any image over 1568 px on its long edge or about
/// 1.15 megapixels, and then reads coordinates in the smaller image. The
/// default display fits that budget, so screenshot pixels are points.
pub const DEFAULT_SIZE: (f64, f64) = (1280.0, 800.0);

/// Screenshot pixels per display point: 1 within the image budget, less for
/// a larger display, so the agent's coordinates and its image agree.
pub fn shot_scale((width, height): (f64, f64)) -> f64 {
    const LONG_EDGE: f64 = 1568.0;
    const PIXELS: f64 = 1_150_000.0;
    (LONG_EDGE / width.max(height))
        .min((PIXELS / (width * height)).sqrt())
        .min(1.0)
}

impl<H> Displays<H> {
    /// The serial for the next display, and its name.
    pub fn next_serial(&mut self) -> (u32, String) {
        let serial = self.next_serial;
        self.next_serial += 1;
        (serial, format!("Sidescreen {serial}"))
    }

    pub fn add(
        &mut self,
        id: DisplayId,
        name: String,
        size: (f64, f64),
        owner: Option<Owner>,
        handle: H,
    ) {
        // A spare display comes back under its old id.
        self.retired.remove(&id);
        self.closed_by_user.remove(&id);
        self.list.push(Display {
            id,
            name,
            width: size.0,
            height: size.1,
            owner,
            ended: None,
            records: BTreeMap::new(),
            started: Default::default(),
            returned: Default::default(),
            user_made: Default::default(),
            released: Default::default(),
            takeover: false,
            help_request: None,
            pointer: None,
            waiters: Vec::new(),
            handle,
        });
    }

    /// Remove a display; returns it so the caller can drop its handle.
    pub fn remove(&mut self, id: DisplayId) -> Option<Display<H>> {
        let index = self.list.iter().position(|d| d.id == id)?;
        let mut display = self.list.remove(index);
        if display.is_ended() {
            self.retired.insert(id);
        }
        if display.ended == Some(EndReason::ClosedByUser) {
            self.closed_by_user.insert(id);
        }
        display.resume_waiters(ControlWait::Ended);
        Some(display)
    }

    pub fn get(&self, id: DisplayId) -> Option<&Display<H>> {
        self.list.iter().find(|d| d.id == id)
    }

    pub fn get_mut(&mut self, id: DisplayId) -> Option<&mut Display<H>> {
        self.list.iter_mut().find(|d| d.id == id)
    }

    /// The live display of an Agent Session, if it has one.
    pub fn for_session(&self, session_key: &str) -> Option<&Display<H>> {
        self.list.iter().find(|d| {
            !d.is_ended()
                && d.owner
                    .as_ref()
                    .is_some_and(|o| o.session_key == session_key)
        })
    }

    /// The Agent Session stopped using the display. Returns false if it had
    /// already ended.
    pub fn end(&mut self, id: DisplayId, reason: EndReason) -> bool {
        let Some(display) = self.get_mut(id) else {
            return false;
        };
        if display.is_ended() {
            return false;
        }
        display.ended = Some(reason);
        display.takeover = false;
        display.help_request = None;
        display.pointer = None;
        display.resume_waiters(ControlWait::Ended);
        true
    }

    /// End every display a closed connection owned.
    pub fn connection_closed(&mut self, connection: u64) -> Vec<DisplayId> {
        let ids: Vec<DisplayId> = self
            .list
            .iter()
            .filter(|d| d.owner.as_ref().is_some_and(|o| o.connection == connection))
            .map(|d| d.id)
            .collect();
        ids.into_iter()
            .filter(|&id| self.end(id, EndReason::Disconnected))
            .collect()
    }

    /// Only a live Agent Display has an Agent Session to refuse.
    pub fn begin_takeover(&mut self, id: DisplayId) -> bool {
        match self.get_mut(id) {
            Some(d) if d.is_live() && !d.takeover => {
                d.takeover = true;
                true
            }
            _ => false,
        }
    }

    /// Hand the display back. This also ends a Help Request.
    pub fn end_takeover(&mut self, id: DisplayId) -> bool {
        match self.get_mut(id) {
            Some(d) if d.takeover => {
                d.takeover = false;
                d.help_request = None;
                d.resume_waiters(ControlWait::Returned);
                true
            }
            _ => false,
        }
    }

    pub fn request_help(&mut self, id: DisplayId, reason: String) -> bool {
        match self.get_mut(id) {
            Some(d) if d.is_live() => {
                d.help_request = Some(reason);
                true
            }
            _ => false,
        }
    }

    /// The agent no longer needs the user: clear its Help Request.
    /// Waiters for control go on unless the user is in control.
    pub fn withdraw_help(&mut self, id: DisplayId) -> bool {
        let Some(d) = self.get_mut(id) else {
            return false;
        };
        let withdrawn = d.help_request.take().is_some();
        if withdrawn && !d.takeover {
            d.resume_waiters(ControlWait::Returned);
        }
        withdrawn
    }

    pub fn point(&mut self, id: DisplayId, x: f64, y: f64, click: bool) {
        if let Some(d) = self.get_mut(id) {
            let seq = d.pointer.map_or(0, |p| p.seq) + 1;
            d.pointer = Some(AgentPointer { x, y, click, seq });
        }
    }

    /// Wait until the user hands the display back or the display ends. The
    /// caller adds the timeout.
    pub fn wait_for_control(&mut self, id: DisplayId) -> Wait {
        let Some(d) = self.get_mut(id) else {
            return Wait::Now(ControlWait::Ended);
        };
        if d.is_ended() {
            return Wait::Now(ControlWait::Ended);
        }
        if !d.awaits_user() {
            return Wait::Now(ControlWait::Returned);
        }
        d.waiters.retain(|w| !w.is_canceled());
        let (tx, rx) = oneshot::channel();
        d.waiters.push(tx);
        Wait::Later(rx)
    }
}

// MARK: Dismissal defaults

/// What happens to one window when the user dismisses its display.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fate {
    GiveBack,
    Close,
}

/// The default for one window. `origin` is None for a window Sidescreen
/// never tracked.
pub fn default_fate(
    origin: Option<Origin>,
    disposition: Option<Disposition>,
    of_launched_app: bool,
) -> Fate {
    match (origin, disposition) {
        (Some(Origin::HandedOver), _) => Fate::GiveBack,
        (_, Some(Disposition::Keep)) => Fate::GiveBack,
        (_, Some(Disposition::Close)) => Fate::Close,
        (Some(Origin::Launched), None) => Fate::Close,
        (Some(Origin::Placed | Origin::Found), None) => Fate::GiveBack,
        // A further window of an app the agent launched is the agent's;
        // anything else may be the user's.
        (None, None) => {
            if of_launched_app {
                Fate::Close
            } else {
                Fate::GiveBack
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rect;

    #[test]
    fn kept_inside_moves_back_and_shrinks_only_when_needed() {
        let area = Rect::new(0.0, 0.0, 1280.0, 800.0);
        let inside = Rect::new(100.0, 100.0, 300.0, 200.0);
        assert_eq!(inside.kept_inside(&area), inside);
        // Calculator widening to Scientific past the right edge.
        let wide = Rect::new(797.0, 392.0, 674.0, 408.0);
        assert_eq!(
            wide.kept_inside(&area),
            Rect::new(606.0, 392.0, 674.0, 408.0)
        );
        let huge = Rect::new(-50.0, 10.0, 2000.0, 900.0);
        assert_eq!(huge.kept_inside(&area), area);
    }

    use super::*;

    #[test]
    fn screenshots_fit_the_image_budget() {
        assert_eq!(shot_scale(DEFAULT_SIZE), 1.0);
        let s = shot_scale((2560.0, 1600.0));
        assert!(s < 1.0 && 2560.0 * s <= 1568.0 && 2560.0 * 1600.0 * s * s <= 1_150_000.0);
    }

    fn owner(connection: u64) -> Option<Owner> {
        Some(Owner {
            session_key: "s".into(),
            client: "Codex".into(),
            title: "订机票".into(),
            connection,
        })
    }

    fn displays() -> Displays<()> {
        let mut d = Displays::default();
        d.add(7, "Sidescreen 1".into(), DEFAULT_SIZE, owner(1), ());
        d
    }

    #[test]
    fn label_and_session_lookup() {
        let d = displays();
        assert_eq!(d.get(7).unwrap().label(), "Codex · 订机票");
        assert_eq!(d.for_session("s").map(|d| d.id), Some(7));
    }

    #[test]
    fn takeover_resumes_waiters_and_clears_help() {
        let mut d = displays();
        assert!(d.request_help(7, "登录".into()));
        let Wait::Later(mut rx) = d.wait_for_control(7) else {
            panic!("should wait")
        };
        assert!(d.begin_takeover(7));
        assert!(d.end_takeover(7));
        assert_eq!(rx.try_recv().unwrap(), Some(ControlWait::Returned));
        assert!(d.get(7).unwrap().help_request.is_none());
        assert!(matches!(
            d.wait_for_control(7),
            Wait::Now(ControlWait::Returned)
        ));
    }

    #[test]
    fn withdrawing_help_frees_waiters() {
        let mut d = displays();
        assert!(!d.withdraw_help(7), "nothing to withdraw");
        assert!(d.request_help(7, "登录".into()));
        let Wait::Later(mut rx) = d.wait_for_control(7) else {
            panic!("should wait")
        };
        assert!(d.withdraw_help(7));
        assert_eq!(rx.try_recv().unwrap(), Some(ControlWait::Returned));
    }

    #[test]
    fn closed_connection_ends_and_retires() {
        let mut d = displays();
        assert_eq!(d.connection_closed(1), vec![7]);
        assert!(d.for_session("s").is_none());
        assert!(
            !d.begin_takeover(7),
            "an ended display has no session to refuse"
        );
        assert!(matches!(
            d.wait_for_control(7),
            Wait::Now(ControlWait::Ended)
        ));
        d.remove(7);
        assert!(d.retired.contains(&7));
    }

    #[test]
    fn handover_survives_a_later_claim() {
        let mut d = displays();
        let target = WindowTarget {
            pid: 1,
            window_id: 9,
        };
        let display = d.get_mut(7).unwrap();
        display.claim(target, Origin::HandedOver, None);
        display.claim(target, Origin::Placed, None);
        assert_eq!(display.records[&9].origin, Origin::HandedOver);
        display.set_disposition(target, Disposition::Close);
        assert_eq!(
            default_fate(Some(Origin::HandedOver), Some(Disposition::Close), false),
            Fate::GiveBack
        );
    }

    #[test]
    fn a_window_taken_back_is_never_a_stray() {
        let mut d = displays();
        let display = d.get_mut(7).unwrap();
        let ours = WindowTarget {
            pid: 5,
            window_id: 1,
        };
        let users = WindowTarget {
            pid: 6,
            window_id: 2,
        };
        display.started.insert(5);
        assert!(display.is_stray(ours, false));
        assert!(!display.is_stray(ours, true), "on a display's records");
        assert!(
            !display.is_stray(users, false),
            "an app the agent didn't start"
        );
        display.claim(ours, Origin::Launched, None);
        display.give_back(ours);
        assert!(!display.is_stray(ours, false));
        assert!(display.released.contains(&5));
        // The app is the user's now: its next window isn't a stray either.
        let next = WindowTarget {
            pid: 5,
            window_id: 3,
        };
        assert!(!display.is_stray(next, false));
        // Handed over again, it's the agent's.
        display.claim(ours, Origin::HandedOver, None);
        assert!(!display.returned.contains(&1));
    }

    #[test]
    fn a_display_the_user_closed_stays_closed_by_them() {
        let mut d = displays();
        assert!(d.end(7, EndReason::ClosedByUser));
        assert!(
            !d.end(7, EndReason::Disconnected),
            "the first reason stands"
        );
        assert!(d.remove(7).is_some());
        assert!(d.retired.contains(&7));
        assert!(d.closed_by_user.contains(&7));
    }

    #[test]
    fn a_window_the_user_made_is_never_a_stray() {
        let mut d = displays();
        let display = d.get_mut(7).unwrap();
        display.started.insert(5);
        let made = WindowTarget {
            pid: 5,
            window_id: 4,
        };
        display.user_made.insert(4);
        assert!(!display.is_stray(made, false));
    }

    #[test]
    fn fates() {
        assert_eq!(
            default_fate(Some(Origin::Launched), None, false),
            Fate::Close
        );
        assert_eq!(
            default_fate(Some(Origin::Placed), None, false),
            Fate::GiveBack
        );
        assert_eq!(default_fate(None, None, true), Fate::Close);
        assert_eq!(default_fate(None, None, false), Fate::GiveBack);
        assert_eq!(
            default_fate(Some(Origin::Found), Some(Disposition::Keep), false),
            Fate::GiveBack
        );
    }
}
