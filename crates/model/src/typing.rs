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
/// Only content counts: letters, digits, emoji and other symbols (see
/// `content`). A field may add, drop or change everything else on its
/// own, as a mask's separators, a smart quote or a non-breaking space.
/// Case and accents don't count either (see `letters`). With `t`, `b` and `n` the content of `typed`, `before` and
/// `now`, the rule is:
///
/// - The change starts at `d`, where `n` first differs from `b`, or
///   earlier when what it added repeats what comes before it. It ends at
///   `e`, before the longest common suffix that leaves `b[..d]` whole.
/// - `t` lands when it is one run in `n`, from `k`, that reaches the
///   change: it ends at `d` or after, and starts before `e`, or at `d`
///   when the change only deleted.
///
/// The run may start before `d`: typed text that replaced a selection can
/// repeat what it replaced, as "1235" typed over all of "12/34" shows
/// "12/35", or "w" typed over "world" leaves only a deletion. It may
/// start after `d`, as a mask adds "+1" in front. Anything after it is a
/// completion. A dropped key breaks the run, unless the field held the
/// same character right beside the change, and a run in text the field
/// held before ends before `d`. When `b` and `n` differ only in case,
/// the change is found from them unfolded. A call of formatting alone,
/// as a space, lands when the field changed and its content didn't.
///
/// Not told apart: "12" typed after "Apt 1" with only "2" shown reads as
/// the "1" selected and typed over. All of "Wonder" selected, "Wo" typed
/// and completed to "Wonderland" reads as dropped, as the completion
/// begins with what the field held. A dropped space or separator inside
/// a call with content goes unseen.
pub fn landed_in(typed: &str, before: &str, now: &str) -> bool {
    if before == now || typed.is_empty() {
        return typed.is_empty();
    }
    let (t, b, n) = (
        letters(typed, true),
        letters(before, true),
        letters(now, true),
    );
    if t.is_empty() {
        return b == n;
    }
    if b != n {
        return runs_reach(&t, &n, &changes(&b, &n));
    }
    // Only case changed: find the change unfolded, and match the run with
    // each character folded alone, so both line up.
    let (b, n) = (letters(before, false), letters(now, false));
    if b == n {
        return false;
    }
    let folded = |text: &[char]| text.iter().map(|&c| fold_one(c)).collect::<Vec<_>>();
    let t = folded(&letters(typed, false));
    runs_reach(&t, &folded(&n), &changes(&b, &n))
}

/// Whether `t` is a run in `n` that reaches one of `changes`.
fn runs_reach(t: &[char], n: &[char], changes: &[(usize, usize)]) -> bool {
    n.windows(t.len()).enumerate().any(|(k, run)| {
        run == t
            && changes
                .iter()
                .any(|&(d, e)| k + t.len() >= d && k < e.max(d + 1))
    })
}

/// The content characters of `text`, without accents, and case folded
/// over the whole text when `fold`: "Straße" and "STRASSE" both give
/// "strasse", and "İ" gives "i".
fn letters(text: &str, fold: bool) -> Vec<char> {
    let text = match fold {
        true => text.to_uppercase().to_lowercase(),
        false => text.to_string(),
    };
    text.chars()
        .filter(|&c| !mark(c))
        .map(unaccented)
        .filter(|&c| content(c))
        .collect()
}

