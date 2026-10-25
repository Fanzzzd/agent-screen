//! A key plus modifiers, parsed from xdotool-style names such as "Return",
//! "ctrl+c" or "cmd+shift+t", the syntax computer-use models already emit.
//! Key codes are macOS virtual key codes (US ANSI).

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub command: bool,
    pub function: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyChord {
    pub code: u16,
    pub modifiers: Modifiers,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    UnknownKey(String),
    Empty,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownKey(name) => write!(f, "unknown key \"{name}\""),
            ParseError::Empty => f.write_str("empty key chord"),
        }
    }
}

impl KeyChord {
    pub fn parse(text: &str) -> Result<KeyChord, ParseError> {
        let parts: Vec<&str> = text
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let (key, mods) = parts.split_last().ok_or(ParseError::Empty)?;
        let mut modifiers = Modifiers::default();
        for name in mods {
            match modifier(name) {
                Some((set, _)) => set(&mut modifiers),
                None => return Err(ParseError::UnknownKey((*name).into())),
            }
        }
        // A modifier on its own is a key too, e.g. "shift" or "Control_L".
        if let Some((_, code)) = modifier(key) {
            return Ok(KeyChord { code, modifiers });
        }
        // "plus" is Shift and the = key on US ANSI.
        if key.eq_ignore_ascii_case("plus") {
            modifiers.shift = true;
            return Ok(KeyChord {
                code: 24,
                modifiers,
            });
        }
        // Uppercase single letters imply Shift, as in xdotool.
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_uppercase()
        {
            modifiers.shift = true;
        }
        if let Some(code) = key_code(&key.to_lowercase()) {
            return Ok(KeyChord { code, modifiers });
        }
        Err(ParseError::UnknownKey((*key).into()))
    }

    /// Chords separated by spaces, pressed in order, as in xdotool's
    /// "ctrl+a BackSpace".
    pub fn parse_sequence(text: &str) -> Result<Vec<KeyChord>, ParseError> {
        let chords = text
            .split_whitespace()
            .map(KeyChord::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if chords.is_empty() {
            return Err(ParseError::Empty);
        }
        Ok(chords)
    }

    /// Modifier names only, as click actions take them: ["shift", "cmd"].
    pub fn parse_modifiers(names: &[String]) -> Result<Modifiers, ParseError> {
        if names.is_empty() {
            return Ok(Modifiers::default());
        }
        Ok(KeyChord::parse(&format!("{}+a", names.join("+")))?.modifiers)
    }

    /// The character a US ANSI keyboard produces for this chord, if any.
    /// Posted on the CGEvent so apps that read `event.key` (Chromium, Electron)
    /// see `]` for `cmd+]` instead of a layout-mangled glyph.
    pub fn character(self) -> Option<char> {
        let base = match self.code {
            0 => 'a',
            1 => 's',
            2 => 'd',
            3 => 'f',
            4 => 'h',
            5 => 'g',
            6 => 'z',
            7 => 'x',
            8 => 'c',
            9 => 'v',
            11 => 'b',
            12 => 'q',
            13 => 'w',
            14 => 'e',
            15 => 'r',
            16 => 'y',
            17 => 't',
            18 => '1',
            19 => '2',
            20 => '3',
            21 => '4',
            22 => '6',
            23 => '5',
            24 => '=',
            25 => '9',
            26 => '7',
            27 => '-',
            28 => '8',
            29 => '0',
            30 => ']',
            31 => 'o',
            32 => 'u',
            33 => '[',
            34 => 'i',
            35 => 'p',
            37 => 'l',
            38 => 'j',
            39 => '\'',
            40 => 'k',
            41 => ';',
            42 => '\\',
            43 => ',',
            44 => '/',
            45 => 'n',
            46 => 'm',
            47 => '.',
            49 => ' ',
            50 => '`',
            _ => return None,
        };
        Some(if self.modifiers.shift {
            shift_us(base)
        } else {
            base
        })
    }
}

fn shift_us(base: char) -> char {
    match base {
        'a'..='z' => base.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        other => other,
    }
}

/// A modifier name, with xdotool's left/right variants: how it sets
/// Modifiers, and its own key code.
type SetModifier = fn(&mut Modifiers);

fn modifier(name: &str) -> Option<(SetModifier, u16)> {
    let name = name.to_lowercase();
    let base = name
        .strip_suffix("_l")
        .or_else(|| name.strip_suffix("_r"))
        .unwrap_or(&name);
    Some(match base {
        "ctrl" | "control" => (|m: &mut Modifiers| m.control = true, 59),
        "shift" => (|m: &mut Modifiers| m.shift = true, 56),
        "alt" | "option" | "opt" => (|m: &mut Modifiers| m.alt = true, 58),
        "cmd" | "command" | "super" | "meta" => (|m: &mut Modifiers| m.command = true, 55),
        "fn" => (|m: &mut Modifiers| m.function = true, 63),
        _ => return None,
    })
}

fn key_code(name: &str) -> Option<u16> {
    const FUNCTION_KEYS: [u16; 20] = [
        122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
    ];
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return FUNCTION_KEYS.get(n.checked_sub(1)?).copied();
    }
    Some(match name {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" | "equal" => 24,
        "9" => 25,
        "7" => 26,
        "-" | "minus" => 27,
        "8" => 28,
        "0" => 29,
        "]" | "bracketright" => 30,
        "o" => 31,
        "u" => 32,
        "[" | "bracketleft" => 33,
        "i" => 34,
        "p" => 35,
        "l" => 37,
        "j" => 38,
        "'" | "apostrophe" => 39,
        "k" => 40,
        ";" | "semicolon" => 41,
        "\\" | "backslash" => 42,
        "," | "comma" => 43,
        "/" | "slash" => 44,
        "n" => 45,
        "m" => 46,
        "." | "period" => 47,
        "`" | "grave" => 50,
        "return" | "enter" => 36,
        "kp_enter" => 76,
        "tab" => 48,
        "space" => 49,
        "backspace" => 51,
        "delete" => 117,
        "escape" | "esc" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "home" => 115,
        "end" => 119,
        "page_up" | "pageup" | "prior" => 116,
        "page_down" | "pagedown" | "next" => 121,
        "insert" | "help" => 114,
        "caps_lock" | "capslock" => 57,
        _ => return None,
    })
}

