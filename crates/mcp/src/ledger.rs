//! Which session made which display, and the apps launch_app started there,
//! so a display that ended when the app quit can say so and hand its apps
//! to the session's next display.
//!
//! Entries belong to a run. A v1 process counts a run per control
//! connection, since each new connection is a new app. The app counts two:
//! the previous app run, read from its file at start, and its own, which it
//! writes back on every change (ADR 0020).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Default)]
pub struct Ledger {
    state: Mutex<State>,
    /// The app's file; `None` for a v1 process, which keeps it in memory.
    file: Option<PathBuf>,
}

#[derive(Default)]
struct State {
    run: u64,
    /// By display id and the run it was made in.
    made: HashMap<(u64, u64), Made>,
}

#[derive(Default, Clone)]
struct Made {
    session: String,
    started: Vec<i64>,
    /// Its apps were handed to a later display.
    adopted: bool,
}

/// One display in the file.
#[derive(Serialize, Deserialize)]
struct Saved {
    display: u64,
    session: String,
    pids: Vec<i64>,
}

impl Ledger {
    /// The app's ledger: the previous run's displays from `file`, then this
    /// run's, written back to it.
    pub fn persistent(file: PathBuf) -> Self {
        let saved: Vec<Saved> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let made = saved
            .into_iter()
            .map(|s| {
                (
                    (s.display, 0),
                    Made {
                        session: s.session,
                        started: s.pids,
                        adopted: false,
                    },
                )
            })
            .collect();
        let ledger = Self {
            state: Mutex::new(State { run: 1, made }),
            file: Some(file),
        };
        ledger.save(&ledger.state.lock().unwrap());
        ledger
    }

    /// A v1 process opened another control connection: the app it reaches
    /// started anew. The app's own ledger has one run for its life.
    pub fn connected(&self) {
        if self.file.is_none() {
            self.state.lock().unwrap().run += 1;
        }
    }

    /// Note a display session `session` made, or reused when `reused`, in
    /// this run. A display made anew, or one the app handed to another
    /// session under an old id, starts with no apps: its old apps were
    /// another display's, maybe another session's (ADR 0016).
    pub fn made_display(&self, id: u64, session: &str, reused: bool) {
        let mut state = self.state.lock().unwrap();
        let run = state.run;
        let entry = state.made.entry((id, run)).or_default();
        if !reused || entry.session != session {
            *entry = Made {
                session: session.into(),
                ..Made::default()
            };
        }
        self.save(&state);
    }

    /// Note an app launch_app started for a display.
    pub fn started_app(&self, display: u64, pid: i64) {
        let mut state = self.state.lock().unwrap();
        let run = state.run;
        if let Some(made) = state.made.get_mut(&(display, run)) {
            made.started.push(pid);
            self.save(&state);
        }
    }

    /// Whether a display session `session` made ended when the app quit:
    /// made in an earlier run, and not made for it again under its id since.
    pub fn ended_with_app(&self, id: u64, session: &str) -> bool {
        let state = self.state.lock().unwrap();
        !state
            .made
            .get(&(id, state.run))
            .is_some_and(|m| m.session == session)
            && state
                .made
                .iter()
                .any(|(&(d, r), m)| d == id && r < state.run && m.session == session)
    }

    /// The apps session `session` started on displays that ended when the
    /// app quit, by the display they were on, taken once, for its next
    /// create_display to adopt. Never another session's: an app one agent
    /// launched once went onto another agent's display (ADR 0016).
    pub fn orphans(&self, session: &str) -> Option<Vec<(u64, Vec<i64>)>> {
        let mut state = self.state.lock().unwrap();
        let run = state.run;
        let mut keys: Vec<(u64, u64)> = state
            .made
            .keys()
            .filter(|&&(_, r)| r < run)
            .copied()
            .collect();
        keys.sort();
        let mut any = false;
        let mut groups = Vec::new();
        for key in keys {
            let Some(m) = state.made.get_mut(&key) else {
                continue;
            };
            if m.session == session && !m.adopted {
                m.adopted = true;
                any = true;
                groups.push((key.0, std::mem::take(&mut m.started)));
            }
        }
        any.then_some(groups)
    }

