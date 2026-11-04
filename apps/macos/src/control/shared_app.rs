//! The agent's input for its window of the user's front app (#110). The
//! user works in one window of an app, such as a browser, while the agent
//! works in another of its windows. Posted keys would reach the user's
//! window, and a posted click would make the agent's window key and take
//! the user's typing. So nothing is posted there: text, Return, Escape,
//! Backspace, cmd+a and plain left clicks go through accessibility, to the
//! element the agent's last click picked, and everything else fails and
//! names the window that holds focus. The rules are in
//! `agent_screen_model::shared_app`.

use super::server::ControlError;
use crate::keychord::KeyChord;
use crate::mac::shared_app::{self as beside, Hit, Selection};
use crate::mac::{ax, windows};
use crate::model::{DisplayId, WindowTarget};
use agent_screen_model::shared_app::{
    ClickRoute, Front, KeyRoute, TextRoute, click_route, key_route, text_route,
};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;

/// The agent's window, in an app that is the user's front app with
/// another window focused.
#[derive(Clone, Copy, Debug)]
pub struct Beside {
    pub target: WindowTarget,
    /// The window that has focus, the user's.
    pub focused: u32,
}

async fn off<R: Send + 'static>(cx: &AsyncApp, f: impl FnOnce() -> R + Send + 'static) -> R {
    cx.background_executor().spawn(async move { f() }).await
}

/// The state of `target`'s app, and its focused window when it is the
/// front app.
pub async fn front(target: WindowTarget, cx: &AsyncApp) -> (Front, Option<u32>) {
    let pid = target.pid;
    if crate::mac::input::front() != Some(pid) {
        return (Front::No, None);
    }
    let focused = off(cx, move || ax::focused_window_id(pid)).await;
    (Front::of(true, focused, target.window_id), focused)
}

/// The app's name and both windows' titles, for messages.
struct Names {
    app: String,
    user: String,
    agent: String,
}

fn names(b: Beside) -> Names {
    let user = windows::find(b.focused);
    let agent = windows::find(b.target.window_id);
    let app = agent
        .as_ref()
        .or(user.as_ref())
        .map_or_else(|| "The app".into(), |w| w.app.clone());
    let title = |w: Option<windows::Window>, id: u32| {
        w.map(|w| w.title)
            .filter(|t| !t.is_empty())
            .map_or_else(|| format!("window {id}"), |t| format!("\"{t}\""))
    };
    Names {
        app,
        user: title(user, b.focused),
        agent: title(agent, b.target.window_id),
    }
}

const ROUTES: &str = "Beside the user's window, type puts text into the field your last click picked, through accessibility; Return confirms or presses what your last click picked, Escape cancels it, and BackSpace and cmd+a edit the picked field. Other keys have no route there that can't reach the user's window: use act_on_element, or wait until the user switches to another app. Agent Screen won't switch it for them.";

/// Keys that can't go in beside the user's window.
pub fn refusal(b: Beside, what: &str) -> ControlError {
    let n = names(b);
    ControlError::failed(format!(
        "{app} is the user's front app with their window {user} focused, not your window {agent}. Keys posted to {app} reach {user}, so {what} was not sent, and nothing reached either window. {ROUTES}",
        app = n.app,
        user = n.user,
        agent = n.agent,
    ))
}

/// Keys when accessibility didn't say which window has focus.
pub fn unknown_focus(app: &str) -> ControlError {
    ControlError::failed(format!(
        "{app} is the foreground app, but Accessibility could not verify its focused window, so posted keys could reach the user's window. User idleness does not make this routing safe. The pending keys were refused. Use act_on_element set_value or a supported press action, or wait until the user switches to another app before retrying. Agent Screen will not switch it for them."
    ))
}

