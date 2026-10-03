//! The Accessibility API: other apps' windows and their element trees.

use crate::model::{Rect, WindowTarget};
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CGPoint, CGSize,
};
use std::ffi::c_void;
use std::ptr::NonNull;

pub type Element = CFRetained<AXUIElement>;

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

pub fn set_attribute(element: &AXUIElement, name: &str, value: &CFType) -> bool {
    let name = CFString::from_str(name);
    unsafe { element.set_attribute_value(&name, value) == AXError::Success }
}

pub fn perform(element: &AXUIElement, action: &str) -> bool {
    let action = CFString::from_str(action);
    unsafe { element.perform_action(&action) == AXError::Success }
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
pub fn move_into(target: WindowTarget, bounds: Rect) -> Result<(), MoveError> {
    let window = window_element(target).ok_or(MoveError::NoWindow)?;
    let area = bounds.inset(40.0, 40.0);
    let size = attribute(&window, "AXSize")
        .and_then(|v| size_value(&v))
        .unwrap_or(CGSize::new(800.0, 600.0));
    let fitted = (size.width.min(area.width), size.height.min(area.height));
    let (mid_x, mid_y) = area.mid();
    if !set_position(&window, mid_x - fitted.0 / 2.0, mid_y - fitted.1 / 2.0) {
        return Err(MoveError::Rejected);
    }
    if fitted != (size.width, size.height) {
        set_size(&window, fitted.0, fitted.1);
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

/// The focused window of an app.
pub fn focused_window(pid: i32) -> Option<WindowTarget> {
    let app = application(pid);
    let window = element_attribute(&app, "AXFocusedWindow")
        .or_else(|| element_attribute(&app, "AXMainWindow"))?;
    Some(WindowTarget {
        pid,
        window_id: window_id(&window)?,
    })
}

// MARK: Element trees

/// A window as an agent that works per window sees it: numbered elements it
/// can act on. Coordinates are points from the window's top-left corner.
pub struct Snapshot {
    pub tree: String,
    pub elements: Vec<Element>,
    pub truncated: bool,
}

pub fn snapshot(target: WindowTarget) -> Option<Snapshot> {
    let window = window_element(target)?;
    // Chromium and Electron apps only publish their web content tree once a
    // client asks for it. Other apps ignore the attribute.
    set_attribute(
        &application(target.pid),
        "AXManualAccessibility",
        &CFBoolean::new(true),
    );
    let origin = frame(&window).map_or((0.0, 0.0), |f| (f.x, f.y));
    let mut walker = TreeWalker {
        origin,
        lines: Vec::new(),
        elements: Vec::new(),
        truncated: false,
    };
    walker.visit(&window, 0);
    Some(Snapshot {
        tree: walker.lines.join("\n"),
        elements: walker.elements,
        truncated: walker.truncated,
    })
}

/// Depth-first walk that numbers every element worth naming. Unlabelled
/// containers are skipped but their children are kept, which keeps the tree
/// short without losing anything an agent could act on.
struct TreeWalker {
    origin: (f64, f64),
    lines: Vec<String>,
    elements: Vec<Element>,
    truncated: bool,
}

const MAX_ELEMENTS: usize = 400;
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
        if self.elements.len() >= MAX_ELEMENTS {
            self.truncated = true;
            return;
        }
        let Some(values) = multiple_values(element) else {
            return;
        };
        let string = |i: usize| {
            let text = values[i].downcast_ref::<CFString>()?.to_string();
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.chars().take(80).collect::<String>())
        };
        let boolean = |i: usize| values[i].downcast_ref::<CFBoolean>().map(|b| b.as_bool());
        let role = string(0).unwrap_or_else(|| "AXUnknown".into());
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
                let x = (f.x - self.origin.0) as i64;
                let y = (f.y - self.origin.1) as i64;
                line += &format!(" ({x},{y} {}×{})", f.width as i64, f.height as i64);
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
