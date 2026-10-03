//! The tools Agent Sessions see. Each one turns into control socket calls.
//!
//! Schemas are written out as JSON rather than derived, so what the model
//! reads is exactly what is here.

use crate::control::{Control, ControlError, Object};
use crate::session::Session;
use serde_json::{Value, json};
use std::time::Duration;

/// What a tool returns: content blocks, or one JSON object that is sent as
/// both text and `structuredContent`.
pub enum Output {
    Content(Vec<Value>),
    Object(Value),
}

pub fn text(value: impl Into<String>) -> Value {
    json!({ "type": "text", "text": value.into() })
}

fn image(data: &Value, mime_type: &Value) -> Value {
    json!({ "type": "image", "data": data, "mimeType": mime_type })
}

// MARK: Definitions

fn display_id() -> Value {
    json!({ "type": "integer", "description": "The display handle returned by create_display." })
}

fn coordinate() -> Value {
    json!({
        "type": "array",
        "items": { "type": "number" },
        "minItems": 2,
        "maxItems": 2,
        "description": "[x, y] in screenshot pixels, from the display's top-left corner."
    })
}

fn timeout() -> Value {
    json!({ "type": "integer", "description": "Seconds to wait before giving up. Default 100, at most 600." })
}

fn window_id() -> Value {
    json!({ "type": "integer", "description": "A window on your display, from list_windows or launch_app." })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

struct Hints {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
}

const PLAIN: Hints = Hints { read_only: false, destructive: false, idempotent: false };
const READ_ONLY: Hints = Hints { read_only: true, destructive: false, idempotent: false };

fn tool(name: &str, title: &str, description: &[&str], input: Value, hints: Hints) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description.join(" "),
        "inputSchema": input,
        "annotations": {
            "title": title,
            "readOnlyHint": hints.read_only,
            "destructiveHint": hints.destructive,
            "idempotentHint": hints.idempotent,
            "openWorldHint": false
        }
    })
}

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "create_display",
            "Create Agent Display",
            &[
                "Get a private virtual display for this session to operate GUI apps on, without touching the user's screen,",
                "cursor or keyboard. Call it only when you need a screen. Calling it again returns the same display.",
                "The returned display_id stays valid until end_display or until this session ends.",
                "Put windows on it with launch_app or place_window, then drive it with the computer tool.",
            ],
            object(
                json!({
                    "title": { "type": "string", "description": "Short task title shown to the user, e.g. \"Book flights\"." },
                    "width": { "type": "integer", "description": "Width in points. Default 1440." },
                    "height": { "type": "integer", "description": "Height in points. Default 900." }
                }),
                &[],
            ),
            PLAIN,
        ),
        tool(
            "end_display",
            "End Agent Display",
            &[
                "Finish with the display. It accepts no more input and its display_id stops working. If windows remain,",
                "it waits for the user to review them; give each a set_disposition first. An empty display closes.",
            ],
            object(json!({ "display_id": display_id() }), &["display_id"]),
            Hints { destructive: true, ..PLAIN },
        ),
        tool(
            "computer",
            "Operate the display",
            &[
                "See and operate your Agent Display. Works like Anthropic's computer-use tool, scoped to one display:",
                "screenshot pixels and coordinates are the same space. Every action except wait returns a fresh screenshot.",
                "key takes xdotool-style chords such as \"Return\", \"cmd+l\" or \"cmd+shift+t\". Click actions accept held",
                "modifiers in text, e.g. \"shift\". Fails with a takeover error while the user is operating the display;",
                "then call wait_for_control.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "action": { "type": "string", "enum": [
                        "screenshot", "left_click", "right_click", "middle_click", "double_click", "triple_click",
                        "mouse_move", "left_click_drag", "scroll", "type", "key", "wait"
                    ] },
                    "coordinate": coordinate(),
                    "start_coordinate": coordinate(),
                    "text": { "type": "string" },
                    "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "scroll_amount": { "type": "integer", "description": "Scroll wheel clicks. Default 3." },
                    "duration": { "type": "number", "description": "Seconds, for wait." }
                }),
                &["display_id", "action"],
            ),
            PLAIN,
        ),
        tool(
            "wait_for_control",
            "Wait for the user to hand the display back",
            &[
                "Block until the user stops operating your display (a Takeover) and hands it back, or until the timeout.",
                "Call it after any tool fails with a takeover error. Returns at once if you already have control.",
            ],
            object(json!({ "display_id": display_id(), "timeout_seconds": timeout() }), &["display_id"]),
            READ_ONLY,
        ),
        tool(
            "request_help",
            "Ask the user to step in",
            &[
                "Ask the user to take over your display for something you cannot do, such as a login, a captcha or a",
                "two-factor code. The user gets a system notification with your reason. The call then waits like",
                "wait_for_control until the user has stepped in and handed the display back, or until the timeout.",
                "On a timeout the request stays open; call wait_for_control to keep waiting.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "reason": { "type": "string", "description": "What you need the user to do, in one short sentence." },
                    "timeout_seconds": timeout()
                }),
                &["display_id", "reason"],
            ),
            PLAIN,
        ),
        tool(
            "list_windows",
            "List windows on the display",
            &[
                "List app windows on the display, front to back, with ids and bounds in screenshot pixels. origin says how",
                "a window got here (launched, placed, handedOver by the user, found, unknown); disposition is your current",
                "set_disposition verdict or null.",
            ],
            object(json!({ "display_id": display_id() }), &["display_id"]),
            READ_ONLY,
        ),
        tool(
            "place_window",
            "Place a window on the display",
            &["Move an existing app window onto the display. Get window ids from list_windows (pass another display's id) or from launch_app."],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": { "type": "integer", "description": "The window to move." }
                }),
                &["display_id", "window_id"],
            ),
            PLAIN,
        ),
        tool(
            "launch_app",
            "Launch an app on the display",
            &["Open an app in the background and put its new window on the display. If the app is already running, a new window is opened for you instead of taking the user's."],
            object(
                json!({
                    "display_id": display_id(),
                    "app": { "type": "string", "description": "App name (\"Safari\") or bundle id (\"com.apple.Safari\")." }
                }),
                &["display_id", "app"],
            ),
            PLAIN,
        ),
        tool(
            "set_disposition",
            "Keep or close a window when the display goes",
            &[
                "Say what should happen to one of your windows when the user dismisses this display: keep it for the user",
                "to review (a result page, a filled form) or close it (scratch windows). Set it as soon as you know and",
                "change it any time; your session may end without warning. The user has the final say. Windows the user",
                "handed over always go back to them, whatever you choose.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": { "type": "integer", "description": "A window on the display, from list_windows." },
                    "disposition": { "type": "string", "enum": ["keep", "close"] }
                }),
                &["display_id", "window_id", "disposition"],
            ),
            Hints { idempotent: true, ..PLAIN },
        ),
        tool(
            "get_window_state",
            "See one window",
            &[
                "Screenshot of one window on your display plus its accessibility tree. Every element has a number in",
                "brackets that act_on_element takes. Each new state of a window renumbers its elements, so always use the",
                "numbers from the latest get_window_state or act_on_element result.",
            ],
            object(json!({ "display_id": display_id(), "window_id": window_id() }), &["display_id", "window_id"]),
            READ_ONLY,
        ),
        tool(
            "act_on_element",
            "Act on a window element",
            &[
                "Act on an element from get_window_state, then return the window's new state. press, show_menu, focus,",
                "set_value and scroll_into_view use accessibility actions, which reach the app without focusing it; click,",
                "double_click and right_click send a real click at the element's center. press falls back to a click when",
                "the element has no press action. Fails with a takeover error while the user operates the display.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": window_id(),
                    "element": { "type": "integer", "description": "The bracketed number from get_window_state." },
                    "action": { "type": "string", "enum": [
                        "press", "click", "double_click", "right_click", "show_menu", "focus", "set_value", "scroll_into_view"
                    ] },
                    "value": { "type": "string", "description": "Text for set_value." }
                }),
                &["display_id", "window_id", "element", "action"],
            ),
            PLAIN,
        ),
    ]
}

