//! The tools Agent Sessions see. Each one turns into control socket calls.
//!
//! Schemas are written out as JSON rather than derived, so what the model
//! reads is exactly what is here.

use crate::control::{Control, ControlError, Object};
use crate::session::Session;
use agent_screen_model::tree::Scope;
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
    json!({ "type": "number", "minimum": 1, "maximum": 600, "description": "Seconds to wait before giving up, 1 to 600. Default 100." })
}

fn window_id() -> Value {
    json!({ "type": "integer", "description": "A window on your display, from list_windows or launch_app." })
}

fn max_depth() -> Value {
    json!({ "type": "integer", "minimum": 0, "description": "Levels to read below root; 0 reads root-level elements only, and unnamed containers don't count. Default: all." })
}

fn tree_root() -> Value {
    json!({ "type": "integer", "minimum": 0, "description": "A bracketed element number to use as the returned tree's root. Omit for the whole window. A stale number fails." })
}

fn element_match(purpose: &str) -> Value {
    json!({
        "type": "object",
        "description": format!("{purpose} Give role, text or both."),
        "properties": {
            "role": { "type": "string", "description": "A role as the tree writes it, such as button, statictext or row; also matches a subrole." },
            "text": { "type": "string", "description": "Part of the element's label or value, in any case." }
        },
        "additionalProperties": false
    })
}

fn user_instruction(purpose: &str) -> Value {
    json!({ "type": "string", "description": format!("The user's own words from this chat that tell you to {purpose}, quoted exactly. Agent Screen can't see the chat; it shows and records them.") })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

struct Hints {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    /// Reaches beyond the Mac: a URL in a browser, or GitHub.
    open_world: bool,
}

const PLAIN: Hints = Hints {
    read_only: false,
    destructive: false,
    idempotent: false,
    open_world: false,
};
const READ_ONLY: Hints = Hints {
    read_only: true,
    ..PLAIN
};
const OPEN_WORLD: Hints = Hints {
    open_world: true,
    ..PLAIN
};

/// Whether tool `name` only reads, so a call cut off midway is safe to
/// repeat.
pub fn read_only(name: &str) -> bool {
    definitions()
        .iter()
        .find(|t| t["name"] == name)
        .is_some_and(|t| t["annotations"]["readOnlyHint"] == true)
}

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
            "openWorldHint": hints.open_world
        }
    })
}

