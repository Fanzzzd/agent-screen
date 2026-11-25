//! Whether every display shares one macOS Space: "Displays have separate
//! Spaces" off in System Settings > Desktop & Dock > Mission Control, as
//! many AeroSpace users run it. Then a desktop switch on the user's screen
//! switches every Agent Display too, and the agents' windows leave their
//! displays until the user switches back. Agent Screen only says so: to
//! the agent at create_display, and to the user once, on a card. It never
//! changes the setting, which takes a logout.

/// What macOS says of its Spaces, read without changing anything: the
/// window server's Space management mode (1 with separate Spaces, 0
/// without), the display identifiers of its managed Spaces list, and the
/// `com.apple.spaces spans-displays` setting. Each is None when unread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub mode: Option<i32>,
    pub entries: Option<Vec<String>>,
    pub spans_displays: Option<bool>,
}

impl Facts {
    /// Whether the displays share one Space now. The window server's live
    /// answer goes first: the setting changes only at the next login, so
    /// it decides only when the window server gives none. Its Spaces list
    /// then has one entry, "Main", for every display.
    pub fn shared(&self) -> bool {
        match self.mode {
            Some(1) => return false,
            Some(0) => return true,
            _ => {}
        }
        match self.entries.as_deref() {
            Some([only]) if only.eq_ignore_ascii_case("main") => true,
            Some(entries) if !entries.is_empty() => false,
            _ => self.spans_displays == Some(true),
        }
    }
}

/// create_display's `spaces` note when the displays share one Space.
pub const SHARED_NOTE: &str = "\"Displays have separate Spaces\" is off on this Mac, so this display shares one macOS Space with the user's screens. When the user switches desktops, your windows leave the display until they switch back: tools then report a window as on another Space, not closed. Agent Screen doesn't switch Spaces. If that happens, ask the user to switch back. The user can turn the setting on in System Settings > Desktop & Dock > Mission Control, then log out and back in";

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(mode: Option<i32>, entries: Option<&[&str]>, spans: Option<bool>) -> Facts {
        Facts {
            mode,
            entries: entries.map(|e| e.iter().map(|s| s.to_string()).collect()),
            spans_displays: spans,
        }
    }

    #[test]
    fn the_window_servers_mode_decides_first() {
        for spans in [None, Some(false), Some(true)] {
            for entries in [None, Some(&["Main"][..]), Some(&["A-UUID", "B-UUID"][..])] {
                assert!(!facts(Some(1), entries, spans).shared());
                assert!(facts(Some(0), entries, spans).shared());
            }
        }
    }

    #[test]
    fn without_a_mode_one_main_entry_means_shared() {
        assert!(facts(None, Some(&["Main"]), None).shared());
        assert!(facts(Some(7), Some(&["main"]), Some(false)).shared());
        assert!(!facts(None, Some(&["37D8832A-2D66"]), Some(true)).shared());
        assert!(!facts(None, Some(&["A-UUID", "B-UUID"]), Some(true)).shared());
        assert!(!facts(None, Some(&["Main", "B-UUID"]), None).shared());
    }

    #[test]
    fn the_setting_decides_only_when_the_window_server_says_nothing() {
        assert!(facts(None, None, Some(true)).shared());
        assert!(facts(None, Some(&[]), Some(true)).shared());
        assert!(!facts(None, None, Some(false)).shared());
        assert!(!facts(None, None, None).shared());
        assert!(!Facts::default().shared());
    }

    #[test]
    fn the_note_names_the_setting_and_never_promises_a_switch() {
        assert!(SHARED_NOTE.contains("Displays have separate Spaces"));
        assert!(SHARED_NOTE.contains("Desktop & Dock > Mission Control"));
        assert!(SHARED_NOTE.contains("doesn't switch Spaces"));
        assert!(SHARED_NOTE.contains("log out"));
    }
}
