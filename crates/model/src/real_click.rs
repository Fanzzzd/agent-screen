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
/// split press, which only asks, and a complete left click. A right or
/// middle click is refused with `NO_MENU_CLICK`.
pub fn takes_real_input(action: &str) -> bool {
    matches!(action, "left_click_drag" | "left_mouse_down")
        || Click::of_action(action).is_some_and(|c| c.button == Button::Left)
}

/// Why a right, middle or Control click can't use the user's pointer.
/// The menu or popover it opens belongs to the app the loan brings to the
/// front, and closes as soon as the user's front app comes back at the
/// grant's end. The agent would never see it, and might ask the user
/// again.
pub const NO_MENU_CLICK: &str = "real_input takes left clicks without Control only: left_click, double_click and triple_click. A menu or popover that a real right, middle or Control click opens closes as soon as the user's front app comes back, so you would never see it. Nothing was sent. For a context menu, use act_on_element's show_menu on the element; for a menu bar item, act_on_element's press, which asks for the user's pointer by itself when an Electron app needs it.";

/// Whether a click with these held modifiers would open a context menu:
/// macOS takes a Control-held left click for a right click.
pub fn opens_menu(button: Button, control: bool) -> bool {
    button != Button::Left || control
}

/// The region a real click compares, just before the loan clicks, with
/// what it showed when the agent asked: the element at the point, or its
/// row or cell, as accessibility frames it. Without a usable frame, a band
/// `LOOK_WIDTH` by `LOOK_SIDE` points around the point. Never the whole
/// window: a sidebar that reorders must not refuse a click in the message
/// pane beside it.
pub const LOOK_WIDTH: f64 = 300.0;
pub const LOOK_SIDE: f64 = 64.0;
/// An element taller than this, such as a whole pane, frames too much: the
/// band is compared instead.
pub const LOOK_MAX_HEIGHT: f64 = 200.0;
/// An element smaller than this either way frames too little to compare.
pub const LOOK_MIN_SIDE: f64 = 8.0;
/// A sample changed when one of its red, green and blue channels moved by
/// more than this, out of 255. Channels, not luma: system red and green
/// differ by only 26 luma levels.
pub const LOOK_LEVELS: u8 = 32;
/// The region looks different when more than this percentage of its
/// samples changed. The pixels only back up the accessibility check, and
/// a click the user allowed should not be refused for a new time stamp or
/// badge in the row: a blinking caret is well under 1 %, another chat's
/// avatar, name and preview well over.
pub const LOOK_PERCENT: usize = 10;

/// The agent's failure when the region changed: the reply tells it to look
/// again.
pub const CONTENT_CHANGED: &str =
    "what was under the point changed while the request waited for the user";

/// The region around `at`, global points, that the loan compares: the
/// accessibility frame `element` of what is at the point, when it holds the
/// point and isn't a whole pane, else the band, either cut to the window and
/// display. In the display's own whole points from its top-left corner.
pub fn look_region(at: (f64, f64), display: Rect, window: Rect, element: Option<Rect>) -> Rect {
    let left = window.x.max(display.x);
    let right = window.max_x().min(display.max_x());
    let top = window.y.max(display.y);
    let bottom = window.max_y().min(display.max_y());
    let cut = |r: Rect| {
        let (x0, x1) = (r.x.max(left), r.max_x().min(right));
        let (y0, y1) = (r.y.max(top), r.max_y().min(bottom));
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    };
    let framed = element
        .filter(|e| e.contains(at) && e.height <= LOOK_MAX_HEIGHT)
        .map(cut)
        .filter(|r| r.width >= LOOK_MIN_SIDE && r.height >= LOOK_MIN_SIDE);
    let global = framed.unwrap_or_else(|| {
        // The band slides to stay inside the window rather than shrink.
        let slide = |center: f64, size: f64, lo: f64, hi: f64| {
            let size = size.min(hi - lo).max(0.0);
            ((center - size / 2.0).clamp(lo, (hi - size).max(lo)), size)
        };
        let (x, w) = slide(at.0, LOOK_WIDTH, left, right);
        let (y, h) = slide(at.1, LOOK_SIDE, top, bottom);
        Rect::new(x, y, w, h)
    });
    let (x0, x1) = (
        (global.x - display.x).ceil(),
        (global.max_x() - display.x).floor(),
    );
    let (y0, y1) = (
        (global.y - display.y).ceil(),
        (global.max_y() - display.y).floor(),
    );
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// How many of the region's samples, three bytes of red, green and blue
/// each, changed between `before` and `now`, and how many there are.
/// None for samples of another size, or none.
pub fn changed_samples(before: &[u8], now: &[u8]) -> Option<(usize, usize)> {
    if before.len() < 3 || before.len() != now.len() || !before.len().is_multiple_of(3) {
        return None;
    }
    let (before, _) = before.as_chunks::<3>();
    let (now, _) = now.as_chunks::<3>();
    let changed = before
        .iter()
        .zip(now)
        .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > LOOK_LEVELS))
        .count();
    Some((changed, before.len()))
}

