//! The menus of the app in front on an Agent Display, opened from the
//! Viewer's menu bar strip.
//!
//! That strip mirrors the menu bar of the user's own front app, not the
//! display's, and posted clicks never open a menu bar anyway. So a click
//! there shows the display's front app menus as one native menu, read
//! through accessibility, and the chosen item is pressed.

use super::ax::{self, Element, Sendable};
use super::native::{Menu, MenuAction, set_checked, set_key_equivalent};
use super::windows;
use crate::model::DisplayId;
use objc2_core_foundation::{CFBoolean, CFNumber};
use std::cell::RefCell;

thread_local! {
    /// Elements of the menu last shown, by `PressProxy` index.
    static ITEMS: RefCell<Vec<Sendable>> = const { RefCell::new(Vec::new()) };
}

/// The front app's menus, read through accessibility. Reading takes one
/// round trip per attribute, so it runs off the main thread.
pub struct Menus {
    app: String,
    titles: Vec<(String, Vec<Node>)>,
}

enum Node {
    Separator,
    Submenu(String, Vec<Node>),
    Item {
        title: String,
        element: Sendable,
        enabled: bool,
        /// Key equivalent and AXMenuItemCmdModifiers.
        key: Option<(String, i64)>,
        checked: bool,
    },
}

/// The front app's menus on `display`, or None when no app there has any.
pub fn read(display: DisplayId) -> Option<Menus> {
    let window = windows::on_display(display).into_iter().next()?;
    let app = ax::application(window.target.pid);
    let bar = ax::element_attribute(&app, "AXMenuBar")?;
    // The first title is the Apple menu, which belongs to the system.
    let titles: Vec<(String, Vec<Node>)> = ax::elements_attribute(&bar, "AXChildren")
        .into_iter()
        .skip(1)
        .filter_map(|item| {
            let submenu = ax::elements_attribute(&item, "AXChildren")
                .into_iter()
                .next()?;
            let title = ax::string_attribute(&item, "AXTitle").unwrap_or_default();
            Some((title, nodes(&submenu)))
        })
        .collect();
    (!titles.is_empty()).then_some(Menus {
        app: window.app,
        titles,
    })
}

fn nodes(ax_menu: &Element) -> Vec<Node> {
    ax::elements_attribute(ax_menu, "AXChildren")
        .into_iter()
        .map(|element| {
            let title = ax::string_attribute(&element, "AXTitle").unwrap_or_default();
            if title.is_empty() {
                return Node::Separator;
            }
            if let Some(sub) = ax::elements_attribute(&element, "AXChildren").first() {
                return Node::Submenu(title, nodes(sub));
            }
            let enabled = ax::attribute(&element, "AXEnabled")
                .and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
                .unwrap_or(true);
            let key = ax::string_attribute(&element, "AXMenuItemCmdChar")
                .filter(|k| !k.is_empty())
                .map(|k| {
                    let mods = ax::attribute(&element, "AXMenuItemCmdModifiers")
                        .and_then(|v| v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64()))
                        .unwrap_or(0);
                    (k, mods)
                });
            let checked =
                ax::string_attribute(&element, "AXMenuItemMarkChar").is_some_and(|m| !m.is_empty());
            Node::Item {
                title,
                element: Sendable(element),
                enabled,
                key,
                checked,
            }
        })
        .collect()
}

/// One native menu with a submenu per title. Main thread.
pub fn build(menus: Menus) -> Menu {
    let menu = Menu::new(&menus.app);
    menu.disabled(&menus.app);
    ITEMS.with(|items| items.borrow_mut().clear());
    for (title, nodes) in menus.titles {
        let content = Menu::new(&title);
        fill(&content, nodes);
        menu.submenu(&title, content);
    }
    menu
}

fn fill(menu: &Menu, nodes: Vec<Node>) {
    for node in nodes {
        match node {
            Node::Separator => menu.separator(),
            Node::Submenu(title, nodes) => {
                let submenu = Menu::new(&title);
                fill(&submenu, nodes);
                menu.submenu(&title, submenu);
            }
            Node::Item {
                title,
                element,
                enabled,
                key,
                checked,
            } => {
                let index = ITEMS.with(|items| {
                    let mut items = items.borrow_mut();
                    items.push(element);
                    items.len() - 1
                });
                let item = menu.action(&title, MenuAction::PressProxy { index });
                item.setEnabled(enabled);
                // AXMenuItemCmdModifiers: bit 3 means no Command; bits 0-2
                // add Shift, Option and Control.
                if let Some((key, mods)) = key {
                    set_key_equivalent(
                        &item,
                        &key,
                        mods & 8 == 0,
                        mods & 1 != 0,
                        mods & 2 != 0,
                        mods & 4 != 0,
                    );
                }
                if checked {
                    set_checked(&item);
                }
            }
        }
    }
}

/// The element behind a chosen proxy item, to press off the main thread.
pub fn chosen(index: usize) -> Option<Sendable> {
    ITEMS.with(|items| items.borrow().get(index).map(|e| Sendable(e.0.clone())))
}
