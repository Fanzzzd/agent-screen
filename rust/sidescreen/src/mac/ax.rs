//! The Accessibility API: other apps' windows and their element trees.

use crate::model::{Rect, WindowTarget};
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CGPoint, CGSize,
};
use std::ffi::c_void;
use std::ptr::NonNull;

pub type Element = CFRetained<AXUIElement>;

/// An element that can move to another thread: the AX API may be called
/// from any thread, and its calls block on the target app, so they belong
/// off the main thread.
pub struct Sendable(pub Element);
unsafe impl Send for Sendable {}

impl Sendable {
    /// The element. A method, so a closure captures the whole Sendable.
    pub fn get(&self) -> &Element {
        &self.0
    }

    pub fn perform(&self, action: &str) -> bool {
        perform(&self.0, action)
    }
}

unsafe extern "C" {
    /// _AXUIElementGetWindow, through the shim: the CGWindowID behind an AX window.
    fn SSAXWindowID(element: &AXUIElement, out: *mut u32) -> bool;
}

pub fn is_trusted() -> bool {
    unsafe { objc2_application_services::AXIsProcessTrusted() }
}

/// Ask for Accessibility, showing the system prompt once.
pub fn request_trust() {
    use objc2_core_foundation::CFDictionary;
    let key = CFString::from_static_str("AXTrustedCheckOptionPrompt");
    let options =
        CFDictionary::<CFString, CFBoolean>::from_slices(&[&*key], &[CFBoolean::new(true)]);
    unsafe { objc2_application_services::AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
}

/// Wait at most 1 s for any app to answer, not the default 6 s. Set on
/// the system-wide element, the timeout applies to every element.
pub fn set_global_timeout() {
    unsafe { AXUIElement::new_system_wide().set_messaging_timeout(1.0) };
}

/// An app's root element. A busy or modal app would otherwise hold each call
/// for 6 s.
pub fn application(pid: i32) -> Element {
    let app = unsafe { AXUIElement::new_application(pid) };
    unsafe { app.set_messaging_timeout(1.0) };
    app
}

pub fn attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let name = CFString::from_str(name);
    let mut value: *const CFType = std::ptr::null();
    let error = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if error != AXError::Success || value.is_null() {
        return None;
    }
    Some(unsafe { CFRetained::from_raw(NonNull::new_unchecked(value as *mut CFType)) })
}

pub fn element_attribute(element: &AXUIElement, name: &str) -> Option<Element> {
    attribute(element, name)?.downcast::<AXUIElement>().ok()
}

pub fn elements_attribute(element: &AXUIElement, name: &str) -> Vec<Element> {
    attribute(element, name)
        .map(|v| elements(&v))
        .unwrap_or_default()
}

fn elements(value: &CFType) -> Vec<Element> {
    let Some(array) = value.downcast_ref::<CFArray>() else {
        return Vec::new();
    };
    let array: &CFArray<CFType> = unsafe { &*(array as *const CFArray as *const CFArray<CFType>) };
    array
        .to_vec()
        .into_iter()
        .filter_map(|v| v.downcast::<AXUIElement>().ok())
        .collect()
}

pub fn string_attribute(element: &AXUIElement, name: &str) -> Option<String> {
    Some(
        attribute(element, name)?
            .downcast_ref::<CFString>()?
            .to_string(),
    )
}

/// Whether the app lets clients change the attribute.
pub fn settable(element: &AXUIElement, name: &str) -> bool {
    let name = CFString::from_str(name);
    let mut settable: u8 = 0;
    let error = unsafe { element.is_attribute_settable(&name, NonNull::from(&mut settable)) };
    error == AXError::Success && settable != 0
}

pub fn set_attribute(element: &AXUIElement, name: &str, value: &CFType) -> bool {
    let name = CFString::from_str(name);
    unsafe { element.set_attribute_value(&name, value) == AXError::Success }
}

pub fn perform(element: &AXUIElement, action: &str) -> bool {
    let action = CFString::from_str(action);
    unsafe { element.perform_action(&action) == AXError::Success }
}

