//! The claim file: tells AeroSpace which displays Agent Screen made and which
//! windows on them are the agents', so it leaves them alone (the fork's
//! AGENT-SCREEN.md, Claims). A window is claimed before it moves onto a display
//! and released before it moves off, so AeroSpace never lays it out between.
//! Without the file, AeroSpace guesses from where windows are.

use agent_screen_model::claims::Claims;
use std::sync::Mutex;

static CLAIMS: Mutex<Claims> = Mutex::new(Claims::new());

fn path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/AeroSpace/claims")
        .join(format!("{}.json", std::process::id()))
}

/// Apply a change and write the file if it changed. AeroSpace reads the
/// directory before each command, so the file is current once this returns.
fn change(apply: impl FnOnce(&mut Claims) -> bool) {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if apply(&mut claims) {
        write(&claims);
    }
}

fn write(claims: &Claims) {
    // Debug builds can leave AeroSpace guessing, for a test to show what
    // the claim prevents.
    if cfg!(debug_assertions) && std::env::var("AGENTSCREEN_NO_CLAIMS").as_deref() == Ok("1") {
        return;
    }
    let path = path();
    if claims.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    // Renamed over the claim file, so AeroSpace never reads half of one.
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            let temporary = path.with_extension("json.tmp");
            std::fs::write(&temporary, claims.to_json())?;
            std::fs::rename(&temporary, &path)
        });
    if let Err(error) = written {
        log::warn!("claims: could not write {}: {error}", path.display());
    }
}

pub fn add_display(display: u32) {
    change(|c| c.add_display(display));
}

pub fn remove_display(display: u32) {
    change(|c| c.remove_display(display));
}

/// A window about to move into `bounds`: claimed when they are one of the
/// claimed displays, released otherwise.
pub fn moving_into(window: u32, bounds: crate::model::Rect) {
    change(|c| {
        let display = super::cg::online()
            .into_iter()
            .find(|&d| c.has_display(d) && super::cg::display_bounds(d) == bounds);
        match display {
            Some(display) => c.claim(window, display),
            None => c.release(window),
        }
    });
}

/// A window that stays where it is on one of the claimed displays.
pub fn claim(window: u32, display: u32) {
    change(|c| c.claim(window, display));
}

/// A window about to leave for the user's screen.
pub fn release(window: u32) {
    change(|c| c.release(window));
}

/// On quit: AeroSpace lays out what is left as the user's.
pub fn delete() {
    let _ = std::fs::remove_file(path());
}
