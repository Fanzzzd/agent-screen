//! A press on a menu item of an Electron app (issue #39, ADR 0013's "Menu
//! items of Electron apps"). AppKit runs an accessibility press with no
//! current event, and Electron reads anything but a mouse click as the
//! item's keyboard shortcut (`triggeredByAccelerator`). Apps that drop
//! shortcuts while a text field has focus then ignore the press. Only a
//! real click on the open menu reaches the item as a click, and that takes
//! the user's pointer, so it goes through the Input Queue.

use crate::model::Rect;

/// How a press on a menu item goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Accessibility's AXPress, in the background.
    Press,
    /// A real click on the Agent Display's menu bar and the open menu,
    /// once the user allows it.
    RealClick,
}

/// What the app and the agent's call say about one press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Press {
    /// An item of the app's own menu bar, not of a context or pop-up menu.
    pub in_menu_bar: bool,
    /// Under the Apple menu, which is the system's, not the app's.
    pub apple_menu: bool,
    pub electron: bool,
    /// The app's focused element takes text.
    pub editing: bool,
    /// The call's `real_input`, if it said.
    pub real_input: Option<bool>,
}

impl Press {
    /// A real click only for an item of an Electron app's own menu bar:
    /// asked for with `real_input: true`, or by default while a text field
    /// has focus, where the press would arrive as an ignored shortcut.
    pub fn route(&self) -> Route {
        let eligible = self.electron && self.in_menu_bar && !self.apple_menu;
        match self.real_input {
            _ if !eligible => Route::Press,
            Some(true) => Route::RealClick,
            Some(false) => Route::Press,
            None if self.editing => Route::RealClick,
            None => Route::Press,
        }
    }
}

/// Where to click a menu bar title on the Agent Display's own menu bar.
/// macOS lays out every display's menu bar from its left edge, so a title
/// accessibility places at `title` on the display `screen` sits at the
/// same offset on `display`. A title already on `display` stays put.
pub fn title_point(title: Rect, screen: Rect, display: Rect) -> Option<(f64, f64)> {
    let (x, y) = title.mid();
    let (dx, dy) = if display.contains((x, y)) {
        (0.0, 0.0)
    } else {
        (display.x - screen.x, display.y - screen.y)
    };
    let at = (x + dx, y + dy);
    // On the menu bar, which is at most a notch's height tall.
    (display.contains(at) && at.1 - display.y < 40.0).then_some(at)
}

/// A menu the click opened lies wholly on the Agent Display, so the item
/// clicked next is there and not on the user's screen.
pub fn on_display(menu: Rect, display: Rect) -> bool {
    menu.width > 0.0
        && menu.height > 0.0
        && menu.x >= display.x
        && menu.y >= display.y
        && menu.max_x() <= display.max_x() + 0.5
        && menu.max_y() <= display.max_y() + 0.5
}

/// The item path as people read it: `Go > Previous view`.
pub fn label(titles: &[String]) -> String {
    titles.join(" > ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(electron: bool, editing: bool, real_input: Option<bool>) -> Press {
        Press {
            in_menu_bar: true,
            apple_menu: false,
            electron,
            editing,
            real_input,
        }
    }

    #[test]
    fn an_electron_item_is_clicked_for_real_only_while_a_field_has_focus() {
        assert_eq!(press(true, false, None).route(), Route::Press);
        assert_eq!(press(true, true, None).route(), Route::RealClick);
    }

    #[test]
    fn real_input_forces_either_way() {
        assert_eq!(press(true, false, Some(true)).route(), Route::RealClick);
        assert_eq!(press(true, true, Some(false)).route(), Route::Press);
    }

    #[test]
    fn other_apps_context_menus_and_the_apple_menu_keep_accessibility() {
        assert_eq!(press(false, true, Some(true)).route(), Route::Press);
        let context = Press {
            in_menu_bar: false,
            ..press(true, true, Some(true))
        };
        assert_eq!(context.route(), Route::Press);
        let apple = Press {
            apple_menu: true,
            ..press(true, true, Some(true))
        };
        assert_eq!(apple.route(), Route::Press);
    }

    #[test]
    fn a_title_on_another_screen_moves_to_the_same_offset_on_the_display() {
        let main = Rect::new(0.0, 0.0, 1512.0, 982.0);
        let display = Rect::new(3000.0, -1200.0, 1100.0, 700.0);
        let go = Rect::new(240.0, 0.0, 30.0, 24.0);
        assert_eq!(title_point(go, main, display), Some((3255.0, -1188.0)));
        let here = go.offset(3000.0, -1200.0);
        assert_eq!(title_point(here, display, display), Some((3255.0, -1188.0)));
    }

    #[test]
    fn a_title_past_the_display_or_below_its_menu_bar_has_no_point() {
        let main = Rect::new(0.0, 0.0, 3000.0, 982.0);
        let display = Rect::new(3000.0, 0.0, 800.0, 600.0);
        assert_eq!(
            title_point(Rect::new(1200.0, 0.0, 40.0, 24.0), main, display),
            None
        );
        assert_eq!(
            title_point(Rect::new(100.0, 300.0, 40.0, 24.0), main, display),
            None
        );
    }

    #[test]
    fn a_menu_counts_as_on_the_display_only_inside_it() {
        let display = Rect::new(3000.0, 0.0, 800.0, 600.0);
        assert!(on_display(Rect::new(3240.0, 24.0, 200.0, 300.0), display));
        assert!(!on_display(Rect::new(240.0, 24.0, 200.0, 300.0), display));
        assert!(!on_display(Rect::new(3700.0, 24.0, 200.0, 300.0), display));
        assert!(!on_display(Rect::new(3240.0, 24.0, 0.0, 0.0), display));
    }

    #[test]
    fn a_closed_electron_submenu_is_not_open_on_the_display() {
        // What a closed submenu of an Electron app reports: 10×10 at the
        // main screen's bottom left (#39).
        let display = Rect::new(8288.0, -700.0, 1100.0, 700.0);
        assert!(!on_display(Rect::new(0.0, 1117.0, 10.0, 10.0), display));
    }
}
