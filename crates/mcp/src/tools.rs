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

fn max_depth() -> Value {
    json!({ "type": "integer", "description": "Leave out elements nested deeper than this many levels; 0 keeps only the window's direct children. Default: all." })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

struct Hints {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
}

const PLAIN: Hints = Hints {
    read_only: false,
    destructive: false,
    idempotent: false,
};
const READ_ONLY: Hints = Hints {
    read_only: true,
    destructive: false,
    idempotent: false,
};

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
                "cursor or keyboard. Call it only when you need a screen. Calling it again returns the same display, and so",
                "does a call from a subagent of this session: let one subagent do all the GUI work.",
                "The returned display_id stays valid until end_display or until this session ends.",
                "The result's width and height are the display in points; scale is screenshot pixels per point",
                "(1 on the default 1280×800, less when the screenshot is scaled down).",
                "list_windows bounds, get_window_state centers and computer clicks use screenshot pixels.",
                "Put windows on it with launch_app or place_window, then drive it with the computer tool.",
            ],
            object(
                json!({
                    "title": { "type": "string", "description": "Short task title shown to the user, e.g. \"Book flights\"." },
                    "width": { "type": "integer", "description": "Width in points. Default 1280. Screenshots of wider displays are scaled down to fit the model's image limit." },
                    "height": { "type": "integer", "description": "Height in points. Default 800." }
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
            Hints {
                destructive: true,
                ..PLAIN
            },
        ),
        tool(
            "record_display",
            "Record the display to video",
            &[
                "start records the display to an MP4 in ~/Movies/Agent Screen, with a timeline beside it: one JSON line per",
                "action you take on the display (clicks with coordinates in screenshot pixels, keys, typing, launches,",
                "element actions) and per Takeover, each with t, its seconds into the video. stop finishes both and",
                "returns their paths. The video has the sound of the apps whose every window is on your display; an app",
                "that also has windows elsewhere, such as the user's own browser, stays silent, since macOS separates",
                "sound by app, not by window. stop lists both under sound. A recording also stops when the display ends,",
                "and after 30 minutes. The user sees that the display is being recorded.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "action": { "type": "string", "enum": ["start", "stop"] }
                }),
                &["display_id", "action"],
            ),
            PLAIN,
        ),
        tool(
            "computer",
            "Operate the display",
            &[
                "See and operate your Agent Display. Works like Anthropic's computer-use tool, scoped to one display:",
                "screenshot pixels and coordinates are the same space. Every action except wait and cursor_position returns a fresh screenshot.",
                "type sends one character at a time, about 200 a second: for long text in a field, use act_on_element insert_text. Keys go to the window you last clicked; after a shortcut opens a window, click in it before typing.",
                "The desktop is plain gray. The strip at the top, where macOS draws the user's menu bar, is empty and takes no",
                "clicks; reach your app's menus through the menu bar list at the end of get_window_state.",
                "Apps on your display stay in the background, so menu shortcuts that act on the selection (copy, paste,",
                "undo, bold) do nothing there, by key or by menu item; cmd+a still selects all text. To copy and paste, use",
                "act_on_element's read_text and insert_text; set_value replaces a field's text. Shortcuts for app commands,",
                "such as cmd+n or cmd+w, work.",
                "key takes xdotool-style chords such as \"Return\", \"cmd+l\" or \"cmd+shift+t\"; hold_key holds one for",
                "duration seconds. Click, drag and scroll actions accept held modifiers in text, e.g. \"shift\". For a drag",
                "left_click_drag can't express, use left_mouse_down, mouse_move steps, then left_mouse_up; without a",
                "coordinate they act at the pointer. cursor_position returns the pointer without a screenshot. zoom",
                "returns a region at up to twice the detail, to read small text; aim with the full screenshot. Before you",
                "solve a captcha, ask the user to confirm that challenge. After they confirm, fill it. Call request_help if they",
                "prefer to do it or you cannot. Fails with a takeover error while the user is operating the display; then call",
                "wait_for_control.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "action": { "type": "string", "enum": [
                        "screenshot", "left_click", "right_click", "middle_click", "double_click", "triple_click",
                        "mouse_move", "left_click_drag", "left_mouse_down", "left_mouse_up", "scroll", "type", "key",
                        "hold_key", "cursor_position", "zoom", "wait"
                    ] },
                    "coordinate": coordinate(),
                    "start_coordinate": coordinate(),
                    "region": {
                        "type": "array",
                        "items": { "type": "number" },
                        "minItems": 4,
                        "maxItems": 4,
                        "description": "[x0, y0, x1, y1] in screenshot pixels, for zoom."
                    },
                    "text": { "type": "string" },
                    "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "scroll_amount": { "type": "integer", "description": "Scroll wheel clicks. Default 3." },
                    "duration": { "type": "number", "description": "Seconds: for wait, at most 30; for hold_key, at most 10." },
                    "via_input_method": {
                        "type": "boolean",
                        "description": "For type and key: send the keys through the user's input method (pinyin, Japanese, Korean), so it composes as when a person types; text is then the keys to press, such as \"nihao \" for 你好. Compose and commit in one call: the user's app comes back to the front after each call, and the input method drops what it was composing. This briefly brings the app to the front and uses the user's keyboard, so only use it to test or reproduce input-method behavior. It is refused while the user is typing or moving the pointer, stops if they start, and takes at most 250 keys a call."
                    },
                    "input_source": {
                        "type": "string",
                        "description": "With via_input_method: the input source to use for the call, such as com.apple.inputmethod.SCIM.ITABC; the user's own comes back after. Default: the current one."
                    }
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
            object(
                json!({ "display_id": display_id(), "timeout_seconds": timeout() }),
                &["display_id"],
            ),
            READ_ONLY,
        ),
        tool(
            "request_help",
            "Ask the user to step in",
            &[
                "Ask the user to take over your display for something you cannot do, such as a login or a",
                "two-factor code. For a captcha, ask them to confirm the current challenge immediately before you",
                "solve it. After they confirm, solve it when you can. Call this if they prefer to do it or you cannot.",
                "The user gets a system notification with your reason. The call then waits like",
                "wait_for_control until the user has stepped in and handed the display back, or until the timeout.",
                "On a timeout the request stays open; call wait_for_control to keep waiting. If you got past it yourself,",
                "call request_help with withdraw: true to take the request back.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "reason": { "type": "string", "description": "What you need the user to do, in one short sentence." },
                    "steps": { "type": "array", "items": { "type": "string" }, "maxItems": 5, "description": "Up to 5 short steps, when the reason alone can't say how. The Viewer lists them while the user is in control." },
                    "withdraw": { "type": "boolean", "description": "Take back the open request instead; reason is then not needed." },
                    "timeout_seconds": timeout()
                }),
                &["display_id"],
            ),
            PLAIN,
        ),
        tool(
            "request_control",
            "Ask the user to hand the display back",
            &[
                "While the user is operating your display (a Takeover) and you need it to go on, ask them to hand it",
                "back, with a short reason. The user gets a notification and a card with Let <agent> Continue and Keep",
                "Control. The call then waits like wait_for_control. Returns control: agent at once if you have control",
                "already. control: user with kept: true means the user chose to keep control: don't ask again right",
                "away. On a timeout the request stays open; call wait_for_control to keep waiting, or request_control",
                "with withdraw: true when you no longer need the display. Only the user ends a Takeover.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "reason": { "type": "string", "description": "Why you need the display back, in one short sentence." },
                    "withdraw": { "type": "boolean", "description": "Take back the open request instead; reason is then not needed." },
                    "timeout_seconds": timeout()
                }),
                &["display_id"],
            ),
            PLAIN,
        ),
        tool(
            "list_windows",
            "List windows on the display",
            &[
                "List app windows on the display, front to back, with ids and bounds in screenshot pixels. scale is",
                "screenshot pixels per display point, the same number create_display returns. origin says how",
                "a window got here (launched, placed, handedOver by the user, found, unknown); disposition is your current",
                "set_disposition verdict or null. With strays: true it lists instead the windows of apps launch_app started",
                "for you that opened off your display, and windows that were on your display and left it, for place_window. It never lists the user's windows; to ask for one, call place_window with the app's name.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "strays": { "type": "boolean", "description": "List your apps' windows that opened off the display, and your windows that left it." }
                }),
                &["display_id"],
            ),
            READ_ONLY,
        ),
        tool(
            "place_window",
            "Place a window on the display",
            &[
                "Move a window onto the display, by window_id or by app name. A window of an app launch_app started for you",
                "moves at once: get ids from list_windows with strays: true. Any other window is the user's, such as one an app",
                "opened that you started with a shell command like open -a: Agent Screen asks the user, and the call waits for",
                "their answer, up to timeout_seconds. Handed over, the window is on the display as a Handover; turned down, it stays",
                "theirs. Start GUI apps with launch_app rather than open, so their windows come to the display without asking.",
                "A new window of an app you started moves to the display by itself within 3 s of your input, element action or",
                "launch there, unless the user meanwhile clicked or typed in the app, Finder, the Dock or a launcher such as Spotlight.",
                "One the app opens later by itself moves there too while none of its other windows is on the user's screen.",
                "x, y, width and height set the window's frame, in screenshot pixels from the display's top-left corner,",
                "as list_windows reports bounds; each one left out keeps the window's own. Pass a window_id already on the",
                "display to resize or move it there. The frame stays inside the display, and apps hold to their own minimum",
                "and maximum sizes, so the result's window is the frame the window actually ended with. For a width wider",
                "than the display, create_display with a larger width first.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": { "type": "integer", "description": "The window to move: a stray, a window of the user's, or one already on the display." },
                    "app": { "type": "string", "description": "Instead of window_id: the app whose front window to move, by name, e.g. \"UTM\"." },
                    "x": { "type": "number", "description": "Left edge in screenshot pixels from the display's left edge." },
                    "y": { "type": "number", "description": "Top edge in screenshot pixels from the display's top edge." },
                    "width": { "type": "number", "description": "Width in screenshot pixels." },
                    "height": { "type": "number", "description": "Height in screenshot pixels." },
                    "timeout_seconds": { "type": "number", "description": "How long to wait for the user to answer a request for one of their windows. Default 120, up to 600." }
                }),
                &["display_id"],
            ),
            PLAIN,
        ),
        tool(
            "launch_app",
            "Launch an app on the display",
            &[
                "Open an app in the background and put its new window on the display. If the app is already running, Agent Screen asks it for a new window and moves only that one, never a window of the user's; a file may then open in one of the app's existing windows, which stays where it is.",
                "With open, the app opens that URL or file in the window. With arguments or environment, a new instance of",
                "the app starts with them, as a development build needs, and its windows go on the display.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "app": { "type": "string", "description": "App name (\"Safari\"), bundle id (\"com.apple.Safari\") or the absolute path of an .app bundle (\"/tmp/build/MyApp.app\")." },
                    "open": { "type": "string", "description": "A URL (https://…, file://…) or a file path starting with / or ~/ to open in the app. Relative paths are refused." },
                    "arguments": { "type": "array", "items": { "type": "string" }, "description": "Command-line arguments for a new instance of the app, passed as given." },
                    "environment": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for a new instance of the app." }
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
            Hints {
                idempotent: true,
                ..PLAIN
            },
        ),
        tool(
            "get_window_state",
            "See one window",
            &[
                "Screenshot of one window on your display plus its accessibility tree. Every element has a number in",
                "brackets that act_on_element takes, and its center in screenshot pixels. An element keeps its number",
                "while it stays in the window; a number whose element went away fails instead of hitting another one.",
                "A large window's tree runs to thousands of tokens: max_depth leaves out deeper elements and says how many,",
                "and their numbers still work.",
            ],
            object(
                json!({ "display_id": display_id(), "window_id": window_id(), "max_depth": max_depth() }),
                &["display_id", "window_id"],
            ),
            READ_ONLY,
        ),
        tool(
            "act_on_element",
            "Act on a window element",
            &[
                "Act on an element from get_window_state, then return the window's new state. press, show_menu, focus,",
                "set_value and scroll_into_view use accessibility actions, which reach the app without focusing it; click,",
                "double_click and right_click send a real click at the element's center. press falls back to a click when",
                "the element has no press action. Pressing a menubaritem lists that menu's items instead of opening it.",
                "read_text returns a text element's whole value and its selected text, which the tree cuts at 80 characters:",
                "use it to copy. insert_text replaces the selection, or inserts at the caret, with value: use it to paste.",
                "Both work in background apps, where Command-C and Command-V do nothing.",
                "result picks what comes back: state, the default, is the whole window state; changes lists only the",
                "elements that appeared, changed (~) or went away (-) since the window's last tree, with the window image;",
                "none says only that it is done. Fails with a takeover error while the user operates the display.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": window_id(),
                    "element": { "type": "integer", "description": "The bracketed number from get_window_state." },
                    "action": { "type": "string", "enum": [
                        "press", "click", "double_click", "right_click", "show_menu", "focus", "set_value", "insert_text",
                        "read_text", "scroll_into_view"
                    ] },
                    "value": { "type": "string", "description": "Text for set_value and insert_text." },
                    "result": { "type": "string", "enum": ["state", "changes", "none"], "description": "What to return after the action. Default state." },
                    "max_depth": max_depth()
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
                .or_else(|| {
                    value
                        .as_f64()
                        .filter(|f| f.fract() == 0.0)
                        .map(|f| f as i64)
                })
                .map(Some)
                .ok_or_else(|| ControlError::invalid(format!("{key} must be an integer"))),
        }
    }

    fn require_int(&self, key: &str) -> Result<i64, ControlError> {
        self.int(key)?
            .ok_or_else(|| ControlError::invalid(format!("{key} is required")))
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
            return Err(ControlError::invalid(format!(
                "{key} must be one of {}",
                allowed.join(", ")
            )));
        }
        Ok(value)
    }

    fn coordinate(&self, key: &str) -> Result<Option<(f64, f64)>, ControlError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) if items.len() == 2 => {
                match (
                    items[0].as_f64().filter(|f| f.is_finite()),
                    items[1].as_f64().filter(|f| f.is_finite()),
                ) {
                    (Some(x), Some(y)) => Ok(Some((x, y))),
                    _ => Err(ControlError::invalid(format!(
                        "{key} must be [x, y] numbers"
                    ))),
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

/// create_display's note after the app quit and took the session's
/// display with it.
const RESTARTED: &str = "Agent Screen quit since your last call and restarted; your earlier display ended with it, and macOS moved its windows to the user's screen. Windows of apps you launched there are strays now: list_windows with strays: true, then place_window.";

/// What a call naming a display that ended when the app quit gets back.
pub fn ended_with_app(id: i64) -> ControlError {
    ControlError::new(
        "ended",
        format!(
            "display {id} ended when the Agent Screen app quit; it has restarted. macOS moved the display's windows to the user's screen. Call create_display, then list_windows with strays: true and place_window to bring back windows of apps you launched there."
        ),
    )
}

// MARK: Handlers

pub async fn run(
    name: &str,
    args: &Object,
    session: &Session,
    control: &Control,
) -> Result<Output, ControlError> {
    let a = Args(args);
    match name {
        "create_display" => {
            let title = a.string("title")?.unwrap_or_else(|| session.folder.clone());
            // A display ended by the app quitting hands its apps on, so
            // their windows, now on the user's screen, are strays here.
            let orphans = control.orphans();
            let reply = control
                .call(
                    "createDisplay",
                    json!({
                        "sessionKey": session.key,
                        "client": session.client,
                        "title": title,
                        "width": a.int("width")?,
                        "height": a.int("height")?,
                        "adopt": orphans
                    }),
                )
                .await?;
            if let Some(id) = reply.get("displayId").and_then(Value::as_u64) {
                control.made_display(id);
            }
            let mut out = json!({
                "display_id": reply.get("displayId"),
                "width": reply.get("width"),
                "height": reply.get("height"),
                "scale": reply.get("scale"),
                "reused": reply.get("reused")
            });
            if let Some(note) = reply.get("note") {
                out["note"] = note.clone();
            } else if orphans.is_some() {
                out["note"] = json!(RESTARTED);
            }
            Ok(Output::Object(out))
        }
        "record_display" => {
            let action = a
                .one_of("action", &["start", "stop"])?
                .ok_or_else(|| ControlError::invalid("action is required"))?;
            let method = if action == "start" {
                "recordStart"
            } else {
                "recordStop"
            };
            let reply = control
                .call(method, json!({ "displayId": a.require_int("display_id")? }))
                .await?;
            Ok(Output::Object(Value::Object(reply)))
        }
        "end_display" => {
            control
                .call(
                    "endDisplay",
                    json!({ "displayId": a.require_int("display_id")? }),
                )
                .await?;
            Ok(Output::Object(json!({ "ended": true })))
        }
        "wait_for_control" => {
            let reply = control
                .call(
                    "waitForControl",
                    json!({ "displayId": a.require_int("display_id")?, "timeoutSeconds": a.int("timeout_seconds")? }),
                )
                .await?;
            let help_open = reply.get("helpOpen") == Some(&Value::Bool(true));
            Ok(Output::Object(describe_wait(&reply, help_open)))
        }
        "request_help" => {
            let display = a.require_int("display_id")?;
            if a.0.get("withdraw").and_then(Value::as_bool) == Some(true) {
                let reply = control
                    .call("withdrawHelp", json!({ "displayId": display }))
                    .await?;
                let withdrawn = reply.get("withdrawn") == Some(&Value::Bool(true));
                return Ok(Output::Object(json!({ "withdrawn": withdrawn })));
            }
            let reason = a
                .string("reason")?
                .ok_or_else(|| ControlError::invalid("reason is required"))?;
            control
                .call(
                    "requestHelp",
                    json!({ "displayId": display, "reason": reason, "steps": a.0.get("steps") }),
                )
                .await?;
            let reply = control
                .call(
                    "waitForControl",
                    json!({ "displayId": display, "timeoutSeconds": a.int("timeout_seconds")? }),
                )
                .await?;
            Ok(Output::Object(describe_wait(&reply, true)))
        }
        "request_control" => {
            let display = a.require_int("display_id")?;
            if a.0.get("withdraw").and_then(Value::as_bool) == Some(true) {
                let reply = control
                    .call("withdrawControlRequest", json!({ "displayId": display }))
                    .await?;
                let withdrawn = reply.get("withdrawn") == Some(&Value::Bool(true));
                return Ok(Output::Object(json!({ "withdrawn": withdrawn })));
            }
            let reason = a
                .string("reason")?
                .ok_or_else(|| ControlError::invalid("reason is required"))?;
            control
                .call(
                    "requestControl",
                    json!({ "displayId": display, "reason": reason }),
                )
                .await?;
            let reply = control
                .call(
                    "waitForControl",
                    json!({ "displayId": display, "timeoutSeconds": a.int("timeout_seconds")? }),
                )
                .await?;
            Ok(Output::Object(describe_wait(&reply, false)))
        }
        "list_windows" => {
            let reply = control
                .call(
                    "listWindows",
                    json!({ "displayId": a.require_int("display_id")?, "strays": a.0.get("strays").and_then(Value::as_bool).unwrap_or(false) }),
                )
                .await?;
            Ok(Output::Object(Value::Object(reply)))
        }
        "place_window" => {
            let asked = [
                ("x", a.number("x")?),
                ("y", a.number("y")?),
                ("width", a.number("width")?),
                ("height", a.number("height")?),
            ];
            for (key, value) in &asked[2..] {
                if value.is_some_and(|v| v <= 0.0) {
                    return Err(ControlError::invalid(format!("{key} must be above 0")));
                }
            }
            let mut params = json!({
                "displayId": a.require_int("display_id")?,
                "windowId": a.int("window_id")?,
                "app": a.string("app")?,
                "timeoutSeconds": a.number("timeout_seconds")?,
            });
            for (key, value) in asked {
                params[key] = json!(value);
            }
            let reply = control.call("placeWindow", params).await?;
            Ok(Output::Object(place_output(&reply, &asked)))
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
            Ok(Output::Object(
                if reply.get("handedOver") == Some(&Value::Bool(true)) {
                    json!({ "disposition": disposition, "note": "The user handed this window over; it returns to them regardless." })
                } else {
                    json!({ "disposition": disposition })
                },
            ))
        }
        "launch_app" => {
            let app = a
                .string("app")?
                .ok_or_else(|| ControlError::invalid("app is required"))?;
            let display = a.require_int("display_id")?;
            let reply = control
                .call(
                    "launchApp",
                    json!({
                        "displayId": display,
                        "app": app,
                        "open": a.string("open")?,
                        "arguments": a.0.get("arguments"),
                        "environment": a.0.get("environment"),
                    }),
                )
                .await?;
            if let Some(pid) = reply.get("pid").and_then(Value::as_i64) {
                control.started_app(display as u64, pid);
            }
            Ok(Output::Object(launch_output(&Value::Object(reply))))
        }
        "get_window_state" => {
            let depth = max_depth_arg(&a)?;
            let state = window_state(
                control,
                a.require_int("display_id")?,
                a.require_int("window_id")?,
            )
            .await?;
            Ok(Output::Content(state.content(depth)))
        }
        "act_on_element" => {
            let display = a.require_int("display_id")?;
            let window = a.require_int("window_id")?;
            let result = a
                .one_of("result", &["state", "changes", "none"])?
                .unwrap_or_else(|| "state".into());
            let depth = max_depth_arg(&a)?;
            let action = a
                .one_of(
                    "action",
                    &[
                        "press",
                        "click",
                        "double_click",
                        "right_click",
                        "show_menu",
                        "focus",
                        "set_value",
                        "insert_text",
                        "read_text",
                        "scroll_into_view",
                    ],
                )?
                .ok_or_else(|| ControlError::invalid("action is required"))?;
            let reply = control
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
            if action == "read_text" {
                let field = |key: &str| reply.get(key).and_then(Value::as_str).map(String::from);
                let mut out = String::new();
                if let Some(selected) = field("selected") {
                    out.push_str(&format!("Selected:\n{selected}\n\n"));
                }
                if let Some(value) = field("text") {
                    out.push_str(&format!("Value:\n{value}"));
                }
                return Ok(Output::Content(vec![text(out.trim_end().to_string())]));
            }
            if let Some(menu) = reply.get("menu").and_then(Value::as_str) {
                return Ok(Output::Content(vec![text(format!(
                    "The menu's items, still closed; press one to choose it. Numbers continue the window's tree.\n\n{menu}"
                ))]));
            }
            // The window state shows the action's result once the display
            // stops changing.
            control
                .call("settle", json!({ "displayId": display }))
                .await?;
            match window_state(control, display, window).await {
                // OK, Save or Close took its own window away: that is the
                // action working, so show what is on the display now.
                Err(error) if error.code == "not_found" => {
                    let list = control
                        .call("listWindows", json!({ "displayId": display }))
                        .await?;
                    let shot = control
                        .call("screenshot", json!({ "displayId": display }))
                        .await?;
                    let get = |key: &str| shot.get(key).cloned().unwrap_or(Value::Null);
                    Ok(Output::Content(vec![
                        text(format!(
                            "Done; the action closed window {window}. Windows on the display now: {}{}",
                            list.get("windows").cloned().unwrap_or(Value::Null),
                            strayed_note(&shot)
                        )),
                        image(&get("data"), &get("mimeType")),
                    ]))
                }
                Err(error) => Err(error),
                Ok(state) => Ok(Output::Content(match result.as_str() {
                    "none" => vec![text(format!("Done.{}", state.strayed))],
                    "changes" => state.changes(depth),
                    _ => state.content(depth),
                })),
            }
        }
        "computer" => computer(&a, control).await.map(Output::Content),
        other => Err(ControlError::invalid(format!("unknown tool {other}"))),
    }
}

/// place_window's result: the frame the window ended with, and a note
/// when it isn't the one asked for.
fn place_output(reply: &Object, asked: &[(&str, Option<f64>); 4]) -> Value {
    let mut out = json!({ "placed": true });
    if reply.get("handedOver") == Some(&Value::Bool(true)) {
        out["handedOver"] = json!(true);
    }
    let Some(window) = reply.get("window") else {
        return out;
    };
    out["window"] = window.clone();
    let missed: Vec<String> = asked
        .iter()
        .filter_map(|(key, value)| {
            let wanted = (*value)?;
            let got = window.get(*key)?.as_f64()?;
            ((got - wanted).abs() > 1.0).then(|| format!("{key} {got} for {wanted}"))
        })
        .collect();
    if !missed.is_empty() {
        out["note"] = json!(format!(
            "The window didn't take the whole frame you asked for: {}. Apps keep to their own minimum and maximum sizes, the frame stays inside the display, and macOS keeps windows below the menu bar strip.",
            missed.join(", ")
        ));
    }
    out
}

/// What a wait for control came to. `helping`: a Help Request is open, and
/// the user may not have seen it yet.
fn describe_wait(reply: &Object, helping: bool) -> Value {
    match reply.get("outcome").and_then(Value::as_str) {
        Some("returned") => {
            json!({ "control": "agent", "note": "You have control. Take a screenshot before you continue." })
        }
        Some("kept") => {
            json!({ "control": "user", "kept": true, "note": "The user chose to keep control of the display. Don't ask again right away; call wait_for_control to wait until they hand it back." })
        }
        Some("timedOut") if reply.get("controlAsked") == Some(&Value::Bool(true)) => {
            json!({ "control": "user", "note": "The user hasn't answered your request to hand the display back; it stays open. Call wait_for_control to keep waiting, or request_control with withdraw: true if you no longer need the display." })
        }
        Some("timedOut") if helping => {
            json!({ "control": "user", "note": "The user hasn't handed the display back yet; the request stays open. Call wait_for_control to keep waiting, or request_help with withdraw: true if you no longer need them." })
        }
        Some("timedOut") => {
            json!({ "control": "user", "note": "The user is still operating the display. Wait again or do something else." })
        }
        Some("closedByUser") => {
            json!({ "control": "none", "note": "The user closed the display, so stop working on screen. Ask the user before you call create_display again." })
        }
        _ => json!({ "control": "none", "note": "The display has ended." }),
    }
}

/// What a result's `strayed` windows are: new windows of the agent's apps
/// that stayed on the user's screen, because the user was using the app
/// too or nobody had just acted.
fn strayed_note(result: &serde_json::Map<String, Value>) -> String {
    let mut note = String::new();
    for w in result
        .get("strayed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let app = w.get("app").and_then(Value::as_str).unwrap_or("app");
        let id = w.get("windowId").cloned().unwrap_or(Value::Null);
        let next = if w.get("placeable") == Some(&Value::Bool(true)) {
            "If your action opened it, call place_window with that id."
        } else {
            "If you need it, call place_window with that id: Agent Screen asks the user for it."
        };
        note.push_str(&format!(
            " A new {app} window, {id}, opened on the user's screen and stayed there: the user was using the app too, or it opened long after your last action. {next}"
        ));
    }
    note
}

fn max_depth_arg(a: &Args<'_>) -> Result<Option<usize>, ControlError> {
    match a.int("max_depth")? {
        Some(d) if d < 0 => Err(ControlError::invalid("max_depth must be 0 or more")),
        d => Ok(d.map(|d| d as usize)),
    }
}

/// One window's state from the app, and the tree it had before.
struct WindowState {
    window: i64,
    /// The window and its element count.
    header: String,
    strayed: String,
    tree: String,
    before: Option<String>,
    image: Value,
}

impl WindowState {
    /// The tree as text, cut at `depth`, then the window image.
    fn content(&self, depth: Option<usize>) -> Vec<Value> {
        let full = self.tree.as_str();
        let cut = depth.map(|d| agent_screen_model::tree::cut_depth(full, d));
        let note = match (&cut, depth) {
            (Some(cut), Some(d)) if cut != full => format!(
                " Elements deeper than {d} levels are left out; call get_window_state with a larger max_depth, or none, to see them."
            ),
            _ => String::new(),
        };
        let tree = cut.as_deref().unwrap_or(full);
        vec![
            text(format!(
                "{} \"at (x,y)\" is an element's center in screenshot pixels, where the computer tool clicks; the image shows just this window.{note}{}\n\n{tree}",
                self.header, self.strayed
            )),
            self.image.clone(),
        ]
    }

    /// What changed since the window's last tree, then the window image.
    /// With no tree before, the whole state.
    fn changes(&self, depth: Option<usize>) -> Vec<Value> {
        let Some(before) = &self.before else {
            return self.content(depth);
        };
        let window = self.window;
        let body = match agent_screen_model::tree::changes(before, &self.tree) {
            Some(changes) => format!(
                "Done. Changes in window {window} since its last tree (+ new, ~ changed, - gone):{}\n\n{changes}",
                self.strayed
            ),
            None => format!(
                "Done. Window {window}'s elements didn't change.{}",
                self.strayed
            ),
        };
        vec![text(body), self.image.clone()]
    }
}

async fn window_state(
    control: &Control,
    display: i64,
    window: i64,
) -> Result<WindowState, ControlError> {
    let state = control
        .call(
            "windowState",
            json!({ "displayId": display, "windowId": window }),
        )
        .await?;
    let get = |key: &str| state.get(key).cloned().unwrap_or(Value::Null);
    let truncated = if get("truncated") == Value::Bool(true) {
        " (truncated)"
    } else {
        ""
    };
    let tree = state
        .get("tree")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(WindowState {
        window,
        header: format!(
            "Window {window}: {} {} elements{truncated}.",
            get("window"),
            get("elements")
        ),
        strayed: strayed_note(&state),
        before: control.swap_tree(display, window, tree.clone()),
        tree,
        image: image(&get("data"), &get("mimeType")),
    })
}

async fn computer(a: &Args<'_>, control: &Control) -> Result<Vec<Value>, ControlError> {
    let display = a.require_int("display_id")?;
    let action = a
        .one_of(
            "action",
            &[
                "screenshot",
                "left_click",
                "right_click",
                "middle_click",
                "double_click",
                "triple_click",
                "mouse_move",
                "left_click_drag",
                "left_mouse_down",
                "left_mouse_up",
                "scroll",
                "type",
                "key",
                "hold_key",
                "cursor_position",
                "zoom",
                "wait",
            ],
        )?
        .ok_or_else(|| ControlError::invalid("action is required"))?;
    let coordinate = a.coordinate("coordinate")?;
    let text_arg = a.string("text")?;
    let via_input_method = a.0.get("via_input_method") == Some(&Value::Bool(true));
    let input_source = a.string("input_source")?;
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
            control
                .call("move", json!({ "displayId": display, "x": x, "y": y }))
                .await?;
        }
        "left_click_drag" => {
            let (from_x, from_y) =
                require_coordinate(a.coordinate("start_coordinate")?, "left_click_drag start")?;
            let (to_x, to_y) = require_coordinate(coordinate, &action)?;
            control
                .call(
                    "drag",
                    json!({ "displayId": display, "fromX": from_x, "fromY": from_y, "toX": to_x, "toY": to_y, "modifiers": modifiers }),
                )
                .await?;
        }
        "left_mouse_down" | "left_mouse_up" => {
            let method = if action == "left_mouse_down" {
                "mouseDown"
            } else {
                "mouseUp"
            };
            let mut params = json!({ "displayId": display, "modifiers": modifiers });
            if let Some((x, y)) = coordinate {
                params["x"] = json!(x);
                params["y"] = json!(y);
            }
            control.call(method, params).await?;
        }
        "hold_key" => {
            control
                .call(
                    "holdKey",
                    json!({
                        "displayId": display,
                        "keys": require_text(text_arg, "hold_key")?,
                        "seconds": a.number("duration")?.unwrap_or(1.0)
                    }),
                )
                .await?;
        }
        "zoom" => {
            let region =
                a.0.get("region")
                    .cloned()
                    .ok_or_else(|| ControlError::invalid("region is required for zoom"))?;
            let shot = control
                .call(
                    "screenshot",
                    json!({ "displayId": display, "region": region }),
                )
                .await?;
            let get = |key: &str| shot.get(key).cloned().unwrap_or(Value::Null);
            return Ok(vec![
                text(format!(
                    "{}×{} zoom of region {region} on display {display}. Its pixels are not screen coordinates.{}",
                    get("width"),
                    get("height"),
                    strayed_note(&shot)
                )),
                image(&get("data"), &get("mimeType")),
            ]);
        }
        "cursor_position" => {
            let at = control
                .call("cursorPosition", json!({ "displayId": display }))
                .await?;
            return Ok(vec![text(match (at.get("x"), at.get("y")) {
                (Some(x), Some(y)) => format!("The pointer is at ({x}, {y}) in screenshot pixels."),
                _ => "No pointer yet: nothing has clicked or moved on this display.".into(),
            })]);
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
                .call(
                    "scroll",
                    json!({ "displayId": display, "x": x, "y": y, "deltaX": dx, "deltaY": dy, "modifiers": modifiers }),
                )
                .await?;
        }
        "type" => {
            control
                .call(
                    "type",
                    json!({
                        "displayId": display,
                        "text": require_text(text_arg, "type")?,
                        "viaInputMethod": via_input_method,
                        "inputSource": input_source,
                    }),
                )
                .await?;
        }
        "key" => {
            control
                .call(
                    "key",
                    json!({
                        "displayId": display,
                        "keys": require_text(text_arg, "key")?,
                        "viaInputMethod": via_input_method,
                        "inputSource": input_source,
                    }),
                )
                .await?;
        }
        _ => {
            // wait
            let seconds = a.number("duration")?.unwrap_or(1.0).clamp(0.0, 30.0);
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
            return Ok(vec![text(format!("Waited {seconds} s."))]);
        }
    }

    // After an action the app waits until the display stops changing, so
    // the screenshot shows the result: no fixed delay, up to 1.5 s.
    let settle = action != "screenshot";
    let shot = control
        .call(
            "screenshot",
            json!({ "displayId": display, "settle": settle }),
        )
        .await?;
    let get = |key: &str| shot.get(key).cloned().unwrap_or(Value::Null);
    let mut header = format!(
        "{}×{} screenshot of display {display}.",
        get("width"),
        get("height")
    );
    header.push_str(&strayed_note(&shot));
    Ok(vec![text(header), image(&get("data"), &get("mimeType"))])
}

