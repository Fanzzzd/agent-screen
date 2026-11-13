//! Agent Displays and what Agent Screen knows about them, independent of the
//! platform. The vocabulary follows CONTEXT.md.
//!
//! `H` is the platform's handle that keeps a display alive. Tests use `()`.

use crate::access::{Access, Authority, Refusal};
pub use crate::request::{ControlAsk, Wait};
use crate::request::{NamedWindow, Requests};
use crate::session::Sessions;
use std::collections::{BTreeMap, HashSet};

pub type DisplayId = u32;
pub type WindowId = u32;

/// A display's role in window ownership. A test stage may supply the User
/// role without changing the hardware's identity or the placement rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayRole {
    User,
    Agent,
    Closet,
}

impl DisplayRole {
    pub fn from_hardware(vendor: u32, model: u32, simulated_user: bool) -> Self {
        match (vendor, model) {
            (0x4145, 1) => Self::Closet,
            (0x5353, _) if !simulated_user => Self::Agent,
            _ => Self::User,
        }
    }

    /// Only a user's window can be offered in a Window Request.
    pub fn accepts_window_request(self) -> bool {
        self == Self::User
    }
}

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

impl EndReason {
    /// Whether the display's Handovers go back to the user as it ends.
    /// When the Agent Session stops, by end_display or a closed
    /// connection, they go at once: the user's window must not wait on a
    /// Review Display they may never open, where the same session can't
    /// take it again either (#161). When the user closes the display, the
    /// dismissal plan returns them.
    pub fn returns_handovers(self) -> bool {
        self != Self::ClosedByUser
    }
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

/// What a take-back changed on a display, for [`Display::give_back_failed`]
/// when the window couldn't leave it.
#[derive(Clone, PartialEq, Debug)]
pub struct GivenBack {
    pub target: WindowTarget,
    /// The window's record before, with its home.
    pub record: Option<Record>,
    was_returned: bool,
    serial: u64,
}

impl GivenBack {
    pub fn home(&self) -> Option<Home> {
        self.record.as_ref().and_then(|r| r.home.clone())
    }
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

/// A hand-over claimed and not yet moved: what authorized it, and the
/// grant's revoke count then.
#[derive(Clone, PartialEq, Debug)]
pub struct Ticket {
    serial: u64,
    pub target: WindowTarget,
    pub authority: Authority,
    pub revokes: u64,
    /// The record the claim replaced.
    pub before: Option<Record>,
    /// The user had taken the window back before.
    pub was_returned: bool,
}

/// A window place_window moved from one of a session's displays to
/// another, with what `Displays::undo_move` puts back if it didn't arrive.
#[derive(Clone, PartialEq, Debug)]
pub struct Moved {
    pub from: DisplayId,
    pub to: DisplayId,
    pub target: WindowTarget,
    /// The source display's record of the window, if it had one.
    record: Option<Record>,
    /// The destination's record before, if it had one.
    before_to: Option<Record>,
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
    /// Windows Agent Screen put on this display, by id.
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
    /// The open Help, Control and Window Requests, and the agent's waits
    /// for the user.
    pub requests: Requests,
    /// The agent is typing through the user's input method, which takes
    /// the user's keyboard for that time (ADR 0008).
    pub keyboard_borrowed: bool,
    pub pointer: Option<AgentPointer>,
    handover_serial: u64,
    /// The latest give-back of each window, by serial: only that one's
    /// failure may put the window's record back.
    given_back: BTreeMap<WindowId, u64>,
    give_back_serial: u64,
    handing_over: BTreeMap<WindowId, u64>,
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
        self.takeover || self.requests.is_open(crate::request::Kind::Help)
    }

    /// A live Agent Display: an Agent Session still uses it.
    pub fn is_live(&self) -> bool {
        self.owner.is_some() && !self.is_ended()
    }

    /// The Agent Session using this display, while one does.
    pub fn session(&self) -> Option<&str> {
        self.owner
            .as_ref()
            .filter(|_| !self.is_ended())
            .map(|o| o.session_key.as_str())
    }

    /// Who has this display, as place_window names it to the session with
    /// key `asking`: the asking session's own live display is Mine, never
    /// another session's (ADR 0016).
    pub fn holder(&self, asking: Option<&str>) -> crate::placement::Holder {
        use crate::placement::Holder;
        if self.is_ended() {
            Holder::Review
        } else if self.owner.is_some() && self.session() == asking {
            Holder::Mine
        } else if self.owner.is_some() {
            Holder::OtherSession
        } else {
            Holder::Other
        }
    }

    /// Record a window Agent Screen put here. A Handover stays a Handover even
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
        self.handing_over.remove(&window);
    }

    /// The user takes a window back: it leaves the records for good.
    /// Taking one window back makes its app the user's: none of its
    /// windows is the agent's to place any more. `Displays::give_back`
    /// also keeps the session's Window Access off the window and releases
    /// the app on the session's other displays.
    fn give_back(&mut self, target: WindowTarget) -> GivenBack {
        let record = self.records.get(&target.window_id).cloned();
        self.forget(target.window_id);
        let was_returned = !self.returned.insert(target.window_id);
        if self.started.remove(&target.pid) {
            self.released.insert(target.pid);
        }
        self.give_back_serial += 1;
        self.given_back
            .insert(target.window_id, self.give_back_serial);
        GivenBack {
            target,
            record,
            was_returned,
            serial: self.give_back_serial,
        }
    }

    /// The window of a give-back is still on this display: its app kept it,
    /// or nothing could move it. It shows and works as before, with its
    /// record and home, so the user or the agent can try again. The user's
    /// word still stands for the rest: no Window Access covers the window,
    /// and its app stays theirs. Nothing changes when the window was placed
    /// here again or given back again meanwhile, or was the user's already
    /// before. True when the window is no longer marked returned.
    pub fn give_back_failed(&mut self, given: GivenBack) -> bool {
        let window = given.target.window_id;
        if given.was_returned
            || self.given_back.get(&window) != Some(&given.serial)
            || self.records.contains_key(&window)
            || !self.returned.remove(&window)
        {
            return false;
        }
        if let Some(record) = given.record {
            self.records.insert(window, record);
        }
        true
    }

    /// Whether a window off every display is the agent's to place: one of
    /// an app it started, or one this display recorded that has left it,
    /// as when its app put it back on the user's screen (#19, #47). Not
    /// one another display records, the user made while using the app, or
    /// took back.
    pub fn is_stray(&self, target: WindowTarget, recorded_elsewhere: bool) -> bool {
        (self.started.contains(&target.pid) || self.records.contains_key(&target.window_id))
            && !recorded_elsewhere
            && !self.returned.contains(&target.window_id)
            && !self.user_made.contains(&target.window_id)
    }

    /// A stray must also be outside every Agent Display. The platform
    /// supplies the source display's role; ownership still decides whose
    /// window it is and whether another display already records it.
    pub fn is_stray_from(
        &self,
        target: WindowTarget,
        recorded_elsewhere: bool,
        source: DisplayRole,
        held_on_source: bool,
    ) -> bool {
        source != DisplayRole::Closet
            && !(source == DisplayRole::Agent && held_on_source)
            && self.is_stray(target, recorded_elsewhere)
    }

    /// The user's windows on this display, which go back to them.
    pub fn handovers(&self) -> Vec<WindowTarget> {
        self.records
            .values()
            .filter(|r| r.origin == Origin::HandedOver)
            .map(|r| r.target)
            .collect()
    }

    /// Whether the user is in control, which pauses Window Access.
    pub fn access_paused(&self) -> bool {
        self.takeover
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
}

pub struct Displays<H> {
    pub list: Vec<Display<H>>,
    /// The Agent Sessions using these displays: each one's live displays
    /// and Window Access (ADR 0016).
    pub sessions: Sessions,
    /// Ended displays that are gone. The agent still gets "ended", not
    /// "does not exist", for them.
    pub retired: HashSet<DisplayId>,
    /// Of those, the ones the user closed: the agent should stop, not make
    /// another display.
    pub closed_by_user: HashSet<DisplayId>,
}

impl<H> Default for Displays<H> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            sessions: Sessions::default(),
            retired: HashSet::new(),
            closed_by_user: HashSet::new(),
        }
    }
}

/// Claude scales down any image over 1568 px on its long edge or about
/// 1.15 megapixels, and then reads coordinates in the smaller image. The
/// default display fits that budget, so screenshot pixels are points.
pub const DEFAULT_SIZE: (f64, f64) = (1280.0, 800.0);

/// A Help Request carries at most this many steps, each at most this long:
/// the user reads them beside the display while doing them.
pub const HELP_STEPS: usize = 5;
pub const HELP_STEP_CHARS: usize = 160;