/// Claude Code defers MCP tools behind its tool search and shows only their
/// names. A tool marked here loads upfront, so the model sees what it is for
/// before it reaches for open -a or AppleScript.
fn always_load(mut tool: Value) -> Value {
    tool["_meta"] = json!({ "anthropic/alwaysLoad": true });
    tool
}

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "get_diagnostics",
            "Collect bug report diagnostics",
            &[
                "Collect build metadata for THIS MCP process and the responding Agent Screen app, for a bug report a person files by hand.",
                "To report an Agent Screen bug yourself, call report_issue, which includes this report.",
                "Works before create_display. Starts nothing, changes nothing and uploads nothing.",
            ],
            object(json!({}), &[]),
            READ_ONLY,
        ),
        tool(
            "report_issue",
            "Report an Agent Screen bug",
            &[
                "Report a bug in Agent Screen itself: one of its tools failed, hung or did something other than its description says.",
                "Not for bugs in the app you operate, and not for your own mistakes. Call it once per failure, in the session where it happened.",
                "Agent Screen adds its versions, your client's name, this session's diagnostics and its log lines.",
                "Reports are public. Known private patterns are redacted, but not every private word can be:",
                "write only about Agent Screen's behavior, never what the user typed or what a window showed.",
                "The report waits on a card under Agent Screen's menu bar icon until the user reviews and submits it; nothing leaves the Mac before.",
                "The result shows the report as it would be sent and a status: waiting_for_user, duplicate (with the known issue's number),",
                "fixed_in_newer_release (ask the user to update), already_reported, too_many or off. Tell the user what happened.",
            ],
            object(
                json!({
                    "title": { "type": "string", "description": "One line naming the failure, such as \"computer left_click on a Safari tab does nothing\"." },
                    "steps": {
                        "type": "array",
                        "description": "What you did, in order, as sentences about Agent Screen's tools.",
                        "items": { "type": "string" }
                    },
                    "calls": {
                        "type": "array",
                        "description": "The Agent Screen tool calls that led to the failure, in order. Arguments keep only handles, coordinates, actions and sizes; text, titles, URLs and paths are removed.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": { "type": "string" },
                                "arguments": { "type": "object" },
                                "result": { "type": "string", "description": "What the call returned, shortened." }
                            },
                            "required": ["tool"]
                        }
                    },
                    "expected": { "type": "string", "description": "What you expected to happen." },
                    "actual": { "type": "string", "description": "What happened instead." },
                    "reproducible": { "type": "string", "enum": ["always", "sometimes", "once", "unknown"], "description": "Whether doing it again fails again. Default unknown." },
                    "tool": { "type": "string", "description": "The Agent Screen tool that failed, such as computer." },
                    "error": { "type": "string", "description": "The error text that tool returned, exactly. Reports with the same tool and error count as the same failure." }
                }),
                &["title", "expected", "actual"],
            ),
            OPEN_WORLD,
        ),
        always_load(tool(
            "create_display",
            "Create Agent Display",
            &[
                "Get a private virtual display for this session to operate GUI apps on, without touching the user's screen,",
                "cursor or keyboard. Call it when a task needs a screen: to open, operate, test or screenshot a Mac app or a",
                "web page, in place of open -a, AppleScript, System Events or screencapture. Calling it again returns the same display, and so",
                "does a call from a subagent of this session: let one subagent do all the GUI work.",
                "Every tool takes the returned display_id, valid until end_display or until this session ends.",
                "The result includes preview_url, a local read-only link to watch the display: give preview_url to the user.",
                "width and height are in points; scale is screenshot pixels per point. Every other tool uses screenshot pixels.",
                "Put windows on it with launch_app or place_window, then operate it with computer.",
            ],
            object(
                json!({
                    "title": { "type": "string", "description": "Short task title shown to the user, e.g. \"Book flights\". Give each of the session's displays its own." },
                    "new": { "type": "boolean", "description": "Make another display instead of returning the latest. Each one makes the whole Mac reconfigure its screens: only when the task needs two at once. At most 4 per session." },
                    "width": { "type": "integer", "minimum": 640, "maximum": 2560, "description": "Width in points, 640 to 2560. Default 1280. A wider display's screenshots are scaled down to fit the image limit." },
                    "height": { "type": "integer", "minimum": 480, "maximum": 1600, "description": "Height in points, 480 to 1600. Default 800." }
                }),
                &[],
            ),
            PLAIN,
        )),
        tool(
            "end_display",
            "End Agent Display",
            &[
                "Finish with the display. It accepts no more input and its display_id stops working. Windows the user",
                "handed over go back to them now. If other windows remain, it waits for the user to review them; give",
                "each a set_disposition first. An empty display closes. Your other displays, if any, go on.",
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
                "action you take and per Takeover, each with t, its seconds into the video. stop finishes both and returns",
                "their paths, and under sound which apps are heard: only an app whose every window is on your display.",
                "A recording also stops when the display ends, and after 30 minutes. The user sees that the display is being recorded.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "action": { "type": "string", "enum": ["start", "stop"], "description": "start or stop." }
                }),
                &["display_id", "action"],
            ),
            PLAIN,
        ),
        tool(
            "computer",
            "Operate the display",
            &[
                "See and operate your Agent Display. Coordinates are screenshot pixels from the display's top-left corner.",
                "Each action returns a fresh screenshot, unless screenshot is false; wait and cursor_position return text only,",
                "and zoom returns a region at up to twice the detail, to read small text (aim with the full screenshot).",
                "type sends about 200 characters a second: for long text in a field, use act_on_element insert_text.",
                "Keys go to the window you last clicked; after a shortcut opens a window, click in it before typing.",
                "Apps here stay in the background, so shortcuts that act on the selection (copy, paste, undo, bold) do nothing;",
                "use act_on_element read_text, insert_text or set_value. App commands such as cmd+n, and cmd+a, work.",
                "The strip at the top takes no clicks: reach an app's menus through the menu bar list at the end of get_window_state.",
                "When the app is also the user's front app, with another of its windows focused, a single left click and type",
                "go in through accessibility. Other input waits up to 30 s for that window to lose focus, then returns",
                "input_pending with a request_id: repeat the action or call input_queue wait. It still runs by itself once the",
                "user leaves, or at once if they press Let the agent continue, unless another action of yours replaces it.",
                "The text says how input went in.",
                "For a drag left_click_drag can't express, use left_mouse_down, mouse_move steps, then left_mouse_up;",
                "without a coordinate they act at the pointer.",
                "A drag that needs the user's pointer, such as one between windows, returns input_pending: nothing was sent; see input_queue.",
                "Click with the normal background route first. If the screenshot shows a click had no effect, retry the same left,",
                "double or triple click with real_input true, which asks the user to allow their real pointer. Never on the first try.",
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
                    "text": { "type": "string", "description": "type: the text. key: an xdotool chord such as \"Return\", \"cmd+l\" or \"cmd+shift+t\"; hold_key: the key to hold. Clicks, drags and scroll: held modifiers such as \"shift\" or \"cmd+shift\"." },
                    "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "up, down, left or right. Default down." },
                    "scroll_amount": { "type": "integer", "description": "Scroll wheel clicks. Default 3." },
                    "duration": { "type": "number", "description": "Seconds: for wait, at most 30; for hold_key, at most 10." },
                    "screenshot": { "type": "boolean", "description": "false returns only the action's text, without the screenshot, for steps you don't need to see, such as a run of key presses. Default true." },
                    "real_input": {
                        "type": "boolean",
                        "description": "For left_click, double_click, triple_click, left_click_drag or left_mouse_down, after a background attempt visibly had no effect: ask the user to allow their real pointer. It runs only once they allow it."
                    },
                    "via_input_method": {
                        "type": "boolean",
                        "description": "For type and key: send the keys through the user's input method (pinyin, Japanese, Korean) so they compose; text is then the keys to press, such as \"nihao \" for 你好. Compose and commit in one call, at most 250 keys. It briefly brings the app to the front and uses the user's keyboard: only to test input-method behavior."
                    },
                    "input_source": {
                        "type": "string",
                        "description": "With via_input_method: the input source for the call, such as com.apple.inputmethod.SCIM.ITABC. Default: the current one."
                    }
                }),
                &["display_id", "action"],
            ),
            PLAIN,
        ),
        tool(
            "input_queue",
            "Inspect or withdraw an input request",
            &[
                "A drag or click that needs the user's pointer enters the shared Input Queue and reports its request_id.",
                "wait waits for approval and then for the drag or click to finish; status lists your requests; withdraw cancels one;",
                "done withdraws and clears this session's requests on all its displays, once you need no more input.",
                "request asks for approval ahead, with a reason and a stable operation token (repeating the token returns its newest request),",
                "but runs nothing: once approved, send the complete drag or click with real_input.",
                "approved means allowed, not run: only completed means it ran. approval_lapsed means the approval ended with nothing run,",
                "and the user won't be asked again. waiting_for_app is a click, key or type that only the user's focused window",
                "of its app blocks: it needs no approval, runs once that window loses focus or the user presses Let the agent continue,",
                "for up to 10 minutes, and wait returns its result.",
                "After input_pending, don't continue a split drag or assume success;",
                "use a background alternative or request_help.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "action": { "type": "string", "enum": ["request", "wait", "status", "withdraw", "done"] },
                    "request_id": { "type": "integer", "minimum": 1, "description": "The request_id reported by the blocked action or input_queue." },
                    "operation": { "type": "string", "minLength": 1, "maxLength": 2048, "description": "For request: a stable token for this intended action. Reuse it when retrying the same request." },
                    "reason": { "type": "string", "minLength": 1, "maxLength": 300, "description": "For request: what the user is being asked to approve." },
                    "timeout_seconds": { "type": "number", "minimum": 0, "maximum": 60, "description": "For request or wait: seconds to wait. Default 60. Timeout keeps the request queued." },
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
                "Ask the user to step in on your display for something you cannot do, such as a login, a two-factor code,",
                "or a captcha they prefer to do. They get a card and a notification with your reason, and the call waits",
                "for their answer: tookOver (they took over and handed the display back), continue (go on without them),",
                "handedOver (one of their windows, named in window), or instructions (user_reply holds their words: follow",
                "them for this step). Take a screenshot before you go on. On a timeout the request stays open; call",
                "wait_for_control to keep waiting. If you got past it yourself, call it with withdraw: true. When the user",
                "answers in this chat instead (\"done, go on\"), pass their words as user_instruction to close the request;",
                "use it only for an explicit answer the user gave in this chat.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "reason": { "type": "string", "description": "What you need the user to do, in one short sentence." },
                    "steps": { "type": "array", "items": { "type": "string" }, "maxItems": 5, "description": "Up to 5 short steps, when the reason alone can't say how. The Viewer lists them while the user is in control." },
                    "withdraw": { "type": "boolean", "description": "Take back the open request instead; reason is then not needed." },
                    "user_instruction": user_instruction("go on without them stepping in"),
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
                "back, with a short reason. They get a card with Let <agent> Continue and Keep Control, and the call",
                "waits like wait_for_control. control: user with kept: true means they chose to keep control: don't ask",
                "again right away. One request per display: asking again while one is open waits on. On a timeout the request stays open;",
                "call wait_for_control, or withdraw: true when you no longer need the display.",
                "When the user tells you in this chat to continue (\"go ahead\"), pass their words as user_instruction:",
                "the Takeover ends at once. When they tell you to stop and let them take over, pass their words with",
                "control: user. Use user_instruction only for an explicit instruction from the user, never on your own judgment.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "reason": { "type": "string", "description": "Why you need the display back, in one short sentence." },
                    "withdraw": { "type": "boolean", "description": "Take back the open request instead; reason is then not needed." },
                    "user_instruction": user_instruction("continue, or to stop and let them take over"),
                    "control": { "type": "string", "enum": ["agent", "user"], "description": "With user_instruction: agent (default) hands the display back to you; user starts a Takeover." },
                    "timeout_seconds": timeout()
                }),
                &["display_id"],
            ),
            PLAIN,
        ),
        tool(
            "window_access",
            "Use the user's windows on their word",
            &[
                "Act on what the user told you in this chat about their windows, quoting their words in user_instruction.",
                "Agent Screen can't check them: use it only for an explicit instruction.",
                "grant lets this session take the user's windows of the named apps, or of every app with all: true, by",
                "place_window without a request card, onto any of its displays. It lasts until revoke, until this session",
                "has no display left, or until its connection closes. It pauses on a display while the user controls it,",
                "never covers a window on another agent's display, and skips a window the user took back until they tell",
                "you to take it again. revoke takes back the grant, or the apps named. return gives Handovers back,",
                "never your own windows. return and status cover display_id only. Every action returns the status.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "action": { "type": "string", "enum": ["grant", "revoke", "return", "status"] },
                    "apps": { "type": "array", "items": { "type": "string" }, "description": "For grant and revoke: app names or bundle ids, e.g. \"Safari\" or \"com.utmapp.UTM\"." },
                    "all": { "type": "boolean", "description": "For grant: every app. For return: every Handover. revoke without apps takes back the whole grant." },
                    "window_id": { "type": "integer", "description": "For return: the Handover to give back." },
                    "user_instruction": user_instruction("do this; required for grant and return")
                }),
                &["display_id", "action"],
            ),
            PLAIN,
        ),
        tool(
            "list_windows",
            "List windows on the display",
            &[
                "List app windows on the display, front to back, with ids and bounds in screenshot pixels; scale is",
                "screenshot pixels per point. origin says how a window got here; disposition is your set_disposition",
                "verdict or null. offDisplay: true means the window is yours but not on the display: call place_window.",
                "offScreen means the window is yours and open but shows nowhere: minimized, hidden (its app is hidden) or",
                "otherSpace (on a macOS Space the display isn't showing); the note says what to do.",
                "With strays: true it lists instead your apps' windows that opened off the display, and your windows that left it.",
                "It never lists the user's windows; to ask for one, call place_window with the app's name.",
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
                "Move a window onto the display, by window_id or by app name or bundle id. A window of an app launch_app",
                "started for you moves at once (ids from list_windows with strays: true), and so does one from another of",
                "your displays. Any other window is the user's or another session's: Agent Screen asks the user and the call",
                "waits up to timeout_seconds; handed over, it is on the display as a Handover. When the user told you in this",
                "chat to take that window, pass their words as user_instruction and it moves without asking; so does a",
                "window of an app under window_access. Start GUI apps with launch_app, not open -a, so their windows come",
                "here without asking; their new windows usually follow by themselves.",
                "x, y, width and height set the frame in screenshot pixels from the display's top-left corner, as list_windows",
                "reports bounds; each one left out keeps the window's own. Pass a window_id already on the display to move",
                "or resize it there. For a window already on this display, no frame arguments leaves its frame unchanged.",
                "An explicit frame stays below the reserved menu bar strip, and the result's window is the frame the window ended with.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": { "type": "integer", "description": "The window to move: a stray, the user's, one on another of your displays or another session's (the user decides), or one already here." },
                    "app": { "type": "string", "description": "Instead of window_id: the app whose front window to move, by name or bundle id, e.g. \"UTM\" or \"com.utmapp.UTM\"." },
                    "x": { "type": "number", "description": "Left edge in screenshot pixels from the display's left edge." },
                    "y": { "type": "number", "description": "Top edge in screenshot pixels from the display's top edge." },
                    "width": { "type": "number", "description": "Width in screenshot pixels." },
                    "height": { "type": "number", "description": "Height in screenshot pixels." },
                    "timeout_seconds": { "type": "number", "minimum": 1, "maximum": 600, "description": "How long to wait for the user to answer a request for one of their windows, 1 to 600. Default 120." },
                    "user_instruction": user_instruction("take this window")
                }),
                &["display_id"],
            ),
            PLAIN,
        ),
        tool(
            "launch_app",
            "Launch an app on the display",
            &[
                "Open an app in the background and put its new window on the display. If the app is already running,",
                "Agent Screen asks it for a new window and moves only that one, never a window of the user's.",
                "open opens a URL or file in the window. For a running app that can't open a new window, such as System Settings,",
                "a URL opens in its window once every window of the app is on your display (place_window first).",
                "arguments or environment start a new instance of the app with them,",
                "as a development build needs. Google Chrome starts as a separate instance for agents, on a profile every agent",
                "session shares, which keeps its sign-ins and has none of the user's. Another Chromium browser the user runs",
                "opens in their own process, and the result says shared: true with a note. For a separate profile in a browser",
                "the user doesn't run, pass arguments such as --user-data-dir=/tmp/profile. It does not reuse the user's existing login session:",
                "ask the user before making one or signing in.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "app": { "type": "string", "description": "App name (\"Safari\", \"Finder\"), bundle id (\"com.apple.Safari\") or the absolute path of an .app bundle (\"/tmp/build/MyApp.app\")." },
                    "open": { "type": "string", "description": "A URL (https://…, file://…) or a file path starting with / or ~/ to open in the app. Relative paths are refused." },
                    "arguments": { "type": "array", "items": { "type": "string" }, "description": "Command-line arguments for a new instance of the app, passed as given." },
                    "environment": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for a new instance of the app." }
                }),
                &["display_id", "app"],
            ),
            OPEN_WORLD,
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
                    "disposition": { "type": "string", "enum": ["keep", "close"], "description": "keep or close." }
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
                "brackets that act_on_element takes, and its center in screenshot pixels. A number stays with its element;",
                "one whose element went away fails instead of hitting another. In a large window, read first with a small",
                "max_depth (a branch with unread children is marked \"more inside\"), then read that branch with root.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": window_id(),
                    "root": tree_root(),
                    "max_depth": max_depth(),
                    "image": { "type": "boolean", "description": "false leaves out the window image and returns the tree only. Default true." }
                }),
                &["display_id", "window_id"],
            ),
            READ_ONLY,
        ),
        tool(
            "act_on_element",
            "Act on a window element",
            &[
                "Act on an element from get_window_state, then return the window's new state. press, show_menu, focus,",
                "set_value, increment, decrement and scroll_into_view use accessibility actions, which reach the app without",
                "focusing it; click, double_click and right_click post a click at the element's center, in the background.",
                "press falls back to that click when the element has no press action; increment and decrement never do.",
                "Pressing a menubaritem lists that menu's items instead of opening it.",
                "set_value replaces a field's text. On a popupbutton or a web page's select it takes the exact full title of",
                "an item, case included, and never opens the menu: prefer it to opening a menu, which closes at the user's",
                "next key anywhere. read_text returns a text element's whole value and selection, which the tree cuts at",
                "80 characters: use it to copy. insert_text replaces the selection, or inserts at the caret: use it to paste.",
                "If an action is not confirmed, check the current value before retrying.",
                "result picks what comes back: state (default), the whole window state; changes, only the elements that",
                "appeared (+), changed (~) or went away (-) since the last read with the same root and max_depth, with the",
                "window image; none, only that it is done. root and max_depth scope the returned state, not the target.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": window_id(),
                    "element": { "type": "integer", "description": "The bracketed number from get_window_state." },
                    "action": { "type": "string", "enum": [
                        "press", "click", "double_click", "right_click", "show_menu", "focus", "set_value", "insert_text",
                        "read_text", "scroll_into_view", "increment", "decrement"
                    ] },
                    "value": { "type": "string", "description": "Text for set_value and insert_text." },
                    "real_input": {
                        "type": "boolean",
                        "description": "Only for press on an Electron app's menu bar item. true: a real click once the user allows it. false: the item's keyboard shortcut, which some apps ignore while a text field has focus. Default: a real click only while a text field has focus."
                    },
                    "result": { "type": "string", "enum": ["state", "changes", "none"], "description": "What to return after the action. Default state." },
                    "root": tree_root(),
                    "max_depth": max_depth()
                }),
                &["display_id", "window_id", "element", "action"],
            ),
            PLAIN,
        ),
        tool(
            "wait_for_change",
            "Wait for a window to change",
            &[
                "Wait until one window on your display changes, then say what changed, instead of sleeping and taking",
                "screenshots: use it after an action whose effect lands later, such as a sent message or a page load.",
                "It compares the window's tree with the last tree you got for it with the same root and max_depth (from",
                "get_window_state, act_on_element or an earlier wait), so a change that landed before the call fires at once.",
                "It fires on changed (any element appearing, changing or going away), or on appears or disappears: more or",
                "fewer elements matching role and text than that tree had. It returns the conditions that fired and the",
                "changes (+ new, ~ changed, - gone), or timed_out; also when the window closes, leaves your display or goes out",
                "of sight (minimized, its app hidden, or on another macOS Space), or the display ends. timeout_seconds is 30 by default and at most 120: call again to keep waiting. A read sees at most",
                "400 elements: in a long window, such as a chat",
                "history, pass root to watch the part you need.",
            ],
            object(
                json!({
                    "display_id": display_id(),
                    "window_id": window_id(),
                    "root": tree_root(),
                    "max_depth": max_depth(),
                    "changed": {
                        "type": "boolean",
                        "description": "Fire on any element appearing, changing or going away. Default: true unless appears or disappears is given."
                    },
                    "appears": element_match("Fire when more elements match than that tree had, such as a new row."),
                    "disappears": element_match("Fire when fewer elements match than that tree had, such as a spinner gone."),
                    "timeout_seconds": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 120,
                        "description": "Seconds to wait, at most 120. Default 30; 0 checks once."
                    }
                }),
                &["display_id", "window_id"],
            ),
            READ_ONLY,
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

    fn bool(&self, key: &str) -> Result<Option<bool>, ControlError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(ControlError::invalid(format!(
                "{key} must be true or false"
            ))),
        }
    }

    fn flag(&self, key: &str) -> bool {
        self.0.get(key) == Some(&Value::Bool(true))
    }

    fn user_instruction(&self) -> Result<Option<String>, ControlError> {
        self.string("user_instruction")?
            .map(|w| agent_screen_model::access::user_instruction(&w))
            .transpose()
            .map_err(ControlError::invalid)
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
/// `new: true` answered by an app older than ADR 0016, which ignores it
/// and reuses the session's display: say so, rather than hand the agent
/// the display it already has as if it were another.
fn needs_newer_app(new: bool, reply: &Object) -> Option<ControlError> {
    let reused = reply.get("reused").and_then(Value::as_bool) == Some(true);
    let able = reply.get("multiDisplay").and_then(Value::as_bool) == Some(true);
    (new && reused && !able).then(|| {
        ControlError::new("failed", format!(
            "the running Agent Screen app is older than this MCP server and can't give a session a second display. Ask the user to update Agent Screen. Until then, keep using display {}",
            reply.get("displayId").and_then(Value::as_u64).unwrap_or_default()
        ))
    })
}

const RESTARTED: &str = "Agent Screen quit since your last call and restarted; your earlier display ended with it, and macOS moved its windows to the user's screen. Windows of apps you launched there are strays now: list_windows with strays: true, then place_window.";

/// What a call naming a display that ended when the app quit gets back.
pub fn ended_with_app(id: i64) -> ControlError {
    ControlError::new("ended", agent_screen_model::ended::app_quit_message(id))
}

// MARK: Handlers

/// Resolves when the client cancels the call.
pub type Cancelled = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

pub async fn run(
    name: &str,
    args: &Object,
    session: &Session,
    control: &Control,
    cancelled: Cancelled,
) -> Result<Output, ControlError> {
    let a = Args(args);
    match name {
        "get_diagnostics" => Ok(Output::Object(
            crate::diagnostics::collect(
                crate::diagnostics::Collector::Mcp,
                env!("CARGO_PKG_VERSION"),
            )
            .await
            .to_json(),
        )),
        "create_display" => create_display(&a, session, control).await,
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
            let display = a.require_int("display_id")?;
            // What is on the display as it ends: Handovers go back, the
            // rest waits for the user's review.
            let windows = control
                .call("listWindows", json!({ "displayId": display }))
                .await
                .ok();
            control
                .call("endDisplay", json!({ "displayId": display }))
                .await?;
            Ok(Output::Object(match windows {
                Some(list) => end_output(&list),
                None => json!({ "ended": true }),
            }))
        }
        "wait_for_control" => {
            let reply = control
                .call(
                    "waitForControl",
                    json!({ "displayId": a.require_int("display_id")?, "timeoutSeconds": a.number("timeout_seconds")? }),
                )
                .await?;
            Ok(Output::Object(describe_wait(&reply)))
        }
        "request_help" => request_help(&a, control).await,
        "request_control" => request_control(&a, control).await,
        "window_access" => {
            let params = window_access_params(&a)?;
            let action = params["action"].as_str().unwrap_or_default().to_string();
            let reply = control.call("windowAccess", params).await?;
            Ok(Output::Object(access_output(&action, reply)))
        }
        "list_windows" => {
            let reply = control
                .call(
                    "listWindows",
                    json!({ "displayId": a.require_int("display_id")?, "strays": a.flag("strays") }),
                )
                .await?;
            Ok(Output::Object(Value::Object(reply)))
        }
        "place_window" => place_window(&a, control).await,
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
                control.ledger().started_app(display as u64, pid);
            }
            Ok(Output::Object(launch_output(&Value::Object(reply))))
        }
        "get_window_state" => {
            let scope = scope_arg(&a)?;
            let depth = scope.max_depth;
            let state = window_state(
                control,
                a.require_int("display_id")?,
                a.require_int("window_id")?,
                scope,
                a.bool("image")?.unwrap_or(true),
            )
            .await?;
            Ok(Output::Content(state.content(depth)))
        }
        "input_queue" => input_queue(&a, control).await,
        "wait_for_change" => wait_for_change(&a, control, cancelled).await,
        "act_on_element" => act_on_element(&a, control).await,
        "computer" => computer(&a, control).await.map(Output::Content),
        other => Err(ControlError::invalid(format!("unknown tool {other}"))),
    }
}

