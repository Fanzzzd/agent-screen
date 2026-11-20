//! Bug reports agents file with `report_issue` (ADR 0024): the user's
//! setting, consents and the reports opened, kept in
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

/// What could name the user in a report, for redaction: their full name,
/// and the Mac's names and the home folder. Runs `id` and `scutil`, so call
/// it off the main thread.
pub fn who() -> (Option<String>, Vec<String>) {
    fn run(program: &str, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }
    let name = run("/usr/bin/id", &["-F"]);
    let mut private: Vec<String> = [
        run("/usr/sbin/scutil", &["--get", "ComputerName"]),
        run("/usr/sbin/scutil", &["--get", "LocalHostName"]),
        run("/bin/hostname", &[]),
        std::env::var("HOME").ok(),
    ]
    .into_iter()
    .flatten()
    .collect();
    // The host name without ".local" too.
    let short: Vec<String> = private
        .iter()
        .filter_map(|n| n.strip_suffix(".local").map(String::from))
        .collect();
    private.extend(short);
    (name, private)
}

/// Open a report's prefilled form in the user's browser, in front: only
/// ever on the user's click on the Bug Report Card or the menu. A debug
/// build on the e2e instance with `AGENTSCREEN_REPORT_OPENS` set writes the
/// URL to that file instead, so a test opens no browser.
pub fn open(url: &str) {
    let test =
        cfg!(debug_assertions) && agent_screen_mcp::endpoint::instance().as_deref() == Some("e2e");
    if let Some(file) = std::env::var_os("AGENTSCREEN_REPORT_OPENS").filter(|_| test) {
        use std::io::Write;
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .and_then(|mut f| writeln!(f, "{url}"));
        if let Err(error) = written {
            log::warn!("bug reports: test opener: {error}");
        }
        return;
    }
    if let Err(error) = std::process::Command::new("/usr/bin/open").arg(url).spawn() {
        log::warn!("bug reports: open: {error}");
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
