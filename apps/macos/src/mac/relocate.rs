//! Move to Applications: copy the running bundle from the disk image,
//! Downloads or its translocated copy into Applications, then open the
//! copy at Settings. Where the bundle is counts as temporary by
//! `agent_screen_model::location`.

use agent_screen_model::location::{self, Location, may_promote, move_plan, same_place};
use objc2_foundation::NSBundle;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The running `.app`, None for a bare binary.
pub fn bundle() -> Option<PathBuf> {
    let path = NSBundle::mainBundle().bundlePath().to_string();
    let path = PathBuf::from(path);
    path.extension().is_some_and(|e| e == "app").then_some(path)
}

/// Where the running bundle is.
pub fn location() -> Location {
    let Some(bundle) = bundle() else {
        return Location::Dev;
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let instance = std::env::var_os("AGENTSCREEN_INSTANCE").is_some_and(|i| !i.is_empty());
    location::classify(&bundle, &home, instance)
}

#[derive(Debug, PartialEq)]
pub enum MoveError {
    /// The destination has an Agent Screen already: ask before replacing.
    Exists(PathBuf),
    Failed(String),
}

/// Copy `source` into `dir` as Agent Screen.app. The copy is made under a
/// hidden name and renamed once complete and verified; an Agent Screen
/// already there is replaced only with `replace`, and only after the new
/// one is in place. The source is never changed: a disk image is
/// read-only, and the user may want the download.
pub fn move_to(source: &Path, dir: &Path, replace: bool) -> Result<PathBuf, MoveError> {
    let plan = move_plan(dir, std::process::id());
    if same_place(source, &plan.dest) {
        return Err(MoveError::Failed(format!(
            "{} is already there",
            source.display()
        )));
    }
    if plan.dest.exists() && !replace {
        return Err(MoveError::Exists(plan.dest));
    }
    let fail = |e: String| MoveError::Failed(e);
    // An attempt of this process that died halfway.
    let _ = std::fs::remove_dir_all(&plan.staging);
    // ditto keeps the code signature, extended attributes and links.
    if let Err(e) = run(
        "/usr/bin/ditto",
        &[source.as_os_str(), plan.staging.as_os_str()],
    ) {
        let _ = std::fs::remove_dir_all(&plan.staging);
        return Err(fail(e));
    }
    // Quarantine travels with the copy and would translocate it again.
    let quarantined = Command::new("/usr/bin/xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(&plan.staging)
        .output()
        .is_ok_and(|o| o.status.success());
    if quarantined {
        let _ = run(
            "/usr/bin/xattr",
            &[
                "-dr".as_ref(),
                "com.apple.quarantine".as_ref(),
                plan.staging.as_os_str(),
            ],
        );
    }
    // Checked on every copy: a damaged download never replaces a working
    // install.
    let had = plan.dest.exists();
    if let Err(why) = may_promote(signed(&plan.staging), had) {
        let _ = std::fs::remove_dir_all(&plan.staging);
        return Err(fail(why.into()));
    }
    if had {
        // One step: the new copy takes the old one's place, and the old one
        // lands at the staging name. A failure leaves the install as it was.
        swap(&plan.staging, &plan.dest).map_err(|e| {
            let _ = std::fs::remove_dir_all(&plan.staging);
            fail(format!("{}: {e}", plan.dest.display()))
        })?;
        if let Err(e) = std::fs::remove_dir_all(&plan.staging) {
            log::warn!("the replaced copy stays at {}: {e}", plan.staging.display());
        }
    } else if let Err(e) = std::fs::rename(&plan.staging, &plan.dest) {
        let _ = std::fs::remove_dir_all(&plan.staging);
        return Err(fail(format!("{}: {e}", plan.dest.display())));
    }
    log::info!(
        "copied {} to {} (quarantine cleared: {quarantined}, replaced: {had})",
        source.display(),
        plan.dest.display()
    );
    Ok(plan.dest)
}

fn signed(bundle: &Path) -> bool {
    Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&out.stderr);
    let error = error.trim();
    Err(if error.is_empty() {
        format!("{program} exited with {}", out.status)
    } else {
        error.to_string()
    })
}

/// Swap two paths on one volume in one step (renamex_np, RENAME_SWAP).
fn swap(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let a = std::ffi::CString::new(a.as_os_str().as_bytes())?;
    let b = std::ffi::CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated paths that live through the call.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
