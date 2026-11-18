//! Whether typed text reached the field (issue #50). A page can drop keys
//! that come faster than its script handles them, and the type action
//! would report success. After typing, every typed character shows in the
//! field's text, in order: the page may add to it, as a phone mask's
//! parentheses or an autocompletion do, or change its case, but a dropped
//! key leaves one out.

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