async fn create_display(
    a: &Args<'_>,
    session: &Session,
    control: &Control,
) -> Result<Output, ControlError> {
    let title = a.string("title")?.unwrap_or_else(|| session.folder.clone());
    let new = a.flag("new");
    // A display ended by the app quitting hands its apps on, so
    // their windows, now on the user's screen, are strays here.
    // Only this session's, by the display they were on, and taken
    // only once a display is made.
    let orphans = control.ledger().orphans(&session.key);
    let groups = orphans.as_ref().map(|groups| {
        groups
            .iter()
            .map(|(display, pids)| json!({ "display": display, "pids": pids }))
            .collect::<Vec<_>>()
    });
    // The same pids flat, for an app older than ADR 0016, which
    // reads only `adopt` (ADR 0005).
    let adopt = orphans.as_ref().map(|groups| {
        groups
            .iter()
            .flat_map(|(_, pids)| pids.clone())
            .collect::<Vec<_>>()
    });
    let reply = control
        .call(
            "createDisplay",
            json!({
                "sessionKey": session.key,
                "client": session.client,
                "title": title,
                "width": a.int("width")?,
                "height": a.int("height")?,
                "new": new,
                "adopt": adopt,
                "adoptGroups": groups
            }),
        )
        .await?;
    let reused = reply.get("reused").and_then(Value::as_bool) == Some(true);
    if let Some(id) = reply.get("displayId").and_then(Value::as_u64) {
        // Only a display made takes the orphans: after a refusal, the
        // session's next create_display still hands them on.
        if orphans.is_some() {
            control.ledger().adopted(&session.key);
        }
        control.ledger().made_display(id, &session.key, reused);
    }
    if let Some(error) = needs_newer_app(new, &reply) {
        return Err(error);
    }
    let mut out = json!({
        "display_id": reply.get("displayId"),
        "width": reply.get("width"),
        "height": reply.get("height"),
        "scale": reply.get("scale"),
        "reused": reply.get("reused")
    });
    if let Some(preview_url) = reply.get("preview_url").filter(|v| !v.is_null()) {
        out["preview_url"] = preview_url.clone();
    }
    if let Some(mode) = reply.get("autonomyMode") {
        out["autonomyMode"] = mode.clone();
    }
    if let Some(displays) = reply.get("displays").and_then(Value::as_array) {
        out["displays"] = json!(
            displays
                .iter()
                .map(|d| json!({ "display_id": d.get("displayId"), "label": d.get("label") }))
                .collect::<Vec<_>>()
        );
    }
    if let Some(note) = reply.get("note") {
        out["note"] = note.clone();
    } else if orphans.is_some() {
        out["note"] = json!(RESTARTED);
    }
    // "Displays have separate Spaces" is off: what a desktop switch does.
    if let Some(spaces) = reply.get("spaces").filter(|v| v.is_string()) {
        out["spaces"] = spaces.clone();
    }
    Ok(Output::Object(out))
}

async fn request_help(a: &Args<'_>, control: &Control) -> Result<Output, ControlError> {
    let display = a.require_int("display_id")?;
    if let Some(words) = a.user_instruction()? {
        control
            .call(
                "answerHelp",
                json!({ "displayId": display, "userInstruction": words }),
            )
            .await?;
        return Ok(Output::Object(json!({
            "control": "agent",
            "answer": "continue",
            "answeredIn": "chat",
            "note": "The Help Request ended on the user's words as you relayed them; Agent Screen didn't check them against the chat. Take a screenshot before you go on."
        })));
    }
    if a.flag("withdraw") {
        let reply = control
            .call("withdrawHelp", json!({ "displayId": display }))
            .await?;
        let withdrawn = reply.get("withdrawn") == Some(&Value::Bool(true));
        return Ok(Output::Object(json!({ "withdrawn": withdrawn })));
    }
    let reason = a
        .string("reason")?
        .ok_or_else(|| ControlError::invalid("reason is required"))?;
    // `answers`: this server tells the agent each answer, so the user
    // is offered Continue, Reply and Give a Window (ADR 0015).
    let asked = control
        .call(
            "requestHelp",
            json!({ "displayId": display, "reason": reason, "steps": a.0.get("steps"), "answers": true }),
        )
        .await?;
    // The user answered the last Help Request after its wait ended:
    // that answer stands, and they aren't asked again.
    if let Some(Value::Object(answered)) = asked.get("answered") {
        let mut out = describe_wait(answered);
        let note = out.get("note").and_then(Value::as_str).unwrap_or_default();
        out["note"] = json!(format!(
            "The user answered your last Help Request after its wait ended, so they weren't asked again. {note}"
        ));
        out["answeredEarlier"] = json!(true);
        return Ok(Output::Object(out));
    }
    let reply = control
        .call(
            "waitForControl",
            json!({ "displayId": display, "timeoutSeconds": a.number("timeout_seconds")? }),
        )
        .await?;
    Ok(Output::Object(describe_wait(&reply)))
}

async fn request_control(a: &Args<'_>, control: &Control) -> Result<Output, ControlError> {
    let display = a.require_int("display_id")?;
    let control_arg = a.one_of("control", &["agent", "user"])?;
    if let Some(words) = a.user_instruction()? {
        let to = control_arg.unwrap_or_else(|| "agent".into());
        let reply = control
            .call(
                "requestControl",
                json!({ "displayId": display, "userInstruction": words, "control": to }),
            )
            .await?;
        return Ok(Output::Object(chat_control_output(&reply, &to)));
    }
    if control_arg.is_some() {
        return Err(ControlError::invalid(
            "control needs user_instruction: the user's words from the chat",
        ));
    }
    if a.flag("withdraw") {
        let reply = control
            .call("withdrawControlRequest", json!({ "displayId": display }))
            .await?;
        let withdrawn = reply.get("withdrawn") == Some(&Value::Bool(true));
        return Ok(Output::Object(json!({ "withdrawn": withdrawn })));
    }
    let reason = a
        .string("reason")?
        .ok_or_else(|| ControlError::invalid("reason is required"))?;
    let asked = control
        .call(
            "requestControl",
            json!({ "displayId": display, "reason": reason }),
        )
        .await?;
    let reply = control
        .call(
            "waitForControl",
            json!({ "displayId": display, "timeoutSeconds": a.number("timeout_seconds")? }),
        )
        .await?;
    let mut out = describe_wait(&reply);
    if asked.get("alreadyAsked") == Some(&Value::Bool(true)) {
        out["alreadyAsked"] = json!(true);
    }
    Ok(Output::Object(out))
}

async fn place_window(a: &Args<'_>, control: &Control) -> Result<Output, ControlError> {
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
        "userInstruction": a.user_instruction()?,
    });
    for (key, value) in asked {
        params[key] = json!(value);
    }
    let reply = control.call("placeWindow", params).await?;
    Ok(Output::Object(place_output(&reply, &asked)))
}

async fn input_queue(a: &Args<'_>, control: &Control) -> Result<Output, ControlError> {
    let action = a
        .one_of("action", &["request", "wait", "status", "withdraw", "done"])?
        .ok_or_else(|| ControlError::invalid("action is required"))?;
    let request_id = a.int("request_id")?;
    if request_id.is_some_and(|id| id < 1) {
        return Err(ControlError::invalid("request_id must be positive"));
    }
    // The app checks the same rule, before anything changes.
    agent_screen_model::input_queue::Action::parse(&action, request_id.map(|id| id as u64))
        .map_err(ControlError::invalid)?;
    let operation = a.string("operation")?;
    let reason = a.string("reason")?;
    if action == "request"
        && (operation
            .as_ref()
            .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 2048)
            || reason
                .as_ref()
                .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 300))
    {
        return Err(ControlError::invalid(
            "request needs operation (1..2048 characters) and reason (1..300 characters)",
        ));
    }
    let timeout = a.number("timeout_seconds")?.unwrap_or(60.0);
    if !(0.0..=60.0).contains(&timeout) {
        return Err(ControlError::invalid(
            "timeout_seconds must be between 0 and 60",
        ));
    }
    let result = control
        .call(
            "inputQueue",
            json!({
                "displayId": a.require_int("display_id")?,
                "action": action,
                "requestId": request_id,
                "operation": operation,
                "reason": reason,
                "timeout": timeout,
            }),
        )
        .await?;
    Ok(Output::Object(Value::Object(result)))
}