/// A Help Request's steps, trimmed; why they can't be shown otherwise.
pub fn help_steps(steps: Vec<String>) -> Result<Vec<String>, String> {
    if steps.len() > HELP_STEPS {
        return Err(format!("give at most {HELP_STEPS} steps"));
    }
    steps
        .into_iter()
        .map(|step| {
            let step = step.trim();
            if step.is_empty() {
                Err("a step is empty".to_string())
            } else if step.chars().count() > HELP_STEP_CHARS {
                Err(format!("keep each step under {HELP_STEP_CHARS} characters"))
            } else {
                Ok(step.to_string())
            }
        })
        .collect()
}

/// Which apps a recording of a display hears: those whose every window is
/// on it. macOS separates audio by app, not by window, so an app with a
/// window anywhere else, such as the user's own browser, is left out
/// rather than let the user's sound in. A window is on the display when
/// it is on screen and its center is there; any other window, hidden and
/// minimized ones included, is elsewhere. `windows` is each window's app
/// (pid), global bounds and whether it is on screen. Returns the apps heard
/// and the apps with a window on the display that are left out.
pub fn heard_apps(
    windows: &[(i32, Rect, bool)],
    display: &Rect,
) -> (
    std::collections::BTreeSet<i32>,
    std::collections::BTreeSet<i32>,
) {
    use std::collections::BTreeSet;
    let here = |(_, b, shown): &&(i32, Rect, bool)| *shown && display.contains(b.mid());
    let on: BTreeSet<i32> = windows.iter().filter(here).map(|w| w.0).collect();
    let elsewhere: BTreeSet<i32> = windows.iter().filter(|w| !here(w)).map(|w| w.0).collect();
    on.iter().copied().partition(|pid| !elsewhere.contains(pid))
}

/// The serial for a new display: the lowest from `first` that no display
/// holds. macOS keeps a ColorSync record for every display serial it has
/// seen, and WindowServer reads all of them once per online display each
/// time displays change, giving up after 50 ms and asking again every 5 s.
/// Counting serials up grows that list without end; reusing them bounds it
/// to the most displays held at once.
pub fn lowest_free_serial(first: u32, taken: &std::collections::BTreeSet<u32>) -> u32 {
    (first..)
        .find(|serial| !taken.contains(serial))
        .expect("a free serial")
}

/// A window frame place_window asked for, in screenshot pixels from the
/// display's top-left corner. A part left out keeps the window's own.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct FrameRequest {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

impl FrameRequest {
    pub fn is_empty(&self) -> bool {
        self.x.is_none() && self.y.is_none() && self.width.is_none() && self.height.is_none()
    }

    /// The global frame in points for a window now at `current`, on a
    /// display at `area`, both global points; `scale` is screenshot pixels
    /// per point. The frame stays inside the display: past an edge, part of
    /// the window would show on the user's screen.
    pub fn resolve(&self, current: Rect, area: Rect, scale: f64) -> Rect {
        self.resolve_with_top_inset(current, area, scale, 0.0)
    }

    /// Resolve against the display's usable area while keeping screenshot
    /// coordinates relative to the full display, including its reserved top.
    pub fn resolve_with_top_inset(
        &self,
        current: Rect,
        area: Rect,
        scale: f64,
        reserved_top: f64,
    ) -> Rect {
        let points = |px: Option<f64>| px.map(|v| v / scale);
        let usable = usable_area(area, reserved_top);
        Rect::new(
            points(self.x).map_or(current.x, |x| area.x + x),
            points(self.y).map_or(current.y, |y| area.y + y),
            points(self.width).unwrap_or(current.width),
            points(self.height).unwrap_or(current.height),
        )
        .kept_inside(&usable)
    }
}

/// Where a display's windows may sit: all of it but the strip at the top
/// that holds the user's menu bar. A window's top edge under the strip
/// can't be clicked: a click there reaches the user's front app.
pub fn usable_area(area: Rect, reserved_top: f64) -> Rect {
    let top = reserved_top.clamp(0.0, area.height);
    Rect::new(area.x, area.y + top, area.width, area.height - top)
}