/// Whether `now` shows what `before` showed, within `LOOK_LEVELS` and
/// `LOOK_PERCENT`. Samples of another size, or none, never match.
pub fn looks_same(before: &[u8], now: &[u8]) -> bool {
    changed_samples(before, now).is_some_and(|(changed, all)| changed * 100 <= all * LOOK_PERCENT)
}

/// What accessibility says is at the point: the hit element's role,
/// subrole, title and description, then the name of the list row, table
/// cell or link it sits in. Each is None when the app didn't answer for
/// it. In a native list the hit element is usually a static text or an
/// image whose text lives in its value, which isn't read: a preview label
/// changes with each new message. The row's name tells two rows with the
/// same avatar apart.
pub type Identity = [Option<String>; 5];
/// Where the row's name sits in an `Identity`.
pub const ITEM_NAME: usize = 4;

/// The identity, or None when the app answered for no attribute.
pub fn element_identity(parts: Identity) -> Option<Identity> {
    parts.iter().any(Option::is_some).then_some(parts)
}

/// A row's name, from its texts in depth-first pre-order: the first with a
/// letter and no digit that isn't a day word, as "Yesterday" or 昨天. A
/// time stamp ("10:30 AM", 下午3:05, "2m"), an unread count or a day would
/// change while the request waits, and a name can sit deeper in the tree
/// than they do. Only when no text qualifies, the first with a letter.
pub fn item_name(texts: &[String]) -> Option<&str> {
    let texts = || texts.iter().map(|t| t.trim());
    let letter = |t: &str| t.chars().any(char::is_alphabetic);
    texts()
        .find(|t| letter(t) && !t.chars().any(|c| c.is_ascii_digit()) && !day_word(t))
        .or_else(|| texts().find(|t| letter(t)))
}

/// A word a list shows for a day instead of a date.
fn day_word(text: &str) -> bool {
    const WORDS: [&str; 20] = [
        "now",
        "just now",
        "today",
        "yesterday",
        "tomorrow",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
        "刚刚",
        "今天",
        "昨天",
        "前天",
        "明天",
        "上午",
        "下午",
        "晚上",
    ];
    let lower = text.to_lowercase();
    if WORDS.contains(&lower.as_str()) {
        return true;
    }
    // 星期一, 周三, 礼拜天.
    let chars: Vec<char> = text.chars().collect();
    let weekday = |c: char| "一二三四五六日天".contains(c);
    match chars.as_slice() {
        ['周', d] => weekday(*d),
        ['星', '期', d] | ['礼', '拜', d] => weekday(*d),
        _ => false,
    }
}

/// Whether the element at the point may still be the one the agent aimed
/// at. Accessibility is the main guard against a list that reordered, but
/// an app that times out says nothing: an unknown element on either side,
/// or an attribute only one read returned, leaves the pixels to decide.
/// Only an attribute both reads returned, with different values, refuses.
/// When both name the row, the hit element's title and description are
/// left out: a link spanning a web row has the whole row's text there,
/// unread count and time included.
pub fn same_element(before: Option<&Identity>, now: Option<&Identity>) -> bool {
    let (Some(before), Some(now)) = (before, now) else {
        return true;
    };
    let named = before[ITEM_NAME].is_some() && now[ITEM_NAME].is_some();
    before
        .iter()
        .zip(now)
        .enumerate()
        .filter(|&(i, _)| !(named && (i == 2 || i == 3)))
        .all(|(_, pair)| match pair {
            (Some(a), Some(b)) => a == b,
            _ => true,
        })
}