async fn wait_for_change(
    a: &Args<'_>,
    control: &Control,
    cancelled: Cancelled,
) -> Result<Output, ControlError> {
    let display = a.require_int("display_id")?;
    let window = a.require_int("window_id")?;
    let scope = scope_arg(a)?;
    let conditions = json!({
        "changed": a.0.get("changed"),
        "appears": a.0.get("appears"),
        "disappears": a.0.get("disappears"),
    });
    // The app checks the same rules, before it reads anything.
    agent_screen_model::watch::Conditions::from_json(&conditions).map_err(ControlError::invalid)?;
    let timeout = agent_screen_model::watch::timeout(a.number("timeout_seconds")?)
        .map_err(ControlError::invalid)?;
    static NEXT_WAIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let wait = NEXT_WAIT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let baseline = control.last_tree(display, window, scope);
    let call = control.call(
        "waitForChange",
        json!({
            "displayId": display,
            "windowId": window,
            "root": scope.root,
            "maxDepth": scope.max_depth,
            "conditions": conditions,
            "timeoutSeconds": timeout.as_secs_f64(),
            "baseline": baseline.as_ref().map(|(tree, _)| tree),
            "baselineTruncated": baseline.as_ref().is_some_and(|(_, cut)| *cut),
            "waitId": wait,
        }),
    );
    // The client cancelled the call: stop the app's reads too. An
    // older app doesn't know the method; its wait ends at the timeout.
    let reply = tokio::select! {
        reply = call => reply,
        () = cancelled => {
            let _ = control.call_open("cancelWaitForChange", json!({ "waitId": wait })).await;
            return Err(ControlError::new("failed", "the call was cancelled"));
        }
    };
    let reply = reply.map_err(|error| {
        if error.message.contains("unknown method") {
            ControlError::new(
                "failed",
                "this Agent Screen app predates wait_for_change; update Agent Screen",
            )
        } else {
            error
        }
    })?;
    // The tree becomes the one the agent got only if it saw every
    // change; otherwise the next comparison still shows the rest.
    let fired = reply.get("outcome").and_then(Value::as_str) == Some("fired");
    let changes = reply.get("changes").and_then(Value::as_str);
    if let Some(tree) = reply.get("tree").and_then(Value::as_str)
        && (fired || agent_screen_model::watch::shown_whole(changes))
    {
        // The tree's own read, not whether any read of the wait was cut.
        let cut = reply.get("treeTruncated") == Some(&Value::Bool(true));
        control.swap_tree(display, window, scope, tree.to_string(), cut);
    }
    Ok(Output::Content(vec![text(wait_output(
        display, window, scope.root, &reply,
    ))]))
}

async fn act_on_element(a: &Args<'_>, control: &Control) -> Result<Output, ControlError> {
    let display = a.require_int("display_id")?;
    let window = a.require_int("window_id")?;
    let result = a
        .one_of("result", &["state", "changes", "none"])?
        .unwrap_or_else(|| "state".into());
    let scope = scope_arg(a)?;
    let depth = scope.max_depth;
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
                "increment",
                "decrement",
            ],
        )?
        .ok_or_else(|| ControlError::invalid("action is required"))?;
    let real_input = a.bool("real_input")?;
    if real_input.is_some() && action != "press" {
        return Err(ControlError::invalid(
            "real_input applies to press on a menu item of an Electron app's menu bar",
        ));
    }
    // A legacy app ignores unknown parameters. Confirm support and
    // the root before even an action with an early text/menu reply.
    if scope.root.is_some() {
        window_state_reply(control, display, window, scope, false).await?;
    }
    let reply = control
        .call(
            "elementAction",
            json!({
                "displayId": display,
                "windowId": window,
                "element": a.require_int("element")?,
                "action": action,
                "value": a.string("value")?,
                "realInput": real_input,
                "root": scope.root,
                "maxDepth": scope.max_depth
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
    // A word on how the app took the action, before what it shows.
    let note = reply.get("note").and_then(Value::as_str).map(String::from);
    let noted = |output: Result<Output, ControlError>| match (output, &note) {
        (Ok(Output::Content(mut content)), Some(note)) => {
            content.insert(0, text(note.clone()));
            Ok(Output::Content(content))
        }
        (output, _) => output,
    };
    // The window state shows the action's result once the display
    // stops changing. Skip the window picture when result is "none".
    control
        .call("settle", json!({ "displayId": display }))
        .await?;
    let need_image = result != "none";
    noted(
        match window_state(control, display, window, scope, need_image).await {
            // The action ran: a stale or busy root afterward must not
            // read as a failure the agent would retry.
            Err(error) if scope.root.is_some() && error.code != "not_found" && result == "none" => {
                Ok(Output::Content(vec![text("Done.")]))
            }
            Err(error) if scope.root.is_some() && error.code != "not_found" => {
                Ok(Output::Content(vec![text(format!(
                    "Done. The action ran, but its requested subtree could not be read afterward: {}. Call get_window_state without root to find the current tree.",
                    error.message
                ))]))
            }
            // The action took its own window out of sight, as Minimize
            // does: it ran, and the window is still open.
            Err(error) if error.code == "off_screen" => Ok(Output::Content(vec![text(format!(
                "Done. Afterwards {}.",
                error.message
            ))])),
            // OK, Save or Close took its own window away: that is the
            // action working, so show what is on the display now. A
            // window the user gave to another session says so itself.
            Err(error)
                if error.code == "not_found"
                    && !error.message.contains("to another agent session") =>
            {
                let list = control
                    .call("listWindows", json!({ "displayId": display }))
                    .await?;
                let shot = control
                    .call("screenshot", json!({ "displayId": display }))
                    .await?;
                let get = |key: &str| shot.get(key).cloned().unwrap_or(Value::Null);
                Ok(Output::Content(vec![
                    text(closed_window_text(window, &list, &shot)),
                    image(&get("data"), &get("mimeType")),
                ]))
            }
            Err(error) => Err(error),
            Ok(state) => Ok(Output::Content(match result.as_str() {
                "none" => vec![text(format!("Done.{}", state.strayed))],
                "changes" => state.changes(depth),
                _ => state.content(depth),
            })),
        },
    )
}

/// place_window's result: the frame the window ended with, and a note
/// when it isn't the one asked for.
fn place_output(reply: &Object, asked: &[(&str, Option<f64>); 4]) -> Value {
    let mut out = json!({ "placed": true });
    if reply.get("handedOver") == Some(&Value::Bool(true)) {
        out["handedOver"] = json!(true);
    }
    // Which of the app's windows came, and whether from the Dock (#221).
    // Who took it and under which Autonomy Mode (ADR 0025): `mode` when
    // the mode took it without a Window Request.
    for key in ["windowId", "title", "fromDock", "authority", "autonomyMode"] {
        if let Some(value) = reply.get(key) {
            out[key] = value.clone();
        }
    }
    // From another of the session's displays (ADR 0016).
    if let Some(from) = reply.get("movedFrom") {
        out["moved_from"] = from.clone();
    }
    // From another agent session's display, on the user's word (ADR 0017).
    if let Some(from) = reply.get("takenFrom").and_then(Value::as_str) {
        out["taken_from"] = json!(from);
        out["note"] = json!(format!(
            "The user gave you this window, which {from} was using; that session is told. It is yours to work in now."
        ));
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
            "The window didn't take the whole frame you asked for: {}. Apps keep to their own minimum and maximum sizes. Agent Screen constrains explicit frames to the display below the reserved menu bar strip; read window for the actual frame. For a window larger than the display, call create_display with new: true and a larger size.",
            missed.join(", ")
        ));
    } else if reply.get("frameConstrained") == Some(&Value::Bool(true)) {
        out["note"] = json!(
            "Agent Screen adjusted the frame to fit below the reserved menu bar strip. This can shrink a dimension you left out; read window for the actual frame."
        );
    }
    out
}

/// What request_control on the user's word did: the app answers with who
/// has control now and whether that changed.
fn chat_control_output(reply: &Object, to: &str) -> Value {
    let changed = reply.get("changed") == Some(&Value::Bool(true));
    match (to, changed) {
        ("user", _) => json!({
            "control": "user",
            "paused": true,
            "note": if changed {
                "Done: the user is in control and your input is refused, as in any Takeover; window access is paused. Agent Screen acted on the words you relayed and didn't check them against the chat. Call wait_for_control, or request_control with user_instruction once they tell you to continue."
            } else {
                "The user was in control already; nothing changed."
            }
        }),
        (_, true) => json!({
            "control": "agent",
            "handedBack": "chat",
            "note": "The Takeover ended on the user's words as you relayed them; Agent Screen didn't check them against the chat. Take a screenshot before you continue: the user may have changed things."
        }),
        _ => json!({ "control": "agent", "note": "You had control already; nothing changed." }),
    }
}

/// window_access's control call, checked here so a mistake costs no
/// round trip: grant and return need the user's words and a target.
fn window_access_params(a: &Args<'_>) -> Result<Value, ControlError> {
    let action = a
        .one_of("action", &["grant", "revoke", "return", "status"])?
        .ok_or_else(|| ControlError::invalid("action is required"))?;
    let all = a.bool("all")?.unwrap_or(false);
    let apps: Option<Vec<String>> = match a.0.get("apps") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| ControlError::invalid("apps are non-empty strings"))
                })
                .collect::<Result<_, _>>()?,
        ),
        Some(_) => return Err(ControlError::invalid("apps is a list of app names")),
    };
    let window = a.int("window_id")?;
    let words = a.user_instruction()?;
    let needs_words = || {
        ControlError::invalid(format!(
            "{action} needs user_instruction: the user's words from this chat that tell you to. Without them, place_window asks the user with a card"
        ))
    };
    match action.as_str() {
        "grant" => {
            words.as_ref().ok_or_else(needs_words)?;
            match (all, apps.as_ref().is_some_and(|a| !a.is_empty())) {
                (true, true) => {
                    return Err(ControlError::invalid("pass apps or all: true, not both"));
                }
                (false, false) => {
                    return Err(ControlError::invalid(
                        "grant needs apps, or all: true when the user allowed every app",
                    ));
                }
                _ => {}
            }
        }
        "return" => {
            words.as_ref().ok_or_else(needs_words)?;
            if all == window.is_some() {
                return Err(ControlError::invalid(
                    "return needs window_id or all: true, one of them",
                ));
            }
        }
        "revoke" if all && apps.as_ref().is_some_and(|a| !a.is_empty()) => {
            return Err(ControlError::invalid(
                "pass apps or nothing, not all with apps",
            ));
        }
        _ => {}
    }
    Ok(json!({
        "displayId": a.require_int("display_id")?,
        "action": action,
        "all": all,
        "apps": apps,
        "windowId": window,
        "userInstruction": words,
    }))
}

/// window_access's result: the app's status, and a word on what changed.
fn access_output(action: &str, reply: Object) -> Value {
    let mut out = Value::Object(reply);
    let changed = out.get("changed") != Some(&Value::Bool(false));
    let stuck = out.get("stuck").is_some();
    let note = match action {
        "return" if stuck => Some(
            "The windows in returned are back where they came from. Those in stuck didn't leave your display and are still Handovers there: tell the user, and call return again if they ask. Window access no longer covers any of them until the user tells you to take them again.",
        ),
        "grant" => Some(
            "Granted on the user's words as you relayed them; Agent Screen didn't check them against the chat. place_window takes covered windows of theirs without a card while you have control.",
        ),
        "revoke" if changed => {
            Some("Revoked. Windows already on your display stay; place_window asks the user again.")
        }
        "revoke" => Some("Nothing to revoke."),
        "return" => Some(
            "Those windows are back where they came from, and window access no longer covers them until the user tells you to take them again.",
        ),
        _ => None,
    };
    if let Some(note) = note {
        out["note"] = json!(note);
    }
    // The app's warning for an older MCP says what the note says.
    if stuck && let Some(out) = out.as_object_mut() {
        out.remove("warning");
    }
    out
}

/// Seconds of the user's quiet, during a Takeover, after which a wait that
/// timed out says they may be done.
const IDLE_HINT_SECONDS: u64 = 15;

/// What a wait for control came to, from the app's resolution: how the
/// request it waited on ended, or which one still waits after a timeout.
fn describe_wait(reply: &Object) -> Value {
    let (resolution, request) = resolution(reply);
    let mut out = wait_outcome(reply, resolution, request);
    let idle = reply.get("userIdleSeconds").and_then(Value::as_u64);
    if resolution == "timedOut"
        && let Some(idle) = idle
    {
        if idle >= IDLE_HINT_SECONDS {
            let quiet = format!(
                "The user still has control, but hasn't touched the mouse or keyboard for {idle} s, so they may be done and only missed the hand-back. Ask them in the chat to click Let ... Continue at the top of the Viewer, or to tell you to continue; then call request_control with their words as user_instruction."
            );
            // The plain note says the user is still operating the display,
            // which their quiet contradicts: it goes. A note about an open
            // request says how to withdraw it: it stays.
            let asked = request.is_some();
            let base = out.get("note").and_then(Value::as_str).unwrap_or_default();
            out["note"] = json!(if asked {
                format!("{base} {quiet}")
            } else {
                quiet
            });
        }
        out["userIdleSeconds"] = json!(idle);
    }
    out
}

