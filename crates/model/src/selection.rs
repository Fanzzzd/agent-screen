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

/// Which closed pop-ups Agent Screen types into. Only those where a typed
/// letter can't open the menu: an AppKit pop-up button, and a Chromium
/// page's `<select>`. Anything else, a WebKit page's select or an ARIA
/// button with a pop-up among them, gets no key: an open menu takes the
/// user's keyboard (ADR 0001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    AppKit,
    ChromiumSelect,
    Other,
}

/// The kind of a pop-up button: in a web page or not, the engine of the
/// app's pages, whether the app draws with AppKit (no Qt, Java, Flutter or
/// Chromium runtime in its bundle), and whether it is known to be a
/// `<select>`, as when its options show to accessibility. Any of them
/// unknown, as when an AX call failed, makes it `Other`: no key goes to a
/// control that may open its menu on one.
pub fn popup_kind(
    in_web_page: Option<bool>,
    engine: Option<Engine>,
    cocoa: Option<bool>,
    select: bool,
) -> PopupKind {
    match (in_web_page, engine) {
        (Some(false), Some(Engine::WebKit)) if cocoa == Some(true) => PopupKind::AppKit,
        (Some(true), Some(Engine::Chromium)) if select => PopupKind::ChromiumSelect,
        _ => PopupKind::Other,
    }
}

/// The items type-ahead with `keys` shows on the way to the one it ends
/// on, each a choice the app may act on, when the items are known: for a
/// Chromium select, one per key that moves it.
pub fn passed(kind: PopupKind, keys: &str, current: &str, items: &[String]) -> Vec<String> {
    let mut shown = current.to_string();
    let mut passed = Vec::new();
    let mut typed = String::new();
    for c in keys.chars() {
        typed.push(c);
        if let Some(now) = reached(kind, &typed, current, items)
            && now != shown
        {
            passed.push(now.to_string());
            shown = now.to_string();
        }
    }
    passed.pop();
    passed
}

/// Why no key goes to a closed pop-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an AppKit pop-up button or a Chromium `<select>`.
    Kind,
    /// A space can press the button and open its menu, so it is never
    /// typed; the part before it, `keys`, alone doesn't reach the title
    /// but `reached`, or the items can't be read to tell (None).
    Space {
        keys: String,
        reached: Option<String>,
    },
    /// The control lists no item with the title.
    NotAnItem,
    /// Typing the title would reach another item first.
    Reaches(String),
    /// A web page's select takes a key typed twice at the start as the
    /// next item that starts with it, not as the title's start.
    RepeatedKey,
    /// A web page's select searches one key from the item after the one it
    /// shows, which starts with that key too.
    FromCurrent,
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// The item type-ahead reaches with `keys`, where it shows `current`. An
/// AppKit pop-up takes the first item that starts with the keys. A
/// Chromium select goes key by key as Blink's type-ahead does: while every
/// key so far is the same one, each moves to the next item after the shown
/// one that starts with it ("cycle" mode); otherwise it takes the first
/// item, from the shown one on, that starts with all the keys so far. Both
/// wrap around.
pub fn reached<'a>(
    kind: PopupKind,
    keys: &str,
    current: &str,
    items: &'a [String],
) -> Option<&'a str> {
    match kind {
        PopupKind::AppKit => {
            let keys = lower(keys);
            items
                .iter()
                .find(|i| lower(i).starts_with(&keys))
                .map(String::as_str)
        }
        PopupKind::ChromiumSelect => {
            let n = items.len();
            let mut at = items.iter().position(|i| i == current);
            let mut typed = String::new();
            let mut repeating = None;
            for c in keys.chars().flat_map(char::to_lowercase) {
                typed.push(c);
                repeating = match repeating {
                    _ if typed.chars().count() == 1 => Some(c),
                    Some(r) if r == c => Some(c),
                    _ => None,
                };
                let (prefix, start) = match repeating {
                    Some(c) => (c.to_string(), at.map_or(0, |a| a + 1)),
                    None => (typed.clone(), at.unwrap_or(0)),
                };
                if let Some(found) = (0..n)
                    .map(|k| (start + k) % n)
                    .find(|&i| lower(&items[i]).starts_with(&prefix))
                {
                    at = Some(found);
                }
            }
            at.map(|i| items[i].as_str())
        }
        PopupKind::Other => None,
    }
}

