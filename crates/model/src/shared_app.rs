//! Input for a window of the user's front app (#110). The user works in one
//! window of an app, such as a browser, while the agent works in another of
//! its windows on an Agent Display. Posted keys reach the app's key window,
//! which is the user's, and a posted click on another window of the front
//! app makes that window key, so the user's typing would go there.
//!
//! In that state Agent Screen posts nothing. Text goes into the field the
//! agent last clicked through accessibility, read back after; Return and
//! Escape become that element's confirm, press and cancel actions; a left
//! click becomes a press or picks a field. Everything else fails and names
//! the window that holds focus.

use crate::keychord::KeyChord;

/// The target app's state as the user sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    /// Not the front app: posted input and the focus record reach the
    /// agent's window.
    No,
    /// The front app, with the agent's window focused, as when an app the
    /// agent started brought itself forward: posted input reaches it.
    TargetFocused,
    /// The front app, with another window focused, the user's: posted
    /// input would reach that window or take its focus.
    OtherFocused,
    /// The front app, and accessibility didn't say which window has focus.
    FocusUnknown,
}

impl Front {
    /// From what the system says: whether the target's app is in front, and
    /// its focused window, if accessibility answered.
    pub fn of(target_is_front: bool, focused: Option<u32>, target_window: u32) -> Front {
        match (target_is_front, focused) {
            (false, _) => Front::No,
            (true, Some(id)) if id == target_window => Front::TargetFocused,
            (true, Some(_)) => Front::OtherFocused,
            (true, None) => Front::FocusUnknown,
        }
    }

    /// Whether posted events reach the target window and nothing else.
    pub fn posts(self) -> bool {
        matches!(self, Front::No | Front::TargetFocused)
    }
}

/// How a `type` call's text goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRoute {
    Post,
    /// Into the remembered field through accessibility. No key events.
    Insert,
    /// Return and Tab are keys, and no field takes them as text.
    RefuseKeysInText,
    Refuse,
}

pub fn text_route(front: Front, text: &str) -> TextRoute {
    match front {
        Front::No | Front::TargetFocused => TextRoute::Post,
        Front::OtherFocused if text.contains(['\n', '\r', '\t']) => TextRoute::RefuseKeysInText,
        Front::OtherFocused => TextRoute::Insert,
        Front::FocusUnknown => TextRoute::Refuse,
    }
}

/// How one key chord of a `key` call goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRoute {
    Post,
    /// Return: the remembered element's confirm action, or its press.
    Confirm,
    /// Escape: the remembered element's cancel action.
    Cancel,
    /// cmd+a: select the remembered field's text.
    SelectAll,
    /// Backspace: delete in the remembered field.
    DeleteBackward,
    Refuse,
}

const RETURN: u16 = 36;
const KEYPAD_ENTER: u16 = 76;
const ESCAPE: u16 = 53;
const BACKSPACE: u16 = 51;
const A: u16 = 0;

pub fn key_route(front: Front, chord: KeyChord) -> KeyRoute {
    match front {
        Front::No | Front::TargetFocused => return KeyRoute::Post,
        Front::FocusUnknown => return KeyRoute::Refuse,
        Front::OtherFocused => {}
    }
    let m = chord.modifiers;
    let plain = !(m.control || m.alt || m.command || m.function || m.shift);
    let only_command = m.command && !(m.control || m.alt || m.function || m.shift);
    match chord.code {
        RETURN | KEYPAD_ENTER if plain => KeyRoute::Confirm,
        ESCAPE if plain => KeyRoute::Cancel,
        BACKSPACE if plain => KeyRoute::DeleteBackward,
        A if only_command => KeyRoute::SelectAll,
        _ => KeyRoute::Refuse,
    }
}

/// How a click goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickRoute {
    Post,
    /// A single left click without modifiers: press what is under the
    /// point, or remember the field there, through accessibility.
    Accessibility,
    /// Right clicks, double clicks and clicks with modifiers have no
    /// accessibility action.
    Refuse,
}

