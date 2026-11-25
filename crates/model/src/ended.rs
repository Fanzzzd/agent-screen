//! Why an Agent Display ended, kept after the display is gone, and what an
//! agent that names it is told (#326).
//!
//! A display ends in the running app in one of three ways
//! ([`EndReason`]): its session called `end_display`, its session's last
//! connection closed and the Disconnect Grace ran out, or the user closed
//! it. A fourth, the app quitting, is known only to the MCP host's ledger
//! ([`app_quit_message`]). Nothing else ends a live display: idle time,
//! macOS reconfiguring the screens and a Takeover leave it live.
//!
//! The agent's error used to say only "has ended", and once an empty
//! display was swept nothing told an end_display from a dropped
//! connection. [`Endings`] keeps the reason and the owner's session key
//! for the last [`KEEP`] ends.

use std::collections::VecDeque;

use crate::model::{DisplayId, EndReason};

/// How many ends [`Endings`] remembers. An agent names a display it lost
/// within minutes; older ids read "does not exist".
pub const KEEP: usize = 64;

/// One display's end: why, and which Agent Session it belonged to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ending {
    pub id: DisplayId,
    pub reason: EndReason,
    /// The owner's session key; `None` for a display the user made.
    pub session_key: Option<String>,
}

/// The last [`KEEP`] ends, one per display id, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Endings {
    list: VecDeque<Ending>,
}

impl Endings {
    pub const fn new() -> Self {
        Self {
            list: VecDeque::new(),
        }
    }

    /// Display `id` ended. A later end of the same id replaces the earlier
    /// one; past [`KEEP`] the oldest goes.
    pub fn record(&mut self, id: DisplayId, reason: EndReason, session_key: Option<String>) {
        self.forget(id);
        if self.list.len() >= KEEP {
            self.list.pop_front();
        }
        self.list.push_back(Ending {
            id,
            reason,
            session_key,
        });
    }

    pub fn get(&self, id: DisplayId) -> Option<&Ending> {
        self.list.iter().find(|e| e.id == id)
    }

    /// The id is live again, as a spare handed back under its old id.
    pub fn forget(&mut self, id: DisplayId) {
        self.list.retain(|e| e.id != id);
    }