/// What the agent's last click on each display picked.
struct Picked {
    window: WindowTarget,
    point: (f64, f64),
    /// The element, when the click went through accessibility; else it is
    /// found at `point` when needed.
    element: Option<ax::Element>,
    /// What the agent's last edit there left.
    selection: Option<Selection>,
}

thread_local! {
    static PICKED: RefCell<HashMap<DisplayId, Picked>> = RefCell::new(HashMap::new());
}

/// A click posted at `point` in `window`: what keys beside the user's
/// window would go to later.
pub fn note_click(id: DisplayId, window: WindowTarget, point: (f64, f64)) {
    PICKED.with(|p| {
        p.borrow_mut().insert(
            id,
            Picked {
                window,
                point,
                element: None,
                selection: None,
            },
        )
    });
}

/// Posted keys may have moved focus away from what the last click picked,
/// as Tab does.
pub fn forget(id: DisplayId) {
    PICKED.with(|p| p.borrow_mut().remove(&id));
}

/// What the last click picked in a window, to take off the main thread.
struct Pick {
    /// The element, or None to find it at `point`.
    element: Option<ax::Sendable>,
    point: (f64, f64),
    selection: Option<Selection>,
}

/// What the last click picked in `target`'s window, and the agent's last
/// edit there.
fn picked(id: DisplayId, target: WindowTarget) -> Option<Pick> {
    PICKED.with(|p| {
        let p = p.borrow();
        let picked = p.get(&id).filter(|p| p.window == target)?;
        Some(Pick {
            element: picked.element.clone().map(ax::Sendable),
            point: picked.point,
            selection: picked.selection.clone(),
        })
    })
}

fn keep_selection(id: DisplayId, element: &ax::Sendable, selection: Selection) {
    PICKED.with(|p| {
        if let Some(picked) = p.borrow_mut().get_mut(&id) {
            picked.element = Some(element.get().clone());
            picked.selection = Some(selection);
        }
    });
}

fn nothing_picked(n: &Names) -> String {
    format!(
        "you haven't clicked anything in your window {} since your last keys, so nothing is picked for them",
        n.agent
    )
}

/// The picked element, found off the main thread.
async fn resolve(
    id: DisplayId,
    b: Beside,
    cx: &AsyncApp,
) -> Result<(ax::Sendable, Option<Selection>), String> {
    let Some(Pick {
        element,
        point,
        selection,
    }) = picked(id, b.target)
    else {
        return Err(nothing_picked(&names(b)));
    };
    let target = b.target;
    let element = off(cx, move || {
        element.or_else(|| match beside::hit(target, point) {
            Hit::Text(e) | Hit::Press(e) => Some(ax::Sendable(e)),
            Hit::Nothing(_) | Hit::Unknown => None,
        })
    })
    .await;
    match element {
        Some(e) => Ok((e, selection)),
        None => Err(format!(
            "what your last click hit in your window {} is no field or control accessibility can reach",
            names(b).agent
        )),
    }
}

/// The front app moved focus to the agent's window during an
/// accessibility action: give it back, as the user's click would, and say
/// so.
async fn check_focus(b: Beside, what: &str, cx: &AsyncApp) -> Result<(), ControlError> {
    let pid = b.target.pid;
    let now = off(cx, move || ax::focused_window_id(pid)).await;
    if now.is_none_or(|now| now == b.focused) {
        return Ok(());
    }
    let user = WindowTarget {
        pid,
        window_id: b.focused,
    };
    off(cx, move || ax::make_main(user)).await;
    let n = names(b);
    log::warn!(
        "shared app: {what} moved {}'s focus to window {now:?}",
        n.app
    );
    Err(ControlError::failed(format!(
        "{what} went in, but {app} then moved its focus from the user's window {user} to window {now}, so Agent Screen gave it back to {user}. Take a screenshot before you go on.",
        app = n.app,
        user = n.user,
        now = now.unwrap_or_default(),
    )))
}