// MARK: Arguments

/// Typed access to a tool's arguments. Errors read like the TypeScript
/// server's schema failures, so the model gets the same feedback.
struct Args<'a>(&'a Object);

impl Args<'_> {
    fn int(&self, key: &str) -> Result<Option<i64>, ControlError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_i64()
                .or_else(|| value.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
                .map(Some)
                .ok_or_else(|| ControlError::invalid(format!("{key} must be an integer"))),
        }
    }

    fn require_int(&self, key: &str) -> Result<i64, ControlError> {
        self.int(key)?.ok_or_else(|| ControlError::invalid(format!("{key} is required")))
    }

    fn number(&self, key: &str) -> Result<Option<f64>, ControlError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|f| f.is_finite())
                .map(Some)
                .ok_or_else(|| ControlError::invalid(format!("{key} must be a finite number"))),
        }
    }

    fn string(&self, key: &str) -> Result<Option<String>, ControlError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(ControlError::invalid(format!("{key} must be a string"))),
        }
    }

    fn one_of(&self, key: &str, allowed: &[&str]) -> Result<Option<String>, ControlError> {
        let value = self.string(key)?;
        if let Some(v) = &value
            && !allowed.contains(&v.as_str())
        {
            return Err(ControlError::invalid(format!("{key} must be one of {}", allowed.join(", "))));
        }
        Ok(value)
    }

    fn coordinate(&self, key: &str) -> Result<Option<(f64, f64)>, ControlError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) if items.len() == 2 => {
                match (items[0].as_f64().filter(|f| f.is_finite()), items[1].as_f64().filter(|f| f.is_finite())) {
                    (Some(x), Some(y)) => Ok(Some((x, y))),
                    _ => Err(ControlError::invalid(format!("{key} must be [x, y] numbers"))),
                }
            }
            Some(_) => Err(ControlError::invalid(format!("{key} must be [x, y]"))),
        }
    }
}

