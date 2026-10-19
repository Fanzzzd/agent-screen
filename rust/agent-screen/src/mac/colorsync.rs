//! Keeping ColorSync's display registry short, through the root helper the
//! app runs with sudo (ADR 0009, `agent_screen_colorsync`).
//!
//! Every display change makes macOS reread the registry once per online
//! display, five times over, with a 50 ms limit before it retries every 5 s.
//! The helper removes records of displays that aren't connected and keeps
//! names in the Mac's languages. Here: whether it is set up, setting it up
//! with the user's administrator password, running it, and the identities of
//! Sidescreen's serials it keeps.

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::uuid::{CFUUID, CFUUIDRef};
use agent_screen_colorsync::{INSTALLED, VERSION, is_plain_user, sudoers_line, sudoers_path};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;

/// Serial slots whose identities reclaim keeps, per instance: serials are
/// reused lowest first, and a record macOS makes anew has all 39 names.
pub const KEPT_SLOTS: u32 = 16;

#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
    fn ColorSyncIterateDeviceProfiles(
        callback: extern "C" fn(core_foundation::dictionary::CFDictionaryRef, *mut c_void) -> bool,
        user_info: *mut c_void,
    );
}

extern "C" fn collect(
    info: core_foundation::dictionary::CFDictionaryRef,
    ids: *mut c_void,
) -> bool {
    use core_foundation::base::CFType;
    use core_foundation::dictionary::CFDictionary;
    let ids = unsafe { &mut *(ids as *mut BTreeSet<String>) };
    let info: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(info) };
    let class = info
        .find(CFString::new("DeviceClass"))
        .and_then(|v| v.downcast::<CFString>())
        .map(|v| v.to_string());
    if class.as_deref() == Some("mntr")
        && let Some(id) = info
            .find(CFString::new("DeviceID"))
            .and_then(|v| v.downcast::<CFUUID>())
    {
        let text = unsafe {
            CFString::wrap_under_create_rule(CFUUIDCreateString(
                std::ptr::null(),
                id.as_concrete_TypeRef(),
            ))
        };
        ids.insert(text.to_string());
    }
    true
}

/// How many displays ColorSync keeps records of, read as the user. Takes up
/// to a second with many records; the helper reports it without that.
pub fn count_records() -> usize {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    unsafe { ColorSyncIterateDeviceProfiles(collect, &mut ids as *mut _ as *mut c_void) };
    ids.len()
}

unsafe extern "C" {
    fn CFUUIDCreateString(allocator: *const c_void, uuid: CFUUIDRef) -> CFStringRef;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Helper {
    /// Set up, this version, and sudo lets this user run it.
    Ready,
    /// Set up with another version: setting it up again replaces it.
    Outdated,
    Missing,
}

/// Whether the helper is set up: sudo runs it without a password, and it
/// is this version. `sudo -l` won't do: it passes for an administrator whom
/// sudo would still ask for the password.
pub fn helper() -> Helper {
    let Ok(meta) = std::fs::symlink_metadata(INSTALLED) else {
        return Helper::Missing;
    };
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 || !meta.is_file() {
        return Helper::Missing;
    }
    let Ok(output) = Command::new("/usr/bin/sudo")
        .args(["-n", INSTALLED, "--version"])
        .env_clear()
        .output()
    else {
        return Helper::Missing;
    };
    if !output.status.success() {
        return Helper::Missing;
    }
    if String::from_utf8_lossy(&output.stdout).trim() == VERSION {
        Helper::Ready
    } else {
        Helper::Outdated
    }
}

/// This user's short name, from the system rather than the environment.
fn user() -> Option<String> {
    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    if entry.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).pw_name) };
    Some(name.to_str().ok()?.to_string())
}

/// `text` as one shell word.
fn shell_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `text` inside an AppleScript string.
fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', r"\\").replace('"', "\\\""))
}

/// Run `script` as root, after macOS's administrator prompt with `prompt`.
fn as_administrator(script: &str, prompt: &str) -> Result<(), String> {
    let source = format!(
        "do shell script {} with prompt {} with administrator privileges",
        applescript_string(script),
        applescript_string(prompt)
    );
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &source])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // -128: the user chose Cancel.
    Err(if error.contains("-128") {
        "cancelled".into()
    } else {
        error
    })
}

/// The helper in this app's bundle, beside the app's binary.
fn bundled() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let helper = exe.with_file_name("agent-screen-colorsync");
    if helper.is_file() {
        Ok(helper)
    } else {
        Err(format!("{} is missing", helper.display()))
    }
}

