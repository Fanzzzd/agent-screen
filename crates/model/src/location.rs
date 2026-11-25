//! Where the running app bundle is, and whether it should move first.
//!
//! A copy opened from the disk image, from Downloads, or from the random
//! folder macOS translocates a quarantined app into gets permissions and
//! agent configs for a path that stops existing once the user moves it.
//! Settings asks to move it to Applications before anything else.

use std::path::{Path, PathBuf};

/// Why a bundle's place is temporary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temporary {
    /// On a mounted volume, as the disk image.
    DiskImage,
    Downloads,
    /// macOS runs a quarantined app from a random read-only copy.
    Translocated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// In /Applications or ~/Applications, or a folder under them.
    Installed,
    Temporary(Temporary),
    /// A development build (inside a `target` folder) or a named instance:
    /// never asked to move.
    Dev,
    /// Anywhere else: left alone.
    Elsewhere,
}

impl Location {
    pub fn temporary(self) -> Option<Temporary> {
        match self {
            Location::Temporary(why) => Some(why),
            _ => None,
        }
    }
}

/// Where `bundle`, the running `.app`, is, for a user whose home is
/// `home`. `instance` is whether AGENTSCREEN_INSTANCE names one.
pub fn classify(bundle: &Path, home: &Path, instance: bool) -> Location {
    let parts: Vec<&std::ffi::OsStr> = bundle
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    if instance || parts.iter().any(|p| *p == "target") {
        return Location::Dev;
    }
    if parts.iter().any(|p| *p == "AppTranslocation") {
        return Location::Temporary(Temporary::Translocated);
    }
    if bundle.starts_with("/Volumes") {
        return Location::Temporary(Temporary::DiskImage);
    }
    if !home.as_os_str().is_empty() && bundle.starts_with(home.join("Downloads")) {
        return Location::Temporary(Temporary::Downloads);
    }
    if bundle.starts_with("/Applications")
        || (!home.as_os_str().is_empty() && bundle.starts_with(home.join("Applications")))
    {
        return Location::Installed;
    }
    Location::Elsewhere
}

/// The name the moved copy takes in Applications.
pub const BUNDLE_NAME: &str = "Agent Screen.app";

/// The paths a move into `dir` uses: the copy goes to `staging` first and
/// takes `dest`'s place only once complete and verified, so a failed copy
/// never leaves a half app where Finder and macOS look. An existing
/// `dest` is swapped with it in one step, and ends up at `staging`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovePlan {
    pub dest: PathBuf,
    pub staging: PathBuf,
}

/// Whether a staged copy may take `dest`'s place: one whose signature
/// verifies always may; one that doesn't never replaces an Agent Screen
/// already there.
pub fn may_promote(staged_verifies: bool, existing: bool) -> Result<(), &'static str> {
    if staged_verifies || !existing {
        Ok(())
    } else {
        Err(
            "the copy's code signature doesn't verify, so the Agent Screen in Applications was left as it is",
        )
    }
}

/// The paths for moving a bundle into `dir`. `tag` keeps two attempts'
/// temporary names apart; the process id does.
pub fn move_plan(dir: &Path, tag: u32) -> MovePlan {
    MovePlan {
        dest: dir.join(BUNDLE_NAME),
        staging: dir.join(format!(".Agent Screen.app.moving-{tag}")),
    }
}

/// A move whose source is its destination, or inside it, would copy the
/// app into itself.
pub fn same_place(source: &Path, dest: &Path) -> bool {
    source.starts_with(dest) || dest.starts_with(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn at(path: &str) -> Location {
        classify(Path::new(path), Path::new(HOME), false)
    }

    #[test]
    fn the_disk_image_downloads_and_translocation_are_temporary() {
        assert_eq!(
            at("/Volumes/Agent Screen/Agent Screen.app"),
            Location::Temporary(Temporary::DiskImage)
        );
        assert_eq!(
            at("/Users/me/Downloads/Agent Screen.app"),
            Location::Temporary(Temporary::Downloads)
        );
        assert_eq!(
            at("/Users/me/Downloads/agent-screen-0.1.43/Agent Screen.app"),
            Location::Temporary(Temporary::Downloads)
        );
        assert_eq!(
            at("/private/var/folders/x1/abc/T/AppTranslocation/0B2A-11/d/Agent Screen.app"),
            Location::Temporary(Temporary::Translocated)
        );
        // Translocation wins over where the original was.
        assert_eq!(
            at("/Volumes/AppTranslocation/Agent Screen.app"),
            Location::Temporary(Temporary::Translocated)
        );
    }

    #[test]
    fn applications_folders_are_installed() {
        assert_eq!(at("/Applications/Agent Screen.app"), Location::Installed);
        assert_eq!(
            at("/Applications/Utilities/Agent Screen.app"),
            Location::Installed
        );
        assert_eq!(
            at("/Users/me/Applications/Agent Screen.app"),
            Location::Installed
        );
    }

    #[test]
    fn names_that_only_look_alike_are_not_matched() {
        // Whole path components only.
        assert_eq!(
            at("/Users/me/Downloads2/Agent Screen.app"),
            Location::Elsewhere
        );
        assert_eq!(at("/ApplicationsOld/Agent Screen.app"), Location::Elsewhere);
        assert_eq!(
            at("/Users/me/Desktop/Agent Screen.app"),
            Location::Elsewhere
        );
        assert_eq!(
            at("/Users/me/Downloads-AppTranslocation-notes/Agent Screen.app"),
            Location::Elsewhere
        );
        // No home: Downloads can't be told.
        assert_eq!(
            classify(
                Path::new("/Users/me/Downloads/Agent Screen.app"),
                Path::new(""),
                false
            ),
            Location::Elsewhere
        );
    }

    #[test]
    fn development_builds_and_instances_are_exempt() {
        assert_eq!(
            at("/Users/me/code/agent-screen/target/debug/Agent Screen Dev.app"),
            Location::Dev
        );
        // Even a build in Downloads.
        assert_eq!(
            at("/Users/me/Downloads/agent-screen/target/release/Agent Screen.app"),
            Location::Dev
        );
        assert_eq!(
            classify(
                Path::new("/Volumes/Agent Screen/Agent Screen.app"),
                Path::new(HOME),
                true
            ),
            Location::Dev
        );
    }

    #[test]
    fn a_move_stages_beside_its_destination() {
        let plan = move_plan(Path::new("/Applications"), 42);
        assert_eq!(plan.dest, Path::new("/Applications/Agent Screen.app"));
        // Beside it, so the swap stays on one volume, and hidden, so
        // Finder and Launchpad don't show a half copy.
        assert_eq!(plan.staging.parent(), Some(Path::new("/Applications")));
        assert!(
            plan.staging
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with('.')
        );
    }

    #[test]
    fn a_copy_that_doesnt_verify_never_replaces_an_install() {
        assert_eq!(may_promote(true, true), Ok(()));
        assert_eq!(may_promote(true, false), Ok(()));
        assert_eq!(may_promote(false, false), Ok(()));
        assert!(may_promote(false, true).is_err());
    }

    #[test]
    fn a_move_into_itself_is_caught() {
        let dest = Path::new("/Applications/Agent Screen.app");
        assert!(same_place(dest, dest));
        assert!(same_place(
            Path::new("/Applications/Agent Screen.app/x"),
            dest
        ));
        assert!(!same_place(
            Path::new("/Volumes/Agent Screen/Agent Screen.app"),
            dest
        ));
    }
}