/// Every place the change from `before` to `now` can sit, as where it
/// starts and ends in `now`. It starts where `now` first differs, or
/// earlier when what it added repeats what comes before it, as an "a"
/// added to "aa" can sit anywhere. It ends before the longest common
/// suffix that leaves the text before the start whole in both.
fn changes(before: &[char], now: &[char]) -> Vec<(usize, usize)> {
    let first = before.iter().zip(now).take_while(|(a, b)| a == b).count();
    let suffix = before
        .iter()
        .rev()
        .zip(now.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let earliest = first.min(before.len() - suffix).min(now.len() - suffix);
    (earliest..=first)
        .map(|start| {
            let end = suffix.min(before.len() - start).min(now.len() - start);
            (start, now.len() - end)
        })
        .collect()
}

/// Whether a field keeps `c` as typed: a letter, a digit, or any other
/// character outside ASCII, as an emoji or a symbol such as "⭐", unless
/// it is formatting. Formatting is what a field may add, drop or change
/// on its own: ASCII spaces, punctuation and symbols such as "+" or "$",
/// and outside ASCII whitespace, the Latin-1 signs, the General
/// Punctuation block (dashes, smart quotes, joiners), currency signs,
/// CJK and full-width punctuation, and variation selectors.
fn content(c: char) -> bool {
    c.is_alphanumeric()
        || !(c.is_ascii()
            || c.is_whitespace()
            || matches!(
                c,
                '\u{80}'..='\u{bf}'
                    | '\u{d7}'
                    | '\u{f7}'
                    | '\u{2000}'..='\u{206f}'
                    | '\u{20a0}'..='\u{20cf}'
                    | '\u{3000}'..='\u{303f}'
                    | '\u{fe00}'..='\u{fe0f}'
                    | '\u{feff}'
                    | '\u{ff00}'..='\u{ff65}'
            ))
}

/// Whether `c` is a combining mark, as the accent in "e\u{301}". Marks
/// don't count, so a field that stores "é" as "e" and its accent matches
/// "é" typed, with `unaccented`.
fn mark(c: char) -> bool {
    matches!(
        c,
        '\u{300}'..='\u{36f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{3099}'..='\u{309a}'
            | '\u{fe20}'..='\u{fe2f}'
    )
}

/// The letters U+00C0 to U+017F without their accents, from their
/// canonical decompositions: "é" is "e". Letters without one stay.
const UNACCENTED: &str = "AAAAAAÆCEEEEIIIIÐNOOOOO×ØUUUUYÞßaaaaaaæceeeeiiiiðnooooo÷øuuuuyþyAaAaAaCcCcCcCcDdĐđEeEeEeEeEeGgGgGgGgHhĦħIiIiIiIiIıĲĳJjKkĸLlLlLlĿŀŁłNnNnNnŉŊŋOoOoOoŒœRrRrRrSsSsSsSsTtTtŦŧUuUuUuUuUuUuWwYyYZzZzZzſ";

/// `c` without its accent, for the letters in `UNACCENTED`.
fn unaccented(c: char) -> char {
    (c as usize)
        .checked_sub(0xc0)
        .and_then(|i| UNACCENTED.chars().nth(i))
        .unwrap_or(c)
}

/// `c` in lower case, when that is one character.
fn fold_one(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
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

    /// typed, before, now, whether it landed, and the case.
    const CASES: &[(&str, &str, &str, bool, &str)] = &[
        (
            "a",
            "Wonderland",
            "Wonderland",
            false,
            "the field unchanged",
        ),
        ("A", "Avenue ", "Avenue ", false, "a key dropped outright"),
        ("box", "", "box", true, "empty field"),
        ("box", "", "bx", false, "empty field, a key dropped"),
        ("c", "abd", "abcd", true, "inserted inside"),
        ("a", "aa", "aaa", true, "a repeated character"),
        ("aa", "a", "aaa", true, "repeated characters"),
        ("l", "Hel", "Hell", true, "a repeated letter"),
        ("ab", "abab", "ababab", true, "text repeating the field"),
        ("box ", "bar", "box bar", true, "typed at the start"),
        (" box", "bar", "bar box", true, "typed at the end"),
        ("box ", "x bar", "x box bar", true, "slid over a repeat"),
        ("xyz", "abc", "abcxz", false, "a middle key dropped"),
        ("aab", "ab", "abab", false, "a repeated key dropped"),
        (
            "x",
            "hello world",
            "hello x",
            true,
            "a selection typed over",
        ),
        (
            "earth",
            "hello world",
            "hello earth",
            true,
            "a word typed over",
        ),
        (
            "worse",
            "hello world",
            "hello worse",
            true,
            "a word typed over, same start",
        ),
        ("o", "hello world", "helloorld", true, "\"o w\" typed over"),
        ("1", "555", "(555) 1", true, "a phone mask"),
        (
            "5551234567",
            "",
            "+1 (555) 123-4567",
            true,
            "a mask adds +1",
        ),
        (
            "1235",
            "12/34",
            "12/35",
            true,
            "all of an expiry typed over",
        ),
        ("1235", "12/34", "12/3", false, "an expiry key dropped"),
        (
            "0234",
            "12/34",
            "02/34",
            true,
            "all of an expiry typed over, as (a)",
        ),
        (
            "0234",
            "12/34",
            "02/3",
            false,
            "an expiry key dropped, as (a)",
        ),
        (
            "12",
            "12 Main St, Apt ",
            "12 Main St, Apt 1",
            false,
            "a match far back, as (b)",
        ),
        (
            "12",
            "12 Main St, Apt ",
            "12 Main St, Apt 12",
            true,
            "the number appended",
        ),
        ("Wonderl", "Wonder", "Wonderland", true, "a completion"),
        (
            "Spring",
            "Main St, ",
            "Main St, Springfield",
            true,
            "a completion",
        ),
        (
            "l",
            "Wonder, NY",
            "Wonderland, NY",
            true,
            "a completion inside",
        ),
        (
            "Apartment",
            "Avenue ",
            "Avenue partment",
            false,
            "the \"A\" dropped",
        ),
        (
            "Apartment",
            "Avenue ",
            "Avenue Apartmet",
            false,
            "the \"n\" dropped",
        ),
        (
            "Apartment",
            "Avenue ",
            "Avenue Apartment",
            true,
            "all of it",
        ),
        ("main st", "", "Main St", true, "the field changed case"),
        (
            " s",
            "main",
            "Main s",
            true,
            "the field capitalized what it held",
        ),
        ("cat", "Cat", "cat", true, "case alone changed"),
        ("n", "Wonderla", "Wonderla ", false, "only a space changed"),
        ("你", "ni", "你", true, "IME text replaced"),
        (
            "你好",
            "我nihao",
            "我你好",
            true,
            "IME text replaced after text",
        ),
        ("好", "你", "你", false, "IME key dropped"),
        ("👍", "ok ", "ok 👍", true, "an emoji"),
        ("hi 👍", "", "hi ", false, "an emoji dropped"),
        ("❤\u{fe0f}", "", "❤", true, "a variation selector dropped"),
        (" ", "1234", "1234 ", true, "a space"),
        (" ", "1234", "1234\u{a0}", true, "a non-breaking space"),
        ("'", "it", "it\u{2019}", true, "a smart quote"),
        ("hi ⭐", "", "hi ", false, "a symbol dropped"),
        ("hi ⭐", "", "hi ⭐", true, "a symbol"),
        ("Straße", "", "STRASSE", true, "the field upper-cased \"ß\""),
        ("STRASSE", "", "straße", true, "the field wrote \"ß\""),
        ("Straße", "", "Strae", false, "the \"ß\" dropped"),
        (
            "İstanbul",
            "",
            "istanbul",
            true,
            "Turkish dotted I lower-cased",
        ),
        (
            "istanbul",
            "",
            "İSTANBUL",
            true,
            "Turkish dotted I upper-cased",
        ),
        ("ı", "Kar", "KarI", true, "Turkish dotless i upper-cased"),
        (
            "café",
            "",
            "cafe\u{301}",
            true,
            "typed composed, shown decomposed",
        ),
        (
            "cafe\u{301}",
            "",
            "café",
            true,
            "typed decomposed, shown composed",
        ),
        ("é", "caf", "caf", false, "an accented letter dropped"),
        ("\u{301}", "cafe", "cafe\u{301}", true, "an accent alone"),
        ("ñ", "Espa", "España", true, "an accented letter, completed"),
        ("/", "12", "12/", true, "a separator"),
        (" ", "1234", "12345", false, "a space, and content changed"),
        ("x", "abc", "", false, "the field emptied"),
        ("x", "abc", "ab", false, "the field lost text"),
        (
            "w",
            "hello world",
            "hello w",
            true,
            "\"world\" typed over by its \"w\"",
        ),
        (
            "w",
            "hello world",
            "hello wiki",
            true,
            "typed over, then completed",
        ),
        ("", "a", "ab", true, "nothing typed"),
    ];

    #[test]
    fn every_letter_from_u_c0_to_u_17f_has_its_base() {
        assert_eq!(super::UNACCENTED.chars().count(), 0x180 - 0xc0);
        assert_eq!(super::unaccented('\u{17f}'), '\u{17f}');
        assert_eq!(super::unaccented('ñ'), 'n');
    }

    #[test]
    fn the_cases_land_as_listed() {
        let wrong: Vec<_> = CASES
            .iter()
            .filter(|(typed, before, now, lands, _)| landed_in(typed, before, now) != *lands)
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// The lossy fixture: one character a call, the "A" of "Apartment"
    /// and its "n" dropped.
    #[test]
    fn a_call_that_lost_its_key_shows() {
        const STREET: &str = "1234 Wonderland Avenue Apartment 56 North";
        let dropped = [23, 30];
        let mut field = String::new();
        for (i, c) in STREET.chars().enumerate() {
            let before = field.clone();
            if !dropped.contains(&i) {
                field.push(c);
            }
            let typed = c.to_string();
            assert_eq!(
                landed_in(&typed, &before, &field),
                !dropped.contains(&i),
                "{typed:?} after {before:?}"
            );
        }
        assert_eq!(field, "1234 Wonderland Avenue partmet 56 North");
        assert!(!landed_in(STREET, "", &field));
        assert!(!landed_in(
            "Apartment",
            "1234 Wonderland Avenue ",
            "1234 Wonderland Avenue partmet"
        ));
    }

    /// xorshift64: the same numbers every run.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }

        fn text(&mut self, from: &str, len: usize) -> Vec<char> {
            let from: Vec<char> = from.chars().collect();
            (0..len).map(|_| from[self.below(from.len())]).collect()
        }

        /// `text` with formatting added at random places and the case of
        /// some letters changed, as a mask or a formatter does.
        fn format(&mut self, text: Vec<char>) -> String {
            let mut out = String::new();
            for c in text {
                if self.below(4) == 0 {
                    out.push([' ', '-', '/', '(', '\u{a0}'][self.below(5)]);
                }
                match self.below(5) {
                    0 => out.extend(c.to_uppercase()),
                    _ => out.push(c),
                }
            }
            out
        }
    }

    fn string(chars: &[char]) -> String {
        chars.iter().collect()
    }

    /// Typed text that reached the field lands, wherever it went and
    /// whatever it replaced, with formatting and a completion of new
    /// letters added.
    #[test]
    fn typed_text_that_reached_the_field_lands() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..5000 {
            let len = rng.below(8);
            let before = rng.text("ab1 /", len);
            let i = rng.below(before.len() + 1);
            let j = i + rng.below(before.len() - i + 1);
            let len = 1 + rng.below(4);
            let mut typed = rng.text("ab1 ", len);
            typed.push('b');
            let completion = match rng.below(3) {
                // A completion that begins with what the field held reads
                // as (b): see `landed_in`.
                0 => rng.text("xz", 3),
                _ => vec![],
            };
            let now = [&before[..i], &typed, &completion, &before[j..]].concat();
            let (before, typed) = (string(&before), string(&typed));
            let now = match rng.below(2) {
                0 => rng.format(now),
                _ => string(&now),
            };
            // Text typed over the same text leaves nothing to judge.
            let same = |text: &str| {
                text.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
            };
            if same(&before) == same(&now) {
                continue;
            }
            assert!(
                landed_in(&typed, &before, &now),
                "{typed:?} {before:?} {now:?}"
            );
        }
    }

    /// A dropped key shows when no character the field held can stand in
    /// for it: letters typed, all different, into digits.
    #[test]
    fn a_dropped_key_with_nothing_to_stand_in_shows() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..5000 {
            let len = rng.below(8);
            let before = rng.text("0123 -/", len);
            let i = rng.below(before.len() + 1);
            let j = i + rng.below(before.len() - i + 1);
            let mut letters: Vec<char> = "abcdefg".chars().collect();
            let len = 1 + rng.below(letters.len());
            let typed: Vec<char> = (0..len)
                .map(|_| letters.remove(rng.below(letters.len())))
                .collect();
            let mut shown = typed.clone();
            shown.remove(rng.below(shown.len()));
            let completion = match rng.below(3) {
                0 => rng.text("0123", 2),
                _ => vec![],
            };
            let now = [&before[..i], &shown, &completion, &before[j..]].concat();
            let (before, typed) = (string(&before), string(&typed));
            let now = match rng.below(2) {
                0 => rng.format(now),
                _ => string(&now),
            };
            assert!(
                !landed_in(&typed, &before, &now),
                "{typed:?} {before:?} {now:?}"
            );
        }
    }

    /// Typed text the field held before, farther back than the change,
    /// doesn't count: as (b), appending "12" to "12 Main St" and seeing
    /// only "1".
    #[test]
    fn typed_text_held_before_does_not_count() {
        let mut rng = Rng(0xdead_beef_cafe_f00d);
        for _ in 0..5000 {
            let mut letters: Vec<char> = "abcdefg".chars().collect();
            let len = 2 + rng.below(letters.len() - 1);
            let typed: Vec<char> = (0..len)
                .map(|_| letters.remove(rng.below(letters.len())))
                .collect();
            let (a, b) = (rng.below(4), 1 + rng.below(4));
            let (head, tail) = (rng.text("0123 ", a), rng.text("0123", b));
            let before = [&head[..], &typed, &tail].concat();
            let shown = match rng.below(2) {
                0 => &typed[..typed.len() - 1 - rng.below(typed.len() - 1)],
                _ => &typed[1 + rng.below(typed.len() - 1)..],
            };
            let now = [&before[..], shown].concat();
            let (before, typed) = (string(&before), string(&typed));
            let now = match rng.below(2) {
                0 => rng.format(now),
                _ => string(&now),
            };
            assert!(
                !landed_in(&typed, &before, &now),
                "{typed:?} {before:?} {now:?}"
            );
        }
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
