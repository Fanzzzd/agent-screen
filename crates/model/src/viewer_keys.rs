//! Where a key the user types in the Viewer goes during a Takeover (ADR
//! 0008, issue #117). With a keyboard layout, the key goes to the target
//! app as a key, so shortcuts, arrows and key repeat act there as on a
//! keyboard. With an input method, such as pinyin, the key goes to the
//! input method, which composes in the Viewer, and the target gets only the
//! finished text. Command and Control shortcuts, and keys that type no
//! text, skip the input method unless it is composing: then it takes every
//! key, as Return commits, Escape cancels and the arrows pick.

/// The kind of the user's current input source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A keyboard layout, such as ABC or U.S.
    Layout,
    /// An input method or one of its modes, such as pinyin.
    InputMethod,
}

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

/// Where `press` goes, with `source` selected and the input method
/// `composing` text in the Viewer.
pub fn route(source: Source, composing: bool, press: Press) -> Route {
    if composing {
        return Route::InputMethod;
    }
    if source == Source::Layout || press.command || press.control || !types_text(press.code) {
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
            assert_eq!(route(Source::Layout, false, key(code)), Route::Raw);
        }
    }

    #[test]
    fn an_input_method_takes_the_keys_that_type_text() {
        for code in [A, N, SPACE, 18, 43] {
            assert_eq!(
                route(Source::InputMethod, false, key(code)),
                Route::InputMethod,
                "code {code}"
            );
        }
    }

    #[test]
    fn keys_with_no_text_skip_an_input_method_with_nothing_marked() {
        for code in [RETURN, ESCAPE, DELETE, 48, LEFT, 126, 115, 117, F5, 122] {
            assert_eq!(
                route(Source::InputMethod, false, key(code)),
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
        assert_eq!(route(Source::InputMethod, false, command), Route::Raw);
        assert_eq!(route(Source::InputMethod, false, control), Route::Raw);
    }

    #[test]
    fn a_composing_input_method_takes_every_key() {
        for code in [A, SPACE, RETURN, ESCAPE, DELETE, LEFT, F5] {
            assert_eq!(
                route(Source::InputMethod, true, key(code)),
                Route::InputMethod,
                "code {code}"
            );
        }
        let command = Press {
            command: true,
            ..key(A)
        };
        assert_eq!(
            route(Source::InputMethod, true, command),
            Route::InputMethod
        );
        // The user switched to a layout mid-composition: the marked text
        // still needs its Return or Escape.
        assert_eq!(route(Source::Layout, true, key(RETURN)), Route::InputMethod);
    }

    #[test]
    fn route_names_match_the_debug_protocol() {
        assert_eq!(Route::Raw.name(), "raw");
        assert_eq!(Route::InputMethod.name(), "inputMethod");
    }
}
