//! How set_value reaches a control (#79). A pop-up button, an HTML select
//! among them, takes no AXValue, and opening its menu would take the user's
//! keyboard: its item is chosen by type-ahead into the closed control. A
//! combo box takes AXValue, then AXConfirm, so the app commits the text.
//! The route depends on the accessibility role alone: native and Chromium
//! controls with the same role take the same route.

use crate::devtools::Engine;

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

/// Which closed pop-ups Agent Screen types into: an AppKit app's pop-up
/// button and a Chromium page's select, where a typed letter can't open
/// the menu. Another toolkit's pop-up gets no key: an open menu takes the
/// user's keyboard (ADR 0001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    AppKit,
    /// A select in a Chromium app's page, without the DevTools pipe
    /// (`web_select`). Letters search its options while it stays closed;
    /// space and the arrow keys open it.
    ChromiumSelect,
    Other,
}

/// How set_value reaches a select in a web page (ADR 0010).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSelect {
    /// Through the app's private DevTools pipe, with no key.
    Pipe,
    /// Focus the closed select, confirm it took focus, and type the
    /// option's title as type-ahead, with no space (`keys_for`).
    TypeAhead,
    /// Keys can't be posted: the user's window of the app may have focus,
    /// and the keys would reach it.
    WaitForFront,
    /// A Chromium pop-up button that holds no menu: a page's button with
    /// aria-haspopup, which takes keys as the page's shortcuts, not a
    /// select.
    NotSelect,
    Refuse,
}

/// The route to a web page's select. `engine`: the app's page engine.
/// `piped`: the pipe reached the select. `holds_menu`: the pop-up button
/// lists a menu or menu items among its children, as Chromium lists a
/// select's options. `posts`: posted keys reach the agent's window only
/// (`Front::posts`). `shared`: the user may be working in another window
/// of the app (`Front::shared`). Only Chromium is known to search a closed
/// select by its typed letters without opening it, as focus and type chose
/// an option in Dia (#219); a WebKit or unknown page's select gets no key.
pub fn web_select(
    engine: Option<Engine>,
    piped: bool,
    holds_menu: bool,
    posts: bool,
    shared: bool,
) -> WebSelect {
    if engine != Some(Engine::Chromium) {
        WebSelect::Refuse
    } else if piped {
        WebSelect::Pipe
    } else if !holds_menu {
        WebSelect::NotSelect
    } else if posts {
        WebSelect::TypeAhead
    } else if shared {
        WebSelect::WaitForFront
    } else {
        WebSelect::Refuse
    }
}

/// The kind of a pop-up button: in a web page or not, the engine of the
/// app's pages, and whether the app draws with AppKit (no Qt, Java,
/// Flutter or Chromium runtime in its bundle). Any of them unknown, as
/// when an AX call failed, makes it `Other`: no key goes to a control that
/// may open its menu on one.
pub fn popup_kind(
    in_web_page: Option<bool>,
    engine: Option<Engine>,
    cocoa: Option<bool>,
) -> PopupKind {
    match (in_web_page, engine, cocoa) {
        (Some(false), Some(Engine::WebKit), Some(true)) => PopupKind::AppKit,
        _ => PopupKind::Other,
    }
}

/// Why no key goes to a closed pop-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an AppKit app's pop-up button.
    Kind,
    /// A space can press the button and open its menu, so it is never
    /// typed, and an AppKit pop-up lists no items while closed to tell
    /// whether `keys`, the part before the space, alone reaches the title.
    Space { keys: String },
}

