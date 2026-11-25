//! Writing a client's config: all at once or not at all, with the file's
//! mode, and through a dotfiles manager's link only to a regular file.

use std::path::{Path, PathBuf};

/// The config's text; None when there is no file yet.
pub fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// The file a write lands in: the path, or the regular file a link there
/// points to.
fn target(path: &Path) -> Result<PathBuf, String> {
    let Ok(meta) = path.symlink_metadata() else {
        return Ok(path.to_path_buf());
    };
    if !meta.file_type().is_symlink() {
        return if meta.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(format!("{} isn't a file", path.display()))
        };
    }
    match std::fs::canonicalize(path) {
        Ok(real) if real.is_file() => Ok(real),
        Ok(real) => Err(format!(
            "{} links to {}, which isn't a file; left alone",
            path.display(),
            real.display()
        )),
        Err(_) => Err(format!(
            "{} is a link to a file that doesn't exist; left alone",
            path.display()
        )),
    }
}

/// Write `text` to `path` through a temporary file in the same folder, so
/// a crash leaves the old file or the new one, never half of one. When the
/// file changed since it read `before`, as a client saving its own config
/// meanwhile, nothing is written and the caller reads it again.
pub fn write(path: &Path, before: Option<&str>, text: &str) -> Result<Written, String> {
    let fail = |e: std::io::Error| format!("could not write {}: {e}", path.display());
    let target = target(path)?;
    let dir = target
        .parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(fail)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        // A new config may hold keys later: only the user reads it.
        std::fs::metadata(&target).map_or(0o600, |m| m.permissions().mode() & 0o7777)
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let temp = dir.join(format!(".{name}.agent-screen-{}.tmp", std::process::id()));
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(fail(e));
    }
    let now = read(&target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    if now.as_deref() != before {
        let _ = std::fs::remove_file(&temp);
        return Ok(Written::Changed);
    }
    std::fs::rename(&temp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        fail(e)
    })?;
    Ok(Written::Done)
}

#[derive(Debug, PartialEq)]
pub enum Written {
    Done,
    /// The file changed after it was read.
    Changed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("agent-screen-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_mode_and_writes_through_a_link_to_a_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir("link");
        let real = dir.join("dotfiles/settings.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(write(&link, Some("{}"), "{\"a\": 1}"), Ok(Written::Done));
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "the link stays"
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\": 1}");
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let leftovers = std::fs::read_dir(real.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file stays");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_link_to_a_folder_or_nowhere() {
        let dir = dir("badlink");
        let link = dir.join("a.json");
        std::os::unix::fs::symlink(dir.join("missing"), &link).unwrap();
        assert!(write(&link, None, "{}").is_err());
        let to_dir = dir.join("b.json");
        std::os::unix::fs::symlink(&dir, &to_dir).unwrap();
        assert!(write(&to_dir, None, "{}").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_nothing_when_the_file_changed_meanwhile() {
        let dir = dir("race");
        let path = dir.join("c.json");
        std::fs::write(&path, "theirs").unwrap();
        assert_eq!(write(&path, Some("mine"), "new"), Ok(Written::Changed));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theirs");
        let new = dir.join("sub/new.json");
        assert_eq!(write(&new, None, "x"), Ok(Written::Done));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&new).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
