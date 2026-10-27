//! The app's state on the main thread: Agent Displays with their platform
//! handles, input queues and the latest element trees.

use crate::mac::ax;
use crate::mac::colorsync;
use crate::mac::display::{self, VirtualDisplay};
use crate::mac::input::Forwarder;
use crate::mac::windows;
use crate::model::{DisplayId, Displays, EndReason, Owner, WindowId};
use crate::numbers::Numbers;
use agent_screen_model::budget::{Budget, ColorState, MadeLimit};
use gpui::{App, AsyncApp, Context, Entity};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Adding or removing a display is a reconfiguration every app on the Mac
/// handles, and while a virtual display is connected each one leaves
/// ColorSync answering more requests from apps for minutes. Dozens in a
/// short while slow the whole Mac down. So an ended, empty display stays
/// connected as a spare for the next display of its size, up to `SPARES` of
/// them for `SPARE_LIFE`, and new displays are limited to `MADE` per
/// `MADE_WINDOW` for agents.
const SPARES: usize = 3;
const SPARE_LIFE: Duration = Duration::from_secs(10 * 60);
const MADE: usize = 10;
/// Each display added or removed stops every window on the Mac from
/// drawing for 0.2 to 0.9 s, Core Animation's own animations too; keeping
/// or reusing a spare stops nothing. So displays are released, and the
/// spare that keeps the next "+" from adding one is made, only once the
/// user has been away from the keyboard and pointer for `IDLE`, checked
/// every `TIDY_EVERY`.
const IDLE: Duration = Duration::from_secs(60);
const TIDY_EVERY: Duration = Duration::from_secs(15);
/// `READY` spares stay ready while displays were used within `WARM`, so
/// that many "+" in a row add no display.
const WARM: Duration = Duration::from_secs(60 * 60);
const READY: usize = 2;
const MADE_WINDOW: Duration = Duration::from_secs(10 * 60);

/// An ended display kept connected for the next one of its size. Nothing
/// shows it: it isn't an Agent Display until it's taken.
pub struct Spare {
    handle: VirtualDisplay,
    name: String,
    size: (f64, f64),
    since: Instant,
}

pub struct Core {
    pub displays: Displays<VirtualDisplay>,
    forwarders: HashMap<DisplayId, Forwarder>,
    /// The numbered elements of each window an agent has looked at, by
    /// display. Dropped with the display.
    pub elements: HashMap<(DisplayId, WindowId), Numbers<ax::Element>>,
    /// Connections that have closed. A request from one can still be queued
    /// behind its close, and must not create a display nobody owns.
    pub closed: HashSet<u64>,
    /// The Agent Session each connection created or reused a display for.
    /// A connection may only use displays its session owns.
    pub sessions: HashMap<u64, String>,
    /// Displays moved to where they stay. Until then a new display can sit
    /// over one of the user's windows parked off screen.
    pub tucked: HashSet<DisplayId>,
    /// Tries at parking a moved display again, since it was last in place.
    reparks: HashMap<DisplayId, u8>,
    /// Tries at giving a display its size back, since it last ran at it.
    remodes: HashMap<DisplayId, u8>,
    /// Apps an Agent Session is launching. Two launches of one app at once
    /// can't tell their new windows apart, so they take turns.
    pub launching: HashSet<std::path::PathBuf>,
    /// Catches on apps launch_app started, kept while their display lives:
    /// a window such an app opens later, as TextEdit does for a second
    /// `open`, goes to the agent's display instead of the user's screen.
    pub kept_catches: Vec<(DisplayId, crate::mac::catch::Catch)>,
    /// For apps the user took back: returns their new windows that open on
    /// the display by themselves.
    pub returning: Vec<(DisplayId, crate::mac::catch::Catch)>,
    /// Take-backs whose returning catch is still to be armed, once the
    /// window is back. A Handover or the display's removal meanwhile
    /// cancels it.
    pub pending_returns: std::collections::HashSet<(DisplayId, i32)>,
    /// Files a running app is still opening, by pid, each watched once.
    pub watching: HashSet<(i32, std::path::PathBuf)>,
    /// Undecided windows of the kept catches already reported to an agent.
    pub told: HashSet<(DisplayId, u32)>,
    /// Windows `adopt_opened` left on the user's screen because the user
    /// had just acted on the app, by display, until reported.
    pub left_alone: Vec<(DisplayId, u32)>,
    /// Ended displays kept for reuse.
    spares: Vec<Spare>,
    /// Ended displays to release once the user is idle.
    retiring: Vec<VirtualDisplay>,
    /// When a display was last taken on, new or spare.
    last_used: Option<Instant>,
    /// A spare is being made while the user is idle.
    warming: bool,
    /// When agents' displays were made new, within `MADE_WINDOW`.
    made: MadeLimit,
    reclaim: Reclaim,
}