/// Perform an action, and say whether the element took it. An action that
/// opens a modal often outlasts the messaging timeout and reports
/// CannotComplete although it ran, so only an unsupported action counts as
/// not taken: a fallback click then cannot press twice.
pub fn try_action(element: &AXUIElement, action: &str) -> bool {
    let action = CFString::from_str(action);
    let error = unsafe { element.perform_action(&action) };
    !matches!(
        error,
        AXError::ActionUnsupported
            | AXError::AttributeUnsupported
            | AXError::IllegalArgument
            | AXError::InvalidUIElement
            | AXError::NoValue
    )
}

pub fn window_id(element: &AXUIElement) -> Option<u32> {
    let mut id = 0u32;
    unsafe { SSAXWindowID(element, &mut id) }.then_some(id)
}

fn point_value(value: &CFType) -> Option<CGPoint> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut point = CGPoint::default();
    unsafe {
        value.value(
            AXValueType::CGPoint,
            NonNull::from(&mut point).cast::<c_void>(),
        )
    }
    .then_some(point)
}

fn size_value(value: &CFType) -> Option<CGSize> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut size = CGSize::default();
    unsafe {
        value.value(
            AXValueType::CGSize,
            NonNull::from(&mut size).cast::<c_void>(),
        )
    }
    .then_some(size)
}

fn frame_from(position: &CFType, size: &CFType) -> Option<Rect> {
    let p = point_value(position)?;
    let s = size_value(size)?;
    Some(Rect::new(p.x, p.y, s.width, s.height))
}

/// Global frame of an element, top-left origin.
pub fn frame(element: &AXUIElement) -> Option<Rect> {
    let position = attribute(element, "AXPosition")?;
    let size = attribute(element, "AXSize")?;
    frame_from(&position, &size)
}

fn ax_value<T>(kind: AXValueType, value: &T) -> Option<CFRetained<AXValue>> {
    unsafe { AXValue::new(kind, NonNull::from(value).cast::<c_void>()) }
}

pub fn set_position(element: &AXUIElement, x: f64, y: f64) -> bool {
    ax_value(AXValueType::CGPoint, &CGPoint::new(x, y))
        .is_some_and(|v| set_attribute(element, "AXPosition", &v))
}

pub fn set_size(element: &AXUIElement, width: f64, height: f64) -> bool {
    ax_value(AXValueType::CGSize, &CGSize::new(width, height))
        .is_some_and(|v| set_attribute(element, "AXSize", &v))
}

// MARK: Windows

/// The AX element of a window, matched by window id.
pub fn window_element(target: WindowTarget) -> Option<Element> {
    elements_attribute(&application(target.pid), "AXWindows")
        .into_iter()
        .find(|w| window_id(w) == Some(target.window_id))
}

/// Whether a global point is on the title bar or toolbar background of a
/// window, where a drag moves the window. WindowServer moves windows for
/// real mouse drags only, so agent drags there move it through here.
pub fn on_title_bar(target: WindowTarget, bounds: Rect, at: (f64, f64)) -> bool {
    // Unified toolbars are up to 52 pt tall.
    if at.1 - bounds.y > 52.0 {
        return false;
    }
    let mut raw: *const AXUIElement = std::ptr::null();
    let error = unsafe {
        application(target.pid).copy_element_at_position(
            at.0 as f32,
            at.1 as f32,
            NonNull::from(&mut raw),
        )
    };
    let Some(raw) = NonNull::new(raw as *mut AXUIElement) else {
        return false;
    };
    let element = unsafe { CFRetained::from_raw(raw) };
    error == AXError::Success
        && matches!(
            string_attribute(&element, "AXRole").as_deref(),
            Some("AXWindow" | "AXToolbar")
        )
}

#[derive(Debug)]
pub enum MoveError {
    NoWindow,
    Rejected,
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MoveError::NoWindow => "no such window",
            MoveError::Rejected => "the app rejected the change",
        })
    }
}

