//! Platform capability negotiation over the `agentReady` control handshake.

use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub const LINUX_FEATURES: &[&str] = &[
    "multi_display",
    "surviving_adoption",
    "web_preview",
    "accessibility",
    "devtools_pipe",
    "recording",
    "in_display_clipboard_shortcuts",
];

pub const MACOS_FEATURES: &[&str] = &[
    "multi_display",
    "web_preview",
    "accessibility",
    "devtools_pipe",
    "recording",
    "host_window_handover",
    "cross_display_move",
    "input_queue",
    "ime_borrow",
    "separate_menubar",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub platform: String,
    pub features: BTreeSet<String>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::default_for_target()
    }
}

impl Capabilities {
    pub fn linux() -> Self {
        Self {
            platform: "linux".into(),
            features: LINUX_FEATURES.iter().copied().map(String::from).collect(),
        }
    }

    pub fn macos() -> Self {
        Self {
            platform: "macos".into(),
            features: MACOS_FEATURES.iter().copied().map(String::from).collect(),
        }
    }

    pub fn default_for_target() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::linux()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::macos()
        }
    }

    /// Parse the daemon's `agentReady` reply. Older macOS builds and legacy
    /// mock sockets return `{}` with no `features` array; preserve the full
    /// macOS tool surface for them.
    pub fn from_agent_ready(reply: &Map<String, Value>) -> Self {
        let Some(items) = reply.get("features").and_then(Value::as_array) else {
            return Self::macos();
        };
        let platform = reply
            .get("platform")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(std::env::consts::OS)
            .to_string();
        let features = items
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
        Self { platform, features }
    }

    pub fn has(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    pub fn to_json(&self) -> Value {
        let list: Vec<&str> = self.features.iter().map(String::as_str).collect();
        json!({
            "platform": self.platform,
            "features": list,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_empty_reply_preserves_macos_features() {
        let caps = Capabilities::from_agent_ready(&Map::new());
        assert_eq!(caps.platform, "macos");
        assert!(caps.has("input_queue"));
        assert!(caps.has("host_window_handover"));
        assert!(caps.has("ime_borrow"));
    }

    #[test]
    fn linux_reply_prunes_host_handover_and_input_queue() {
        let wire = Capabilities::linux().to_json();
        let caps = Capabilities::from_agent_ready(wire.as_object().unwrap());
        assert_eq!(caps.platform, "linux");
        assert!(caps.has("in_display_clipboard_shortcuts"));
        assert!(caps.has("surviving_adoption"));
        assert!(!caps.has("input_queue"));
        assert!(!caps.has("host_window_handover"));
        assert!(!caps.has("ime_borrow"));
        assert!(!caps.has("separate_menubar"));
    }
}
