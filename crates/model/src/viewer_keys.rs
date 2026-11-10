//! Where a key the user types in the Viewer goes during a Takeover (ADR
//! 0008, issue #117). With a keyboard layout, the key goes to the target
//! app as a key, as before. With an input method, such as pinyin, every
//! key without Command or Control goes through the Viewer's input context
//! first, and what the input method does with it decides what the target
//! gets (`outcome`): a key it passes through unchanged goes out as the key
//! itself, keycode and all, so arrows, Return, a game's letters and a
//! page's shortcuts act as on a keyboard; only text it composed goes out as
//! text. A password field, or secure input, never goes through the input
//! method: composing there would show the password in the Viewer.

/// The kind of the user's current input source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A keyboard layout, such as ABC or U.S.
    Layout,
    /// An input method or one of its modes, such as pinyin.
    InputMethod,
}

use crate::model::Rect;

/// What has the keyboard in the target, as accessibility reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// Editable text: a field, a text area, a page's editable element.
    Text,
    /// A password field, or secure input is on.
    Secure,
    /// Anything else the app reported: no field, or one in another
    /// window.
    #[default]
    Other,
    /// No answer to rely on: an accessibility call failed or timed out, or
    /// a Chromium or Electron app has yet to publish its page, whose
    /// focused element may be a password field it doesn't report yet.
    Unanswered,
}

/// What a lookup of the focus saw: the focus and the focused element, as
/// a hash of its accessibility element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub focus: Focus,
    pub element: Option<u64>,
}

/// Whether a lookup after the focus may have moved tells where it is now.
/// An app may move its focus some time after it handles the key or click,
/// as Chrome's renderer does, so a first look can still see the old field:
/// a look settles it when its element differs from the one focused before
/// (`before`), or when it agrees with the look just before it
/// (`previous`). A password field is no exception: a click from one into a
/// text field first sees the password field still. Keys wait meanwhile,
/// so nothing composes in one. An unanswered look never settles.
pub fn settled(before: Option<u64>, previous: Option<Look>, now: Look) -> bool {
    now.focus != Focus::Unanswered
        && ((before.is_some() && now.element.is_some() && now.element != before)
            || previous == Some(now))
}

/// The focused element's accessibility, for `focus_of`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Element<'a> {
    pub role: Option<&'a str>,
    pub subrole: Option<&'a str>,
    /// AXEditable, which web pages set on editable elements.
    pub editable: bool,
    /// AXValue can be set.
    pub value_settable: bool,
    /// The element reports AXSelectedTextRange.
    pub selection: bool,
}

/// The focus an element makes.
pub fn focus_of(element: Element) -> Focus {
    if element.subrole == Some("AXSecureTextField") || element.role == Some("AXSecureTextField") {
        return Focus::Secure;
    }
    let text_role = matches!(
        element.role,
        Some("AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField")
    );
    if text_role || element.editable || (element.value_settable && element.selection) {
        Focus::Text
    } else {
        Focus::Other
    }
}

/// The caret of a text element in its `frame`, from the bounds the element
/// reports. `before` is the bounds of the character before an empty
/// selection at `location`; `selection` the bounds of the selection itself.
/// An NSTextView reports an empty selection as a rect one line high,
/// placed anywhere: the character before it ends where the caret is, and an
/// empty selection at the start is at the frame's top-left corner. The
/// caret stays inside the frame.
pub fn caret_in(
    frame: Rect,
    location: usize,
    length: usize,
    before: Option<Rect>,
    selection: Option<Rect>,
) -> Option<Rect> {
    let caret = if length == 0 && location > 0 {
        before
            .map(|b| Rect::new(b.max_x(), b.y, 0.0, b.height))
            .or(selection)?
    } else if length == 0 {
        let height = selection.map_or(LINE, |s| s.height);
        Rect::new(frame.x, frame.y, 0.0, height)
    } else {
        selection?
    };
    let height = caret.height.clamp(1.0, frame.height.max(1.0));
    let width = caret.width.clamp(0.0, frame.width.max(0.0));
    Some(Rect::new(
        caret.x.clamp(frame.x, (frame.max_x() - width).max(frame.x)),
        caret
            .y
            .clamp(frame.y, (frame.max_y() - height).max(frame.y)),
        width,
        height,
    ))
}

