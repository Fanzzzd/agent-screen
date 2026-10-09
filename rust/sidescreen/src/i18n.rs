//! The language of the app's own words: Simplified Chinese when the user's
//! most preferred macOS language is Chinese, English otherwise.
//! `SIDESCREEN_LANG=en|zh` overrides it.
//!
//! Each string keeps its two forms at the call site:
//! `t("New Display", "新建显示器")`, or with arguments
//! `tf!("Let {agent} Continue", "让 {agent} 继续")`. Words sent to agents
//! and log lines stay English and don't go through here.

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    ZhHans,
}

static LANG: OnceLock<Lang> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_LANG: std::cell::Cell<Option<Lang>> = const { std::cell::Cell::new(None) };
}

/// The app's language, chosen once on first use.
pub fn lang() -> Lang {
    #[cfg(test)]
    if let Some(lang) = TEST_LANG.with(|l| l.get()) {
        return lang;
    }
    *LANG.get_or_init(|| {
        let forced = std::env::var("SIDESCREEN_LANG").ok();
        forced
            .as_deref()
            .and_then(parse)
            .unwrap_or_else(|| choose(&preferred_languages()))
    })
}

/// Tests pin the language for their own thread.
#[cfg(test)]
pub fn set_for_test(lang: Lang) {
    TEST_LANG.with(|l| l.set(Some(lang)));
}

/// `SIDESCREEN_LANG`: `en` or `zh`.
fn parse(value: &str) -> Option<Lang> {
    match value.trim().to_ascii_lowercase().as_str() {
        "en" => Some(Lang::En),
        "zh" | "zh-hans" => Some(Lang::ZhHans),
        _ => None,
    }
}

/// The language for a preferred-languages list, most preferred first:
/// Chinese if that is the first entry, English otherwise.
pub fn choose<S: AsRef<str>>(preferred: &[S]) -> Lang {
    match preferred.first() {
        Some(first) if first.as_ref().to_ascii_lowercase().starts_with("zh") => Lang::ZhHans,
        _ => Lang::En,
    }
}

#[cfg(target_os = "macos")]
fn preferred_languages() -> Vec<String> {
    objc2_foundation::NSLocale::preferredLanguages()
        .iter()
        .map(|l| l.to_string())
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn preferred_languages() -> Vec<String> {
    Vec::new()
}

/// One of the two forms of a fixed string.
pub fn t(en: &'static str, zh: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::ZhHans => zh,
    }
}

/// `format!` in the app's language. Both forms take the same arguments:
/// `tf!("{n} windows", "{n} 个窗口")` captures `n` from scope, and
/// `tf!("{} needs help", "{} 需要帮助", name)` passes it.
macro_rules! tf {
    ($en:literal, $zh:literal $(, $($arg:tt)*)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => format!($en $(, $($arg)*)?),
            $crate::i18n::Lang::ZhHans => format!($zh $(, $($arg)*)?),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_preferred_language_decides() {
        assert_eq!(choose(&["zh-Hans-CN", "en-US"]), Lang::ZhHans);
        assert_eq!(choose(&["zh-Hant-TW"]), Lang::ZhHans);
        assert_eq!(choose(&["en-US", "zh-Hans-CN"]), Lang::En);
        assert_eq!(choose(&["ja-JP", "zh-Hans"]), Lang::En);
        assert_eq!(choose::<&str>(&[]), Lang::En);
    }

    #[test]
    fn the_override_takes_en_or_zh() {
        assert_eq!(parse("en"), Some(Lang::En));
        assert_eq!(parse("ZH"), Some(Lang::ZhHans));
        assert_eq!(parse("fr"), None);
    }

    #[test]
    fn strings_follow_the_language() {
        set_for_test(Lang::En);
        let agent = "Codex";
        assert_eq!(t("Take Over", "接手操作"), "Take Over");
        assert_eq!(
            tf!("Let {agent} Continue", "让 {agent} 继续"),
            "Let Codex Continue"
        );
        set_for_test(Lang::ZhHans);
        assert_eq!(
            tf!("Let {agent} Continue", "让 {agent} 继续"),
            "让 Codex 继续"
        );
    }
}
