//! Input for a window of the user's front app (#110). The user works in one
//! window of an app, such as a browser, while the agent works in another of
//! its windows on an Agent Display. Posted keys reach the app's key window,
//! which is the user's, and a posted click on another window of the front
//! app makes that window key, so the user's typing would go there.
//!
//! In that state Agent Screen posts nothing. Text goes into the field the
//! agent last clicked through accessibility, read back after; Return and
//! Escape become that element's confirm, press and cancel actions; Tab and
//! shift+Tab set focus on the next or previous element of the page; a left
//! click becomes a press or picks a field. A control whose press opens a
//! menu, as a select, is never pressed: an open menu takes the user's
//! keyboard. Everything else fails and names the window that holds focus.

use crate::keychord::KeyChord;

/// The target app's state as the user sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    /// Not the front app: posted input and the focus record reach the
    /// agent's window.
    No,
    /// The front app, with a window of the agent's focused, as when an app
    /// the agent started brought itself forward: the target, or another
    /// window on the target's Agent Display, such as a Handover of the
    /// app's (#162). Posted input reaches the agent's windows only.
    TargetFocused,
    /// The front app, with another window focused, the user's: posted
    /// input would reach that window or take its focus.
    OtherFocused,
    /// The front app, and accessibility didn't say which window has focus.
    FocusUnknown,
}

/// The front app's focused window, as accessibility reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focused {
    pub window: u32,
    /// Whether it is on the Agent Display the target window is on, the
    /// Agent Session's own: a Handover there, or another window of the
    /// agent's. Such a window isn't the user's, even when the app is their
    /// front app, as WeCom was with its only window handed over (#162).
    pub on_target_display: bool,
    /// Whether the app also has a window on the user's screens. The user
    /// may click it during a long `type`, and the keys still going out
    /// would reach it, so then a focused window on the target's display
    /// counts as the user's too.
    pub user_window: bool,
}

impl Front {
    /// From what the system says: whether the target's app is in front, and
    /// its focused window, if accessibility answered. A focused window
    /// anywhere off the target's display, on the user's screen, is the
    /// user's (#110).
    pub fn of(target_is_front: bool, focused: Option<Focused>, target_window: u32) -> Front {
        match (target_is_front, focused) {
            (false, _) => Front::No,
            (true, Some(f))
                if f.window == target_window || (f.on_target_display && !f.user_window) =>
            {
                Front::TargetFocused
            }
            (true, Some(_)) => Front::OtherFocused,
            (true, None) => Front::FocusUnknown,
        }
    }

    /// Whether posted events reach the target window and nothing else.
    pub fn posts(self) -> bool {
        matches!(self, Front::No | Front::TargetFocused)
    }

