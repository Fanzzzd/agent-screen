//! Agent Displays and what Agent Screen knows about them, independent of the
//! platform. The vocabulary follows CONTEXT.md.
//!
//! `H` is the platform's handle that keeps a display alive. Tests use `()`.

use futures::channel::oneshot;
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

/// The agent asks for one of the user's windows, one it didn't start
/// (ADR 0007). The user hands it over or turns the request down.
#[derive(Debug)]
pub struct WindowRequest {
    pub target: WindowTarget,
    pub app: String,
    pub title: String,
    answer: Option<oneshot::Sender<bool>>,
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
    /// The user answered a Control Request with Keep Control.
    Kept,
}

impl ControlWait {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlWait::Returned => "returned",
            ControlWait::TimedOut => "timedOut",
            ControlWait::Ended => "ended",
            ControlWait::Kept => "kept",
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
    /// The reason of an open Help Request.
    pub help_request: Option<String>,
    /// Its steps, shown in the Viewer while the user is in control.
    pub help_steps: Vec<String>,
    /// An open Window Request: the agent asks for a window it didn't start.
    pub window_request: Option<WindowRequest>,
    /// The reason of an open Control Request: during a Takeover, the agent
    /// asks the user to hand the display back.
    pub control_request: Option<String>,
    /// The agent is typing through the user's input method, which takes
    /// the user's keyboard for that time (ADR 0008).
    pub keyboard_borrowed: bool,
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
}

