//! Input for an agent's window of the user's front app, through
//! accessibility, with no events posted (#110; the rules are in
//! `agent_screen_model::shared_app`). Every call blocks on the app, so
//! everything here runs off the main thread.

use super::ax::{self, Element};
use crate::model::WindowTarget;
use agent_screen_model::shared_app::{self as rules, ElementUse};
use agent_screen_model::typing::landed;
use objc2_application_services::AXUIElement;
use objc2_core_foundation::{CFBoolean, CFString, CFURL};
use std::time::{Duration, Instant};

/// What a field holds after the agent's last edit there, and its
/// selection then. An unfocused field's own selection means nothing, as
/// no click placed a caret in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub value: String,
    pub range: (usize, usize),
}

/// What is under a click in the agent's window.
pub enum Hit {
    Text(Element),
    Press(Element),
    /// A control whose press opens a menu, as a page's select.
    Menu(Element),
    /// Nothing that takes a press or text: the role and actions of the
    /// element under the point.
    Nothing(String, Vec<String>),
    /// Accessibility named no element of that window there.
    Unknown,
}

/// What an element is called, for a message: its title, description or
/// placeholder, else its role.
pub fn label(element: &ax::Element) -> String {
    ["AXTitle", "AXDescription", "AXPlaceholderValue"]
        .into_iter()
        .find_map(|name| ax::string_attribute(element, name).filter(|s| !s.trim().is_empty()))
        .or_else(|| ax::string_attribute(element, "AXRoleDescription"))
        .unwrap_or_else(|| "element".into())
}

fn role(element: &ax::Element) -> String {
    ax::string_attribute(element, "AXRole").unwrap_or_default()
}

/// Whether pressing `element` opens a menu, which takes the user's
/// keyboard while it is open (`rules::opens_menu`).
pub fn opens_menu(element: &ax::Element) -> bool {
    let role = role(element);
    if !rules::may_open_menu(&role) {
        return false;
    }
    // Chromium lists a select's options as menu items right under it.
    let holds_menu = ax::elements_attribute(element, "AXChildren")
        .iter()
        .any(|c| matches!(self::role(c).as_str(), "AXMenu" | "AXMenuItem"));
    rules::opens_menu(&role, ax::in_web_area(element), holds_menu)
}

/// Where a press's effect is read: the agent's window, its page, or the
/// window itself where it shows none, and the element pressed.
#[derive(Clone)]
pub struct Page {
    window: Element,
    root: Element,
    pressed: Element,
}

// SAFETY: accessibility elements are CF objects that may be used from
// any thread, as `ax::Sendable` does.
unsafe impl Send for Page {}

/// Elements of the page a digest reads, and of the pressed element: a
/// read takes about 50 µs an element in Chromium.
const PAGE_ELEMENTS: usize = 1000;
const PRESSED_ELEMENTS: usize = 200;

pub fn page(target: WindowTarget, pressed: &Element) -> Option<Page> {
    let window = ax::window_element(target)?;
    let root = ax::web_area(&window).unwrap_or_else(|| window.clone());
    Some(Page {
        window,
        root,
        pressed: pressed.clone(),
    })
}

/// A digest of the page's first `PAGE_ELEMENTS`: read before a press and
/// once at the end, as it keeps the app busy for tens of milliseconds.
pub fn page_digest(page: &Page) -> u64 {
    ax::page_digest(&page.root, PAGE_ELEMENTS).0
}

/// A digest of what changes cheaply read while waiting for a press's
/// effect: the page's address, the window's title and the pressed
/// element's own tree.
pub fn quick_digest(page: &Page) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (element, name) in [(&page.root, "AXURL"), (&page.window, "AXTitle")] {
        text(element, name).hash(&mut hasher);
    }
    ax::page_digest(&page.pressed, PRESSED_ELEMENTS)
        .0
        .hash(&mut hasher);
    hasher.finish()
}

/// The app's focused element, when it is in the agent's window `window`:
/// the pressed element, which a press focuses, reads as one value. None
/// when the read timed out or focus is elsewhere, as in the user's
/// window, where they may move it while the press is watched.
pub fn focus_digest(page: &Page, pid: i32, window: u32) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let app = ax::application(pid);
    unsafe { app.set_messaging_timeout(0.2) };
    let focused = ax::element_attribute(&app, "AXFocusedUIElement")?;
    if ax::same(&focused, &page.pressed) {
        return Some(1);
    }
    if ax::window_of(&focused) != Some(window) {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let f: &objc2_core_foundation::CFType = &focused;
    f.hash(&mut hasher);
    Some(hasher.finish())
}

