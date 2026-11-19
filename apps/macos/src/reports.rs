//! Bug reports agents file with `report_issue` (ADR 0024): the user's
//! setting, consent and the reports opened, kept in
//! `reports-<instance>.json` in the state folder, and the facts the app
//! adds to a report.

use agent_screen_model::report::Reports;
use std::path::PathBuf;

fn path() -> PathBuf {
    let instance = agent_screen_mcp::endpoint::instance().unwrap_or_else(|| "default".into());
    agent_screen_mcp::endpoint::state_dir().join(format!("reports-{instance}.json"))
}

pub fn load() -> Reports {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .map(|value| Reports::from_json(&value))
        .unwrap_or_default()
}

pub fn save(reports: &Reports) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(error) = std::fs::write(&path, reports.to_json().to_string()) {
        log::warn!("bug reports: {}: {error}", path.display());
    }
}

/// The macOS version, as About This Mac shows it: "26.1".
pub fn macos_version() -> String {
    let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    if v.patchVersion > 0 {
        format!("{}.{}.{}", v.majorVersion, v.minorVersion, v.patchVersion)
    } else {
        format!("{}.{}", v.majorVersion, v.minorVersion)
    }
}

/// Open a prefilled form in the user's browser. In the background unless
/// the user just asked for it with a click: an agent's report never brings
/// the browser to the front.
pub fn open(url: &str, front: bool) {
    let mut open = std::process::Command::new("/usr/bin/open");
    if !front {
        open.arg("-g");
    }
    if let Err(error) = open.arg(url).spawn() {
        log::warn!("bug reports: open: {error}");
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
