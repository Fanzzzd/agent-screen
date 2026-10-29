//! Window Access: what an Agent Session may take of the user's windows on
//! the user's word, as relayed in chat. Agent Screen trusts the relay; it
//! cannot check the chat. The words go into the log and the recording, and
//! the Viewer shows them.
//!
//! A grant lives on the session's display and ends with it. It never moves
//! a window by itself: it lets `place_window` hand a covered window over
//! without a Window Request, while the user isn't in control.

use crate::model::WindowId;
use std::collections::{BTreeMap, BTreeSet};

/// The longest instruction kept, in characters.
pub const INSTRUCTION_CHARS: usize = 300;

/// The user's words as the agent relays them, trimmed; why they can't be
/// used otherwise.
pub fn user_instruction(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("user_instruction is empty: pass the user's words from the chat".into());
    }
    if text.chars().count() > INSTRUCTION_CHARS {
        return Err(format!(
            "keep user_instruction under {INSTRUCTION_CHARS} characters: quote the user's instruction, not the conversation"
        ));
    }
    Ok(text.to_string())
}

/// An app in a grant: its bundle id decides; the name is for showing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AppId {
    pub bundle_id: String,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Every app's windows.
    All,
    /// These apps' windows, by bundle id, with their names.
    Apps(BTreeMap<String, String>),
}

/// What the agent did on the user's word, for the log and the Viewer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChatAction {
    HandBack,
    Pause,
    Place,
    Grant,
    Revoke,
    Return,
}

impl ChatAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatAction::HandBack => "handBack",
            ChatAction::Pause => "pause",
            ChatAction::Place => "place",
            ChatAction::Grant => "grant",
            ChatAction::Revoke => "revoke",
            ChatAction::Return => "return",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Instruction {
    pub action: ChatAction,
    pub text: String,
}

/// Why a window can't be handed over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// The display ended.
    Ended,
    /// The user is in control.
    Takeover,
    /// No grant covers the window's app.
    NotGranted,
    /// The user took the window back; only their explicit word brings it
    /// back.
    Excluded,
    /// Another display holds it: another session's or a Review Display's.
    Elsewhere,
    /// A handover of this window has not finished yet.
    Busy,
}

impl Refusal {
    /// What the agent reads.
    pub fn message(self, app: &str) -> String {
        match self {
            Refusal::Ended => "the display has ended".into(),
            Refusal::Busy => format!("this {app} window is already being handed over; wait for that placement to finish"),
            Refusal::Takeover => "the user is in control of the display, which pauses window access; call wait_for_control".into(),
            Refusal::NotGranted => format!("window access doesn't cover {app}"),
            Refusal::Excluded => format!(
                "the user took this {app} window back, so window access no longer covers it. Ask the user; if they tell you to take it again, call place_window with that window_id and their words as user_instruction"
            ),
            Refusal::Elsewhere => format!(
                "this {app} window belongs to another agent's display, or one waiting for review; it isn't the user's to give from here"
            ),
        }
    }
}

/// How a hand-over is authorized.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Authority {
    /// The user, by their own click or drag, or by a Window Request's
    /// Hand Over.
    User,
    /// The user's word in chat, for this one window.
    Once,
    /// The session's grant, for a window of this app.
    Grant { bundle_id: Option<String> },
}

/// One session's Window Access on its display.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Access {
    pub scope: Option<Scope>,
    /// Windows the user took back. No grant covers them again; only the
    /// user's word for that window, or their own Hand Over, does.
    pub excluded: BTreeSet<WindowId>,
    /// The last instruction the agent relayed.
    pub last: Option<Instruction>,
    /// Counts revokes, so a hand-over under the grant that was decided
    /// before one doesn't go ahead after it.
    pub revokes: u64,
}

impl Access {
    /// Add to the grant: all apps replace any list; apps join the list.
    pub fn grant_all(&mut self) {
        self.scope = Some(Scope::All);
    }

    pub fn grant_apps(&mut self, apps: impl IntoIterator<Item = AppId>) {
        match &mut self.scope {
            Some(Scope::All) => {}
            Some(Scope::Apps(list)) => {
                list.extend(apps.into_iter().map(|a| (a.bundle_id, a.name)));
            }
            None => {
                let list: BTreeMap<String, String> =
                    apps.into_iter().map(|a| (a.bundle_id, a.name)).collect();
                if !list.is_empty() {
                    self.scope = Some(Scope::Apps(list));
                }
            }
        }
    }

    /// Take the grant back, or some of its apps. Nothing moves. Taking an
    /// app out of an all-apps grant can't be said: Err.
    pub fn revoke(&mut self, apps: Option<&[String]>) -> Result<bool, String> {
        let changed = self.revoke_scope(apps)?;
        if changed {
            self.revokes += 1;
        }
        Ok(changed)
    }

    fn revoke_scope(&mut self, apps: Option<&[String]>) -> Result<bool, String> {
        let Some(apps) = apps else {
            return Ok(self.scope.take().is_some());
        };
        match &mut self.scope {
            None => Ok(false),
            Some(Scope::All) => Err(
                "window access covers all apps; revoke it all, then grant the apps the user still allows"
                    .into(),
            ),
            Some(Scope::Apps(list)) => {
                let before = list.len();
                list.retain(|id, _| !apps.contains(id));
                let changed = list.len() != before;
                if list.is_empty() {
                    self.scope = None;
                }
                Ok(changed)
            }
        }
    }

