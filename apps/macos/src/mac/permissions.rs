//! Screen Recording and Accessibility: whether Agent Screen has them, asking
//! for them, and the System Settings panes where the user turns them on.
//!
//! macOS shows each permission's prompt only while the app has no entry in
//! the list; after that the only way is the pane. The prompts never come up
//! at launch: the setup window asks when the user clicks. On macOS 26 the
//! Accessibility check at launch already adds an entry, switched off, but
//! nothing adds a Screen Recording entry until the app asks: before that
//! the Screen & System Audio Recording pane has no Agent Screen row.

use super::cg;
use crate::i18n::t;
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use std::ffi::{CString, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    ScreenRecording,
    Accessibility,
}

impl Permission {
    pub const ALL: [Permission; 2] = [Permission::ScreenRecording, Permission::Accessibility];

    /// The name System Settings uses.
    pub fn name(self) -> &'static str {
        match self {
            Permission::ScreenRecording => t("Screen & System Audio Recording", "录屏与系统录音"),
            Permission::Accessibility => t("Accessibility", "辅助功能"),
        }
    }

    /// The name agents read in errors.
    pub fn english(self) -> &'static str {
        match self {
            Permission::ScreenRecording => "Screen Recording",
            Permission::Accessibility => "Accessibility",
        }
    }

    fn service(self) -> &'static str {
        match self {
            Permission::ScreenRecording => "kTCCServiceScreenCapture",
            Permission::Accessibility => "kTCCServiceAccessibility",
        }
    }

    fn anchor(self) -> &'static str {
        match self {
            Permission::ScreenRecording => "Privacy_ScreenCapture",
            Permission::Accessibility => "Privacy_Accessibility",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub screen: bool,
    pub accessibility: bool,
}

impl Status {
    pub fn has(self, p: Permission) -> bool {
        match p {
            Permission::ScreenRecording => self.screen,
            Permission::Accessibility => self.accessibility,
        }
    }

    pub fn missing(self) -> Vec<Permission> {
        Permission::ALL
            .into_iter()
            .filter(|p| !self.has(*p))
            .collect()
    }

    pub fn complete(self) -> bool {
        self.screen && self.accessibility
    }
}

/// Check both now, and remember the answer for `last`.
pub fn status() -> Status {
    let pretend = PRETEND_OFF.load(Ordering::Relaxed);
    let status = Status {
        screen: screen_recording() && pretend & 1 == 0,
        accessibility: super::ax::is_trusted() && pretend & 2 == 0,
    };
    LAST.store(
        status.screen as u8 | (status.accessibility as u8) << 1 | 4,
        Ordering::Relaxed,
    );
    status
}

static LAST: AtomicU8 = AtomicU8::new(0);

/// `debugShow pretend-screen-off` and `pretend-ax-off`: the UI acts as if
/// that permission were off (1 Screen Recording, 2 Accessibility), so
/// scripts can see its states without the user turning one off.
pub static PRETEND_OFF: AtomicU8 = AtomicU8::new(0);

/// The last check's answer, for drawing: checking costs a window list.
pub fn last() -> Status {
    let bits = LAST.load(Ordering::Relaxed);
    if bits == 0 {
        return status();
    }
    Status {
        screen: bits & 1 != 0,
        accessibility: bits & 2 != 0,
    }
}

/// Whether Screen Recording is on now.
///
/// CGPreflightScreenCaptureAccess answers for the whole life of the
/// process, so a grant made while Agent Screen runs only shows another way:
/// WindowServer hands out other apps' window titles only to processes that
/// may record the screen.
pub fn screen_recording() -> bool {
    if CGPreflightScreenCaptureAccess() {
        return true;
    }
    let me = std::process::id() as i32;
    cg::on_screen_windows()
        .iter()
        .any(|w| w.target.pid != me && w.layer == 0 && !w.title.is_empty())
}

/// Set while Screen Recording is on but capture fails until Agent Screen
/// reopens.
pub static NEEDS_REOPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether Screen Recording was on when Agent Screen started. Off then,
/// capture may need a fresh process even after the user turns it on.
pub fn screen_recording_at_launch() -> bool {
    static AT_LAUNCH: OnceLock<bool> = OnceLock::new();
    *AT_LAUNCH.get_or_init(|| CGPreflightScreenCaptureAccess())
}

/// Whether the permission's list in System Settings has an entry for
/// Agent Screen, on or off. None when macOS doesn't say.
pub fn listed(p: Permission) -> Option<bool> {
    type Preflight = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
    static PREFLIGHT: OnceLock<Option<Preflight>> = OnceLock::new();
    let preflight = (*PREFLIGHT.get_or_init(|| unsafe {
        let path = CString::new("/System/Library/PrivateFrameworks/TCC.framework/TCC").ok()?;
        let lib = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY);
        if lib.is_null() {
            return None;
        }
        let name = CString::new("TCCAccessPreflight").ok()?;
        let f = libc::dlsym(lib, name.as_ptr());
        (!f.is_null()).then(|| std::mem::transmute::<*mut c_void, Preflight>(f))
    }))?;
    let service = objc2_core_foundation::CFString::from_str(p.service());
    // 0 granted, 1 denied, 2 no entry yet.
    let answer = unsafe { preflight(&*service as *const _ as *const c_void, std::ptr::null()) };
    Some(answer != 2)
}

/// Ask for the permission: put Agent Screen in the permission's list, then
/// open its pane. Only asking adds the entry: a pane opened before it, as
/// by a user who went to System Settings alone, has no Agent Screen row.
/// macOS shows its prompt, or on macOS 26 opens the pane itself, only the
/// first time; asking again is harmless, so it asks whatever `listed`
/// says, which can be wrong or unknown.
pub fn request(p: Permission) {
    let before = listed(p);
    match p {
        Permission::ScreenRecording => {
            CGRequestScreenCaptureAccess();
        }
        Permission::Accessibility => super::ax::request_trust(),
    }
    log::info!(
        "{p:?}: asked macOS; in the list before: {before:?}, after: {:?}",
        listed(p)
    );
    open_pane(p);
}

pub fn open_pane(p: Permission) {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{}",
            p.anchor()
        ))
        .spawn();
}

/// Start a fresh copy of the app once this one has quit, then quit.
pub fn relaunch() {
    let Some(bundle) = std::env::current_exe().ok().and_then(|exe| {
        exe.ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(|p| p.to_path_buf())
    }) else {
        return;
    };
    let pid = std::process::id();
    // --setup: the new process shows the setup window again, at the step
    // after the permissions.
    let script =
        format!("while kill -0 {pid} 2>/dev/null; do sleep 0.1; done; open \"$0\" --args --setup");
    let _ = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .arg(bundle)
        .spawn();
}
