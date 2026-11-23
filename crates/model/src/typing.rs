//! Whether typed text reached the field (issue #50). A page can drop keys
//! that come faster than its script handles them, and the type action
//! would report success. After typing, every typed character shows in the
//! field's text, in order: the page may add to it, as a phone mask's
//! parentheses or an autocompletion do, or change its case, but a dropped
//! key leaves one out.

/// The text as typed: a Windows line ending, \r\n, is one Return.
pub fn typed_text(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// A character as compared: case and non-breaking spaces don't count.
fn fold(c: char) -> String {
    let c = if c == '\u{a0}' { ' ' } else { c };
    c.to_lowercase().collect()
}

/// Whether `typed` shows in `shown`, in order, with anything between.
pub fn landed(typed: &str, shown: &str) -> bool {
    let mut shown = shown.chars().map(fold);
    typed
        .chars()
        .map(fold)
        .all(|want| shown.any(|have| have == want))
}

/// Whether `typed` shows in what changed from `before` to `now`. The whole
/// field can't tell: a one-character call finds its character almost
/// anywhere, so the call returned before the page handled the key, and the
/// next key came too soon and was dropped.
///
/// What changed is `now` without as much of `before`'s start and end as it
/// keeps. Where that span sits can be ambiguous: "box " typed before "bar"
/// gives "box bar", which keeps "b" at the start and "bar" at the end. And
/// typed text that replaced a selection can repeat what it replaced: "o"
/// typed over "o w" in "hello world" looks like " w" deleted. So the span
/// is at least as long as the typed text, and every place for it is tried.
/// The span may also reach back over what the field kept at its start
/// when its letters and digits then begin as the typed text's do: "1235"
/// typed over all of "12/34" shows "12/35" in an expiry mask, which keeps
/// "12/3". A field that didn't change took nothing.
pub fn landed_in(typed: &str, before: &str, now: &str) -> bool {
    if before == now {
        return typed.is_empty();
    }
    let before: Vec<char> = before.chars().collect();
    let now: Vec<char> = now.chars().collect();
    let start = before.iter().zip(&now).take_while(|(a, b)| a == b).count();
    let end = before
        .iter()
        .rev()
        .zip(now.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let kept = (start + end).min(before.len()).min(now.len());
    let span = (now.len() - kept).max(typed.chars().count()).min(now.len());
    (0..=start).any(|head| {
        let Some(tail) = (now.len() - span).checked_sub(head) else {
            return false;
        };
        if tail > end || head + tail > before.len() {
            return false;
        }
        let reach = head + span;
        let lands = |from: usize| landed(typed, &now[from..reach].iter().collect::<String>());
        lands(head) || (0..head).any(|from| begins(typed, &now[from..reach]) && lands(from))
    })
}

/// Whether the letters and digits of `typed` and of `shown` begin the
/// same way, as far as the shorter goes, with at least one of each.
fn begins(typed: &str, shown: &[char]) -> bool {
    let typed = typed.chars().filter(|c| c.is_alphanumeric()).map(fold);
    let shown = shown
        .iter()
        .filter(|c| c.is_alphanumeric())
        .map(|&c| fold(c));
    let mut pairs = typed.zip(shown).peekable();
    pairs.peek().is_some() && pairs.all(|(a, b)| a == b)
}

/// What a field read back after a type call says, read every
/// `READ_EVERY_MS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readback {
    /// The typed text shows where the field changed.
    Landed,
    /// The field changed, then held still without the typed text.
    Dropped,
    /// The field never changed: the text may have replaced the same text,
    /// or the app doesn't update what it says. The call can't tell.
    Unconfirmed,
    /// Read again.
    Wait,
}

/// How often the field is read back, in ms.
pub const READ_EVERY_MS: u64 = 25;

/// Reads in a row that show the same text before a changed field is
/// judged: 1.5 s, as a busy page is still taking keys in while its text
/// grows.
const HELD_STILL: u32 = 60;

/// Reads before an unchanged field that already holds the typed text, as
/// a word typed over the same word, counts as unconfirmed: 300 ms.
const SAME_TEXT: u32 = 12;

/// The readback's verdict, with `still` the reads in a row that showed
/// `now`, counting from the first.
pub fn readback(typed: &str, before: &str, now: &str, still: u32) -> Readback {
    if landed_in(typed, before, now) {
        Readback::Landed
    } else if now != before {
        if still >= HELD_STILL {
            Readback::Dropped
        } else {
            Readback::Wait
        }
    } else if still >= HELD_STILL || (still >= SAME_TEXT && landed(typed, now)) {
        Readback::Unconfirmed
    } else {
        Readback::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::{Readback, landed, landed_in, readback, typed_text};

    #[test]
    fn a_windows_line_ending_is_one_return() {
        assert_eq!(typed_text("a\r\nb\nc\rd"), "a\nb\nc\rd");
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

    #[test]
    fn a_key_dropped_outright_changes_nothing() {
        let before = "1234 Wonderland Avenue ";
        assert!(!landed_in("A", before, before));
    }

    #[test]
    fn a_key_counts_only_where_the_field_changed() {
        assert!(landed_in("a", "aa", "aaa"));
        assert!(landed_in("c", "abd", "abcd"));
        assert!(!landed_in("a", "Wonderland", "Wonderland"));
    }

    #[test]
    fn a_selection_replaced_by_the_key_counts() {
        assert!(landed_in("x", "hello world", "hello x"));
    }

    #[test]
    fn a_mask_that_reformats_the_field_counts() {
        assert!(landed_in("1", "555", "(555) 1"));
    }

    #[test]
    fn text_that_starts_like_what_follows_it_counts() {
        assert!(landed_in("box ", "bar", "box bar"));
        assert!(landed_in(" box", "bar", "bar box"));
        assert!(landed_in("ab", "abab", "ababab"));
        // The "y" of "xyz" dropped, and of "aab".
        assert!(!landed_in("xyz", "abc", "abcxz"));
        assert!(!landed_in("aab", "ab", "abab"));
    }

    #[test]
    fn a_selection_typed_over_from_what_the_field_kept_counts() {
        // All of "12/34" selected, "1235" typed, the mask shows "12/35".
        assert!(landed_in("1235", "12/34", "12/35"));
        assert!(!landed_in("1235", "12/34", "12/3"));
        // An autocompletion after it.
        assert!(landed_in("Wonderl", "Wonder", "Wonderland"));
        // The "A" or the second "n" of "Apartment" dropped.
        let before = "1234 Wonderland Avenue ";
        assert!(!landed_in(
            "Apartment",
            before,
            "1234 Wonderland Avenue partment"
        ));
        assert!(!landed_in(
            "Apartment",
            before,
            "1234 Wonderland Avenue Apartmet"
        ));
        assert!(landed_in(
            "Apartment",
            before,
            "1234 Wonderland Avenue Apartment"
        ));
    }

    #[test]
    fn typing_into_an_empty_field_counts() {
        assert!(landed_in("box", "", "box"));
        assert!(!landed_in("box", "", "bx"));
    }

    #[test]
    fn a_selection_typed_over_counts() {
        // "world" selected, "earth" typed.
        assert!(landed_in("earth", "hello world", "hello earth"));
        // "o w" selected in "hello world", "o" typed.
        assert!(landed_in("o", "hello world", "helloorld"));
    }

    #[test]
    fn a_changed_field_is_judged_once_it_holds_still() {
        assert_eq!(readback("a", "x", "xa", 0), Readback::Landed);
        assert_eq!(readback("ab", "x", "xa", 10), Readback::Wait);
        assert_eq!(readback("ab", "x", "xa", 60), Readback::Dropped);
    }

    #[test]
    fn an_unchanged_field_is_unconfirmed_soon_after() {
        // A word typed over the same word, selected: the field holds it.
        assert_eq!(readback("cat", "a cat", "a cat", 5), Readback::Wait);
        assert_eq!(readback("cat", "a cat", "a cat", 12), Readback::Unconfirmed);
        // The key may still be on its way.
        assert_eq!(readback("A", "Avenue ", "Avenue ", 5), Readback::Wait);
        // The key was dropped, or the app doesn't say what it holds.
        assert_eq!(readback("z", "Avenue ", "Avenue ", 12), Readback::Wait);
        assert_eq!(
            readback("z", "Avenue ", "Avenue ", 60),
            Readback::Unconfirmed
        );
    }
}