/// What the success reply says of the wait, in tenths of a second.
pub fn waited_seconds(asked_ms: u64, now_ms: u64) -> f64 {
    (now_ms.saturating_sub(asked_ms) / 100) as f64 / 10.0
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
    fn left_clicks_take_real_input_and_others_do_not() {
        for action in CLICK_ACTIONS {
            assert!(Click::of_action(action).is_some(), "{action}");
        }
        for action in ["left_click", "double_click", "triple_click"] {
            assert!(takes_real_input(action), "{action}");
        }
        // The menu they open would close when the user's app comes back.
        assert!(!takes_real_input("right_click"));
        assert!(!takes_real_input("middle_click"));
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

    #[test]
    fn the_region_is_the_element_s_frame_inside_the_window() {
        let display = Rect::new(3000.0, -200.0, 1280.0, 800.0);
        let window = Rect::new(3100.0, -100.0, 900.0, 600.0);
        // A sidebar row.
        let row = Rect::new(3100.0, 0.0, 250.0, 64.0);
        assert_eq!(
            look_region((3200.0, 30.0), display, window, Some(row)),
            Rect::new(100.0, 200.0, 250.0, 64.0)
        );
        // A row reaching past the window is cut at its edge.
        let wide = Rect::new(3000.0, 0.0, 400.0, 64.0);
        assert_eq!(
            look_region((3200.0, 30.0), display, window, Some(wide)),
            Rect::new(100.0, 200.0, 300.0, 64.0)
        );
    }

    #[test]
    fn without_a_usable_frame_a_band_surrounds_the_point() {
        let display = Rect::new(3000.0, -200.0, 1280.0, 800.0);
        let window = Rect::new(3100.0, -100.0, 900.0, 600.0);
        let band = Rect::new(450.0, 268.0, 300.0, 64.0);
        let at = (3600.0, 100.0);
        assert_eq!(look_region(at, display, window, None), band);
        // A whole pane, a frame elsewhere, a speck: the band.
        let pane = Rect::new(3400.0, -100.0, 600.0, 600.0);
        assert_eq!(look_region(at, display, window, Some(pane)), band);
        let elsewhere = Rect::new(3100.0, 0.0, 250.0, 64.0);
        assert_eq!(look_region(at, display, window, Some(elsewhere)), band);
        let speck = Rect::new(3598.0, 98.0, 4.0, 4.0);
        assert_eq!(look_region(at, display, window, Some(speck)), band);
        // Near the window's corner it slides inside rather than shrinking.
        assert_eq!(
            look_region((3105.0, 495.0), display, window, None),
            Rect::new(100.0, 636.0, 300.0, 64.0)
        );
        // A small window holds a smaller band.
        let small = Rect::new(3100.0, -100.0, 200.0, 40.0);
        assert_eq!(
            look_region((3200.0, -80.0), display, small, None),
            Rect::new(100.0, 100.0, 200.0, 40.0)
        );
    }

    fn band(width: usize, rgb: [u8; 3]) -> Vec<u8> {
        rgb.repeat(width * LOOK_SIDE as usize)
    }

    fn paint(
        samples: &mut [u8],
        width: usize,
        xs: std::ops::Range<usize>,
        ys: std::ops::Range<usize>,
        rgb: [u8; 3],
    ) {
        for y in ys {
            for x in xs.clone() {
                let i = (y * width + x) * 3;
                samples[i..i + 3].copy_from_slice(&rgb);
            }
        }
    }

    #[test]
    fn a_caret_blink_looks_the_same_and_another_row_does_not() {
        let width = 250;
        let before = band(width, [240, 240, 240]);
        assert!(looks_same(&before, &before));
        // A 2 by 16 point caret appears.
        let mut caret = before.clone();
        paint(&mut caret, width, 30..32, 20..36, [20, 20, 20]);
        assert!(looks_same(&before, &caret));
        // A slightly different shade everywhere, as a hover tint.
        let tint: Vec<u8> = before.iter().map(|v| v - 20).collect();
        assert!(looks_same(&before, &tint));
        // A new time stamp and badge in the same row.
        let mut badge = before.clone();
        paint(&mut badge, width, 200..240, 8..20, [90, 90, 90]);
        paint(&mut badge, width, 220..240, 36..52, [250, 60, 50]);
        assert!(looks_same(&before, &badge));
        // Another chat: its avatar, name and preview where this one's were.
        let mut other = before.clone();
        paint(&mut other, width, 4..44, 10..50, [60, 60, 60]);
        paint(&mut other, width, 56..160, 12..24, [30, 30, 30]);
        paint(&mut other, width, 56..200, 36..46, [120, 120, 120]);
        assert!(!looks_same(&before, &other));
        assert_eq!(
            changed_samples(&before, &other).map(|c| c.1),
            Some(width * 64)
        );
        // Just past the limit, and at it.
        let limit = width * LOOK_SIDE as usize * LOOK_PERCENT / 100;
        let mut edge = before.clone();
        for v in edge.iter_mut().take((limit + 1) * 3) {
            *v = 0;
        }
        assert!(!looks_same(&before, &edge));
        edge[limit * 3..limit * 3 + 3].copy_from_slice(&[240, 240, 240]);
        assert!(looks_same(&before, &edge));
    }

    #[test]
    fn red_and_green_of_the_same_luma_differ() {
        // System red and green: 26 luma levels apart, far apart in red.
        let red = band(64, [255, 59, 48]);
        let green = band(64, [52, 199, 89]);
        assert!(!looks_same(&red, &green));
    }

    #[test]
    fn missing_or_mismatched_samples_never_match() {
        assert!(!looks_same(&[], &[]));
        assert!(!looks_same(&[1, 2, 3], &[1, 2]));
        assert!(!looks_same(&[1, 2, 3, 4], &[1, 2, 3, 4]));
    }

    /// A native list's hit element: a static text under a row, whose own
    /// text isn't read, and the row's name.
    fn text_in_row(name: &str) -> Option<Identity> {
        element_identity([
            Some("AXStaticText".into()),
            None,
            None,
            None,
            Some(name.into()),
        ])
    }

    #[test]
    fn another_row_under_the_point_is_refused_but_an_unknown_one_is_not() {
        let a = text_in_row("张三丰");
        assert!(same_element(a.as_ref(), text_in_row("张三丰").as_ref()));
        // Same placeholder avatar, a shared name prefix: the row's name
        // still differs.
        assert!(!same_element(a.as_ref(), text_in_row("张三").as_ref()));
        // A hit test that timed out, on either side: the pixels decide.
        assert!(same_element(a.as_ref(), None));
        assert!(same_element(None, a.as_ref()));
        // A row name that timed out on one read compares the role alone.
        let unnamed = element_identity([Some("AXStaticText".into()), None, None, None, None]);
        assert!(same_element(a.as_ref(), unnamed.as_ref()));
        let image = element_identity([Some("AXImage".into()), None, None, None, None]);
        assert!(!same_element(a.as_ref(), image.as_ref()));
        assert_eq!(element_identity([None, None, None, None, None]), None);
    }

    #[test]
    fn a_web_row_s_link_compares_its_name_not_its_badge() {
        let link = |title: &str, name: &str| {
            element_identity([
                Some("AXLink".into()),
                None,
                Some(title.into()),
                None,
                Some(name.into()),
            ])
        };
        let a = link("张三丰 3 unread 12:01", "张三丰");
        assert!(same_element(
            a.as_ref(),
            link("张三丰 4 unread 12:02", "张三丰").as_ref()
        ));
        assert!(!same_element(
            a.as_ref(),
            link("张三 1 unread", "张三").as_ref()
        ));
        // Without the row's name on both sides the title counts.
        let bare = |title: &str| {
            element_identity([Some("AXLink".into()), None, Some(title.into()), None, None])
        };
        assert!(!same_element(bare("a 3").as_ref(), bare("a 4").as_ref()));
    }

    #[test]
    fn a_row_s_name_skips_times_days_and_counts() {
        let texts = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // In pre-order the time and the preview come before a name nested
        // deeper: the time is skipped, and the preview, with a digit, too.
        assert_eq!(
            item_name(&texts(&["10:30 AM", "3", "see you at 5", "Alice"])),
            Some("Alice")
        );
        assert_eq!(
            item_name(&texts(&[
                "下午3:05",
                "昨天",
                "星期三",
                "周五",
                " 张三丰 ",
                "你好"
            ])),
            Some("张三丰")
        );
        assert_eq!(item_name(&texts(&["2m", "Yesterday", "Bob"])), Some("Bob"));
        // A surname is not a weekday.
        assert_eq!(item_name(&texts(&["周杰"])), Some("周杰"));
        // Nothing qualifies: the first text with a letter.
        assert_eq!(item_name(&texts(&["12:01", "Room 101"])), Some("Room 101"));
        assert_eq!(item_name(&texts(&["12:01", "3"])), None);
        assert_eq!(item_name(&[]), None);
    }

    #[test]
    fn control_and_other_buttons_open_menus() {
        assert!(!opens_menu(Button::Left, false));
        assert!(opens_menu(Button::Left, true));
        assert!(opens_menu(Button::Right, false));
        assert!(opens_menu(Button::Middle, false));
    }

    #[test]
    fn the_wait_is_reported_in_tenths_of_a_second() {
        assert_eq!(waited_seconds(1_000, 13_470), 12.4);
        assert_eq!(waited_seconds(5_000, 4_000), 0.0);
    }
}