/// What launch_app returns: the window, or why there is none and what to
/// do next. `wasRunning` says whether the app was already running.
fn launch_output(reply: &Value) -> Value {
    let was_running = reply
        .get("wasRunning")
        .cloned()
        .unwrap_or(Value::Bool(false));
    match reply.get("window") {
        Some(window) if !window.is_null() => match reply.get("reason").and_then(|r| r.as_str()) {
            Some("already_running") => json!({
                "window": window,
                "wasRunning": true,
                "reason": "already_running",
                "note": "The app was already running on your display and allows only one instance: the new one quit and passed its arguments and environment to the running one. This is the running instance's window; nothing new started.",
            }),
            _ => json!({ "window": window, "wasRunning": was_running }),
        },
        _ => {
            let reason = reply
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("still_starting");
            let note = match reason {
                "existing_window" => {
                    "The app was already running and opened the file in a window it already had, which may be the user's; nothing moved. If you need that window, call place_window with its id: Agent Screen asks the user for it."
                }
                "just_started" => {
                    "The app started moments ago, by the user or another agent, and isn't ready to open a window yet; nothing moved. Wait a few seconds and call launch_app again. Don't ask the user to quit it: its first window may be someone's work."
                }
                "no_new_window" => {
                    "The app was already running and its menus have no command that opens a plain new window, as in Calculator or Notes; nothing was created or moved. Ask the user to quit it and call launch_app again, or to hand its window over."
                }
                "menus_unreadable" => {
                    "The app was already running and Agent Screen couldn't read or use its menus, so it didn't ask for a window; nothing moved. Ask the user to quit it and call launch_app again, or to hand its window over."
                }
                "app_busy" => {
                    "The app was already running and its New Window command is greyed out, as while it shows a dialog; nothing moved. Ask the user to finish what the app is showing, then call launch_app again."
                }
                "still_opening" => {
                    "The app was already running and hadn't shown the file yet. When a window showing the file opens in the next 30 s, it moves onto your display; call list_windows to check. A window the user opens meanwhile stays theirs. If none comes, ask the user to hand the window over."
                }
                _ => {
                    "The app is still starting. Its first window in the next 30 s moves onto the display, unless the user opens it themselves or takes the app back; call list_windows to check, and list_windows with strays: true for a window to place_window."
                }
            };
            json!({ "window": null, "wasRunning": was_running, "reason": reason, "note": note })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(before: Option<&str>, tree: &str) -> WindowState {
        WindowState {
            window: 7,
            header: "Window 7: {} 3 elements.".into(),
            strayed: String::new(),
            tree: tree.into(),
            before: before.map(String::from),
            image: json!({ "type": "image" }),
        }
    }

    fn first_text(content: &[Value]) -> &str {
        content[0]["text"].as_str().unwrap()
    }

    #[test]
    fn window_state_cuts_at_max_depth_and_says_so() {
        let tree = "[0] group \"A\"\n  [1] button \"B\"\n[2] button \"C\"";
        let full = state(None, tree).content(None);
        assert!(first_text(&full).ends_with(tree));
        assert!(!first_text(&full).contains("left out"));
        let cut = state(None, tree).content(Some(0));
        let text = first_text(&cut);
        assert!(text.contains("  … 1 more inside\n[2]"), "{text}");
        assert!(text.contains("larger max_depth"), "{text}");
        assert_eq!(cut[1]["type"], "image");
        // Deep enough for the whole tree: no note.
        assert!(!first_text(&state(None, tree).content(Some(3))).contains("left out"));
    }

    #[test]
    fn act_on_element_changes_list_only_what_moved() {
        let before = "[0] button \"Send\"\n[1] textfield value=\"hi\"";
        let after = "[0] button \"Send\" disabled\n[1] textfield value=\"hi\"";
        let changed = state(Some(before), after).changes(None);
        let text = first_text(&changed);
        assert!(text.ends_with("~ [0] button \"Send\" disabled"), "{text}");
        assert!(!text.contains("textfield"), "{text}");
        assert!(first_text(&state(Some(after), after).changes(None)).contains("didn't change"));
        // No tree before: the whole state.
        assert!(first_text(&state(None, after).changes(None)).ends_with(after));
    }

    #[test]
    fn place_output_reports_the_frame_and_what_the_app_refused() {
        let reply = |w: Value| json!({ "window": w }).as_object().unwrap().clone();
        let asked = [
            ("x", None),
            ("y", None),
            ("width", Some(600.0)),
            ("height", Some(400.0)),
        ];
        let took = place_output(
            &reply(json!({ "x": 10, "y": 30, "width": 600, "height": 400 })),
            &asked,
        );
        assert_eq!(took["window"]["width"], 600);
        assert!(took.get("note").is_none());
        // An app with a 700-wide minimum.
        let clamped = place_output(
            &reply(json!({ "x": 10, "y": 30, "width": 700, "height": 400 })),
            &asked,
        );
        let note = clamped["note"].as_str().unwrap();
        assert!(note.contains("width 700 for 600"), "{note}");
        assert!(!note.contains("height"), "{note}");
        // A plain place has no frame to compare.
        assert_eq!(
            place_output(
                &Object::new(),
                &[("x", None), ("y", None), ("width", None), ("height", None)]
            ),
            json!({ "placed": true })
        );
    }

    #[test]
    fn launch_output_says_why_and_whether_it_ran() {
        let placed = launch_output(&json!({ "window": { "windowId": 7 }, "wasRunning": true }));
        assert_eq!(placed["wasRunning"], true);
        assert_eq!(placed["window"]["windowId"], 7);
        assert!(placed.get("note").is_none());
        let single = launch_output(&json!({
            "window": { "windowId": 8 },
            "wasRunning": true,
            "reason": "already_running",
        }));
        assert_eq!(single["window"]["windowId"], 8);
        assert_eq!(single["reason"], "already_running");
        assert!(single["note"].as_str().unwrap().contains("one instance"));
        for reason in [
            "still_starting",
            "existing_window",
            "no_new_window",
            "app_busy",
            "menus_unreadable",
            "still_opening",
            "just_started",
        ] {
            let none = launch_output(
                &json!({ "wasRunning": reason != "still_starting", "reason": reason }),
            );
            assert!(none["window"].is_null());
            assert_eq!(none["reason"], reason);
            assert!(none["wasRunning"].is_boolean());
            assert!(none["note"].as_str().unwrap().len() > 40, "{reason}");
        }
    }

    #[test]
    fn thirteen_tools_with_object_schemas() {
        let tools = definitions();
        assert_eq!(tools.len(), 13);
        for tool in &tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        }
    }

    #[test]
    fn coordinate_copy_uses_screenshot_pixels() {
        let tools = definitions();
        let description = |name: &str| {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("{name}"))["description"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let create = description("create_display");
        assert!(create.contains("screenshot pixels"), "{create}");
        assert!(create.contains("scale"), "{create}");
        let list = description("list_windows");
        assert!(list.contains("screenshot pixels"), "{list}");
        assert!(list.contains("scale"), "{list}");
        let state = description("get_window_state");
        assert!(state.contains("screenshot pixels"), "{state}");
        assert!(!state.contains("display coordinates"), "{state}");
    }

    #[test]
    fn argument_errors_name_the_field() {
        let args = json!({ "display_id": "one", "coordinate": [1, "x"] });
        let a = Args(args.as_object().unwrap());
        assert_eq!(
            a.require_int("display_id").unwrap_err().message,
            "display_id must be an integer"
        );
        assert!(a.coordinate("coordinate").is_err());
        assert_eq!(
            a.require_int("window_id").unwrap_err().message,
            "window_id is required"
        );
    }
}