/// A click in the agent's window beside the user's. Nothing is posted: a
/// field under the point is picked for `type`, a control pressed.
pub async fn click(
    id: DisplayId,
    point: (f64, f64),
    b: Beside,
    left: bool,
    count: i64,
    modifiers: bool,
    cx: &AsyncApp,
) -> Result<Value, ControlError> {
    let n = names(b);
    let why = format!(
        "{app} is the user's front app with their window {user} focused, and a posted click on your window {agent} would make it {app}'s key window and take the user's typing",
        app = n.app,
        user = n.user,
        agent = n.agent,
    );
    if click_route(Front::OtherFocused, left, count, modifiers) == ClickRoute::Refuse {
        return Err(ControlError::failed(format!(
            "{why}, so nothing was sent. Beside the user's window only a single left click without modifiers works, through accessibility: it presses the control under the point or picks the text field there for type. Use act_on_element for other actions, or wait until the user switches to another app."
        )));
    }
    let target = b.target;
    let hit = off(cx, move || {
        let hit = beside::hit(target, point);
        let label = match &hit {
            Hit::Text(e) | Hit::Press(e) => beside::label(e),
            _ => String::new(),
        };
        let pressed = match &hit {
            Hit::Press(e) => ax::try_action(e, "AXPress"),
            _ => false,
        };
        (
            match hit {
                Hit::Text(e) => Ok((true, ax::Sendable(e))),
                Hit::Press(e) => Ok((false, ax::Sendable(e))),
                Hit::Nothing(role) => Err(Some(role)),
                Hit::Unknown => Err(None),
            },
            label,
            pressed,
        )
    })
    .await;
    let (picked, label, pressed) = hit;
    let (field, element) = match picked {
        Ok(found) => found,
        Err(role) => {
            let there = role.map_or_else(
                || "accessibility names nothing there".to_string(),
                |r| format!("the element there is {r}"),
            );
            return Err(ControlError::failed(format!(
                "{why}, so nothing was sent; and nothing under the point takes a press or text through accessibility ({there}). Click a button, link or text field, use act_on_element, or wait until the user switches to another app."
            )));
        }
    };
    PICKED.with(|p| {
        p.borrow_mut().insert(
            id,
            Picked {
                window: target,
                point,
                element: Some(element.get().clone()),
                selection: None,
            },
        )
    });
    if field {
        return Ok(json!({
            "windowId": target.window_id,
            "route": "accessibility",
            "note": format!("{why}, so no click was posted. The text field \"{label}\" under the point is picked: type puts text into it through accessibility, with no key events, at the end of its text or over what cmd+a selected."),
        }));
    }
    if !pressed {
        return Err(ControlError::failed(format!(
            "{why}, so nothing was posted, and \"{label}\" under the point refused an accessibility press."
        )));
    }
    check_focus(b, &format!("The press on \"{label}\""), cx).await?;
    Ok(json!({
        "windowId": target.window_id,
        "route": "accessibility",
        "note": format!("{why}, so the click went to \"{label}\" as an accessibility press, with no mouse events. A page sees a click event but no mousedown or mouseup."),
    }))
}