    /// Whether the user may be working in another window of the target's
    /// app, so a posted click or press could take their focus: one of its
    /// windows has focus, or none is known to and the app has a window on
    /// the user's screens (`user_window`). An app whose windows are all
    /// on Agent Displays, as one that never answers which window has
    /// focus, such as a Java app or a game, has no window of the user's.
    pub fn shared(self, user_window: bool) -> bool {
        match self {
            Front::OtherFocused => true,
            Front::FocusUnknown => user_window,
            Front::No | Front::TargetFocused => false,
        }
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
    /// Tab: focus the page's next element that takes focus.
    FocusNext,
    /// shift+Tab: focus the previous one.
    FocusPrevious,
    Refuse,
}

const RETURN: u16 = 36;
const KEYPAD_ENTER: u16 = 76;
const ESCAPE: u16 = 53;
const BACKSPACE: u16 = 51;
const TAB: u16 = 48;
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
    let only_shift = m.shift && !(m.control || m.alt || m.command || m.function);
    match chord.code {
        RETURN | KEYPAD_ENTER if plain => KeyRoute::Confirm,
        ESCAPE if plain => KeyRoute::Cancel,
        BACKSPACE if plain => KeyRoute::DeleteBackward,
        A if only_command => KeyRoute::SelectAll,
        TAB if plain => KeyRoute::FocusNext,
        TAB if only_shift => KeyRoute::FocusPrevious,
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

/// `user_window`: whether the app has a window on the user's screens.
pub fn click_route(
    front: Front,
    user_window: bool,
    left: bool,
    count: i64,
    modifiers: bool,
) -> ClickRoute {
    match front.shared(user_window) {
        false => ClickRoute::Post,
        // With no window known to hold focus, the user's may: a posted
        // click could take it, so nothing is posted, as beside it.
        true if left && count <= 1 && !modifiers => ClickRoute::Accessibility,
        true => ClickRoute::Refuse,
    }
}

/// What an element under a click is good for, by its accessibility role
/// and actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementUse {
    /// Text goes in through its value.
    Text,
    Press,
    /// A press opens a menu, which takes the user's keyboard while it is
    /// open: nothing presses it.
    Menu,
    Nothing,
}

const TEXT_ROLES: [&str; 4] = ["AXTextField", "AXTextArea", "AXSearchField", "AXComboBox"];

/// Roles whose press can open a menu: a pop-up button, as AppKit's and a
/// web page's select, and a pull-down menu button.
const MENU_ROLES: [&str; 2] = ["AXPopUpButton", "AXMenuButton"];

/// Whether an element's role is one whose press can open a menu.
pub fn may_open_menu(role: &str) -> bool {
    MENU_ROLES.contains(&role)
}

/// Whether pressing an element opens a menu. Chromium also calls a page's
/// button with aria-haspopup a pop-up button, and the page draws the menu
/// that one opens; a select holds its menu among its children. So in a
/// web page a pop-up button without a menu child takes a press, and
/// everywhere else, or where accessibility can't tell the page, it opens
/// a menu.
pub fn opens_menu(role: &str, in_web_page: Option<bool>, holds_menu: bool) -> bool {
    MENU_ROLES.contains(&role) && (in_web_page != Some(true) || holds_menu)
}

/// Roles drawn as something to press. A click on one is pressed through
/// accessibility even when it lists no AXPress, as act_on_element's press
/// already does: a click on a GitHub expander in Dia was refused, its
/// element an AXButton where neither it nor its parents listed AXPress.
const PRESS_ROLES: [&str; 5] = [
    "AXButton",
    "AXLink",
    "AXCheckBox",
    "AXRadioButton",
    "AXDisclosureTriangle",
];

/// Whether a click on the element under the point, which neither it nor
/// its parents says takes a press, is tried as a press all the same.
pub fn press_anyway(role: &str) -> bool {
    PRESS_ROLES.contains(&role)
}

/// `menu` is what `opens_menu` says of the element.
pub fn element_use(role: &str, actions: &[String], menu: bool) -> ElementUse {
    if TEXT_ROLES.contains(&role) {
        return ElementUse::Text;
    }
    if menu {
        return ElementUse::Menu;
    }
    if actions.iter().any(|a| a == "AXPress") {
        return ElementUse::Press;
    }
    ElementUse::Nothing
}

/// What Agent Screen sees of the agent's window around a press. Each is
/// None when it couldn't be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// Visible changes counted on its display so far. A small change, as
    /// one digit of a counter, can stay under the count's threshold.
    pub display_changes: Option<u64>,
    /// How many windows its app has.
    pub windows: Option<usize>,
    /// A digest of what can be read cheaply while waiting: the page's
    /// address and title, its focused element and the pressed element's
    /// own tree.
    pub quick: Option<u64>,
    /// The focused element in the agent's window, while one there has
    /// focus. None, no information, as when the read timed out or focus
    /// is in the user's window, where they may move it: never a change.
    pub focus: Option<u64>,
    /// A digest of its page's accessibility tree, which shows a changed
    /// text or value however small; read before the press and once at the
    /// end, as it costs the app tens of milliseconds.
    pub page: Option<u64>,
}

/// What a press did, as far as the display, the page and the app's
/// windows show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Seen,
    /// The page was read before and after, and neither it, the display
    /// nor the app's windows changed.
    NoneSeen,
    /// Nothing changed that could be read, but the page couldn't be.
    Unknown,
}