    /// The last `n` ends, newest first.
    pub fn latest(&self, n: usize) -> impl Iterator<Item = &Ending> {
        self.list.iter().rev().take(n)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

impl EndReason {
    /// The reason as log lines, diagnostics and the control protocol name
    /// it.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Finished => "end_display",
            Self::Disconnected => "disconnected",
            Self::ClosedByUser => "closed_by_user",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        [Self::Finished, Self::Disconnected, Self::ClosedByUser]
            .into_iter()
            .find(|r| r.tag() == tag)
    }
}

/// What an agent is told when it names display `id`, which ended for
/// `reason`. `own` is whether the asking session is the one the display
/// belonged to; a session that can't say which it is gets `false`.
pub fn ended_message(id: DisplayId, reason: EndReason, own: bool) -> String {
    match (reason, own) {
        // The user wants the agent to stop (ADR 0004), whoever asks.
        (EndReason::ClosedByUser, _) => format!(
            "the user closed display {id}, so stop working on screen; ask the user before you call create_display again"
        ),
        (EndReason::Finished, true) => format!(
            "display {id} has ended: this session ended it with end_display. Call create_display for a new one"
        ),
        (EndReason::Finished, false) => format!(
            "display {id} has ended: the session that made it ended it with end_display. Call create_display for a display of your own"
        ),
        (EndReason::Disconnected, true) => format!(
            "display {id} has ended: this session's connection to Agent Screen closed, and no call of the session came within the 2 s Disconnect Grace, so Agent Screen counted the session as gone. Call create_display for a new one; windows left on the old display wait there for the user's review"
        ),
        (EndReason::Disconnected, false) => format!(
            "display {id} has ended: the connection of the session that made it closed, and no call of that session came within the 2 s Disconnect Grace. If your client restarted its MCP server (AgentScreen mcp), that was your own earlier session: a client that sends no session ID starts a new session with each server process. Call create_display for a new display; windows left on the old one wait there for the user's review"
        ),
    }
}

/// What a call naming display `id` gets when the display ended because the
/// Agent Screen app quit, and has restarted since.
pub fn app_quit_message(id: i64) -> String {
    format!(
        "display {id} ended when the Agent Screen app quit; it has restarted. macOS moved the display's windows to the user's screen. Call create_display, then list_windows with strays: true and place_window to bring back windows of apps you launched there."
    )
}

/// A session key as logs and diagnostics show it: its kind and a short
/// hash, so lines of one session match and a restarted MCP server shows as
/// a new session, while the client's own session ID stays out of a bug
/// report. "none" for a display the user made.
pub fn session_tag(key: Option<&str>) -> String {
    let Some(key) = key else {
        return "none".into();
    };
    let kind = key
        .split_once(':')
        .map(|(kind, _)| kind)
        .filter(|kind| {
            !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or("other");
    // FNV-1a, 32 bits: stable across runs and builds.
    let hash = key.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    format!("{kind}:{hash:08x}")
}

/// What follows the display id in the line logged as it ends.
pub fn log_detail(reason: EndReason, session_key: Option<&str>) -> String {
    format!(
        "reason={} session={}",
        reason.tag(),
        session_tag(session_key)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [EndReason; 3] = [
        EndReason::Finished,
        EndReason::Disconnected,
        EndReason::ClosedByUser,
    ];

    #[test]
    fn every_reason_and_asker_gets_its_own_text() {
        let texts: Vec<String> = ALL
            .iter()
            .flat_map(|&r| [ended_message(7, r, true), ended_message(7, r, false)])
            .collect();
        // Closed by the user reads the same to anyone; the rest differ.
        let mut distinct = texts.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 5, "{texts:#?}");
        for text in &texts {
            assert!(text.contains("display 7"), "{text}");
            assert!(text.contains("create_display"), "{text}");
            // Clients and scripts look for this word.
            assert!(text.contains("ended") || text.contains("closed"), "{text}");
        }
    }

    #[test]
    fn end_display_names_who_ended_it() {
        assert_eq!(
            ended_message(7, EndReason::Finished, true),
            "display 7 has ended: this session ended it with end_display. Call create_display for a new one"
        );
        let other = ended_message(7, EndReason::Finished, false);
        assert!(other.contains("the session that made it ended it with end_display"));
        assert!(other.contains("a display of your own"));
    }

    #[test]
    fn a_disconnect_names_the_grace_and_a_restarted_server() {
        let own = ended_message(7, EndReason::Disconnected, true);
        assert!(own.starts_with("display 7 has ended: this session's connection"));
        assert!(own.contains("2 s Disconnect Grace"));
        assert!(own.contains("wait there for the user's review"));
        let other = ended_message(7, EndReason::Disconnected, false);
        assert!(other.contains("If your client restarted its MCP server"));
        assert!(other.contains("sends no session ID"));
        assert!(other.contains("Call create_display for a new display"));
    }

    #[test]
    fn closed_by_the_user_tells_any_asker_to_stop() {
        for own in [true, false] {
            assert_eq!(
                ended_message(3, EndReason::ClosedByUser, own),
                "the user closed display 3, so stop working on screen; ask the user before you call create_display again"
            );
        }
    }

    #[test]
    fn an_app_quit_says_it_restarted_and_how_to_get_windows_back() {
        let text = app_quit_message(9);
        assert!(text.starts_with("display 9 ended when the Agent Screen app quit"));
        assert!(text.contains("list_windows with strays: true"));
    }

    #[test]
    fn tags_name_each_reason_and_read_back() {
        let tags: Vec<_> = ALL.iter().map(|r| r.tag()).collect();
        assert_eq!(tags, ["end_display", "disconnected", "closed_by_user"]);
        for r in ALL {
            assert_eq!(EndReason::from_tag(r.tag()), Some(r));
        }
        assert_eq!(EndReason::from_tag("idle"), None);
    }

    #[test]
    fn a_session_tag_keeps_the_kind_and_hides_the_id() {
        let a = session_tag(Some("claude-code:4f1c2b9e-1111-2222-3333-444455556666"));
        assert!(a.starts_with("claude-code:"), "{a}");
        assert!(!a.contains("4f1c2b9e"), "{a}");
        assert_eq!(a.len(), "claude-code:".len() + 8);
        // Stable, and another session differs.
        assert_eq!(
            a,
            session_tag(Some("claude-code:4f1c2b9e-1111-2222-3333-444455556666"))
        );
        assert_ne!(
            session_tag(Some("pipe:123-aaaa")),
            session_tag(Some("pipe:124-aaaa"))
        );
        assert!(session_tag(Some("pipe:123-aaaa")).starts_with("pipe:"));
        assert!(session_tag(Some("codex:x")).starts_with("codex:"));
        assert!(session_tag(Some("pid:42")).starts_with("pid:"));
        assert!(session_tag(Some("/Users/me/x")).starts_with("other:"));
        assert_eq!(session_tag(None), "none");
    }

    #[test]
    fn the_log_detail_names_reason_and_session() {
        let detail = log_detail(EndReason::Disconnected, Some("pipe:1-ab"));
        assert!(
            detail.starts_with("reason=disconnected session=pipe:"),
            "{detail}"
        );
        assert_eq!(
            log_detail(EndReason::Finished, None),
            "reason=end_display session=none"
        );
    }

    #[test]
    fn endings_keep_the_reason_and_key_one_per_id() {
        let mut e = Endings::new();
        e.record(7, EndReason::Finished, Some("pipe:a".into()));
        e.record(8, EndReason::Disconnected, None);
        e.record(7, EndReason::ClosedByUser, Some("pipe:b".into()));
        assert_eq!(e.len(), 2);
        let seven = e.get(7).unwrap();
        assert_eq!(seven.reason, EndReason::ClosedByUser);
        assert_eq!(seven.session_key.as_deref(), Some("pipe:b"));
        let newest: Vec<_> = e.latest(5).map(|x| x.id).collect();
        assert_eq!(newest, [7, 8]);
        e.forget(7);
        assert!(e.get(7).is_none());
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn endings_stay_bounded_and_drop_the_oldest() {
        let mut e = Endings::new();
        for id in 0..(KEEP as u32 + 10) {
            e.record(id, EndReason::Finished, None);
        }
        assert_eq!(e.len(), KEEP);
        assert!(e.get(9).is_none());
        assert!(e.get(10).is_some());
        assert_eq!(e.latest(1).next().unwrap().id, KEEP as u32 + 9);
    }
}