/// A global frame in points as screenshot pixels from the top-left corner
/// of the display at `area`: the space list_windows reports.
pub fn frame_in_shot(frame: Rect, area: Rect, scale: f64) -> Rect {
    let px = |v: f64| (v * scale).round();
    Rect::new(
        px(frame.x - area.x),
        px(frame.y - area.y),
        px(frame.width),
        px(frame.height),
    )
}

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
        if let Some(owner) = &owner {
            self.sessions.join(&owner.session_key, id);
        }
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
            requests: Requests::default(),
            keyboard_borrowed: false,
            pointer: None,
            handover_serial: 0,
            given_back: BTreeMap::new(),
            give_back_serial: 0,
            handing_over: BTreeMap::new(),
            handle,
        });
    }

    /// Remove a display; returns it so the caller can drop its handle.
    pub fn remove(&mut self, id: DisplayId) -> Option<Display<H>> {
        let index = self.list.iter().position(|d| d.id == id)?;
        let mut display = self.list.remove(index);
        if let Some(owner) = &display.owner {
            self.sessions.leave(&owner.session_key, id);
        }
        if display.is_ended() {
            self.retired.insert(id);
        }
        if display.ended == Some(EndReason::ClosedByUser) {
            self.closed_by_user.insert(id);
        }
        display.requests.end();
        Some(display)
    }

    pub fn get(&self, id: DisplayId) -> Option<&Display<H>> {
        self.list.iter().find(|d| d.id == id)
    }

    pub fn get_mut(&mut self, id: DisplayId) -> Option<&mut Display<H>> {
        self.list.iter_mut().find(|d| d.id == id)
    }

    /// An Agent Session's live displays, oldest first.
    pub fn of_session(&self, session_key: &str) -> Vec<DisplayId> {
        self.sessions.live(session_key).to_vec()
    }

    /// The display a session's `create_display` without `new` reuses: its
    /// latest live one.
    pub fn for_session(&self, session_key: &str) -> Option<&Display<H>> {
        self.sessions
            .get(session_key)
            .and_then(|s| s.current())
            .and_then(|id| self.get(id))
    }

    /// Whether displays `a` and `b` are live displays of one Agent Session.
    pub fn same_session(&self, a: DisplayId, b: DisplayId) -> bool {
        let key = |id| self.get(id).and_then(Display::session);
        key(a).is_some() && key(a) == key(b)
    }

    /// The Window Access of the session using display `id`, which covers
    /// all its displays. None for a Review Display or one the user made.
    pub fn access(&self, id: DisplayId) -> Option<&Access> {
        let key = self.get(id)?.session()?;
        self.sessions.get(key).map(|s| &s.access)
    }

    pub fn access_mut(&mut self, id: DisplayId) -> Option<&mut Access> {
        let key = self.get(id)?.session()?.to_string();
        self.sessions.get_mut(&key).map(|s| &mut s.access)
    }

    /// The Agent Session stopped using the display. Returns false if it had
    /// already ended. The session's Window Access ends with its last live
    /// display (ADR 0016).
    pub fn end(&mut self, id: DisplayId, reason: EndReason) -> bool {
        let Some(display) = self.get_mut(id) else {
            return false;
        };
        if display.is_ended() {
            return false;
        }
        let key = display.owner.as_ref().map(|o| o.session_key.clone());
        display.ended = Some(reason);
        display.takeover = false;
        display.pointer = None;
        if let Some(key) = key {
            self.sessions.leave(&key, id);
        }
        // Every request ends, and every wait hears it.
        self.end_requests(id);
        true
    }

    /// The Handovers of ended display `id` to send home now, when its end
    /// reason returns them. The caller gives each back, as the user's Take
    /// Back would, right before moving it, so the others stay recorded
    /// until their turn. The agent's own windows stay for review. A
    /// Handover that then can't leave goes back through
    /// `give_back_failed`, and the dismissal plan returns it later.
    pub fn handovers_at_end(&self, id: DisplayId) -> Vec<WindowTarget> {
        self.get(id)
            .filter(|d| d.ended.is_some_and(EndReason::returns_handovers))
            .map(|d| d.handovers())
            .unwrap_or_default()
    }

    /// End every display a closed connection owned. Each session's
    /// Window Access ends once, with its last display.
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

    /// Whether another session's display, or a display no session uses,
    /// holds this window: in its records, or as a window of an app its
    /// session started that the user didn't make or take back. Window
    /// Access never reaches such a window. The session's own other
    /// displays don't count: moving a window between them is a Placement
    /// (ADR 0016).
    pub fn owned_elsewhere(&self, id: DisplayId, target: WindowTarget) -> bool {
        self.list
            .iter()
            .filter(|d| d.id != id && !self.same_session(id, d.id))
            .any(|d| {
                d.records.contains_key(&target.window_id)
                    || (d.started.contains(&target.pid)
                        && !d.user_made.contains(&target.window_id)
                        && !d.returned.contains(&target.window_id))
            })
    }

    /// Whether `authority` lets one of the user's windows go onto display
    /// `id` now. Checked again when the window is claimed, so a revoke, a
    /// Takeover or the display ending in between wins.
    pub fn may_hand_over(
        &self,
        id: DisplayId,
        target: WindowTarget,
        authority: &Authority,
    ) -> Result<(), Refusal> {
        let d = self.get(id).ok_or(Refusal::Ended)?;
        if self
            .list
            .iter()
            .any(|d| d.handing_over.contains_key(&target.window_id))
        {
            return Err(Refusal::Busy);
        }
        if *authority == Authority::User {
            return Ok(());
        }
        if !d.is_live() {
            return Err(Refusal::Ended);
        }
        if d.takeover {
            return Err(Refusal::Takeover);
        }
        if self.owned_elsewhere(id, target) {
            return Err(Refusal::Elsewhere);
        }
        if let Authority::Grant { bundle_id } = authority {
            let access = self.access(id).ok_or(Refusal::Ended)?;
            if access.excluded.contains(&target.window_id) {
                return Err(Refusal::Excluded);
            }
            if !access.covers(bundle_id.as_deref()) {
                return Err(Refusal::NotGranted);
            }
        }
        Ok(())
    }

    /// Of the user's windows of an app, front to back with their bundle
    /// ids, the one the session's grant may take now.
    pub fn grantable(
        &self,
        id: DisplayId,
        candidates: &[(WindowTarget, Option<String>)],
    ) -> Option<usize> {
        candidates.iter().position(|(target, bundle_id)| {
            let authority = Authority::Grant {
                bundle_id: bundle_id.clone(),
            };
            self.may_hand_over(id, *target, &authority).is_ok()
        })
    }

    /// Claim one of the user's windows as a Handover, if `authority` still
    /// allows it, in one step with the check, so a Take Back while it
    /// moves knows its home. The ticket remembers what the claim changed:
    /// `abandon_hand_over` puts it back if the move doesn't happen, and
    /// `commit_hand_over` finishes it once the window is there.
    pub fn hand_over(
        &mut self,
        id: DisplayId,
        target: WindowTarget,
        authority: &Authority,
        home: Option<Home>,
    ) -> Result<Ticket, Refusal> {
        self.may_hand_over(id, target, authority)?;
        let revokes = self.access(id).map_or(0, |a| a.revokes);
        let d = self.get_mut(id).ok_or(Refusal::Ended)?;
        let before = d.records.get(&target.window_id).cloned();
        let was_returned = d.returned.contains(&target.window_id);
        d.claim(target, Origin::HandedOver, home);
        d.handover_serial += 1;
        let serial = d.handover_serial;
        d.handing_over.insert(target.window_id, serial);
        Ok(Ticket {
            serial,
            target,
            authority: authority.clone(),
            revokes,
            before,
            was_returned,
        })
    }

    /// Whether the record is still the one this ticket made: a Take Back
    /// or another hand-over since replaced it.
    fn still_ours(&self, id: DisplayId, ticket: &Ticket) -> bool {
        self.get(id).is_some_and(|d| {
            d.handing_over.get(&ticket.target.window_id) == Some(&ticket.serial)
                && d.records
                    .get(&ticket.target.window_id)
                    .is_some_and(|r| r.target == ticket.target)
                && !self.returned_now(id, ticket)
        })
    }

    fn returned_now(&self, id: DisplayId, ticket: &Ticket) -> bool {
        self.get(id)
            .is_some_and(|d| d.returned.contains(&ticket.target.window_id))
    }

    /// Right before the window moves: whether the hand-over still stands.
    /// A Takeover, the display ending, the user taking the window back or,
    /// for a grant, a revoke since the claim all stop it; the caller then
    /// calls `abandon_hand_over`. Changes nothing.
    pub fn confirm_hand_over(
        &self,
        id: DisplayId,
        target: WindowTarget,
        ticket: &Ticket,
    ) -> Result<(), Refusal> {
        let d = self.get(id).ok_or(Refusal::Ended)?;
        if ticket.authority == Authority::User {
            return Ok(());
        }
        if !d.is_live() {
            return Err(Refusal::Ended);
        }
        if d.takeover {
            return Err(Refusal::Takeover);
        }
        if target != ticket.target || !self.still_ours(id, ticket) {
            return Err(Refusal::Excluded);
        }
        if let Authority::Grant { bundle_id } = &ticket.authority
            && self
                .access(id)
                .is_none_or(|a| a.revokes != ticket.revokes || !a.covers(bundle_id.as_deref()))
        {
            return Err(Refusal::NotGranted);
        }
        Ok(())
    }

    /// The window is on the display: the user's own Hand Over, or their
    /// word for it, brings it back under Window Access. `gave` names the
    /// window when the user gave it with Give, from the Viewer or the Help
    /// Card: that Handover also answers the display's open Help Request
    /// (ADR 0015). A Window Request's Hand Over, a drag or a window opened
    /// from a launcher (ADR 0007) passes None. False when a Take Back or
    /// another hand-over came first; that one stands.
    pub fn commit_hand_over(
        &mut self,
        id: DisplayId,
        ticket: &Ticket,
        gave: Option<NamedWindow>,
    ) -> bool {
        if !self.still_ours(id, ticket) {
            return false;
        }
        if let Some(d) = self.get_mut(id) {
            d.handing_over.remove(&ticket.target.window_id);
        }
        if matches!(ticket.authority, Authority::User | Authority::Once)
            && let Some(access) = self.access_mut(id)
        {
            access.excluded.remove(&ticket.target.window_id);
        }
        if let Some(window) = gave.filter(|_| ticket.authority == Authority::User) {
            self.user_gave(id, window);
        }
        true
    }

    /// The window didn't move: put back what the claim changed, unless a
    /// Take Back or another hand-over has replaced it since.
    pub fn abandon_hand_over(&mut self, id: DisplayId, ticket: &Ticket) {
        if !self.still_ours(id, ticket) {
            return;
        }
        let Some(d) = self.get_mut(id) else { return };
        let window = ticket.target.window_id;
        d.handing_over.remove(&window);
        match ticket.before.clone() {
            Some(record) => {
                d.records.insert(window, record);
            }
            None => d.forget(window),
        }
        if ticket.was_returned {
            d.returned.insert(window);
        }
    }

    /// Give a Handover back to the user on their word, as Take Back does.
    /// Only a window the user handed over: never one the agent opened or
    /// placed. Returns what the give-back changed, with the record's home.
    pub fn return_handover(
        &mut self,
        id: DisplayId,
        window: WindowId,
    ) -> Result<GivenBack, String> {
        let d = self
            .get_mut(id)
            .ok_or_else(|| "the display has ended".to_string())?;
        let record = d
            .records
            .get(&window)
            .filter(|r| r.origin == Origin::HandedOver)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "window {window} isn't a Handover on this display; only windows the user handed over go back this way"
                )
            })?;
        self.give_back(id, record.target)
            .ok_or_else(|| "the display has ended".to_string())
    }

    /// The user takes a window back from display `id`, as Take Back does:
    /// it leaves the records for good, and its app becomes the user's on
    /// every display of the session. No Window Access of the session covers
    /// the window again until the user says so for it.
    pub fn give_back(&mut self, id: DisplayId, target: WindowTarget) -> Option<GivenBack> {
        if let Some(access) = self.access_mut(id) {
            access.excluded.insert(target.window_id);
        }
        let key = self.get(id)?.session().map(str::to_string);
        if let Some(key) = key {
            for other in self.of_session(&key) {
                if other != id
                    && let Some(d) = self.get_mut(other)
                    && d.started.remove(&target.pid)
                {
                    d.released.insert(target.pid);
                }
            }
        }
        Some(self.get_mut(id)?.give_back(target))
    }

    /// Whether a window off every Agent Display is display `id`'s to place:
    /// a stray of it or of another live display of its session, so a
    /// window an app the session started on one display opened on the
    /// user's screen goes onto whichever of its displays asks (ADR 0016).
    /// `recorded_elsewhere` is whether a display of another session, or
    /// one no session uses, records it.
    pub fn is_session_stray(
        &self,
        id: DisplayId,
        target: WindowTarget,
        recorded_elsewhere: bool,
        source: DisplayRole,
        held_on_source: bool,
    ) -> bool {
        let Some(d) = self.get(id) else {
            return false;
        };
        let mut displays = vec![d];
        if let Some(key) = d.session() {
            displays.extend(
                self.of_session(key)
                    .into_iter()
                    .filter(|&o| o != id)
                    .filter_map(|o| self.get(o)),
            );
        }
        displays
            .iter()
            .any(|d| d.is_stray_from(target, recorded_elsewhere, source, held_on_source))
    }

    /// Move a window's record from display `from` to display `to`, both
    /// live displays of one session, as place_window moves it between them.
    /// The record keeps its origin, Disposition and home, so a Handover
    /// stays a Handover and still goes home. A window `from` never recorded
    /// becomes `to`'s as launched when `from`'s session started its app,
    /// placed otherwise. Returns what `undo_move` needs if the move fails.
    pub fn move_between(
        &mut self,
        from: DisplayId,
        to: DisplayId,
        target: WindowTarget,
    ) -> Result<Moved, String> {
        if from == to || !self.same_session(from, to) {
            return Err(format!(
                "display {from} isn't another live display of this session"
            ));
        }
        let source = self.get(from).ok_or("the display has ended")?;
        if source.takeover {
            return Err(format!(
                "the user is operating display {from}, where the window is. Call wait_for_control on it, or ask the user to hand it back"
            ));
        }
        if source.handing_over.contains_key(&target.window_id) {
            return Err(
                "the window is being handed over right now; wait for that placement to finish"
                    .into(),
            );
        }
        let launched = source.started.contains(&target.pid);
        let record = source.records.get(&target.window_id).cloned();
        let before_to = self
            .get(to)
            .and_then(|d| d.records.get(&target.window_id).cloned());
        let moved = record.clone().unwrap_or(Record {
            target,
            origin: if launched {
                Origin::Launched
            } else {
                Origin::Placed
            },
            disposition: None,
            home: None,
        });
        if let Some(d) = self.get_mut(from) {
            d.forget(target.window_id);
        }
        if let Some(d) = self.get_mut(to) {
            d.records
                .insert(target.window_id, Record { target, ..moved });
        }
        Ok(Moved {
            from,
            to,
            target,
            record,
            before_to,
        })
    }

    /// The window didn't reach display `to`: put both records back as they
    /// were, unless a Take Back or another Placement changed them since.
    pub fn undo_move(&mut self, moved: Moved) {
        let window = moved.target.window_id;
        let untouched = |d: &Display<H>| !d.returned.contains(&window);
        if let Some(d) = self.get_mut(moved.to).filter(|d| untouched(d)) {
            match moved.before_to {
                Some(record) => {
                    d.records.insert(window, record);
                }
                None => d.forget(window),
            }
        }
        if let Some(record) = moved.record
            && let Some(d) = self.get_mut(moved.from).filter(|d| untouched(d))
            && !d.records.contains_key(&window)
        {
            d.records.insert(window, record);
        }
    }

    pub fn point(&mut self, id: DisplayId, x: f64, y: f64, click: bool) {
        if let Some(d) = self.get_mut(id) {
            let seq = d.pointer.map_or(0, |p| p.seq) + 1;
            d.pointer = Some(AgentPointer { x, y, click, seq });
        }
    }
}