/// Runs of the ColorSync helper (ADR 0009): one at a time, those asked for
/// meanwhile merged into one after it.
#[derive(Default)]
struct Reclaim {
    due: bool,
    running: bool,
    again: bool,
    /// Display records ColorSync keeps, from the last run or count.
    records: Option<usize>,
    helper: Option<colorsync::Helper>,
}

/// A new display waits this long for its record, and merges with others.
const RECLAIM_DELAY: Duration = Duration::from_secs(3);
const RECLAIM_EVERY: Duration = Duration::from_secs(30 * 60);

impl Core {
    pub fn new() -> Self {
        Core {
            displays: Displays::default(),
            forwarders: HashMap::new(),
            elements: HashMap::new(),
            closed: HashSet::new(),
            sessions: HashMap::new(),
            tucked: HashSet::new(),
            reparks: HashMap::new(),
            remodes: HashMap::new(),
            launching: HashSet::new(),
            kept_catches: Vec::new(),
            returning: Vec::new(),
            pending_returns: Default::default(),
            watching: HashSet::new(),
            told: HashSet::new(),
            left_alone: Vec::new(),
            spares: Vec::new(),
            retiring: Vec::new(),
            last_used: None,
            warming: false,
            made: MadeLimit::new(MADE, MADE_WINDOW),
            reclaim: Reclaim::default(),
        }
    }