fn require_coordinate(value: Option<(f64, f64)>, action: &str) -> Result<(f64, f64), ControlError> {
    value.ok_or_else(|| ControlError::invalid(format!("coordinate is required for {action}")))
}

fn require_text(value: Option<String>, action: &str) -> Result<String, ControlError> {
    value
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ControlError::invalid(format!("text is required for {action}")))
}

// MARK: Handlers

const SETTLE: Duration = Duration::from_millis(350);

pub async fn run(name: &str, args: &Object, session: &Session, control: &Control) -> Result<Output, ControlError> {
    let a = Args(args);
    match name {
        "create_display" => {
            let title = a.string("title")?.unwrap_or_else(|| session.folder.clone());
            let reply = control
                .call(
                    "createDisplay",
                    json!({
                        "sessionKey": session.key,
                        "client": session.client,
                        "title": title,
                        "width": a.int("width")?,
                        "height": a.int("height")?
                    }),
                )
                .await?;
            Ok(Output::Object(json!({
                "display_id": reply.get("displayId"),
                "width": reply.get("width"),
                "height": reply.get("height"),
                "reused": reply.get("reused")
            })))
        }
        "end_display" => {
            control.call("endDisplay", json!({ "displayId": a.require_int("display_id")? })).await?;
            Ok(Output::Object(json!({ "ended": true })))
        }
        "wait_for_control" => {
            let reply = control
                .call(
                    "waitForControl",
                    json!({ "displayId": a.require_int("display_id")?, "timeoutSeconds": a.int("timeout_seconds")? }),
                )
                .await?;
            Ok(Output::Object(describe_wait(&reply)))
        }
        "request_help" => {
            let display = a.require_int("display_id")?;
            let reason = a.string("reason")?.ok_or_else(|| ControlError::invalid("reason is required"))?;
            control.call("requestHelp", json!({ "displayId": display, "reason": reason })).await?;
            let reply = control
                .call("waitForControl", json!({ "displayId": display, "timeoutSeconds": a.int("timeout_seconds")? }))
                .await?;
            Ok(Output::Object(describe_wait(&reply)))
        }
        "list_windows" => {
            let reply = control.call("listWindows", json!({ "displayId": a.require_int("display_id")? })).await?;
            Ok(Output::Object(Value::Object(reply)))
        }
        "place_window" => {
            control
                .call(
                    "placeWindow",
                    json!({ "displayId": a.require_int("display_id")?, "windowId": a.require_int("window_id")? }),
                )
                .await?;
            Ok(Output::Object(json!({ "placed": true })))
        }
        "set_disposition" => {
            let disposition = a
                .one_of("disposition", &["keep", "close"])?
                .ok_or_else(|| ControlError::invalid("disposition is required"))?;
            let reply = control
                .call(
                    "setDisposition",
                    json!({
                        "displayId": a.require_int("display_id")?,
                        "windowId": a.require_int("window_id")?,
                        "disposition": disposition
                    }),
                )
                .await?;
            Ok(Output::Object(if reply.get("handedOver") == Some(&Value::Bool(true)) {
                json!({ "disposition": disposition, "note": "The user handed this window over; it returns to them regardless." })
            } else {
                json!({ "disposition": disposition })
            }))
        }
        "launch_app" => {
            let app = a.string("app")?.ok_or_else(|| ControlError::invalid("app is required"))?;
            let reply = control
                .call("launchApp", json!({ "displayId": a.require_int("display_id")?, "app": app }))
                .await?;
            Ok(Output::Object(match reply.get("window") {
                Some(window) if !window.is_null() => json!({ "window": window }),
                _ => json!({ "window": null, "note": "The app opened but no new window appeared." }),
            }))
        }
        "get_window_state" => {
            window_state(control, a.require_int("display_id")?, a.require_int("window_id")?).await.map(Output::Content)
        }
        "act_on_element" => {
            let display = a.require_int("display_id")?;
            let window = a.require_int("window_id")?;
            let action = a
                .one_of(
                    "action",
                    &["press", "click", "double_click", "right_click", "show_menu", "focus", "set_value", "scroll_into_view"],
                )?
                .ok_or_else(|| ControlError::invalid("action is required"))?;
            control
                .call(
                    "elementAction",
                    json!({
                        "displayId": display,
                        "windowId": window,
                        "element": a.require_int("element")?,
                        "action": action,
                        "value": a.string("value")?
                    }),
                )
                .await?;
            tokio::time::sleep(SETTLE).await;
            window_state(control, display, window).await.map(Output::Content)
        }
        "computer" => computer(&a, control).await.map(Output::Content),
        other => Err(ControlError::invalid(format!("unknown tool {other}"))),
    }
}

