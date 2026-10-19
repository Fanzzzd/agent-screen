//! A record of every virtual display Agent Screen adds, keeps, reuses and
//! removes, in `~/Library/Logs/Agent Screen/displays.log` (`displays-NAME.log`
//! for a test instance). Each of these is a display reconfiguration every
//! app on the Mac handles, and while a virtual display is connected each one
//! leaves ColorSync answering more requests for minutes. The file survives
//! restarts, so a slow or frozen Mac can be checked against it.

use crate::model::DisplayId;
use std::io::Write;
use std::path::PathBuf;

/// The file moves to `.old` past this size.
const LIMIT: u64 = 1 << 20;

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let name = match agent_screen_mcp::endpoint::instance() {
        Some(instance) => format!("displays-{instance}.log"),
        None => "displays.log".into(),
    };
    Some(
        PathBuf::from(home)
            .join("Library/Logs/Agent Screen")
            .join(name),
    )
}

/// One line: local time, the event, the display, its unit number, size and
/// position, and `detail`.
pub fn note(event: &str, id: DisplayId, detail: &str) {
    let bounds = crate::mac::cg::display_bounds(id);
    let line = format!(
        "{} {event} display={id} unit={} at={},{} size={}x{} {detail}\n",
        now(),
        crate::mac::cg::unit_number(id),
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
    );
    append(&line);
}

/// One line that isn't about one display: local time and `text`.
/// The folder the log lives in.
pub fn folder() -> Option<PathBuf> {
    path()?.parent().map(PathBuf::from)
}

/// The time as log lines show it.
pub fn stamp() -> String {
    now()
}

pub fn line(text: &str) {
    append(&format!("{} {text}\n", now()));
}

fn append(line: &str) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LIMIT) {
        let _ = std::fs::rename(&path, path.with_extension("old"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Local date and time to the millisecond, as the unified log shows it.
fn now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        now.subsec_millis()
    )
}