/// A line's height when the element reports none.
const LINE: f64 = 18.0;

/// Where a key goes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// To the target app, as the key itself.
    Raw,
    /// Through the Viewer's input context; `outcome` says what follows.
    InputMethod,
}

/// The key press, as the Viewer sees it.
#[derive(Clone, Copy, Debug)]
pub struct Press {
    /// The macOS virtual key code.
    pub code: u16,
    pub command: bool,
    pub control: bool,
    /// A repeat of a key held down.
    pub repeat: bool,
}

/// Whether the target's focus is a password field, as last looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secrecy {
    /// Not looked up since the focus may have moved: a click, Tab, Return,
    /// an arrow, a shortcut or a commit.
    Unknown,
    /// A password field, or secure input is on.
    Secure,
    /// Anything else.
    Open,
}

/// Where `press` goes first, with `source` selected and the target's focus
/// `secrecy`. Until a lookup says the focus is no password field, keys go
/// as keys: a field reached by Tab or Return, or one a page focused by
/// itself, may be one. The Viewer holds such keys for the lookup first
/// (see `holds`), so this applies once the wait is over.
pub fn route(source: Source, secrecy: Secrecy, press: Press) -> Route {
    if source == Source::Layout || secrecy != Secrecy::Open || press.command || press.control {
        Route::Raw
    } else {
        Route::InputMethod
    }
}

/// How long a key waits for the lookup of the target's focus before it
/// goes as the key, in milliseconds. Lookups in the e2e runs took 55 ms
/// at the median and 142 ms at most; a timeout sends pinyin's first
/// letter as the key, so the limit leaves room above that.
pub const HOLD_MS: u64 = 250;

/// Whether `press` waits for the lookup of the target's focus, with
/// `source` selected: only a key the input method would see, while the
/// focus is unknown, unless an earlier wait ran out (`gave_up`) and no
/// lookup has answered since. A key that went as the key at once would
/// put pinyin's first letter in a text field as a letter.
pub fn holds(source: Source, secrecy: Secrecy, press: Press, gave_up: bool) -> bool {
    source == Source::InputMethod
        && secrecy == Secrecy::Unknown
        && !press.command
        && !press.control
        && !gave_up
}

/// Whether a modifier change waits for the lookup like a key: Shift and
/// Caps Lock, which switch an input method between Chinese and English, so
/// it must see them in order with the keys around them. Command, Control
/// and Option go on at once, as the shortcuts they begin do.
pub fn holds_modifier(source: Source, secrecy: Secrecy, code: u16, gave_up: bool) -> bool {
    matches!(code, 56 | 57 | 60)
        && holds(
            source,
            secrecy,
            Press {
                code,
                command: false,
                control: false,
                repeat: false,
            },
            gave_up,
        )
}

/// Whether a key the target got may have moved its focus, so whether the
/// focus is a password field must be looked up again: Tab, which moves to
/// the next field; Return and Enter, which submit a form, as a login page
/// that asks for the password next; and any Command or Control shortcut,
/// as one opening a tab or focusing the address bar. Arrows move within a
/// field, a list or a group of buttons, never into a password field, and
/// a held key's repeats move nothing its first press didn't: a lookup for
/// each would hold the next key back while the arrow keeps firing.
pub fn moves_focus(press: Press) -> bool {
    !press.repeat && (press.command || press.control || matches!(press.code, 36 | 48 | 76))
}

/// What the input method did with a key, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// It marked this text, empty for none: composing.
    Marked(String),
    /// It inserted this text, ending any composition.
    Inserted(String),
    /// It asked for the key's command, as doCommandBySelector: Return,
    /// Tab, an arrow, Delete, Escape.
    Command,
    /// It made a call the Viewer can't follow, such as replacing text
    /// already sent: the key is spent.
    Ignored,
}

/// What the target gets for a key that went through the input context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The key itself, keycode and all: the input method passed it through.
    Key,
    /// Text the input method composed, as Unicode text.
    Text(String),
    /// Text it composed, then the key itself, as Korean input commits a
    /// syllable and then moves on with Return, Tab or an arrow.
    TextThenKey(String),
    /// Nothing: the input method kept the key, as part of a composition.
    Nothing,
}