fn describe_wait(reply: &Object) -> Value {
    match reply.get("outcome").and_then(Value::as_str) {
        Some("returned") => json!({ "control": "agent", "note": "You have control. Take a screenshot before you continue." }),
        Some("timedOut") => json!({ "control": "user", "note": "The user is still operating the display. Wait again or do something else." }),
        _ => json!({ "control": "none", "note": "The display has ended." }),
    }
}

/// Window state as tool content: the tree as text, then the window image.
async fn window_state(control: &Control, display: i64, window: i64) -> Result<Vec<Value>, ControlError> {
    let state = control.call("windowState", json!({ "displayId": display, "windowId": window })).await?;
    let get = |key: &str| state.get(key).cloned().unwrap_or(Value::Null);
    let elements = get("elements");
    let truncated = if get("truncated") == Value::Bool(true) { " (truncated)" } else { "" };
    let header = format!(
        "Window {window}: {} {} elements{truncated}. Coordinates are window points from its top-left corner, the same space as the {}×{} image.",
        get("window"),
        elements,
        get("width"),
        get("height")
    );
    let tree = state.get("tree").and_then(Value::as_str).unwrap_or_default();
    Ok(vec![text(format!("{header}\n\n{tree}")), image(&get("data"), &get("mimeType"))])
}