    pub fn covers(&self, bundle_id: Option<&str>) -> bool {
        match (&self.scope, bundle_id) {
            (Some(Scope::All), _) => true,
            (Some(Scope::Apps(list)), Some(id)) => list.contains_key(id),
            _ => false,
        }
    }

    /// The apps' names, for the Viewer and Peek; None for all apps.
    pub fn app_names(&self) -> Option<Vec<String>> {
        match &self.scope {
            Some(Scope::Apps(list)) => Some(list.values().cloned().collect()),
            _ => None,
        }
    }

    /// The session is done with the display: nothing is left of it.
    pub fn clear(&mut self) {
        let revokes = self.revokes + u64::from(self.scope.is_some());
        *self = Access {
            revokes,
            ..Access::default()
        };
    }

    /// What the agent and the e2e scripts read: the scope, whether it's
    /// paused, and what the user took back.
    pub fn scope_name(&self) -> &'static str {
        match self.scope {
            None => "none",
            Some(Scope::All) => "all",
            Some(Scope::Apps(_)) => "apps",
        }
    }

    /// The apps in the grant, by bundle id and name.
    pub fn apps(&self) -> Vec<AppId> {
        match &self.scope {
            Some(Scope::Apps(list)) => list
                .iter()
                .map(|(id, name)| AppId {
                    bundle_id: id.clone(),
                    name: name.clone(),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// An app named in a revoke: its bundle id when the grant has it by
    /// that id or name.
    pub fn granted_id(&self, app: &str) -> Option<String> {
        self.apps()
            .into_iter()
            .find(|a| a.bundle_id.eq_ignore_ascii_case(app) || a.name.eq_ignore_ascii_case(app))
            .map(|a| a.bundle_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> AppId {
        AppId {
            bundle_id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn instructions_are_the_users_words_trimmed() {
        assert_eq!(user_instruction("  好了，继续 ").unwrap(), "好了，继续");
        assert!(user_instruction("   ").is_err());
        assert!(user_instruction(&"x".repeat(INSTRUCTION_CHARS + 1)).is_err());
    }

    #[test]
    fn apps_join_and_all_replaces() {
        let mut a = Access::default();
        assert!(!a.covers(Some("com.apple.Safari")));
        a.grant_apps([app("com.apple.Safari", "Safari")]);
        a.grant_apps([app("com.utmapp.UTM", "UTM")]);
        assert!(a.covers(Some("com.apple.Safari")) && a.covers(Some("com.utmapp.UTM")));
        assert!(!a.covers(Some("com.apple.TextEdit")));
        assert!(!a.covers(None), "an app with no bundle id needs all apps");
        assert_eq!(a.app_names().unwrap(), ["Safari", "UTM"]);
        a.grant_all();
        assert!(a.covers(Some("com.apple.TextEdit")) && a.covers(None));
        a.grant_apps([app("com.apple.Notes", "Notes")]);
        assert_eq!(a.scope, Some(Scope::All), "all stays all");
    }

    #[test]
    fn revoking_takes_apps_or_everything() {
        let mut a = Access::default();
        assert_eq!(a.revoke(None), Ok(false));
        a.grant_apps([app("a.b", "A"), app("c.d", "C")]);
        assert_eq!(a.revoke(Some(&["a.b".into()])), Ok(true));
        assert!(!a.covers(Some("a.b")) && a.covers(Some("c.d")));
        assert_eq!(a.revoke(Some(&["c.d".into()])), Ok(true));
        assert_eq!(a.scope, None, "the last app gone is no grant");
        a.grant_all();
        assert!(a.revoke(Some(&["a.b".into()])).is_err());
        assert_eq!(a.scope, Some(Scope::All));
        assert_eq!(a.revoke(None), Ok(true));
        assert_eq!(a.scope, None);
    }

    #[test]
    fn a_new_grant_keeps_what_the_user_took_back() {
        let mut a = Access::default();
        a.excluded.insert(9);
        a.grant_all();
        a.revoke(None).unwrap();
        a.grant_apps([app("a.b", "A")]);
        assert!(a.excluded.contains(&9));
    }

    #[test]
    fn a_revoke_counts_only_when_it_takes_something() {
        let mut a = Access::default();
        a.revoke(None).unwrap();
        assert_eq!(a.revokes, 0);
        a.grant_apps([app("a.b", "Arc")]);
        assert_eq!(a.granted_id("arc").as_deref(), Some("a.b"));
        assert_eq!(a.granted_id("A.B").as_deref(), Some("a.b"));
        assert_eq!(a.granted_id("Safari"), None);
        a.revoke(Some(&["x.y".into()])).unwrap();
        assert_eq!(a.revokes, 0);
        a.revoke(Some(&["a.b".into()])).unwrap();
        assert_eq!(a.revokes, 1);
        a.grant_all();
        let before = a.revokes;
        a.clear();
        assert!(a.revokes > before, "ending the display counts as a revoke");
        assert_eq!(a.scope_name(), "none");
    }
}