impl<H> Default for Displays<H> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
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
            help_steps: Vec::new(),
            window_request: None,
            control_request: None,
            keyboard_borrowed: false,
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
        display.help_steps.clear();
        // Its waiter hears the request went unanswered.
        display.window_request = None;
        display.control_request = None;
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
                d.control_request = None;
                d.help_request = None;
                d.help_steps.clear();
                d.resume_waiters(ControlWait::Returned);
                true
            }
            _ => false,
        }
    }

    pub fn request_help(&mut self, id: DisplayId, reason: String, steps: Vec<String>) -> bool {
        match self.get_mut(id) {
            Some(d) if d.is_live() => {
                d.help_request = Some(reason);
                d.help_steps = steps;
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
        d.help_steps.clear();
        if withdrawn && !d.takeover {
            d.resume_waiters(ControlWait::Returned);
        }
        withdrawn
    }

    /// Ask the user for a window, replacing an earlier request, whose
    /// waiter then hears nothing. The receiver gets the user's answer:
    /// true to hand the window over.
    pub fn request_window(
        &mut self,
        id: DisplayId,
        target: WindowTarget,
        app: String,
        title: String,
    ) -> Option<oneshot::Receiver<bool>> {
        let d = self.get_mut(id).filter(|d| d.is_live())?;
        let (tx, rx) = oneshot::channel();
        d.window_request = Some(WindowRequest {
            target,
            app,
            title,
            answer: Some(tx),
        });
        Some(rx)
    }

    /// During a Takeover, ask the user to hand the display back, replacing
    /// an earlier request's reason. False when the agent has control
    /// already, or the display isn't live: nothing to ask.
    pub fn request_control(&mut self, id: DisplayId, reason: String) -> bool {
        match self.get_mut(id) {
            Some(d) if d.is_live() && d.takeover => {
                d.control_request = Some(reason);
                true
            }
            _ => false,
        }
    }

    /// The agent no longer needs the display back. The Takeover goes on.
    pub fn withdraw_control_request(&mut self, id: DisplayId) -> bool {
        self.get_mut(id)
            .is_some_and(|d| d.control_request.take().is_some())
    }

    /// The user answered the display's Control Request: `give` hands the
    /// display back, as the Viewer's button does; otherwise the Takeover
    /// goes on and waiters hear the user kept control. False when there
    /// was no request.
    pub fn answer_control(&mut self, id: DisplayId, give: bool) -> bool {
        let Some(d) = self.get_mut(id) else {
            return false;
        };
        if d.control_request.take().is_none() {
            return false;
        }
        if give {
            self.end_takeover(id);
        } else {
            d.resume_waiters(ControlWait::Kept);
        }
        true
    }

    /// The user answered the display's Window Request. Returns the window
    /// to hand over when they gave it.
    pub fn answer_window(&mut self, id: DisplayId, give: bool) -> Option<WindowTarget> {
        let mut request = self.get_mut(id)?.window_request.take()?;
        if let Some(answer) = request.answer.take() {
            let _ = answer.send(give);
        }
        give.then_some(request.target)
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

    #[test]
    fn steps_go_with_the_help_request() {
        let mut d = displays();
        assert!(d.request_help(7, "登录".into(), vec!["点终端".into()]));
        assert!(d.begin_takeover(7));
        assert_eq!(d.get(7).unwrap().help_steps, ["点终端"]);
        assert!(d.end_takeover(7));
        assert!(d.get(7).unwrap().help_steps.is_empty());
    }

    #[test]
    fn a_window_request_hears_the_users_answer() {
        let mut d = displays();
        let target = WindowTarget {
            pid: 40,
            window_id: 9,
        };
        let mut no = d
            .request_window(7, target, "UTM".into(), "Ubuntu".into())
            .unwrap();
        assert_eq!(d.answer_window(7, false), None);
        assert_eq!(no.try_recv(), Ok(Some(false)));
        assert!(d.get(7).unwrap().window_request.is_none());

        let mut yes = d
            .request_window(7, target, "UTM".into(), "Ubuntu".into())
            .unwrap();
        assert_eq!(d.answer_window(7, true), Some(target));
        assert_eq!(yes.try_recv(), Ok(Some(true)));
        assert_eq!(d.answer_window(7, true), None);
    }

    #[test]
    fn a_window_request_ends_with_its_display() {
        let mut d = displays();
        let target = WindowTarget {
            pid: 40,
            window_id: 9,
        };
        let mut rx = d
            .request_window(7, target, "UTM".into(), "".into())
            .unwrap();
        d.end(7, EndReason::Finished);
        assert!(rx.try_recv().is_err());
        assert!(
            d.request_window(7, target, "UTM".into(), "".into())
                .is_none()
        );
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
    fn takeover_resumes_waiters_and_clears_help() {
        let mut d = displays();
        assert!(d.request_help(7, "登录".into(), Vec::new()));
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
    fn a_control_request_needs_a_takeover() {
        let mut d = displays();
        assert!(
            !d.request_control(7, "继续订票".into()),
            "the agent has control already"
        );
        assert!(d.get(7).unwrap().control_request.is_none());
    }

    #[test]
    fn continue_hands_the_display_back() {
        let mut d = displays();
        assert!(d.begin_takeover(7));
        assert!(d.request_control(7, "继续订票".into()));
        assert!(
            d.request_control(7, "继续订票，已登录".into()),
            "a new reason replaces the old"
        );
        let Wait::Later(mut rx) = d.wait_for_control(7) else {
            panic!("should wait")
        };
        assert!(d.answer_control(7, true));
        assert_eq!(rx.try_recv().unwrap(), Some(ControlWait::Returned));
        let display = d.get(7).unwrap();
        assert!(!display.takeover && display.control_request.is_none());
    }

    #[test]
    fn keep_control_leaves_the_takeover_and_tells_the_waiter() {
        let mut d = displays();
        assert!(d.begin_takeover(7));
        assert!(d.request_control(7, "继续订票".into()));
        let Wait::Later(mut rx) = d.wait_for_control(7) else {
            panic!("should wait")
        };
        assert!(d.answer_control(7, false));
        assert_eq!(rx.try_recv().unwrap(), Some(ControlWait::Kept));
        assert!(d.get(7).unwrap().takeover, "only the user ends a Takeover");
        assert!(!d.answer_control(7, true), "the request is gone");
        assert!(d.get(7).unwrap().takeover);
    }

    #[test]
    fn a_control_request_ends_with_the_takeover_or_its_withdrawal() {
        let mut d = displays();
        assert!(d.begin_takeover(7));
        assert!(d.request_control(7, "继续".into()));
        assert!(d.withdraw_control_request(7));
        assert!(
            d.get(7).unwrap().takeover,
            "withdrawing doesn't end the Takeover"
        );
        assert!(d.request_control(7, "继续".into()));
        assert!(d.end_takeover(7));
        assert!(d.get(7).unwrap().control_request.is_none());
        assert!(d.begin_takeover(7));
        assert!(d.request_control(7, "继续".into()));
        d.end(7, EndReason::Finished);
        assert!(d.get(7).unwrap().control_request.is_none());
    }

    #[test]
    fn withdrawing_help_frees_waiters() {
        let mut d = displays();
        assert!(!d.withdraw_help(7), "nothing to withdraw");
        assert!(d.request_help(7, "登录".into(), Vec::new()));
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
}