/// The reply's resolution and the request it names. An app built before
/// resolutions answers with `outcome` and two flags only.
fn resolution(reply: &Object) -> (&str, Option<&str>) {
    if let Some(resolution) = reply.get("resolution").and_then(Value::as_str) {
        return (resolution, reply.get("request").and_then(Value::as_str));
    }
    let flag = |key: &str| reply.get(key) == Some(&Value::Bool(true));
    let request = if flag("controlAsked") {
        Some("control")
    } else if flag("helpOpen") {
        Some("help")
    } else {
        None
    };
    match reply.get("outcome").and_then(Value::as_str) {
        Some("returned") => ("handedBack", None),
        Some("kept") => ("kept", Some("control")),
        Some("timedOut") => ("timedOut", request),
        Some("closedByUser") => ("closedByUser", None),
        _ => ("ended", None),
    }
}

fn wait_outcome(reply: &Object, resolution: &str, request: Option<&str>) -> Value {
    match (resolution, request) {
        ("continue", _) => {
            json!({ "control": "agent", "answer": "continue", "note": "The user let you continue without taking over. They may have done the step themselves: take a screenshot before you go on." })
        }
        ("instructions", _) => {
            let text = reply
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({
                "control": "agent",
                "answer": "instructions",
                "user_reply": text,
                "note": format!("The user replied instead of taking over. Their words, exactly as typed: \"{text}\". Follow them as the user's instruction for this step, and take a screenshot before you go on.")
            })
        }
        ("handedOver", _) => {
            let window = reply.get("window").cloned().unwrap_or(Value::Null);
            let app = window
                .get("app")
                .and_then(Value::as_str)
                .filter(|a| !a.is_empty())
                .unwrap_or("a");
            let title = window
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = window.get("windowId").cloned().unwrap_or(Value::Null);
            let named = if title.is_empty() {
                format!("their {app} window")
            } else {
                format!("their {app} window \"{title}\"")
            };
            json!({
                "control": "agent",
                "answer": "handedOver",
                "window": { "window_id": id, "app": app, "title": title },
                "note": format!("The user handed over {named} instead of taking over; it is on your display as window {id}. Work in it, and take a screenshot first.")
            })
        }
        ("tookOver", _) => {
            let mut out = json!({ "control": "agent", "answer": "tookOver", "note": "The user took over and handed the display back. Take a screenshot before you continue: they may have changed things." });
            // An answer the agent hadn't heard when the user took over:
            // their words or window still count.
            if let Some(text) = reply.get("text").and_then(Value::as_str) {
                out["user_reply"] = json!(text);
                out["note"] = json!(format!(
                    "The user replied, then took over and handed the display back. Their words, exactly as typed: \"{text}\". Follow them as the user's instruction, and take a screenshot before you continue: they may have changed things."
                ));
            } else if let Some(window) = reply.get("window") {
                let app = window.get("app").and_then(Value::as_str).unwrap_or("a");
                let id = window.get("windowId").cloned().unwrap_or(Value::Null);
                out["window"] = json!({ "window_id": id, "app": app, "title": window.get("title").cloned().unwrap_or(Value::Null) });
                out["note"] = json!(format!(
                    "The user handed over their {app} window, window {id}, then took over and handed the display back. Take a screenshot before you continue: they may have changed things."
                ));
            }
            out
        }
        ("handedBack", _) => {
            json!({ "control": "agent", "note": "You have control. Take a screenshot before you continue." })
        }
        ("withdrawn", _) => {
            json!({ "control": "agent", "withdrawn": true, "note": "Your Help Request was withdrawn before the user answered it; nobody stepped in. Carry on yourself, or call request_help again if you still need them." })
        }
        ("kept", _) => {
            json!({ "control": "user", "kept": true, "note": "The user chose to keep control of the display. Don't ask again right away; call wait_for_control to wait until they hand it back." })
        }
        ("timedOut", Some("control")) => {
            json!({ "control": "user", "note": "The user hasn't answered your request to hand the display back; it stays open. Call wait_for_control to keep waiting, or request_control with withdraw: true if you no longer need the display." })
        }
        ("timedOut", Some("help")) => {
            json!({ "control": "user", "note": "The user hasn't answered your Help Request yet; it stays open. Call wait_for_control to keep waiting, or request_help with withdraw: true if you no longer need them." })
        }
        ("timedOut", _) => {
            json!({ "control": "user", "note": "The user is still operating the display. Wait again or do something else." })
        }
        ("closedByUser", _) => {
            json!({ "control": "none", "note": "The user closed the display, so stop working on screen. Ask the user before you call create_display again." })
        }
        _ => {
            // Why, from an app that says (#326).
            let note = match reply.get("endedMessage").and_then(Value::as_str) {
                Some(why) => format!("{why}."),
                None => "The display has ended.".into(),
            };
            json!({ "control": "none", "note": note })
        }
    }
}

/// act_on_element's answer when the action took its own window away: the
/// windows left, and the notes of both replies. The list reply carries a
/// window the user gave to another session (ADR 0017), and only once.
fn closed_window_text(window: i64, list: &Object, shot: &Object) -> String {
    format!(
        "Done; the action closed window {window}. Windows on the display now: {}{}{}",
        list.get("windows").cloned().unwrap_or(Value::Null),
        strayed_note(list),
        strayed_note(shot)
    )
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
    // Windows the user gave from this display to another agent session
    // since the agent last heard (ADR 0017).
    for w in result
        .get("given")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let app = w.get("app").and_then(Value::as_str).unwrap_or("app");
        let id = w.get("windowId").cloned().unwrap_or(Value::Null);
        note.push_str(&format!(
            " The user gave {app} to another agent session: window {id} is no longer on this display."
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

fn scope_arg(a: &Args<'_>) -> Result<Scope, ControlError> {
    let root = match a.0.get("root") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| ControlError::invalid("root must be a non-negative integer"))?,
        ),
    };
    Ok(Scope {
        root,
        max_depth: max_depth_arg(a)?,
    })
}

/// One window's state from the app, and the tree it had before.
struct WindowState {
    window: i64,
    root: Option<usize>,
    depth_limited: bool,
    /// The app applied max_depth itself and marked its cuts; an older app
    /// read past it, so the MCP cuts the tree.
    app_cut: bool,
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
        let cut = depth
            .filter(|_| !self.app_cut)
            .map(|d| agent_screen_model::tree::cut_depth(full, d));
        let note = match (&cut, depth) {
            (_, Some(d)) if self.depth_limited => format!(
                " Deeper elements were not read; use a larger max_depth than {d}, or omit it, to read them."
            ),
            (Some(cut), Some(d)) if cut != full => format!(
                " Elements deeper than {d} levels are left out; call get_window_state with a larger max_depth, or none, to see them."
            ),
            _ => String::new(),
        };
        let tree = cut.as_deref().unwrap_or(full);
        let pictured = self.has_image();
        let mut content = vec![text(format!(
            "{} \"at (x,y)\" is an element's center in screenshot pixels, where the computer tool clicks{}.{note}{}\n\n{tree}",
            self.header,
            if pictured {
                "; the image shows just this window"
            } else {
                ""
            },
            self.strayed
        ))];
        if pictured {
            content.push(self.image.clone());
        }
        content
    }

    /// What changed since the window's last tree, then the window image.
    /// With no tree before, the whole state.
    fn changes(&self, depth: Option<usize>) -> Vec<Value> {
        let Some(before) = &self.before else {
            return self.content(depth);
        };
        let window = self.window;
        let scope = self
            .root
            .map(|root| format!(", subtree [{root}]"))
            .unwrap_or_default();
        let body = match agent_screen_model::tree::changes(before, &self.tree) {
            Some(changes) => format!(
                "Done. Changes in window {window}{scope} since its last tree in the same scope (+ new, ~ changed, - gone):{}\n\n{changes}",
                self.strayed
            ),
            None => format!(
                "Done. Window {window}{scope}'s elements didn't change.{}",
                self.strayed
            ),
        };
        let mut content = vec![text(body)];
        if self.has_image() {
            content.push(self.image.clone());
        }
        content
    }

    /// Whether the app sent the window image: not when asked to leave it out.
    fn has_image(&self) -> bool {
        self.image["data"].is_string()
    }
}

