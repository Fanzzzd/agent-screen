//! What Agent Screen tells a window manager is its own: the displays it made
//! and the windows on them that are the agents'. AeroSpace leaves both alone
//! while the claim is live (the fork's SIDESCREEN.md, Claims).

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Claims {
    displays: BTreeSet<u32>,
    /// Each claimed window, by the display it is on or on its way to.
    windows: BTreeMap<u32, u32>,
}

impl Claims {
    pub const fn new() -> Self {
        Claims {
            displays: BTreeSet::new(),
            windows: BTreeMap::new(),
        }
    }

    pub fn add_display(&mut self, display: u32) -> bool {
        self.displays.insert(display)
    }

    /// A display gone: its windows are nobody's to claim any more.
    pub fn remove_display(&mut self, display: u32) -> bool {
        let had = self.displays.remove(&display);
        let before = self.windows.len();
        self.windows.retain(|_, d| *d != display);
        had || self.windows.len() != before
    }

    pub fn display_of(&self, window: u32) -> Option<u32> {
        self.windows.get(&window).copied()
    }

    pub fn has_display(&self, display: u32) -> bool {
        self.displays.contains(&display)
    }

    /// Claim a window going onto one of the claimed displays. A window going
    /// anywhere else isn't claimed.
    pub fn claim(&mut self, window: u32, display: u32) -> bool {
        self.displays.contains(&display) && self.windows.insert(window, display) != Some(display)
    }

    pub fn release(&mut self, window: u32) -> bool {
        self.windows.remove(&window).is_some()
    }

    /// Nothing to claim: the claim file should not exist.
    pub fn is_empty(&self) -> bool {
        self.displays.is_empty() && self.windows.is_empty()
    }

    /// The claim file's contents, version 1.
    pub fn to_json(&self) -> String {
        let list = |ids: &mut dyn Iterator<Item = u32>| {
            ids.map(|id| id.to_string()).collect::<Vec<_>>().join(",")
        };
        format!(
            "{{\"version\":1,\"displays\":[{}],\"windows\":[{}]}}",
            list(&mut self.displays.iter().copied()),
            list(&mut self.windows.keys().copied())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_only_windows_going_onto_its_displays() {
        let mut claims = Claims::default();
        assert!(!claims.claim(7, 3));
        claims.add_display(3);
        assert!(claims.claim(7, 3));
        assert!(!claims.claim(7, 3));
        assert_eq!(claims.display_of(7), Some(3));
        assert_eq!(
            claims.to_json(),
            r#"{"version":1,"displays":[3],"windows":[7]}"#
        );
    }

    #[test]
    fn a_display_going_takes_its_windows_claims_with_it() {
        let mut claims = Claims::default();
        claims.add_display(3);
        claims.add_display(4);
        claims.claim(7, 3);
        claims.claim(8, 4);
        assert!(claims.remove_display(3));
        assert_eq!(
            claims.to_json(),
            r#"{"version":1,"displays":[4],"windows":[8]}"#
        );
        claims.release(8);
        claims.remove_display(4);
        assert!(claims.is_empty());
    }
}
