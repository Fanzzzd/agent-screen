//! The claim file: tells AeroSpace which displays Agent Screen made and which
//! windows on them are the agents', so it leaves them alone (the fork's
//! AGENT-SCREEN.md, Claims). A window is claimed before it moves onto a display
//! and released before it moves off, so AeroSpace never lays it out between.
//! Without the file, AeroSpace guesses from where windows are.

use agent_screen_model::claims::{Claims, Publication};
use std::sync::Mutex;

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

/// The stage gate uses the same publisher and mutex as production moves.
/// Unlike best-effort production calls, it cannot acknowledge a failed write.
#[cfg(debug_assertions)]
pub fn stage_claim(window: u32, display: u32) -> std::io::Result<u64> {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if !claims.desired().has_display(display)
        || std::env::var("AGENTSCREEN_NO_CLAIMS").as_deref() == Ok("1")
    {
        return Err(std::io::Error::other("stage claims are unavailable"));
    }
    claims.publish(true, |c| c.claim(window, display), write)?;
    Ok(claims
        .desired()
        .generation_of(window)
        .expect("published stage claim"))
}

#[cfg(debug_assertions)]
pub fn release_stage(window: u32, display: u32, generation: u64) -> std::io::Result<()> {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    claims.publish(false, |c| c.release_if(window, display, generation), write)
}

#[cfg(debug_assertions)]
pub fn still_stage(window: u32, display: u32, generation: u64) -> bool {
    let claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    claims.desired().display_of(window) == Some(display)
        && claims.desired().generation_of(window) == Some(generation)
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
