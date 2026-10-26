//! Private recovery custody for a guard helper. Never log event payloads or
//! return them over the agent control socket. Only acknowledged input may be
//! deleted automatically. An incomplete journal requires explicit recovery.

use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

const MAX_EVENT: usize = 64 * 1024;
const MAX_RECORD: usize = 2 * MAX_EVENT;
static CHECKPOINT: AtomicU64 = AtomicU64::new(0);

pub(super) struct RetainedRecord {
    pub id: u64,
    pub bytes: Vec<u8>,
    pub dispatched: bool,
}

fn transaction(nonce: u64) -> String {
    format!("guard-{}-{nonce:016x}", std::process::id())
}
fn canonical(root: &Path, nonce: u64) -> PathBuf {
    root.join(format!("{}.jsonl", transaction(nonce)))
}

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
    pub transaction: Option<String>,
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
        let path = canonical(root, nonce);
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("cannot create input recovery custody: {e}"))?;
        let mut journal = Self { file, path };
        journal.record(json!({"version":1,"kind":"start","transaction":transaction(nonce),"target":target,"user":user}))?;
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

    /// Replace the transaction's one authoritative recovery record. A
    /// delayed old writer holds an old inode and cannot append stale ACKs
    /// to this checkpoint. Repeating the same snapshot remains idempotent.
    pub fn checkpoint(
        root: &Path,
        nonce: u64,
        target: i32,
        user: i32,
        events: &[RetainedRecord],
    ) -> Result<(), String> {
        let directory = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
        if !directory.is_dir() || directory.mode() & 0o077 != 0 {
            return Err("checkpoint directory is not private".into());
        }
        let path = canonical(root, nonce);
        let temporary = root.join(format!(
            "{}.checkpoint-{}",
            transaction(nonce),
            CHECKPOINT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> Result<(), String> {
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            let mut checkpoint = Self {
                file,
                path: temporary.clone(),
            };
            checkpoint.record(json!({"version":1,"kind":"start","transaction":transaction(nonce),"target":target,"user":user}))?;
            let mut ids = std::collections::HashSet::new();
            for event in events {
                if !ids.insert(event.id) {
                    return Err("duplicate checkpoint event id".into());
                }
                checkpoint.held(event.id, &event.bytes)?;
                if event.dispatched {
                    checkpoint.dispatched(event.id)?;
                }
            }
            checkpoint.file.sync_data().map_err(|e| e.to_string())?;
            fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
            File::open(root)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
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
        transaction: None,
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
            Some("start") => {
                if result.transaction.is_some() {
                    return Err("duplicate recovery transaction header".into());
                }
                result.transaction = value["transaction"].as_str().map(str::to_owned);
            }
            Some("fault") => {}
            _ => return Err("invalid input recovery record; custody was preserved".into()),
        }
    }
    Ok(result)
}

/// Only canonical transaction records appear in the recovery inventory.
/// Checkpoint temporaries are never independent pending-input records.
pub(super) fn summaries(root: &Path) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("guard-") || !name.ends_with(".jsonl") {
            continue;
        }
        match inspect(&entry.path()) {
            Ok(record)=>result.push(json!({"guard":name,"transaction":record.transaction,"retained":record.events.len(),"deliveryUncertain":record.dispatched.len(),"complete":record.complete})),
            Err(error)=>result.push(json!({"guard":name,"error":error})),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    fn directory() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agent-screen-guard-journal-{}-{nonce}-{}",
            std::process::id(),
            TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn failed_finish_and_repeated_checkpoint_have_one_authoritative_inventory() {
        let root = directory();
        let mut original = Journal::create(&root, 44, 10, 11).unwrap();
        let path = original.path().to_path_buf();
        original.held(1, b"already acknowledged").unwrap();
        original.held(2, b"pending").unwrap();
        original.dispatched(1).unwrap();
        original.dispatched(2).unwrap();
        original.acknowledged(1).unwrap();
        assert!(original.finish(1, true, true).is_err());
        let snapshot = [RetainedRecord {
            id: 2,
            bytes: b"pending".to_vec(),
            dispatched: true,
        }];
        Journal::checkpoint(&root, 44, 10, 11, &snapshot).unwrap();
        Journal::checkpoint(&root, 44, 10, 11, &snapshot).unwrap();
        // Even a leftover incomplete temporary is not another transaction.
        fs::write(root.join("guard-incomplete.checkpoint-99"), b"partial").unwrap();
        let inventory = summaries(&root).unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0]["retained"], 1);
        assert_eq!(inventory[0]["deliveryUncertain"], 1);
        assert_eq!(inventory[0]["transaction"], transaction(44));
        let recovered = inspect(&path).unwrap();
        assert_eq!(recovered.events.keys().copied().collect::<Vec<_>>(), [2]);
        assert_eq!(recovered.dispatched, [2]);
        fs::remove_dir_all(root).unwrap();
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
