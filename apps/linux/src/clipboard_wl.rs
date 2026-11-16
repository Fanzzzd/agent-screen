//! Cross-compositor clipboard isolation and Takeover bridge (`wlr-data-control`).
//!
//! Each isolated `sway` Agent Display keeps its own `wl_data_device` selection.
//! While the agent runs in the background, host and display clipboards never
//! touch: apps inside the display can `ctrl+c` and `ctrl+v` freely without
//! reading the user's host clipboard or overwriting it.
//!
//! Only while `takeover` is active AND the user's Viewer holds host focus may
//! `ClipboardBridge` synchronize text or PNG images across the boundary; as
//! soon as the user hands control back or leaves the Viewer, synchronization
//! stops immediately.

use crate::ports::{ClipboardPayload, ClipboardSync, Display, PortError};
use agent_screen_model::model::DisplayId;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct BridgeSlot {
    viewer_focused: bool,
    last_synced: Option<ClipboardPayload>,
}

#[derive(Default)]
pub struct ClipboardBridge {
    slots: Mutex<HashMap<DisplayId, BridgeSlot>>,
}

impl ClipboardBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update whether the user's Viewer window for `display_id` holds host focus.
    pub fn set_viewer_focused(&self, display_id: DisplayId, focused: bool) {
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.entry(display_id).or_default();
        slot.viewer_focused = focused;
        if !focused {
            slot.last_synced = None;
        }
    }

    /// Clear bridge state when Takeover ends or a display is removed.
    pub fn disconnect(&self, display_id: DisplayId) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get_mut(&display_id) {
            slot.viewer_focused = false;
            slot.last_synced = None;
        }
    }

    pub fn remove_display(&self, display_id: DisplayId) {
        self.slots.lock().unwrap().remove(&display_id);
    }

    /// Whether clipboard sync is permitted right now: requires BOTH active
    /// Takeover on the display AND host focus on its Viewer window.
    pub fn may_sync(&self, display_id: DisplayId, takeover_active: bool) -> bool {
        if !takeover_active {
            return false;
        }
        self.slots
            .lock()
            .unwrap()
            .get(&display_id)
            .is_some_and(|s| s.viewer_focused)
    }

    /// Push a host clipboard payload into `display` when Takeover + Viewer
    /// focus permit it. Returns `Ok(false)` without touching `display` when
    /// isolated.
    pub async fn sync_host_to_display(
        &self,
        display: &dyn Display,
        takeover_active: bool,
        payload: ClipboardPayload,
    ) -> Result<bool, PortError> {
        let id = display.id();
        if !self.may_sync(id, takeover_active) {
            return Ok(false);
        }
        {
            let slots = self.slots.lock().unwrap();
            if slots
                .get(&id)
                .and_then(|s| s.last_synced.as_ref())
                .is_some_and(|prev| prev == &payload)
            {
                return Ok(true);
            }
        }
        match &payload {
            ClipboardPayload::Empty => {
                display.clipboard(ClipboardSync::Clear).await?;
            }
            ClipboardPayload::Text(text) => {
                display
                    .clipboard(ClipboardSync::WriteText(text.clone()))
                    .await?;
            }
            ClipboardPayload::ImagePng(png) => {
                display
                    .clipboard(ClipboardSync::WriteImagePng(png.clone()))
                    .await?;
            }
        }
        if let Some(slot) = self.slots.lock().unwrap().get_mut(&id) {
            slot.last_synced = Some(payload);
        }
        Ok(true)
    }

    /// Pull the current payload from `display` for writing to the host
    /// clipboard when Takeover + Viewer focus permit it. Returns `Ok(None)`
    /// without reading `display` when isolated.
    pub async fn sync_display_to_host(
        &self,
        display: &dyn Display,
        takeover_active: bool,
    ) -> Result<Option<ClipboardPayload>, PortError> {
        let id = display.id();
        if !self.may_sync(id, takeover_active) {
            return Ok(None);
        }
        let payload = display.clipboard(ClipboardSync::Read).await?;
        if payload == ClipboardPayload::Empty {
            return Ok(None);
        }
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.entry(id).or_default();
        slot.last_synced = Some(payload.clone());
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_requires_both_takeover_and_viewer_focus() {
        let bridge = ClipboardBridge::new();
        assert!(!bridge.may_sync(1, false));
        assert!(!bridge.may_sync(1, true));

        bridge.set_viewer_focused(1, true);
        assert!(
            !bridge.may_sync(1, false),
            "focused Viewer without Takeover must not sync clipboard"
        );
        assert!(bridge.may_sync(1, true));

        bridge.disconnect(1);
        assert!(!bridge.may_sync(1, true));
    }
}
