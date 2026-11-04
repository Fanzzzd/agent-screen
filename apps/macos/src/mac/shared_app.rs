//! Input for an agent's window of the user's front app, through
//! accessibility, with no events posted (#110; the rules are in
//! `agent_screen_model::shared_app`). Every call blocks on the app, so
//! everything here runs off the main thread.

use super::ax::{self, Element};
use crate::model::WindowTarget;
use agent_screen_model::shared_app::{self as rules, ElementUse};
use agent_screen_model::typing::landed;
use objc2_core_foundation::CFString;
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
    /// Nothing that takes a press or text: its role.
    Nothing(String),
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
    for _ in 0..8 {
        let role = role(&element);
        if matches!(role.as_str(), "AXWindow" | "AXWebArea" | "AXSheet") {
            break;
        }
        match rules::element_use(&role, &ax::actions(&element)) {
            ElementUse::Text => return Hit::Text(element),
            ElementUse::Press => return Hit::Press(element),
            ElementUse::Nothing => {}
        }
        let Some(parent) = ax::element_attribute(&element, "AXParent") else {
            break;
        };
        element = parent;
    }
    Hit::Nothing(first)
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

/// Return: the element's confirm action, or its press. The action taken.
pub fn confirm(element: &ax::Element) -> Result<&'static str, String> {
    let actions = ax::actions(element);
    if actions.iter().any(|a| a == "AXConfirm") && ax::try_action(element, "AXConfirm") {
        return Ok("AXConfirm");
    }
    let field = rules::element_use(&role(element), &actions) == ElementUse::Text;
    if !field && actions.iter().any(|a| a == "AXPress") && ax::try_action(element, "AXPress") {
        return Ok("AXPress");
    }
    Err(format!(
        "\"{}\" has no confirm action, so Return can't reach it without a key event{}",
        label(element),
        if field {
            "; a web form submits through its button, so click that"
        } else {
            ""
        }
    ))
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
