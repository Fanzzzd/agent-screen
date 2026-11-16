//! Bilingual UI localization (`En` / `ZhHans`) for `apps/linux` and shared GPUI
//! surfaces. `AGENTSCREEN_LANG=en|zh` overrides environment locale variables.

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    ZhHans,
}

static LANG: OnceLock<Lang> = OnceLock::new();

pub fn lang() -> Lang {
    *LANG.get_or_init(|| {
        if let Some(forced) = std::env::var("AGENTSCREEN_LANG")
            .ok()
            .as_deref()
            .and_then(parse)
        {
            return forced;
        }
        let locales: Vec<String> = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|k| std::env::var(k).ok())
            .filter(|v| !v.trim().is_empty())
            .collect();
        choose(&locales)
    })
}

pub fn parse(value: &str) -> Option<Lang> {
    match value.trim().to_ascii_lowercase().as_str() {
        "en" => Some(Lang::En),
        "zh" | "zh-hans" | "zh_cn" => Some(Lang::ZhHans),
        _ => None,
    }
}

pub fn choose<S: AsRef<str>>(preferred: &[S]) -> Lang {
    match preferred.first() {
        Some(first) if first.as_ref().to_ascii_lowercase().starts_with("zh") => Lang::ZhHans,
        _ => Lang::En,
    }
}

pub fn t(en: &'static str, zh: &'static str) -> &'static str {
    match lang() {
        Lang::En => en,
        Lang::ZhHans => zh,
    }
}

#[macro_export]
macro_rules! tf_ui {
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
    fn choose_picks_zh_when_first_locale_starts_with_zh() {
        assert_eq!(choose(&["zh_CN.UTF-8", "en_US.UTF-8"]), Lang::ZhHans);
        assert_eq!(choose(&["en_US.UTF-8", "zh_CN.UTF-8"]), Lang::En);
        assert_eq!(choose::<&str>(&[]), Lang::En);
    }
}