/// The key presses that type `text` on a US keyboard, as a hand types it
/// into an input method, which composes from keys, not characters. The
/// first character no key types is the error.
pub fn keys_for_text(text: &str) -> Result<Vec<KeyChord>, char> {
    const SHIFTED: [(char, char); 21] = [
        ('!', '1'),
        ('@', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('^', '6'),
        ('&', '7'),
        ('*', '8'),
        ('(', '9'),
        (')', '0'),
        ('_', '-'),
        ('+', '='),
        ('{', '['),
        ('}', ']'),
        ('|', '\\'),
        (':', ';'),
        ('"', '\''),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
        ('~', '`'),
    ];
    text.chars()
        .map(|c| {
            let (name, shift) = match c {
                ' ' => ("space".to_string(), false),
                '\n' | '\r' => ("return".to_string(), false),
                '\t' => ("tab".to_string(), false),
                c if c.is_ascii_uppercase() => (c.to_ascii_lowercase().to_string(), true),
                c => match SHIFTED.iter().find(|(s, _)| *s == c) {
                    Some((_, base)) => (base.to_string(), true),
                    None => (c.to_string(), false),
                },
            };
            let code = key_code(&name).ok_or(c)?;
            Ok(KeyChord {
                code,
                modifiers: Modifiers {
                    shift,
                    ..Modifiers::default()
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_becomes_the_keys_a_hand_presses() {
        let keys = super::keys_for_text("zH !").unwrap();
        assert_eq!(
            keys.iter()
                .map(|k| (k.code, k.modifiers.shift))
                .collect::<Vec<_>>(),
            [(6, false), (4, true), (49, false), (18, true)]
        );
        assert_eq!(super::keys_for_text("中"), Err('中'));
    }

    use super::*;

    #[test]
    fn xdotool_names_models_emit() {
        let c = KeyChord::parse("Control_L+a").unwrap();
        assert!(c.modifiers.control && c.code == 0);
        let plus = KeyChord::parse("cmd+plus").unwrap();
        assert!(plus.modifiers.command && plus.modifiers.shift && plus.code == 24);
        assert_eq!(KeyChord::parse("shift").unwrap().code, 56);
        assert_eq!(KeyChord::parse("F13").unwrap().code, 105);
        assert_eq!(KeyChord::parse("Insert").unwrap().code, 114);
        let seq = KeyChord::parse_sequence("ctrl+a BackSpace").unwrap();
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[1].code, 51);
    }

    #[test]
    fn chords() {
        let chord = KeyChord::parse("cmd+shift+t").unwrap();
        assert_eq!(chord.code, 17);
        assert!(chord.modifiers.command && chord.modifiers.shift);
        assert_eq!(KeyChord::parse("Return").unwrap().code, 36);
        assert!(KeyChord::parse("A").unwrap().modifiers.shift);
        assert_eq!(KeyChord::parse("f12").unwrap().code, 111);
        assert_eq!(
            KeyChord::parse("cmd+nope"),
            Err(ParseError::UnknownKey("nope".into()))
        );
        assert_eq!(KeyChord::parse("+"), Err(ParseError::Empty));
        assert_eq!(
            KeyChord::parse("f21"),
            Err(ParseError::UnknownKey("f21".into()))
        );
    }

    #[test]
    fn bracket_keys_carry_the_right_character() {
        assert_eq!(KeyChord::parse("]").unwrap().character(), Some(']'));
        assert_eq!(
            KeyChord::parse("bracketright").unwrap().character(),
            Some(']')
        );
        assert_eq!(KeyChord::parse("cmd+]").unwrap().character(), Some(']'));
        assert_eq!(KeyChord::parse("[").unwrap().character(), Some('['));
        assert_eq!(
            KeyChord::parse("bracketleft").unwrap().character(),
            Some('[')
        );
        assert_eq!(KeyChord::parse("cmd+[").unwrap().character(), Some('['));
        assert_eq!(KeyChord::parse("shift+]").unwrap().character(), Some('}'));
        assert_eq!(KeyChord::parse("Return").unwrap().character(), None);
        assert_eq!(KeyChord::parse("Escape").unwrap().character(), None);
    }
}