    /// Run the ColorSync helper soon. Without it, count the records.
    pub fn reclaim_soon(&mut self, cx: &mut Context<Self>) {
        if self.reclaim.running {
            self.reclaim.again = true;
            return;
        }
        if self.reclaim.due {
            return;
        }
        self.reclaim.due = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RECLAIM_DELAY).await;
            let _ = this.update(cx, |core, _| {
                core.reclaim.due = false;
                core.reclaim.running = true;
            });
            let (helper, result) = cx
                .background_executor()
                .spawn(async move {
                    let helper = colorsync::helper();
                    let result = match helper {
                        colorsync::Helper::Ready => colorsync::reclaim(),
                        _ => Ok(colorsync::Report {
                            records: colorsync::count_records(),
                            ..Default::default()
                        }),
                    };
                    (helper, result)
                })
                .await;
            let _ = this.update(cx, |core, cx| {
                core.reclaim.running = false;
                core.reclaim.helper = Some(helper);
                match result {
                    Ok(report) => {
                        core.reclaim.records = Some(report.records);
                        if helper == colorsync::Helper::Ready {
                            crate::lifecycle::line(&format!("colorsync {}", report.line));
                        } else {
                            crate::lifecycle::line(&format!(
                                "colorsync helper {helper:?}; {} display records",
                                report.records
                            ));
                        }
                    }
                    Err(error) => {
                        log::warn!("ColorSync helper: {error}");
                        crate::lifecycle::line(&format!("colorsync failed: {error}"));
                    }
                }
                if std::mem::take(&mut core.reclaim.again) {
                    core.reclaim_soon(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The helper's state from the last run, if one ran.
    pub fn colorsync_helper(&self) -> Option<colorsync::Helper> {
        self.reclaim.helper
    }

    /// Whether one display more keeps WindowServer's color request in
    /// time (ADR 0009). Releases spares when that is what it takes.
    pub fn budget_allows(&mut self) -> Result<(), String> {
        let state = ColorState {
            records: self.reclaim.records,
            helper_ready: self.reclaim.helper == Some(colorsync::Helper::Ready),
        };
        let online = crate::mac::cg::online().len();
        match state.allows(online, self.spares.len()) {
            Budget::Allow => Ok(()),
            Budget::ReleaseSpares => {
                for spare in std::mem::take(&mut self.spares) {
                    crate::lifecycle::note("released", spare.handle.id, "spare over budget");
                }
                Ok(())
            }
            Budget::Refuse => Err(format!(
                "This Mac has {online} displays online, and with another, macOS would take longer than it allows to look up their color profiles on every display change, so every app would stutter. Keep working on a display you have, or end one you no longer need with end_display."
            )),
        }
    }

    /// Ids of every virtual display of this app, spares included: they
    /// take room where displays park.
    pub fn virtual_ids(&self) -> Vec<DisplayId> {
        self.displays
            .list
            .iter()
            .map(|d| d.id)
            .chain(self.spares.iter().map(|s| s.handle.id))
            .chain(self.retiring.iter().map(|d| d.id))
            .chain(display::filler_ids())
            .collect()
    }

    /// A spare to use instead of a new display, with its name: one of
    /// `size`, or else the newest, which the caller resizes. One that macOS
    /// no longer shows, or that now shares its unit number, goes instead.
    pub fn take_spare(&mut self, size: (f64, f64)) -> Option<(VirtualDisplay, String, bool)> {
        loop {
            let index = self
                .spares
                .iter()
                .position(|s| s.size == size)
                .or_else(|| self.spares.len().checked_sub(1))?;
            let spare = self.spares.remove(index);
            let id = spare.handle.id;
            if crate::mac::cg::active_displays().contains(&id)
                && crate::mac::cg::unit_twin(id).is_none()
            {
                let resize = spare.size != size;
                self.last_used = Some(Instant::now());
                crate::lifecycle::note(
                    "reused",
                    id,
                    &if resize {
                        format!("resizing to {}x{}", size.0, size.1)
                    } else {
                        String::new()
                    },
                );
                return Some((spare.handle, spare.name, resize));
            }
            crate::lifecycle::note("released", id, "spare unusable");
        }
    }

    /// Whether an agent may have a new display now; otherwise how long
    /// until it may.
    pub fn may_make(&mut self) -> Result<(), Duration> {
        // Test builds may raise it, as the fleet script does for its twelve.
        let limit = if cfg!(debug_assertions) {
            std::env::var("AGENTSCREEN_NEW_DISPLAYS")
                .ok()
                .and_then(|v| v.parse().ok())
        } else {
            None
        };
        self.made.allows(Instant::now(), limit)
    }

    /// An agent's display was made new.
    pub fn count_made(&mut self) {
        self.made.record(Instant::now());
    }

    /// While the user is idle: release what waited for it, spares kept
    /// longer than `SPARE_LIFE` among them, and make spares when displays
    /// were used lately and fewer than `READY` are ready.
    fn tidy(&mut self, cx: &mut Context<Self>) {
        if crate::mac::ime::user_idle() < idle().as_secs_f64() {
            return;
        }
        let mut released = false;
        for display in std::mem::take(&mut self.retiring) {
            crate::lifecycle::note("released", display.id, "ended, released while idle");
            released = true;
        }
        let warm = self.last_used.is_some_and(|t| t.elapsed() < WARM);
        let (mut old, mut kept): (Vec<Spare>, Vec<Spare>) = std::mem::take(&mut self.spares)
            .into_iter()
            .partition(|s| s.since.elapsed() >= SPARE_LIFE);
        // The newest stay as the ready ones.
        while warm
            && kept.len() < READY
            && let Some(spare) = old.pop()
        {
            kept.push(spare);
        }
        self.spares = kept;
        for spare in old {
            crate::lifecycle::note("released", spare.handle.id, "spare unused");
            released = true;
        }
        if !warm && self.displays.list.is_empty() && self.spares.is_empty() {
            released |= !display::filler_ids().is_empty();
            display::release_fillers();
        }
        if released {
            self.reclaim_soon(cx);
        }
        // One at a time, a tidy apart.
        if warm && self.spares.len() < READY && !self.warming && self.budget_allows().is_ok() {
            self.make_spare(cx);
        }
    }

    /// Make a spare of the size "+" makes, parked like any display.
    fn make_spare(&mut self, cx: &mut Context<Self>) {
        self.warming = true;
        let (serial, name) = self.next_name();
        let others = self.virtual_ids();
        let size = crate::model::DEFAULT_SIZE;
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(display::create_parked(
                    name.clone(),
                    size.0 as u32,
                    size.1 as u32,
                    serial,
                    others,
                ))
                .await;
            let _ = this.update(cx, |core, cx| {
                core.warming = false;
                let Some(handle) = made else {
                    return;
                };
                crate::lifecycle::note("kept", handle.id, "spare made while idle");
                if let Some((serial, slot)) = handle.serial() {
                    colorsync::remember(serial, slot, handle.id);
                }
                core.spares.push(Spare {
                    handle,
                    name,
                    size,
                    since: Instant::now(),
                });
                core.reclaim_soon(cx);
            });
        })
        .detach();
    }

    /// The user left the Viewer: each display hands key focus back to the
    /// user's windows its keys took it from.
    pub fn hand_back_keys(&self) {
        for forwarder in self.forwarders.values() {
            forwarder.hand_back();
        }
    }

    /// The display's input queue, if it has one yet.
    pub fn existing_forwarder(&self, id: DisplayId) -> Option<Forwarder> {
        self.forwarders.get(&id).cloned()
    }

    /// One input queue per display, so each display remembers its own
    /// focused window. The Viewer and the Agent Session of a display share it.
    pub fn forwarder(&mut self, id: DisplayId) -> Forwarder {
        self.forwarders.entry(id).or_default().clone()
    }

    /// The serial and name for the next display. A test instance's
    /// displays must not pass for the user's.
    pub fn next_name(&mut self) -> (display::Serial, String) {
        let instance = agent_screen_mcp::endpoint::instance();
        let first = if instance.is_some() { 10_001 } else { 1 };
        let serial = display::Serial::lease(first);
        let number = serial.get() + 1 - first;
        let name = match instance {
            Some(instance) => format!("Agent Screen {instance} {number}"),
            None => format!("Agent Screen {number}"),
        };
        (serial, name)
    }

    /// Take on a display made off the main thread, as macOS takes about
    /// half a second, and park it out of the cursor's way.
    pub fn adopt(
        &mut self,
        handle: VirtualDisplay,
        name: String,
        size: (f64, f64),
        owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) -> DisplayId {
        let id = handle.id;
        if let Some((serial, slot)) = handle.serial() {
            colorsync::remember(serial, slot, id);
        }
        self.displays.add(id, name, size, owner, handle);
        self.last_used = Some(Instant::now());
        // Its record is new and has all 39 names.
        self.reclaim_soon(cx);
        label_soon(id, cx);
        // A reused display is already in place: moving it again would be
        // another reconfiguration.
        let virtual_ids = self.virtual_ids();
        if display::is_tucked(id, &virtual_ids) {
            self.tucked.insert(id);
            cx.notify();
            return id;
        }
        // WindowServer usually takes the position at once; a display it has
        // only just added needs a moment first, so try until it does. On
        // the display thread: each try waits on WindowServer.
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let mut tries = 0;
            let mut parked = false;
            for attempt in 0..75 {
                // At once, then every 20 ms: each try that WindowServer takes
                // ends the wait for create_display.
                if attempt > 0 {
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                }
                tries += 1;
                let Ok(virtual_ids) = this.read_with(cx, |core, _| core.virtual_ids()) else {
                    return;
                };
                let placed = display::configure(move || {
                    park_fillers(&virtual_ids);
                    display::tuck_away(id, &virtual_ids)
                })
                .await;
                if placed {
                    parked = true;
                    break;
                }
            }
            if parked {
                log::debug!(
                    "display {id} parked after {tries} tries, {} ms",
                    started.elapsed().as_millis()
                );
            } else {
                // create_display answers anyway: the display works where it
                // is, but the user's pointer may wander onto it.
                log::warn!("display {id} could not be parked after {tries} tries");
            }
            let _ = this.update(cx, |core, _| core.tucked.insert(id));
        })
        .detach();
        cx.notify();
        id
    }

