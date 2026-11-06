//! A click with the user's real pointer (ADR 0013, "Clicks"). Some apps
//! take a click sent in the background only partway: WeCom selects a
//! sidebar row and never opens the conversation (issue #134). Agent Screen
//! can't tell that a click had no effect; the agent sees it in a
//! screenshot and asks again with `real_input: true`. That click goes
//! through the Input Queue like a drag, and only once the user allows it.

use crate::model::Rect;

/// The `computer` actions that are one complete click.
pub const CLICK_ACTIONS: [&str; 5] = [
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "triple_click",
];

/// How long a loan holds each press, and waits after each release, in ms.
pub const HOLD_MS: u64 = 50;
pub const GAP_MS: u64 = 40;
// A triple click fits the shortest double-click interval: macOS's
// double-click speed setting goes down to about 0.15 s between presses.
const _: () = assert!(HOLD_MS + GAP_MS < 150);

/// The `computer` actions that accept `real_input`: a complete drag, a
/// split press, which only asks, and a complete click.
pub fn takes_real_input(action: &str) -> bool {
    matches!(action, "left_click_drag" | "left_mouse_down") || CLICK_ACTIONS.contains(&action)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(name: &str) -> Option<Button> {
        match name {
            "left" => Some(Button::Left),
            "right" => Some(Button::Right),
            "middle" => Some(Button::Middle),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        }
    }
}

/// What the approval card calls a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Click,
    DoubleClick,
    TripleClick,
    RightClick,
    MiddleClick,
}

/// One event of a click, in the order it goes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The pointer goes to the point.
    Move,
    /// A press, with its click state: 1 for the first press, 2 for the
    /// second of a double click.
    Down(i64),
    /// The release of the press with the same click state.
    Up(i64),
}

/// One complete click: a button pressed and released `count` times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Click {
    pub button: Button,
    pub count: u8,
}

impl Click {
    /// A click the queue can run: one to three presses of one button.
    pub fn new(button: Button, count: i64) -> Option<Click> {
        let count = u8::try_from(count).ok().filter(|c| (1..=3).contains(c))?;
        Some(Click { button, count })
    }

    /// The click a `computer` action makes.
    pub fn of_action(action: &str) -> Option<Click> {
        let (button, count) = match action {
            "left_click" => (Button::Left, 1),
            "right_click" => (Button::Right, 1),
            "middle_click" => (Button::Middle, 1),
            "double_click" => (Button::Left, 2),
            "triple_click" => (Button::Left, 3),
            _ => return None,
        };
        Click::new(button, count)
    }

    /// The pointer moves to the point first. Then each press is released
    /// before the next, and each pair carries the next click state, so the
    /// app reads a double click as one.
    pub fn events(self) -> Vec<Event> {
        let mut events = vec![Event::Move];
        for state in 1..=i64::from(self.count) {
            events.push(Event::Down(state));
            events.push(Event::Up(state));
        }
        events
    }

    pub fn verb(self) -> Verb {
        match (self.button, self.count) {
            (Button::Right, _) => Verb::RightClick,
            (Button::Middle, _) => Verb::MiddleClick,
            (Button::Left, 2) => Verb::DoubleClick,
            (Button::Left, 3) => Verb::TripleClick,
            (Button::Left, _) => Verb::Click,
        }
    }
}

/// The queue's token for a click: the same click on the same window, at
/// the same whole point with the same modifiers, finds its request again.
pub fn operation(window: u32, at: (f64, f64), click: Click, modifiers: i64) -> String {
    format!(
        "click:{window}:{},{}:{}:{}:{modifiers}",
        at.0.round() as i64,
        at.1.round() as i64,
        click.button.name(),
        click.count
    )
}

/// A click goes only where the agent may click: inside its Agent Display
/// and inside the window it aimed at, both in global points.
pub fn contained(at: (f64, f64), display: Rect, window: Rect) -> bool {
    display.contains(at) && window.contains(at)
}

/// The point as the card shows it, in whole points from the display's
/// top-left corner.
pub fn display_point(at: (f64, f64), display: Rect) -> (i64, i64) {
    (
        (at.0 - display.x).round() as i64,
        (at.1 - display.y).round() as i64,
    )
}

