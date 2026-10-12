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
    /// Files a running app is still opening, by pid, each watched once.
    pub watching: HashSet<(i32, std::path::PathBuf)>,
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
            watching: HashSet::new(),
        }
    }

    /// One input queue per display, so each display remembers its own
    /// focused window. The Viewer and the Agent Session of a display share it.
    pub fn forwarder(&mut self, id: DisplayId) -> Forwarder {
        self.forwarders.entry(id).or_default().clone()
    }

    /// Create a virtual display and park it out of the cursor's way.
    pub fn create(
        &mut self,
        size: (f64, f64),
        owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) -> Option<DisplayId> {
        let (mut serial, mut name) = self.displays.next_serial();
        // A test instance's displays must not pass for the user's.
        if let Some(instance) = sidescreen_mcp::endpoint::instance() {
            name = format!("Sidescreen {instance} {serial}");
            serial += 10_000;
        }
        let handle = VirtualDisplay::create(&name, size.0 as u32, size.1 as u32, serial)?;
        let id = handle.id;
        self.displays.add(id, name, size, owner, handle);
        // WindowServer needs a moment before the display accepts
        // configuration; try until it does.
        cx.spawn(async move |this, cx| {
            for _ in 0..30 {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let Ok(placed) = this.update(cx, |core, _| {
                    let virtual_ids: Vec<DisplayId> =
                        core.displays.list.iter().map(|d| d.id).collect();
                    display::tuck_away(id, &virtual_ids)
                }) else {
                    return;
                };
                if placed {
                    break;
                }
            }
            let _ = this.update(cx, |core, _| core.tucked.insert(id));
        })
        .detach();
        cx.notify();
        Some(id)
    }

    pub fn end(&mut self, id: DisplayId, reason: EndReason, cx: &mut Context<Self>) {
        if self.displays.end(id, reason) {
            // An ended display takes no more windows, from catches either.
            self.kept_catches.retain(|(display, _)| *display != id);
            cx.notify();
            schedule_sweep(cx);
        }
    }

    pub fn connection_closed(&mut self, connection: u64, cx: &mut Context<Self>) {
        self.closed.insert(connection);
        self.sessions.remove(&connection);
        if !self.displays.connection_closed(connection).is_empty() {
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
