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
            match name.to_lowercase().as_str() {
                "ctrl" | "control" => modifiers.control = true,
                "shift" => modifiers.shift = true,
                "alt" | "option" | "opt" => modifiers.alt = true,
                "cmd" | "command" | "super" | "meta" => modifiers.command = true,
                "fn" => modifiers.function = true,
                _ => return Err(ParseError::UnknownKey((*name).into())),
            }
        }
        if let Some(code) = key_code(&key.to_lowercase()) {
            return Ok(KeyChord { code, modifiers });
        }
        // Uppercase single letters imply Shift, as in xdotool.
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && let Some(code) = key_code(&c.to_lowercase().to_string())
        {
            if c.is_uppercase() {
                modifiers.shift = true;
            }
            return Ok(KeyChord { code, modifiers });
        }
        Err(ParseError::UnknownKey((*key).into()))
    }

    /// Modifier names only, as click actions take them: ["shift", "cmd"].
    pub fn parse_modifiers(names: &[String]) -> Result<Modifiers, ParseError> {
        if names.is_empty() {
            return Ok(Modifiers::default());
        }
        Ok(KeyChord::parse(&format!("{}+a", names.join("+")))?.modifiers)
    }
}

fn key_code(name: &str) -> Option<u16> {
    const FUNCTION_KEYS: [u16; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
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
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            KeyChord::parse("f13"),
            Err(ParseError::UnknownKey("f13".into()))
        );
    }
}