pub fn effect(before: Watch, after: Watch) -> Effect {
    let differ = |a: Option<u64>, b: Option<u64>| matches!((a, b), (Some(a), Some(b)) if a != b);
    if differ(before.display_changes, after.display_changes)
        || differ(before.quick, after.quick)
        || differ(before.focus, after.focus)
        || differ(before.page, after.page)
        || differ(
            before.windows.map(|w| w as u64),
            after.windows.map(|w| w as u64),
        )
    {
        return Effect::Seen;
    }
    // The display's count alone misses small changes, so only a page
    // read on both sides tells that nothing happened.
    match (before.page, after.page) {
        (Some(_), Some(_)) => Effect::NoneSeen,
        _ => Effect::Unknown,
    }
}

/// Whether a click beside the user's window goes in as a press on the
/// element under it: only when the element lists AXPress. An element
/// found only by its role (`press_anyway`) may ignore the press: Dia lists
/// no AXPress on a page's elements and ignored every press, 0 of 14
/// (#219). Pressing it anyway left the agent unable to tell whether it
/// acted, and a retry could submit twice. Such a click waits in the
/// Input Queue for the app to leave the front instead, and then goes in
/// as a posted click.
pub fn press_goes_in(listed: bool) -> bool {
    listed
}

/// Whether Tab stops at an element: one that is enabled and lets a client
/// set its focus. The page itself and the scroll area around it take
/// focus too, but Tab moves among what is in them. An element a page
/// keeps out of its tab order with tabindex="-1" still takes focus, so
/// this order is only close to a browser's.
pub fn tab_stop(role: &str, focus_settable: bool, enabled: bool) -> bool {
    focus_settable && enabled && !matches!(role, "AXWebArea" | "AXScrollArea")
}

/// Where Tab (`forward`) or shift+Tab goes among `count` stops in tree
/// order, from the stop at `from`, or from none. None past either end,
/// where a browser moves into its own controls.
pub fn next_stop(count: usize, from: Option<usize>, forward: bool) -> Option<usize> {
    match (from, forward) {
        _ if count == 0 => None,
        (None, true) => Some(0),
        (None, false) => Some(count - 1),
        (Some(i), true) => (i + 1 < count).then_some(i + 1),
        (Some(i), false) => i.checked_sub(1).filter(|&p| p < count),
    }
}

/// Whether an element is a web form as Chromium shows it: a group whose
/// subrole is AXLandmarkForm when the form has a name, else whose role
/// description is "form". Chromium translates role descriptions, so in
/// another language an unnamed form isn't recognised.
pub fn is_form(role: &str, subrole: &str, role_description: &str) -> bool {
    role == "AXGroup" && (subrole == "AXLandmarkForm" || role_description == "form")
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

/// The window of a shared app to hand key focus back to once the agent's
/// input is done: the app's focused window as the agent starts, when it is
/// the user's, not one on an Agent Display. Read before anything is sent,
/// since a posted click or the wake click makes the agent's window key.
pub fn user_window_to_remember(focused: Option<u32>, on_agent_display: bool) -> Option<u32> {
    focused.filter(|_| !on_agent_display)
}

/// Whether a focus record may go to a window of an app in the background
/// whose key window, as Accessibility names it, is `key`.
///
/// The record is AppKit's "application activated" event. AppKit in the
/// app takes it for its app becoming active, posts that to the app, and
/// Electron apps report it as `did-become-active`, though WindowServer
/// keeps the user's app in front (#273). When another window of the app
/// is key, it moves nothing: a click or accessibility moves key status.
/// When the target is key already, keys reach it without the record, and
/// after the front app changed the wake click makes its page take clicks
/// again (electronfront and nativeclick, measured with and without it).
/// So it goes out only where the app names no key window, as a new window
/// or an app too busy to answer.
pub fn focus_record_needed(key: Option<u32>) -> bool {
    key.is_none()
}

/// What to do with a remembered user window once the agent's input is
/// done, by the app's focused window now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandBack {
    /// The user's window still has focus: keep it for a later hand-back.
    Keep,
    /// The app didn't say: try again on the next hand-back.
    Retry,
    /// Another window of the user's has focus, as one they picked since.
    Leave,
    /// The agent's window took focus: give it back to the user's.
    Restore,
}

