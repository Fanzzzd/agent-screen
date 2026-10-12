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

/// The pid of the app with keyboard focus, which may not be the frontmost
/// one: a launcher panel such as Spotlight's takes keys from behind it.
pub fn focused_app_pid() -> Option<i32> {
    let system = unsafe { AXUIElement::new_system_wide() };
    let app = element_attribute(&system, "AXFocusedApplication")?;
    let mut pid = 0;
    let ok = unsafe { app.pid(NonNull::from(&mut pid)) } == AXError::Success;
    ok.then_some(pid)
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

/// Make `target` its app's main window, as a click on it would, without
/// the focus event that would end the front app's text input session.
pub fn make_main(target: WindowTarget) -> bool {
    window_element(target).is_some_and(|w| set_attribute(&w, "AXMain", CFBoolean::new(true)))
}

/// The window an app sends keys to: its focused window. Asked before
/// input, so a hung app gets 0.1 s, not the usual 1 s.
pub fn focused_window_id(pid: i32) -> Option<u32> {
    let app = application(pid);
    unsafe { app.set_messaging_timeout(0.1) };
    let window = element_attribute(&app, "AXFocusedWindow")?;
    window_id(&window)
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
/// `origin` is the display's top-left corner; `scale` turns its points
/// into screenshot pixels.
pub fn snapshot(target: WindowTarget, origin: (f64, f64), scale: f64) -> Option<Snapshot> {
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
        scale,
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
    // The app's focused element may be in another of its windows, the
    // user's: use it only when it is in this one.
    let focused = element_attribute(&application(target.pid), "AXFocusedUIElement")
        .filter(|e| string_attribute(e, "AXValue").is_some())
        .filter(|e| {
            element_attribute(e, "AXWindow")
                .and_then(|w| window_id(&w))
                .is_some_and(|id| id == target.window_id)
        })
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

/// The path of a file URL, with its percent escapes decoded.
pub fn file_url_path(url: &str) -> Option<std::path::PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = rest
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).ok()?;
    Some(std::path::PathBuf::from(path.trim_end_matches('/')))
}

/// The file a window shows, from its AXDocument URL, with symlinks resolved.
pub fn document_path(window: &AXUIElement) -> Option<std::path::PathBuf> {
    string_attribute(window, "AXDocument")
        .and_then(|url| file_url_path(&url))
        .and_then(|p| std::fs::canonicalize(p).ok())
}

/// What came of asking a running app for a new window through its menus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NewWindow {
    Pressed,
    /// The item is there but greyed out, as while a dialog is open.
    Disabled,
    /// No item opens a plain window.
    Missing,
    /// The menus can't be read or the press failed, as just after launch.
    NotReady,
}

/// A menu item as `pick_new_window` sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    pub title: String,
    pub shortcut: Option<String>,
    /// The title of the submenu's item this one sits under, if any.
    pub parent: Option<String>,
    pub has_submenu: bool,
}

/// New Window as macOS titles it: Terminal's MainMenu.loctable in macOS
/// 26 for 21 languages, and Finder's New Finder Window in English and
/// Simplified Chinese.
const WINDOW_TITLES: &[&str] = &[
    "new window",
    "neues fenster",
    "nouvelle fenêtre",
    "nueva ventana",
    "nuova finestra",
    "nova janela",
    "nieuw venster",
    "nytt fönster",
    "nyt vindue",
    "nytt vindu",
    "uusi ikkuna",
    "nowe okno",
    "новое окно",
    "нове вікно",
    "yeni pencere",
    "新規ウインドウ",
    "새로운 윈도우",
    "新建窗口",
    "新增視窗",
    "new finder window",
    "新建访达窗口",
];
/// A document app's New: TextEdit's Edit.loctable in macOS 26, plus the
/// English New Document.
const DOCUMENT_TITLES: &[&str] = &[
    "new",
    "new document",
    "neu",
    "nouveau",
    "nuevo",
    "nuovo",
    "novo",
    "nieuw",
    "nytt",
    "ny",
    "uusi",
    "nowy",
    "новый",
    "новий",
    "yeni",
    "新規",
    "신규",
    "新建",
    "新增",
];

fn is_one_of(title: &str, titles: &[&str]) -> bool {
    let title = title.trim().to_lowercase();
    titles.contains(&title.as_str())
}

/// Which item opens a plain new window: one titled New Window, or the
/// Command-N item of a New Window submenu, as in Terminal, or a document
/// app's Command-N New. Command-N alone isn't enough: in Notes, Music or
/// Calendar it makes a note, playlist or event in the user's data.
pub fn pick_new_window(items: &[MenuEntry]) -> Option<usize> {
    let is_cmd_n = |e: &MenuEntry| e.shortcut.as_deref() == Some("cmd+n");
    let rank = |e: &MenuEntry| {
        if e.has_submenu {
            None
        } else if e.parent.is_none() && is_one_of(&e.title, WINDOW_TITLES) {
            Some(0)
        } else if e
            .parent
            .as_deref()
            .is_some_and(|p| is_one_of(p, WINDOW_TITLES))
            && is_cmd_n(e)
        {
            Some(1)
        } else if e.parent.is_none() && is_one_of(&e.title, DOCUMENT_TITLES) && is_cmd_n(e) {
            Some(2)
        } else {
            None
        }
    };
    (0..items.len())
        .filter_map(|i| rank(&items[i]).map(|r| (r, i)))
        .min()
        .map(|(_, i)| i)
}