impl Outcome {
    /// The name the debug protocol reports.
    pub fn name(&self) -> &'static str {
        match self {
            Outcome::Key => "raw",
            Outcome::Text(_) => "text",
            Outcome::TextThenKey(_) => "textThenKey",
            Outcome::Nothing => "inputMethod",
        }
    }
}

/// What the target gets for a key whose characters are `characters`, from
/// what the input method did with it: `calls`, with text marked before it
/// when `composing`. Inserting exactly the key's characters with nothing
/// marked, or doing nothing the Viewer sees, as for a command such as an
/// arrow, Return or Delete, passes the key through. Doing nothing while
/// composing keeps it, as a key that moves through the candidates does.
/// Asking for the key's command passes it through too, after any text it
/// inserted; Pinyin's Return, which commits the letters as typed, asks for
/// none, so it adds no Return.
pub fn outcome(composing: bool, characters: &str, calls: &[Call]) -> Outcome {
    if calls.is_empty() {
        return if composing {
            Outcome::Nothing
        } else {
            Outcome::Key
        };
    }
    let commanded = calls.contains(&Call::Command);
    let marked = calls.iter().any(|c| matches!(c, Call::Marked(_)));
    let ignored = calls.contains(&Call::Ignored);
    let inserted: String = calls
        .iter()
        .filter_map(|c| match c {
            Call::Inserted(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if commanded && !ignored {
        return if inserted.is_empty() {
            Outcome::Key
        } else {
            Outcome::TextThenKey(inserted)
        };
    }
    if !composing && !marked && !ignored && !characters.is_empty() && inserted == characters {
        Outcome::Key
    } else if inserted.is_empty() {
        Outcome::Nothing
    } else {
        Outcome::Text(inserted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u16 = 0;
    const RETURN: u16 = 36;
    const LEFT: u16 = 123;

    fn key(code: u16) -> Press {
        Press {
            code,
            command: false,
            control: false,
            repeat: false,
        }
    }

    #[test]
    fn a_layout_sends_every_key_as_a_key() {
        for code in [A, RETURN, LEFT] {
            assert_eq!(route(Source::Layout, Secrecy::Open, key(code)), Route::Raw);
        }
    }

    #[test]
    fn an_input_method_sees_every_key_but_shortcuts() {
        for code in [A, RETURN, LEFT] {
            assert_eq!(
                route(Source::InputMethod, Secrecy::Open, key(code)),
                Route::InputMethod
            );
        }
        let command = Press {
            command: true,
            ..key(A)
        };
        let control = Press {
            control: true,
            ..key(A)
        };
        assert_eq!(
            route(Source::InputMethod, Secrecy::Open, command),
            Route::Raw
        );
        assert_eq!(
            route(Source::InputMethod, Secrecy::Open, control),
            Route::Raw
        );
    }

    #[test]
    fn a_password_field_bypasses_the_input_method() {
        assert_eq!(
            route(Source::InputMethod, Secrecy::Secure, key(A)),
            Route::Raw
        );
    }

    #[test]
    fn keys_go_as_keys_until_the_focus_is_known_to_be_no_password_field() {
        assert_eq!(
            route(Source::InputMethod, Secrecy::Unknown, key(A)),
            Route::Raw
        );
    }

    #[test]
    fn a_key_waits_for_the_lookup_only_while_the_focus_is_unknown() {
        assert!(holds(Source::InputMethod, Secrecy::Unknown, key(A), false));
        assert!(holds(
            Source::InputMethod,
            Secrecy::Unknown,
            key(RETURN),
            false
        ));
        assert!(!holds(Source::InputMethod, Secrecy::Open, key(A), false));
        assert!(!holds(Source::InputMethod, Secrecy::Secure, key(A), false));
        assert!(!holds(Source::Layout, Secrecy::Unknown, key(A), false));
        // Once a wait ran out, keys go as keys until a lookup answers.
        assert!(!holds(Source::InputMethod, Secrecy::Unknown, key(A), true));
        // Shortcuts never go through the input method, so never wait.
        let command = Press {
            command: true,
            ..key(A)
        };
        assert!(!holds(
            Source::InputMethod,
            Secrecy::Unknown,
            command,
            false
        ));
    }

    #[test]
    fn shift_and_caps_lock_wait_with_the_keys() {
        for code in [56, 57, 60] {
            assert!(holds_modifier(
                Source::InputMethod,
                Secrecy::Unknown,
                code,
                false
            ));
            assert!(!holds_modifier(
                Source::InputMethod,
                Secrecy::Open,
                code,
                false
            ));
            assert!(!holds_modifier(
                Source::InputMethod,
                Secrecy::Unknown,
                code,
                true
            ));
        }
        // Command and Option.
        for code in [55, 58] {
            assert!(!holds_modifier(
                Source::InputMethod,
                Secrecy::Unknown,
                code,
                false
            ));
        }
    }

    #[test]
    fn tab_return_and_shortcuts_may_move_the_focus() {
        for code in [36, 48, 76] {
            assert!(moves_focus(key(code)), "code {code}");
        }
        for code in [A, 51, 123, 124, 125, 126] {
            assert!(!moves_focus(key(code)), "code {code}");
        }
        assert!(moves_focus(Press {
            command: true,
            ..key(17)
        }));
        // A held Return's repeats look nothing up again.
        assert!(!moves_focus(Press {
            repeat: true,
            ..key(RETURN)
        }));
    }

    #[test]
    fn a_look_settles_the_focus_once_it_moved_or_two_agree() {
        let look = |focus, element| Look { focus, element };
        let field = look(Focus::Text, Some(1));
        // The field focused before the key: maybe the app hasn't moved on.
        assert!(!settled(Some(1), None, field));
        // A second look that agrees settles it.
        assert!(settled(Some(1), Some(field), field));
        // Another element: the focus moved.
        assert!(settled(Some(1), None, look(Focus::Text, Some(2))));
        // Nothing known before: two looks must agree.
        assert!(!settled(None, None, look(Focus::Text, Some(2))));
        // The password field focused before: maybe the app hasn't moved on
        // to the field clicked.
        assert!(!settled(Some(1), None, look(Focus::Secure, Some(1))));
        assert!(settled(Some(1), None, look(Focus::Secure, Some(2))));
        // No answer never settles, even twice.
        let none = look(Focus::Unanswered, None);
        assert!(!settled(Some(1), Some(none), none));
    }

    #[test]
    fn a_command_after_composed_text_sends_the_text_then_the_key() {
        // Korean 2-Set: Return after a syllable commits it and moves on.
        assert_eq!(
            outcome(true, "\r", &[inserted("한"), Call::Command]),
            Outcome::TextThenKey("한".into())
        );
        // A command alone, even while composing, is the key.
        assert_eq!(outcome(true, "\u{f702}", &[Call::Command]), Outcome::Key);
        assert_eq!(outcome(false, "\r", &[Call::Command]), Outcome::Key);
        // Pinyin's Return commits the letters and asks for no command.
        assert_eq!(
            outcome(true, "\r", &[inserted("nihao")]),
            Outcome::Text("nihao".into())
        );
    }

    fn marked(text: &str) -> Call {
        Call::Marked(text.into())
    }

    fn inserted(text: &str) -> Call {
        Call::Inserted(text.into())
    }

    #[test]
    fn a_key_passed_through_goes_as_the_key() {
        // English mode, a digit, a space, a game's letter.
        assert_eq!(outcome(false, "a", &[inserted("a")]), Outcome::Key);
        assert_eq!(outcome(false, " ", &[inserted(" ")]), Outcome::Key);
        // A command: arrows, Return, Delete, Escape, Tab.
        assert_eq!(outcome(false, "\u{f702}", &[]), Outcome::Key);
        assert_eq!(outcome(false, "\r", &[]), Outcome::Key);
    }

    #[test]
    fn composed_text_goes_as_text() {
        // Pinyin's own punctuation.
        assert_eq!(
            outcome(false, ",", &[inserted("，")]),
            Outcome::Text("，".into())
        );
        // Space or Return commits a composition.
        assert_eq!(
            outcome(true, " ", &[inserted("你好")]),
            Outcome::Text("你好".into())
        );
        // Even the key's own character: it ends a composition.
        assert_eq!(
            outcome(true, "a", &[inserted("a")]),
            Outcome::Text("a".into())
        );
        // A commit and a new composition from one key.
        assert_eq!(
            outcome(false, "n", &[inserted("n"), marked("n")]),
            Outcome::Text("n".into())
        );
    }

    #[test]
    fn a_key_the_input_method_keeps_sends_nothing() {
        assert_eq!(outcome(false, "n", &[marked("n")]), Outcome::Nothing);
        assert_eq!(outcome(true, "i", &[marked("ni")]), Outcome::Nothing);
        // Escape cancels.
        assert_eq!(outcome(true, "\u{1b}", &[marked("")]), Outcome::Nothing);
        // A candidate page key, with no call the Viewer sees.
        assert_eq!(outcome(true, "=", &[]), Outcome::Nothing);
        // A correction of text already sent.
        assert_eq!(outcome(false, "a", &[Call::Ignored]), Outcome::Nothing);
    }

    #[test]
    fn editable_text_is_a_text_role_or_editable_or_a_settable_value_with_a_selection() {
        let of = |role, subrole| Element {
            role: Some(role),
            subrole,
            ..Element::default()
        };
        for role in ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"] {
            assert_eq!(focus_of(of(role, None)), Focus::Text, "{role}");
        }
        assert_eq!(
            focus_of(of("AXTextField", Some("AXSecureTextField"))),
            Focus::Secure
        );
        assert_eq!(focus_of(of("AXButton", None)), Focus::Other);
        assert_eq!(focus_of(of("AXWebArea", None)), Focus::Other);
        assert_eq!(focus_of(Element::default()), Focus::Other);
        let editable = Element {
            editable: true,
            ..of("AXGroup", None)
        };
        assert_eq!(focus_of(editable), Focus::Text);
        let value = Element {
            value_settable: true,
            selection: true,
            ..of("AXGroup", None)
        };
        assert_eq!(focus_of(value), Focus::Text);
        // A slider's value can be set, but it selects no text.
        let slider = Element {
            value_settable: true,
            ..of("AXSlider", None)
        };
        assert_eq!(focus_of(slider), Focus::Other);
    }

    #[test]
    fn the_caret_comes_from_the_character_before_it_and_stays_in_the_frame() {
        let frame = Rect::new(100.0, 200.0, 600.0, 300.0);
        // After "ab": the end of "b", whatever the empty range says.
        let b = Rect::new(120.0, 204.0, 12.0, 28.0);
        let wrong = Rect::new(100.0, 170.0, 0.0, 28.0);
        assert_eq!(
            caret_in(frame, 2, 0, Some(b), Some(wrong)),
            Some(Rect::new(132.0, 204.0, 0.0, 28.0))
        );
        // At the start: the frame's top-left corner, a line high.
        assert_eq!(
            caret_in(frame, 0, 0, None, Some(wrong)),
            Some(Rect::new(100.0, 200.0, 0.0, 28.0))
        );
        assert_eq!(
            caret_in(frame, 0, 0, None, None),
            Some(Rect::new(100.0, 200.0, 0.0, 18.0))
        );
        // A rect above the element, as one line over the title bar, comes
        // inside it.
        assert_eq!(
            caret_in(frame, 3, 0, None, Some(wrong)),
            Some(Rect::new(100.0, 200.0, 0.0, 28.0))
        );
        // A selection keeps its bounds, clamped.
        let selected = Rect::new(650.0, 480.0, 100.0, 40.0);
        assert_eq!(
            caret_in(frame, 1, 4, None, Some(selected)),
            Some(Rect::new(600.0, 460.0, 100.0, 40.0))
        );
        assert_eq!(caret_in(frame, 1, 4, None, None), None);
    }

    #[test]
    fn outcome_names_match_the_debug_protocol() {
        assert_eq!(Outcome::Key.name(), "raw");
        assert_eq!(Outcome::Text("x".into()).name(), "text");
        assert_eq!(Outcome::TextThenKey("x".into()).name(), "textThenKey");
        assert_eq!(Outcome::Nothing.name(), "inputMethod");
    }
}