/// How many windows of `pid` are on Agent Displays: the user opening or
/// closing a window of their own doesn't count as a press's effect.
pub fn agent_windows(pid: i32) -> usize {
    super::windows::all()
        .iter()
        .filter(|w| w.target.pid == pid && super::input::on_agent_display_at(w.bounds))
        .count()
}

/// Whether window `focused` is on the Agent Display that window `target`
/// is on, the Agent Session's own: a Handover or the agent's window
/// (#162), or a stray of the user's under ADR 0007. False when either isn't
/// on screen, or `target` isn't on an Agent Display.
pub fn on_display_of(target: u32, focused: u32) -> bool {
    let all = super::windows::all();
    let mid = |id: u32| {
        all.iter()
            .find(|w| w.target.window_id == id)
            .map(|w| w.bounds.mid())
    };
    let (Some(target), Some(focused)) = (mid(target), mid(focused)) else {
        return false;
    };
    super::cg::active_displays()
        .into_iter()
        .filter(|&d| super::cg::is_agent_screen(d))
        .map(super::cg::display_bounds)
        .any(|area| area.contains(target) && area.contains(focused))
}

/// Whether `pid` has a window on the user's screens, off the Agent
/// Displays, where the user may be working.
pub fn user_window(pid: i32) -> bool {
    super::windows::all()
        .iter()
        .any(|w| w.target.pid == pid && !super::input::on_agent_display_at(w.bounds))
}

/// A string or URL attribute as text; empty when it is neither.
fn text(element: &ax::Element, name: &str) -> String {
    let Some(value) = ax::attribute(element, name) else {
        return String::new();
    };
    if let Some(s) = value.downcast_ref::<CFString>() {
        return s.to_string();
    }
    value
        .downcast_ref::<CFURL>()
        .map(|u| u.string().to_string())
        .unwrap_or_default()
}

/// Whether Return on `element` would press or confirm it, as `confirm`
/// does, rather than fail.
pub fn confirm_acts(element: &ax::Element) -> bool {
    if opens_menu(element) {
        return false;
    }
    let actions = ax::actions(element);
    let field = rules::element_use(&role(element), &actions, false) == ElementUse::Text;
    actions.iter().any(|a| a == "AXConfirm") || (!field && actions.iter().any(|a| a == "AXPress"))
}

/// Why a control that opens a menu isn't pressed beside the user's
/// window, for the agent.
pub fn menu_refusal(element: &ax::Element) -> String {
    format!(
        "\"{}\" opens a menu when pressed, as a web page's select does, and an open menu takes the user's keyboard, so it was not pressed and no menu was opened. To choose one of its items, call act_on_element with action set_value and the item's exact title, which chooses it without opening the menu where Agent Screen can, and otherwise says why not and what to do instead",
        label(element)
    )
}

fn flag(element: &AXUIElement, name: &str) -> Option<bool> {
    ax::attribute(element, name).and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
}

/// The elements up from `element` to its window, nearest first.
fn ancestors(element: &ax::Element) -> Vec<ax::Element> {
    let mut out = Vec::new();
    let mut at = element.clone();
    while out.len() < 64 {
        let Some(parent) = ax::element_attribute(&at, "AXParent") else {
            break;
        };
        if role(&parent) == "AXWindow" {
            break;
        }
        out.push(parent.clone());
        at = parent;
    }
    out
}

/// The elements under `scope` in tree order, as far as `MAX` visits go,
/// that `keep` takes.
fn descendants(scope: &ax::Element, keep: impl Fn(&ax::Element) -> bool) -> Vec<ax::Element> {
    const MAX: usize = 5000;
    let mut out = Vec::new();
    let mut stack: Vec<ax::Element> = ax::elements_attribute(scope, "AXChildren")
        .into_iter()
        .rev()
        .collect();
    let mut visits = 0;
    while let Some(element) = stack.pop() {
        visits += 1;
        if visits > MAX {
            break;
        }
        if keep(&element) {
            out.push(element.clone());
        }
        stack.extend(
            ax::elements_attribute(&element, "AXChildren")
                .into_iter()
                .rev(),
        );
    }
    out
}

