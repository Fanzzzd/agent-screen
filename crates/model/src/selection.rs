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
}