async fn computer(a: &Args<'_>, control: &Control) -> Result<Vec<Value>, ControlError> {
    let display = a.require_int("display_id")?;
    let action = a
        .one_of(
            "action",
            &[
                "screenshot", "left_click", "right_click", "middle_click", "double_click", "triple_click", "mouse_move",
                "left_click_drag", "scroll", "type", "key", "wait",
            ],
        )?
        .ok_or_else(|| ControlError::invalid("action is required"))?;
    let coordinate = a.coordinate("coordinate")?;
    let text_arg = a.string("text")?;
    let modifiers: Vec<String> = text_arg
        .as_deref()
        .unwrap_or_default()
        .split('+')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();

    match action.as_str() {
        "screenshot" => {}
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
            let (x, y) = require_coordinate(coordinate, &action)?;
            let button = match action.as_str() {
                "right_click" => "right",
                "middle_click" => "middle",
                _ => "left",
            };
            let count = match action.as_str() {
                "double_click" => 2,
                "triple_click" => 3,
                _ => 1,
            };
            control
                .call(
                    "click",
                    json!({ "displayId": display, "x": x, "y": y, "button": button, "count": count, "modifiers": modifiers }),
                )
                .await?;
        }
        "mouse_move" => {
            let (x, y) = require_coordinate(coordinate, &action)?;
            control.call("move", json!({ "displayId": display, "x": x, "y": y })).await?;
        }
        "left_click_drag" => {
            let (from_x, from_y) = require_coordinate(a.coordinate("start_coordinate")?, "left_click_drag start")?;
            let (to_x, to_y) = require_coordinate(coordinate, &action)?;
            control
                .call(
                    "drag",
                    json!({ "displayId": display, "fromX": from_x, "fromY": from_y, "toX": to_x, "toY": to_y }),
                )
                .await?;
        }
        "scroll" => {
            let (x, y) = require_coordinate(coordinate, &action)?;
            let amount = a.int("scroll_amount")?.unwrap_or(3) as f64;
            let direction = a.one_of("scroll_direction", &["up", "down", "left", "right"])?;
            // Line-based wheel deltas: positive scrolls content up/left into view.
            let (dx, dy) = match direction.as_deref().unwrap_or("down") {
                "down" => (0.0, -amount),
                "up" => (0.0, amount),
                "right" => (-amount, 0.0),
                _ => (amount, 0.0),
            };
            control
                .call("scroll", json!({ "displayId": display, "x": x, "y": y, "deltaX": dx, "deltaY": dy }))
                .await?;
        }
        "type" => {
            control.call("type", json!({ "displayId": display, "text": require_text(text_arg, "type")? })).await?;
        }
        "key" => {
            control.call("key", json!({ "displayId": display, "keys": require_text(text_arg, "key")? })).await?;
        }
        _ => {
            // wait
            let seconds = a.number("duration")?.unwrap_or(1.0);
            tokio::time::sleep(Duration::from_secs_f64(seconds.clamp(0.0, 30.0))).await;
            return Ok(vec![text(format!("Waited {seconds} s."))]);
        }
    }

    // Let the UI settle so the screenshot shows the result of the action.
    if action != "screenshot" {
        tokio::time::sleep(SETTLE).await;
    }
    let shot = control.call("screenshot", json!({ "displayId": display })).await?;
    let get = |key: &str| shot.get(key).cloned().unwrap_or(Value::Null);
    Ok(vec![
        text(format!("{}×{} screenshot of display {display}.", get("width"), get("height"))),
        image(&get("data"), &get("mimeType")),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eleven_tools_with_object_schemas() {
        let tools = definitions();
        assert_eq!(tools.len(), 11);
        for tool in &tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        }
    }

    #[test]
    fn argument_errors_name_the_field() {
        let args = json!({ "display_id": "one", "coordinate": [1, "x"] });
        let a = Args(args.as_object().unwrap());
        assert_eq!(a.require_int("display_id").unwrap_err().message, "display_id must be an integer");
        assert!(a.coordinate("coordinate").is_err());
        assert_eq!(a.require_int("window_id").unwrap_err().message, "window_id is required");
    }
}