pub fn click_route(front: Front, left: bool, count: i64, modifiers: bool) -> ClickRoute {
    match front {
        // With no window known to hold focus, as in an app still starting
        // that doesn't answer yet, the click goes out as before.
        Front::No | Front::TargetFocused | Front::FocusUnknown => ClickRoute::Post,
        Front::OtherFocused if left && count <= 1 && !modifiers => ClickRoute::Accessibility,
        Front::OtherFocused => ClickRoute::Refuse,
    }
}

/// What an element under a click is good for, by its accessibility role
/// and actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementUse {
    /// Text goes in through its value.
    Text,
    Press,
    Nothing,
}

const TEXT_ROLES: [&str; 4] = ["AXTextField", "AXTextArea", "AXSearchField", "AXComboBox"];

pub fn element_use(role: &str, actions: &[String]) -> ElementUse {
    if TEXT_ROLES.contains(&role) {
        return ElementUse::Text;
    }
    if actions.iter().any(|a| a == "AXPress") {
        return ElementUse::Press;
    }
    ElementUse::Nothing
}

/// Whether text can go into an element through accessibility: a field
/// that says what it holds and lets its value be set. A password field
/// says nothing, so what went in can't be read back.
pub fn takes_text(role: &str, value_known: bool, value_settable: bool) -> bool {
    TEXT_ROLES.contains(&role) && value_known && value_settable
}

/// `value` with `text` in place of the selection, a UTF-16 range as
/// accessibility gives it, or at the end when there is none, and the
/// caret after it, in UTF-16 units.
pub fn insert(value: &str, selection: Option<(usize, usize)>, text: &str) -> (String, usize) {
    let units: Vec<u16> = value.encode_utf16().collect();
    let (start, length) = selection.unwrap_or((units.len(), 0));
    let start = start.min(units.len());
    let end = start.saturating_add(length).min(units.len());
    let mut out: Vec<u16> = units[..start].to_vec();
    out.extend(text.encode_utf16());
    let caret = out.len();
    out.extend_from_slice(&units[end..]);
    (String::from_utf16_lossy(&out), caret)
}

