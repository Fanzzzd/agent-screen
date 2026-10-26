//! Private recovery custody for a guard helper. Never log event payloads or
//! return them over the agent control socket. Only acknowledged input may be
//! deleted automatically. An incomplete journal requires explicit recovery.

use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

const MAX_EVENT: usize = 64 * 1024;
const MAX_RECORD: usize = 2 * MAX_EVENT;

pub(super) struct Journal {
    file: File,
    path: PathBuf,
}

/// Payloads stay private to the helper/recovery path. Deliberately no Debug
/// or Serialize implementation: stats and tool replies contain counts only.
pub(super) struct Recovery {
    pub events: BTreeMap<u64, Vec<u8>>,
    pub dispatched: Vec<u64>,
    pub complete: bool,
}

impl Journal {
    pub fn create(root: &Path, nonce: u64, target: i32, user: i32) -> Result<Self, String> {
        match DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(format!(
                    "cannot create the private input recovery directory: {e}"
                ));
            }
        }
        let directory = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
        if !directory.is_dir() || directory.mode() & 0o077 != 0 {
            return Err("the input recovery directory must be a private directory".into());
        }
        let path = root.join(format!("guard-{}-{nonce:016x}.jsonl", std::process::id()));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("cannot create input recovery custody: {e}"))?;
        let mut journal = Self { file, path };
        journal.record(json!({"version":1,"kind":"start","target":target,"user":user}))?;
        Ok(journal)
    }

    fn record(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("input custody write failed: {e}"))
    }

    pub fn held(&mut self, id: u64, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_EVENT {
            return Err("the retained input event exceeds the recovery record limit".into());
        }
        self.record(json!({"kind":"held","id":id,"event":STANDARD.encode(bytes)}))
    }

    pub fn dispatched(&mut self, id: u64) -> Result<(), String> {
        self.record(json!({"kind":"dispatched","id":id}))
    }

    pub fn acknowledged(&mut self, id: u64) -> Result<(), String> {
        self.record(json!({"kind":"ack","id":id}))
    }

    pub fn fault(&mut self, reason: &str) -> Result<(), String> {
        self.record(json!({"kind":"fault","reason":reason}))
    }

    /// The owner must prove no retained or in-flight event remains and the
    /// menu closed. Merely reaching a deadline is never completion.
    pub fn finish(
        mut self,
        retained: usize,
        menu_closed: bool,
        focus_returned: bool,
    ) -> Result<(), String> {
        if retained != 0 || !menu_closed || !focus_returned {
            return Err(
                "input recovery custody cannot be removed before cleanup and acknowledgements"
                    .into(),
            );
        }
        self.record(json!({"kind":"complete"}))?;
        self.file.sync_data().map_err(|e| e.to_string())?;
        fs::remove_file(&self.path).map_err(|e| e.to_string())
    }

    /// Seal retained payloads before a helper may exit without replay.
    /// This preserves custody; it does not assert delivery or delete input.
    pub fn seal_pending(&mut self) -> Result<(), String> {
        self.file.sync_data().map_err(|e| e.to_string())
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }
}

/// Inspection never replays or deletes input. A dispatched-but-unacknowledged
/// event has ambiguous delivery and must not be retried automatically.
pub(super) fn inspect(path: &Path) -> Result<Recovery, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.len() > 64 * 1024 * 1024 {
        return Err("input recovery custody is not a bounded private file".into());
    }
    let mut result = Recovery {
        events: BTreeMap::new(),
        dispatched: Vec::new(),
        complete: false,
    };
    let file = File::open(path).map_err(|e| e.to_string())?;
    for line in BufReader::new(file).split(b'\n') {
        let line = line.map_err(|e| e.to_string())?;
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_RECORD {
            return Err("input recovery record is too large; custody was preserved".into());
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|_| "input recovery record is incomplete; custody was preserved")?;
        let id = || {
            value["id"]
                .as_u64()
                .ok_or("input recovery record has no event id")
        };
        match value["kind"].as_str() {
            Some("held") => {
                let bytes = STANDARD
                    .decode(
                        value["event"]
                            .as_str()
                            .ok_or("input recovery event is missing")?,
                    )
                    .map_err(|_| "invalid input recovery event")?;
                if bytes.len() > MAX_EVENT {
                    return Err("input recovery event is too large".into());
                }
                if result.events.insert(id()?, bytes).is_some() {
                    return Err("duplicate input recovery event id".into());
                }
            }
            Some("dispatched") => {
                let id = id()?;
                if !result.events.contains_key(&id) {
                    return Err("unknown input recovery dispatch".into());
                }
                result.dispatched.push(id);
            }
            Some("ack") => {
                let id = id()?;
                if result.events.remove(&id).is_none() {
                    return Err("unknown input recovery acknowledgement".into());
                }
                result.dispatched.retain(|pending| *pending != id);
            }
            Some("complete") if result.events.is_empty() => result.complete = true,
            Some("start" | "fault") => {}
            _ => return Err("invalid input recovery record; custody was preserved".into()),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agent-screen-guard-journal-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn panic_and_owner_drop_preserve_unacknowledged_custody() {
        let root = directory();
        let mut journal = Journal::create(&root, 1, 10, 11).unwrap();
        let path = journal.path().to_path_buf();
        journal.held(1, b"opaque fake event").unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            journal.dispatched(1).unwrap();
            panic!("owner fault");
        }));
        assert!(panic.is_err());
        drop(journal);
        let recovered = inspect(&path).unwrap();
        assert_eq!(recovered.events.len(), 1);
        assert_eq!(recovered.dispatched, [1]);
        assert!(!recovered.complete);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_cleanup_cannot_remove_a_journal() {
        let root = directory();
        let mut journal = Journal::create(&root, 2, 10, 11).unwrap();
        let path = journal.path().to_path_buf();
        journal.held(1, b"fake").unwrap();
        assert!(journal.finish(1, true, true).is_err());
        assert!(path.exists());
        assert_eq!(inspect(&path).unwrap().events.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_acknowledged_events_can_finish() {
        let root = directory();
        let mut journal = Journal::create(&root, 3, 10, 11).unwrap();
        let path = journal.path().to_path_buf();
        journal.held(1, b"fake").unwrap();
        journal.acknowledged(1).unwrap();
        journal.finish(0, true, true).unwrap();
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
