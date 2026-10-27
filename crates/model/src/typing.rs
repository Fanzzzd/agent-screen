//! Whether typed text reached the field (issue #50). A page can drop keys
//! that come faster than its script handles them, and the type action
//! would report success. After typing, every typed character shows in the
//! field's text, in order: the page may add to it, as a phone mask's
//! parentheses or an autocompletion do, or change its case, but a dropped
//! key leaves one out.

/// PID-posted keys in a foreground app must target its current key window.
pub fn keys_reach_target(target_is_front: bool, target_is_key_window: bool) -> bool {
    !target_is_front || target_is_key_window
}

/// Space can activate a closed button before a search starts. AppKit's
/// private-use function-key characters are commands, not item-title text.
pub fn popup_title_is_safe(title: &str) -> bool {
    !title.is_empty()
        && !title.starts_with(char::is_whitespace)
        && !title
            .chars()
            .any(|c| c.is_control() || ('\u{f700}'..='\u{f747}').contains(&c))
}

/// Whether `typed` shows in `shown`, in order, with anything between.
pub fn landed(typed: &str, shown: &str) -> bool {
    let fold = |c: char| {
        let c = if c == '\u{a0}' { ' ' } else { c };
        c.to_lowercase().collect::<String>()
    };
    let mut shown = shown.chars().map(fold);
    typed
        .chars()
        .map(fold)
        .all(|want| shown.any(|have| have == want))
}

#[cfg(test)]
mod tests {
    use super::landed;

    #[test]
    fn popup_titles_cannot_start_activation_or_navigation() {
        for title in ["", " ", " Apple", "Cherry\n", "\u{f701}Cherry"] {
            assert!(!super::popup_title_is_safe(title));
        }
        for title in ["Apple", "+852 (Hong Kong)", "United States", "中文"] {
            assert!(super::popup_title_is_safe(title));
        }
    }

    #[test]
    fn foreground_keys_must_not_reach_another_window() {
        assert!(!super::keys_reach_target(true, false));
        assert!(super::keys_reach_target(true, true));
        assert!(super::keys_reach_target(false, false));
        assert!(super::keys_reach_target(false, true));
    }

    #[test]
    fn all_of_it_landed() {
        assert!(landed("1234 Main Street", "1234 Main Street"));
        assert!(landed("Main", "12 Main"));
    }

    #[test]
    fn a_page_may_add_to_it_or_change_its_case() {
        assert!(landed("5551234567", "(555) 123-4567"));
        assert!(landed("main st", "Main St, Springfield"));
        assert!(landed("a b", "a\u{a0}b"));
    }

    #[test]
    fn a_dropped_key_shows() {
        assert!(!landed("1234 Wonderland Avenue", "1234 Wondnue"));
        assert!(!landed("ab", "ba"));
        assert!(!landed("aa", "a"));
    }
}
