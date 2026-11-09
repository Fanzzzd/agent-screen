//! The agent's input for its window of the user's front app (#110). The
//! user works in one window of an app, such as a browser, while the agent
//! works in another of its windows. Posted keys would reach the user's
//! window, and a posted click would make the agent's window key and take
//! the user's typing. So nothing is posted there: text, Return, Escape,
//! Backspace, cmd+a and plain left clicks go through accessibility, to the
//! element the agent's last click picked, Tab and shift+Tab set focus on
//! the page's next or previous element, and everything else fails and
//! names the window that holds focus. The rules are in
//! `agent_screen_model::shared_app`.

use super::server::ControlError;
use crate::keychord::KeyChord;
use crate::mac::shared_app::{self as beside, Hit, Selection};
use crate::mac::{ax, windows};
use crate::model::{DisplayId, WindowTarget};
use agent_screen_model::shared_app::{
    ClickRoute, Effect, Front, KeyRoute, TextRoute, Watch, click_route, effect, key_route,
    text_route,
};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

const ROUTES: &str = "Beside the user's window, type puts text into the field your last click picked, through accessibility; Return confirms or presses what your last click picked, Escape cancels it, BackSpace and cmd+a edit the picked field, and Tab and shift+Tab move focus to the next or previous element of your page and pick it. Other keys have no route there that can't reach the user's window: use act_on_element, or wait until the user switches to another app. Agent Screen won't switch it for them.";

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

