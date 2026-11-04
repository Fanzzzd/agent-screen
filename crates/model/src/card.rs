//! Which card shows under the menu bar icon. One card shows at a time, and
//! the card's kind decides what it renders, so the choice and its kind come
//! from the same place.

use crate::model::{DisplayId, Displays};

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
/// Request. `later` says whether the user put this display's Help Request
/// aside; `input` is the display the Input Queue's card would show, if it
/// needs one. Nothing shows while Peek is open.
pub fn select<H>(
    displays: &Displays<H>,
    viewing: Option<DisplayId>,
    later: impl Fn(DisplayId, &str) -> bool,
    input: Option<DisplayId>,
    peek_shown: bool,
) -> Option<Card> {
    if peek_shown {
        return None;
    }
    let request = displays
        .list
        .iter()
        .find(|d| d.is_live() && (d.window_request.is_some() || d.control_request.is_some()))
        .map(|d| Card::Request(d.id));
    let help = displays
        .list
        .iter()
        .find(|d| {
            d.is_live()
                && !d.takeover
                && viewing != Some(d.id)
                && d.help_request.as_deref().is_some_and(|r| !later(d.id, r))
        })
        .map(|d| Card::Help(d.id));
    request.or(help).or(input.map(Card::Input))
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
        d.get_mut(id).unwrap().help_request = Some("Sign in".into());
    }

    fn never(_: DisplayId, _: &str) -> bool {
        false
    }

    #[test]
    fn an_open_help_request_outranks_the_input_queue() {
        let mut d = displays(&[1, 2]);
        help(&mut d, 1);
        assert_eq!(select(&d, None, never, Some(2), false), Some(Card::Help(1)));
    }

    #[test]
    fn help_and_queue_on_one_display_show_the_help_card() {
        let mut d = displays(&[1]);
        help(&mut d, 1);
        let card = select(&d, None, never, Some(1), false);
        assert_eq!(card, Some(Card::Help(1)));
        assert_ne!(card, Some(Card::Input(1)));
    }

    #[test]
    fn window_and_control_requests_outrank_help_and_queue() {
        let mut d = displays(&[1, 2, 3]);
        help(&mut d, 1);
        d.get_mut(3).unwrap().control_request = Some("Hand back".into());
        // A Control Request waits during a Takeover, even in the Viewer.
        d.get_mut(3).unwrap().takeover = true;
        assert_eq!(
            select(&d, Some(3), never, Some(2), false),
            Some(Card::Request(3))
        );
    }

    #[test]
    fn the_queue_shows_when_no_help_is_eligible() {
        let mut d = displays(&[1, 2]);
        help(&mut d, 1);
        // Put aside, in a Takeover, or open in the Viewer: not eligible.
        let aside = |id, r: &str| id == 1 && r == "Sign in";
        assert_eq!(
            select(&d, None, aside, Some(2), false),
            Some(Card::Input(2))
        );
        assert_eq!(
            select(&d, Some(1), never, Some(2), false),
            Some(Card::Input(2))
        );
        d.get_mut(1).unwrap().takeover = true;
        assert_eq!(
            select(&d, None, never, Some(2), false),
            Some(Card::Input(2))
        );
        assert_eq!(select(&d, None, never, None, false), None);
    }

    #[test]
    fn peek_hides_every_card() {
        let mut d = displays(&[1, 2]);
        help(&mut d, 1);
        d.get_mut(2).unwrap().control_request = Some("Hand back".into());
        assert_eq!(select(&d, None, never, Some(1), true), None);
    }
}
