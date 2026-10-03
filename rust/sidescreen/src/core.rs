//! The app's state on the main thread: Agent Displays with their platform
//! handles, input queues and the latest element trees.

use crate::mac::ax;
use crate::mac::display::{self, VirtualDisplay};
use crate::mac::input::Forwarder;
use crate::mac::windows;
use crate::model::{DisplayId, Displays, EndReason, Owner, WindowId};
use gpui::Context;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub struct Core {
    pub displays: Displays<VirtualDisplay>,
    forwarders: HashMap<DisplayId, Forwarder>,
    /// Elements from the latest snapshot of each window, by their number.
    /// The last accessibility snapshot of each window, by display, so
    /// element numbers refer to it. Dropped with the display.
    pub elements: HashMap<(DisplayId, WindowId), Vec<ax::Element>>,
    /// Connections that have closed. A request from one can still be queued
    /// behind its close, and must not create a display nobody owns.
    pub closed: HashSet<u64>,
}

impl Core {
    pub fn new() -> Self {
        Core {
            displays: Displays::default(),
            forwarders: HashMap::new(),
            elements: HashMap::new(),
            closed: HashSet::new(),
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
        let (serial, name) = self.displays.next_serial();
        let handle = VirtualDisplay::create(&name, size.0 as u32, size.1 as u32, serial)?;
        let id = handle.id;
        self.displays.add(id, name, size, owner, handle);
        // WindowServer needs a moment before the display accepts configuration.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |core, _| {
                let virtual_ids: Vec<DisplayId> = core.displays.list.iter().map(|d| d.id).collect();
                display::tuck_away(id, &virtual_ids);
            });
        })
        .detach();
        cx.notify();
        Some(id)
    }

    pub fn end(&mut self, id: DisplayId, reason: EndReason, cx: &mut Context<Self>) {
        if self.displays.end(id, reason) {
            cx.notify();
            schedule_sweep(cx);
        }
    }

    pub fn connection_closed(&mut self, connection: u64, cx: &mut Context<Self>) {
        self.closed.insert(connection);
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
