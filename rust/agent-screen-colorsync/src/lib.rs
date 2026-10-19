//! Keeps ColorSync's display registry short: Sidescreen's root helper and
//! the rules it follows (ADR 0009).

pub mod policy;
#[cfg(target_os = "macos")]
pub mod reclaim;

/// The helper's version, raised when what it does changes, not with each
/// release: the app sets the helper up again, with the user's password, when
/// the installed one reports another.
pub const VERSION: &str = "1";

/// Where setup puts the helper, root's and not writable by the user.
pub const INSTALLED: &str = "/Library/PrivilegedHelperTools/dev.sidescreen.colorsync";

/// The sudoers file that lets `user` run the helper as root. sudo skips
/// files in sudoers.d whose name has a `.`, so the user's dots become `_`.
pub fn sudoers_path(user: &str) -> String {
    format!("{SUDOERS_PREFIX}{}", user.replace('.', "_"))
}

pub const SUDOERS_PREFIX: &str = "/etc/sudoers.d/agent-screen-colorsync-";

/// Whether `user` can go in a sudoers line as it is.
pub fn is_plain_user(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 64
        && !user.starts_with('-')
        && !user.eq_ignore_ascii_case("ALL")
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The one line of that file.
pub fn sudoers_line(user: &str) -> String {
    format!("{user} ALL=(root) NOPASSWD: {INSTALLED}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudoers_names_one_plain_user_and_the_helper() {
        assert!(is_plain_user("fanzhende"));
        assert!(!is_plain_user("a b"));
        assert!(!is_plain_user("ALL"));
        assert!(!is_plain_user("x,y"));
        assert!(!is_plain_user("-x"));
        assert_eq!(
            sudoers_path("first.last"),
            "/etc/sudoers.d/agent-screen-colorsync-first_last"
        );
        assert_eq!(
            sudoers_line("fanzhende"),
            "fanzhende ALL=(root) NOPASSWD: /Library/PrivilegedHelperTools/dev.sidescreen.colorsync\n"
        );
    }
}
