//! Private recovery custody for a guard helper. Never log event payloads or
//! return them over the agent control socket. Only acknowledged input may be
//! deleted automatically. An incomplete journal requires explicit recovery.

use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

const MAX_EVENT: usize = 64 * 1024;
const MAX_RECORD: usize = 2 * MAX_EVENT;
static CHECKPOINT: AtomicU64 = AtomicU64::new(0);
// Serialize authority validation with canonical-path mutation. Old file handles
// stay open through validation, so an unlinked inode cannot be reused.
static AUTHORITY: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());
static AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

fn owns_authority(
    owners: &BTreeMap<PathBuf, u64>,
    path: &Path,
    authority: u64,
) -> Result<(), String> {
    if owners.get(path) == Some(&authority) {
        Ok(())
    } else {
        Err("input recovery custody has a successor authority".into())
    }
}

fn same_authority(path: &Path, expected: Option<&File>) -> Result<(), String> {
    let current = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let expected = expected
        .map(File::metadata)
        .transpose()
        .map_err(|e| e.to_string())?;
    match (current, expected) {
        (None, None) => Ok(()),
        (Some(current), Some(expected))
            if current.dev() == expected.dev() && current.ino() == expected.ino() =>
        {
            Ok(())
        }
        _ => Err("input recovery custody has a successor authority".into()),
    }
}

fn sync_directory(root: &Path) -> std::io::Result<()> {
    File::open(root).and_then(|directory| directory.sync_all())
}

#[derive(Clone)]
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
    authority: u64,
}

/// A frozen fallback snapshot and its authority across publication retries.
/// A successful rename advances the lease even when directory sync fails.
/// Reserving another lease permanently supersedes this one, even before that
/// lease publishes. Only one fallback owner may acquire a lease per transaction.
pub(super) struct FallbackCheckpoint {
    root: PathBuf,
    nonce: u64,
    target: i32,
    user: i32,
    events: Vec<RetainedRecord>,
    expected: Option<File>,
    authority: u64,
}

impl FallbackCheckpoint {
    pub fn retry(&mut self) -> Result<(), String> {
        self.retry_with_io(|from, to| fs::rename(from, to), sync_directory)
    }