/// `value` after a Backspace: the selection deleted, or the character
/// before the caret, and the caret after.
pub fn delete_backward(value: &str, selection: Option<(usize, usize)>) -> (String, usize) {
    let units: Vec<u16> = value.encode_utf16().collect();
    let (start, length) = selection.unwrap_or((units.len(), 0));
    let start = start.min(units.len());
    if length > 0 {
        return insert(value, Some((start, length)), "");
    }
    if start == 0 {
        return (value.to_string(), 0);
    }
    // A character outside the BMP is two units.
    let back = if start >= 2 && (0xDC00..0xE000).contains(&units[start - 1]) {
        2
    } else {
        1
    };
    insert(value, Some((start - back, back)), "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> KeyChord {
        KeyChord::parse(text).unwrap()
    }

    #[test]
    fn front_state_comes_from_the_front_app_and_its_focused_window() {
        assert_eq!(Front::of(false, Some(9), 7), Front::No);
        assert_eq!(Front::of(false, None, 7), Front::No);
        assert_eq!(Front::of(true, Some(7), 7), Front::TargetFocused);
        assert_eq!(Front::of(true, Some(9), 7), Front::OtherFocused);
        assert_eq!(Front::of(true, None, 7), Front::FocusUnknown);
        assert!(Front::No.posts() && Front::TargetFocused.posts());
        assert!(!Front::OtherFocused.posts() && !Front::FocusUnknown.posts());
    }

    #[test]
    fn text_is_posted_unless_the_users_window_has_focus() {
        assert_eq!(text_route(Front::No, "hi"), TextRoute::Post);
        assert_eq!(text_route(Front::TargetFocused, "hi\n"), TextRoute::Post);
        assert_eq!(text_route(Front::OtherFocused, "hi"), TextRoute::Insert);
        assert_eq!(text_route(Front::FocusUnknown, "hi"), TextRoute::Refuse);
    }

    #[test]
    fn return_and_tab_in_text_are_keys_no_field_takes() {
        for text in ["hi\n", "a\tb", "x\r"] {
            assert_eq!(
                text_route(Front::OtherFocused, text),
                TextRoute::RefuseKeysInText
            );
        }
    }

    #[test]
    fn only_keys_with_an_accessibility_action_go_in_beside_the_users_window() {
        let f = Front::OtherFocused;
        assert_eq!(key_route(f, chord("Return")), KeyRoute::Confirm);
        assert_eq!(key_route(f, chord("KP_Enter")), KeyRoute::Confirm);
        assert_eq!(key_route(f, chord("Escape")), KeyRoute::Cancel);
        assert_eq!(key_route(f, chord("BackSpace")), KeyRoute::DeleteBackward);
        assert_eq!(key_route(f, chord("cmd+a")), KeyRoute::SelectAll);
        for refused in [
            "Tab",
            "shift+Return",
            "cmd+Return",
            "a",
            "cmd+v",
            "ctrl+a",
            "Down",
        ] {
            assert_eq!(key_route(f, chord(refused)), KeyRoute::Refuse, "{refused}");
        }
    }

    #[test]
    fn keys_are_posted_where_they_reach_only_the_agents_window() {
        for front in [Front::No, Front::TargetFocused] {
            assert_eq!(key_route(front, chord("Tab")), KeyRoute::Post);
            assert_eq!(key_route(front, chord("Return")), KeyRoute::Post);
        }
        assert_eq!(
            key_route(Front::FocusUnknown, chord("Return")),
            KeyRoute::Refuse
        );
    }

    #[test]
    fn only_a_plain_left_click_has_an_accessibility_route() {
        let f = Front::OtherFocused;
        assert_eq!(click_route(Front::No, false, 2, true), ClickRoute::Post);
        assert_eq!(
            click_route(Front::TargetFocused, true, 1, false),
            ClickRoute::Post
        );
        assert_eq!(click_route(f, true, 1, false), ClickRoute::Accessibility);
        assert_eq!(
            click_route(Front::FocusUnknown, true, 1, false),
            ClickRoute::Post
        );
        assert_eq!(click_route(f, false, 1, false), ClickRoute::Refuse);
        assert_eq!(click_route(f, true, 2, false), ClickRoute::Refuse);
        assert_eq!(click_route(f, true, 1, true), ClickRoute::Refuse);
    }

    #[test]
    fn a_field_takes_text_and_a_control_takes_a_press() {
        let press = vec!["AXPress".to_string()];
        assert_eq!(element_use("AXTextField", &press), ElementUse::Text);
        assert_eq!(element_use("AXTextArea", &[]), ElementUse::Text);
        assert_eq!(element_use("AXButton", &press), ElementUse::Press);
        assert_eq!(element_use("AXLink", &press), ElementUse::Press);
        assert_eq!(element_use("AXGroup", &[]), ElementUse::Nothing);
        assert!(takes_text("AXTextField", true, true));
        assert!(!takes_text("AXTextField", false, true), "a password field");
        assert!(!takes_text("AXTextField", true, false));
        assert!(!takes_text("AXButton", true, true));
    }

    #[test]
    fn text_replaces_the_selection_or_goes_at_the_end() {
        assert_eq!(insert("", None, "hello"), ("hello".into(), 5));
        assert_eq!(insert("ab", None, "c"), ("abc".into(), 3));
        assert_eq!(insert("abcd", Some((1, 2)), "X"), ("aXd".into(), 2));
        assert_eq!(insert("abcd", Some((0, 0)), "X"), ("Xabcd".into(), 1));
        // Out of range, as a stale selection after the page changed it.
        assert_eq!(insert("ab", Some((9, 4)), "c"), ("abc".into(), 3));
        // UTF-16 units, as accessibility counts.
        assert_eq!(insert("😀b", Some((2, 1)), "你"), ("😀你".into(), 3));
    }

    #[test]
    fn backspace_deletes_the_selection_or_the_character_before_the_caret() {
        assert_eq!(delete_backward("abc", None), ("ab".into(), 2));
        assert_eq!(delete_backward("abc", Some((1, 0))), ("bc".into(), 0));
        assert_eq!(delete_backward("abc", Some((0, 2))), ("c".into(), 0));
        assert_eq!(delete_backward("abc", Some((0, 0))), ("abc".into(), 0));
        assert_eq!(delete_backward("a😀", None), ("a".into(), 1));
    }
}
