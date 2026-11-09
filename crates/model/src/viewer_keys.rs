//! Where a key the user types in the Viewer goes during a Takeover (ADR
//! 0008, issue #117). With a keyboard layout, the key goes to the target
//! app as a key, so shortcuts, arrows and key repeat act there as on a
//! keyboard. With an input method, such as pinyin, the key goes to the
//! input method, which composes in the Viewer, and the target gets only the
//! finished text. Command and Control shortcuts, and keys that type no
//! text, skip the input method unless it is composing: then it takes every
//! key, as Return commits, Escape cancels and the arrows pick.
//!
//! The input method only gets keys while the target's focus is editable
//! text. In a password field the keys go as keys: composing there would
//! show the password in the Viewer in the clear. A game, a VM or a page's
//! j and k shortcuts want the keys themselves.

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
    /// Anything else, or not known yet.
    #[default]
    Other,
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

/// Where a key goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// To the target app, as the key itself.
    Raw,
    /// To the input method in the Viewer.
    InputMethod,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Raw => "raw",
            Route::InputMethod => "inputMethod",
        }
    }
}

/// The key press, as the Viewer sees it.
#[derive(Clone, Copy, Debug)]
pub struct Press {
    /// The macOS virtual key code.
    pub code: u16,
    pub command: bool,
    pub control: bool,
}

/// Where `press` goes, with `source` selected, `focus` in the target and
/// the input method `composing` text in the Viewer.
pub fn route(source: Source, focus: Focus, composing: bool, press: Press) -> Route {
    if focus == Focus::Secure {
        return Route::Raw;
    }
    if composing {
        return Route::InputMethod;
    }
    if source == Source::Layout
        || focus != Focus::Text
        || press.command
        || press.control
        || !types_text(press.code)
    {
        return Route::Raw;
    }
    Route::InputMethod
}

/// Whether the key at virtual key code `code` types text. Return, Tab,
/// Delete, Escape, the arrows, the function keys and the other keys that
/// move or edit don't. Their codes are the same on every keyboard layout.
pub fn types_text(code: u16) -> bool {
    !matches!(
        code,
        36 // Return
        | 48 // Tab
        | 51 // Delete
        | 53 // Escape
        | 76 // keypad Enter
        | 71 // keypad Clear
        | 114 // Help
        | 115 // Home
        | 116 // Page Up
        | 117 // Forward Delete
        | 119 // End
        | 121 // Page Down
        | 123..=126 // arrows
        // F1 to F20
        | 64 | 79 | 80 | 90 | 96..=101 | 103 | 105..=107 | 109 | 111 | 113 | 118 | 120 | 122
        // Caps Lock and the modifier keys, which macOS reports as key
        // codes too
        | 54..=63
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u16 = 0;
    const N: u16 = 45;
    const SPACE: u16 = 49;
    const RETURN: u16 = 36;
    const ESCAPE: u16 = 53;
    const DELETE: u16 = 51;
    const LEFT: u16 = 123;
    const F5: u16 = 96;

    fn key(code: u16) -> Press {
        Press {
            code,
            command: false,
            control: false,
        }
    }

    #[test]
    fn a_layout_sends_every_key_as_a_key() {
        for code in [A, N, SPACE, RETURN, LEFT, F5] {
            assert_eq!(
                route(Source::Layout, Focus::Text, false, key(code)),
                Route::Raw
            );
        }
    }

    #[test]
    fn an_input_method_takes_the_keys_that_type_text() {
        for code in [A, N, SPACE, 18, 43] {
            assert_eq!(
                route(Source::InputMethod, Focus::Text, false, key(code)),
                Route::InputMethod,
                "code {code}"
            );
        }
    }

    #[test]
    fn keys_with_no_text_skip_an_input_method_with_nothing_marked() {
        for code in [RETURN, ESCAPE, DELETE, 48, LEFT, 126, 115, 117, F5, 122] {
            assert_eq!(
                route(Source::InputMethod, Focus::Text, false, key(code)),
                Route::Raw,
                "code {code}"
            );
        }
    }

    #[test]
    fn shortcuts_skip_the_input_method() {
        let command = Press {
            command: true,
            ..key(A)
        };
        let control = Press {
            control: true,
            ..key(A)
        };
        assert_eq!(
            route(Source::InputMethod, Focus::Text, false, command),
            Route::Raw
        );
        assert_eq!(
            route(Source::InputMethod, Focus::Text, false, control),
            Route::Raw
        );
    }

    #[test]
    fn a_composing_input_method_takes_every_key() {
        for code in [A, SPACE, RETURN, ESCAPE, DELETE, LEFT, F5] {
            assert_eq!(
                route(Source::InputMethod, Focus::Text, true, key(code)),
                Route::InputMethod,
                "code {code}"
            );
        }
        let command = Press {
            command: true,
            ..key(A)
        };
        assert_eq!(
            route(Source::InputMethod, Focus::Text, true, command),
            Route::InputMethod
        );
        // The user switched to a layout mid-composition: the marked text
        // still needs its Return or Escape.
        assert_eq!(
            route(Source::Layout, Focus::Text, true, key(RETURN)),
            Route::InputMethod
        );
    }

    #[test]
    fn keys_go_as_keys_unless_the_focus_is_editable_text() {
        for focus in [Focus::Other, Focus::Secure] {
            for code in [A, N, SPACE] {
                assert_eq!(
                    route(Source::InputMethod, focus, false, key(code)),
                    Route::Raw,
                    "{focus:?} code {code}"
                );
            }
        }
    }

    #[test]
    fn a_password_field_never_composes() {
        // Not even with a composition left over from another field.
        assert_eq!(
            route(Source::InputMethod, Focus::Secure, true, key(A)),
            Route::Raw
        );
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
    fn route_names_match_the_debug_protocol() {
        assert_eq!(Route::Raw.name(), "raw");
        assert_eq!(Route::InputMethod.name(), "inputMethod");
    }
}
