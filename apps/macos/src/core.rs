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
use agent_screen_model::request::{Answer, Effect, Kind, RequestId, Unanswered};
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
const DISCONNECT_GRACE: Duration = Duration::from_millis(2000);

fn disconnect_grace() -> Duration {
    if cfg!(debug_assertions)
        && let Some(ms) = std::env::var("AGENTSCREEN_DISCONNECT_GRACE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    DISCONNECT_GRACE
}

/// Off-thread snapshot of online displays, window counts, and unit-twin status
/// for `take_spare_with` and `remove_with` (L3).
pub struct SpareSnapshot {
    pub online: Vec<DisplayId>,
    pub counts: HashMap<DisplayId, usize>,
    pub physically_occupied: HashSet<DisplayId>,
    pub untwinned: HashSet<DisplayId>,
}

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
    pub input_queue: crate::input_queue::InputQueue,
    forwarders: HashMap<DisplayId, Forwarder>,
    /// The numbered elements of each window an agent has looked at, by
    /// display. Dropped with the display.
    pub elements: HashMap<(DisplayId, WindowId), Numbers<ax::Element>>,
    /// Connections that have closed. A request from one can still be queued
    /// behind its close, and must not create a display nobody owns.
    pub closed: HashSet<u64>,
    closed_order: std::collections::VecDeque<u64>,
    /// The Agent Session each connection created or reused a display for.
    /// A connection may only use displays its session owns.
    pub sessions: HashMap<u64, String>,
    /// Displays moved to where they stay. Until then a new display can sit
    /// over one of the user's windows parked off screen.
    pub tucked: HashSet<DisplayId>,
    /// Last verified global bounds of each adopted display, so windows left
    /// behind at an old viewport follow when a display moves (#204).
    /// Serialized repark state and last verified global rect of each adopted
    /// display, so overlapping `ScreensChanged` notifications never shift
    /// unrecorded windows twice or overwrite newer baselines (H1).
    repark_state: agent_screen_model::arrangement::ReparkState,
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
    /// window is back. A Handover, a later Take Back or the display's
    /// removal meanwhile cancels it.
    pub pending_returns: agent_screen_model::returns::Returns,
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
    /// Monotonic origin for session activity and disconnect grace timestamps.
    started_at: Instant,
    /// In-flight calls whose `handle` future dropped mid-await (L8).
    pub dropped_calls: std::sync::Arc<std::sync::Mutex<Vec<(String, u64)>>>,
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
        let mut displays = Displays::default();
        displays.bind_recorded_owners(windows::recorded_owners_handle());
        Core {
            displays,
            input_queue: Default::default(),
            forwarders: HashMap::new(),
            elements: HashMap::new(),
            closed: HashSet::new(),
            closed_order: std::collections::VecDeque::new(),
            sessions: HashMap::new(),
            tucked: HashSet::new(),
            repark_state: Default::default(),
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
            started_at: Instant::now(),
            dropped_calls: Default::default(),
            last_used: None,
            warming: false,
            made: MadeLimit::new(MADE, MADE_WINDOW),
            reclaim: Reclaim::default(),
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn started_at_instant(&self) -> Instant {
        self.started_at
    }

    pub fn drain_dropped_calls(&mut self) {
        if let Ok(mut guard) = self.dropped_calls.lock() {
            for (key, at_ms) in guard.drain(..) {
                self.displays.sessions.note_call_end(&key, at_ms);
            }
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

    /// Ids of unused spare displays kept for reuse.
    pub fn spare_ids(&self) -> Vec<DisplayId> {
        self.spares.iter().map(|s| s.handle.id).collect()
    }

    fn online_unit_in(id: DisplayId, online: &[DisplayId]) -> Option<u32> {
        online
            .contains(&id)
            .then(|| crate::mac::cg::unit_number(id))
    }

    fn spare_infos_with(
        spares: &[(DisplayId, (f64, f64), bool)],
        online: &[DisplayId],
        counts: &HashMap<DisplayId, usize>,
        physically_occupied: &HashSet<DisplayId>,
    ) -> Vec<agent_screen_model::spares::SpareInfo> {
        spares
            .iter()
            .map(
                |&(id, size, expired)| agent_screen_model::spares::SpareInfo {
                    size,
                    unit: Self::online_unit_in(id, online),
                    expired,
                    has_windows: counts.get(&id).copied().unwrap_or(0) > 0,
                    physically_covered: physically_occupied.contains(&id),
                },
            )
            .collect()
    }

    /// Gather online displays, window counts, physical occupancy, and twin
    /// status off the UI thread for `take_spare_with` and `remove_with` (L3, L4).
    pub fn probe_spares(ids: &[DisplayId]) -> SpareSnapshot {
        let online = crate::mac::cg::active_displays();
        let (counts, physically_occupied) = windows::counts_and_physical(ids);
        let untwinned = ids
            .iter()
            .copied()
            .filter(|&id| online.contains(&id) && crate::mac::cg::unit_twin(id).is_none())
            .collect();
        SpareSnapshot {
            online,
            counts,
            physically_occupied,
            untwinned,
        }
    }

    /// A spare to use instead of a new display, with its name: one of
    /// `size` with the lowest unit number, or else the lowest-unit spare,
    /// which the caller resizes. Spares holding stray windows stay untouched
    /// so a new session never inherits another window; offline or twinned
    /// spares are released instead. Uses an off-thread `SpareSnapshot` (L3).
    pub fn take_spare_with(
        &mut self,
        size: (f64, f64),
        snap: &SpareSnapshot,
    ) -> Option<(VirtualDisplay, String, bool)> {
        loop {
            let raw: Vec<_> = self
                .spares
                .iter()
                .map(|s| (s.handle.id, s.size, false))
                .collect();
            let infos =
                Self::spare_infos_with(&raw, &snap.online, &snap.counts, &snap.physically_occupied);
            let index = agent_screen_model::spares::pick_spare(&infos, size)?;
            let spare = self.spares.remove(index);
            let id = spare.handle.id;
            if snap.untwinned.contains(&id) {
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
    /// were used lately and fewer than `READY` are ready. Window and display
    /// queries run off the UI thread.
    fn tidy(&mut self, cx: &mut Context<Self>) {
        if crate::mac::ime::user_idle() < idle().as_secs_f64() {
            return;
        }
        // A display change while an app starts makes Chrome lose the place
        // it was told to open its window at, and open it on the user's
        // screen (#123). The next tidy comes soon enough.
        if !self.launching.is_empty() {
            return;
        }
        let mut released = false;
        for display in std::mem::take(&mut self.retiring) {
            crate::lifecycle::note("released", display.id, "ended, released while idle");
            released = true;
        }
        let spare_ids = self.spare_ids();
        let live_ids: Vec<DisplayId> = self.displays.list.iter().map(|d| d.id).collect();
        cx.spawn(async move |this, cx| {
            let (online, counts, physical) = cx
                .background_executor()
                .spawn(async move {
                    evacuate_spare_windows(&spare_ids);
                    let online = crate::mac::cg::active_displays();
                    let (counts, physical) = windows::counts_and_physical(&spare_ids);
                    (online, counts, physical)
                })
                .await;
            let _ = this.update(cx, |core, cx| {
                if !core.launching.is_empty() {
                    if released {
                        core.reclaim_soon(cx);
                    }
                    return;
                }
                let warm = core.last_used.is_some_and(|t| t.elapsed() < WARM);
                let raw: Vec<_> = core
                    .spares
                    .iter()
                    .map(|s| (s.handle.id, s.size, s.since.elapsed() >= SPARE_LIFE))
                    .collect();
                let infos = Self::spare_infos_with(&raw, &online, &counts, &physical);
                let live_units: Vec<u32> = live_ids
                    .iter()
                    .filter_map(|&id| Self::online_unit_in(id, &online))
                    .collect();
                let keep = agent_screen_model::spares::keep_spares(
                    &infos,
                    warm,
                    READY,
                    SPARES,
                    &live_units,
                );
                let mut kept = Vec::new();
                for (spare, stay) in std::mem::take(&mut core.spares).into_iter().zip(keep) {
                    if stay {
                        kept.push(spare);
                    } else {
                        crate::lifecycle::note("released", spare.handle.id, "spare unused");
                        released = true;
                    }
                }
                core.spares = kept;
                if !warm && core.displays.list.is_empty() && core.spares.is_empty() {
                    released |= !display::filler_ids().is_empty();
                    display::release_fillers();
                }
                if released {
                    core.reclaim_soon(cx);
                }
                // One at a time, a tidy apart.
                if warm
                    && core.spares.len() < READY
                    && !core.warming
                    && display::creation_refusal().is_none()
                    && core.budget_allows().is_ok()
                {
                    core.make_spare(cx);
                }
            });
        })
        .detach();
    }

    /// Whether a spare display is being made, which changes the displays.
    pub fn making_spare(&self) -> bool {
        self.warming
    }

    /// Make a spare of the size "+" makes, parked like any display.
    fn make_spare(&mut self, cx: &mut Context<Self>) {
        self.warming = true;
        let size = crate::model::DEFAULT_SIZE;
        let (serial, name) = self.next_name((size.0 as u32, size.1 as u32));
        let others = self.virtual_ids();
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
                .await
                .ok();
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

    /// The serial and name for the next display, of `size` in points. A
    /// test instance's displays must not pass for the user's.
    pub fn next_name(&mut self, size: (u32, u32)) -> (display::Serial, String) {
        let instance = agent_screen_mcp::endpoint::instance();
        let first = if instance.is_some() { 10_001 } else { 1 };
        let serial = display::Serial::lease(first, size);
        let number = serial.get() + 1 - first;
        let name = match instance {
            Some(instance) => format!("Agent Screen {instance} {number}"),
            None => format!("Agent Screen {number}"),
        };
        (serial, name)
    }

    /// Take on a display only after its position is verified. New displays
    /// arrive parked; a reused or resized spare must still be safe.
    pub fn adopt(
        &mut self,
        handle: VirtualDisplay,
        name: String,
        size: (f64, f64),
        owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) -> Option<DisplayId> {
        let id = handle.id;
        let mut virtual_ids = self.virtual_ids();
        virtual_ids.push(id);
        if !display::is_tucked(id, &virtual_ids) {
            log::warn!("display {id}: refusing to adopt an unverified position");
            crate::lifecycle::note("released", id, "unsafe at adoption");
            return None;
        }
        if let Some((serial, slot)) = handle.serial() {
            colorsync::remember(serial, slot, id);
        }
        if let Some(owner) = &owner {
            self.input_queue.register(id, owner);
        }
        self.displays.add(id, name, size, owner, handle);
        if let Some(display) = self.displays.get(id) {
            let _ = crate::web::register_display(id, display.label(), size);
        }
        self.last_used = Some(Instant::now());
        // Its record is new and has all 39 names.
        self.reclaim_soon(cx);
        label_soon(id, cx);
        self.tucked.insert(id);
        self.repark_state
            .note_adopted(id, crate::mac::cg::display_bounds(id));
        cx.notify();
        Some(id)
    }

    /// Rebind any disconnected displays of `key` to open `connection` and
    /// register their new `InputQueue` owners (`H1`, `M1`, `L3`).
    pub(crate) fn rebind_session_in<H>(
        displays: &mut Displays<H>,
        input_queue: &mut crate::input_queue::InputQueue,
        closed: &HashSet<u64>,
        key: &str,
        connection: u64,
        now_ms: u64,
    ) -> Vec<DisplayId> {
        if closed.contains(&connection) {
            return Vec::new();
        }
        let rebound = displays.rebind_disconnected(key, connection, now_ms);
        displays.register_queue_owners(&mut input_queue.queue, &rebound);
        rebound
    }

    /// Track an incoming non-debug request on `connection`, rebinding any
    /// disconnect-grace displays when `named_key` is given while rejecting
    /// closed connections (`M1`, `L3`). Returns `(resolved_key, rebound_any)`.
    pub fn begin_session_call(
        &mut self,
        connection: u64,
        named_key: Option<&str>,
        shown: Option<DisplayId>,
        now_ms: u64,
    ) -> (Option<String>, bool) {
        Self::begin_session_call_in(
            &mut self.displays,
            &mut self.input_queue,
            &self.closed,
            &mut self.sessions,
            connection,
            named_key,
            shown,
            now_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_session_call_in<H>(
        displays: &mut Displays<H>,
        input_queue: &mut crate::input_queue::InputQueue,
        closed: &HashSet<u64>,
        sessions: &mut HashMap<u64, String>,
        connection: u64,
        named_key: Option<&str>,
        shown: Option<DisplayId>,
        now_ms: u64,
    ) -> (Option<String>, bool) {
        if closed.contains(&connection) {
            return (None, false);
        }
        let mut rebound = false;
        if let Some(k) = named_key {
            sessions.insert(connection, k.to_string());
            rebound =
                !Self::rebind_session_in(displays, input_queue, closed, k, connection, now_ms)
                    .is_empty();
        }
        let key = named_key
            .map(String::from)
            .or_else(|| shown.and_then(|id| displays.get(id)?.session().map(String::from)))
            .or_else(|| sessions.get(&connection).cloned());
        if let Some(ref k) = key {
            displays.sessions.note_call_start(k, now_ms);
        }
        (key, rebound)
    }

    /// Look up and rebind `key`'s latest live display for `createDisplay`
    /// reuse on open `connection`, rejecting closed connections (`M1`, `L3`).
    pub fn reuse_session_display(
        &mut self,
        connection: u64,
        key: &str,
        now_ms: u64,
    ) -> Option<DisplayId> {
        Self::reuse_session_display_in(
            &mut self.displays,
            &mut self.input_queue,
            &self.closed,
            connection,
            key,
            now_ms,
        )
    }

    pub(crate) fn reuse_session_display_in<H>(
        displays: &mut Displays<H>,
        input_queue: &mut crate::input_queue::InputQueue,
        closed: &HashSet<u64>,
        connection: u64,
        key: &str,
        now_ms: u64,
    ) -> Option<DisplayId> {
        if closed.contains(&connection) {
            return None;
        }
        let id = displays.for_session(key)?.id;
        Self::rebind_session_in(displays, input_queue, closed, key, connection, now_ms);
        Some(id)
    }

    /// The displays changed. Park any display of ours that left its verified
    /// arrangement immediately without waiting for in-flight Accessibility
    /// window writes (`M3`), then run a serialized window-shift pass (`H1`, `L2`).
    pub fn repark(&mut self, cx: &mut Context<Self>) {
        let virtual_ids = self.virtual_ids();
        let mut candidates = Vec::new();
        for d in &self.displays.list {
            if self.tucked.contains(&d.id) {
                let tries = self.reparks.get(&d.id).copied().unwrap_or(0);
                candidates.push((d.id, tries));
            }
        }
        cx.spawn(async move |this, cx| {
            let (tucked_ok, retried) = cx
                .background_executor()
                .spawn(async move {
                    display::configure(move || {
                        park_fillers(&virtual_ids);
                        let mut connected = display::arrangement_connected(&virtual_ids);
                        let mut tucked_ok = Vec::new();
                        let mut retried = Vec::new();
                        for &(id, tries) in &candidates {
                            if display::is_tucked(id, &virtual_ids) && connected {
                                tucked_ok.push(id);
                            } else if tries < 3 {
                                let parked = display::tuck_away(id, &virtual_ids);
                                connected = display::arrangement_connected(&virtual_ids);
                                log::debug!("display {id} was moved; parked again: {parked}");
                                if parked && connected {
                                    tucked_ok.push(id);
                                } else {
                                    retried.push(id);
                                }
                            }
                        }
                        (tucked_ok, retried)
                    })
                    .await
                })
                .await;
            let _ = this.update(cx, |core, cx| {
                for id in tucked_ok {
                    core.reparks.remove(&id);
                }
                for id in retried {
                    *core.reparks.entry(id).or_default() += 1;
                }
                core.run_repark_shifts(cx);
            });
        })
        .detach();
    }

    /// Run one serialized pass of window-shift planning and bounded AX writes
    /// (`M3`, `L2`). Overlapping requests coalesce into `rerun`, slow/hung PIDs
    /// skip after their first AX timeout, total write time per pass is capped,
    /// and failed moves retry on subsequent passes.
    fn run_repark_shifts(&mut self, cx: &mut Context<Self>) {
        let adopted: Vec<DisplayId> = self.displays.list.iter().map(|d| d.id).collect();
        let has_retries = self.repark_state.has_pending_retries(&adopted);
        let Some((generation, recorded)) = self.repark_state.begin(&adopted) else {
            return;
        };
        let virtual_ids = self.virtual_ids();
        cx.spawn(async move |this, cx| {
            let (settled_rects, stationary, raw_windows) = cx
                .background_executor()
                .spawn(async move {
                    let (settled_rects, stationary, any_moved) = display::configure(move || {
                        let online = crate::mac::cg::active_displays();
                        let mut settled_rects = Vec::new();
                        let mut any_moved = false;
                        for &(id, old) in &recorded {
                            if display::is_tucked(id, &virtual_ids) {
                                let now = crate::mac::cg::display_bounds(id);
                                if now.width > 0.0
                                    && ((now.x - old.x).abs() > 0.5 || (now.y - old.y).abs() > 0.5)
                                {
                                    any_moved = true;
                                }
                                settled_rects.push((id, now));
                            }
                        }
                        let stationary: Vec<crate::model::Rect> = online
                            .into_iter()
                            .filter(|d| !recorded.iter().any(|(rid, _)| rid == d))
                            .map(crate::mac::cg::display_bounds)
                            .collect();
                        (settled_rects, stationary, any_moved)
                    })
                    .await;
                    let raw_windows = if any_moved || has_retries {
                        windows::all()
                    } else {
                        Vec::new()
                    };
                    (settled_rects, stationary, raw_windows)
                })
                .await;
            let Ok(shifts) = this.update(cx, |core, _| {
                let live_adopted: Vec<DisplayId> =
                    core.displays.list.iter().map(|d| d.id).collect();
                let owners = core.displays.recorded_owners();
                let annotated: Vec<_> = raw_windows
                    .iter()
                    .map(|w| {
                        (
                            w.target,
                            w.bounds,
                            owners.get(&w.target.window_id).copied(),
                        )
                    })
                    .collect();
                let (shifts, active_moves) = core.repark_state.plan_shifts(
                    generation,
                    &live_adopted,
                    &settled_rects,
                    &stationary,
                    &annotated,
                );
                for (id, old, now) in active_moves {
                    let on_old = raw_windows
                        .iter()
                        .filter(|w| old.contains(w.bounds.mid()))
                        .count();
                    let on_now = raw_windows
                        .iter()
                        .filter(|w| now.contains(w.bounds.mid()))
                        .count();
                    log::info!(
                        "display {id}: parked origin moved ({:.0},{:.0}) -> ({:.0},{:.0}); windows centered on old={on_old}, new={on_now}, planned shifts={}",
                        old.x,
                        old.y,
                        now.x,
                        now.y,
                        shifts.len()
                    );
                }
                shifts
            }) else {
                return;
            };
            let failed = if shifts.is_empty() {
                Vec::new()
            } else {
                cx.background_executor()
                    .spawn(async move {
                        let start = Instant::now();
                        execute_repark_writes(
                            &shifts,
                            Duration::from_millis(1000),
                            || start.elapsed(),
                            |target, next| ax::set_frame_repark(target, next, 0.25),
                        )
                    })
                    .await
            };
            let _ = this.update(cx, |core, cx| {
                let (rerun, first_failures) =
                    core.repark_state.finish_with_failures(generation, &failed);
                for target in first_failures {
                    log::warn!(
                        "repark: window {} (pid {}) failed to shift to its moved display; queued for retry",
                        target.window_id,
                        target.pid
                    );
                }
                let live_adopted: Vec<DisplayId> =
                    core.displays.list.iter().map(|d| d.id).collect();
                if rerun {
                    core.run_repark_shifts(cx);
                } else if core.repark_state.has_pending_retries(&live_adopted) {
                    let max_f = core.repark_state.max_pending_failures(&live_adopted);
                    if max_f < agent_screen_model::arrangement::MAX_PENDING_SHIFT_FAILURES {
                        let secs = 2u64.saturating_pow(max_f.saturating_sub(1).min(4)).min(30);
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(Duration::from_secs(secs))
                                .await;
                            let _ = this.update(cx, |core, cx| core.run_repark_shifts(cx));
                        })
                        .detach();
                    }
                }
            });
        })
        .detach();
    }

    /// Give each display and spare its size back if it runs at another (issue
    /// #33). WindowServer can switch a virtual display to another of the modes
    /// macOS lists for it, or recall an old mode stored for its serial, such
    /// as 800×500; screenshots and windows on it then no longer fit.
    pub fn hold_modes(&mut self, cx: &mut Context<Self>) {
        let mut wrong = Vec::new();
        let wanted = self
            .displays
            .list
            .iter()
            .map(|d| (d.id, d.width as u32, d.height as u32))
            .chain(
                self.spares
                    .iter()
                    .map(|s| (s.handle.id, s.size.0 as u32, s.size.1 as u32)),
            );
        for (id, width, height) in wanted {
            let now = crate::mac::cg::display_bounds(id);
            if (now.width as u32, now.height as u32) == (width, height) {
                self.remodes.remove(&id);
                continue;
            }
            let tries = self.remodes.entry(id).or_default();
            if *tries >= 3 {
                continue;
            }
            *tries += 1;
            wrong.push((id, width, height));
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

    /// The Agent Session stops using display `id`: by end_display, or the
    /// user closing it. Its other displays go on.
    pub fn end(&mut self, id: DisplayId, reason: EndReason, cx: &mut Context<Self>) {
        if self.displays.end(id, reason) {
            self.finished(&[id], cx);
        }
    }

    /// What the platform does once for each display the model just ended,
    /// whichever way it ended: end_display, the user closing it, or the
    /// session's connection closing, which ends all of its displays
    /// (ADR 0004, ADR 0016).
    fn finished(&mut self, ended: &[DisplayId], cx: &mut Context<Self>) {
        if ended.is_empty() {
            return;
        }
        for &id in ended {
            #[cfg(debug_assertions)]
            crate::mac::cg::clear_user_stage(id);
            crate::web::expire_display(id);
            self.input_queue.end_display(id);
            if let Some(forwarder) = self.forwarders.get(&id) {
                forwarder.stop_agent(true);
            }
            crate::control::agent::release_hold(id);
            // An ended display takes no more windows, from catches either.
            // An app it handed to a live display of the session is caught
            // there instead.
            let mut handed: std::collections::BTreeMap<DisplayId, Vec<i32>> = Default::default();
            for (display, catch) in &self.kept_catches {
                if *display == id
                    && let Some(to) = self.displays.starter_of(catch.pid())
                {
                    handed.entry(to).or_default().push(catch.pid());
                }
            }
            self.kept_catches.retain(|(display, _)| *display != id);
            for (to, pids) in handed {
                crate::control::agent::catch_again(cx.entity(), to, pids, cx);
            }
            stop_recording(id, cx);
            self.return_handovers(id, cx);
        }
        cx.notify();
        schedule_sweep(cx);
    }

    /// The user's windows go home as the agent stops using display `id`
    /// (#161), one at a time, as window_access return sends them, each
    /// given back right before its move. Only a window still on the display
    /// moves: one that already left, as one its app put back on the user's
    /// screen, stays where the user has it. A window that can't leave stays
    /// a Handover there, for the dismissal to return. The display closes
    /// once none is left.
    fn return_handovers(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        let Some(shell) = crate::ui::shell::current() else {
            return;
        };
        let targets = self.displays.handovers_at_end(id);
        if targets.is_empty() {
            return;
        }
        // After this update: sending a window home updates Core too.
        cx.spawn(async move |this, cx| {
            for target in targets {
                let window = target.window_id;
                // Only one still a Handover: the user may have taken it
                // back while an earlier window moved, and a second give-back
                // would race their move.
                let Ok(given) = this.update(cx, |core, _| {
                    core.displays
                        .get(id)
                        .is_some_and(|d| d.handovers().contains(&target))
                        .then(|| core.displays.give_back(id, target))
                        .flatten()
                }) else {
                    return;
                };
                if given.is_none() {
                    continue;
                }
                let Ok(task) =
                    shell.update(cx, |shell, cx| shell.send_home_if_there(target, id, given, cx))
                else {
                    return;
                };
                if task.await {
                    log::info!("display {id}: window {window} went back to the user as the display ended");
                } else {
                    log::warn!("display {id}: window {window} didn't leave the ended display; it stays a Handover there");
                }
            }
            let _ = this.update(cx, |core, cx| core.sweep_ended(cx));
        })
        .detach();
    }

    /// The user takes the display over: by their first input in the
    /// Viewer, Take Over, or by telling the agent in chat. The agent's
    /// input stops and Window Access pauses. False when nothing changed.
    pub fn take_over(&mut self, id: DisplayId, cx: &mut Context<Self>) -> bool {
        let effects = self.displays.begin_takeover(id);
        let changed = !effects.is_empty();
        self.apply(effects, cx);
        changed
    }

    /// The user hands the display back: by Let … Continue in the Viewer or
    /// on the card, or by telling the agent in chat. One action for all of
    /// them. False when the user wasn't in control.
    pub fn hand_back(&mut self, id: DisplayId, cx: &mut Context<Self>) -> bool {
        let effects = self.displays.end_takeover(id);
        let changed = !effects.is_empty();
        self.apply(effects, cx);
        changed
    }

    /// The user answers display `id`'s request of `kind`, from the card, a
    /// notification or the conversation. The model decides what ends; this
    /// applies what it needs from the platform.
    pub fn answer(
        &mut self,
        id: DisplayId,
        kind: Kind,
        request: Option<RequestId>,
        answer: Answer,
        cx: &mut Context<Self>,
    ) -> Result<(), Unanswered> {
        let what = match &answer {
            Answer::Continue => "continue",
            Answer::Reply(_) => "reply",
            Answer::Give => "give",
            Answer::Keep => "keep",
        };
        let effects = self.displays.answer(id, kind, request, answer)?;
        log::info!(
            "display {id}: the user answered its {} request: {what}",
            kind.as_str()
        );
        self.apply(effects, cx);
        cx.notify();
        Ok(())
    }

    /// What a request or Takeover transition needs from the platform, once.
    fn apply(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Takeover { display, on } => {
                    self.input_queue.takeover(display, on);
                    self.stop_agent_input(display, on);
                }
                // The Shell moves windows; it may be the caller, so after
                // this update.
                Effect::HandOver { display, target } => {
                    if let Some(shell) = crate::ui::shell::current() {
                        cx.defer(move |cx| {
                            let _ = shell.update(cx, |shell, cx| {
                                shell.hand_over(target, None, display, false, cx)
                            });
                        });
                    }
                }
            }
            cx.notify();
        }
    }

    /// The user took the display over, or handed it back: an agent's
    /// typing or drag in flight stops while they're in control.
    pub fn stop_agent_input(&self, id: DisplayId, stop: bool) {
        // Before anything renders or a card takes the keyboard: the
        // Viewer's composition ends with the Takeover.
        crate::mac::viewer_input::takeover_changed(id, stop);
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
        self.drain_dropped_calls();
        #[cfg(debug_assertions)]
        crate::mac::cg::clear_user_stage_connection(connection);
        let grace = disconnect_grace();
        let now_ms = self.elapsed_ms();
        let marked = Self::disconnect_connection_in(
            &mut self.displays,
            &mut self.input_queue,
            &mut self.closed,
            &mut self.closed_order,
            &mut self.sessions,
            connection,
            now_ms,
        );
        if grace.is_zero() {
            let ended = self.displays.expire_disconnected(now_ms, 0);
            self.finished(&ended, cx);
            return;
        }
        if marked > 0 {
            cx.notify();
            let grace_ms = grace.as_millis() as u64;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(grace).await;
                let _ = this.update(cx, |core, cx| {
                    let ended = core
                        .displays
                        .expire_disconnected(core.elapsed_ms(), grace_ms);
                    core.finished(&ended, cx);
                });
            })
            .detach();
        }
    }

    pub(crate) fn disconnect_connection_in<H>(
        displays: &mut Displays<H>,
        input_queue: &mut crate::input_queue::InputQueue,
        closed: &mut HashSet<u64>,
        closed_order: &mut std::collections::VecDeque<u64>,
        sessions: &mut HashMap<u64, String>,
        connection: u64,
        now_ms: u64,
    ) -> usize {
        if closed.insert(connection) {
            if closed_order.len() >= agent_screen_model::session::MAX_CLOSED_CONNECTIONS
                && let Some(oldest) = closed_order.pop_front()
            {
                closed.remove(&oldest);
            }
            closed_order.push_back(connection);
        }
        // First, so the queue's requests stop as disconnected, not as
        // their displays ending.
        input_queue.disconnect(connection);
        sessions.remove(&connection);
        let (marked, migrated) = displays.connection_disconnected_with_migrated(connection, now_ms);
        displays.register_queue_owners(&mut input_queue.queue, &migrated);
        marked
    }

    /// Drop a display using an off-thread `SpareSnapshot` (L3). An empty one
    /// stays connected as a spare while there is room; otherwise macOS removes
    /// it with its handle.
    pub fn remove_with(&mut self, id: DisplayId, snap: &SpareSnapshot, cx: &mut Context<Self>) {
        crate::web::expire_display(id);
        self.input_queue.end_display(id);
        #[cfg(debug_assertions)]
        crate::mac::cg::clear_user_stage(id);
        stop_recording(id, cx);
        if let Some(display) = self.displays.remove(id) {
            let size = (display.width, display.height);
            let is_empty = snap.counts.get(&id).copied().unwrap_or(0) == 0;
            let empty_untwinned = is_empty && snap.untwinned.contains(&id);
            if empty_untwinned && self.spares.len() < SPARES {
                crate::lifecycle::note("kept", id, "spare");
                self.spares.push(Spare {
                    handle: display.handle,
                    name: display.name,
                    size,
                    since: Instant::now(),
                });
            } else if empty_untwinned
                && let Some(evict) = {
                    let raw: Vec<_> = self
                        .spares
                        .iter()
                        .map(|s| (s.handle.id, s.size, s.since.elapsed() >= SPARE_LIFE))
                        .collect();
                    let infos = Self::spare_infos_with(
                        &raw,
                        &snap.online,
                        &snap.counts,
                        &snap.physically_occupied,
                    );
                    agent_screen_model::spares::retire_instead(
                        &infos,
                        SPARES,
                        Self::online_unit_in(id, &snap.online),
                    )
                }
            {
                let higher = self.spares.remove(evict);
                crate::lifecycle::note("kept", higher.handle.id, "retiring");
                self.retiring.push(higher.handle);
                crate::lifecycle::note("kept", id, "spare");
                self.spares.push(Spare {
                    handle: display.handle,
                    name: display.name,
                    size,
                    since: Instant::now(),
                });
            } else if is_empty {
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
            self.repark_state.remove_display(id);
            self.reparks.remove(&id);
            self.remodes.remove(&id);
            self.kept_catches.retain(|(display, _)| *display != id);
            self.returning.retain(|(display, _)| *display != id);
            self.pending_returns.remove_display(id);
            self.told.retain(|(display, _)| *display != id);
            self.left_alone.retain(|(display, _)| *display != id);
            // The agents' browser instance goes with its last window, so it
            // never takes the user's Dock click or links (ADR 0022).
            cx.background_executor()
                .spawn(async { crate::mac::devtools::end_agents(false) })
                .detach();
            cx.notify();
        }
    }

    /// Ended displays with no window left close by themselves.
    /// Window and display probes run off the UI thread (L3).
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
        let mut probe_ids = ended.clone();
        for id in self.spare_ids() {
            if !probe_ids.contains(&id) {
                probe_ids.push(id);
            }
        }
        cx.spawn(async move |this, cx| {
            let snap = cx
                .background_executor()
                .spawn(async move { Self::probe_spares(&probe_ids) })
                .await;
            let _ = this.update(cx, |core, cx| {
                for id in ended {
                    // Still ended: the user may have taken it back meanwhile.
                    if snap.counts.get(&id).copied().unwrap_or(0) == 0
                        && core.displays.get(id).is_some_and(|d| d.is_ended())
                    {
                        core.remove_with(id, &snap, cx);
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

/// Return an unowned window that strayed onto a spare or newly created virtual
/// display back to the user's screen (or the user stand-in stage in debug e2e
/// runs). Returns `true` when the window moved. Blocking.
pub(crate) fn evacuate_window_to_user(
    target: crate::model::WindowTarget,
    app: &str,
    from: DisplayId,
    what: &str,
) -> bool {
    #[cfg(debug_assertions)]
    if let Some(stage) = crate::mac::cg::user_stage() {
        if crate::mac::claims::claim_confirmed(target.window_id, stage)
            && ax::move_into(target, crate::mac::cg::display_bounds(stage)).is_ok()
        {
            log::info!(
                "moved stray window {} ({app}) off {what} {from} to user stage {stage}",
                target.window_id
            );
            return true;
        }
        return false;
    }
    if crate::dismissal::give_back(target, None, None) {
        log::info!(
            "moved stray window {} ({app}) off {what} {from} to the user's screen",
            target.window_id
        );
        return true;
    }
    false
}

fn evacuate_spare_windows(spares: &[DisplayId]) {
    static BACKOFF: std::sync::LazyLock<std::sync::Mutex<HashMap<u32, (u32, Instant)>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
    let now = Instant::now();
    let mut present = HashSet::new();
    for &spare in spares {
        for window in windows::on_display(spare) {
            let wid = window.target.window_id;
            present.insert(wid);
            let skip = BACKOFF
                .lock()
                .ok()
                .and_then(|g| g.get(&wid).copied())
                .is_some_and(|(_, retry_at)| retry_at > now);
            if skip {
                continue;
            }
            if evacuate_window_to_user(window.target, &window.app, spare, "spare") {
                if let Ok(mut g) = BACKOFF.lock() {
                    g.remove(&wid);
                }
            } else if let Ok(mut g) = BACKOFF.lock() {
                let failures = g.get(&wid).map_or(0, |(f, _)| *f).saturating_add(1);
                let secs = 2u64.saturating_pow(failures.min(5)).min(60);
                g.insert(wid, (failures, now + Duration::from_secs(secs)));
                if failures == 1 {
                    log::warn!(
                        "spare {spare}: failed to return stray window {wid} ({}) to the user's screen; backing off {secs}s",
                        window.app
                    );
                }
            }
        }
    }
    if let Ok(mut g) = BACKOFF.lock() {
        g.retain(|wid, _| present.contains(wid));
    }
}

pub fn watch_alerts(core: Entity<Core>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let (ids, spares) = core.read_with(cx, |c, _| (c.virtual_ids(), c.spare_ids()));
            if ids.is_empty() {
                continue;
            }
            let moved = cx
                .background_executor()
                .spawn(async move {
                    evacuate_spare_windows(&spares);
                    crate::mac::alerts::send_home(&ids)
                })
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

/// Execute planned repark window shifts under a total pass time budget (`M3`,
/// `L2`). Stops sending further AX writes to a `pid` in this pass after its
/// first `ReparkMoveError::TimedOut`, and returns every unshifted target so
/// `ReparkState` retries it on a later pass.
fn execute_repark_writes(
    shifts: &[(crate::model::WindowTarget, crate::model::Rect)],
    budget: Duration,
    mut elapsed: impl FnMut() -> Duration,
    mut write_one: impl FnMut(
        crate::model::WindowTarget,
        crate::model::Rect,
    ) -> Result<(), ax::ReparkMoveError>,
) -> Vec<crate::model::WindowTarget> {
    let mut timed_out_pids = HashSet::new();
    let mut failed = Vec::new();
    for (idx, &(target, next)) in shifts.iter().enumerate() {
        if elapsed() >= budget {
            failed.extend(shifts[idx..].iter().map(|&(t, _)| t));
            break;
        }
        if timed_out_pids.contains(&target.pid) {
            failed.push(target);
            continue;
        }
        match write_one(target, next) {
            Ok(()) => {}
            Err(ax::ReparkMoveError::TimedOut) => {
                timed_out_pids.insert(target.pid);
                failed.push(target);
            }
            Err(ax::ReparkMoveError::Failed) => {
                failed.push(target);
            }
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Rect, WindowTarget};

    #[test]
    fn repark_writes_skip_timed_out_pid_and_respect_pass_budget() {
        let shifts = vec![
            (
                WindowTarget {
                    pid: 10,
                    window_id: 1,
                },
                Rect::new(0.0, 0.0, 100.0, 100.0),
            ),
            (
                WindowTarget {
                    pid: 10,
                    window_id: 2,
                },
                Rect::new(0.0, 0.0, 100.0, 100.0),
            ),
            (
                WindowTarget {
                    pid: 20,
                    window_id: 3,
                },
                Rect::new(0.0, 0.0, 100.0, 100.0),
            ),
            (
                WindowTarget {
                    pid: 30,
                    window_id: 4,
                },
                Rect::new(0.0, 0.0, 100.0, 100.0),
            ),
        ];
        let now_ms = std::cell::Cell::new(0u64);
        let mut called = Vec::new();
        let failed = execute_repark_writes(
            &shifts,
            Duration::from_millis(500),
            || Duration::from_millis(now_ms.get()),
            |target, _| {
                called.push(target.window_id);
                match target.window_id {
                    1 => {
                        now_ms.set(now_ms.get() + 250);
                        Err(ax::ReparkMoveError::TimedOut)
                    }
                    3 => {
                        now_ms.set(now_ms.get() + 300);
                        Ok(())
                    }
                    _ => Ok(()),
                }
            },
        );
        // Window 2 of hung pid 10 was skipped without calling AX; window 3 of pid 20
        // ran and succeeded; window 4 hit the 500ms budget cap and was deferred!
        assert_eq!(called, vec![1, 3]);
        assert_eq!(
            failed.into_iter().map(|t| t.window_id).collect::<Vec<_>>(),
            vec![1, 2, 4]
        );
    }

    #[test]
    fn spare_infos_mark_physically_occupied_spare_without_releasing_it_on_tidy() {
        let raw = vec![(9, (1280.0, 800.0), false), (10, (1280.0, 800.0), false)];
        let counts = HashMap::from([(9, 0usize), (10, 0usize)]);
        let mut physical = HashSet::new();
        physical.insert(9);
        let infos = Core::spare_infos_with(&raw, &[], &counts, &physical);
        assert!(!infos[0].has_windows);
        assert!(infos[0].physically_covered);
        assert!(!infos[1].physically_covered);
        // `pick_spare` skips covered spare 9 and hands out clean spare 10 (index 1).
        assert_eq!(
            agent_screen_model::spares::pick_spare(&infos, (1280.0, 800.0)),
            Some(1)
        );
        // `keep_spares` keeps both spares so idle `tidy` causes zero churn (M1).
        assert_eq!(
            agent_screen_model::spares::keep_spares(&infos, true, 2, 3, &[]),
            vec![true, true]
        );
    }

    #[test]
    fn core_session_wiring_reregisters_input_queue_owners_and_rejects_closed_connections() {
        use crate::model::Owner;

        let mut displays = Displays::<()>::default();
        let mut input_queue = crate::input_queue::InputQueue::default();
        let mut closed = HashSet::new();
        let mut closed_order = std::collections::VecDeque::new();
        let mut sessions = HashMap::new();

        // Session "s1" creates display 7 on connection 1 and registers in InputQueue.
        let owner1 = Owner {
            session_key: "s1".into(),
            client: "Claude".into(),
            title: "Work".into(),
            connection: 1,
        };
        displays.add(
            7,
            "Agent Screen 1".into(),
            (1280.0, 800.0),
            Some(owner1.clone()),
            (),
        );
        input_queue.register(7, &owner1);
        sessions.insert(1, "s1".into());

        // 1. Overlapping connection 2 joins "s1"; closing connection 1 migrates display 7
        // AND re-registers InputQueue ownership onto connection 2 (`L3` / `RV6`).
        let (started_on_2, _) = Core::begin_session_call_in(
            &mut displays,
            &mut input_queue,
            &closed,
            &mut sessions,
            2,
            Some("s1"),
            None,
            100,
        );
        assert_eq!(started_on_2.as_deref(), Some("s1"));
        assert_eq!(
            Core::reuse_session_display_in(&mut displays, &mut input_queue, &closed, 2, "s1", 100),
            Some(7)
        );
        let marked = Core::disconnect_connection_in(
            &mut displays,
            &mut input_queue,
            &mut closed,
            &mut closed_order,
            &mut sessions,
            1,
            200,
        );
        assert_eq!(marked, 0);
        assert!(input_queue.owner(7, 2).is_ok());
        assert!(input_queue.owner(7, 1).is_err());

        // Closed connection 1 is rejected by begin_session_call_in, reuse_session_display_in,
        // and rebind_session_in (`L3` / `RV6`).
        assert_eq!(
            Core::begin_session_call_in(
                &mut displays,
                &mut input_queue,
                &closed,
                &mut sessions,
                1,
                Some("s1"),
                Some(7),
                210,
            ),
            (None, false)
        );
        assert_eq!(
            Core::reuse_session_display_in(&mut displays, &mut input_queue, &closed, 1, "s1", 210),
            None
        );

        // 2. Closing connection 2 puts display 7 into disconnect grace; a late call on
        // closed connection 2 cannot rebind it, while open connection 3 rebinds display 7
        // AND re-registers InputQueue ownership onto connection 3 (`L3` / `RV6`).
        let marked_2 = Core::disconnect_connection_in(
            &mut displays,
            &mut input_queue,
            &mut closed,
            &mut closed_order,
            &mut sessions,
            2,
            300,
        );
        assert_eq!(marked_2, 1);
        assert!(
            Core::rebind_session_in(&mut displays, &mut input_queue, &closed, "s1", 2, 350)
                .is_empty()
        );
        let (started_on_3, rebound_3) = Core::begin_session_call_in(
            &mut displays,
            &mut input_queue,
            &closed,
            &mut sessions,
            3,
            Some("s1"),
            Some(7),
            400,
        );
        assert_eq!(started_on_3.as_deref(), Some("s1"));
        assert!(rebound_3);
        assert!(input_queue.owner(7, 3).is_ok());
    }
}
