//! The app's state on the main thread: Agent Displays with their platform
//! handles, input queues and the latest element trees.

use crate::mac::ax;
use crate::mac::display::{self, VirtualDisplay};
use crate::mac::input::Forwarder;
use crate::mac::windows;
use crate::model::{DisplayId, Displays, EndReason, Owner, WindowId};
use crate::numbers::Numbers;
use gpui::Context;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

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
}

impl Core {
    pub fn new() -> Self {
        Core {
            displays: Displays::default(),
            forwarders: HashMap::new(),
            elements: HashMap::new(),
            closed: HashSet::new(),
            sessions: HashMap::new(),
            tucked: HashSet::new(),
            launching: HashSet::new(),
            kept_catches: Vec::new(),
            returning: Vec::new(),
            pending_returns: Default::default(),
            watching: HashSet::new(),
            told: HashSet::new(),
            left_alone: Vec::new(),
        }
    }

    /// One input queue per display, so each display remembers its own
    /// focused window. The Viewer and the Agent Session of a display share it.
    pub fn forwarder(&mut self, id: DisplayId) -> Forwarder {
        self.forwarders.entry(id).or_default().clone()
    }

    /// The serial and name for the next display. A test instance's
    /// displays must not pass for the user's.
    pub fn next_name(&mut self) -> (u32, String) {
        let (serial, name) = self.displays.next_serial();
        match sidescreen_mcp::endpoint::instance() {
            Some(instance) => (serial + 10_000, format!("Sidescreen {instance} {serial}")),
            None => (serial, name),
        }
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
        self.displays.add(id, name, size, owner, handle);
        // Usually WindowServer takes the position at once. Here, before the
        // main thread turns to everything a new display sets off, which
        // held a spawned first try back by up to 400 ms.
        let virtual_ids: Vec<DisplayId> = self.displays.list.iter().map(|d| d.id).collect();
        if display::tuck_away(id, &virtual_ids) {
            self.tucked.insert(id);
            cx.notify();
            return id;
        }
        // Otherwise it needs a moment before it accepts configuration; try
        // until it does.
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
                let Ok(placed) = this.update(cx, |core, _| {
                    let virtual_ids: Vec<DisplayId> =
                        core.displays.list.iter().map(|d| d.id).collect();
                    display::tuck_away(id, &virtual_ids)
                }) else {
                    return;
                };
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

    pub fn end(&mut self, id: DisplayId, reason: EndReason, cx: &mut Context<Self>) {
        if self.displays.end(id, reason) {
            // An ended display takes no more windows, from catches either.
            self.kept_catches.retain(|(display, _)| *display != id);
            cx.notify();
            schedule_sweep(cx);
        }
    }

    /// The user took the display over, or handed it back: an agent's
    /// typing or drag in flight stops while they're in control.
    pub fn stop_agent_input(&self, id: DisplayId, stop: bool) {
        if let Some(forwarder) = self.forwarders.get(&id) {
            forwarder.stop_agent(stop);
        }
    }

    pub fn connection_closed(&mut self, connection: u64, cx: &mut Context<Self>) {
        self.closed.insert(connection);
        self.sessions.remove(&connection);
        let ended = self.displays.connection_closed(connection);
        if !ended.is_empty() {
            // As in `end`: an ended display takes no more windows.
            self.kept_catches
                .retain(|(display, _)| !ended.contains(display));
            cx.notify();
            schedule_sweep(cx);
        }
    }

    /// Drop a display: macOS removes it with its handle.
    pub fn remove(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        if self.displays.remove(id).is_some() {
            self.forwarders.remove(&id);
            self.elements.retain(|(display, _), _| *display != id);
            self.tucked.remove(&id);
            self.kept_catches.retain(|(display, _)| *display != id);
            self.returning.retain(|(display, _)| *display != id);
            self.pending_returns.retain(|(display, _)| *display != id);
            self.told.retain(|(display, _)| *display != id);
            self.left_alone.retain(|(display, _)| *display != id);
            cx.notify();
        }
    }

    /// Ended displays with no window left close by themselves.
    pub fn sweep_ended(&mut self, cx: &mut Context<Self>) {
        let empty: Vec<DisplayId> = self
            .displays
            .list
            .iter()
            .filter(|d| d.is_ended() && windows::on_display(d.id).is_empty())
            .map(|d| d.id)
            .collect();
        for id in empty {
            self.remove(id, cx);
        }
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
