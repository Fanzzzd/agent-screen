//! Which pop-up menus on an Agent Display count as open for the agent's
//! next input (#48, #79, #276). macOS closes an app's open menu at the
//! user's next click or key anywhere, so a click or key the agent meant
//! for a menu that closed since its last call would land on the window
//! under it, and is refused instead.
//!
//! A menu closed by a choice stays on screen a moment: AppKit flashes the
//! chosen item for about 70 ms with the menu opaque, then fades the menu
//! out over about 250 ms. Seen in that time, right after the agent's own
//! click, Return or press chose an item, it looked open, and the agent's
//! next input was refused as if the user had closed it. A menu is open
//! only while opaque, and after an input a menu that was open at the
//! agent's last call gets a moment to show it is closing.
//!
//! Chrome drops a click that comes while the menu a Return closed is
//! still fading: AppKit has not ended the menu's session yet. So the
//! agent's next click or key waits until a menu its last input closed
//! is gone, 400 ms at most.

/// A pop-up menu window on the display, as WindowServer lists it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Menu {
    pub window: u32,
    pub alpha: f64,
}

/// How long, after an input, a menu that was open at the agent's last
/// call gets to start fading out, in ms: the chosen item's flash and some
/// margin.
pub const CLOSING_WAIT_MS: u64 = 300;

/// How often to look again during that wait, in ms.
pub const LOOK_EVERY_MS: u64 = 15;

/// How long the agent's next input waits at most for a menu its last
/// input closed to fade out, in ms: the fade and some margin.
pub const FADED_WAIT_MS: u64 = 400;

/// Whether `menu` is open: a menu fading out, after a choice or Escape,
/// is closing.
pub fn is_open(menu: &Menu) -> bool {
    menu.alpha >= 0.99
}

/// The open menus among `listed`.
pub fn open(listed: &[Menu]) -> Vec<u32> {
    listed
        .iter()
        .filter(|m| is_open(m))
        .map(|m| m.window)
        .collect()
}

/// The menus to remember after an input, from what is listed now, or None
/// to look again: a menu open at the agent's last call (`seen`) that is
/// still opaque may be flashing the item the input chose. Once the wait
/// is over (`waited`), what is open stays open, as a menu after an arrow
/// key, a typed letter or a click on a submenu's item.
pub fn after_input(seen: &[u32], listed: &[Menu], waited: bool) -> Option<Vec<u32>> {
    let now = open(listed);
    if !waited && now.iter().any(|w| seen.contains(w)) {
        return None;
    }
    Some(now)
}

/// The menus an input closed: open before it (`seen`) and not after.
pub fn closed_by(seen: &[u32], after: &[u32]) -> Vec<u32> {
    seen.iter()
        .filter(|w| !after.contains(w))
        .copied()
        .collect()
}

/// Whether a menu the agent's last input closed (`closed`) is still
/// fading out, so its next input should wait: a click then gets lost.
pub fn closing(closed: &[u32], listed: &[Menu]) -> bool {
    listed
        .iter()
        .any(|m| closed.contains(&m.window) && !is_open(m))
}

/// Whether the agent's input may go out: no menu was open at its last
/// call, or one of those is still open. Otherwise the menu it was meant
/// for closed in between, and the input would land under it.
pub fn input_may_go(seen: &[u32], listed: &[Menu]) -> bool {
    seen.is_empty() || open(listed).iter().any(|w| seen.contains(w))
}

/// Whether text the agent typed went to an open menu rather than a field:
/// a menu was open before the keys and still is. Type-ahead in a menu
/// moves its highlight and chooses nothing.
pub fn typed_into_menu(seen: &[u32], after: &[u32]) -> bool {
    after.iter().any(|w| seen.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(window: u32, alpha: f64) -> Menu {
        Menu { window, alpha }
    }

    #[test]
    fn a_fading_menu_is_closing() {
        assert!(is_open(&menu(1, 1.0)));
        assert!(!is_open(&menu(1, 0.92)));
        assert_eq!(open(&[menu(1, 0.5), menu(2, 1.0)]), vec![2]);
    }

    #[test]
    fn a_chosen_item_flashing_gets_a_look_again() {
        // Right after Return chose an item, the menu is still opaque.
        assert_eq!(after_input(&[7], &[menu(7, 1.0)], false), None);
        // Then it fades: nothing is open, and the next input goes out.
        assert_eq!(after_input(&[7], &[menu(7, 0.6)], false), Some(vec![]));
        assert_eq!(after_input(&[7], &[], false), Some(vec![]));
    }

    #[test]
    fn a_menu_that_stays_open_is_remembered_after_the_wait() {
        // An arrow key moved the highlight; the menu stays.
        assert_eq!(after_input(&[7], &[menu(7, 1.0)], true), Some(vec![7]));
    }

    #[test]
    fn a_menu_the_input_opened_is_remembered_at_once() {
        // A click opened a select's menu: no menu was open before it.
        assert_eq!(after_input(&[], &[menu(9, 1.0)], false), Some(vec![9]));
        // A click on an item that opens a submenu, while the first fades.
        assert_eq!(
            after_input(&[7], &[menu(7, 0.4), menu(9, 1.0)], false),
            Some(vec![9])
        );
    }

    #[test]
    fn input_goes_out_only_while_the_menu_it_was_meant_for_is_open() {
        assert!(input_may_go(&[], &[]));
        assert!(input_may_go(&[7], &[menu(7, 1.0)]));
        // The user's click closed it: gone, or still fading.
        assert!(!input_may_go(&[7], &[]));
        assert!(!input_may_go(&[7], &[menu(7, 0.3)]));
        // Another menu opened in its place.
        assert!(!input_may_go(&[7], &[menu(8, 1.0)]));
    }

    #[test]
    fn a_menu_the_input_closed_is_the_one_open_before_and_not_after() {
        assert_eq!(closed_by(&[7], &[]), vec![7]);
        assert_eq!(closed_by(&[7], &[7]), Vec::<u32>::new());
        assert_eq!(closed_by(&[7], &[9]), vec![7]);
        assert_eq!(closed_by(&[], &[9]), Vec::<u32>::new());
    }

    #[test]
    fn the_next_input_waits_while_the_menu_it_closed_fades() {
        // Return chose an item; the menu is still fading out.
        assert!(closing(&[7], &[menu(7, 0.96)]));
        // Gone: the input goes out.
        assert!(!closing(&[7], &[]));
        // Opaque again, or never closed: nothing to wait for.
        assert!(!closing(&[7], &[menu(7, 1.0)]));
        // A menu the last input did not close, fading or not.
        assert!(!closing(&[7], &[menu(8, 0.5)]));
        assert!(!closing(&[], &[menu(8, 0.5)]));
    }

    #[test]
    fn typing_into_a_menu_that_stays_open_is_noted() {
        assert!(typed_into_menu(&[7], &[7]));
        assert!(!typed_into_menu(&[7], &[]));
        assert!(!typed_into_menu(&[], &[9]));
    }
}