/// `agent_main`: the agent's window, on its Agent Display, says it is the
/// app's main window. Chromium apps such as Dia keep naming the user's
/// window as focused after a click made the agent's window key, so that
/// counts as the agent's window having focus.
pub fn hand_back(
    user: u32,
    focused: Option<u32>,
    focused_on_agent_display: bool,
    agent_main: bool,
) -> HandBack {
    match focused {
        Some(id) if id == user && agent_main => HandBack::Restore,
        Some(id) if id == user => HandBack::Keep,
        None => HandBack::Retry,
        Some(_) if focused_on_agent_display => HandBack::Restore,
        Some(_) => HandBack::Leave,
    }
}

/// Hand-back passes in a row that may fail to make the user's window
/// main again, before it is let go.
pub const HAND_BACK_TRIES: u8 = 3;

/// How long after a failed hand-back the next pass runs, in milliseconds,
/// when no other input asks for one sooner.
pub const HAND_BACK_RETRY_MS: u64 = 300;

/// What becomes of a remembered user window after a hand-back pass:
/// `None` to forget it, or its count of failed passes to keep it with.
/// More than 0 asks for another pass after `HAND_BACK_RETRY_MS`.
///
/// `step` is `hand_back` read just now, so a pass after a failure checks
/// the app's focus again first: a window the user picked meanwhile is
/// left alone. `written`, read for `HandBack::Restore` only: whether
/// making the user's window main through accessibility succeeded. A busy
/// app can time out on that write, and the agent's window would then stay
/// key until the user switches back to the app (#273).
pub fn after_hand_back(step: HandBack, written: bool, failures: u8) -> Option<u8> {
    let failed = || Some(failures + 1).filter(|&f| f < HAND_BACK_TRIES);
    match step {
        HandBack::Leave => None,
        // The user's window has focus again: nothing to retry.
        HandBack::Keep => Some(0),
        // The app didn't say. Before any failure, wait for the next
        // hand-back as always; after one, this pass counts.
        HandBack::Retry if failures == 0 => Some(0),
        HandBack::Retry => failed(),
        HandBack::Restore if written => None,
        HandBack::Restore => failed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> KeyChord {
        KeyChord::parse(text).unwrap()
    }

    fn users(window: u32) -> Option<Focused> {
        Some(Focused {
            window,
            on_target_display: false,
            user_window: true,
        })
    }

    #[test]
    fn front_state_comes_from_the_front_app_and_its_focused_window() {
        assert_eq!(Front::of(false, users(9), 7), Front::No);
        assert_eq!(Front::of(false, None, 7), Front::No);
        assert_eq!(Front::of(true, users(7), 7), Front::TargetFocused);
        assert_eq!(Front::of(true, users(9), 7), Front::OtherFocused);
        assert_eq!(Front::of(true, None, 7), Front::FocusUnknown);
        assert!(Front::No.posts() && Front::TargetFocused.posts());
        assert!(!Front::OtherFocused.posts() && !Front::FocusUnknown.posts());
    }

    #[test]
    fn a_focused_window_on_the_agents_display_is_not_the_users() {
        // #162: WeCom's only window, handed over, is focused, and the
        // agent's click lands on another of its windows there.
        let handover = Some(Focused {
            window: 9,
            on_target_display: true,
            user_window: false,
        });
        let front = Front::of(true, handover, 7);
        assert_eq!(front, Front::TargetFocused);
        assert!(front.posts() && !front.shared(true));
        assert_eq!(text_route(front, "hi\n"), TextRoute::Post);
        assert_eq!(key_route(front, chord("Right")), KeyRoute::Post);
        assert_eq!(click_route(front, true, true, 2, false), ClickRoute::Post);
        // #110: the user's window focused on their screen still refuses.
        let theirs = Front::of(true, users(9), 7);
        assert_eq!(theirs, Front::OtherFocused);
        assert_eq!(
            click_route(theirs, true, true, 2, false),
            ClickRoute::Refuse
        );
        assert_eq!(key_route(theirs, chord("Right")), KeyRoute::Refuse);
        // Not the front app: nothing changes.
        assert_eq!(Front::of(false, handover, 7), Front::No);
        // A Handover focused while the app has another window on the
        // user's screen: they may click that window mid-type, so it
        // refuses as theirs.
        let beside_theirs = Some(Focused {
            window: 9,
            on_target_display: true,
            user_window: true,
        });
        assert_eq!(Front::of(true, beside_theirs, 7), Front::OtherFocused);
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
        assert_eq!(key_route(f, chord("Tab")), KeyRoute::FocusNext);
        assert_eq!(key_route(f, chord("shift+Tab")), KeyRoute::FocusPrevious);
        for refused in [
            "ctrl+Tab",
            "cmd+shift+Tab",
            "alt+Tab",
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
        assert_eq!(
            click_route(Front::No, true, false, 2, true),
            ClickRoute::Post
        );
        assert_eq!(
            click_route(Front::TargetFocused, true, true, 1, false),
            ClickRoute::Post
        );
        for user_window in [true, false] {
            assert_eq!(
                click_route(f, user_window, true, 1, false),
                ClickRoute::Accessibility
            );
            assert_eq!(
                click_route(f, user_window, false, 1, false),
                ClickRoute::Refuse
            );
            assert_eq!(
                click_route(f, user_window, true, 2, false),
                ClickRoute::Refuse
            );
            assert_eq!(
                click_route(f, user_window, true, 1, true),
                ClickRoute::Refuse
            );
        }
    }

    #[test]
    fn with_focus_unknown_a_click_is_posted_only_when_the_user_has_no_window_of_the_app() {
        let u = Front::FocusUnknown;
        // A window of the app on the user's screens may have focus: a
        // posted click could take it.
        assert!(u.shared(true));
        assert_eq!(
            click_route(u, true, true, 1, false),
            ClickRoute::Accessibility
        );
        assert_eq!(click_route(u, true, true, 2, false), ClickRoute::Refuse);
        // All its windows on Agent Displays, as a Java app or a game that
        // never says which window has focus: clicks go out as before.
        assert!(!u.shared(false));
        assert_eq!(click_route(u, false, true, 1, false), ClickRoute::Post);
        assert_eq!(click_route(u, false, false, 2, true), ClickRoute::Post);
        assert!(Front::OtherFocused.shared(false));
        assert!(!Front::No.shared(true) && !Front::TargetFocused.shared(true));
    }

    #[test]
    fn a_field_takes_text_and_a_control_takes_a_press() {
        let press = vec!["AXPress".to_string()];
        assert_eq!(element_use("AXTextField", &press, false), ElementUse::Text);
        assert_eq!(element_use("AXTextArea", &[], false), ElementUse::Text);
        assert_eq!(element_use("AXButton", &press, false), ElementUse::Press);
        assert_eq!(element_use("AXLink", &press, false), ElementUse::Press);
        assert_eq!(element_use("AXGroup", &[], false), ElementUse::Nothing);
        assert!(press_anyway("AXButton") && press_anyway("AXLink"));
        assert!(!press_anyway("AXGroup") && !press_anyway("AXStaticText"));
        assert!(
            MENU_ROLES.iter().all(|r| !press_anyway(r)),
            "a press role never opens a menu"
        );
        assert_eq!(element_use("AXPopUpButton", &press, true), ElementUse::Menu);
        assert_eq!(
            element_use("AXPopUpButton", &press, false),
            ElementUse::Press
        );
        assert!(takes_text("AXTextField", true, true));
        assert!(!takes_text("AXTextField", false, true), "a password field");
        assert!(!takes_text("AXTextField", true, false));
        assert!(!takes_text("AXButton", true, true));
    }

    #[test]
    fn a_select_or_native_pop_up_opens_a_menu_and_a_page_drawn_one_does_not() {
        // A web page's select holds its menu.
        assert!(opens_menu("AXPopUpButton", Some(true), true));
        // A page's button with aria-haspopup: the page draws its menu.
        assert!(!opens_menu("AXPopUpButton", Some(true), false));
        // AppKit's pop-up buttons, and any where the page can't be told.
        assert!(opens_menu("AXPopUpButton", Some(false), false));
        assert!(opens_menu("AXPopUpButton", None, false));
        assert!(opens_menu("AXMenuButton", Some(false), false));
        assert!(!opens_menu("AXButton", Some(false), true));
        assert!(!opens_menu("AXLink", None, true));
    }

    #[test]
    fn a_press_did_something_when_the_display_the_page_or_the_apps_windows_changed() {
        let watch = |display_changes, windows, page| Watch {
            display_changes,
            windows,
            quick: Some(1),
            focus: Some(3),
            page,
        };
        let same = watch(Some(4), Some(2), Some(7));
        assert_eq!(effect(same, same), Effect::NoneSeen);
        assert_eq!(effect(same, watch(Some(5), Some(2), Some(7))), Effect::Seen);
        assert_eq!(
            effect(same, watch(Some(4), Some(3), Some(7))),
            Effect::Seen,
            "a window opened off the display"
        );
        assert_eq!(
            effect(same, watch(Some(4), Some(2), Some(8))),
            Effect::Seen,
            "a counter's digit, too small for the display's count"
        );
        assert_eq!(
            effect(watch(None, None, Some(7)), watch(None, None, Some(7))),
            Effect::NoneSeen
        );
        // Without the page on both sides, an unchanged display says
        // nothing.
        assert_eq!(
            effect(watch(Some(4), Some(2), None), watch(Some(4), Some(2), None)),
            Effect::Unknown
        );
        assert_eq!(effect(same, watch(Some(4), Some(2), None)), Effect::Unknown);
        assert_eq!(
            effect(watch(None, Some(2), None), watch(None, Some(3), None)),
            Effect::Seen
        );
        let moved = Watch {
            quick: Some(2),
            ..same
        };
        assert_eq!(effect(same, moved), Effect::Seen, "the address moved");
        let refocused = Watch {
            focus: Some(4),
            ..same
        };
        assert_eq!(
            effect(same, refocused),
            Effect::Seen,
            "focus moved in the agent's window"
        );
        // A focus read that timed out, or focus in the user's window,
        // says nothing.
        let unread = Watch {
            focus: None,
            ..same
        };
        assert_eq!(effect(same, unread), Effect::NoneSeen);
        assert_eq!(effect(unread, same), Effect::NoneSeen);
    }

    #[test]
    fn tab_stops_at_enabled_elements_that_take_focus_inside_the_page() {
        assert!(tab_stop("AXTextField", true, true));
        assert!(tab_stop("AXButton", true, true));
        assert!(tab_stop("AXLink", true, true));
        assert!(
            tab_stop("AXGroup", true, true),
            "an element with tabindex=0"
        );
        assert!(!tab_stop("AXButton", true, false), "a disabled button");
        assert!(!tab_stop("AXStaticText", false, true));
        assert!(!tab_stop("AXGroup", false, true), "a form");
        assert!(!tab_stop("AXWebArea", true, true));
        assert!(!tab_stop("AXScrollArea", true, true));
    }

    #[test]
    fn tab_goes_to_the_next_stop_and_shift_tab_to_the_previous() {
        assert_eq!(next_stop(3, Some(0), true), Some(1));
        assert_eq!(next_stop(3, Some(1), false), Some(0));
        // From no stop: the first or the last, as from the page's start.
        assert_eq!(next_stop(3, None, true), Some(0));
        assert_eq!(next_stop(3, None, false), Some(2));
        // Past either end a browser moves into its own controls.
        assert_eq!(next_stop(3, Some(2), true), None);
        assert_eq!(next_stop(3, Some(0), false), None);
        assert_eq!(next_stop(0, None, true), None);
        assert_eq!(next_stop(0, None, false), None);
        assert_eq!(next_stop(2, Some(5), false), None, "a stale index");
    }

    #[test]
    fn a_form_is_a_group_chromium_calls_one() {
        assert!(is_form("AXGroup", "", "form"));
        assert!(is_form("AXGroup", "AXLandmarkForm", "form"));
        assert!(is_form("AXGroup", "AXLandmarkForm", "formulaire"));
        assert!(!is_form("AXGroup", "", "group"));
        assert!(!is_form("AXGroup", "AXLandmarkSearch", "search"));
        assert!(!is_form("AXTextField", "", "form"));
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

    #[test]
    fn a_press_goes_in_only_on_an_element_that_lists_it() {
        assert!(press_goes_in(true));
        assert!(!press_goes_in(false));
    }

    #[test]
    fn the_users_window_is_remembered_before_the_agents_input_never_the_agents() {
        // The user's window has focus as the replay starts.
        assert_eq!(user_window_to_remember(Some(7), false), Some(7));
        // Read after a wake, the agent's window would be focused: never
        // remembered as the user's.
        assert_eq!(user_window_to_remember(Some(9), true), None);
        assert_eq!(user_window_to_remember(None, false), None);
    }

    #[test]
    fn focus_goes_back_only_from_the_agents_window() {
        assert_eq!(hand_back(7, Some(9), true, false), HandBack::Restore);
        assert_eq!(hand_back(7, Some(7), false, false), HandBack::Keep);
        assert_eq!(hand_back(7, None, false, false), HandBack::Retry);
        assert_eq!(hand_back(7, Some(8), false, false), HandBack::Leave);
    }

    #[test]
    fn a_chromium_app_naming_the_users_window_while_the_agents_is_main_gets_it_back() {
        assert_eq!(hand_back(7, Some(7), false, true), HandBack::Restore);
        // A window the user picked since stays theirs.
        assert_eq!(hand_back(7, Some(8), false, true), HandBack::Leave);
        assert_eq!(hand_back(7, None, false, true), HandBack::Retry);
    }

    #[test]
    fn no_focus_record_while_another_window_of_the_app_is_key() {
        // The user's window 7 is key in the background app: waking the
        // agent's window 9, or handing focus back to 7 while 9 is key.
        assert!(!focus_record_needed(Some(7)));
    }

    #[test]
    fn no_focus_record_to_a_window_that_is_key_already() {
        // The agent's window 9 was key before the front app changed.
        assert!(!focus_record_needed(Some(9)));
    }

    #[test]
    fn a_focus_record_still_goes_when_no_key_window_is_known() {
        // The app didn't say, as a new window or a hung app.
        assert!(focus_record_needed(None));
    }

    #[test]
    fn a_failed_hand_back_is_kept_and_tried_again_a_few_times() {
        // The AXMain write timed out: keep the user's window, retry soon.
        assert_eq!(after_hand_back(HandBack::Restore, false, 0), Some(1));
        assert_eq!(after_hand_back(HandBack::Restore, false, 1), Some(2));
        // Then let it go, as for an app that stopped answering.
        assert_eq!(after_hand_back(HandBack::Restore, false, 2), None);
        // A pass the app didn't answer counts once a write failed.
        assert_eq!(after_hand_back(HandBack::Retry, false, 1), Some(2));
        assert_eq!(after_hand_back(HandBack::Retry, false, 2), None);
    }

    #[test]
    fn a_retry_ends_when_focus_went_back_or_the_user_picked_a_window() {
        assert_eq!(after_hand_back(HandBack::Restore, true, 2), None);
        // The user picked another window of theirs meanwhile.
        assert_eq!(after_hand_back(HandBack::Leave, false, 1), None);
        // The user's window is focused again: kept as before any failure.
        assert_eq!(after_hand_back(HandBack::Keep, false, 2), Some(0));
    }

    #[test]
    fn without_a_failure_a_hand_back_waits_as_before() {
        assert_eq!(after_hand_back(HandBack::Restore, true, 0), None);
        assert_eq!(after_hand_back(HandBack::Keep, false, 0), Some(0));
        assert_eq!(after_hand_back(HandBack::Retry, false, 0), Some(0));
        assert_eq!(after_hand_back(HandBack::Leave, false, 0), None);
    }
}
