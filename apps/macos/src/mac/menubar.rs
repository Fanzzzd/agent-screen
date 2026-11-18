//! The menu bar of the app in front on an Agent Display, for the Viewer.
//!
//! macOS draws the menu bar of the user's own front app on every display,
//! the Agent Display's included: agent apps never become active. So the
//! Viewer covers that strip with the display's front app's own titles, read
//! here through accessibility, and a click on a title opens that menu and
//! presses the chosen item. Reads take a round trip per attribute, so they
//! run off the main thread; menus are built on it.

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

/// The menu bar of the front app on a display: its name and its menu
/// titles, without the Apple menu.
#[derive(Clone, PartialEq, Debug)]
pub struct Bar {
    pub pid: i32,
    pub app: String,
    pub titles: Vec<String>,
}

/// The front app on `display` and its menu bar, or None when it has none.
/// Its topmost document window there, as a menu command acts on one: a
/// panel or alert alone gives no strip.
pub fn bar(display: DisplayId) -> Option<Bar> {
    let area = crate::mac::cg::display_bounds(display);
    let local = crate::model::Rect::new(0.0, 0.0, area.width, area.height);
    let window = windows::on_display(display)
        .into_iter()
        .find(|w| w.layer == 0 && local.contains(w.bounds.mid()))?;
    let app = ax::application(window.target.pid);
    let bar = ax::element_attribute(&app, "AXMenuBar")?;
    let titles: Vec<String> = ax::elements_attribute(&bar, "AXChildren")
        .into_iter()
        .skip(1)
        .map(|item| ax::string_attribute(&item, "AXTitle").unwrap_or_default())
        .collect();
    (!titles.is_empty()).then_some(Bar {
        pid: window.target.pid,
        app: window.app,
        titles,
    })
}

/// The items of one menu of `pid`'s menu bar, `index` counting from the
/// first title after the Apple menu.
pub struct Items(Vec<Node>);

pub fn items(pid: i32, index: usize) -> Option<Items> {
    let app = ax::application(pid);
    let bar = ax::element_attribute(&app, "AXMenuBar")?;
    let title = ax::elements_attribute(&bar, "AXChildren")
        .into_iter()
        .nth(index + 1)?;
    let menu = ax::elements_attribute(&title, "AXChildren")
        .into_iter()
        .next()?;
    Some(Items(nodes(&menu)))
}

/// One menu, ready to pop up. Main thread.
pub fn build_items(title: &str, items: Items) -> Menu {
    let menu = Menu::new(title);
    ITEMS.with(|items| items.borrow_mut().clear());
    fill(&menu, items.0);
    menu
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
