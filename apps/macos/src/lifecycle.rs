//! A record of every virtual display Agent Screen adds, keeps, reuses and
//! removes, in `~/Library/Logs/Agent Screen/displays.log` (`displays-NAME.log`
//! for a test instance). Each of these is a display reconfiguration every
//! app on the Mac handles, and while a virtual display is connected each one
//! leaves ColorSync answering more requests for minutes. The file survives
//! restarts, so a slow or frozen Mac can be checked against it.

use crate::model::{DisplayId, EndReason};
use agent_screen_model::ended::{self, Ending};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// The file moves to `.old` past this size.
const LIMIT: u64 = 1 << 20;

/// The log target whose lines go to this file too, in every build. The app
/// logs only warnings otherwise in a release build, which dropped the
/// reason a display ended (#326). Keep it to one line per event.
pub const TARGET: &str = "AgentScreen::displays";

/// How many ends `get_diagnostics` lists.
pub const DIAGNOSED_ENDS: usize = 8;

/// The last ends, oldest first, for `get_diagnostics`, with session tags
/// in place of the keys. Every end counts, also a spare's under its old id.
/// Read off the main thread, as diagnostics must answer while the app
/// hangs.
static ENDS: Mutex<VecDeque<Ending>> = Mutex::new(VecDeque::new());

fn path() -> Option<PathBuf> {
    // Unit tests never write the user's log: the orphan sweep's test once
    // left 32 fake "ended orphaned MCP process" lines in it.
    if cfg!(test) {
        return Some(
            std::env::temp_dir()
                .join(format!("agent-screen-tests-{}", std::process::id()))
                .join("displays.log"),
        );
    }
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

/// Display `id` ended: one line, in this file and the app's log, with the
/// reason and the owner's session tag (#326).
pub fn ended(id: DisplayId, reason: EndReason, session_key: Option<&str>) {
    log::info!(target: TARGET, "ended display={id} {}", ended::log_detail(reason, session_key));
    let mut ends = ENDS.lock().unwrap_or_else(|e| e.into_inner());
    if ends.len() >= DIAGNOSED_ENDS {
        ends.pop_front();
    }
    ends.push_back(Ending {
        id,
        reason,
        session_key: Some(ended::session_tag(session_key)),
    });
}

/// The last ends, newest first: display ids, reasons and session tags only.
pub fn recent_ends() -> serde_json::Value {
    let ends = ENDS.lock().unwrap_or_else(|e| e.into_inner());
    ends.iter()
        .rev()
        .map(|e| {
            serde_json::json!({
                "display": e.id,
                "reason": e.reason.tag(),
                "session": e.session_key,
            })
        })
        .collect()
}

/// The last `max` lines about any of `displays`, oldest first, for a bug
/// report, which redacts them (ADR 0024).
pub fn lines_about(displays: &[DisplayId], max: usize) -> Vec<String> {
    if displays.is_empty() {
        return Vec::new();
    }
    // The last 256 KB: the log grows for as long as the app runs.
    let Some(text) = path().and_then(|p| tail(&p, 256 * 1024)) else {
        return Vec::new();
    };
    let marks: Vec<String> = displays.iter().map(|id| format!("display={id} ")).collect();
    let mut lines: Vec<String> = text
        .lines()
        .rev()
        .filter(|line| marks.iter().any(|m| line.contains(m.as_str())))
        .take(max)
        .map(String::from)
        .collect();
    lines.reverse();
    lines
}

/// The last `max` bytes of a file, from the first whole line in them.
fn tail(path: &std::path::Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(match (start > 0, text.split_once('\n')) {
        (true, Some((_, rest))) => rest.to_string(),
        _ => text,
    })
}

/// The folder the log lives in.
pub fn folder() -> Option<PathBuf> {
    path()?.parent().map(PathBuf::from)
}

/// The time as log lines show it.
pub fn stamp() -> String {
    now()
}

/// One line that isn't about one display: local time and `text`.
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