/// Fit the window inside `bounds` with a 40 pt margin, centered.
/// Minimize a window: what's left when it can't go where it belongs.
pub fn minimize(target: WindowTarget) -> bool {
    window_element(target).is_some_and(|w| set_attribute(&w, "AXMinimized", CFBoolean::new(true)))
}

pub fn move_into(target: WindowTarget, bounds: Rect) -> Result<(), MoveError> {
    // A removed display has empty bounds; the window would land in a
    // corner of the main screen.
    if bounds.width <= 80.0 || bounds.height <= 80.0 {
        return Err(MoveError::Rejected);
    }
    let window = window_element(target).ok_or(MoveError::NoWindow)?;
    move_element_into(&window, bounds)
}

/// Move every window of `pid` into `bounds`; how many there were.
pub fn move_all_into(pid: i32, bounds: Rect) -> usize {
    elements_attribute(&application(pid), "AXWindows")
        .iter()
        .filter(|w| string_attribute(w, "AXRole").as_deref() == Some("AXWindow"))
        .filter(|w| move_element_into(w, bounds).is_ok())
        .count()
}

/// Center a window element inside `bounds`, shrunk to fit.
pub fn move_element_into(window: &AXUIElement, bounds: Rect) -> Result<(), MoveError> {
    if bounds.width <= 80.0 || bounds.height <= 80.0 {
        return Err(MoveError::Rejected);
    }
    let area = bounds.inset(40.0, 40.0);
    let size = attribute(window, "AXSize")
        .and_then(|v| size_value(&v))
        .unwrap_or(CGSize::new(800.0, 600.0));
    let fitted = (size.width.min(area.width), size.height.min(area.height));
    let (mid_x, mid_y) = area.mid();
    if !set_position(window, mid_x - fitted.0 / 2.0, mid_y - fitted.1 / 2.0) {
        return Err(MoveError::Rejected);
    }
    if fitted != (size.width, size.height) {
        set_size(window, fitted.0, fitted.1);
    }
    Ok(())
}

/// Put a window back at an exact global frame.
pub fn set_frame(target: WindowTarget, frame: Rect) -> Result<(), MoveError> {
    let window = window_element(target).ok_or(MoveError::NoWindow)?;
    if !set_position(&window, frame.x, frame.y) {
        return Err(MoveError::Rejected);
    }
    set_size(&window, frame.width, frame.height);
    Ok(())
}

/// Press the window's close button. The app may still keep it open, for
/// example to ask about unsaved changes.
pub fn close(target: WindowTarget) -> Result<(), MoveError> {
    let window = window_element(target).ok_or(MoveError::NoWindow)?;
    let button = element_attribute(&window, "AXCloseButton").ok_or(MoveError::Rejected)?;
    if perform(&button, "AXPress") {
        Ok(())
    } else {
        Err(MoveError::Rejected)
    }
}

// MARK: Element trees

/// A window as an agent that works per window sees it: numbered elements it
/// can act on. Coordinates are points from the window's top-left corner.
pub struct Snapshot {
    pub tree: String,
    pub elements: Vec<Element>,
    pub truncated: bool,
}
// See Sendable.
unsafe impl Send for Snapshot {}