/// The element at `at` in `target`'s window: a field, or the nearest
/// element up from there that takes a press, as a link around its text.
pub fn hit(target: WindowTarget, at: (f64, f64)) -> Hit {
    ax::ensure_web_tree(target);
    let Some(mut element) = ax::element_at(target.pid, at) else {
        return Hit::Unknown;
    };
    if ax::window_of(&element) != Some(target.window_id) {
        return Hit::Unknown;
    }
    let first = role(&element);
    let hit = element.clone();
    for _ in 0..8 {
        let role = role(&element);
        if matches!(role.as_str(), "AXWindow" | "AXWebArea" | "AXSheet") {
            break;
        }
        match rules::element_use(&role, &ax::actions(&element), opens_menu(&element)) {
            ElementUse::Text => return Hit::Text(element),
            ElementUse::Press => return Hit::Press(element),
            ElementUse::Menu => return Hit::Menu(element),
            ElementUse::Nothing => {}
        }
        let Some(parent) = ax::element_attribute(&element, "AXParent") else {
            break;
        };
        element = parent;
    }
    if rules::press_anyway(&first) && !opens_menu(&hit) {
        return Hit::Press(hit);
    }
    let actions = ax::actions(&hit);
    Hit::Nothing(first, actions)
}

/// Whether `field` is a text field of `target`'s window that takes text
/// and says what it holds.
pub fn takes_text(field: &ax::Element, target: WindowTarget) -> bool {
    ax::window_of(field) == Some(target.window_id)
        && rules::takes_text(
            &role(field),
            ax::string_attribute(field, "AXValue").is_some(),
            ax::settable(field, "AXValue"),
        )
}

/// The selection to edit: the agent's own, while the field still holds
/// what its last edit left; else the end of the field's text.
fn current(field: &ax::Element, last: Option<&Selection>) -> Selection {
    let value = ax::string_attribute(field, "AXValue").unwrap_or_default();
    let range = last
        .filter(|l| l.value == value)
        .map(|l| l.range)
        .unwrap_or((value.encode_utf16().count(), 0));
    Selection { value, range }
}

