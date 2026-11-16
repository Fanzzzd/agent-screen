//! Session disconnect grace tracking (`DISCONNECT_GRACE_MS`): when an MCP
//! socket connection drops, its `sessionKey` gets a 2 s grace window to
//! reconnect before its displays end with `EndReason::Disconnected`.

use std::collections::HashMap;
use std::time::Duration;

pub const DISCONNECT_GRACE_MS: u64 = 2000;
pub const DISCONNECT_GRACE: Duration = Duration::from_millis(DISCONNECT_GRACE_MS);

/// Maps live socket connection IDs to the last `sessionKey` seen on each
/// connection so `Closed` events know which session disconnected.
#[derive(Default)]
pub struct ConnectionSessions {
    by_connection: HashMap<u64, String>,
}

impl ConnectionSessions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, connection: u64, session_key: impl Into<String>) {
        self.by_connection.insert(connection, session_key.into());
    }

    /// Remove `connection` and return its `sessionKey` only if no other live
    /// connection is still bound to the same `sessionKey`.
    pub fn on_closed(&mut self, connection: u64) -> Option<String> {
        let session = self.by_connection.remove(&connection)?;
        let still_attached = self.by_connection.values().any(|s| s == &session);
        (!still_attached).then_some(session)
    }

    pub fn has_live_connection(&self, session_key: &str) -> bool {
        self.by_connection.values().any(|s| s == session_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_connection_for_same_session_prevents_premature_disconnect() {
        let mut sessions = ConnectionSessions::new();
        sessions.bind(1, "sess-a");
        sessions.bind(2, "sess-a");
        assert_eq!(sessions.on_closed(1), None);
        assert!(sessions.has_live_connection("sess-a"));
        assert_eq!(sessions.on_closed(2).as_deref(), Some("sess-a"));
        assert!(!sessions.has_live_connection("sess-a"));
    }
}