/// The window's elements, positioned by their centers in display points
/// from `origin`, the display's top-left corner: the space the computer
/// tool clicks in.
pub fn snapshot(target: WindowTarget, origin: (f64, f64)) -> Option<Snapshot> {
    let window = window_element(target)?;
    // Chromium and Electron apps only publish their web content tree once a
    // client asks for it. Other apps ignore the attribute.
    set_attribute(
        &application(target.pid),
        "AXManualAccessibility",
        CFBoolean::new(true),
    );
    let mut walker = TreeWalker {
        origin,
        lines: Vec::new(),
        elements: Vec::new(),
        truncated: false,
        visited: 0,
    };
    walker.visit(&window, 0);
    // The menu bar the agent's display shows belongs to the user's front
    // app, so list this app's menu titles here; pressing one lists its items.
    let titles = element_attribute(&application(target.pid), "AXMenuBar")
        .map(|bar| elements_attribute(&bar, "AXChildren"))
        .unwrap_or_default();
    // The first title is the Apple menu.
    for (n, item) in titles.into_iter().skip(1).enumerate() {
        let Some(title) = string_attribute(&item, "AXTitle").filter(|t| !t.is_empty()) else {
            continue;
        };
        if n == 0 {
            walker
                .lines
                .push("Menu bar (press a title to list its items):".into());
        }
        walker.lines.push(format!(
            "[{}] menubaritem \"{title}\"",
            walker.elements.len()
        ));
        walker.elements.push(item);
    }
    Some(Snapshot {
        tree: walker.lines.join("\n"),
        elements: walker.elements,
        truncated: walker.truncated,
    })
}

/// The items of a menu bar title, numbered from `first`, or None when
/// `item` is not a menu bar title. Reading items leaves the menu closed;
/// AXPress on an item chooses it without opening anything.
pub fn menu_items(item: &AXUIElement, first: usize) -> Option<Snapshot> {
    if string_attribute(item, "AXRole")? != "AXMenuBarItem" {
        return None;
    }
    let menu = elements_attribute(item, "AXChildren").into_iter().next()?;
    let mut snapshot = Snapshot {
        tree: String::new(),
        elements: Vec::new(),
        truncated: false,
    };
    let mut lines = Vec::new();
    list_menu(&menu, 0, first, &mut lines, &mut snapshot.elements);
    snapshot.tree = lines.join("\n");
    Some(snapshot)
}

fn list_menu(
    menu: &AXUIElement,
    depth: usize,
    first: usize,
    lines: &mut Vec<String>,
    elements: &mut Vec<Element>,
) {
    let indent = "  ".repeat(depth);
    for element in elements_attribute(menu, "AXChildren") {
        let Some(title) = string_attribute(&element, "AXTitle").filter(|t| !t.is_empty()) else {
            continue; // a separator
        };
        if let Some(sub) = elements_attribute(&element, "AXChildren").first() {
            if depth < 2 {
                lines.push(format!("{indent}\"{title}\" submenu:"));
                list_menu(sub, depth + 1, first, lines, elements);
            }
            continue;
        }
        let mut line = format!("{indent}[{}] menuitem \"{title}\"", first + elements.len());
        if let Some(key) = shortcut(&element) {
            line += &format!(" key=\"{key}\"");
        }
        let flag = |name: &str| {
            attribute(&element, name)
                .and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
        };
        if flag("AXEnabled") == Some(false) {
            line += " disabled";
        }
        if string_attribute(&element, "AXMenuItemMarkChar").is_some_and(|m| !m.is_empty()) {
            line += " checked";
        }
        lines.push(line);
        elements.push(element);
    }
}

/// Select all the text in `pid`'s focused text element. Select All in a
/// menu acts on the key window's first responder, which an app in the
/// background doesn't have, so neither its menu item nor a posted cmd+a
/// selects anything there.
pub fn select_all(target: WindowTarget) -> bool {
    // An app that hasn't had a click yet may have no focused element: take
    // the window's first text field and focus it.
    let focused = element_attribute(&application(target.pid), "AXFocusedUIElement")
        .filter(|e| string_attribute(e, "AXValue").is_some())
        .or_else(|| {
            let window = window_element(target)?;
            let field = first_text(&window, 0, &mut 0)?;
            set_attribute(&field, "AXFocused", CFBoolean::new(true));
            Some(field)
        });
    let Some(focused) = focused else {
        return false;
    };
    let Some(mut text) = string_attribute(&focused, "AXValue") else {
        return false;
    };
    // Typing just before can still be landing, and text checking such as
    // TextEdit's automatic capitals rewrites a word late and moves the
    // caret: wait until the text holds still for 100 ms, at most 600 ms.
    let mut still = 0;
    for _ in 0..12 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let now = string_attribute(&focused, "AXValue").unwrap_or_default();
        if now == text {
            still += 1;
            if still >= 2 {
                break;
            }
        } else {
            still = 0;
            text = now;
        }
    }
    if !settable(&focused, "AXSelectedTextRange") {
        return false;
    }
    let range = objc2_core_foundation::CFRange::new(0, text.encode_utf16().count() as isize);
    let Some(value) =
        (unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&range).cast::<c_void>()) })
    else {
        return false;
    };
    set_attribute(&focused, "AXSelectedTextRange", &value)
}

