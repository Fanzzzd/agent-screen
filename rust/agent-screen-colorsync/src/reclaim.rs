//! Reclaim, as root: unregister the display records `policy::plan` picks,
//! then keep the remaining records' names in the Mac's languages, writing
//! the registry safely beside colorsyncd (ADR 0009).

use crate::policy;
use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::uuid::{CFUUID, CFUUIDRef};
use std::collections::BTreeSet;
use std::ffi::{CString, c_void};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

pub const REGISTRY: &str = "/Library/Caches/ColorSync/com.apple.colorsync.devices";
const LOCK: &str = "/var/run/dev.sidescreen.colorsync.lock";
/// Rewrites tried before a run gives up because colorsyncd keeps writing.
const TRIES: usize = 3;
const UNREGISTER_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long the registry must have been left alone before a rewrite.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn ColorSyncUnregisterDevice(device_class: CFStringRef, device_id: CFUUIDRef) -> bool;
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
    static kColorSyncDisplayDeviceClass: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
}

unsafe extern "C" {
    fn CFUUIDCreateString(allocator: *const c_void, uuid: CFUUIDRef) -> CFStringRef;
    fn CFUUIDCreateFromString(allocator: *const c_void, text: CFStringRef) -> CFUUIDRef;
}

/// What a run did, printed as one JSON line.
#[derive(Default)]
pub struct Report {
    pub records_before: usize,
    pub records_after: usize,
    pub online: usize,
    pub removed: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub wrote: bool,
    /// The rewrite was still there a moment later.
    pub kept: bool,
}

impl Report {
    pub fn json(&self) -> String {
        serde_json::json!({
            "records_before": self.records_before,
            "records_after": self.records_after,
            "online": self.online,
            "removed": self.removed,
            "bytes_before": self.bytes_before,
            "bytes_after": self.bytes_after,
            "wrote": self.wrote,
            "kept": self.wrote.then_some(self.kept),
        })
        .to_string()
    }
}

/// UUIDs of the online displays, upper case.
pub fn online() -> BTreeSet<String> {
    let mut ids = [0u32; 64];
    let mut count = 0u32;
    unsafe { CGGetOnlineDisplayList(64, ids.as_mut_ptr(), &mut count) };
    ids[..count as usize]
        .iter()
        .filter_map(|&display| {
            let id = unsafe { CGDisplayCreateUUIDFromDisplayID(display) };
            if id.is_null() {
                return None;
            }
            let id = unsafe { CFUUID::wrap_under_create_rule(id) };
            let text = unsafe {
                CFString::wrap_under_create_rule(CFUUIDCreateString(
                    std::ptr::null(),
                    id.as_concrete_TypeRef(),
                ))
            };
            Some(text.to_string().to_ascii_uppercase())
        })
        .collect()
}

/// Unregister, giving up after `UNREGISTER_LIMIT`: SimpleDisplay saw the
/// call hang for good in its authorization step. The call then keeps
/// running on its thread until the helper exits.
fn unregister(uuid: &str) -> Result<bool, String> {
    let (done, wait) = std::sync::mpsc::channel();
    let id = uuid.to_string();
    std::thread::spawn(move || {
        let _ = done.send(unregister_now(&id));
    });
    wait.recv_timeout(UNREGISTER_LIMIT).map_err(|_| {
        format!(
            "ColorSyncUnregisterDevice didn't return in {} s",
            UNREGISTER_LIMIT.as_secs()
        )
    })
}

fn unregister_now(uuid: &str) -> bool {
    let text = CFString::new(uuid);
    let id = unsafe { CFUUIDCreateFromString(std::ptr::null(), text.as_concrete_TypeRef()) };
    if id.is_null() {
        return false;
    }
    let id = unsafe { CFUUID::wrap_under_create_rule(id) };
    unsafe { ColorSyncUnregisterDevice(kColorSyncDisplayDeviceClass, id.as_concrete_TypeRef()) }
}

