//! Which session made which display, and the apps launch_app started there,
//! so a display that ended when the app quit can say so and hand its apps
//! to the session's next display.
//!
//! Entries belong to a run. A v1 process counts a run per control
//! connection, since each new connection is a new app. The app counts two:
//! the previous app run, read from its file at start, and its own, which it
//! writes back on every change (ADR 0021).
//!
//! Each app is kept with its process's identity, its start time and
//! executable, and handed on only while the process with its pid still has
//! it: a pid macOS gave to another process since, maybe one of the user's
//! apps, is never adopted.

use crate::process::{self, Identity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Ledger {
    state: Mutex<State>,
    /// The app's file; `None` for a v1 process, which keeps it in memory.
    file: Option<PathBuf>,
    /// Who has a pid now; tests give their own.
    identify: fn(i64) -> Option<Identity>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            file: None,
            identify: process::identity,
        }
    }
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
    started: Vec<Started>,
    /// Its apps were handed to a later display.
    adopted: bool,
}

/// An app launch_app started, and its process when it started.
#[derive(Clone, Serialize, Deserialize)]
struct Started {
    pid: i64,
    /// `None` when macOS didn't say; such an app is never handed on.
    process: Option<Identity>,
}

/// One display in the file.
#[derive(Serialize, Deserialize)]
struct Saved {
    display: u64,
    session: String,
    apps: Vec<Started>,
}

impl Ledger {
    /// The app's ledger: the previous run's displays from `file`, then this
    /// run's, written back to it.
    pub fn persistent(file: PathBuf) -> Self {
        Self::persistent_with(file, process::identity, process::boot_time())
    }

    /// `persistent`, with who has a pid and when the Mac booted given. A
    /// file from before the boot is the run before a restart, whose
    /// displays and apps are long gone.
    pub fn persistent_with(
        file: PathBuf,
        identify: fn(i64) -> Option<Identity>,
        boot: Option<std::time::SystemTime>,
    ) -> Self {
        let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        let stale = matches!((modified, boot), (Some(m), Some(b)) if m < b);
        let saved: Vec<Saved> = std::fs::read(&file)
            .ok()
            .filter(|_| !stale)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let made = saved
            .into_iter()
            .map(|s| {
                (
                    (s.display, 0),
                    Made {
                        session: s.session,
                        started: s.apps,
                        adopted: false,
                    },
                )
            })
            .collect();
        let ledger = Self {
            state: Mutex::new(State { run: 1, made }),
            file: Some(file),
            identify,
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
        let process = (self.identify)(pid);
        let mut state = self.state.lock().unwrap();
        let run = state.run;
        if let Some(made) = state.made.get_mut(&(display, run)) {
            made.started.push(Started { pid, process });
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
    /// launched once went onto another agent's display (ADR 0016). Never a
    /// pid another process has now.
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
                let alive = std::mem::take(&mut m.started)
                    .into_iter()
                    .filter(|s| s.process.is_some() && (self.identify)(s.pid) == s.process)
                    .map(|s| s.pid)
                    .collect();
                groups.push((key.0, alive));
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
                apps: m.started.clone(),
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
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Pid 666 is a new process every time it is asked about; any other
    /// pid is the same one throughout.
    fn identify(pid: i64) -> Option<Identity> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let start_us = match pid {
            666 => NEXT.fetch_add(1, Ordering::Relaxed),
            p if p > 0 => p as u64,
            _ => return None,
        };
        Some(Identity {
            start_us,
            executable: format!("/Applications/App{pid}.app/Contents/MacOS/App"),
        })
    }

    fn ledger() -> Ledger {
        Ledger {
            identify,
            ..Ledger::default()
        }
    }

    #[test]
    fn a_display_ends_with_the_connection_it_was_made_on() {
        let ledger = ledger();
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
        let ledger = ledger();
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
        let ledger = ledger();
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
        let ledger = ledger();
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
    fn a_pid_another_process_has_now_is_never_adopted() {
        let ledger = ledger();
        ledger.connected();
        ledger.made_display(7, "a", false);
        ledger.started_app(7, 100);
        // macOS gave pid 666 to another process after launch_app.
        ledger.started_app(7, 666);
        ledger.connected();
        assert_eq!(ledger.orphans("a"), Some(vec![(7, vec![100])]));
    }

    #[test]
    fn a_ledger_written_before_the_mac_booted_is_dropped() {
        let dir = std::env::temp_dir().join(format!("as-ledger-boot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("displays-test.json");
        let first = Ledger::persistent_with(file.clone(), identify, None);
        first.made_display(3, "s", false);
        first.started_app(3, 501);
        drop(first);

        let booted_after = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        let after_boot = Ledger::persistent_with(file.clone(), identify, Some(booted_after));
        assert!(!after_boot.ended_with_app(3, "s"));
        assert_eq!(after_boot.orphans("s"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_apps_ledger_outlives_its_run_and_only_one() {
        let dir = std::env::temp_dir().join(format!("as-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("displays-test.json");
        let _ = std::fs::remove_file(&file);

        let first = Ledger::persistent_with(file.clone(), identify, None);
        first.made_display(3, "claude-code:s", false);
        first.started_app(3, 501);
        // The app aborts here: nothing else is written.
        drop(first);

        let second = Ledger::persistent_with(file.clone(), identify, None);
        // A reconnecting pipe's connection is still this run.
        second.connected();
        assert!(second.ended_with_app(3, "claude-code:s"));
        assert!(!second.ended_with_app(3, "codex:t"));
        assert_eq!(second.orphans("claude-code:s"), Some(vec![(3, vec![501])]));
        second.made_display(4, "codex:t", false);
        drop(second);

        // The run before last is gone; only the last run's displays stay.
        let third = Ledger::persistent_with(file.clone(), identify, None);
        assert!(!third.ended_with_app(3, "claude-code:s"));
        assert!(third.ended_with_app(4, "codex:t"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