// MARK: Where an app's windows are

/// The Agent Displays to point the user at when they bring an app to the
/// front: every one its windows are on, when it has a window and none is
/// on the user's screens (#51). Each entry is the Agent Display a window
/// is on, or None for a window on the user's screens. Empty otherwise.
pub fn only_on_agent_displays(windows: &[Option<DisplayId>]) -> Vec<DisplayId> {
    if windows.is_empty() || windows.iter().any(Option::is_none) {
        return Vec::new();
    }
    let mut ids: Vec<DisplayId> = windows.iter().flatten().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

// MARK: Dismissal defaults

/// What happens to one window when the user dismisses its display.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fate {
    GiveBack,
    Close,
}

/// The default for one window. `origin` is None for a window Agent Screen
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

    #[test]
    fn a_recording_hears_only_apps_wholly_on_its_display() {
        let display = Rect::new(5000.0, 0.0, 1280.0, 800.0);
        let windows = [
            (10, Rect::new(5100.0, 100.0, 300.0, 400.0), true), // agent's Calculator
            (20, Rect::new(5200.0, 100.0, 600.0, 400.0), true), // a browser window here
            (20, Rect::new(100.0, 100.0, 800.0, 600.0), false), // and the user's, minimized
            (30, Rect::new(100.0, 100.0, 800.0, 600.0), true),  // the user's music app
            // A hidden window left over where the display now is.
            (40, Rect::new(5300.0, 200.0, 300.0, 300.0), false),
            (40, Rect::new(200.0, 200.0, 300.0, 300.0), true),
        ];
        let (heard, left_out) = heard_apps(&windows, &display);
        assert_eq!(heard.into_iter().collect::<Vec<_>>(), vec![10]);
        assert_eq!(left_out.into_iter().collect::<Vec<_>>(), vec![20]);
    }

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

    #[test]
    fn a_zoomed_window_is_kept_below_the_reserved_strip() {
        // Zoom fills the whole display, toolbar under the menu bar (#103).
        let area = usable_area(Rect::new(0.0, 0.0, 1280.0, 800.0), 24.0);
        let zoomed = Rect::new(0.0, 0.0, 1280.0, 800.0);
        assert_eq!(
            zoomed.kept_inside(&area),
            Rect::new(0.0, 24.0, 1280.0, 776.0)
        );
        let below = Rect::new(100.0, 200.0, 400.0, 300.0);
        assert_eq!(below.kept_inside(&area), below);
    }

    #[test]
    fn serials_fill_the_lowest_gap() {
        let taken = |s: &[u32]| s.iter().copied().collect();
        assert_eq!(super::lowest_free_serial(1, &taken(&[])), 1);
        assert_eq!(super::lowest_free_serial(1, &taken(&[1, 2, 4])), 3);
        // A test instance's range starts higher and ignores the user's.
        assert_eq!(
            super::lowest_free_serial(10_001, &taken(&[1, 10_001])),
            10_002
        );
    }

    use super::*;
    use crate::access::{Authority, Refusal};

    #[test]
    fn screenshots_fit_the_image_budget() {
        assert_eq!(shot_scale(DEFAULT_SIZE), 1.0);
        let s = shot_scale((2560.0, 1600.0));
        assert!(s < 1.0 && 2560.0 * s <= 1568.0 && 2560.0 * 1600.0 * s * s <= 1_150_000.0);
    }

    #[test]
    fn help_steps_are_short_and_few() {
        let steps = |s: &[&str]| help_steps(s.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            steps(&[" Click the terminal ", "Type it"]).unwrap(),
            ["Click the terminal", "Type it"]
        );
        assert!(steps(&["a"; HELP_STEPS + 1]).is_err());
        assert!(steps(&["a", "  "]).is_err());
        assert!(steps(&[&"x".repeat(HELP_STEP_CHARS + 1)]).is_err());
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
        d.add(7, "Agent Screen 1".into(), DEFAULT_SIZE, owner(1), ());
        d
    }

    #[test]
    fn label_and_session_lookup() {
        let d = displays();
        assert_eq!(d.get(7).unwrap().label(), "Codex · 订机票");
        assert_eq!(d.for_session("s").map(|d| d.id), Some(7));
    }

    #[test]
    fn an_app_whose_windows_are_all_on_agent_displays_points_there() {
        assert_eq!(only_on_agent_displays(&[Some(7)]), vec![7]);
        assert_eq!(
            only_on_agent_displays(&[Some(9), Some(7), Some(9)]),
            vec![7, 9],
            "every display, once each"
        );
    }

    #[test]
    fn an_app_with_a_window_on_the_users_screen_needs_no_pointer() {
        assert!(only_on_agent_displays(&[Some(7), None]).is_empty());
        assert!(only_on_agent_displays(&[None]).is_empty());
        assert!(
            only_on_agent_displays(&[]).is_empty(),
            "no window: nothing to point at"
        );
    }

    #[test]
    fn a_repeated_request_keeps_waiting_for_the_answer() {
        let mut d = displays();
        assert!(!d.begin_takeover(7).is_empty());
        assert_eq!(d.request_control(7, "继续".into()), ControlAsk::Asked);
        let Wait::Later(mut first) = d.wait_for_control(7) else {
            panic!("should wait")
        };
        assert_eq!(
            d.request_control(7, "继续".into()),
            ControlAsk::AlreadyAsked
        );
        let Wait::Later(mut second) = d.wait_for_control(7) else {
            panic!("still waits")
        };
        assert_eq!(first.try_recv(), Ok(None), "no answer yet");
        assert!(
            !d.end_takeover(7).is_empty(),
            "the Viewer's button, or chat"
        );
        assert_eq!(
            first.try_recv().unwrap(),
            Some(crate::request::Woken::of(
                Some(crate::request::Kind::Control),
                crate::request::Resolution::HandedBack
            ))
        );
        assert_eq!(
            second.try_recv().unwrap(),
            Some(crate::request::Woken::of(
                Some(crate::request::Kind::Control),
                crate::request::Resolution::HandedBack
            ))
        );
    }

    #[test]
    fn a_control_request_ends_with_the_takeover_or_its_withdrawal() {
        let mut d = displays();
        assert!(!d.begin_takeover(7).is_empty());
        assert_eq!(d.request_control(7, "继续".into()), ControlAsk::Asked);
        assert!(d.withdraw_control_request(7));
        assert!(
            d.get(7).unwrap().takeover,
            "withdrawing doesn't end the Takeover"
        );
        assert_eq!(d.request_control(7, "继续".into()), ControlAsk::Asked);
        assert!(!d.end_takeover(7).is_empty());
        assert!(
            !d.get(7)
                .unwrap()
                .requests
                .is_open(crate::request::Kind::Control)
        );
        assert!(!d.begin_takeover(7).is_empty());
        assert_eq!(d.request_control(7, "继续".into()), ControlAsk::Asked);
        d.end(7, EndReason::Finished);
        assert!(
            !d.get(7)
                .unwrap()
                .requests
                .is_open(crate::request::Kind::Control)
        );
    }

    /// Claim and confirm, as place_window does around the move.
    fn over(
        d: &mut Displays<()>,
        id: DisplayId,
        target: WindowTarget,
        authority: &Authority,
    ) -> Result<(), Refusal> {
        let ticket = d.hand_over(id, target, authority, None)?;
        d.confirm_hand_over(id, target, &ticket)?;
        assert!(d.commit_hand_over(id, &ticket, None));
        Ok(())
    }

    #[test]
    fn what_happens_between_the_claim_and_the_move_wins() {
        let mut d = displays();
        grant_safari(&mut d);
        // A revoke, even with the app granted again after it.
        let ticket = d.hand_over(7, users(1), &safari(), None).unwrap();
        d.access_mut(7).unwrap().revoke(None).unwrap();
        grant_safari(&mut d);
        assert_eq!(
            d.confirm_hand_over(7, users(1), &ticket),
            Err(Refusal::NotGranted)
        );
        d.abandon_hand_over(7, &ticket);
        assert!(d.get(7).unwrap().records.is_empty(), "no record left");
        // A Takeover.
        let ticket = d.hand_over(7, users(1), &safari(), None).unwrap();
        assert!(!d.begin_takeover(7).is_empty());
        assert_eq!(
            d.confirm_hand_over(7, users(1), &ticket),
            Err(Refusal::Takeover)
        );
        d.abandon_hand_over(7, &ticket);
        assert!(!d.end_takeover(7).is_empty());
        // The user taking it back.
        let ticket = d.hand_over(7, users(2), &Authority::Once, None).unwrap();
        d.give_back(7, users(2)).unwrap();
        assert_eq!(
            d.confirm_hand_over(7, users(2), &ticket),
            Err(Refusal::Excluded)
        );
        assert!(
            d.access(7).unwrap().excluded.contains(&2),
            "their word doesn't outlast their own Take Back"
        );
        // The display ending.
        let ticket = d.hand_over(7, users(3), &Authority::Once, None).unwrap();
        d.end(7, EndReason::Disconnected);
        assert_eq!(
            d.confirm_hand_over(7, users(3), &ticket),
            Err(Refusal::Ended)
        );
    }

    #[test]
    fn a_hand_over_that_never_moved_keeps_the_exclusion() {
        let mut d = displays();
        d.give_back(7, users(1)).unwrap();
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(d.access(7).unwrap().excluded.contains(&1));
        assert!(!d.begin_takeover(7).is_empty());
        assert!(d.confirm_hand_over(7, users(1), &ticket).is_err());
        d.abandon_hand_over(7, &ticket);
        assert!(d.access(7).unwrap().excluded.contains(&1));
    }

    fn users(window: WindowId) -> WindowTarget {
        WindowTarget {
            pid: 60,
            window_id: window,
        }
    }

    fn safari() -> Authority {
        Authority::Grant {
            bundle_id: Some("com.apple.Safari".into()),
        }
    }

    fn grant_safari(d: &mut Displays<()>) {
        d.access_mut(7).unwrap().grant_apps([crate::access::AppId {
            bundle_id: "com.apple.Safari".into(),
            name: "Safari".into(),
        }]);
    }

    #[test]
    fn a_grant_hands_over_only_the_apps_it_names() {
        let mut d = displays();
        assert_eq!(
            over(&mut d, 7, users(1), &safari()),
            Err(Refusal::NotGranted)
        );
        grant_safari(&mut d);
        assert_eq!(over(&mut d, 7, users(1), &safari()), Ok(()));
        assert_eq!(d.get(7).unwrap().records[&1].origin, Origin::HandedOver);
        let notes = Authority::Grant {
            bundle_id: Some("com.apple.Notes".into()),
        };
        assert_eq!(over(&mut d, 7, users(2), &notes), Err(Refusal::NotGranted));
        assert_eq!(
            over(&mut d, 7, users(2), &Authority::Once),
            Ok(()),
            "the user's word for one window needs no grant"
        );
    }

    #[test]
    fn a_takeover_pauses_window_access_until_handed_back() {
        let mut d = displays();
        grant_safari(&mut d);
        assert!(!d.begin_takeover(7).is_empty());
        assert!(d.get(7).unwrap().access_paused());
        for authority in [safari(), Authority::Once] {
            assert_eq!(
                over(&mut d, 7, users(1), &authority),
                Err(Refusal::Takeover)
            );
        }
        assert!(d.get(7).unwrap().records.is_empty());
        assert!(!d.end_takeover(7).is_empty());
        assert_eq!(over(&mut d, 7, users(1), &safari()), Ok(()));
    }

    #[test]
    fn a_revoke_between_asking_and_claiming_wins() {
        let mut d = displays();
        grant_safari(&mut d);
        assert_eq!(d.may_hand_over(7, users(1), &safari()), Ok(()));
        d.access_mut(7).unwrap().revoke(None).unwrap();
        assert_eq!(
            over(&mut d, 7, users(1), &safari()),
            Err(Refusal::NotGranted)
        );
        assert!(d.get(7).unwrap().records.is_empty());
    }

    #[test]
    fn window_access_ends_with_the_display() {
        for reason in [
            EndReason::Finished,
            EndReason::Disconnected,
            EndReason::ClosedByUser,
        ] {
            let mut d = displays();
            grant_safari(&mut d);
            d.access_mut(7).unwrap().excluded.insert(3);
            assert!(d.end(7, reason));
            assert_eq!(d.access(7), None, "a Review Display has none");
            let access = &d.sessions.get("s").unwrap().access;
            assert_eq!(access.scope, None, "{reason:?}");
            assert!(access.excluded.is_empty() && access.last.is_none());
            assert_eq!(over(&mut d, 7, users(1), &safari()), Err(Refusal::Ended));
            assert_eq!(
                over(&mut d, 7, users(1), &Authority::Once),
                Err(Refusal::Ended)
            );
        }
        // A display that comes back as a spare starts with none.
        let mut d = displays();
        grant_safari(&mut d);
        d.connection_closed(1);
        d.remove(7);
        d.add(7, "Agent Screen 1".into(), DEFAULT_SIZE, owner(2), ());
        let access = d.access(7).unwrap();
        assert_eq!(access.scope, None);
        assert!(access.excluded.is_empty() && access.last.is_none());
    }

    #[test]
    fn a_window_taken_back_leaves_the_grant_until_the_user_says_so() {
        let mut d = displays();
        d.access_mut(7).unwrap().grant_all();
        assert_eq!(over(&mut d, 7, users(1), &safari()), Ok(()));
        d.give_back(7, users(1)).unwrap();
        assert_eq!(over(&mut d, 7, users(1), &safari()), Err(Refusal::Excluded));
        // Granting all apps again, or the app, doesn't undo it.
        d.access_mut(7).unwrap().revoke(None).unwrap();
        d.access_mut(7).unwrap().grant_all();
        grant_safari(&mut d);
        assert_eq!(over(&mut d, 7, users(1), &safari()), Err(Refusal::Excluded));
        // Another window of the app is still covered.
        assert_eq!(over(&mut d, 7, users(2), &safari()), Ok(()));
        // The user's word for that window brings it back.
        assert_eq!(over(&mut d, 7, users(1), &Authority::Once), Ok(()));
        assert!(!d.access(7).unwrap().excluded.contains(&1));
        d.give_back(7, users(1)).unwrap();
        // So does their own Hand Over.
        assert_eq!(over(&mut d, 7, users(1), &Authority::User), Ok(()));
        assert_eq!(over(&mut d, 7, users(1), &safari()), Ok(()));
    }

    #[test]
    fn window_access_never_reaches_another_displays_windows() {
        let mut d = displays();
        d.add(
            8,
            "Agent Screen 2".into(),
            DEFAULT_SIZE,
            Some(Owner {
                session_key: "other".into(),
                client: "Claude Code".into(),
                title: "别的".into(),
                connection: 2,
            }),
            (),
        );
        d.access_mut(7).unwrap().grant_all();
        let other = d.get_mut(8).unwrap();
        other.claim(users(1), Origin::HandedOver, None);
        other.started.insert(70);
        let launched = WindowTarget {
            pid: 70,
            window_id: 5,
        };
        for target in [users(1), launched] {
            for authority in [safari(), Authority::Once] {
                assert_eq!(over(&mut d, 7, target, &authority), Err(Refusal::Elsewhere));
            }
        }
        // Still held once that display waits for review.
        d.end(8, EndReason::Finished);
        assert_eq!(
            over(&mut d, 7, users(1), &safari()),
            Err(Refusal::Elsewhere)
        );
        // A window the user made in that app is theirs to give.
        d.get_mut(8).unwrap().user_made.insert(6);
        let made = WindowTarget {
            pid: 70,
            window_id: 6,
        };
        assert_eq!(over(&mut d, 7, made, &safari()), Ok(()));
    }

    #[test]
    fn only_a_handover_goes_back_on_the_users_word() {
        let mut d = displays();
        let home = Home {
            frame: Rect::new(10.0, 20.0, 300.0, 200.0),
            workspace: Some("2".into()),
        };
        assert_eq!(
            d.hand_over(7, users(1), &Authority::Once, Some(home.clone()))
                .map(|_| ()),
            Ok(())
        );
        let display = d.get_mut(7).unwrap();
        display.claim(
            WindowTarget {
                pid: 5,
                window_id: 2,
            },
            Origin::Launched,
            None,
        );
        display.claim(
            WindowTarget {
                pid: 6,
                window_id: 3,
            },
            Origin::Placed,
            None,
        );
        assert_eq!(display.handovers(), vec![users(1)]);
        assert!(d.return_handover(7, 2).is_err(), "the agent's own window");
        assert!(d.return_handover(7, 3).is_err(), "a placed window");
        assert!(d.get(7).unwrap().records.contains_key(&2));
        let given = d.return_handover(7, 1).unwrap();
        assert_eq!(given.home(), Some(home));
        let display = d.get(7).unwrap();
        assert!(display.handovers().is_empty());
        assert!(
            d.access(7).unwrap().excluded.contains(&1),
            "as after Take Back"
        );
        assert!(d.return_handover(7, 1).is_err(), "already back");
    }

    #[test]
    fn handovers_go_back_when_the_agent_ends_its_display() {
        for reason in [EndReason::Finished, EndReason::Disconnected] {
            let mut d = displays();
            assert!(d.hand_over(7, users(1), &Authority::Once, None).is_ok());
            let launched = WindowTarget {
                pid: 5,
                window_id: 2,
            };
            d.get_mut(7)
                .unwrap()
                .claim(launched, Origin::Launched, None);
            assert!(d.handovers_at_end(7).is_empty(), "still live");
            assert!(d.end(7, reason));
            assert_eq!(d.handovers_at_end(7), vec![users(1)], "{reason:?}");
            d.give_back(7, users(1)).unwrap();
            let display = d.get(7).unwrap();
            assert!(display.handovers().is_empty());
            assert!(
                display.records.contains_key(&2),
                "the agent's window stays for review"
            );
            assert!(d.handovers_at_end(7).is_empty(), "given back once");
        }
    }

    #[test]
    fn the_dismissal_returns_handovers_when_the_user_closes_the_display() {
        let mut d = displays();
        assert!(d.hand_over(7, users(1), &Authority::Once, None).is_ok());
        assert!(d.end(7, EndReason::ClosedByUser));
        assert!(d.handovers_at_end(7).is_empty());
        assert_eq!(d.get(7).unwrap().handovers(), vec![users(1)]);
    }

    #[test]
    fn a_window_that_couldnt_leave_shows_again_for_another_try() {
        let mut d = displays();
        let home = Home {
            frame: Rect::new(10.0, 20.0, 300.0, 200.0),
            workspace: Some("2".into()),
        };
        assert!(
            d.hand_over(7, users(1), &Authority::Once, Some(home.clone()))
                .is_ok()
        );
        let given = d.return_handover(7, 1).unwrap();
        let display = d.get_mut(7).unwrap();
        assert!(display.give_back_failed(given));
        assert_eq!(display.handovers(), vec![users(1)]);
        assert_eq!(display.records[&1].home, Some(home.clone()));
        assert!(!display.returned.contains(&1));
        // The user's word stands: no grant covers the window again.
        assert!(d.access(7).unwrap().excluded.contains(&1));
        // The second try goes home the same way.
        assert_eq!(d.return_handover(7, 1).unwrap().home(), Some(home));

        // An app the agent started stays the user's.
        let launched = WindowTarget {
            pid: 5,
            window_id: 2,
        };
        let display = d.get_mut(7).unwrap();
        display.started.insert(5);
        display.claim(launched, Origin::Launched, None);
        let given = display.give_back(launched);
        assert!(display.give_back_failed(given));
        assert_eq!(display.records[&2].origin, Origin::Launched);
        assert!(display.released.contains(&5) && !display.started.contains(&5));
    }

    #[test]
    fn a_failed_give_back_leaves_later_choices_alone() {
        let mut d = displays();
        let display = d.get_mut(7).unwrap();
        display.claim(users(1), Origin::Placed, None);
        let given = display.give_back(users(1));
        // Placed again before the failure came in.
        display.claim(users(1), Origin::HandedOver, None);
        assert!(!display.give_back_failed(given));
        assert_eq!(display.records[&1].origin, Origin::HandedOver);

        // A window the user had taken back already stays theirs.
        let display = d.get_mut(7).unwrap();
        display.give_back(users(3));
        let given = display.give_back(users(3));
        assert!(!display.give_back_failed(given));
        assert!(display.returned.contains(&3) && !display.records.contains_key(&3));
    }

    #[test]
    fn only_the_latest_give_back_can_put_a_record_back() {
        let mut d = displays();
        let display = d.get_mut(7).unwrap();
        display.claim(users(1), Origin::Placed, None);
        let first = display.give_back(users(1));
        // Handed over again and taken back again before the first move
        // home came to an end.
        display.claim(users(1), Origin::HandedOver, None);
        let second = display.give_back(users(1));
        assert!(!display.give_back_failed(first), "an outdated record");
        assert!(display.returned.contains(&1) && !display.records.contains_key(&1));
        // The second move home is the one that counts.
        assert!(display.give_back_failed(second));
        assert_eq!(display.records[&1].origin, Origin::HandedOver);
    }

    #[test]
    fn a_failed_move_leaves_what_the_user_took_back_as_it_was() {
        let mut d = displays();
        let launched = WindowTarget {
            pid: 5,
            window_id: 1,
        };
        let display = d.get_mut(7).unwrap();
        display.started.insert(5);
        display.claim(launched, Origin::Launched, None);
        d.give_back(7, launched);
        let display = d.get(7).unwrap();
        let before = (
            display.returned.clone(),
            d.access(7).unwrap().excluded.clone(),
            display.released.clone(),
        );
        let ticket = d.hand_over(7, launched, &Authority::Once, None).unwrap();
        assert!(d.confirm_hand_over(7, launched, &ticket).is_ok());
        // The OS move fails.
        d.abandon_hand_over(7, &ticket);
        let display = d.get(7).unwrap();
        assert_eq!(
            (
                display.returned.clone(),
                d.access(7).unwrap().excluded.clone(),
                display.released.clone()
            ),
            before
        );
        assert!(!display.records.contains_key(&1));
        assert!(!display.is_stray(launched, false), "still the user's");
    }

    #[test]
    fn a_window_has_only_one_pending_handover() {
        let mut d = displays();
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(matches!(
            d.hand_over(7, users(1), &Authority::User, None),
            Err(Refusal::Busy)
        ));
        d.abandon_hand_over(7, &ticket);
        assert!(d.hand_over(7, users(1), &Authority::Once, None).is_ok());
    }

    #[test]
    fn a_pending_handover_blocks_every_display() {
        let mut d = displays();
        d.add(8, "Agent Screen 2".into(), DEFAULT_SIZE, owner(2), ());
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        for authority in [Authority::User, Authority::Once, safari()] {
            assert!(matches!(
                d.hand_over(8, users(1), &authority, None),
                Err(Refusal::Busy)
            ));
        }
        assert!(!d.get(8).unwrap().records.contains_key(&1));
        d.abandon_hand_over(7, &ticket);
        assert!(d.hand_over(8, users(1), &Authority::User, None).is_ok());
    }

    #[test]
    fn a_disposition_change_does_not_cancel_the_handover() {
        let mut d = displays();
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        d.get_mut(7)
            .unwrap()
            .set_disposition(users(1), Disposition::Keep);
        assert!(d.commit_hand_over(7, &ticket, None));
        assert_eq!(
            d.get(7).unwrap().records[&1].disposition,
            Some(Disposition::Keep)
        );
    }

    #[test]
    fn an_old_failure_does_not_remove_a_new_identical_handover() {
        let mut d = displays();
        let old = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(d.commit_hand_over(7, &old, None));
        let new = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(d.commit_hand_over(7, &new, None));
        d.abandon_hand_over(7, &old);
        assert!(d.get(7).unwrap().records.contains_key(&1));
    }

    #[test]
    fn a_failed_move_keeps_an_earlier_record_and_its_home() {
        let mut d = displays();
        let home = Home {
            frame: Rect::new(1.0, 2.0, 3.0, 4.0),
            workspace: None,
        };
        let placed = WindowTarget {
            pid: 6,
            window_id: 4,
        };
        d.get_mut(7)
            .unwrap()
            .claim(placed, Origin::Placed, Some(home.clone()));
        d.access_mut(7).unwrap().grant_all();
        let ticket = d.hand_over(7, placed, &safari(), None).unwrap();
        assert!(d.confirm_hand_over(7, placed, &ticket).is_ok());
        d.abandon_hand_over(7, &ticket);
        let record = &d.get(7).unwrap().records[&4];
        assert_eq!(
            (record.origin, record.home.clone()),
            (Origin::Placed, Some(home))
        );
    }

    #[test]
    fn a_take_back_during_the_move_outlasts_the_failure() {
        let mut d = displays();
        d.access_mut(7).unwrap().grant_all();
        let ticket = d.hand_over(7, users(1), &safari(), None).unwrap();
        assert!(d.confirm_hand_over(7, users(1), &ticket).is_ok());
        d.give_back(7, users(1)).unwrap();
        d.abandon_hand_over(7, &ticket);
        let display = d.get(7).unwrap();
        assert!(display.returned.contains(&1) && d.access(7).unwrap().excluded.contains(&1));
        assert!(!display.records.contains_key(&1));
    }

    #[test]
    fn a_grant_by_app_skips_the_window_the_user_just_took_back() {
        let mut d = displays();
        grant_safari(&mut d);
        let candidates = |ids: &[WindowId]| -> Vec<(WindowTarget, Option<String>)> {
            ids.iter()
                .map(|&w| (users(w), Some("com.apple.Safari".to_string())))
                .collect()
        };
        assert_eq!(d.grantable(7, &candidates(&[1, 2])), Some(0));
        d.give_back(7, users(1)).unwrap();
        assert_eq!(
            d.grantable(7, &candidates(&[1, 2])),
            Some(1),
            "the front window was taken back; the next is covered"
        );
        assert_eq!(d.grantable(7, &candidates(&[1])), None, "ask instead");
        assert_eq!(d.grantable(7, &[]), None);
        assert!(!d.begin_takeover(7).is_empty());
        assert_eq!(d.grantable(7, &candidates(&[1, 2])), None, "paused");
    }

    #[test]
    fn only_a_hand_over_that_landed_commits() {
        let mut d = displays();
        d.give_back(7, users(1)).unwrap();
        // A Takeover after the claim: no commit, so the caller keeps the
        // app's returning catch and the exclusion stays.
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(!d.begin_takeover(7).is_empty());
        assert!(d.confirm_hand_over(7, users(1), &ticket).is_err());
        d.abandon_hand_over(7, &ticket);
        assert!(!d.commit_hand_over(7, &ticket, None));
        assert!(d.access(7).unwrap().excluded.contains(&1));
        assert!(!d.end_takeover(7).is_empty());
        // Taken back after the move: the Take Back stands.
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(d.confirm_hand_over(7, users(1), &ticket).is_ok());
        d.give_back(7, users(1)).unwrap();
        assert!(!d.commit_hand_over(7, &ticket, None));
        assert!(d.access(7).unwrap().excluded.contains(&1));
        // Landed: their word for it lifts the exclusion.
        let ticket = d.hand_over(7, users(1), &Authority::Once, None).unwrap();
        assert!(d.confirm_hand_over(7, users(1), &ticket).is_ok());
        assert!(d.commit_hand_over(7, &ticket, None));
        assert!(!d.access(7).unwrap().excluded.contains(&1));
    }

    #[test]
    fn withdrawing_help_frees_waiters() {
        let mut d = displays();
        assert!(!d.withdraw_help(7), "nothing to withdraw");
        assert!(d.request_help(7, "登录".into(), Vec::new(), true).is_some());
        let Wait::Later(mut rx) = d.wait_for_control(7) else {
            panic!("should wait")
        };
        assert!(d.withdraw_help(7));
        assert_eq!(
            rx.try_recv().unwrap().map(|w| w.resolution),
            Some(crate::request::Resolution::Withdrawn)
        );
    }

    #[test]
    fn closed_connection_ends_and_retires() {
        let mut d = displays();
        assert_eq!(d.connection_closed(1), vec![7]);
        assert!(d.for_session("s").is_none());
        assert!(
            d.begin_takeover(7).is_empty(),
            "an ended display has no session to refuse"
        );
        assert!(matches!(
            d.wait_for_control(7),
            Wait::Now(w) if w.resolution == crate::request::Resolution::Ended
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
    fn a_simulated_user_display_uses_the_users_placement_rules() {
        let mut d = displays();
        let display = d.get_mut(7).unwrap();
        let window = WindowTarget {
            pid: 5,
            window_id: 1,
        };
        let user = DisplayRole::from_hardware(0x610, 1, false);
        let agent = DisplayRole::from_hardware(0x5353, 1, false);
        let stage = DisplayRole::from_hardware(0x5353, 1, true);
        for source in [user, stage] {
            assert!(source.accepts_window_request());
            assert!(
                !display.is_stray_from(window, false, source, true),
                "not our app"
            );
        }
        display.started.insert(window.pid);
        for source in [user, stage] {
            assert!(display.is_stray_from(window, false, source, true));
            assert!(
                !display.is_stray_from(window, true, source, true),
                "recorded elsewhere"
            );
        }
        assert!(
            display.is_stray_from(window, false, agent, false),
            "AeroSpace still files the window in the user's workspace, so geometry alone is not a placement"
        );
        assert!(!agent.accepts_window_request());
        assert!(!display.is_stray_from(window, false, agent, true));
        display.give_back(window);
        for source in [user, stage] {
            assert!(
                !display.is_stray_from(window, false, source, true),
                "taken back"
            );
        }
        let closet = DisplayRole::from_hardware(0x4145, 1, true);
        assert!(!closet.accepts_window_request());
        assert!(!display.is_stray_from(window, false, closet, true));
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
        assert!(
            !display.is_stray(ours, true),
            "on another display's records"
        );
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
    fn a_recorded_window_that_left_the_display_is_a_stray() {
        let mut d = displays();
        let display = d.get_mut(7).unwrap();
        // Handed over: an app the agent didn't start.
        let given = WindowTarget {
            pid: 6,
            window_id: 2,
        };
        display.claim(given, Origin::HandedOver, None);
        assert!(display.is_stray(given, false));
        assert!(!display.is_stray(given, true));
        display.give_back(given);
        assert!(!display.is_stray(given, false), "taken back");
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

    #[test]
    fn a_frame_request_in_screenshot_pixels_lands_on_the_display() {
        use super::{FrameRequest, frame_in_shot};
        let area = Rect::new(4448.0, -870.0, 2560.0, 1600.0);
        let scale = super::shot_scale((2560.0, 1600.0));
        let current = Rect::new(4548.0, -770.0, 800.0, 600.0);
        // x, y, width and height scale up to points from the display corner.
        let all = FrameRequest {
            x: Some(40.0),
            y: Some(30.0),
            width: Some(600.0),
            height: Some(400.0),
        };
        let frame = all.resolve(current, area, scale);
        assert_eq!(frame.x, 4448.0 + 40.0 / scale);
        assert_eq!(frame.width, 600.0 / scale);
        let back = frame_in_shot(frame, area, scale);
        assert_eq!(back, Rect::new(40.0, 30.0, 600.0, 400.0));
        // A size alone keeps the window where it is.
        let size = FrameRequest {
            width: Some(250.0),
            ..Default::default()
        };
        assert_eq!(
            size.resolve(current, area, 0.5),
            Rect::new(4548.0, -770.0, 500.0, 600.0)
        );
        assert!(FrameRequest::default().is_empty());
        assert!(!size.is_empty());
    }

    #[test]
    fn an_explicit_frame_stays_below_the_same_reserved_strip_that_rejects_input() {
        use super::{FrameRequest, frame_in_shot, shot_scale};
        let area = Rect::new(4448.0, -1931.0, 1440.0, 900.0);
        let scale = shot_scale((1440.0, 900.0));
        let asked = FrameRequest {
            x: Some(0.0),
            y: Some(0.0),
            width: Some(1356.0),
            height: Some(848.0),
        };
        let resolved = asked.resolve_with_top_inset(area, area, scale, 33.0);
        assert_eq!(
            resolved.y, -1898.0,
            "a requested full-display frame must not keep its toolbar in the reserved strip"
        );
        assert_eq!(resolved.max_y(), area.max_y());
        assert!(resolved.width <= area.width);
        let response = frame_in_shot(resolved, area, scale);
        assert!(response.y >= 31.0);
        assert!(response.height < 848.0);
        let oversized = FrameRequest {
            width: Some(3000.0),
            height: Some(2000.0),
            ..Default::default()
        };
        assert_eq!(
            oversized.resolve_with_top_inset(area, area, scale, 33.0),
            Rect::new(4448.0, -1898.0, 1440.0, 867.0)
        );
        let ordinary = Rect::new(4500.0, -1800.0, 800.0, 600.0);
        let partial = FrameRequest {
            y: Some(60.0),
            ..Default::default()
        };
        let moved = partial.resolve_with_top_inset(ordinary, area, scale, 33.0);
        assert_eq!(
            moved.y,
            area.y + 60.0 / scale,
            "the inset is a constraint, not an extra coordinate offset"
        );
        assert_eq!(moved.height, ordinary.height);
        assert_eq!(moved.x, ordinary.x);
    }

    #[test]
    fn a_frame_request_never_reaches_past_the_display() {
        use super::FrameRequest;
        let area = Rect::new(5000.0, 0.0, 1280.0, 800.0);
        let current = Rect::new(5100.0, 100.0, 400.0, 300.0);
        // Growing at the right edge shifts the window back in.
        let wide = FrameRequest {
            width: Some(1000.0),
            ..Default::default()
        };
        assert_eq!(
            wide.resolve(Rect::new(5900.0, 100.0, 380.0, 300.0), area, 1.0),
            Rect::new(5280.0, 100.0, 1000.0, 300.0)
        );
        // Larger than the display: as large as the display.
        let huge = FrameRequest {
            x: Some(-50.0),
            width: Some(1320.0),
            height: Some(900.0),
            ..Default::default()
        };
        assert_eq!(
            huge.resolve(current, area, 1.0),
            Rect::new(5000.0, 0.0, 1280.0, 800.0)
        );
    }

    // MARK: One session, several displays (ADR 0016)

    fn session_owner(key: &str, connection: u64, title: &str) -> Option<Owner> {
        Some(Owner {
            session_key: key.into(),
            client: "Codex".into(),
            title: title.into(),
            connection,
        })
    }

    /// Session "s" uses displays 7 and 8; session "t" uses 9.
    fn two_of_one_session() -> Displays<()> {
        let mut d = displays();
        d.add(
            8,
            "Agent Screen 2".into(),
            DEFAULT_SIZE,
            session_owner("s", 1, "退票"),
            (),
        );
        d.add(
            9,
            "Agent Screen 3".into(),
            DEFAULT_SIZE,
            session_owner("t", 2, "Other"),
            (),
        );
        d
    }

    fn launched(window: WindowId) -> WindowTarget {
        WindowTarget {
            pid: 5,
            window_id: window,
        }
    }

    #[test]
    fn a_session_with_two_displays_reuses_the_latest() {
        let d = two_of_one_session();
        assert_eq!(d.of_session("s"), vec![7, 8]);
        assert_eq!(d.for_session("s").map(|d| d.id), Some(8));
        assert_eq!(d.of_session("t"), vec![9]);
        assert!(d.same_session(7, 8) && !d.same_session(8, 9));
        assert_ne!(d.get(7).unwrap().label(), d.get(8).unwrap().label());
    }

    #[test]
    fn the_session_authorization_covers_both_displays() {
        let mut d = two_of_one_session();
        grant_safari(&mut d);
        assert_eq!(over(&mut d, 8, users(1), &safari()), Ok(()));
        assert_eq!(
            over(&mut d, 9, users(2), &safari()),
            Err(Refusal::NotGranted),
            "another session's grant is its own"
        );
        // A revoke from either display revokes it for the session.
        d.access_mut(8).unwrap().revoke(None).unwrap();
        assert_eq!(
            over(&mut d, 7, users(3), &safari()),
            Err(Refusal::NotGranted)
        );
        // A window taken back on one display stays out of the grant on the
        // other.
        d.access_mut(7).unwrap().grant_all();
        d.give_back(8, users(1)).unwrap();
        assert_eq!(over(&mut d, 7, users(1), &safari()), Err(Refusal::Excluded));
    }

    #[test]
    fn a_takeover_pauses_the_grant_on_its_own_display_only() {
        let mut d = two_of_one_session();
        grant_safari(&mut d);
        d.begin_takeover(7);
        assert_eq!(over(&mut d, 7, users(1), &safari()), Err(Refusal::Takeover));
        assert_eq!(over(&mut d, 8, users(1), &safari()), Ok(()));
    }

    #[test]
    fn the_session_own_other_display_is_mine() {
        use crate::placement::Holder;
        let mut d = two_of_one_session();
        assert_eq!(d.get(8).unwrap().holder(Some("s")), Holder::Mine);
        assert_eq!(d.get(9).unwrap().holder(Some("s")), Holder::OtherSession);
        assert_eq!(d.get(8).unwrap().holder(Some("t")), Holder::OtherSession);
        assert_eq!(d.get(8).unwrap().holder(None), Holder::OtherSession);
        d.end(8, EndReason::Finished);
        assert_eq!(d.get(8).unwrap().holder(Some("s")), Holder::Review);
    }

    #[test]
    fn a_window_on_the_session_other_display_is_not_held_elsewhere() {
        let mut d = two_of_one_session();
        let display = d.get_mut(7).unwrap();
        display.started.insert(5);
        display.claim(launched(1), Origin::Launched, None);
        assert!(!d.owned_elsewhere(8, launched(1)), "the same session");
        assert!(d.owned_elsewhere(9, launched(1)), "another session");
        // A window of an app display 7 started, off every display, is a
        // stray of display 8 too, and never of the other session's 9.
        let window = launched(2);
        assert!(d.is_session_stray(8, window, false, DisplayRole::User, false));
        assert!(!d.is_session_stray(9, window, false, DisplayRole::User, false));
    }

    #[test]
    fn a_window_moves_between_the_session_displays_with_its_record() {
        let mut d = two_of_one_session();
        let home = Home {
            frame: Rect::new(1.0, 2.0, 300.0, 200.0),
            workspace: None,
        };
        assert!(
            d.hand_over(7, users(1), &Authority::Once, Some(home.clone()))
                .is_ok()
        );
        let ticket = d.hand_over(7, users(4), &Authority::User, None).unwrap();
        assert!(d.commit_hand_over(7, &ticket, None));
        let display = d.get_mut(7).unwrap();
        display.handing_over.clear();
        display.started.insert(5);
        // A Handover stays a Handover, with its home.
        let moved = d.move_between(7, 8, users(1)).unwrap();
        assert!(!d.get(7).unwrap().records.contains_key(&1));
        let record = &d.get(8).unwrap().records[&1];
        assert_eq!(
            (record.origin, record.home.clone()),
            (Origin::HandedOver, Some(home.clone()))
        );
        assert_eq!(d.handovers_at_end(8), Vec::new(), "not ended yet");
        // The move failed: both records are as they were.
        d.undo_move(moved);
        assert_eq!(d.get(7).unwrap().records[&1].home, Some(home));
        assert!(!d.get(8).unwrap().records.contains_key(&1));
        // A window display 7 never recorded, of an app it started.
        d.move_between(7, 8, launched(2)).unwrap();
        assert_eq!(d.get(8).unwrap().records[&2].origin, Origin::Launched);
        // Never onto another session's display, and never from a display
        // the user is operating.
        assert!(d.move_between(8, 9, launched(2)).is_err());
        d.begin_takeover(8);
        let refused = d.move_between(8, 7, launched(2)).unwrap_err();
        assert!(refused.contains("operating display 8"), "{refused}");
    }

    #[test]
    fn taking_a_window_back_releases_its_app_on_every_session_display() {
        let mut d = two_of_one_session();
        d.get_mut(7).unwrap().started.insert(5);
        d.move_between(7, 8, launched(1)).unwrap();
        d.give_back(8, launched(1)).unwrap();
        let first = d.get(7).unwrap();
        assert!(!first.started.contains(&5) && first.released.contains(&5));
        assert!(!d.is_session_stray(7, launched(2), false, DisplayRole::User, false));
    }

    #[test]
    fn ending_one_display_keeps_the_grant_for_the_other() {
        let mut d = two_of_one_session();
        d.access_mut(7).unwrap().grant_all();
        assert!(d.end(7, EndReason::Finished));
        assert_eq!(d.of_session("s"), vec![8]);
        assert_eq!(d.for_session("s").map(|d| d.id), Some(8));
        assert!(d.access(8).unwrap().scope.is_some());
        assert_eq!(over(&mut d, 8, users(1), &safari()), Ok(()));
        assert!(d.end(8, EndReason::Finished));
        assert_eq!(d.sessions.get("s").unwrap().access.scope, None);
        assert!(d.for_session("s").is_none());
    }

    #[test]
    fn a_closed_connection_ends_every_session_display_and_the_grant_once() {
        let mut d = two_of_one_session();
        d.access_mut(7).unwrap().grant_all();
        let revokes = d.access(7).unwrap().revokes;
        assert_eq!(d.connection_closed(1), vec![7, 8]);
        assert!(d.get(7).unwrap().is_ended() && d.get(8).unwrap().is_ended());
        let access = &d.sessions.get("s").unwrap().access;
        assert_eq!(access.scope, None);
        assert_eq!(access.revokes, revokes + 1, "cleared once");
        assert!(d.of_session("s").is_empty());
        assert!(d.get(9).unwrap().is_live(), "another session's display");
    }
}