    /// The displays changed. Park again any display of ours that is no
    /// longer out of the way, as when the user dragged it in System
    /// Settings. Each move is a display change too, so a display macOS
    /// keeps putting elsewhere gets three tries, not a loop.
    pub fn repark(&mut self, cx: &mut Context<Self>) {
        let virtual_ids = self.virtual_ids();
        let mut moving = Vec::new();
        let ours: Vec<DisplayId> = self.displays.list.iter().map(|d| d.id).collect();
        for &id in &ours {
            if !self.tucked.contains(&id) || display::is_tucked(id, &virtual_ids) {
                self.reparks.remove(&id);
                continue;
            }
            let tries = self.reparks.entry(id).or_default();
            if *tries >= 3 {
                continue;
            }
            *tries += 1;
            moving.push(id);
        }
        cx.background_executor()
            .spawn(display::configure(move || {
                park_fillers(&virtual_ids);
                for id in moving {
                    let parked = display::tuck_away(id, &virtual_ids);
                    log::debug!("display {id} was moved; parked again: {parked}");
                }
            }))
            .detach();
    }

    /// Give each display its size back if it runs at another (issue #33).
    /// WindowServer can switch a virtual display to another of the modes
    /// macOS lists for it, such as 800×600, under load; the agent's
    /// screenshots and the windows on it then no longer fit.
    pub fn hold_modes(&mut self, cx: &mut Context<Self>) {
        let mut wrong = Vec::new();
        for d in &self.displays.list {
            let (width, height) = (d.width as u32, d.height as u32);
            let now = crate::mac::cg::display_bounds(d.id);
            if (now.width as u32, now.height as u32) == (width, height) {
                self.remodes.remove(&d.id);
                continue;
            }
            let tries = self.remodes.entry(d.id).or_default();
            if *tries >= 3 {
                continue;
            }
            *tries += 1;
            wrong.push((d.id, width, height));
        }
        if wrong.is_empty() {
            return;
        }
        cx.background_executor()
            .spawn(display::configure(move || {
                for (id, width, height) in wrong {
                    let was = crate::mac::cg::display_bounds(id);
                    let back = display::restore_mode(id, width, height);
                    crate::lifecycle::note(
                        "mode",
                        id,
                        &format!(
                            "ran at {}x{}, wants {width}x{height}, back: {back}",
                            was.width, was.height
                        ),
                    );
                }
            }))
            .detach();
    }