/// Whether Blink would take `keys` in cycle mode for their first two
/// keys: the same key twice, as in "11" or "1100".
fn cycles(keys: &str) -> bool {
    let mut chars = keys.chars().flat_map(char::to_lowercase);
    matches!((chars.next(), chars.next()), (Some(a), Some(b)) if a == b)
}

/// The keys to type into a closed pop-up of `kind` that shows `current`
/// so it shows `title`, or why none may go. The keys never hold a space:
/// for a title with one they are its start, typed only when exactly one
/// of the `items`, read without opening the menu, starts with them, and it
/// is the title. `items` is None when the control doesn't list them.
pub fn keys_for(
    title: &str,
    kind: PopupKind,
    current: &str,
    items: Option<&[String]>,
) -> Result<String, Refusal> {
    if kind == PopupKind::Other {
        return Err(Refusal::Kind);
    }
    let keys: String = title.chars().take_while(|c| !c.is_whitespace()).collect();
    if kind == PopupKind::ChromiumSelect {
        if cycles(&keys) {
            return Err(Refusal::RepeatedKey);
        }
        if keys.chars().count() == 1 && lower(current).starts_with(&lower(&keys)) {
            return Err(Refusal::FromCurrent);
        }
    }
    if keys != title {
        let Some(items) = items else {
            return Err(Refusal::Space {
                keys,
                reached: None,
            });
        };
        let starting: Vec<_> = items
            .iter()
            .filter(|i| lower(i).starts_with(&lower(&keys)))
            .collect();
        if starting.is_empty() {
            return Err(Refusal::NotAnItem);
        }
        if starting.len() != 1 || starting[0] != title {
            let reached = reached(kind, &keys, current, items).map(String::from);
            return Err(Refusal::Space { keys, reached });
        }
        return Ok(keys);
    }
    if let Some(items) = items {
        if !items.iter().any(|i| i == title) {
            return Err(Refusal::NotAnItem);
        }
        match reached(kind, &keys, current, items) {
            Some(item) if item == title => {}
            Some(item) => return Err(Refusal::Reaches(item.to_string())),
            None => return Err(Refusal::NotAnItem),
        }
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

    fn items(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_appkit_popups_and_chromium_selects_take_keys() {
        use Engine::*;
        assert_eq!(
            popup_kind(Some(false), Some(WebKit), Some(true), false),
            PopupKind::AppKit
        );
        assert_eq!(
            popup_kind(Some(true), Some(Chromium), Some(true), true),
            PopupKind::ChromiumSelect
        );
        // An ARIA button with a pop-up, Safari's select, Chromium's own UI,
        // Firefox, an app with no bundle.
        assert_eq!(
            popup_kind(Some(true), Some(Chromium), Some(true), false),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(Some(true), Some(WebKit), Some(true), true),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(Some(false), Some(Chromium), Some(true), true),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(Some(true), Some(Gecko), Some(true), true),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(Some(false), Some(Gecko), Some(true), false),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(Some(false), None, Some(true), false),
            PopupKind::Other
        );
        // Qt, Java or Flutter, or a bundle that can't be found.
        assert_eq!(
            popup_kind(Some(false), Some(WebKit), Some(false), false),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(Some(false), Some(WebKit), None, false),
            PopupKind::Other
        );
        // Accessibility couldn't say whether it is in a web page.
        assert_eq!(
            popup_kind(None, Some(WebKit), Some(true), false),
            PopupKind::Other
        );
        assert_eq!(
            popup_kind(None, Some(Chromium), Some(true), true),
            PopupKind::Other
        );
        assert_eq!(
            keys_for("Apple", PopupKind::Other, "", None),
            Err(Refusal::Kind)
        );
    }

    #[test]
    fn a_space_is_never_typed() {
        let list = items(&[
            "Apple",
            "New York",
            "Newark",
            "United States",
            "Dragon fruit",
            "Dragon",
        ]);
        for kind in [PopupKind::AppKit, PopupKind::ChromiumSelect] {
            for title in ["United States", "New York", "Dragon fruit", "Dragon"] {
                if let Ok(keys) = keys_for(title, kind, "Apple", Some(&list)) {
                    assert!(!keys.contains(char::is_whitespace), "{keys:?}");
                }
            }
        }
    }

    #[test]
    fn a_spaced_title_is_typed_only_as_a_start_that_reaches_it_alone() {
        let list = items(&["Apple", "New York", "Newark", "United States"]);
        for kind in [PopupKind::AppKit, PopupKind::ChromiumSelect] {
            assert_eq!(
                keys_for("United States", kind, "Apple", Some(&list)),
                Ok("United".into())
            );
            assert_eq!(
                keys_for("New York", kind, "Apple", Some(&list)),
                Err(Refusal::Space {
                    keys: "New".into(),
                    reached: Some("New York".into())
                })
            );
            assert!(matches!(
                keys_for("New York", kind, "Apple", None),
                Err(Refusal::Space { reached: None, .. })
            ));
        }
    }

    #[test]
    fn an_earlier_item_or_a_missing_one_is_refused_when_the_items_are_known() {
        let list = items(&["Apple", "Dragon fruit", "Dragon", "1", "10"]);
        assert_eq!(
            keys_for("Dragon", PopupKind::AppKit, "Apple", Some(&list)),
            Err(Refusal::Reaches("Dragon fruit".into()))
        );
        assert_eq!(
            keys_for("Kiwi", PopupKind::AppKit, "Apple", Some(&list)),
            Err(Refusal::NotAnItem)
        );
        assert_eq!(
            keys_for("1", PopupKind::AppKit, "10", Some(&list)),
            Ok("1".into())
        );
        assert_eq!(
            keys_for("Dragon", PopupKind::AppKit, "Apple", None),
            Ok("Dragon".into())
        );
    }

    #[test]
    fn a_web_select_refuses_a_repeated_key_and_a_single_key_it_starts_with() {
        let list = items(&["1", "10", "11", "Banana"]);
        let web = PopupKind::ChromiumSelect;
        assert_eq!(
            keys_for("11", web, "Banana", Some(&list)),
            Err(Refusal::RepeatedKey)
        );
        assert_eq!(keys_for("1", web, "10", None), Err(Refusal::FromCurrent));
        assert_eq!(keys_for("1", web, "Banana", Some(&list)), Ok("1".into()));
        assert_eq!(keys_for("10", web, "1", Some(&list)), Ok("10".into()));
        // A native pop-up types "11" as a prefix.
        assert_eq!(
            keys_for("11", PopupKind::AppKit, "1", None),
            Ok("11".into())
        );
    }

    #[test]
    fn a_chromium_select_searches_from_the_item_it_shows() {
        let list = items(&["Banana", "Blue", "Cherry", "Black"]);
        let web = PopupKind::ChromiumSelect;
        assert_eq!(reached(web, "b", "Blue", &list), Some("Black"));
        assert_eq!(reached(web, "bl", "Cherry", &list), Some("Black"));
        // "b" moves to Blue after Banana, then "bl" stays there.
        assert_eq!(reached(web, "bl", "Banana", &list), Some("Blue"));
        // Cycle mode: each "b" moves on to the next item with a b.
        assert_eq!(reached(web, "bb", "Banana", &list), Some("Black"));
        assert_eq!(
            reached(PopupKind::AppKit, "bl", "Cherry", &list),
            Some("Blue")
        );
    }

    #[test]
    fn the_items_passed_on_the_way_are_named() {
        let list = items(&["Apple", "Banana", "Blueberry", "Cherry"]);
        let web = PopupKind::ChromiumSelect;
        assert_eq!(passed(web, "Blueberry", "Apple", &list), ["Banana"]);
        assert!(passed(web, "Cherry", "Apple", &list).is_empty());
        assert_eq!(passed(PopupKind::AppKit, "Blu", "Apple", &list), ["Banana"]);
    }
}