/// A window title short enough for the card's one line, or None for an
/// untitled window.
pub fn short_title(title: &str, max: usize) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    if title.chars().count() <= max {
        return Some(title);
    }
    let cut: String = title.chars().take(max.saturating_sub(1)).collect();
    Some(format!("{}…", cut.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_click_action_takes_real_input_and_others_do_not() {
        for action in CLICK_ACTIONS {
            assert!(takes_real_input(action), "{action}");
            assert!(Click::of_action(action).is_some(), "{action}");
        }
        assert!(takes_real_input("left_click_drag"));
        assert!(takes_real_input("left_mouse_down"));
        for action in ["scroll", "type", "key", "mouse_move", "left_mouse_up"] {
            assert!(!takes_real_input(action), "{action}");
        }
    }

    #[test]
    fn actions_map_to_their_button_and_count() {
        let of = |a| Click::of_action(a).map(|c| (c.button, c.count));
        assert_eq!(of("left_click"), Some((Button::Left, 1)));
        assert_eq!(of("right_click"), Some((Button::Right, 1)));
        assert_eq!(of("middle_click"), Some((Button::Middle, 1)));
        assert_eq!(of("double_click"), Some((Button::Left, 2)));
        assert_eq!(of("triple_click"), Some((Button::Left, 3)));
        assert_eq!(of("scroll"), None);
    }

    #[test]
    fn only_one_to_three_presses_run() {
        assert!(Click::new(Button::Left, 0).is_none());
        assert!(Click::new(Button::Left, 4).is_none());
        assert!(Click::new(Button::Left, -1).is_none());
        assert!(Click::new(Button::Right, 1).is_some());
    }

    #[test]
    fn a_double_click_moves_first_and_releases_each_press_before_the_next() {
        let click = Click::of_action("double_click").unwrap();
        assert_eq!(
            click.events(),
            [
                Event::Move,
                Event::Down(1),
                Event::Up(1),
                Event::Down(2),
                Event::Up(2)
            ]
        );
        let triple = Click::of_action("triple_click").unwrap().events();
        let mut held = None;
        for event in &triple[1..] {
            match *event {
                Event::Down(state) => {
                    assert_eq!(held, None, "a press while another is held");
                    held = Some(state);
                }
                Event::Up(state) => assert_eq!(held.take(), Some(state)),
                Event::Move => panic!("a move between presses"),
            }
        }
        assert_eq!(held, None, "the last press is released");
    }

    #[test]
    fn verbs_name_the_button_and_count() {
        let verb = |a| Click::of_action(a).unwrap().verb();
        assert_eq!(verb("left_click"), Verb::Click);
        assert_eq!(verb("double_click"), Verb::DoubleClick);
        assert_eq!(verb("triple_click"), Verb::TripleClick);
        assert_eq!(verb("right_click"), Verb::RightClick);
        assert_eq!(verb("middle_click"), Verb::MiddleClick);
    }

    #[test]
    fn the_same_click_has_the_same_token_and_another_does_not() {
        let click = Click::of_action("left_click").unwrap();
        let a = operation(7, (100.2, 200.4), click, 0);
        assert_eq!(a, operation(7, (99.8, 199.6), click, 0));
        assert_ne!(a, operation(8, (100.0, 200.0), click, 0));
        assert_ne!(a, operation(7, (100.0, 200.0), click, 8));
        let double = Click::of_action("double_click").unwrap();
        assert_ne!(a, operation(7, (100.0, 200.0), double, 0));
        let right = Click::of_action("right_click").unwrap();
        assert_ne!(a, operation(7, (100.0, 200.0), right, 0));
    }

    #[test]
    fn a_click_stays_inside_its_display_and_window() {
        let display = Rect::new(3000.0, 0.0, 1280.0, 800.0);
        let window = Rect::new(3100.0, 100.0, 400.0, 300.0);
        assert!(contained((3300.0, 250.0), display, window));
        assert!(!contained((3050.0, 250.0), display, window));
        assert!(!contained((2900.0, 250.0), display, window));
        let spilling = Rect::new(2900.0, 100.0, 400.0, 300.0);
        assert!(!contained((2950.0, 250.0), display, spilling));
    }

    #[test]
    fn the_card_shows_points_from_the_display_corner() {
        let display = Rect::new(3000.0, -200.0, 1280.0, 800.0);
        assert_eq!(display_point((3412.4, 30.6), display), (412, 231));
    }

    #[test]
    fn titles_fit_one_line() {
        assert_eq!(short_title("  ", 10), None);
        assert_eq!(short_title(" Chats \n", 10), Some("Chats".into()));
        assert_eq!(
            short_title("A very long window title", 10),
            Some("A very lo…".into())
        );
        assert_eq!(short_title("企业微信", 10), Some("企业微信".into()));
    }
}