/// The first text area or field under `element`, depth first.
fn first_text(element: &AXUIElement, depth: usize, visited: &mut usize) -> Option<Element> {
    *visited += 1;
    if depth > 12 || *visited > 400 {
        return None;
    }
    for child in elements_attribute(element, "AXChildren") {
        let role = string_attribute(&child, "AXRole").unwrap_or_default();
        if matches!(role.as_str(), "AXTextArea" | "AXTextField") {
            return Some(child);
        }
        if let Some(found) = first_text(&child, depth + 1, visited) {
            return Some(found);
        }
    }
    None
}

/// A menu item's key equivalent in the computer tool's key syntax, such as
/// "cmd+shift+n". AXMenuItemCmdModifiers: bit 3 means no Command; bits 0-2
/// add Shift, Option and Control.
/// Whether an app's menus offer an enabled Command-N, as apps that open
/// more windows do. None when its menu bar can't be read.
pub fn has_new_window_command(pid: i32) -> Option<bool> {
    let bar = element_attribute(&application(pid), "AXMenuBar")?;
    let menus = elements_attribute(&bar, "AXChildren");
    if menus.is_empty() {
        return None;
    }
    let found = menus.iter().any(|title| {
        elements_attribute(title, "AXChildren").iter().any(|menu| {
            elements_attribute(menu, "AXChildren").iter().any(|item| {
                shortcut(item).as_deref() == Some("cmd+n")
                    && attribute(item, "AXEnabled")
                        .and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
                        .unwrap_or(true)
            })
        })
    });
    Some(found)
}

fn shortcut(element: &AXUIElement) -> Option<String> {
    let key = string_attribute(element, "AXMenuItemCmdChar")?;
    let key = key.trim();
    if key.chars().count() != 1 || !key.chars().all(|c| c.is_ascii_graphic()) {
        return None;
    }
    let mods = attribute(element, "AXMenuItemCmdModifiers")
        .and_then(|v| v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64()))
        .unwrap_or(0);
    // Higher bits, such as Fn for Enter Full Screen, have no key name here.
    if mods & !0xF != 0 {
        return None;
    }
    let mut parts = Vec::new();
    if mods & 4 != 0 {
        parts.push("ctrl");
    }
    if mods & 2 != 0 {
        parts.push("alt");
    }
    if mods & 1 != 0 {
        parts.push("shift");
    }
    if mods & 8 == 0 {
        parts.push("cmd");
    }
    let key = key.to_lowercase();
    parts.push(&key);
    Some(parts.join("+"))
}

/// Depth-first walk that numbers every element worth naming. Unlabelled
/// containers are skipped but their children are kept, which keeps the tree
/// short without losing anything an agent could act on.
struct TreeWalker {
    origin: (f64, f64),
    lines: Vec<String>,
    elements: Vec<Element>,
    truncated: bool,
    visited: usize,
}

const MAX_ELEMENTS: usize = 400;
const MAX_VISITS: usize = 3000;
const MAX_DEPTH: usize = 40;
const NAMES: [&str; 10] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXEnabled",
    "AXFocused",
    "AXPosition",
    "AXSize",
    "AXChildren",
];
const CONTAINERS: [&str; 7] = [
    "AXGroup",
    "AXSplitGroup",
    "AXScrollArea",
    "AXLayoutArea",
    "AXUnknown",
    "AXSplitter",
    "AXLayoutItem",
];