/// act_on_element's press, click or menu on a control that opens a menu,
/// beside the user's window.
pub fn menu_press_refusal(b: Beside, action: &str, refusal: &str) -> ControlError {
    let n = names(b);
    ControlError::failed(format!(
        "{app} is the user's front app with their window {user} focused, so {action} was not sent: {refusal}.",
        app = n.app,
        user = n.user,
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
            Hit::Text(e) | Hit::Press(e) | Hit::Menu(e) => Some(ax::Sendable(e)),
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
        match hit {
            Hit::Text(e) => Ok((Use::Text, beside::label(&e), ax::Sendable(e))),
            Hit::Press(e) => Ok((Use::Press, beside::label(&e), ax::Sendable(e))),
            Hit::Menu(e) => Ok((
                Use::Menu(beside::menu_refusal(&e)),
                beside::label(&e),
                ax::Sendable(e),
            )),
            Hit::Nothing(role) => Err(Some(role)),
            Hit::Unknown => Err(None),
        }
    })
    .await;
    let (kind, label, element) = match hit {
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
    match kind {
        Use::Text => {
            return Ok(json!({
                "windowId": target.window_id,
                "route": "accessibility",
                "note": format!("{why}, so no click was posted. The text field \"{label}\" under the point is picked: type puts text into it through accessibility, with no key events, at the end of its text or over what cmd+a selected."),
            }));
        }
        Use::Menu(refusal) => {
            return Err(ControlError::failed(format!(
                "{why}, so no click was posted. {refusal}."
            )));
        }
        Use::Press => {}
    }
    let display = display_watch(id, cx);
    let before = watch(&display, target, true, true, cx).await;
    let pressed = off(cx, move || ax::action_result(element.get(), "AXPress")).await;
    if pressed == ax::Action::NotTaken {
        return Err(ControlError::failed(format!(
            "{why}, so nothing was posted, and \"{label}\" under the point refused an accessibility press."
        )));
    }
    let seen = after_press(&display, target, before, cx).await;
    check_focus(b, &format!("The press on \"{label}\""), cx).await?;
    let what = format!(
        "{why}, so the click went to \"{label}\" as an accessibility press, with no mouse events"
    );
    let note = outcome(&what, pressed, seen)?;
    Ok(json!({
        "windowId": target.window_id,
        "route": "accessibility",
        "note": note,
    }))
}

/// What a click beside the user's window found under the point.
enum Use {
    Text,
    Press,
    /// A control whose press opens a menu, and why it isn't pressed.
    Menu(String),
}

/// How long a press gets to show an effect.
const EFFECT_WAIT: Duration = Duration::from_millis(1000);

/// The change counter of the display's picture, when it runs.
fn display_watch(id: DisplayId, cx: &AsyncApp) -> Option<Arc<crate::mac::capture::Latest>> {
    crate::ui::stream::global()?.read_with(cx, |s, _| s.latest(id))
}

/// What the display, the page and the app's windows show now. `wait`
/// gives the display's counter a moment to compare its first frame, as
/// when its stream just started for this action; `page` reads the page,
/// which takes longer.
async fn watch(
    display: &Option<Arc<crate::mac::capture::Latest>>,
    target: WindowTarget,
    wait: bool,
    page: bool,
    cx: &AsyncApp,
) -> Watch {
    let started = Instant::now();
    let counted = || display.as_ref().and_then(|d| d.counted());
    let mut changes = counted();
    while wait
        && changes.is_none()
        && display.is_some()
        && started.elapsed() < Duration::from_millis(300)
    {
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        changes = counted();
    }
    let (windows, page) = off(cx, move || {
        (
            ax::window_count(target.pid),
            page.then(|| beside::page_digest(target)).flatten(),
        )
    })
    .await;
    Watch {
        display_changes: changes,
        windows,
        page,
    }
}

/// Watch for the press's effect, up to `EFFECT_WAIT`: the display and the
/// app's windows all along, then the page.
async fn after_press(
    display: &Option<Arc<crate::mac::capture::Latest>>,
    target: WindowTarget,
    before: Watch,
    cx: &AsyncApp,
) -> Effect {
    let started = Instant::now();
    loop {
        let last = started.elapsed() >= EFFECT_WAIT;
        let now = watch(display, target, false, last, cx).await;
        let seen = effect(before, now);
        if seen == Effect::Seen || last {
            return seen;
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}

/// The answer to a press beside the user's window, by what the app
/// answered and what changed after: `what` says where it went. A press
/// after which nothing changed fails, so the agent doesn't take it for
/// done, as a link whose page cancels the click.
fn outcome(what: &str, pressed: ax::Action, seen: Effect) -> Result<String, ControlError> {
    let answered = match pressed {
        ax::Action::Unsure(error) => {
            format!(
                " The app answered the press with an error ({error:?}), which it also gives when the press ran."
            )
        }
        _ => String::new(),
    };
    let wait = EFFECT_WAIT.as_millis();
    match seen {
        Effect::Seen => Ok(format!(
            "{what}. The window changed after it.{answered} A page sees a click event but no mousedown or mouseup."
        )),
        Effect::NoneSeen => Err(ControlError::failed(format!(
            "{what}, but nothing changed: within {wait} ms, the page's text, values and address, your display and the app's windows all stayed the same.{answered} The page may ignore the press, as a link whose page cancels its click does, or act only on a real mouse click, which can't be posted while the user's window of the same app has focus. Don't repeat the press: take a screenshot to check, then try another element or act_on_element, or call request_help."
        ))),
        Effect::Unknown if answered.is_empty() => Ok(format!(
            "{what}. Accessibility didn't show the window's contents before and after, so Agent Screen can't say whether the press did anything: take a screenshot to check. A page sees a click event but no mousedown or mouseup."
        )),
        Effect::Unknown => Err(ControlError::failed(format!(
            "{what}.{answered} Accessibility didn't show the window's contents before and after, so Agent Screen can't say whether the press did anything. Take a screenshot to check before you press again."
        ))),
    }
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
            "{before}{app} is the user's front app with their window {user} focused, and Return and Tab in text are keys, which would reach {user}; nothing more was sent. Type the text without them, then press key Return or Tab on its own. {ROUTES}",
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

/// One key chord beside the user's window: Return, Escape, Backspace,
/// cmd+a, Tab and shift+Tab through accessibility, anything else refused.
/// A note for the reply, when the key went in.
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
    if let KeyRoute::FocusNext | KeyRoute::FocusPrevious = route {
        return tab(id, b, route == KeyRoute::FocusNext, name, cx)
            .await
            .map_err(|e| e.unwrap_or_else(fail));
    }
    let (element, last) = resolve(id, b, cx).await.map_err(&fail)?;
    let target = b.target;
    let display = display_watch(id, cx);
    let before = match route {
        KeyRoute::Confirm => Some(watch(&display, target, true, true, cx).await),
        _ => None,
    };
    let (done, element) = off(cx, move || {
        let e = element.get();
        let done = match route {
            KeyRoute::Confirm => beside::confirm(e)
                .map(|(action, answer)| (format!("its {action} action"), None, Some(answer))),
            KeyRoute::Cancel => {
                beside::cancel(e).map(|()| ("its AXCancel action".to_string(), None, None))
            }
            KeyRoute::SelectAll | KeyRoute::DeleteBackward if !beside::takes_text(e, target) => {
                Err(format!("\"{}\" is no text field", beside::label(e)))
            }
            KeyRoute::SelectAll => Ok((
                "selecting its text".into(),
                Some(beside::select_all(e)),
                None,
            )),
            KeyRoute::DeleteBackward => beside::delete_backward(e, last.as_ref())
                .map(|s| ("deleting through its value".into(), Some(s), None)),
            KeyRoute::Post | KeyRoute::Refuse | KeyRoute::FocusNext | KeyRoute::FocusPrevious => {
                Err("no route".into())
            }
        };
        (done.map(|d| (d, beside::label(e))), element)
    })
    .await;
    let ((how, selection, answer), label) = done.map_err(fail)?;
    if let Some(selection) = selection {
        keep_selection(id, &element, selection);
    }
    let seen = match (before, answer) {
        (Some(before), Some(_)) => Some(after_press(&display, target, before, cx).await),
        _ => None,
    };
    check_focus(b, &format!("The key {name}"), cx).await?;
    let what = format!(
        "{app} is the user's front app with their window {user} focused, so {name} was not posted: it went to \"{label}\" in your window through accessibility, by {how}",
        app = n.app,
        user = n.user,
    );
    match (answer, seen) {
        (Some(answer), Some(seen)) => outcome(&what, answer, seen),
        _ => Ok(format!("{what}.")),
    }
}

/// Tab or shift+Tab beside the user's window: focus moves to the next or
/// previous element of the agent's page through accessibility, and that
/// element is picked for the keys and text after. Err(Ok) when focus went
/// in but the app moved its focused window, Err(Err) a reason for the
/// caller to word.
async fn tab(
    id: DisplayId,
    b: Beside,
    forward: bool,
    name: &str,
    cx: &AsyncApp,
) -> Result<String, Result<ControlError, String>> {
    // From what the last click or Tab picked; with nothing picked, from
    // the page's focused element or its start.
    let from = resolve(id, b, cx).await.ok().map(|(e, _)| e);
    let target = b.target;
    let moved = off(cx, move || {
        beside::tab(target, from.as_ref().map(|e| e.get()), forward).map(|(element, label)| {
            let point = ax::frame(&element)
                .map(|f| (f.x + f.width / 2.0, f.y + f.height / 2.0))
                .unwrap_or_default();
            (ax::Sendable(element), label, point)
        })
    })
    .await;
    let (element, label, point) = moved.map_err(Err)?;
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
    check_focus(b, &format!("The key {name}"), cx)
        .await
        .map_err(Ok)?;
    let n = names(b);
    Ok(format!(
        "{app} is the user's front app with their window {user} focused, so {name} was not posted: focus moved to \"{label}\", the {which} element of the page in your window that takes focus, through accessibility, and it is picked for the keys and text after. This order is an approximation of the page's tab order: elements in accessibility tree order whose focus can be set, so a page's tabindex order, and elements it keeps out of the order, can differ. The page saw no Tab key event.",
        app = n.app,
        user = n.user,
        which = if forward { "next" } else { "previous" },
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
