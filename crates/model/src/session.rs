//! Agent Sessions: what one run of an agent holds across its Agent
//! Displays (ADR 0016).
//!
//! A session can use up to [`MAX_LIVE`] displays at once. It owns its
//! Window Authorization, which covers every one of them, and the order it
//! made them in, so a `create_display` without `new` reuses the latest. Each
//! display keeps its own records, Takeover, pointer, requests and Review
//! state in [`crate::model::Display`].
//!
//! Display ids here are the session's live displays only. A display leaves
//! the list when it ends; when the last one leaves, the Window
//! Authorization ends with it, as it did with the session's one display.

use crate::access::Access;
use crate::model::DisplayId;
use std::collections::BTreeMap;

/// The most live Agent Displays one session may use at once. Every new
/// display makes WindowServer reconfigure each screen, and dozens in a few
/// hours once froze a Mac. Review Displays don't count.
pub const MAX_LIVE: usize = 4;

#[derive(Debug, Default)]
pub struct Session {
    /// Its live displays, oldest first.
    pub displays: Vec<DisplayId>,
    /// The session's Window Authorization: what it may take of the user's
    /// windows on their word, onto any of its displays (ADR 0013).
    pub access: Access,
}

impl Session {
    /// The display a `create_display` without `new` reuses: the latest.
    pub fn current(&self) -> Option<DisplayId> {
        self.displays.last().copied()
    }
}

/// Every Agent Session that has had a display, by session key.
#[derive(Debug, Default)]
pub struct Sessions {
    map: BTreeMap<String, Session>,
}

/// Why a session may not have another display now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtCap {
    pub live: Vec<DisplayId>,
}

impl AtCap {
    /// What the agent reads.
    pub fn message(&self) -> String {
        let handles: Vec<String> = self.live.iter().map(|id| id.to_string()).collect();
        format!(
            "this session already uses {} displays ({}), the most one session may have at once. Keep working on one of them, or call end_display on one you no longer need, then call create_display with new: true again",
            self.live.len(),
            handles.join(", ")
        )
    }
}

impl Sessions {
    pub fn get(&self, key: &str) -> Option<&Session> {
        self.map.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Session> {
        self.map.get_mut(key)
    }

    /// The session's live displays, oldest first.
    pub fn live(&self, key: &str) -> &[DisplayId] {
        self.map.get(key).map_or(&[], |s| s.displays.as_slice())
    }

    /// Whether the session may add a display now.
    pub fn may_add(&self, key: &str) -> Result<(), AtCap> {
        let live = self.live(key);
        if live.len() >= MAX_LIVE {
            Err(AtCap {
                live: live.to_vec(),
            })
        } else {
            Ok(())
        }
    }

    /// A display the session now uses.
    pub fn join(&mut self, key: &str, id: DisplayId) {
        let session = self.map.entry(key.to_string()).or_default();
        if !session.displays.contains(&id) {
            session.displays.push(id);
        }
    }

    /// The session stopped using display `id`. When it was the last, the
    /// Window Authorization ends: nothing is left for it to cover, and a
    /// later display starts without it. True when it was the session's.
    pub fn leave(&mut self, key: &str, id: DisplayId) -> bool {
        let Some(session) = self.map.get_mut(key) else {
            return false;
        };
        let before = session.displays.len();
        session.displays.retain(|&d| d != id);
        let left = session.displays.len() != before;
        if left && session.displays.is_empty() {
            session.access.clear();
        }
        left
    }
}

// MARK: Adoption after a restart

/// Apps the session started on a display that ended when the app quit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orphans {
    /// The display they were on, by its old id.
    pub display: DisplayId,
    pub pids: Vec<i32>,
}

/// Which display adopts each orphaned app, so its windows, which macOS
/// moved to the user's screen, are that display's strays. Orphans come only
/// from the asking session's own earlier displays: the MCP server keeps
/// them by session key, so another session's apps never arrive here. A
/// display made again under its old id gets its own apps back; every other
/// app goes to `new`, the display this call made or reused. A pid in
/// `held`, which a live display already started, or listed twice, is left
/// out.
pub fn adoption(
    orphans: &[Orphans],
    session_live: &[DisplayId],
    new: DisplayId,
    held: impl Fn(i32) -> bool,
) -> Vec<(DisplayId, i32)> {
    let mut out: Vec<(DisplayId, i32)> = Vec::new();
    for group in orphans {
        let to = if session_live.contains(&group.display) {
            group.display
        } else {
            new
        };
        for &pid in &group.pids {
            if pid > 0 && !held(pid) && !out.iter().any(|(_, p)| *p == pid) {
                out.push((to, pid));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_keeps_its_displays_in_order_and_reuses_the_latest() {
        let mut s = Sessions::default();
        s.join("a", 7);
        s.join("a", 8);
        s.join("a", 8);
        assert_eq!(s.live("a"), &[7, 8]);
        assert_eq!(s.get("a").unwrap().current(), Some(8));
        assert!(s.live("b").is_empty());
    }

    #[test]
    fn a_session_has_at_most_four_live_displays() {
        let mut s = Sessions::default();
        for id in 1..=MAX_LIVE as DisplayId {
            assert!(s.may_add("a").is_ok());
            s.join("a", id);
        }
        let cap = s.may_add("a").unwrap_err();
        assert_eq!(cap.live, vec![1, 2, 3, 4]);
        let message = cap.message();
        assert!(message.contains("1, 2, 3, 4") && message.contains("end_display"));
        assert!(s.may_add("b").is_ok(), "another session has its own count");
        // A display that ended, a Review Display included, frees a place.
        s.leave("a", 2);
        assert!(s.may_add("a").is_ok());
    }

    #[test]
    fn the_authorization_ends_with_the_last_display_only() {
        let mut s = Sessions::default();
        s.join("a", 1);
        s.join("a", 2);
        s.get_mut("a").unwrap().access.grant_all();
        assert!(s.leave("a", 1));
        assert!(
            s.get("a").unwrap().access.scope.is_some(),
            "display 2 lives"
        );
        assert!(s.leave("a", 2));
        assert_eq!(s.get("a").unwrap().access.scope, None);
        assert!(!s.leave("a", 2), "already gone");
    }

    #[test]
    fn orphans_go_to_their_own_display_made_again_or_else_to_the_new_one() {
        let orphans = [
            Orphans {
                display: 5,
                pids: vec![100, 101],
            },
            Orphans {
                display: 6,
                pids: vec![200, 100],
            },
        ];
        // Display 5 was made again under its id; 6 wasn't.
        let given = adoption(&orphans, &[5], 9, |_| false);
        assert_eq!(given, vec![(5, 100), (5, 101), (9, 200)]);
        // A pid a live display already started stays with it.
        let given = adoption(&orphans, &[], 9, |pid| pid == 101);
        assert_eq!(given, vec![(9, 100), (9, 200)]);
        assert!(adoption(&[], &[5], 9, |_| false).is_empty());
    }
}
