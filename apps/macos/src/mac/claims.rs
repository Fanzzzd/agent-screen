//! The claim file: tells AeroSpace which displays Agent Screen made and which
//! windows on them are the agents', so it leaves them alone (the fork's
//! AGENT-SCREEN.md, Claims). A window is claimed before it moves onto a display
//! and released once it is off every claimed display, so AeroSpace never lays
//! it out between. A window that doesn't make it off keeps its claim:
//! AeroSpace leaves an unclaimed window its app keeps on an Agent Display
//! where it is, out of the user's sight. Without the file, AeroSpace guesses
//! from where windows are.

use agent_screen_model::claims::{AfterLeaving, Claims, Publication, after_leaving};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CLAIMS: Mutex<Publication> = Mutex::new(Publication::new());

fn path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/AeroSpace/claims")
        .join(format!("{}.json", std::process::id()))
}

/// Preserve the desired state and retry while its file snapshot is dirty.
/// Production callers stay best-effort: publication failures only log.
fn change(apply: impl FnOnce(&mut Claims) -> bool) {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = claims.publish(false, apply, write) {
        log::warn!("claims: could not write {}: {error}", path().display());
    }
}

fn write(claims: &Claims) -> std::io::Result<()> {
    if cfg!(debug_assertions) && std::env::var("AGENTSCREEN_NO_CLAIMS").as_deref() == Ok("1") {
        return Ok(());
    }
    write_at(claims, &path())
}

fn write_at(claims: &Claims, path: &std::path::Path) -> std::io::Result<()> {
    if claims.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, claims.to_json())?;
    std::fs::rename(&temporary, path)
}

pub fn add_display(display: u32) {
    change(|c| c.add_display(display));
}

pub fn remove_display(display: u32) {
    change(|c| c.remove_display(display));
}

fn online() -> Vec<(u32, crate::model::Rect)> {
    super::cg::online()
        .into_iter()
        .map(|d| (d, super::cg::display_bounds(d)))
        .collect()
}

/// A window about to move into `bounds`: claimed for them when they are one
/// of the claimed displays, and true then. Going anywhere else, it keeps its
/// claim until [`release_once_off`] sees it off.
pub fn moving_into(window: u32, bounds: crate::model::Rect) -> bool {
    let online = online();
    let mut onto = false;
    change(|c| {
        onto = c.display_at(bounds, &online).is_some();
        c.moving_into(window, bounds, &online)
    });
    onto
}

/// The display a window is claimed for, if it is claimed.
pub fn display_of(window: u32) -> Option<u32> {
    let claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    claims.desired().display_of(window)
}

/// Whether a window that was to leave the claimed displays is off all of
/// them, by its frame in the window list. After a move that went through, it
/// looks again for up to 0.5 s, as an app or AeroSpace may take a moment to
/// put the window where it was asked. Blocking.
pub fn is_off(window: u32, moved: bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let online = online();
        let claimed = {
            let claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
            claims.desired().claimed_bounds(&online)
        };
        let frame = super::cg::window_bounds(window);
        match after_leaving(moved, frame, &claimed) {
            AfterLeaving::Release => return true,
            AfterLeaving::Keep if !moved || Instant::now() >= deadline => return false,
            AfterLeaving::Keep => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// After a move meant to take a window off the claimed displays: release it
/// once it is off all of them, and say whether it is. A failed move, or a
/// window still on one, keeps its claim. Blocking.
pub fn release_once_off(window: u32, moved: bool) -> bool {
    if !is_off(window, moved) {
        if display_of(window).is_some() {
            log::warn!(
                "claims: window {window} isn't seen off the Agent Displays; it stays claimed"
            );
        }
        return false;
    }
    change(|c| c.release(window));
    true
}

/// A window that stays where it is on one of the claimed displays.
pub fn claim(window: u32, display: u32) {
    change(|c| c.claim(window, display));
}

/// Claim a window that stays on a claimed display, and say whether the
/// claim file now names it there. False when the display isn't claimed or
/// the file couldn't be written.
pub fn claim_confirmed(window: u32, display: u32) -> bool {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = claims.publish(false, |c| c.claim(window, display), write) {
        log::warn!("claims: could not write {}: {error}", path().display());
        return false;
    }
    claims.published().display_of(window) == Some(display)
}

/// A window about to leave for the user's screen through AeroSpace, which
/// only moves a window that isn't claimed. Confirm it left with [`is_off`],
/// and claim it again if it didn't.
pub fn release(window: u32) {
    change(|c| c.release(window));
}

/// Write the claim file again as it is: AeroSpace refreshes when its
/// directory changes, reading the claims first.
pub fn touch() {
    let claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    let published = claims.published().clone();
    if !published.is_empty() {
        let _ = write(&published);
    }
}

/// On quit: AeroSpace lays out what is left as the user's.
pub fn delete() {
    let _ = std::fs::remove_file(path());
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn failed_display_write_recovers_on_claim_with_the_real_publisher() {
        let root = std::env::temp_dir().join(format!(
            "agent-screen-publication-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("claims.json");
        let mut state = Publication::new();
        // A directory at the temporary-file path makes the canonical write
        // fail without changing any user path or permission.
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        assert!(
            state
                .publish(false, |c| c.add_display(3), |c| write_at(c, &path))
                .is_err()
        );
        assert!(state.desired().has_display(3));
        assert!(!state.published().has_display(3));
        assert!(!path.exists());
        std::fs::remove_dir(path.with_extension("json.tmp")).unwrap();
        state
            .publish(false, |c| c.claim(7, 3), |c| write_at(c, &path))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"version":1,"displays":[3],"windows":[7]}"#
        );
        assert_eq!(state.desired(), state.published());
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        let published = state.published().to_json();
        assert!(
            state
                .publish(false, |c| c.release(7), |c| write_at(c, &path))
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), published);
        assert_eq!(state.published().to_json(), published);
        std::fs::remove_dir(path.with_extension("json.tmp")).unwrap();
        state
            .publish(false, |c| c.release(7), |c| write_at(c, &path))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            state.published().to_json()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