/// Text into the picked field beside the user's window. `posted` is how
/// many characters went out as keys before the app came to the front.
pub async fn type_text(
    id: DisplayId,
    b: Beside,
    text: &str,
    posted: usize,
    cx: &AsyncApp,
) -> Result<Value, ControlError> {
    let n = names(b);
    let before = match posted {
        0 => String::new(),
        n => format!("{n} characters went in as keys, then "),
    };
    if text_route(Front::OtherFocused, text) == TextRoute::RefuseKeysInText {
        return Err(ControlError::failed(format!(
            "{before}{app} is the user's front app with their window {user} focused, and Return and Tab in text are keys, which would reach {user}; nothing more was sent. Type the text without them, then press key Return on its own. {ROUTES}",
            app = n.app,
            user = n.user,
        )));
    }
    let fail = |why: String| {
        ControlError::failed(format!(
            "{before}{app} is the user's front app with their window {user} focused, so keys would reach {user}, and the text couldn't go in through accessibility instead: {why}. Nothing reached the user's window. Click the text field in your window first, which picks it, then type again.",
            app = n.app,
            user = n.user,
        ))
    };
    let (field, last) = resolve(id, b, cx).await.map_err(&fail)?;
    let target = b.target;
    let typed = text.to_string();
    let (result, field, label) = off(cx, move || {
        if !beside::takes_text(field.get(), target) {
            let label = beside::label(field.get());
            return (Err(format!("\"{label}\" is no text field that says what it holds, as a password field doesn't")), field, label);
        }
        let label = beside::label(field.get());
        (beside::insert(field.get(), &typed, last.as_ref()), field, label)
    })
    .await;
    let selection = result.map_err(fail)?;
    keep_selection(id, &field, selection);
    check_focus(b, "The text", cx).await?;
    Ok(json!({
        "route": "accessibility",
        "note": format!(
            "{before}{app} is the user's front app with their window {user} focused, so no key events were sent: the text went into the field \"{label}\" of your window through accessibility, and reads back with it. A page that acts on each key, such as a search box's suggestions, saw no keys.",
            app = n.app,
            user = n.user,
        ),
    }))
}

/// One key chord beside the user's window: Return, Escape, Backspace and
/// cmd+a through accessibility, anything else refused. A note for the
/// reply, when the key went in.
pub async fn key(
    id: DisplayId,
    b: Beside,
    chord: KeyChord,
    name: &str,
    cx: &AsyncApp,
) -> Result<String, ControlError> {
    let route = key_route(Front::OtherFocused, chord);
    if route == KeyRoute::Refuse {
        return Err(refusal(b, &format!("the key {name}")));
    }
    let n = names(b);
    let fail = |why: String| {
        ControlError::failed(format!(
            "{app} is the user's front app with their window {user} focused, so the key {name} was not posted, and it couldn't go in through accessibility: {why}. Nothing reached either window. {ROUTES}",
            app = n.app,
            user = n.user,
        ))
    };
    let (element, last) = resolve(id, b, cx).await.map_err(&fail)?;
    let target = b.target;
    let (done, element) = off(cx, move || {
        let e = element.get();
        let done = match route {
            KeyRoute::Confirm => {
                beside::confirm(e).map(|action| (format!("its {action} action"), None))
            }
            KeyRoute::Cancel => {
                beside::cancel(e).map(|()| ("its AXCancel action".to_string(), None))
            }
            KeyRoute::SelectAll | KeyRoute::DeleteBackward if !beside::takes_text(e, target) => {
                Err(format!("\"{}\" is no text field", beside::label(e)))
            }
            KeyRoute::SelectAll => Ok(("selecting its text".into(), Some(beside::select_all(e)))),
            KeyRoute::DeleteBackward => beside::delete_backward(e, last.as_ref())
                .map(|s| ("deleting through its value".into(), Some(s))),
            KeyRoute::Post | KeyRoute::Refuse => Err("no route".into()),
        };
        (done.map(|d| (d, beside::label(e))), element)
    })
    .await;
    let ((how, selection), label) = done.map_err(fail)?;
    if let Some(selection) = selection {
        keep_selection(id, &element, selection);
    }
    check_focus(b, &format!("The key {name}"), cx).await?;
    Ok(format!(
        "{app} is the user's front app with their window {user} focused, so {name} was not posted: it went to \"{label}\" in your window through accessibility, by {how}.",
        app = n.app,
        user = n.user,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unverified_focus_is_refused_without_blaming_the_user() {
        let unknown = super::unknown_focus("Dia").message;
        assert!(unknown.contains("Accessibility could not verify"));
        assert!(unknown.contains("User idleness does not make this routing safe"));
        assert!(!unknown.contains("user is working"));
    }
}