/// What wait_for_change came to, for the agent: the outcome's name first,
/// then the elements, in act_on_element's changes form.
fn wait_output(display: i64, window: i64, root: Option<usize>, reply: &Object) -> String {
    let waited = reply
        .get("waitedSeconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let scope = root
        .map(|root| format!(", subtree [{root}]"))
        .unwrap_or_default();
    let since = if reply.get("baselineFromAgent") == Some(&Value::Bool(true)) {
        "since its last tree you got in the same scope"
    } else {
        "since the call started"
    };
    let takeover = if reply.get("takeover") == Some(&Value::Bool(true)) {
        " The user is operating the display, so these changes may be theirs."
    } else {
        ""
    };
    let changes = reply.get("changes").and_then(Value::as_str);
    let mut cut = String::new();
    if reply.get("truncated") == Some(&Value::Bool(true)) {
        cut.push_str(&format!(
            " A read stopped at its budget of {} elements, so elements past it are not watched and none counts as gone; pass root to watch the part you need.",
            agent_screen_model::tree::MAX_ELEMENTS
        ));
    }
    if reply.get("depthLimited") == Some(&Value::Bool(true)) {
        cut.push_str(" Elements deeper than max_depth are not watched.");
    }
    match reply.get("outcome").and_then(Value::as_str) {
        Some("fired") => {
            let fired: Vec<&str> = reply
                .get("fired")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let matched: Vec<&str> = reply
                .get("matched")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let matched = if matched.is_empty() {
                String::new()
            } else {
                format!(" Matching: {}.", matched.join("; "))
            };
            format!(
                "{} after {waited} s in window {window}{scope}.{matched}{takeover}{cut} Changes {since} (+ new, ~ changed, - gone):\n\n{}",
                fired.join(" and "),
                changes.unwrap_or_default()
            )
        }
        Some("timedOut") => {
            let elements = match reply.get("elements").and_then(Value::as_u64) {
                Some(n) => format!(" The window has {n} elements."),
                None => " The app answered no read of the window's accessibility tree: it may be busy, or Accessibility is off.".into(),
            };
            let unanswered = match reply.get("unanswered").and_then(Value::as_u64) {
                Some(n) if n > 0 && reply.get("elements").is_some_and(|e| !e.is_null()) => {
                    format!(" {n} reads got no answer from the app.")
                }
                _ => String::new(),
            };
            let absent = if reply.get("disappearsAbsent") == Some(&Value::Bool(true)) {
                " No element matched disappears in the tree it compared with, so nothing could disappear."
            } else {
                ""
            };
            let rest = match changes {
                Some(changes) => format!(
                    " These changes {since} matched no condition (+ new, ~ changed, - gone):\n\n{}",
                    agent_screen_model::watch::first_lines(
                        changes,
                        agent_screen_model::watch::SUMMARY_LINES
                    )
                ),
                None if reply.get("elements").is_some_and(|e| !e.is_null()) => {
                    format!(" Nothing changed {since}.")
                }
                None => String::new(),
            };
            format!(
                "timed_out after {waited} s in window {window}{scope}: no condition fired.{elements}{unanswered}{absent}{takeover}{cut}{rest}"
            )
        }
        Some("windowClosed") => format!(
            "window_closed after {waited} s: window {window} closed during the wait. See list_windows."
        ),
        // Still open, but out of sight: never reported as closed.
        Some(outcome @ ("windowOnOtherSpace" | "windowMinimized" | "windowHidden")) => {
            let word = match outcome {
                "windowOnOtherSpace" => "window_on_other_space",
                "windowMinimized" => "window_minimized",
                _ => "window_hidden",
            };
            let advice = agent_screen_model::presence::Presence::of_outcome(outcome)
                .and_then(|p| p.advice(window, display as u32))
                .unwrap_or_default();
            format!("{word} after {waited} s: {advice}.")
        }
        Some("windowLeft") => format!(
            "window_left after {waited} s: window {window} moved off display {display} and shows elsewhere. See list_windows with strays: true."
        ),
        Some("rootGone") => format!(
            "root_gone after {waited} s: element [{}] left window {window}. Call get_window_state without root to find the current tree.",
            root.unwrap_or_default()
        ),
        // Why, from an app that says (#326).
        Some("displayEnded")
            if let Some(why) = reply.get("endedMessage").and_then(Value::as_str) =>
        {
            format!("display_ended after {waited} s: {why}.")
        }
        Some("displayEnded") if reply.get("closedByUser") == Some(&Value::Bool(true)) => format!(
            "display_ended after {waited} s: the user closed display {display}, so stop working on screen. Ask the user before you call create_display again."
        ),
        Some("displayEnded") => format!(
            "display_ended after {waited} s: display {display} has ended. Call create_display for a new one."
        ),
        other => format!("The app answered an unknown outcome {other:?}; update Agent Screen."),
    }
}

async fn window_state_reply(
    control: &Control,
    display: i64,
    window: i64,
    scope: Scope,
    image: bool,
) -> Result<Object, ControlError> {
    let mut params = json!({
        "displayId": display,
        "windowId": window,
        "root": scope.root,
        "maxDepth": scope.max_depth
    });
    if !image {
        params["image"] = json!(false);
    }
    let state = control.call("windowState", params).await?;
    if scope.root.is_some() && state.get("root") != Some(&json!(scope.root)) {
        return Err(ControlError::new(
            "failed",
            "the app did not confirm the requested subtree root; update Agent Screen before using root",
        ));
    }
    Ok(state)
}

async fn window_state(
    control: &Control,
    display: i64,
    window: i64,
    scope: Scope,
    want_image: bool,
) -> Result<WindowState, ControlError> {
    let state = window_state_reply(control, display, window, scope, want_image).await?;
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
    let root_note = scope
        .root
        .map(|root| format!(" Subtree root [{root}]."))
        .unwrap_or_default();
    // An app older than root and max_depth ignores both and reads the
    // whole budget; only the MCP's cut hides the deeper lines.
    let old_app = if scope.max_depth.is_some() && state.get("depthLimited").is_none() {
        " This Agent Screen app predates max_depth's traversal limit, so a long list can still use up the read; update Agent Screen."
    } else {
        ""
    };
    Ok(WindowState {
        window,
        root: scope.root,
        depth_limited: get("depthLimited") == Value::Bool(true),
        app_cut: state.contains_key("depthLimited"),
        header: format!(
            "Window {window}: {} {} elements{truncated}.{root_note}{old_app}",
            get("window"),
            get("elements")
        ),
        strayed: strayed_note(&state),
        before: control.swap_tree(
            display,
            window,
            scope,
            tree.clone(),
            get("truncated") == Value::Bool(true),
        ),
        tree,
        image: image(&get("data"), &get("mimeType")),
    })
}

/// computer's duration for `action`: 1 s by default, refused past `max`
/// instead of cut short without a word.
fn duration(a: &Args<'_>, action: &str, max: f64) -> Result<f64, ControlError> {
    let seconds = a.number("duration")?.unwrap_or(1.0);
    if !(0.0..=max).contains(&seconds) {
        let more = if action == "wait" {
            "; call wait again to wait longer"
        } else {
            ""
        };
        return Err(ControlError::invalid(format!(
            "duration for {action} is 0 to {max} seconds{more}"
        )));
    }
    Ok(seconds)
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
    let real_input = a.bool("real_input")?.unwrap_or(false);
    if real_input && matches!(action.as_str(), "right_click" | "middle_click") {
        return Err(ControlError::invalid(
            agent_screen_model::real_click::NO_MENU_CLICK,
        ));
    }
    if real_input && !agent_screen_model::real_click::takes_real_input(&action) {
        return Err(ControlError::invalid(
            "real_input applies to left_click, double_click, triple_click, left_click_drag and left_mouse_down only",
        ));
    }
    let mut waited = None;
    let mut route = None;
    let coordinate = a.coordinate("coordinate")?;
    let text_arg = a.string("text")?;
    let via_input_method = a.flag("via_input_method");
    let input_source = a.string("input_source")?;
    let modifiers: Vec<String> = text_arg
        .as_deref()
        .unwrap_or_default()
        .split('+')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();

    // What the app said about how an action went in, as beside the user's
    // window in their front app (#110).
    let mut note = None;
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
            let result = control
                .call(
                    "click",
                    json!({ "displayId": display, "x": x, "y": y, "button": button, "count": count, "modifiers": modifiers, "realInput": real_input }),
                )
                .await?;
            note = result.get("note").cloned();
            route = result
                .get("route")
                .and_then(Value::as_str)
                .map(String::from);
            waited = result.get("waitedSeconds").and_then(Value::as_f64);
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
            let result = control
                .call(
                    "drag",
                    json!({ "displayId": display, "fromX": from_x, "fromY": from_y, "toX": to_x, "toY": to_y, "modifiers": modifiers, "realInput": real_input }),
                )
                .await?;
            route = result
                .get("route")
                .and_then(Value::as_str)
                .map(String::from);
        }
        "left_mouse_down" | "left_mouse_up" => {
            let method = if action == "left_mouse_down" {
                "mouseDown"
            } else {
                "mouseUp"
            };
            let mut params =
                json!({ "displayId": display, "modifiers": modifiers, "realInput": real_input });
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
                        "seconds": duration(a, "hold_key", 10.0)?
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
            note = control
                .call(
                    "type",
                    json!({
                        "displayId": display,
                        "text": require_text(text_arg, "type")?,
                        "viaInputMethod": via_input_method,
                        "inputSource": input_source,
                    }),
                )
                .await?
                .get("note")
                .cloned();
        }
        "key" => {
            note = control
                .call(
                    "key",
                    json!({
                        "displayId": display,
                        "keys": require_text(text_arg, "key")?,
                        "viaInputMethod": via_input_method,
                        "inputSource": input_source,
                    }),
                )
                .await?
                .get("note")
                .cloned();
        }
        _ => {
            // wait
            let seconds = duration(a, "wait", 30.0)?;
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
            return Ok(vec![text(format!("Waited {seconds} s."))]);
        }
    }

    // After an action the app waits until the display stops changing, so
    // the screenshot shows the result: no fixed delay, up to 1.5 s. Without
    // a screenshot it still waits, so the next call sees the result too.
    let settle = action != "screenshot";
    let want_shot = !settle || a.bool("screenshot")?.unwrap_or(true);
    let shot = control
        .call(
            if want_shot { "screenshot" } else { "settle" },
            json!({ "displayId": display, "settle": settle, "news": true }),
        )
        .await?;
    let get = |key: &str| shot.get(key).cloned().unwrap_or(Value::Null);
    let mut header = if want_shot {
        format!(
            "{}×{} screenshot of display {display}.",
            get("width"),
            get("height")
        )
    } else {
        format!("Done on display {display}, without a screenshot.")
    };
    if let Some(route) = route {
        header.push_str(&format!(" Input route: {route}."));
    }
    if let Some(waited) = waited {
        header.push_str(&format!(
            " The request waited {waited} s for the user's answer."
        ));
    }
    header.push_str(&strayed_note(&shot));
    if let Some(note) = note.as_ref().and_then(Value::as_str) {
        header.push(' ');
        header.push_str(note);
    }
    if !want_shot {
        return Ok(vec![text(header)]);
    }
    Ok(vec![text(header), image(&get("data"), &get("mimeType"))])
}

