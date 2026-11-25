//! Whether "Displays have separate Spaces" is off, read from the window
//! server and the setting without changing either
//! (`agent_screen_model::spaces`). Under a millisecond.

use agent_screen_model::spaces::Facts;

unsafe extern "C" {
    fn SSSpaceManagementMode() -> i32;
    fn SSManagedDisplayIdentifiers(out: *mut std::ffi::c_char, size: i32) -> i32;
    fn SSSpansDisplaysSetting() -> i32;
}

/// What macOS says of its Spaces now. Blocking, briefly.
pub fn facts() -> Facts {
    let mode = unsafe { SSSpaceManagementMode() };
    let mut buffer = vec![0 as std::ffi::c_char; 4096];
    let count = unsafe { SSManagedDisplayIdentifiers(buffer.as_mut_ptr(), buffer.len() as i32) };
    let entries = (count >= 0).then(|| {
        let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
        if count == 0 {
            Vec::new()
        } else {
            text.split('\n').map(String::from).collect()
        }
    });
    let spans = unsafe { SSSpansDisplaysSetting() };
    Facts {
        mode: (mode >= 0).then_some(mode),
        entries,
        spans_displays: (spans >= 0).then_some(spans == 1),
    }
}

/// Whether every display shares one Space now. Blocking, briefly.
pub fn shared() -> bool {
    let facts = facts();
    let shared = facts.shared();
    if shared {
        log::info!("spaces: the displays share one Space: {facts:?}");
    }
    shared
}

fn noticed_path() -> std::path::PathBuf {
    let instance = agent_screen_mcp::endpoint::instance().unwrap_or_else(|| "default".into());
    agent_screen_mcp::endpoint::state_dir().join(format!("spaces-noticed-{instance}"))
}

/// Whether the user has seen the Spaces card on this instance.
pub fn noticed() -> bool {
    noticed_path().exists()
}

/// Remember that the user has seen the Spaces card: it doesn't come back.
pub fn set_noticed() {
    let path = noticed_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(error) = std::fs::write(&path, b"") {
        log::warn!("spaces: {}: {error}", path.display());
    }
}

/// Open System Settings at Desktop & Dock, where Mission Control's
/// "Displays have separate Spaces" is: on the user's click only.
pub fn open_settings() {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.Desktop-Settings.extension")
        .spawn();
}