fn open_no_follow(path: &str, flags: i32, mode: u32) -> Result<File, String> {
    let c = CString::new(path).map_err(|e| e.to_string())?;
    let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode) };
    if fd < 0 {
        return Err(format!("{path}: {}", std::io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Hold the run lock until the returned file closes, at exit.
pub fn lock() -> Result<File, String> {
    let file = open_no_follow(LOCK, libc::O_RDWR | libc::O_CREAT, 0o600)?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if meta.uid() != 0 || !meta.is_file() {
        return Err(format!("{LOCK} isn't root's"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!("lock: {}", std::io::Error::last_os_error()));
    }
    Ok(file)
}

/// The registry's folder and file must be root's and writable by no one
/// else, and no link.
fn check_paths() -> Result<(), String> {
    let dir = Path::new(REGISTRY).parent().unwrap();
    for path in [dir, Path::new(REGISTRY)] {
        let meta =
            std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{} is a link", path.display()));
        }
        if meta.uid() != 0 || meta.permissions().mode() & 0o022 != 0 {
            return Err(format!("{} isn't root's alone", path.display()));
        }
    }
    Ok(())
}

/// What identifies one version of the registry file.
#[derive(PartialEq, Eq, Clone, Copy)]
struct Stamp {
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

fn stamp(meta: &std::fs::Metadata) -> Stamp {
    Stamp {
        ino: meta.ino(),
        size: meta.size(),
        mtime: meta.mtime(),
        mtime_nsec: meta.mtime_nsec(),
    }
}

/// Whether the registry was last written long enough ago that colorsyncd
/// is likely done with it.
fn settled(meta: &std::fs::Metadata) -> bool {
    meta.modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age >= SETTLE)
}

/// The registry, its stamp, owner and mode.
fn read() -> Result<(plist::Value, Stamp, u32, u32, u32), String> {
    check_paths()?;
    let mut file = open_no_follow(REGISTRY, libc::O_RDONLY, 0)?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let value =
        plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    Ok((
        value,
        stamp(&meta),
        meta.uid(),
        meta.gid(),
        meta.permissions().mode() & 0o7777,
    ))
}

/// Write `value` over the registry if it is still the version `was`.
/// Ok(false) when colorsyncd wrote it meanwhile.
fn write_over(
    value: &plist::Value,
    was: Stamp,
    uid: u32,
    gid: u32,
    mode: u32,
) -> Result<bool, String> {
    let mut bytes = Vec::new();
    value
        .to_writer_binary(&mut bytes)
        .map_err(|e| e.to_string())?;
    let dir = Path::new(REGISTRY).parent().unwrap();
    let mut name = [0u8; 8];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut name))
        .map_err(|e| e.to_string())?;
    let temp = dir.join(format!(
        ".sidescreen-{}",
        name.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    let temp = temp.to_str().unwrap().to_string();
    let mut file = open_no_follow(&temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
    let result = (|| {
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        if unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0
            || unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0
        {
            return Err(format!("owner: {}", std::io::Error::last_os_error()));
        }
        file.sync_all().map_err(|e| e.to_string())?;
        // colorsyncd may write between this check and the rename; nothing
        // can lock it out. It writes when displays change or someone picks
        // a profile, in bursts, so leave a registry written in the last
        // seconds for a later try.
        let now = std::fs::symlink_metadata(REGISTRY).map_err(|e| e.to_string())?;
        if stamp(&now) != was || !settled(&now) {
            return Ok(false);
        }
        std::fs::rename(&temp, REGISTRY).map_err(|e| e.to_string())?;
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Languages of every user with a home folder in /Users, as "en" and "zh",
/// always with English.
pub fn languages() -> BTreeSet<String> {
    let mut languages: BTreeSet<String> = ["en".to_string()].into();
    let Ok(homes) = std::fs::read_dir("/Users") else {
        return languages;
    };
    for home in homes.flatten() {
        let path = home
            .path()
            .join("Library/Preferences/.GlobalPreferences.plist");
        let Some(path) = path.to_str() else { continue };
        let Ok(mut file) = open_no_follow(path, libc::O_RDONLY, 0) else {
            continue;
        };
        let mut bytes = Vec::new();
        if Read::by_ref(&mut file)
            .take(4 << 20)
            .read_to_end(&mut bytes)
            .is_err()
        {
            continue;
        }
        let Ok(prefs) = plist::Value::from_reader(std::io::Cursor::new(bytes)) else {
            continue;
        };
        let Some(list) = prefs
            .as_dictionary()
            .and_then(|d| d.get("AppleLanguages"))
            .and_then(|l| l.as_array())
        else {
            continue;
        };
        for language in list.iter().filter_map(|l| l.as_string()) {
            let code = policy::language_of(language);
            if (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase()) {
                languages.insert(code.to_string());
            }
        }
    }
    languages
}

/// One run. Needs root and the lock.
pub fn run(keep: &BTreeSet<String>) -> Result<Report, String> {
    let online = online();
    let (registry, _, _, _, _) = read()?;
    let bytes_before = std::fs::metadata(REGISTRY).map(|m| m.len()).unwrap_or(0);
    let plan = policy::plan(&registry, &online, keep).map_err(|e| e.0)?;
    let mut report = Report {
        records_before: plan.records,
        online: online.len(),
        bytes_before,
        ..Report::default()
    };
    for uuid in &plan.remove {
        // A display that came online since keeps its record: look again
        // right before each one.
        if self::online().contains(uuid) {
            continue;
        }
        if unregister(uuid)? {
            report.removed += 1;
        }
    }
    let languages = languages();
    for _ in 0..TRIES {
        // Read again: unregistering rewrote it.
        let (mut registry, was, uid, gid, mode) = read()?;
        report.records_after = policy::plan(&registry, &online, keep)
            .map_err(|e| e.0)?
            .records;
        if !policy::trim_names(&mut registry, &languages) {
            report.bytes_after = std::fs::metadata(REGISTRY).map(|m| m.len()).unwrap_or(0);
            return Ok(report);
        }
        if write_over(&registry, was, uid, gid, mode)? {
            report.wrote = true;
            // colorsyncd keeps the registry in memory too; see that it
            // didn't write its copy back over this one.
            std::thread::sleep(SETTLE);
            let (mut after, _, _, _, _) = read()?;
            report.kept = !policy::trim_names(&mut after, &languages);
            report.bytes_after = std::fs::metadata(REGISTRY).map(|m| m.len()).unwrap_or(0);
            return Ok(report);
        }
        std::thread::sleep(SETTLE);
    }
    Err("colorsyncd kept writing the registry; try again later".into())
}