    /// Write this run's displays, for the next app run. Earlier runs' are
    /// left out: the next run can't reach them either.
    fn save(&self, state: &State) {
        let Some(file) = &self.file else {
            return;
        };
        let mut saved: Vec<Saved> = state
            .made
            .iter()
            .filter(|&(&(_, r), _)| r == state.run)
            .map(|(&(display, _), m)| Saved {
                display,
                session: m.session.clone(),
                pids: m.started.clone(),
            })
            .collect();
        saved.sort_by_key(|s| s.display);
        let Ok(bytes) = serde_json::to_vec(&saved) else {
            return;
        };
        // Write beside it and rename, so an abort mid-write keeps the last
        // whole ledger.
        let partial = file.with_extension("json.partial");
        if std::fs::write(&partial, bytes).is_ok() {
            let _ = std::fs::rename(&partial, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_ends_with_the_connection_it_was_made_on() {
        let ledger = Ledger::default();
        ledger.connected();
        ledger.made_display(7, "a", false);
        assert!(!ledger.ended_with_app(7, "a"));
        ledger.connected();
        assert!(ledger.ended_with_app(7, "a"));
        // Another session's, or none: the app's own answer stands.
        assert!(!ledger.ended_with_app(7, "b"));
        assert!(!ledger.ended_with_app(8, "a"));
        // The restarted app gave the id to a new display.
        ledger.made_display(7, "a", false);
        assert!(!ledger.ended_with_app(7, "a"));
    }

    #[test]
    fn the_next_display_adopts_the_apps_started_on_ended_ones_once() {
        let ledger = Ledger::default();
        ledger.connected();
        assert_eq!(ledger.orphans("a"), None);
        ledger.made_display(7, "a", false);
        ledger.started_app(7, 100);
        ledger.started_app(7, 101);
        // An app for a display this ledger doesn't know isn't kept.
        ledger.started_app(9, 102);
        assert_eq!(ledger.orphans("a"), None);
        ledger.connected();
        assert_eq!(ledger.orphans("a"), Some(vec![(7, vec![100, 101])]));
        assert_eq!(ledger.orphans("a"), None);
        // A display with no apps still says the app quit.
        ledger.made_display(8, "a", false);
        ledger.connected();
        assert_eq!(ledger.orphans("a"), Some(vec![(8, vec![])]));
    }

    #[test]
    fn a_session_adopts_only_its_own_apps_by_the_display_they_were_on() {
        let ledger = Ledger::default();
        ledger.connected();
        ledger.made_display(7, "a", false);
        ledger.started_app(7, 100);
        ledger.made_display(8, "a", false);
        ledger.started_app(8, 101);
        ledger.made_display(9, "b", false);
        ledger.started_app(9, 200);
        ledger.connected();
        // The app gave id 9 to session a's new display: b's apps stay b's.
        ledger.made_display(9, "a", false);
        assert_eq!(
            ledger.orphans("a"),
            Some(vec![(7, vec![100]), (8, vec![101])])
        );
        assert_eq!(ledger.orphans("b"), Some(vec![(9, vec![200])]));
        assert_eq!(ledger.orphans("c"), None);
    }

    #[test]
    fn a_display_id_made_again_forgets_the_apps_of_the_display_before() {
        let ledger = Ledger::default();
        ledger.connected();
        ledger.made_display(7, "a", false);
        ledger.started_app(7, 100);
        // Reused by its own session: the apps stay.
        ledger.made_display(7, "a", true);
        ledger.started_app(7, 101);
        // Session a ended it; the app handed the spare back to b as 7.
        ledger.made_display(7, "b", false);
        ledger.started_app(7, 200);
        ledger.connected();
        assert_eq!(ledger.orphans("a"), None);
        assert_eq!(ledger.orphans("b"), Some(vec![(7, vec![200])]));
    }

    #[test]
    fn the_apps_ledger_outlives_its_run_and_only_one() {
        let dir = std::env::temp_dir().join(format!("as-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("displays-test.json");
        let _ = std::fs::remove_file(&file);

        let first = Ledger::persistent(file.clone());
        first.made_display(3, "claude-code:s", false);
        first.started_app(3, 501);
        // The app aborts here: nothing else is written.
        drop(first);

        let second = Ledger::persistent(file.clone());
        // A reconnecting pipe's connection is still this run.
        second.connected();
        assert!(second.ended_with_app(3, "claude-code:s"));
        assert!(!second.ended_with_app(3, "codex:t"));
        assert_eq!(second.orphans("claude-code:s"), Some(vec![(3, vec![501])]));
        second.made_display(4, "codex:t", false);
        drop(second);

        // The run before last is gone; only the last run's displays stay.
        let third = Ledger::persistent(file.clone());
        assert!(!third.ended_with_app(3, "claude-code:s"));
        assert!(third.ended_with_app(4, "codex:t"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