/// The keys to type into a closed pop-up of `kind` so it shows `title`,
/// or why none may go. The keys never hold a space.
pub fn keys_for(title: &str, kind: PopupKind) -> Result<String, Refusal> {
    if kind == PopupKind::Other {
        return Err(Refusal::Kind);
    }
    let keys: String = title.chars().take_while(|c| !c.is_whitespace()).collect();
    if keys != title {
        return Err(Refusal::Space { keys });
    }
    Ok(keys)
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
    fn a_miss_says_why_the_title_was_not_reached() {
        assert_eq!(popup_miss("banana", "Banana", "Banana"), PopupMiss::Case);
        assert_eq!(popup_miss("Kiwi", "Apple", "Apple"), PopupMiss::Unchanged);
        assert_eq!(
            popup_miss("Dragon", "Apple", "Dragon fruit"),
            PopupMiss::EarlierItem
        );
        assert_eq!(popup_miss("11", "Apple", "10"), PopupMiss::Other);
    }

    #[test]
    fn only_an_appkit_apps_popups_take_keys() {
        use Engine::*;
        assert_eq!(
            popup_kind(Some(false), Some(WebKit), Some(true)),
            PopupKind::AppKit
        );
        // A web page's select of any engine, Chromium's own controls,
        // Firefox, Qt, Java or Flutter, and anything accessibility or the
        // bundle couldn't tell.
        for (web, engine, cocoa) in [
            (Some(true), Some(Chromium), Some(false)),
            (Some(true), Some(WebKit), Some(true)),
            (Some(false), Some(Chromium), Some(false)),
            (Some(false), Some(Gecko), Some(true)),
            (Some(false), Some(WebKit), Some(false)),
            (Some(false), Some(WebKit), None),
            (Some(false), None, Some(true)),
            (None, Some(WebKit), Some(true)),
        ] {
            assert_eq!(popup_kind(web, engine, cocoa), PopupKind::Other);
        }
        assert_eq!(keys_for("Apple", PopupKind::Other), Err(Refusal::Kind));
    }

    #[test]
    fn a_chromium_select_without_a_pipe_types_ahead_when_keys_reach_it() {
        use Engine::Chromium;
        assert_eq!(
            web_select(Some(Chromium), true, true, false, true),
            WebSelect::Pipe
        );
        assert_eq!(
            web_select(Some(Chromium), true, true, true, false),
            WebSelect::Pipe
        );
        assert_eq!(
            web_select(Some(Chromium), false, true, true, false),
            WebSelect::TypeAhead
        );
    }

    #[test]
    fn a_chromium_select_beside_the_users_window_waits() {
        assert_eq!(
            web_select(Some(Engine::Chromium), false, true, false, true),
            WebSelect::WaitForFront
        );
        // Focus unknown with no window of the user's: keys are refused
        // there as everywhere else.
        assert_eq!(
            web_select(Some(Engine::Chromium), false, true, false, false),
            WebSelect::Refuse
        );
    }

    #[test]
    fn a_webkit_or_unknown_select_is_refused() {
        for engine in [Some(Engine::WebKit), Some(Engine::Gecko), None] {
            for (piped, posts, shared) in [
                (false, true, false),
                (true, true, false),
                (false, false, true),
            ] {
                assert_eq!(
                    web_select(engine, piped, true, posts, shared),
                    WebSelect::Refuse,
                    "{engine:?}"
                );
            }
        }
    }

    #[test]
    fn a_chromium_pop_up_button_without_a_menu_is_no_select_and_gets_no_key() {
        // A page's aria-haspopup button: whatever the front, no key. The
        // pipe tells a select itself, by the element at the point.
        for (posts, shared) in [(true, false), (false, true), (false, false)] {
            assert_eq!(
                web_select(Some(Engine::Chromium), false, false, posts, shared),
                WebSelect::NotSelect
            );
        }
        assert_eq!(
            web_select(Some(Engine::Chromium), true, false, true, false),
            WebSelect::Pipe
        );
    }

    #[test]
    fn a_chromium_select_never_gets_a_space() {
        assert_eq!(
            keys_for("Preview", PopupKind::ChromiumSelect),
            Ok("Preview".into())
        );
        for title in ["Dragon fruit", "United States", "A\u{a0}B"] {
            assert!(matches!(
                keys_for(title, PopupKind::ChromiumSelect),
                Err(Refusal::Space { .. })
            ));
        }
    }

    #[test]
    fn a_space_is_never_typed() {
        assert_eq!(keys_for("Banana", PopupKind::AppKit), Ok("Banana".into()));
        assert_eq!(keys_for("11", PopupKind::AppKit), Ok("11".into()));
        for title in ["Dragon fruit", "United States", "A\u{a0}B"] {
            assert!(matches!(
                keys_for(title, PopupKind::AppKit),
                Err(Refusal::Space { .. })
            ));
        }
    }
}