/// Put `wanted` in the field and read it back: an app may take the value
/// late, as Chromium does, or change it. `typed` is what must show in it.
fn set_value(field: &ax::Element, before: &str, wanted: &str, typed: &str) -> Result<(), String> {
    if !ax::set_attribute(field, "AXValue", &CFString::from_str(wanted)) {
        return Err("the field refused the new text".into());
    }
    let started = Instant::now();
    let mut shown = before.to_string();
    while started.elapsed() < Duration::from_millis(1500) {
        shown = ax::string_attribute(field, "AXValue").unwrap_or_default();
        if shown == wanted || (shown != before && landed(typed, &shown)) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let shown: String = shown.chars().take(300).collect();
    Err(format!(
        "the field holds {shown:?}, not the text with {typed:?} in it"
    ))
}

/// Insert `text` at the agent's selection in `field`, without key events.
pub fn insert(
    field: &ax::Element,
    text: &str,
    last: Option<&Selection>,
) -> Result<Selection, String> {
    let now = current(field, last);
    let (wanted, caret) = rules::insert(&now.value, Some(now.range), text);
    set_value(field, &now.value, &wanted, text)?;
    // So the app's own caret follows, where it lets a client move it.
    ax::set_selected_range(field, (caret, 0));
    Ok(Selection {
        value: ax::string_attribute(field, "AXValue").unwrap_or(wanted),
        range: (caret, 0),
    })
}

/// Backspace at the agent's selection in `field`.
pub fn delete_backward(field: &ax::Element, last: Option<&Selection>) -> Result<Selection, String> {
    let now = current(field, last);
    let (wanted, caret) = rules::delete_backward(&now.value, Some(now.range));
    if wanted != now.value {
        set_value(field, &now.value, &wanted, "")?;
        ax::set_selected_range(field, (caret, 0));
    }
    Ok(Selection {
        value: wanted,
        range: (caret, 0),
    })
}

/// Select all of `field`'s text, for the next text or Backspace to
/// replace.
pub fn select_all(field: &ax::Element) -> Selection {
    let value = ax::string_attribute(field, "AXValue").unwrap_or_default();
    let length = value.encode_utf16().count();
    ax::set_selected_range(field, (0, length));
    Selection {
        value,
        range: (0, length),
    }
}

/// Return: the element's confirm action, or its press. The action taken,
/// and what the app answered: an error may come back from an action that
/// ran. A control that opens a menu gets neither.
pub fn confirm(element: &ax::Element) -> Result<(&'static str, ax::Action), String> {
    let menu = opens_menu(element);
    if menu {
        return Err(menu_refusal(element));
    }
    let actions = ax::actions(element);
    let field = rules::element_use(&role(element), &actions, menu) == ElementUse::Text;
    for (name, allowed) in [("AXConfirm", true), ("AXPress", !field)] {
        if !allowed || !actions.iter().any(|a| a == name) {
            continue;
        }
        match ax::action_result(element, name) {
            ax::Action::NotTaken => {}
            answer => return Ok((name, answer)),
        }
    }
    if field {
        return Err(no_submit(element));
    }
    Err(format!(
        "\"{}\" has no confirm action, so Return can't reach it without a key event",
        label(element)
    ))
}

/// Why Return can't submit a web field's form. Chromium gives a field no
/// confirm action, and nothing it publishes tells the button that
/// submits, which Return's implicit submission clicks, from one that
/// doesn't: both are AXButton with the same attributes. So Agent Screen
/// doesn't guess, and names the form's buttons for the agent to click.
fn no_submit(field: &ax::Element) -> String {
    let name = label(field);
    let form = ancestors(field).into_iter().find(|e| {
        rules::is_form(
            &role(e),
            &ax::string_attribute(e, "AXSubrole").unwrap_or_default(),
            &ax::string_attribute(e, "AXRoleDescription").unwrap_or_default(),
        )
    });
    let Some(form) = form else {
        return format!(
            "\"{name}\" has no confirm action and is in no form accessibility shows, so Return can't reach it without a key event. If the page has a button that acts on the field, click it"
        );
    };
    let buttons: Vec<String> = descendants(&form, |e| role(e) == "AXButton")
        .iter()
        .take(8)
        .map(|b| format!("\"{}\"", label(b)))
        .collect();
    if buttons.is_empty() {
        return format!(
            "\"{name}\" is a web text field with no confirm action, in a form without a button, which only a Return key event submits"
        );
    }
    format!(
        "\"{name}\" is a web text field with no confirm action, and accessibility doesn't say which of its form's buttons ({}) submits it, so Return can't submit the form without a key event, and Agent Screen doesn't guess. Click the button that submits it: the click presses it through accessibility, as Return would",
        buttons.join(", ")
    )
}

/// Where Tab moves: the outermost page around `element`, so it goes on
/// past a frame's end.
fn page_of(element: &ax::Element) -> Option<ax::Element> {
    std::iter::once(element.clone())
        .chain(ancestors(element))
        .filter(|e| role(e) == "AXWebArea")
        .last()
}

/// Tab (`forward`) or shift+Tab in `target`'s page: focus the next or
/// previous element that takes focus, from `from`, the element the agent
/// last picked, or else the page's focused element. The element focused,
/// and its label.
pub fn tab(
    target: WindowTarget,
    from: Option<&ax::Element>,
    forward: bool,
) -> Result<(ax::Element, String), String> {
    ax::ensure_web_tree(target);
    let page = from
        .filter(|e| ax::window_of(e) == Some(target.window_id))
        .and_then(page_of)
        .or_else(|| ax::window_element(target).and_then(|w| ax::web_area(&w)))
        .ok_or("your window shows no web page accessibility can reach, and Tab moves focus only within one")?;
    let stops = descendants(&page, |e| {
        let role = role(e);
        rules::tab_stop(
            &role,
            ax::settable(e, "AXFocused"),
            flag(e, "AXEnabled").unwrap_or(true),
        ) && !(role == "AXMenuItem" && in_menu_control(e))
    });
    let picked = from.and_then(|f| stops.iter().position(|s| **s == **f));
    let focused = || {
        stops
            .iter()
            .position(|s| flag(s, "AXFocused") == Some(true))
    };
    let at = picked.or_else(focused);
    let Some(next) = rules::next_stop(stops.len(), at, forward) else {
        return Err(match stops.is_empty() {
            true => "the page in your window has no element that takes focus".into(),
            false => format!(
                "focus is on the page's {} element that takes focus, and past it a browser moves into its own controls, which have no route here",
                if forward { "last" } else { "first" }
            ),
        });
    };
    let element = stops[next].clone();
    let name = label(&element);
    if !ax::set_attribute(&element, "AXFocused", CFBoolean::new(true)) {
        return Err(format!("\"{name}\" refused focus through accessibility"));
    }
    let started = Instant::now();
    while flag(&element, "AXFocused") != Some(true) {
        if started.elapsed() > Duration::from_millis(1000) {
            return Err(format!("\"{name}\" took no focus through accessibility"));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Ok((element, name))
}

/// Whether a menu item is an option of a closed control that opens a
/// menu, as a select's, which Chromium lists as focusable but Tab never
/// stops at.
fn in_menu_control(item: &ax::Element) -> bool {
    ancestors(item)
        .iter()
        .take(2)
        .any(|e| rules::may_open_menu(&role(e)))
}

/// Escape: the element's cancel action.
pub fn cancel(element: &ax::Element) -> Result<(), String> {
    if ax::actions(element).iter().any(|a| a == "AXCancel") && ax::try_action(element, "AXCancel") {
        return Ok(());
    }
    Err(format!(
        "\"{}\" has no cancel action, so Escape can't reach it without a key event",
        label(element)
    ))
}
