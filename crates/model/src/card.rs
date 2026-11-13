//! Which card shows under the menu bar icon. One card shows at a time, and
//! the card's kind decides what it renders, so the choice and its kind come
//! from the same place.

use crate::model::{DisplayId, Displays};
use crate::request::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// A Window or Control Request. It waits for an answer even beside the
    /// Viewer, where the user operates the display during a Takeover.
    Request(DisplayId),
    Help(DisplayId),
    /// The Input Queue's approval or grant card, drawn over a display.
    Input(DisplayId),
}

impl Card {
    pub fn display(self) -> DisplayId {
        match self {
            Self::Request(id) | Self::Help(id) | Self::Input(id) => id,
        }
    }
}

/// Window and Control Requests come first, then an eligible Help Request,
/// then the Input Queue: a queue that cannot run must not hide a Help
/// Request. Each display's request is the one `Display::asking` shows; a
/// Help Request is eligible unless the user put it off or the Viewer shows
/// its display. `input` is the display the Input Queue's card would show,
/// if it needs one. Nothing shows while Peek is open.
pub fn select<H>(
    displays: &Displays<H>,
    viewing: Option<DisplayId>,
    input: Option<DisplayId>,
    peek_shown: bool,
) -> Option<Card> {
    if peek_shown {
        return None;
    }
    let asking = || {
        displays
            .list
            .iter()
            .filter_map(|d| d.asking().map(|r| (d.id, r)))
    };
    let request = asking()
        .find(|(_, r)| r.kind() != Kind::Help)
        .map(|(id, _)| Card::Request(id));
    let help = asking()
        .find(|(id, r)| r.kind() == Kind::Help && !r.put_off && viewing != Some(*id))
        .map(|(id, _)| Card::Help(id));
    request.or(help).or(input.map(Card::Input))
}

/// Whether any display asks the user something: the menu bar icon turns
/// orange for it. The same request as the card's, put off or not.
pub fn asking<H>(displays: &Displays<H>) -> bool {
    displays.list.iter().any(|d| d.asking().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Owner;

    fn displays(ids: &[DisplayId]) -> Displays<()> {
        let mut d = Displays::default();
        for &id in ids {
            let owner = Owner {
                session_key: format!("s{id}"),
                client: "Test".into(),
                title: "Task".into(),
                connection: id.into(),
            };
            d.add(id, format!("d{id}"), (800.0, 600.0), Some(owner), ());
        }
        d
    }

    fn help(d: &mut Displays<()>, id: DisplayId) {
        d.request_help(id, "Sign in".into(), Vec::new()).unwrap();
    }

    fn control(d: &mut Displays<()>, id: DisplayId) {
        d.begin_takeover(id);
        d.request_control(id, "Hand back".into());
    }

    #[test]
    fn an_open_help_request_outranks_the_input_queue() {
        let mut d = displays(&[1, 2]);
        help(&mut d, 1);
        assert_eq!(select(&d, None, Some(2), false), Some(Card::Help(1)));
    }

    #[test]
    fn help_and_queue_on_one_display_show_the_help_card() {
        let mut d = displays(&[1]);
        help(&mut d, 1);
        let card = select(&d, None, Some(1), false);
        assert_eq!(card, Some(Card::Help(1)));
        assert_ne!(card, Some(Card::Input(1)));
    }

    #[test]
    fn window_and_control_requests_outrank_help_and_queue() {
        let mut d = displays(&[1, 2, 3]);
        help(&mut d, 1);
        // A Control Request waits during a Takeover, even in the Viewer.
        control(&mut d, 3);
        assert_eq!(select(&d, Some(3), Some(2), false), Some(Card::Request(3)));
    }

    #[test]
    fn the_queue_shows_when_no_help_is_eligible() {
        let mut d = displays(&[1, 2]);
        help(&mut d, 1);
        // Put off, in a Takeover, or open in the Viewer: not eligible.
        d.put_off_help(1);
        assert_eq!(select(&d, None, Some(2), false), Some(Card::Input(2)));
        help(&mut d, 1);
        assert_eq!(select(&d, None, Some(2), false), Some(Card::Help(1)));
        assert_eq!(select(&d, Some(1), Some(2), false), Some(Card::Input(2)));
        d.begin_takeover(1);
        assert_eq!(select(&d, None, Some(2), false), Some(Card::Input(2)));
        assert_eq!(select(&d, None, None, false), None);
    }

    #[test]
    fn the_icon_asks_for_what_the_card_would_show() {
        let mut d = displays(&[1]);
        assert!(!asking(&d));
        help(&mut d, 1);
        d.put_off_help(1);
        // Put off: no card, but the icon still asks.
        assert_eq!(select(&d, None, None, false), None);
        assert!(asking(&d));
        // In a Takeover the hand-back answers it: neither asks.
        d.begin_takeover(1);
        assert!(!asking(&d));
        control(&mut d, 1);
        assert!(asking(&d));
        assert_eq!(select(&d, None, None, false), Some(Card::Request(1)));
        d.end(1, crate::model::EndReason::Finished);
        assert!(!asking(&d));
        assert_eq!(select(&d, None, None, false), None);
    }

    #[test]
    fn peek_hides_every_card() {
        let mut d = displays(&[1, 2]);
        help(&mut d, 1);
        control(&mut d, 2);
        assert_eq!(select(&d, None, Some(1), true), None);
    }
}