impl TreeWalker {
    fn visit(&mut self, element: &AXUIElement, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        // Unlabelled containers are not numbered but still cost a round
        // trip each; a web page has thousands.
        self.visited += 1;
        if self.elements.len() >= MAX_ELEMENTS || self.visited > MAX_VISITS {
            self.truncated = true;
            return;
        }
        let Some(values) = multiple_values(element) else {
            return;
        };
        let string = |i: usize| {
            let text = values[i].downcast_ref::<CFString>()?.to_string();
            // Apps wrap numbers and names in invisible direction marks.
            let text: String = text.chars().filter(|c| !is_direction_mark(*c)).collect();
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.chars().take(80).collect::<String>())
        };
        let boolean = |i: usize| values[i].downcast_ref::<CFBoolean>().map(|b| b.as_bool());
        let role = string(0).unwrap_or_else(|| "AXUnknown".into());
        // A ruler's tick marks: dozens of elements nobody acts on.
        if role == "AXRulerMarker" {
            return;
        }
        let label = string(2).or_else(|| string(3));
        let value = values[4]
            .downcast_ref::<CFNumber>()
            .and_then(|n| n.as_f64())
            .map(|n| {
                if n.fract() == 0.0 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            })
            .or_else(|| boolean(4).map(|b| if b { "1".into() } else { "0".into() }))
            .or_else(|| string(4));
        let children = elements(&values[9]);

        let mut indent = depth;
        if !CONTAINERS.contains(&role.as_str()) || label.is_some() {
            let mut line = format!("[{}] {}", self.elements.len(), short(&role));
            if let Some(subrole) = string(1) {
                line += &format!("({})", short(&subrole));
            }
            if let Some(label) = &label {
                line += &format!(" \"{label}\"");
            }
            if let Some(value) = &value
                && Some(value) != label.as_ref()
            {
                line += &format!(" value=\"{value}\"");
            }
            if boolean(5) == Some(false) {
                line += " disabled";
            }
            if boolean(6) == Some(true) {
                line += " focused";
            }
            if let Some(f) =
                frame_from(&values[7], &values[8]).filter(|f| f.width > 0.0 && f.height > 0.0)
            {
                let x = (f.x - self.origin.0 + f.width / 2.0).round() as i64;
                let y = (f.y - self.origin.1 + f.height / 2.0).round() as i64;
                line += &format!(" at ({x},{y}) {}×{}", f.width as i64, f.height as i64);
            }
            self.lines.push(format!("{}{line}", "  ".repeat(depth)));
            self.elements
                .push(unsafe { CFRetained::retain(NonNull::from(element)) });
            indent = depth + 1;
        }
        for child in children {
            self.visit(&child, indent);
        }
    }
}

/// Left-to-right and right-to-left marks, embeddings and isolates.
fn is_direction_mark(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// "AXButton" → "button".
fn short(name: &str) -> String {
    name.strip_prefix("AX").unwrap_or(name).to_lowercase()
}

fn multiple_values(element: &AXUIElement) -> Option<Vec<CFRetained<CFType>>> {
    let names: Vec<CFRetained<CFString>> =
        NAMES.iter().map(|n| CFString::from_static_str(n)).collect();
    let names: CFRetained<CFArray<CFString>> = CFArray::from_retained_objects(&names);
    let mut raw: *const CFArray = std::ptr::null();
    let error = unsafe {
        element.copy_multiple_attribute_values(
            names.as_opaque(),
            objc2_application_services::AXCopyMultipleAttributeOptions(0),
            NonNull::from(&mut raw),
        )
    };
    if error != AXError::Success || raw.is_null() {
        return None;
    }
    let array: CFRetained<CFArray> =
        unsafe { CFRetained::from_raw(NonNull::new_unchecked(raw as *mut CFArray)) };
    let array: &CFArray<CFType> =
        unsafe { &*(&*array as *const CFArray as *const CFArray<CFType>) };
    let values = array.to_vec();
    (values.len() == NAMES.len()).then_some(values)
}