    pub fn end(&mut self, id: DisplayId, reason: EndReason, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        crate::mac::cg::clear_user_stage(id);
        if self.displays.end(id, reason) {
            // An ended display takes no more windows, from catches either.
            self.kept_catches.retain(|(display, _)| *display != id);
            stop_recording(id, cx);
            cx.notify();
            schedule_sweep(cx);
        }
    }

    /// The user took the display over, or handed it back: an agent's
    /// typing or drag in flight stops while they're in control.
    pub fn stop_agent_input(&self, id: DisplayId, stop: bool) {
        crate::mac::recording::note(
            id,
            serde_json::json!({ "action": if stop { "takeoverStart" } else { "takeoverEnd" } }),
        );
        if let Some(forwarder) = self.forwarders.get(&id) {
            forwarder.stop_agent(stop);
        }
        if stop {
            crate::control::agent::release_hold(id);
        }
    }

    pub fn connection_closed(&mut self, connection: u64, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        crate::mac::cg::clear_user_stage_connection(connection);
        self.closed.insert(connection);
        self.sessions.remove(&connection);
        let ended = self.displays.connection_closed(connection);
        if !ended.is_empty() {
            // As in `end`: an ended display takes no more windows.
            self.kept_catches
                .retain(|(display, _)| !ended.contains(display));
            for &id in &ended {
                stop_recording(id, cx);
            }
            cx.notify();
            schedule_sweep(cx);
        }
    }

    /// Drop a display. An empty one stays connected as a spare while there
    /// is room; otherwise macOS removes it with its handle.
    pub fn remove(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        crate::mac::cg::clear_user_stage(id);
        stop_recording(id, cx);
        if let Some(display) = self.displays.remove(id) {
            let size = (display.width, display.height);
            let keep = self.spares.len() < SPARES
                && windows::on_display(id).is_empty()
                && crate::mac::cg::unit_twin(id).is_none();
            if keep {
                crate::lifecycle::note("kept", id, "spare");
                self.spares.push(Spare {
                    handle: display.handle,
                    name: display.name,
                    size,
                    since: Instant::now(),
                });
            } else if windows::on_display(id).is_empty() {
                // Released once the user is idle, so the Mac doesn't stop
                // drawing while they look.
                crate::lifecycle::note("kept", id, "retiring");
                self.retiring.push(display.handle);
            } else {
                // Windows left on it go to the user's screen now.
                crate::lifecycle::note("released", id, "");
                self.reclaim_soon(cx);
            }
            self.forwarders.remove(&id);
            self.elements.retain(|(display, _), _| *display != id);
            self.tucked.remove(&id);
            self.reparks.remove(&id);
            self.remodes.remove(&id);
            self.kept_catches.retain(|(display, _)| *display != id);
            self.returning.retain(|(display, _)| *display != id);
            self.pending_returns.retain(|(display, _)| *display != id);
            self.told.retain(|(display, _)| *display != id);
            self.left_alone.retain(|(display, _)| *display != id);
            cx.notify();
        }
    }