/// Set the helper up, or again, with the administrator password: copy it
/// where only root can change it, and let this user run it with sudo. The
/// sudoers file passes `visudo -c` before it goes in place.
pub fn set_up(prompt: &str) -> Result<(), String> {
    let user = user().ok_or("no user name")?;
    if !is_plain_user(&user) {
        return Err(format!("the user name {user} can't go in a sudoers file"));
    }
    let source = bundled()?;
    let script = format!(
        "set -e; umask 077; \
         /usr/bin/install -d -o root -g wheel -m 755 /Library/PrivilegedHelperTools; \
         /usr/bin/install -o root -g wheel -m 755 {source} {helper}.new; \
         /bin/mv -f {helper}.new {helper}; \
         t=$(/usr/bin/mktemp /etc/sudoers.d/.sidescreen.XXXXXX); \
         /usr/bin/printf %s {line} > \"$t\"; \
         /usr/sbin/visudo -cqf \"$t\"; \
         /usr/sbin/chown root:wheel \"$t\"; /bin/chmod 440 \"$t\"; \
         /bin/mv -f \"$t\" {rule}",
        source = shell_word(&source.to_string_lossy()),
        helper = shell_word(INSTALLED),
        line = shell_word(&sudoers_line(&user)),
        rule = shell_word(&sudoers_path(&user)),
    );
    as_administrator(&script, prompt)?;
    match helper() {
        Helper::Ready => Ok(()),
        other => Err(format!("set up, but the helper is {other:?}")),
    }
}

/// Remove this user's sudoers file, and the helper when no user's is left,
/// with the password.
pub fn remove(prompt: &str) -> Result<(), String> {
    let user = user().ok_or("no user name")?;
    if !is_plain_user(&user) {
        return Err(format!("the user name {user} can't go in a sudoers file"));
    }
    let script = format!(
        "/bin/rm -f {rule}; \
         /bin/ls {prefix}* >/dev/null 2>&1 || /bin/rm -f {helper}",
        rule = shell_word(&sudoers_path(&user)),
        prefix = agent_screen_colorsync::SUDOERS_PREFIX,
        helper = shell_word(INSTALLED),
    );
    as_administrator(&script, prompt)
}

/// A run takes a second or two; one that hangs is killed, so the next can
/// start.
const RUN_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// `command`'s output, or an error once it has run for `limit`.
fn output_within(
    command: &mut Command,
    limit: std::time::Duration,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => return child.wait_with_output().map_err(|e| e.to_string()),
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the helper ran over {} s", limit.as_secs()));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
}

/// What a reclaim run reported.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Display records left.
    pub records: usize,
    pub line: String,
}

/// Run the helper. Blocking, up to a few seconds.
pub fn reclaim() -> Result<Report, String> {
    let mut args = vec![
        "-n".to_string(),
        INSTALLED.to_string(),
        "reclaim".to_string(),
    ];
    for uuid in kept() {
        args.push("--keep".into());
        args.push(uuid);
    }
    let output = output_within(
        Command::new("/usr/bin/sudo").args(&args).env_clear(),
        RUN_LIMIT,
    )?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let value: serde_json::Value = serde_json::from_str(&line).map_err(|_| {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if error.is_empty() {
            line.clone()
        } else {
            error
        }
    })?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.to_string());
    }
    Ok(Report {
        records: value["records_after"].as_u64().unwrap_or(0) as usize,
        line,
    })
}

/// The display's identity, as ColorSync's registry names it.
pub fn uuid_of(display: u32) -> Option<String> {
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
}

/// Identities of serials, shared by every instance of the app, in
/// `~/Library/Application Support/Sidescreen/identities.json`.
fn identities_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Sidescreen/identities.json"))
}

fn identities() -> BTreeMap<String, String> {
    identities_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Remember display `display`'s identity under its serial, if its slot is
/// one reclaim keeps.
pub fn remember(serial: u32, slot: u32, display: u32) {
    if slot > KEPT_SLOTS {
        return;
    }
    let Some(uuid) = uuid_of(display) else { return };
    let Some(path) = identities_path() else {
        return;
    };
    let mut all = identities();
    if all.get(&serial.to_string()) == Some(&uuid) {
        return;
    }
    all.insert(serial.to_string(), uuid);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("json.new");
    if std::fs::write(&temp, serde_json::to_vec_pretty(&all).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// The identities reclaim keeps.
fn kept() -> BTreeSet<String> {
    identities()
        .into_values()
        .filter(|uuid| agent_screen_colorsync::policy::is_uuid(uuid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_quotes_and_spaces() {
        assert_eq!(shell_word("Sidescreen Dev.app"), "'Sidescreen Dev.app'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(applescript_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }
}