    fn retry_with_io(
        &mut self,
        rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
        sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<(), String> {
        Journal::checkpoint_with_io(
            &self.root,
            self.nonce,
            self.target,
            self.user,
            &self.events,
            &mut self.expected,
            self.authority,
            rename,
            sync_directory,
        )
    }
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
        let mut owners = AUTHORITY.lock().map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("cannot create input recovery custody: {e}"))?;
        let authority = AUTHORITY_ID.fetch_add(1, Ordering::Relaxed);
        owners.insert(path.clone(), authority);
        let mut journal = Self {
            file,
            path,
            authority,
        };
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
        let mut owners = AUTHORITY.lock().map_err(|e| e.to_string())?;
        owns_authority(&owners, &self.path, self.authority)?;
        same_authority(&self.path, Some(&self.file))?;
        fs::remove_file(&self.path).map_err(|e| e.to_string())?;
        owners.remove(&self.path);
        Ok(())
    }

    /// Freeze fallback input and reserve authority once after writer failure or
    /// timeout. Reservation revokes old writer publication/removal immediately.
    /// Retry the returned lease; acquiring a fresh lease could publish an older
    /// snapshot over a successor. Writers must use `checkpoint_owned`.
    pub fn fallback(
        root: &Path,
        nonce: u64,
        target: i32,
        user: i32,
        events: Vec<RetainedRecord>,
    ) -> Result<FallbackCheckpoint, String> {
        let path = canonical(root, nonce);
        let mut owners = AUTHORITY.lock().map_err(|e| e.to_string())?;
        let expected = match File::open(&path) {
            Ok(file) => Some(file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        let authority = AUTHORITY_ID.fetch_add(1, Ordering::Relaxed);
        owners.insert(path, authority);
        Ok(FallbackCheckpoint {
            root: root.to_path_buf(),
            nonce,
            target,
            user,
            events,
            expected,
            authority,
        })
    }

    /// Publish only while this writer still owns the canonical inode.
    pub fn checkpoint_owned(
        self,
        root: &Path,
        nonce: u64,
        target: i32,
        user: i32,
        events: &[RetainedRecord],
    ) -> Result<(), String> {
        Self::checkpoint_with_io(
            root,
            nonce,
            target,
            user,
            events,
            &mut Some(self.file),
            self.authority,
            |from, to| fs::rename(from, to),
            sync_directory,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn checkpoint_with_io(
        root: &Path,
        nonce: u64,
        target: i32,
        user: i32,
        events: &[RetainedRecord],
        expected: &mut Option<File>,
        authority: u64,
        rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
        sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
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
                authority,
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
            {
                let owners = AUTHORITY.lock().map_err(|e| e.to_string())?;
                owns_authority(&owners, &path, authority)?;
                same_authority(&path, expected.as_ref())?;
                rename(&temporary, &path).map_err(|e| e.to_string())?;
                *expected = Some(checkpoint.file);
            }
            sync_directory(root).map_err(|e| e.to_string())?;
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

    fn snapshot() -> Vec<RetainedRecord> {
        vec![RetainedRecord {
            id: 1,
            bytes: b"pending input".to_vec(),
            dispatched: true,
        }]
    }

    fn assert_inventory(root: &Path, nonce: u64) {
        let inventory = summaries(root).unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0]["transaction"], transaction(nonce));
        assert_eq!(inventory[0]["retained"], 1);
        assert_eq!(inventory[0]["deliveryUncertain"], 1);
        assert_eq!(inventory[0]["complete"], false);
        let recovered = inspect(&canonical(root, nonce)).unwrap();
        assert_eq!(
            recovered.events,
            BTreeMap::from([(1, b"pending input".to_vec())])
        );
        assert_eq!(recovered.dispatched, [1]);
        assert!(!recovered.complete);
    }

    #[test]
    fn checkpoint_rename_failure_preserves_old_authority() {
        let root = directory();
        let mut original = Journal::create(&root, 45, 10, 11).unwrap();
        original.held(9, b"original input").unwrap();
        let path = original.path().to_path_buf();
        let before = fs::read(&path).unwrap();
        let mut expected = Some(original.file.try_clone().unwrap());
        let error = Journal::checkpoint_with_io(
            &root,
            45,
            10,
            11,
            &snapshot(),
            &mut expected,
            original.authority,
            |_, _| Err(std::io::Error::other("injected rename failure")),
            |_| panic!("directory sync must not run after rename failure"),
        )
        .unwrap_err();
        assert!(error.contains("injected rename failure"));
        assert_eq!(fs::read(&path).unwrap(), before);
        same_authority(&path, Some(&original.file)).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(summaries(&root).unwrap()[0]["retained"], 1);
        original.acknowledged(9).unwrap();
        assert!(inspect(&path).unwrap().events.is_empty());
        owns_authority(&AUTHORITY.lock().unwrap(), &path, original.authority).unwrap();
        let mut fallback = Journal::fallback(&root, 45, 10, 11, snapshot()).unwrap();
        fallback.retry().unwrap();
        assert_inventory(&root, 45);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkpoint_directory_sync_failure_keeps_new_canonical_and_retry_is_idempotent() {
        let root = directory();
        let original = Journal::create(&root, 46, 10, 11).unwrap();
        let path = original.path().to_path_buf();
        let mut fallback = Journal::fallback(&root, 46, 10, 11, snapshot()).unwrap();
        let error = fallback
            .retry_with_io(
                |from, to| fs::rename(from, to),
                |_| Err(std::io::Error::other("injected directory sync failure")),
            )
            .unwrap_err();
        assert!(error.contains("injected directory sync failure"));
        assert!(same_authority(&path, Some(&original.file)).is_err());
        assert_inventory(&root, 46);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let published = fs::read(&path).unwrap();
        fallback.retry().unwrap();
        assert_eq!(fs::read(&path).unwrap(), published);
        assert_inventory(&root, 46);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timeout_fallback_rejects_stale_ack_then_late_finish_or_checkpoint() {
        for finish in [true, false] {
            let root = directory();
            let mut original = Journal::create(&root, 47, 10, 11).unwrap();
            original.held(1, b"pending input").unwrap();
            original.dispatched(1).unwrap();
            let path = original.path().to_path_buf();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (resume_tx, resume_rx) = std::sync::mpsc::channel();
            let writer_root = root.clone();
            let writer = std::thread::spawn(move || {
                // A channel gate models a writer stalled past the owner's deadline.
                ready_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                original.acknowledged(1).unwrap();
                if finish {
                    original.finish(0, true, true)
                } else {
                    original.checkpoint_owned(&writer_root, 47, 10, 11, &[])
                }
            });
            ready_rx.recv().unwrap();
            let mut fallback = Journal::fallback(&root, 47, 10, 11, snapshot()).unwrap();
            fallback.retry().unwrap();
            let published = fs::read(&path).unwrap();
            let inode = fs::metadata(&path).unwrap().ino();
            resume_tx.send(()).unwrap();
            assert!(
                writer
                    .join()
                    .unwrap()
                    .unwrap_err()
                    .contains("successor authority")
            );
            assert_eq!(fs::read(&path).unwrap(), published);
            assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
            assert_inventory(&root, 47);
            assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn in_progress_checkpoint_cannot_retry_over_timeout_fallback() {
        let root = directory();
        let original = Journal::create(&root, 48, 10, 11).unwrap();
        let (published_tx, published_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let writer_root = root.clone();
        let writer = std::thread::spawn(move || {
            let mut expected = Some(original.file);
            let error = Journal::checkpoint_with_io(
                &writer_root,
                48,
                10,
                11,
                &[],
                &mut expected,
                original.authority,
                |from, to| fs::rename(from, to),
                |_| {
                    // Publication happened, but the old checkpoint has not returned.
                    published_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    Err(std::io::Error::other("late old directory sync failure"))
                },
            )
            .unwrap_err();
            assert!(error.contains("late old directory sync failure"));
            Journal::checkpoint_with_io(
                &writer_root,
                48,
                10,
                11,
                &[],
                &mut expected,
                original.authority,
                |from, to| fs::rename(from, to),
                sync_directory,
            )
        });
        published_rx.recv().unwrap();
        let mut fallback = Journal::fallback(&root, 48, 10, 11, snapshot()).unwrap();
        fallback.retry().unwrap();
        let path = canonical(&root, 48);
        let published = fs::read(&path).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();
        resume_tx.send(()).unwrap();
        assert!(
            writer
                .join()
                .unwrap()
                .unwrap_err()
                .contains("successor authority")
        );
        assert_eq!(fs::read(&path).unwrap(), published);
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_inventory(&root, 48);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fallback_reservation_revokes_writer_before_first_publication() {
        let root = directory();
        let mut original = Journal::create(&root, 50, 10, 11).unwrap();
        original.held(1, b"pending input").unwrap();
        original.dispatched(1).unwrap();
        let path = original.path().to_path_buf();
        let before = fs::read(&path).unwrap();
        let mut fallback = Journal::fallback(&root, 50, 10, 11, snapshot()).unwrap();
        assert!(
            original
                .checkpoint_owned(&root, 50, 10, 11, &[])
                .unwrap_err()
                .contains("successor authority")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        fallback.retry().unwrap();
        assert_inventory(&root, 50);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn superseded_fallback_lease_cannot_refresh_authority_on_retry() {
        let root = directory();
        let _original = Journal::create(&root, 49, 10, 11).unwrap();
        let mut old = Journal::fallback(&root, 49, 10, 11, vec![]).unwrap();
        old.retry().unwrap();
        let mut successor = Journal::fallback(&root, 49, 10, 11, snapshot()).unwrap();
        successor.retry().unwrap();
        let published = fs::read(canonical(&root, 49)).unwrap();
        assert!(old.retry().unwrap_err().contains("successor authority"));
        assert_eq!(fs::read(canonical(&root, 49)).unwrap(), published);
        assert_inventory(&root, 49);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
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
        let mut fallback = Journal::fallback(&root, 44, 10, 11, snapshot.to_vec()).unwrap();
        fallback.retry().unwrap();
        fallback.retry().unwrap();
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