    /// Ended displays with no window left close by themselves.
    /// The window lists run off the UI thread.
    pub fn sweep_ended(&mut self, cx: &mut Context<Self>) {
        let ended: Vec<DisplayId> = self
            .displays
            .list
            .iter()
            .filter(|d| d.is_ended())
            .map(|d| d.id)
            .collect();
        if ended.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let empty: Vec<DisplayId> = cx
                .background_executor()
                .spawn(async move {
                    ended
                        .into_iter()
                        .filter(|&id| windows::on_display(id).is_empty())
                        .collect()
                })
                .await;
            let _ = this.update(cx, |core, cx| {
                for id in empty {
                    // Still ended: the user may have taken it back meanwhile.
                    if core.displays.get(id).is_some_and(|d| d.is_ended()) {
                        core.remove(id, cx);
                    }
                }
            });
        })
        .detach();
    }
}

/// Sweep after the windows had a moment to go.
fn schedule_sweep(cx: &mut Context<Core>) {
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(600))
            .await;
        let _ = this.update(cx, |core, cx| core.sweep_ended(cx));
    })
    .detach();
}

/// A recording ends with its Agent Session's use of the display. Writing
/// the video out takes a moment, off the main thread.
fn stop_recording(id: DisplayId, cx: &mut Context<Core>) {
    if crate::mac::recording::is_recording(id) {
        cx.background_executor()
            .spawn(async move { crate::mac::recording::stop(id) })
            .detach();
    }
}

/// Once a second while this app has displays, move the system's alerts and
/// prompts that landed on one to the user's main display.
/// Run the ColorSync helper 10 s after launch and every 30 minutes
/// (ADR 0009). Display changes run it too.
pub fn watch_colorsync(core: Entity<Core>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut wait = Duration::from_secs(10);
        loop {
            cx.background_executor().timer(wait).await;
            core.update(cx, |core, cx| core.reclaim_soon(cx));
            wait = RECLAIM_EVERY;
        }
    })
    .detach();
}

/// How long the user must be away before displays change: `IDLE`, or
/// AGENTSCREEN_IDLE_SECS in a test build.
fn idle() -> Duration {
    if cfg!(debug_assertions)
        && let Some(secs) = std::env::var("AGENTSCREEN_IDLE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
    {
        return Duration::from_secs(secs);
    }
    IDLE
}

/// Releases and the ready spare wait for the user to be idle (`tidy`).
pub fn watch_idle(core: Entity<Core>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(TIDY_EVERY).await;
            core.update(cx, |core, cx| core.tidy(cx));
        }
    })
    .detach();
}

pub fn watch_alerts(core: Entity<Core>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let ids = core.read_with(cx, |c, _| c.virtual_ids());
            if ids.is_empty() {
                continue;
            }
            let moved = cx
                .background_executor()
                .spawn(async move { crate::mac::alerts::send_home(&ids) })
                .await;
            for alert in moved {
                log::info!(
                    "moved the system alert {alert} from an agent display to the main display"
                );
            }
        }
    })
    .detach();
}

/// Park the kept twins out of the way like any display of Agent Screen's.
pub fn park_fillers(virtual_ids: &[DisplayId]) {
    display::release_fillers_twinning_ours();
    for id in display::filler_ids() {
        if !display::is_tucked(id, virtual_ids) {
            display::tuck_away(id, virtual_ids);
        }
    }
}

/// Give a new display its desktop picture, whoever made it: the user with
/// "+" or an agent. Off the moment it is made, as macOS lists its screen a
/// little later and takes up to seconds the first time for a serial.
fn label_soon(id: DisplayId, cx: &mut Context<Core>) {
    cx.spawn(async move |this, cx| {
        for _ in 0..10 {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let Ok(Some(labelled)) = this.update(cx, |core, _| {
                core.displays.get(id)?;
                let mtm = objc2::MainThreadMarker::new()?;
                let labelling = std::time::Instant::now();
                let labelled = crate::mac::app::label_display(id, mtm);
                log::debug!(
                    "display {id}: desktop picture {labelled} in {} ms",
                    labelling.elapsed().as_millis()
                );
                Some(labelled)
            }) else {
                return;
            };
            if labelled {
                return;
            }
        }
    })
    .detach();
}