/// What end_display tells the agent, from the display's windows as it
/// ended: which went back to the user and which wait for their review.
fn end_output(list: &Object) -> Value {
    let windows = list
        .get("windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (returned, review): (Vec<Value>, Vec<Value>) = windows
        .into_iter()
        .filter(|w| w.get("offDisplay") != Some(&Value::Bool(true)))
        .partition(|w| w["origin"] == "handedOver");
    let returned: Vec<Value> = returned.iter().map(|w| w["windowId"].clone()).collect();
    let review: Vec<Value> = review
        .iter()
        .map(|w| {
            json!({
                "window_id": w["windowId"],
                "app": w.get("app").cloned().unwrap_or(Value::Null),
                "disposition": w["disposition"],
            })
        })
        .collect();
    let mut note = if review.is_empty() {
        "The display closed: none of your windows was left on it.".to_string()
    } else {
        let unset = review.iter().filter(|w| w["disposition"].is_null()).count();
        let mut note = format!(
            "The display waits for the user to review its {} window{}.",
            review.len(),
            if review.len() == 1 { "" } else { "s" }
        );
        if unset > 0 {
            let them = if unset == 1 { "it" } else { "them" };
            note.push_str(&format!(
                " {unset} had no disposition, so the user decides {them} without your verdict."
            ));
        }
        note
    };
    if !returned.is_empty() {
        note.push_str(" The windows the user handed over go back to them.");
    }
    json!({ "ended": true, "review": review, "returned": returned, "note": note })
}

/// What launch_app returns: the window, or why there is none and what to
/// do next. `wasRunning` says whether the app was already running.
/// launch_app's note for a window in the user's own process of a browser
/// that refuses a separate instance for agents (ADR 0022).
const SHARED_BROWSER: &str = "This window opened in the user's own browser process: Agent Screen runs a separate instance for agents only for browsers verified to work that way, such as Google Chrome. While the user works in another window of this browser, your clicks and keys here go through accessibility only, which most web pages ignore, so your work in this window may stop until the user switches to another app. For long unattended web work, launch Google Chrome.";

fn launch_output(reply: &Value) -> Value {
    let mut out = launch_reason(reply);
    if reply.get("shared") == Some(&Value::Bool(true)) {
        out["shared"] = json!(true);
        out["note"] = json!(match out.get("note").and_then(Value::as_str) {
            Some(note) => format!("{note} {SHARED_BROWSER}"),
            None => SHARED_BROWSER.to_string(),
        });
    }
    out
}

fn launch_reason(reply: &Value) -> Value {
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
            Some("display_window") => json!({
                "window": window,
                "wasRunning": true,
                "reason": "display_window",
                "note": "The app can't open a new window, so the URL opened in its window already on your display. Take a screenshot before you act.",
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
                    "The app was already running and its menus have no command that opens a plain new window, as in Calculator or Notes; nothing was created or moved. Ask the user to quit it and call launch_app again, or to hand its window over with place_window. Once all its windows are on your display, launch_app with open opens a URL in its window."
                }
                "menus_unreadable" => {
                    "The app was already running and Agent Screen couldn't read or use its menus, so it didn't ask for a window; nothing moved. Ask the user to quit it and call launch_app again, or to hand its window over with place_window. Once all its windows are on your display, launch_app with open opens a URL in its window."
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
    #[test]
    fn end_output_says_what_waits_for_review_and_what_went_back() {
        let list = |windows: Value| -> Object {
            json!({ "windows": windows, "scale": 1.0 })
                .as_object()
                .unwrap()
                .clone()
        };
        let out = end_output(&list(json!([
            { "windowId": 1, "app": "Safari", "origin": "launched", "disposition": "keep" },
            { "windowId": 2, "app": "TextEdit", "origin": "placed", "disposition": null },
            { "windowId": 3, "app": "Notes", "origin": "handedOver", "disposition": null },
            { "windowId": 4, "app": "Safari", "origin": "launched", "offDisplay": true }
        ])));
        assert_eq!(out["returned"], json!([3]));
        assert_eq!(out["review"].as_array().unwrap().len(), 2, "{out}");
        assert_eq!(out["review"][0]["window_id"], 1);
        let note = out["note"].as_str().unwrap();
        assert!(note.contains("review its 2 windows"), "{note}");
        assert!(note.contains("1 had no disposition"), "{note}");
        assert!(note.contains("go back to them"), "{note}");
        let empty = end_output(&list(json!([])));
        assert_eq!(empty["review"], json!([]));
        assert!(
            empty["note"].as_str().unwrap().contains("closed"),
            "{empty}"
        );
    }

    #[test]
    fn durations_past_their_limit_fail_with_it() {
        let args = |d: Value| json!({ "duration": d }).as_object().unwrap().clone();
        let long = args(json!(45));
        let error = duration(&Args(&long), "wait", 30.0).unwrap_err();
        assert!(
            error.message.contains("0 to 30 seconds"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("call wait again"),
            "{}",
            error.message
        );
        let held = args(json!(12));
        assert!(duration(&Args(&held), "hold_key", 10.0).is_err());
        let fine = args(json!(2.5));
        assert_eq!(duration(&Args(&fine), "wait", 30.0).unwrap(), 2.5);
        let none = Object::new();
        assert_eq!(duration(&Args(&none), "wait", 30.0).unwrap(), 1.0);
    }

    #[test]
    fn hints_say_which_tools_reach_beyond_the_mac_and_which_only_read() {
        let tools = definitions();
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            let open = tool["annotations"]["openWorldHint"] == true;
            assert_eq!(
                open,
                matches!(name, "launch_app" | "report_issue"),
                "{name}"
            );
        }
        assert!(read_only("list_windows") && read_only("get_window_state"));
        assert!(!read_only("computer") && !read_only("launch_app"));
        assert!(!read_only("no_such_tool"));
    }

    use super::*;

    fn state(before: Option<&str>, tree: &str) -> WindowState {
        WindowState {
            window: 7,
            root: None,
            depth_limited: false,
            app_cut: false,
            header: "Window 7: {} 3 elements.".into(),
            strayed: String::new(),
            tree: tree.into(),
            before: before.map(String::from),
            image: json!({ "type": "image", "data": "AAAA" }),
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
    fn a_tree_the_app_cut_keeps_its_marks() {
        let tree = "[0] group \"List\"\n  … more inside: 800 children not read\n[1] group \"Pane\"\n  … more inside: 1 child not read";
        let mut cut = state(None, tree);
        cut.app_cut = true;
        cut.depth_limited = true;
        let text = first_text(&cut.content(Some(0))).to_string();
        assert!(text.ends_with(tree), "{text}");
        assert!(text.contains("Deeper elements were not read"), "{text}");
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
    fn wait_output_names_what_fired_and_lists_changes() {
        let reply = |v: Value| v.as_object().unwrap().clone();
        let fired = wait_output(
            3,
            5,
            None,
            &reply(json!({
                "outcome": "fired", "fired": ["appeared", "disappeared"],
                "matched": ["[9] statictext value=\"Delivered\"", "[7] statictext value=\"Sending\""],
                "changes": "+ [9] statictext value=\"Delivered\"\n- [7] statictext value=\"Sending\"",
                "baselineFromAgent": true, "waitedSeconds": 1.6
            })),
        );
        assert!(
            fired.starts_with("appeared and disappeared after 1.6 s in window 5."),
            "{fired}"
        );
        assert!(fired.contains("since its last tree you got"), "{fired}");
        assert!(
            fired.ends_with(
                "\n\n+ [9] statictext value=\"Delivered\"\n- [7] statictext value=\"Sending\""
            ),
            "{fired}"
        );
        assert!(!fired.contains("operating"), "{fired}");
        let mine = wait_output(
            3,
            5,
            Some(4),
            &reply(
                json!({ "outcome": "fired", "fired": ["changed"], "changes": "~ [6] row", "takeover": true, "waitedSeconds": 0.5 }),
            ),
        );
        assert!(
            mine.starts_with("changed after 0.5 s in window 5, subtree [4]."),
            "{mine}"
        );
        assert!(
            mine.contains("since the call started") && mine.contains("may be theirs"),
            "{mine}"
        );
    }

    #[test]
    fn wait_output_summarizes_a_timeout_and_says_why_nothing_could_fire() {
        let reply = |v: Value| v.as_object().unwrap().clone();
        let quiet = wait_output(
            3,
            5,
            None,
            &reply(
                json!({ "outcome": "timedOut", "elements": 12, "baselineFromAgent": false, "waitedSeconds": 2.0 }),
            ),
        );
        assert!(
            !quiet.contains("budget") && !quiet.contains("max_depth"),
            "{quiet}"
        );
        let cut = wait_output(
            3,
            5,
            None,
            &reply(
                json!({ "outcome": "fired", "fired": ["appeared"], "changes": "+ [500] row", "truncated": true, "depthLimited": true }),
            ),
        );
        assert!(
            cut.contains("budget of 400 elements") && cut.contains("pass root"),
            "{cut}"
        );
        assert!(cut.contains("deeper than max_depth"), "{cut}");
        assert!(
            quiet.starts_with("timed_out after 2 s in window 5: no condition fired."),
            "{quiet}"
        );
        assert!(
            quiet.contains("12 elements")
                && quiet.ends_with("Nothing changed since the call started."),
            "{quiet}"
        );
        let many = (0..30)
            .map(|n| format!("~ [{n}] row"))
            .collect::<Vec<_>>()
            .join("\n");
        let busy = wait_output(
            3,
            5,
            None,
            &reply(
                json!({ "outcome": "timedOut", "elements": 40, "changes": many, "disappearsAbsent": true, "waitedSeconds": 30.0 }),
            ),
        );
        assert!(
            busy.contains("matched no condition") && busy.ends_with("… and 10 more"),
            "{busy}"
        );
        assert!(busy.contains("nothing could disappear"), "{busy}");
        let silent = wait_output(
            3,
            5,
            None,
            &reply(json!({ "outcome": "timedOut", "elements": null, "unanswered": 4 })),
        );
        assert!(silent.contains("answered no read"), "{silent}");
        for (outcome, start) in [
            (json!({ "outcome": "windowClosed" }), "window_closed"),
            (json!({ "outcome": "windowLeft" }), "window_left"),
            (
                json!({ "outcome": "windowOnOtherSpace" }),
                "window_on_other_space",
            ),
            (json!({ "outcome": "windowMinimized" }), "window_minimized"),
            (json!({ "outcome": "windowHidden" }), "window_hidden"),
            (json!({ "outcome": "rootGone" }), "root_gone"),
            (json!({ "outcome": "displayEnded" }), "display_ended"),
        ] {
            assert!(wait_output(3, 5, None, &reply(outcome)).starts_with(start));
        }
        let closed = wait_output(
            3,
            5,
            None,
            &reply(json!({ "outcome": "displayEnded", "closedByUser": true })),
        );
        assert!(closed.contains("the user closed display 3"), "{closed}");
        // An app that says why: the agent reads its words.
        let why = wait_output(
            3,
            5,
            None,
            &reply(json!({
                "outcome": "displayEnded",
                "endedReason": "end_display",
                "endedMessage": "display 3 has ended: this session ended it with end_display. Call create_display for a new one",
            })),
        );
        assert!(
            why.starts_with("display_ended after 0 s: display 3 has ended: this session ended it"),
            "{why}"
        );
        // A window out of sight is never called closed, and the agent is
        // told what to ask the user.
        let space = wait_output(
            3,
            5,
            None,
            &reply(json!({ "outcome": "windowOnOtherSpace" })),
        );
        assert!(
            space.contains("still open") && space.contains("doesn't switch Spaces"),
            "{space}"
        );
        assert!(!space.contains("closed"), "{space}");
        let hidden = wait_output(3, 5, None, &reply(json!({ "outcome": "windowHidden" })));
        assert!(hidden.contains("Ask the user to show the app"), "{hidden}");
    }

    #[test]
    fn a_closed_window_answer_keeps_the_given_note() {
        let list = json!({ "windows": [], "given": [{ "windowId": 12, "app": "WeChat" }] });
        let text = closed_window_text(5, list.as_object().unwrap(), json!({}).as_object().unwrap());
        assert!(text.contains("closed window 5"), "{text}");
        assert!(
            text.contains("The user gave WeChat to another agent session: window 12"),
            "{text}"
        );
    }

    #[test]
    fn a_help_request_says_how_the_user_answered() {
        let wait = |reply: Value| describe_wait(reply.as_object().unwrap());
        let note = |out: &Value| out["note"].as_str().unwrap().to_string();

        let go_on = wait(json!({ "resolution": "continue", "request": "help" }));
        assert_eq!(
            (go_on["control"].as_str(), go_on["answer"].as_str()),
            (Some("agent"), Some("continue"))
        );
        assert!(
            note(&go_on).contains("let you continue without taking over"),
            "{go_on}"
        );

        let words = "Use the work account,\nthen pick seat 12A ";
        let replied =
            wait(json!({ "resolution": "instructions", "request": "help", "text": words }));
        assert_eq!(replied["user_reply"], words, "the user's words, as typed");
        assert!(
            note(&replied).contains(&format!("\"{words}\"")),
            "{replied}"
        );
        assert!(note(&replied).contains("user's instruction"), "{replied}");

        let given = wait(json!({
            "resolution": "handedOver", "request": "help",
            "window": { "windowId": 31, "app": "Safari", "title": "United Sign In" }
        }));
        assert_eq!(given["window"]["window_id"], 31);
        assert!(
            note(&given).contains("their Safari window \"United Sign In\"")
                && note(&given).contains("on your display"),
            "{given}"
        );

        let took = wait(json!({ "resolution": "tookOver", "request": "help" }));
        assert!(
            note(&took).contains("took over and handed the display back"),
            "{took}"
        );
        // A Reply or Give the Takeover superseded still reaches the agent.
        let took_words = wait(json!({
            "resolution": "tookOver", "request": "help",
            "superseded": "instructions", "text": words
        }));
        assert_eq!(took_words["user_reply"], words);
        assert!(note(&took_words).contains("then took over"), "{took_words}");
        let took_window = wait(json!({
            "resolution": "tookOver", "request": "help", "superseded": "handedOver",
            "window": { "windowId": 31, "app": "Safari", "title": "United" }
        }));
        assert_eq!(took_window["window"]["window_id"], 31);
        assert!(
            note(&took_window).contains("Safari window"),
            "{took_window}"
        );

        // The agent's own withdrawal is never the user's hand-back.
        let withdrawn = wait(json!({ "resolution": "withdrawn", "request": "help" }));
        assert!(
            !note(&withdrawn).contains("You have control"),
            "{withdrawn}"
        );
        assert_eq!(withdrawn["withdrawn"], true);

        let open = wait(json!({ "resolution": "timedOut", "request": "help" }));
        assert!(
            note(&open).contains("hasn't answered your Help Request"),
            "{open}"
        );
        let closed = wait(json!({ "resolution": "closedByUser" }));
        assert!(note(&closed).contains("closed the display"), "{closed}");
        // An ended display says why, from an app that says (#326).
        let ended = wait(json!({ "resolution": "ended" }));
        assert_eq!(note(&ended), "The display has ended.");
        let why = wait(json!({
            "resolution": "ended",
            "endedReason": "end_display",
            "endedMessage": "display 4 has ended: this session ended it with end_display. Call create_display for a new one",
        }));
        assert!(
            note(&why).starts_with("display 4 has ended: this session ended it"),
            "{why}"
        );
        // In a Takeover the user is still at the display.
        let busy = wait(json!({ "resolution": "timedOut", "request": null }));
        assert!(note(&busy).contains("still operating"), "{busy}");

        // An app built before resolutions still reads right.
        let old = wait(json!({ "outcome": "timedOut", "helpOpen": true, "controlAsked": false }));
        assert!(
            note(&old).contains("hasn't answered your Help Request"),
            "{old}"
        );
        let old = wait(json!({ "outcome": "returned" }));
        assert!(note(&old).contains("You have control"), "{old}");
    }

    #[test]
    fn a_timed_out_wait_says_when_the_user_has_gone_quiet() {
        let waiting = |idle: Value, request: Value| {
            json!({ "resolution": "timedOut", "request": request, "outcome": "timedOut", "userIdleSeconds": idle })
                .as_object()
                .unwrap()
                .clone()
        };
        // During a Takeover no Help Request is open, as in the passkey case.
        for helping in [false, true] {
            let request = if helping { json!("help") } else { Value::Null };
            let quiet = describe_wait(&waiting(json!(42), request));
            let note = quiet["note"].as_str().unwrap();
            assert!(
                note.contains("for 42 s")
                    && note.contains("Continue")
                    && note.contains("user_instruction")
                    && !note.contains("still operating"),
                "{note}"
            );
            assert_eq!(quiet["userIdleSeconds"], 42);
            // An open Help Request's note keeps how to withdraw it.
            assert_eq!(note.contains("withdraw: true"), helping, "{note}");
        }
        let asked = waiting(json!(42), json!("control"));
        let note = describe_wait(&asked)["note"].as_str().unwrap().to_string();
        assert!(
            note.contains("request_control with withdraw: true") && note.contains("for 42 s"),
            "{note}"
        );
        let busy = describe_wait(&waiting(json!(3), json!("help")));
        assert!(!busy["note"].as_str().unwrap().contains("touched"));
        assert_eq!(busy["userIdleSeconds"], 3);
        let none = describe_wait(&waiting(Value::Null, json!("help")));
        assert!(none.get("userIdleSeconds").is_none());
    }

    #[test]
    fn wait_for_change_says_its_default_and_cap() {
        let tools = definitions();
        let wait = tools
            .iter()
            .find(|t| t["name"] == "wait_for_change")
            .unwrap();
        let description = wait["description"].as_str().unwrap();
        assert!(
            description.contains("30 by default and at most 120"),
            "{description}"
        );
        assert!(
            description.contains("(+ new, ~ changed, - gone)"),
            "{description}"
        );
        let props = &wait["inputSchema"]["properties"];
        assert_eq!(props["appears"]["additionalProperties"], false);
        assert!(props["root"].is_object() && props["max_depth"].is_object());
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
        // A top-strip constraint can shrink an omitted height while the
        // explicitly requested width still matches. It must still be disclosed.
        let constrained = place_output(
            json!({ "window": { "x": 0, "y": 31, "width": 600, "height": 817 }, "frameConstrained": true }).as_object().unwrap(),
            &[("x", None), ("y", None), ("width", Some(600.0)), ("height", None)],
        );
        assert!(
            constrained["note"]
                .as_str()
                .unwrap_or_default()
                .contains("reserved menu bar strip")
        );
        // Another session's window the user gave.
        let taken = place_output(
            json!({ "takenFrom": "Claude Code · task B" })
                .as_object()
                .unwrap(),
            &[("x", None), ("y", None), ("width", None), ("height", None)],
        );
        assert_eq!(taken["taken_from"], "Claude Code · task B");
        assert!(
            taken["note"]
                .as_str()
                .unwrap()
                .contains("Claude Code · task B was using")
        );
        // A handed-over window says which it is, and whether it came from
        // the Dock (#221).
        let handed = place_output(
            json!({ "handedOver": true, "windowId": 72281, "title": "Field", "fromDock": true, "by": "chat" })
                .as_object()
                .unwrap(),
            &[("x", None), ("y", None), ("width", None), ("height", None)],
        );
        assert_eq!(handed["windowId"], 72281);
        assert_eq!(handed["title"], "Field");
        assert_eq!(handed["fromDock"], true);
        assert!(handed.get("by").is_none());
        // A window the Autonomy Mode took says so (ADR 0025).
        let by_mode = place_output(
            json!({ "handedOver": true, "by": "mode", "authority": "mode", "autonomyMode": "auto" })
                .as_object()
                .unwrap(),
            &[("x", None), ("y", None), ("width", None), ("height", None)],
        );
        assert_eq!(by_mode["authority"], "mode");
        assert_eq!(by_mode["autonomyMode"], "auto");
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
        let held = launch_output(&json!({
            "window": { "windowId": 9 },
            "wasRunning": true,
            "reason": "display_window",
        }));
        assert_eq!(held["window"]["windowId"], 9);
        assert_eq!(held["wasRunning"], true);
        assert_eq!(held["reason"], "display_window");
        assert!(
            held["note"]
                .as_str()
                .unwrap()
                .contains("already on your display")
        );
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
            let note = none["note"].as_str().unwrap();
            assert!(note.len() > 40, "{reason}");
            if matches!(reason, "no_new_window" | "menus_unreadable") {
                assert!(note.contains("place_window"), "{reason}");
            }
        }
        let shared = launch_output(&json!({
            "window": { "windowId": 9 }, "wasRunning": true, "shared": true,
        }));
        assert_eq!(shared["shared"], true);
        let note = shared["note"].as_str().unwrap();
        assert!(note.contains("user's own browser") && note.contains("Google Chrome"));
        assert!(placed.get("shared").is_none());
    }

    #[test]
    fn eighteen_tools_with_object_schemas() {
        let tools = definitions();
        assert_eq!(tools.len(), 18);
        let diagnostics = tools
            .iter()
            .find(|t| t["name"] == "get_diagnostics")
            .unwrap();
        assert_eq!(diagnostics["annotations"]["readOnlyHint"], true);
        assert_eq!(diagnostics["inputSchema"]["properties"], json!({}));
        assert_eq!(diagnostics["inputSchema"]["required"], json!([]));
        for tool in &tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        }
        // Gemini's function-calling schema converter has refused anyOf.
        let report = tools.iter().find(|t| t["name"] == "report_issue").unwrap();
        let schema = report["inputSchema"].to_string();
        assert!(
            !schema.contains("anyOf") && !schema.contains("oneOf"),
            "{schema}"
        );
    }

    #[test]
    fn current_schema_exposes_frame_recovery_and_separate_profile_signins() {
        let tools = definitions();
        let place = tools
            .iter()
            .find(|tool| tool["name"] == "place_window")
            .unwrap();
        for key in ["x", "y", "width", "height"] {
            assert_eq!(place["inputSchema"]["properties"][key]["type"], "number");
        }
        let description = place["description"].as_str().unwrap();
        assert!(description.contains("reserved menu bar strip"));
        assert!(description.contains(
            "For a window already on this display, no frame arguments leaves its frame unchanged"
        ));
        let launch = tools
            .iter()
            .find(|tool| tool["name"] == "launch_app")
            .unwrap();
        assert!(
            launch["description"]
                .as_str()
                .unwrap()
                .contains("does not reuse the user's existing login session")
        );
    }

    #[test]
    fn only_create_display_loads_upfront() {
        let upfront: Vec<_> = definitions()
            .into_iter()
            .filter(|tool| tool["_meta"]["anthropic/alwaysLoad"] == true)
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(upfront, ["create_display"]);
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

    fn args(value: Value) -> Object {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn grant_and_return_need_the_users_words_and_a_target() {
        let check = |v: Value| {
            let o = args(v);
            window_access_params(&Args(&o))
        };
        let words = "你可以用我的 Safari 窗口";
        let err =
            check(json!({ "display_id": 1, "action": "grant", "apps": ["Safari"] })).unwrap_err();
        assert!(err.message.contains("user_instruction"), "{}", err.message);
        assert!(
            check(json!({ "display_id": 1, "action": "grant", "user_instruction": words }))
                .is_err()
        );
        assert!(
            check(json!({ "display_id": 1, "action": "grant", "all": true, "apps": ["Safari"], "user_instruction": words }))
                .is_err()
        );
        assert!(
            check(json!({ "display_id": 1, "action": "grant", "user_instruction": "   ", "all": true })).is_err(),
            "blank words are no words"
        );
        let grant = check(json!({ "display_id": 1, "action": "grant", "apps": [" Safari "], "user_instruction": format!(" {words} ") })).unwrap();
        assert_eq!(grant["apps"], json!(["Safari"]));
        assert_eq!(grant["userInstruction"], words);
        assert_eq!(grant["displayId"], 1);
        assert!(check(json!({ "display_id": 1, "action": "grant", "apps": [""], "user_instruction": words })).is_err());

        assert!(check(json!({ "display_id": 1, "action": "return", "all": true })).is_err());
        assert!(
            check(json!({ "display_id": 1, "action": "return", "user_instruction": "还给我" }))
                .is_err()
        );
        assert!(
            check(json!({ "display_id": 1, "action": "return", "all": true, "window_id": 4, "user_instruction": "还给我" }))
                .is_err()
        );
        let back = check(json!({ "display_id": 1, "action": "return", "window_id": 4, "user_instruction": "还给我" })).unwrap();
        assert_eq!(back["windowId"], 4);

        // Taking access back needs no words, and nothing else does either.
        assert_eq!(
            check(json!({ "display_id": 1, "action": "revoke" })).unwrap()["apps"],
            Value::Null
        );
        assert!(check(json!({ "display_id": 1, "action": "status" })).is_ok());
        assert!(check(json!({ "display_id": 1, "action": "take" })).is_err());
    }

    #[test]
    fn chat_control_says_who_has_control_and_that_nothing_was_checked() {
        let reply = |changed: bool| args(json!({ "changed": changed }));
        let back = chat_control_output(&reply(true), "agent");
        assert_eq!(back["control"], "agent");
        assert_eq!(back["handedBack"], "chat");
        assert!(back["note"].as_str().unwrap().contains("didn't check"));
        let already = chat_control_output(&reply(false), "agent");
        assert_eq!(already["control"], "agent");
        assert!(already.get("handedBack").is_none());
        let paused = chat_control_output(&reply(true), "user");
        assert_eq!(paused["control"], "user");
        assert_eq!(paused["paused"], true);
        assert!(
            paused["note"]
                .as_str()
                .unwrap()
                .contains("window access is paused")
        );
    }

    #[test]
    fn access_output_passes_the_status_through() {
        let status = args(json!({
            "scope": "apps",
            "apps": [{ "bundleId": "com.apple.Safari", "name": "Safari" }],
            "paused": false,
            "excluded": [],
            "handovers": [],
            "lastInstruction": null,
            "changed": true
        }));
        let out = access_output("grant", status.clone());
        assert_eq!(out["apps"][0]["bundleId"], "com.apple.Safari");
        assert!(out["note"].as_str().unwrap().contains("didn't check"));
        let mut none = status.clone();
        none.insert("changed".into(), json!(false));
        assert_eq!(access_output("revoke", none)["note"], "Nothing to revoke.");
        assert!(
            access_output("status", status.clone())
                .get("note")
                .is_none()
        );
        let mut stuck = status;
        stuck.insert("returned".into(), json!([]));
        stuck.insert("stuck".into(), json!([4]));
        stuck.insert("warning".into(), json!("For an older MCP."));
        let out = access_output("return", stuck);
        assert_eq!(out["stuck"], json!([4]));
        assert!(out["note"].as_str().unwrap().contains("didn't leave"));
        assert!(out.get("warning").is_none(), "said once, in the note");
    }

    #[test]
    fn chat_paths_are_only_for_the_users_explicit_words() {
        let tools = definitions();
        let find = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap().clone();
        let control = find("request_control");
        assert_eq!(
            control["inputSchema"]["properties"]["control"]["enum"],
            json!(["agent", "user"])
        );
        let description = control["description"].as_str().unwrap();
        assert!(
            description.contains("never on your own judgment"),
            "{description}"
        );
        assert!(
            description.contains("One request per display"),
            "{description}"
        );
        assert_eq!(control["inputSchema"]["required"], json!(["display_id"]));
        let help = find("request_help");
        let description = help["description"].as_str().unwrap();
        assert!(
            description.contains("only for an explicit answer the user gave in this chat"),
            "{description}"
        );
        let place = find("place_window");
        assert_eq!(
            place["inputSchema"]["properties"]["user_instruction"]["type"],
            "string"
        );
        let access = find("window_access");
        assert_eq!(
            access["inputSchema"]["properties"]["action"]["enum"],
            json!(["grant", "revoke", "return", "status"])
        );
        let text = access["description"].as_str().unwrap();
        assert!(text.contains("can't check them"), "{text}");
        assert!(text.contains("pauses on a display while the"), "{text}");
        assert!(text.contains("display_id only"), "{text}");
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

    #[test]
    fn new_against_an_app_without_multi_display_is_an_error() {
        let reply = |reused: bool, able: bool| -> Object {
            let mut reply = json!({ "displayId": 7, "reused": reused });
            if able {
                reply["multiDisplay"] = json!(true);
            }
            reply.as_object().unwrap().clone()
        };
        let error = needs_newer_app(true, &reply(true, false)).unwrap();
        assert!(
            error.message.contains("update Agent Screen"),
            "{}",
            error.message
        );
        assert!(error.message.contains("display 7"), "{}", error.message);
        // The old app made a first display: that is what new asked for.
        assert!(needs_newer_app(true, &reply(false, false)).is_none());
        assert!(needs_newer_app(true, &reply(true, true)).is_none());
        assert!(needs_newer_app(false, &reply(true, false)).is_none());
    }

    #[test]
    fn create_display_tells_agent_to_share_preview_url() {
        let tools = definitions();
        let create = tools
            .iter()
            .find(|t| t["name"] == "create_display")
            .unwrap();
        let description = create["description"].as_str().unwrap();
        assert!(description.contains("preview_url"), "{description}");
        assert!(
            description.contains("give preview_url to the user"),
            "{description}"
        );
    }
}
