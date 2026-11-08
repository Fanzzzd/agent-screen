//! How set_value reaches a control (#79). A pop-up button, an HTML select
//! among them, takes no AXValue, and opening its menu would take the user's
//! keyboard: its item is chosen by type-ahead into the closed control. A
//! combo box takes AXValue, then AXConfirm, so the app commits the text.
//! The route depends on the accessibility role alone: native and Chromium
//! controls with the same role take the same route.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueRoute {
    /// Type the item's title into the closed pop-up button.
    PopupTypeAhead,
    /// Set AXValue, then AXConfirm.
    ComboConfirm,
    /// Set AXValue.
    Attribute,
}

pub fn value_route(role: Option<&str>) -> ValueRoute {
    match role {
        Some("AXPopUpButton") => ValueRoute::PopupTypeAhead,
        Some("AXComboBox") => ValueRoute::ComboConfirm,
        _ => ValueRoute::Attribute,
    }
}

/// Whether `title` can be typed into a closed pop-up button as type-ahead.
/// Space can press a closed button and open its menu before a search
/// starts, control characters such as Return and Tab press or leave it,
/// and AppKit's private-use function-key characters are arrow and page
/// keys, not title text.
pub fn popup_title_is_safe(title: &str) -> bool {
    !title.is_empty()
        && !title.starts_with(char::is_whitespace)
        && !title
            .chars()
            .any(|c| c.is_control() || ('\u{f700}'..='\u{f747}').contains(&c))
}

/// Whether `title` is one character typed more than once, as "11". A web
/// page's select takes a repeated key as "the next item that starts with
/// it", not as the title: type-ahead can't reach such an item there.
pub fn repeats_one_character(title: &str) -> bool {
    let mut chars = title.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let lower = |c: char| c.to_lowercase().collect::<String>();
    let mut rest = chars.peekable();
    rest.peek().is_some() && rest.all(|c| lower(c) == lower(first))
}

/// Why typing `title` into a closed pop-up left it showing `shown`, not
/// the title, when it showed `before` first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupMiss {
    /// Nothing changed: no item starts with the title, or the app ignored
    /// the keys.
    Unchanged,
    /// The item's title differs from the agent's in case only.
    Case,
    /// An item that starts with the title comes before it: type-ahead stops
    /// there, so it can't reach the title.
    EarlierItem,
    /// Another item.
    Other,
}

pub fn popup_miss(title: &str, before: &str, shown: &str) -> PopupMiss {
    let lower = |s: &str| s.to_lowercase();
    if shown.to_lowercase() == title.to_lowercase() {
        PopupMiss::Case
    } else if shown == before {
        PopupMiss::Unchanged
    } else if lower(shown).starts_with(&lower(title)) {
        PopupMiss::EarlierItem
    } else {
        PopupMiss::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_role_picks_the_route() {
        assert_eq!(
            value_route(Some("AXPopUpButton")),
            ValueRoute::PopupTypeAhead
        );
        assert_eq!(value_route(Some("AXComboBox")), ValueRoute::ComboConfirm);
        assert_eq!(value_route(Some("AXTextField")), ValueRoute::Attribute);
        assert_eq!(value_route(None), ValueRoute::Attribute);
    }

    #[test]
    fn a_title_cannot_press_the_button_or_move_its_selection() {
        for title in [
            "",
            " ",
            " Apple",
            "\tApple",
            "Cherry\n",
            "\u{f701}Cherry",
            "A\u{1b}",
        ] {
            assert!(!popup_title_is_safe(title), "{title:?}");
        }
        for title in ["Apple", "+852 (Hong Kong)", "United States", "中文", "1990"] {
            assert!(popup_title_is_safe(title), "{title:?}");
        }
    }

    #[test]
    fn a_repeated_key_is_not_a_title_to_a_web_select() {
        for title in ["11", "aa", "aA", "222"] {
            assert!(repeats_one_character(title), "{title:?}");
        }
        for title in ["1", "10", "Anna", "", "中文"] {
            assert!(!repeats_one_character(title), "{title:?}");
        }
    }

    #[test]
    fn a_miss_says_why_the_title_was_not_reached() {
        assert_eq!(popup_miss("banana", "Banana", "Banana"), PopupMiss::Case);
        assert_eq!(popup_miss("Kiwi", "Apple", "Apple"), PopupMiss::Unchanged);
        assert_eq!(
            popup_miss("Dragon", "Apple", "Dragon fruit"),
            PopupMiss::EarlierItem
        );
        assert_eq!(popup_miss("11", "Apple", "10"), PopupMiss::Other);
    }
}
