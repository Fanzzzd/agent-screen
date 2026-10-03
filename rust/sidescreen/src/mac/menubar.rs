//! The menus of the app in front on an Agent Display, opened from the
//! Viewer's menu bar strip.
//!
//! That strip mirrors the menu bar of the user's own front app, not the
//! display's, and posted clicks never open a menu bar anyway. So a click
//! there shows the display's front app menus as one native menu, read
//! through accessibility, and the chosen item is pressed.

use super::ax::{self, Element};
use super::native::{Menu, MenuAction, set_checked, set_key_equivalent};
use super::windows;
use crate::model::DisplayId;
use objc2_core_foundation::{CFBoolean, CFNumber};
use std::cell::RefCell;

thread_local! {
    /// Elements of the menu last shown, by `PressProxy` index.
    static ITEMS: RefCell<Vec<Element>> = const { RefCell::new(Vec::new()) };
}

/// The front app's menus on `display`, one submenu per menu title, or None
/// when no app there has any.
pub fn menu(display: DisplayId) -> Option<Menu> {
    let window = windows::on_display(display).into_iter().next()?;
    let app = ax::application(window.target.pid);
    let bar = ax::element_attribute(&app, "AXMenuBar")?;
    let menu = Menu::new(&window.app);
    menu.disabled(&window.app);
    ITEMS.with(|items| items.borrow_mut().clear());
    let mut titles = 0;
    // The first title is the Apple menu, which belongs to the system.
    for item in ax::elements_attribute(&bar, "AXChildren")
        .into_iter()
        .skip(1)
    {
        let Some(submenu) = ax::elements_attribute(&item, "AXChildren")
            .into_iter()
            .next()
        else {
            continue;
        };
        let title = ax::string_attribute(&item, "AXTitle").unwrap_or_default();
        let content = Menu::new(&title);
        fill(&content, &submenu);
        menu.submenu(&title, content);
        titles += 1;
    }
    (titles > 0).then_some(menu)
}

fn fill(menu: &Menu, ax_menu: &Element) {
    for element in ax::elements_attribute(ax_menu, "AXChildren") {
        let name = ax::string_attribute(&element, "AXTitle").unwrap_or_default();
        if name.is_empty() {
            menu.separator();
            continue;
        }
        let children = ax::elements_attribute(&element, "AXChildren");
        let item = if let Some(sub) = children.first() {
            let submenu = Menu::new(&name);
            fill(&submenu, sub);
            menu.submenu(&name, submenu)
        } else {
            let index = ITEMS.with(|items| {
                let mut items = items.borrow_mut();
                items.push(element.clone());
                items.len() - 1
            });
            menu.action(&name, MenuAction::PressProxy { index })
        };
        let enabled = ax::attribute(&element, "AXEnabled")
            .and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
            .unwrap_or(true);
        item.setEnabled(enabled);
        if let Some(key) =
            ax::string_attribute(&element, "AXMenuItemCmdChar").filter(|k| !k.is_empty())
        {
            // AXMenuItemCmdModifiers: bit 3 means no Command; bits 0-2 add
            // Shift, Option and Control.
            let mods = ax::attribute(&element, "AXMenuItemCmdModifiers")
                .and_then(|v| v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64()))
                .unwrap_or(0);
            set_key_equivalent(
                &item,
                &key,
                mods & 8 == 0,
                mods & 1 != 0,
                mods & 2 != 0,
                mods & 4 != 0,
            );
        }
        if ax::string_attribute(&element, "AXMenuItemMarkChar").is_some_and(|m| !m.is_empty()) {
            set_checked(&item);
        }
    }
}

/// Press the element behind a chosen proxy item.
pub fn press(index: usize) {
    if let Some(element) = ITEMS.with(|items| items.borrow().get(index).cloned()) {
        ax::perform(&element, "AXPress");
    }
}