/// Presses the item `pick_new_window` chooses in a running app's menus.
pub fn press_new_window(pid: i32) -> NewWindow {
    let Some(bar) = element_attribute(&application(pid), "AXMenuBar") else {
        return NewWindow::NotReady;
    };
    let menus = elements_attribute(&bar, "AXChildren");
    if menus.is_empty() {
        return NewWindow::NotReady;
    }
    let mut entries = Vec::new();
    let mut elements = Vec::new();
    let entry = |item: &AXUIElement, parent: Option<String>, has_submenu: bool| MenuEntry {
        title: string_attribute(item, "AXTitle").unwrap_or_default(),
        shortcut: shortcut(item),
        parent,
        has_submenu,
    };
    for title in &menus {
        for menu in elements_attribute(title, "AXChildren") {
            for item in elements_attribute(&menu, "AXChildren") {
                let submenu = elements_attribute(&item, "AXChildren");
                entries.push(entry(&item, None, !submenu.is_empty()));
                elements.push(item.clone());
                let parent = string_attribute(&item, "AXTitle");
                for sub in submenu {
                    for inner in elements_attribute(&sub, "AXChildren") {
                        entries.push(entry(&inner, parent.clone(), false));
                        elements.push(inner);
                    }
                }
            }
        }
    }
    match pick_new_window(&entries) {
        None => NewWindow::Missing,
        Some(i) => {
            let enabled = attribute(&elements[i], "AXEnabled")
                .and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
                .unwrap_or(true);
            // The item's own shortcut, where it has one: the app makes the
            // window as for a key press, and tells the catch at once. A
            // press through accessibility reports the window only after.
            let chord = entries[i]
                .shortcut
                .as_deref()
                .and_then(|s| crate::keychord::KeyChord::parse(s).ok());
            if !enabled {
                NewWindow::Disabled
            } else if let Some(chord) = chord {
                super::input::post_key_to_pid(pid, chord);
                NewWindow::Pressed
            } else if perform(&elements[i], "AXPress") {
                NewWindow::Pressed
            } else {
                NewWindow::NotReady
            }
        }
    }
}

/// A menu item's key equivalent in the computer tool's key syntax, such as
/// "cmd+shift+n". AXMenuItemCmdModifiers: bit 3 means no Command; bits 0-2
/// add Shift, Option and Control.
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
    scale: f64,
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
                let s = self.scale;
                let x = ((f.x - self.origin.0 + f.width / 2.0) * s).round() as i64;
                let y = ((f.y - self.origin.1 + f.height / 2.0) * s).round() as i64;
                let (w, h) = ((f.width * s).round() as i64, (f.height * s).round() as i64);
                line += &format!(" at ({x},{y}) {w}×{h}");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, shortcut: Option<&str>) -> MenuEntry {
        MenuEntry {
            title: title.into(),
            shortcut: shortcut.map(String::from),
            parent: None,
            has_submenu: false,
        }
    }

    #[test]
    fn picks_the_item_that_opens_a_window() {
        // Safari and VS Code: New Window, whatever else Command-N does.
        let vscode = [
            item("New Text File", Some("cmd+n")),
            item("New Window", Some("cmd+shift+n")),
        ];
        assert_eq!(pick_new_window(&vscode), Some(1));
        // TextEdit: a document app's New.
        assert_eq!(
            pick_new_window(&[item("New", Some("cmd+n")), item("Open…", Some("cmd+o"))]),
            Some(0)
        );
        // Terminal: New Window is a submenu; its Command-N item.
        let mut basic = item("Basic", Some("cmd+n"));
        basic.parent = Some("New Window".into());
        let mut submenu = item("New Window", None);
        submenu.has_submenu = true;
        assert_eq!(pick_new_window(&[submenu, basic]), Some(1));
        // Notes, Calendar, Music: Command-N makes the user's data.
        assert_eq!(pick_new_window(&[item("New Note", Some("cmd+n"))]), None);
        assert_eq!(pick_new_window(&[item("New Event", Some("cmd+n"))]), None);
        let mut playlist = item("Playlist", Some("cmd+n"));
        playlist.parent = Some("New".into());
        assert_eq!(pick_new_window(&[playlist]), None);
        // Other languages, as macOS ships them: Terminal's New Window,
        // TextEdit's New, and Notes' New Note, which must pick nothing.
        for title in [
            "Neues Fenster",
            "Nouvelle fenêtre",
            "Nueva ventana",
            "Nova Janela",
            "Nowe okno",
            "Новое окно",
            "Yeni Pencere",
            "新規ウインドウ",
            "새로운 윈도우",
            "新增視窗",
        ] {
            assert_eq!(
                pick_new_window(&[item(title, Some("cmd+n"))]),
                Some(0),
                "{title}"
            );
        }
        for title in [
            "Neu",
            "Nouveau",
            "Nuovo",
            "Nytt",
            "Новый",
            "新規",
            "신규",
            "新增",
        ] {
            assert_eq!(
                pick_new_window(&[item(title, Some("cmd+n"))]),
                Some(0),
                "{title}"
            );
        }
        for title in [
            "Neue Notiz",
            "Nouvelle note",
            "Nueva nota",
            "新規メモ",
            "새로운 메모",
            "新建备忘录",
            "新增備忘錄",
            "Новая заметка",
            "Yeni Not",
            "Nowa notatka",
        ] {
            assert_eq!(
                pick_new_window(&[item(title, Some("cmd+n"))]),
                None,
                "{title}"
            );
        }
        // Calculator: nothing.
        assert_eq!(pick_new_window(&[item("Close", Some("cmd+w"))]), None);
    }
}
