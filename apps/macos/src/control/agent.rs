//! Serves control requests from Agent Sessions. Coordinates are in points
//! relative to the display's top-left corner, the same space as screenshots.

use super::off;
use super::server::{ControlError, Reply};
use super::shared_app::{self, Beside};
use crate::core::Core;
use crate::keychord::KeyChord;
use crate::mac::devtools::{self, Route};
use crate::mac::input::{Button, Sent};
use crate::mac::screenshot::{self, Format};
use crate::mac::windows::Grab;
use crate::mac::{app, ax, cg, windows};
use crate::model::{
    ControlAsk, DEFAULT_SIZE, DisplayId, Disposition, EndReason, FrameRequest, Origin, Owner, Rect,
    Wait, WindowTarget, frame_in_shot, shot_scale,
};
use crate::numbers;
use agent_screen_model::access::{self, Authority, ChatAction, Instruction, Refusal};
use agent_screen_model::bring_out::Refusal as BringOutRefusal;
use agent_screen_model::held::{self, Held};
use agent_screen_model::input_queue::Action;
use agent_screen_model::placement::{self, AppSighting, Holder, NamedApp, PutAway, WindowSighting};
use agent_screen_model::presence::Presence;
use agent_screen_model::real_click;
use agent_screen_model::request::{Answer, HelpAsked, Kind, NamedWindow, Resolution, Woken};
use agent_screen_model::session::Orphans;
use agent_screen_model::shared_app::{
    Center, ClickEnd, ElementPress, Front, bound_click_reaches, element_press,
};
use base64::Engine;
use futures::FutureExt;
use gpui::{App, AsyncApp, Entity};
use objc2::MainThreadMarker;
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

type Params = Map<String, Value>;

struct CallGuard {
    dropped: std::sync::Arc<std::sync::Mutex<Vec<(String, u64)>>>,
    started_at: Instant,
    key: String,
    finished: bool,
}

impl CallGuard {
    fn finish(mut self, core: &Entity<Core>, cx: &mut AsyncApp) {
        self.finished = true;
        let key = std::mem::take(&mut self.key);
        core.update(cx, |c, _| {
            c.drain_dropped_calls();
            let now_ms = c.elapsed_ms();
            c.displays.sessions.note_call_end(&key, now_ms);
        });
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if !self.finished
            && !self.key.is_empty()
            && let Ok(mut g) = self.dropped.lock()
        {
            g.push((
                std::mem::take(&mut self.key),
                self.started_at.elapsed().as_millis() as u64,
            ));
        }
    }
}

pub async fn handle(
    core: Entity<Core>,
    connection: u64,
    method: &str,
    p: Params,
    cx: &mut AsyncApp,
) -> Reply {
    // A click, drag, scroll, element action or launch focuses the agent's
    // window too: hand key focus back to the user's window in a shared app
    // once it's done, as after keys.
    let pointer = matches!(
        method,
        "click" | "drag" | "mouseUp" | "scroll" | "elementAction" | "launchApp"
    )
    .then(|| Args(&p).int("displayId"))
    .flatten()
    .map(|id| id as DisplayId);
    let timeline = recorded(method, &p);
    let shown = Args(&p).int("displayId").map(|id| id as DisplayId);
    let call_guard = (!method.starts_with("debug"))
        .then(|| {
            let named_key = Args(&p).string("sessionKey").filter(|s| !s.is_empty());
            core.update(cx, |c, cx| {
                c.drain_dropped_calls();
                let now_ms = c.elapsed_ms();
                let (key, rebound) =
                    c.begin_session_call(connection, named_key.as_deref(), shown, now_ms);
                if rebound {
                    cx.notify();
                }
                let key = key?;
                Some(CallGuard {
                    dropped: c.dropped_calls.clone(),
                    started_at: c.started_at_instant(),
                    key,
                    finished: false,
                })
            })
        })
        .flatten();
    if matches!(method, "click" | "mouseDown" | "key" | "type")
        && let Some(id) = shown
    {
        // A menu the last input closed, as Return choosing its item, may
        // still be fading out, and a click then gets lost.
        let closed = CLOSED.with(|m| m.borrow_mut().remove(&id).unwrap_or_default());
        until_faded(&core, id, closed, cx).await;
        menu_still_open(id, method, cx).await?;
    }
    // The menus open at the agent's last call, for this input.
    let seen = shown
        .map(|id| MENUS.with(|m| m.borrow().get(&id).cloned().unwrap_or_default()))
        .unwrap_or_default();
    let named = Args(&p)
        .int("displayId")
        .zip(Args(&p).int("windowId"))
        .filter(|_| method != "placeWindow");
    let reply = handle_one(core.clone(), connection, method, p, cx).await;
    if let Some(guard) = call_guard {
        guard.finish(&core, cx);
    }
    // A window the user gave to another session: say so, rather than that
    // there is no such window (ADR 0017).
    let reply = match (reply, named) {
        (Err(error), Some((id, window))) if error.code == "not_found" => {
            let given = core.read_with(cx, |c, _| {
                c.displays
                    .get(id as DisplayId)
                    .and_then(|d| d.given_away(window as u32).map(String::from))
            });
            Err(match given {
                Some(app) => ControlError::not_found(held::given_message(window as u32, &app)),
                None => error,
            })
        }
        (reply, _) => reply,
    };
    let mut reply = reply;
    // The menus the agent's next input may be meant for.
    if let Some(id) = shown
        && matches!(
            method,
            "screenshot"
                | "windowState"
                | "listWindows"
                | "click"
                | "mouseDown"
                | "mouseUp"
                | "move"
                | "drag"
                | "scroll"
                | "key"
                | "holdKey"
                | "type"
                | "elementAction"
        )
    {
        // An input may have chosen an item of a menu open before it, which
        // stays on screen a moment (#276). Other calls change no menu.
        let chooses = matches!(
            method,
            "click" | "mouseUp" | "drag" | "key" | "holdKey" | "type" | "elementAction"
        );
        let waited = !chooses || reply.is_err();
        let before = seen.clone();
        let menus = off(cx, move || windows::menus_after_input(id, &before, waited)).await;
        if method == "type"
            && agent_screen_model::open_menu::typed_into_menu(&seen, &menus)
            && let Ok(object) = &mut reply
        {
            let note = match object.get("note").and_then(Value::as_str) {
                Some(note) => format!("{note} {TYPED_INTO_MENU}"),
                None => TYPED_INTO_MENU.to_string(),
            };
            object.insert("note".into(), json!(note));
        }
        if chooses {
            let closed = agent_screen_model::open_menu::closed_by(&seen, &menus);
            CLOSED.with(|m| m.borrow_mut().insert(id, closed));
        }
        MENUS.with(|m| m.borrow_mut().insert(id, menus));
    }
    if let Some((id, event)) = timeline
        && reply.is_ok()
    {
        crate::mac::recording::note(id, event);
    }
    if let Some(id) = pointer
        && let Some(forwarder) = core.read_with(cx, |c, _| c.existing_forwarder(id))
    {
        forwarder.keys_done().await;
    }
    reply
}

thread_local! {
    /// The pop-up menus open on each display after the agent's last call.
    static MENUS: std::cell::RefCell<std::collections::HashMap<DisplayId, Vec<u32>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// The pop-up menus the agent's last input on each display closed,
    /// which may still be fading out.
    static CLOSED: std::cell::RefCell<std::collections::HashMap<DisplayId, Vec<u32>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Wait until no menu the agent's last input closed (`closed`) is still
/// fading out on display `id`: a click in that time gets lost
/// (`agent_screen_model::open_menu::closing`). 400 ms at most, and not
/// in a Takeover, where the input is refused at once: control is checked
/// after every look.
async fn until_faded(core: &Entity<Core>, id: DisplayId, closed: Vec<u32>, cx: &mut AsyncApp) {
    use agent_screen_model::open_menu::{FADED_WAIT_MS, LOOK_EVERY_MS, closing};
    if closed.is_empty() {
        return;
    }
    let closed = std::sync::Arc::new(closed);
    // One deadline for all of it, slow WindowServer queries too.
    let deadline = cx
        .background_executor()
        .timer(Duration::from_millis(FADED_WAIT_MS));
    let wait = async {
        while !takeover(core, id, cx) {
            let closed = closed.clone();
            let fading = off(cx, move || closing(&closed, &windows::menus_on(id))).await;
            if !fading || takeover(core, id, cx) {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(LOOK_EVERY_MS))
                .await;
        }
    };
    futures::future::select(std::pin::pin!(wait), deadline).await;
}

/// `type`'s note when its text went to a menu open before and after it.
const TYPED_INTO_MENU: &str = "A menu was open, and still is, so the text went to the menu, not to a field: type-ahead in an open menu moves its highlight and chooses nothing. Press Return to choose the highlighted item, or Escape to close the menu.";

/// Refuse a click or key meant for a menu that closed since the agent's
/// last call. macOS ends an app's open menu at the user's next click or
/// key anywhere, their own screen included, and the agent's input would
/// land on the page under it instead (#48, #79).
async fn menu_still_open(
    id: DisplayId,
    method: &str,
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
    let seen = MENUS.with(|m| m.borrow().get(&id).cloned().unwrap_or_default());
    if seen.is_empty() {
        return Ok(());
    }
    let listed = off(cx, move || windows::menus_on(id)).await;
    if agent_screen_model::open_menu::input_may_go(&seen, &listed) {
        return Ok(());
    }
    let open = agent_screen_model::open_menu::open(&listed);
    MENUS.with(|m| m.borrow_mut().insert(id, open));
    let action = match method {
        "type" => "typing",
        "key" => "key",
        _ => "click",
    };
    Err(ControlError::failed(format!(
        "the menu open on display {id} closed before this {action}, with no choice made, so nothing was sent. macOS closes an app's open menu at the user's next click or key anywhere, on their own screen too. For a pop-up button or a web page's select, call act_on_element set_value with the item's exact full title: it chooses the item with the menu closed. Otherwise take a screenshot, open the menu again and choose its item in the next call, with act_on_element press on the item or a click on it"
    )))
}

/// The timeline line an agent action adds to a recording of its display:
/// the action and its arguments, in screenshot pixels as the agent gave them.
fn recorded(method: &str, p: &Params) -> Option<(DisplayId, Value)> {
    let id = Args(p).int("displayId")? as DisplayId;
    if !crate::mac::recording::is_recording(id) {
        return None;
    }
    let kept = matches!(
        method,
        "click"
            | "mouseDown"
            | "mouseUp"
            | "move"
            | "drag"
            | "scroll"
            | "type"
            | "key"
            | "holdKey"
            | "elementAction"
            | "launchApp"
            | "placeWindow"
            | "requestHelp"
            | "withdrawHelp"
            | "requestControl"
            | "withdrawControlRequest"
            | "windowAccess"
    );
    if !kept {
        return None;
    }
    let mut event = serde_json::Map::new();
    event.insert("action".into(), json!(method));
    for (key, value) in p {
        if key != "displayId" && key != "sessionKey" && !value.is_null() {
            event.insert(key.clone(), value.clone());
        }
    }
    Some((id, Value::Object(event)))
}

async fn handle_one(
    core: Entity<Core>,
    connection: u64,
    method: &str,
    p: Params,
    cx: &mut AsyncApp,
) -> Reply {
    let p = Args(&p);
    if needs_permissions(method) {
        permitted(cx).await?;
        if cx
            .background_executor()
            .spawn(async { cg::screen_locked() })
            .await
        {
            return Err(ControlError {
                blocked: None,
                code: "locked",
                message: "The Mac's screen is locked, so macOS blocks input and screenshots on \
                          every display. Wait and retry after the user unlocks it, or call \
                          request_help to ask for them."
                    .into(),
            });
        }
    }
    if method != "createDisplay"
        && !method.starts_with("debug")
        && let Some(id) = p.int("displayId")
    {
        usable(&core, connection, &p, id as DisplayId, method, cx)?;
    }
    // A window an app makes right after the agent drives the display is
    // the agent's; the kept catches go by this.
    let acting = matches!(
        method,
        "click"
            | "mouseDown"
            | "mouseUp"
            | "move"
            | "drag"
            | "scroll"
            | "type"
            | "key"
            | "holdKey"
            | "elementAction"
            | "launchApp"
    )
    .then(|| p.int("displayId"))
    .flatten()
    .map(|id| id as DisplayId);
    // The same element action again after input_pending: it waits on the
    // queued click, or reads what came of it, and never presses twice.
    if method == "elementAction"
        && let Some(id) = acting
        && let Some(reply) =
            super::front_queue::rejoin(&core, connection, id, &element_operation(&p), cx).await
    {
        return reply;
    }
    if let Some(id) = acting {
        crate::mac::catch::agent_acted(id);
        if method != "launchApp" {
            watch_for_changes(&core, id, cx);
        }
        // A newer action of the agent replaces its queued front request
        // (ADR 0013); click, type and key do so as they go in.
        if matches!(
            method,
            "mouseDown" | "mouseUp" | "drag" | "scroll" | "holdKey" | "elementAction"
        ) {
            super::front_queue::replaced_by(&core, connection, id, cx);
        }
    }
    // A queued key or type checks no target: a window launched or placed
    // since could take it, so these replace the queued request too.
    if matches!(method, "launchApp" | "placeWindow")
        && let Some(id) = p.int("displayId")
    {
        super::front_queue::replaced_by(&core, connection, id as DisplayId, cx);
    }
    let refresh_acted = || {
        if let Some(id) = acting {
            crate::mac::catch::agent_acted(id);
        }
    };
    let result: Result<Value, ControlError> = match method {
        "createDisplay" => {
            let mut reply = create_display(&core, connection, &p, cx).await?;
            // "Displays have separate Spaces" off: the agent hears it with
            // each display, and the user once, on a card (spaces).
            if off(cx, crate::mac::spaces::shared).await {
                reply["spaces"] = json!(agent_screen_model::spaces::SHARED_NOTE);
                if let Some(shell) = crate::ui::shell::current() {
                    let _ = shell.update(cx, |shell, cx| shell.note_spaces(true, cx));
                }
            }
            Ok(reply)
        }
        "recordStart" => {
            let id = display(&core, &p, true, cx)?;
            let label = core.read_with(cx, |c, _| {
                c.displays.get(id).map(|d| d.label()).unwrap_or_default()
            });
            let (video, timeline, codec) = cx
                .background_executor()
                .spawn(async move { crate::mac::recording::start(id, &label) })
                .await
                .map_err(ControlError::failed)?;
            core.update(cx, |_, cx| cx.notify());
            Ok(
                json!({ "video": video, "timeline": timeline, "selectedCodec": codec,
                    "hardwareEncoderVerified": false,
                    "codecNote": if codec == "avc1" { "HEVC is unavailable; using H.264. Hardware encoder use has not been verified." } else { "Configured HEVC. Hardware encoder use has not been verified." }
                }),
            )
        }
        "recordStop" => {
            let id = display(&core, &p, false, cx)?;
            let done = cx
                .background_executor()
                .spawn(async move { crate::mac::recording::stop(id) })
                .await
                .ok_or_else(|| ControlError::invalid("the display isn't being recorded"))?;
            core.update(cx, |_, cx| cx.notify());
            Ok(json!({
                "video": done.video,
                "timeline": done.timeline,
                "selectedCodec": done.codec,
                "hardwareEncoderVerified": false,
                "seconds": (done.seconds * 10.0).round() / 10.0,
                "actions": done.actions,
                "problem": done.problem,
                "sound": {
                    "apps": done.heard,
                    "leftOut": done.left_out,
                    "problem": done.sound_problem,
                },
            }))
        }
        "inputQueue" => input_queue_action(&core, connection, &p, cx).await,
        "endDisplay" => {
            let id = display(&core, &p, false, cx)?;
            core.update(cx, |core, cx| core.end(id, EndReason::Finished, cx));
            Ok(json!({}))
        }
        "settle" => {
            let id = display(&core, &p, false, cx)?;
            let size = core
                .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
                .unwrap_or(DEFAULT_SIZE);
            settle_display(&core, id, size, cx).await;
            // computer's screenshot: false still hears of windows that
            // strayed or were given away, as a screenshot would. Only when
            // asked: the news is told once, and other callers read it later.
            let mut result = json!({});
            if p.flag("news") {
                add_strayed(&core, id, &mut result, cx);
            }
            Ok(result)
        }
        "screenshot" => {
            let id = display(&core, &p, false, cx)?;
            let size = core
                .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
                .unwrap_or(DEFAULT_SIZE);
            let format = if p.string("format").as_deref() == Some("png") {
                Format::Png
            } else {
                Format::Jpeg
            };
            if p.flag("settle") {
                settle_display(&core, id, size, cx).await;
            }
            // Within the image budget the agent reads coordinates in, and
            // `global` maps them back with the same scale.
            let scale = shot_scale(size);
            let pixels = ((size.0 * scale).round(), (size.1 * scale).round());
            // A zoom: a region of the screenshot, in its pixels, captured
            // at up to twice the display's points for detail.
            let region = match p.0.get("region").and_then(Value::as_array) {
                Some(r) => {
                    let n: Vec<f64> = r.iter().filter_map(Value::as_f64).collect();
                    let [x0, y0, x1, y1] = n[..] else {
                        return Err(ControlError::invalid("region is [x0, y0, x1, y1]"));
                    };
                    // The screenshot's own size, rounded the same way.
                    let (w, h) = pixels;
                    if !(0.0 <= x0 && x0 < x1 && x1 <= w && 0.0 <= y0 && y0 < y1 && y1 <= h) {
                        return Err(ControlError::invalid(format!(
                            "region must lie within the {}×{} screenshot, x0 < x1 and y0 < y1",
                            w as i64, h as i64
                        )));
                    }
                    Some(Rect::new(
                        x0 / scale,
                        y0 / scale,
                        (x1 - x0) / scale,
                        (y1 - y0) / scale,
                    ))
                }
                None => None,
            };
            let pixels = match region {
                Some(r) => {
                    let zoom = (2.0f64).min(1568.0 / r.width.max(r.height));
                    ((r.width * zoom).round(), (r.height * zoom).round())
                }
                None => pixels,
            };
            let shot = cx
                .background_executor()
                .spawn(async move {
                    let shot = screenshot::display(id, pixels, format, region)?;
                    Ok::<_, String>(encode_shot(shot))
                })
                .await
                .map_err(ControlError::failed)?;
            let mut result = shot.into_json();
            result["scale"] = json!(scale);
            add_strayed(&core, id, &mut result, cx);
            Ok(result)
        }
        "click" => {
            let first = finish_arm(click_action(&core, connection, &p, cx).await, refresh_acted);
            return super::front_queue::queue_if_blocked(
                &core, connection, method, &p, first, None, cx,
            )
            .await;
        }
        "move" => {
            let (id, point) = target(&core, &p, cx).await?;
            if let Some(grab) = GRAB.with(|g| g.borrow().get(&id).copied()) {
                show_pointer(&core, id, point, false, cx);
                drag_to(grab, point, cx).await;
                return reply(json!({}));
            }
            match HOLDS.with(|h| h.borrow_mut().remove(&id)) {
                Some(Hold::Pipe(drag)) => {
                    if forwarder_stopped(&core, id, cx) {
                        off(cx, move || drag.cancel()).await;
                        return Err(ControlError::takeover(format!(
                            "the user took over display {id}, which let go of the button where it was. Call wait_for_control to wait until they hand it back."
                        )));
                    }
                    show_pointer(&core, id, point, false, cx);
                    let mut drag = drag;
                    let (drag, moved) = off(cx, move || {
                        let moved = drag.move_to(point);
                        (drag, moved)
                    })
                    .await;
                    if let Err(e) = moved {
                        off(cx, move || drag.cancel()).await;
                        return Err(ControlError::failed(format!(
                            "the move didn't reach the page, so the button was let go: {e}"
                        )));
                    }
                    HOLDS.with(|h| h.borrow_mut().insert(id, Hold::Pipe(drag)));
                    return reply(json!({}));
                }
                None => {}
            }
            guard_agent_point(&core, id, point, cx).await?;
            show_pointer(&core, id, point, false, cx)
                .move_to(point)
                .await;
            Ok(json!({}))
        }
        "drag" => {
            return finish_arm(drag_action(&core, connection, &p, cx).await, refresh_acted);
        }
        "mouseDown" | "mouseUp" => {
            return finish_arm(
                mouse_button_action(&core, connection, method == "mouseDown", &p, cx).await,
                refresh_acted,
            );
        }
        "cursorPosition" => {
            let id = display(&core, &p, false, cx)?;
            let (size, pointer) = core.read_with(cx, |c, _| {
                let d = c.displays.get(id);
                (
                    d.map(|d| (d.width, d.height)).unwrap_or(DEFAULT_SIZE),
                    d.and_then(|d| d.pointer).map(|p| (p.x, p.y)),
                )
            });
            let scale = shot_scale(size);
            Ok(match pointer {
                Some((x, y)) => json!({ "x": (x * scale).round(), "y": (y * scale).round() }),
                None => json!({}),
            })
        }
        "scroll" => {
            let (id, point) = target(&core, &p, cx).await?;
            guard_agent_point(&core, id, point, cx).await?;
            let delta = (
                p.number("deltaX").unwrap_or(0.0),
                p.number("deltaY").unwrap_or(0.0),
            );
            let modifiers = modifiers(&p)?;
            show_pointer(&core, id, point, false, cx)
                .wheel(point, delta, modifiers)
                .await;
            Ok(json!({}))
        }
        "type" => {
            let first = finish_arm(type_action(&core, &p, cx).await, refresh_acted);
            return super::front_queue::queue_if_blocked(
                &core, connection, method, &p, first, None, cx,
            )
            .await;
        }
        "key" => {
            let first = finish_arm(key_action(&core, &p, cx).await, refresh_acted);
            return super::front_queue::queue_if_blocked(
                &core, connection, method, &p, first, None, cx,
            )
            .await;
        }
        "holdKey" => {
            let id = input_display(&core, &p, cx).await?;
            let keys = p
                .string("keys")
                .ok_or_else(|| ControlError::invalid("keys is required"))?;
            let chord = KeyChord::parse(&keys).map_err(|e| ControlError::invalid(e.to_string()))?;
            let held =
                Duration::from_secs_f64(p.number("seconds").unwrap_or(1.0).clamp(0.05, 10.0));
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            let within = cg::display_bounds(id);
            if let Err(e) = keys_allowed(&forwarder, within, cx).await {
                forwarder.keys_done().await;
                return Err(e);
            }
            let Some(pressed) = forwarder.chord(chord, true, within).await else {
                let e = if forwarder.agent_stopped() {
                    stopped_input(id, "the held keys", false)
                } else {
                    keys_refused(&forwarder, id).await
                };
                forwarder.keys_done().await;
                return Err(e);
            };
            // Let go at once if the user takes over: their keys would come
            // out with the agent's modifier held.
            let until = std::time::Instant::now() + held;
            while std::time::Instant::now() < until && !forwarder.agent_stopped() {
                cx.background_executor()
                    .timer(Duration::from_millis(50).min(until - std::time::Instant::now()))
                    .await;
            }
            // Released where it was pressed, wherever keys go now.
            forwarder.release_chord(chord, pressed).await;
            forwarder.keys_done().await;
            if forwarder.agent_stopped() {
                return Err(ControlError::takeover(format!(
                    "the user took over display {id}, so the keys were released early. Call wait_for_control to wait until they hand it back."
                )));
            }
            Ok(json!({}))
        }
        "listWindows" if p.flag("strays") => {
            let id = display(&core, &p, false, cx)?;
            let list: Vec<Value> = strays(&core, id, cx)
                .await
                .iter()
                .map(|w| w.json())
                .collect();
            Ok(json!({ "windows": list }))
        }
        "listWindows" => {
            let id = display(&core, &p, false, cx)?;
            let (records, started, not_its) = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .map(|d| {
                        // Windows the user took back or gave away: never
                        // listed as the display's, wherever they are.
                        let not_its: std::collections::HashSet<u32> =
                            d.returned.iter().chain(d.given.keys()).copied().collect();
                        (d.records.clone(), d.started.clone(), not_its)
                    })
                    .unwrap_or_default()
            });
            let scale = display_scale(&core, id, cx);
            // The display's windows that show nowhere now stay listed, with
            // why: minimized, their app hidden, or on another Space.
            let recorded: Vec<_> = records.values().map(|r| r.target).collect();
            let (shown, away) = off(cx, move || {
                let shown = windows::on_display(id);
                let ids = shown.iter().map(|w| w.target.window_id).collect();
                let away = windows::held_off_screen(id, &recorded, &ids, &not_its);
                (shown, away)
            })
            .await;
            let advice: Vec<String> = away
                .iter()
                .filter_map(|(w, p)| p.advice(w.target.window_id as i64, id))
                .map(|advice| format!("{advice}."))
                .collect();
            let away_json = away.iter().filter_map(|(w, p)| {
                let mut json = w.off_screen_json(p.off_screen()?);
                let record = records.get(&w.target.window_id);
                let unrecorded = if started.contains(&w.target.pid) {
                    "launched"
                } else {
                    "unknown"
                };
                json["origin"] = json!(record.map_or(unrecorded, |r| r.origin.as_str()));
                json["disposition"] =
                    json!(record.and_then(|r| r.disposition).map(Disposition::as_str));
                Some(json)
            });
            let list: Vec<Value> = shown
                .iter()
                .map(|w| {
                    let mut json = w.display_json(scale);
                    let record = records.get(&w.target.window_id);
                    // A further window of an app the agent started, which its
                    // catch moved here, is as launched as the first.
                    let unrecorded = if started.contains(&w.target.pid) {
                        "launched"
                    } else {
                        "unknown"
                    };
                    json["origin"] = json!(record.map_or(unrecorded, |r| r.origin.as_str()));
                    json["disposition"] =
                        json!(record.and_then(|r| r.disposition).map(Disposition::as_str));
                    json
                })
                .chain(away_json)
                .collect();
            let mut reply = json!({ "windows": list, "scale": scale });
            // list_windows shows the reply as it is: the note says it.
            let mut notes = Vec::new();
            if let Some(given) = add_given(&core, id, &mut reply, cx) {
                notes.push(held::given_note(&given));
            }
            notes.extend(advice);
            if !notes.is_empty() {
                reply["note"] = json!(notes.join(" "));
            }
            Ok(reply)
        }
        "setDisposition" => {
            let id = display(&core, &p, true, cx)?;
            let wanted = p.int("windowId");
            let window = off(cx, move || {
                let w = wanted?;
                windows::on_display(id)
                    .into_iter()
                    .find(|x| x.target.window_id as i64 == w)
            })
            .await
            .ok_or_else(|| {
                ControlError::not_found(format!(
                    "no window with that id on display {id}; call list_windows for its windows"
                ))
            })?;
            let disposition = p
                .string("disposition")
                .and_then(|d| Disposition::parse(&d))
                .ok_or_else(|| ControlError::invalid("disposition must be keep or close"))?;
            let handed_over = core.update(cx, |c, cx| {
                let d = c.displays.get_mut(id)?;
                d.set_disposition(window.target, disposition);
                cx.notify();
                Some(d.records[&window.target.window_id].origin == Origin::HandedOver)
            });
            Ok(json!({ "handedOver": handed_over.unwrap_or(false) }))
        }
        "placeWindow" => return place_window_action(&core, &p, cx).await,
        "windowState" => window_state_action(&core, &p, cx).await,
        "waitForChange" => wait_for_change(&core, connection, &p, cx).await,
        "cancelWaitForChange" => {
            let wait = p
                .int("waitId")
                .ok_or_else(|| ControlError::invalid("waitId is required"))?;
            cancel_wait((connection, wait as u64));
            Ok(json!({ "cancelled": true }))
        }
        "elementAction" => {
            return finish_arm(
                element_action(&core, connection, &p, cx).await,
                refresh_acted,
            );
        }
        "launchApp" => {
            let id = input_display(&core, &p, cx).await?;
            let name = p
                .string("app")
                .ok_or_else(|| ControlError::invalid("app is required"))?;
            let target = p.string("open").map(|t| open_target(&t)).transpose()?;
            let start = Start::parse(&p)?;
            let launched = launch(&core, id, &name, target.as_deref(), &start, cx).await?;
            let mut reply = match launched.window {
                Some(w) => json!({
                    "window": in_shot(w.json(), display_scale(&core, id, cx)),
                    "wasRunning": launched.was_running,
                    "reason": launched.reason,
                }),
                None => json!({ "wasRunning": launched.was_running, "reason": launched.reason }),
            };
            if launched.shared {
                reply["shared"] = json!(true);
            }
            // The app this display started, which the MCP server hands to
            // the session's next display if this app quits.
            if let Some(pid) = launched.pid.filter(|pid| {
                core.read_with(cx, |c, _| {
                    c.displays.get(id).is_some_and(|d| d.started.contains(pid))
                })
            }) {
                reply["pid"] = json!(pid);
            }
            Ok(reply)
        }
        // What only the app knows for an agent's bug report (ADR 0024):
        // the setting, the macOS version, whether AeroSpace runs, the
        // titles of windows on the session's displays and their Display
        // Labels' task titles, which the report removes, and the display
        // log's lines about them.
        "reportContext" => {
            let key = p.string("sessionKey").unwrap_or_default();
            let (ids, labels) = core.read_with(cx, |c, _| {
                let ids = c.displays.of_session(&key);
                let labels: Vec<String> = ids
                    .iter()
                    .filter_map(|&id| c.displays.get(id))
                    .filter_map(|d| d.owner.as_ref().map(|o| o.title.clone()))
                    .collect();
                (ids, labels)
            });
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let (automatic, consent, aerospace) = shell
                .update(cx, |shell, _| {
                    (
                        shell.reports.automatic,
                        shell.reports.consent,
                        shell.aerospace_running(),
                    )
                })
                .map_err(|e| ControlError::failed(e.to_string()))?;
            let (titles, log, (name, who)) = cx
                .background_executor()
                .spawn(async move {
                    let titles: Vec<String> = ids
                        .iter()
                        .flat_map(|&id| crate::mac::windows::on_display(id))
                        .map(|w| w.title)
                        .filter(|t| !t.is_empty())
                        .collect();
                    (
                        titles,
                        crate::lifecycle::lines_about(&ids, 30),
                        crate::reports::who(),
                    )
                })
                .await;
            // The app's own build: what the user updates. The client's
            // process may be older.
            let build = agent_screen_mcp::diagnostics::build_identity(env!("CARGO_PKG_VERSION"));
            Ok(json!({
                "automatic": automatic,
                "consent": consent,
                "version": build.package_version,
                "revision": build.git_revision,
                "dirty": build.dirty,
                "macos": crate::reports::macos_version(),
                "aerospace": aerospace,
                "titles": titles,
                "private": labels.into_iter().chain(who).collect::<Vec<_>>(),
                "name": name,
                "log": log,
            }))
        }
        // An agent's redacted bug report, for the menu bar (ADR 0024). It
        // opens nothing: the report waits on the card for the user's click,
        // and only a relay report may go out at once, after the user agreed
        // to that on a card that said so.
        "fileReport" => {
            use agent_screen_model::report::{self as model, Filed, Via};
            let prefix = format!(
                "https://github.com/{}/issues/new?template={}&",
                model::REPO,
                model::TEMPLATE
            );
            let filing = model::Filing {
                agent: p.string("agent").unwrap_or_else(|| "An agent".into()),
                title: p
                    .string("title")
                    .ok_or_else(|| ControlError::invalid("title is required"))?,
                fingerprint: p.string("fingerprint").unwrap_or_default(),
                url: p
                    .string("url")
                    .filter(|u| u.starts_with(&prefix) && u.len() <= model::MAX_URL)
                    .ok_or_else(|| {
                        ControlError::invalid(format!("url must start with {prefix}"))
                    })?,
                body: p
                    .string("body")
                    .unwrap_or_default()
                    .chars()
                    .take(20_000)
                    .collect(),
            };
            let via = match p.string("via").as_deref() {
                Some("relay") => Via::Relay,
                _ => Via::Form,
            };
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let filed = shell
                .update(cx, |shell, cx| shell.file_report(filing, via, cx))
                .map_err(|e| ControlError::failed(e.to_string()))?;
            Ok(match filed {
                Filed::Off => json!({ "status": "off" }),
                Filed::Send => json!({ "status": "send" }),
                Filed::Waiting(id) => json!({ "status": "waiting", "report": id }),
                Filed::Again(earlier) => json!({ "status": "again", "url": earlier.url }),
                Filed::TooMany => json!({ "status": "too_many" }),
            })
        }
        // `AgentScreen permissions reset`, confirmed in Terminal. Any local
        // process can reach this socket, so the app asks again in Settings
        // (Reset & Reopen runs it) unless test hooks are on; a request
        // without the Terminal's confirmation is refused. Checked first,
        // as Settings' own reset is: a copy without Agent Screen's bundle
        // id never gets this far.
        "resetPermissions" => {
            use agent_screen_model::tcc_reset::{SocketReset, socket_reset};
            let confirmed = p.0.get("confirmed").and_then(Value::as_bool) == Some(true);
            let script = crate::mac::permissions::reset_script().map_err(ControlError::failed)?;
            match socket_reset(confirmed, super::debug_methods()) {
                SocketReset::Refuse => {
                    return Err(ControlError::invalid(
                        "resetPermissions needs the user's confirmation",
                    ));
                }
                SocketReset::Ask => {
                    if let Some(shell) = crate::ui::shell::current() {
                        let _ = shell.update(cx, |shell, cx| shell.ask_reset_in_settings(cx));
                    }
                    Ok(json!({ "status": "confirm" }))
                }
                SocketReset::Run => {
                    let steps = script.steps.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(100))
                            .await;
                        match crate::mac::permissions::spawn_reset(&script) {
                            Ok(()) => cx.update(|cx| cx.quit()),
                            Err(error) => log::warn!("reset permissions: {error}"),
                        }
                    })
                    .detach();
                    Ok(json!({ "status": "resetting", "pid": std::process::id(), "steps": steps }))
                }
            }
        }
        // The MCP server of an agent session listed the tools.
        "agentReady" => {
            // Stored and logged in English; `shell::shown_client` says it
            // in the UI's language.
            let client = p
                .string("client")
                .unwrap_or_else(|| crate::ui::shell::UNNAMED_CLIENT.into());
            if let Some(shell) = crate::ui::shell::current() {
                let _ = shell.update(cx, |shell, cx| shell.agent_ready(client, cx));
            }
            Ok(json!({}))
        }
        "requestHelp" => {
            let id = display(&core, &p, true, cx)?;
            let reason = p
                .string("reason")
                .filter(|r| !r.is_empty())
                .ok_or_else(|| ControlError::invalid("reason is required"))?;
            let steps = match p.0.get("steps") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(String::from)
                            .ok_or_else(|| ControlError::invalid("steps are strings"))
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(ControlError::invalid("steps is a list of strings")),
            };
            let steps =
                agent_screen_model::model::help_steps(steps).map_err(ControlError::invalid)?;
            // The asker's MCP server understands answers other than a
            // Takeover (ADR 0015). One built earlier doesn't say so.
            let answers = p.flag("answers");
            let asked = core.update(cx, |c, cx| {
                let asked = c.displays.request_help(id, reason, steps, answers);
                cx.notify();
                asked
            });
            Ok(match asked {
                Some(HelpAsked::Answered(woken)) => json!({ "answered": woken.to_json(false) }),
                Some(HelpAsked::Opened(request)) => json!({ "requestId": request }),
                None => json!({ "requestId": null }),
            })
        }
        "withdrawHelp" => {
            let id = display(&core, &p, false, cx)?;
            let withdrawn = core.update(cx, |c, cx| {
                let withdrawn = c.displays.withdraw_help(id);
                if withdrawn {
                    cx.notify();
                }
                withdrawn
            });
            Ok(json!({ "withdrawn": withdrawn }))
        }
        // The user answered the Help Request in the conversation, and the
        // agent relays their words: the request ends as Continue, as the
        // card's button ends it (ADR 0013, ADR 0015).
        "answerHelp" => {
            let id = display(&core, &p, true, cx)?;
            let words = p
                .string("userInstruction")
                .filter(|w| !w.trim().is_empty())
                .ok_or_else(|| ControlError::invalid("userInstruction is required"))?;
            core.update(cx, |c, cx| {
                c.answer(id, Kind::Help, None, Answer::Continue, cx)
                    .map_err(|why| ControlError::failed(why.message()))?;
                // The agent relayed it: its next wait needn't hear it.
                c.displays.forget_unheard(id);
                log::info!("display {id}: Help Request answered in the conversation");
                crate::mac::recording::note(
                    id,
                    json!({ "action": "helpAnswered", "by": "chat", "instruction": words }),
                );
                Ok::<_, ControlError>(())
            })?;
            Ok(json!({ "answered": true }))
        }
        // During a Takeover, ask the user to hand the display back. The
        // agent then waits with waitForControl.
        "requestControl" => {
            if let Some(words) = p.string("userInstruction") {
                let id = display(&core, &p, true, cx)?;
                return chat_control(&core, id, &words, p.string("control").as_deref(), cx)
                    .and_then(reply);
            }
            let id = display(&core, &p, false, cx)?;
            let reason = p
                .string("reason")
                .filter(|r| !r.is_empty())
                .ok_or_else(|| ControlError::invalid("reason is required"))?;
            let asked = core.update(cx, |c, cx| {
                let asked = c.displays.request_control(id, reason);
                if asked == ControlAsk::Asked {
                    cx.notify();
                }
                asked
            });
            Ok(json!({
                "requested": asked != ControlAsk::NotNeeded,
                "alreadyAsked": asked == ControlAsk::AlreadyAsked,
            }))
        }
        "windowAccess" => window_access(&core, &p, cx).await,
        "withdrawControlRequest" => {
            let id = display(&core, &p, false, cx)?;
            let withdrawn = core.update(cx, |c, cx| {
                let withdrawn = c.displays.withdraw_control_request(id);
                if withdrawn {
                    cx.notify();
                }
                withdrawn
            });
            Ok(json!({ "withdrawn": withdrawn }))
        }
        "waitForControl" => {
            let id = display(&core, &p, false, cx)?;
            let seconds = p
                .number("timeoutSeconds")
                .unwrap_or(100.0)
                .clamp(1.0, 600.0);
            let wait = core.update(cx, |c, _| c.displays.wait_for_control(id));
            let woken = match wait {
                Wait::Now(woken) => woken,
                Wait::Later(rx) => {
                    let timer = cx
                        .background_executor()
                        .timer(Duration::from_secs_f64(seconds));
                    futures::select! {
                        woken = rx.fuse() => woken.unwrap_or(Woken::of(None, Resolution::Ended)),
                        _ = FutureExt::fuse(timer) => {
                            // What still waits for the user.
                            let open = core.read_with(cx, |c, _| c.displays.waiting_on(id));
                            Woken::of(open, Resolution::TimedOut)
                        }
                    }
                }
            };
            // Ended because the user closed it: the agent should stop.
            let ended = (woken.resolution == Resolution::Ended)
                .then(|| core.read_with(cx, |c, _| c.displays.ended_reason(id)))
                .flatten();
            let closed = ended == Some(EndReason::ClosedByUser);
            // An MCP server built before resolutions reads these flags.
            let (help_open, control_asked) = core.read_with(cx, |c, _| {
                c.displays.get(id).map_or((false, false), |d| {
                    (
                        d.requests.is_open(Kind::Help) && !d.takeover,
                        d.requests.is_open(Kind::Control),
                    )
                })
            });
            // How long the user has left the mouse and keyboard alone while
            // they hold the display: done, they may only have missed the
            // hand-back, as after a passkey that ended in seconds.
            let idle = core
                .read_with(cx, |c, _| c.displays.get(id).is_some_and(|d| d.takeover))
                .then(|| crate::mac::ime::user_idle().floor() as u64);
            let mut out = woken.to_json(closed);
            if let Some(reason) = ended {
                out["endedReason"] = json!(reason.tag());
                out["endedMessage"] =
                    json!(agent_screen_model::ended::ended_message(id, reason, true));
            }
            out["helpOpen"] = json!(help_open);
            out["controlAsked"] = json!(control_asked);
            out["userIdleSeconds"] = json!(idle);
            Ok(out)
        }
        other => match super::debug::handle(&core, connection, other, &p, cx).await {
            Some(result) => result,
            None => Err(ControlError::invalid(format!("unknown method {other}"))),
        },
    };
    refresh_acted();
    result.and_then(reply)
}

/// Outcome of an extracted `handle_one` arm: preserves the exact control flow
/// of inline arms with respect to `handle_one`'s trailing `agent_acted` refresh
/// (ADR 0007).
enum Arm {
    /// Arm completed via its tail expression; `handle_one` refreshes
    /// `agent_acted` before encoding the reply.
    Finish(Result<Value, ControlError>),
    /// Arm returned early (`return reply(...)`, `queued_click`, `queued_drag`,
    /// `pipe_drag`, `through_input_method`, `shared_app::...`); `handle_one`
    /// returns directly and skips the trailing `agent_acted` refresh.
    Early(Reply),
}

/// Run a queued click, key or type call again (`front_queue`).
pub(super) async fn replay(
    core: &Entity<Core>,
    connection: u64,
    method: &str,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Reply {
    let acted = p.int("displayId").map(|id| id as DisplayId);
    let refresh = || {
        if let Some(id) = acted {
            crate::mac::catch::agent_acted(id);
        }
    };
    match method {
        // Boxed: a click with real_input waits in the Input Queue too.
        "click" => finish_arm(
            Box::pin(click_action(core, connection, p, cx)).await,
            refresh,
        ),
        "type" => finish_arm(Box::pin(type_action(core, p, cx)).await, refresh),
        "key" => finish_arm(Box::pin(key_action(core, p, cx)).await, refresh),
        other => Err(ControlError::invalid(format!(
            "{other} can't wait in the queue"
        ))),
    }
}

fn finish_arm(arm: Result<Arm, ControlError>, on_finish: impl FnOnce()) -> Reply {
    match arm? {
        Arm::Finish(result) => {
            on_finish();
            result.and_then(reply)
        }
        Arm::Early(reply) => reply,
    }
}

async fn click_action(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Arm, ControlError> {
    let (id, point) = target(core, p, cx).await?;
    outside_menu_bar(id, point)?;
    let button =
        Button::parse(p.string("button").as_deref().unwrap_or("left")).ok_or_else(|| {
            ControlError::invalid(format!(
                "unknown button {}",
                p.string("button").unwrap_or_default()
            ))
        })?;
    let modifiers = modifiers(p)?;
    let count = p.int("count").unwrap_or(1);
    // The agent saw a background click have no effect and asks for
    // the user's pointer (ADR 0013, "Clicks"). Nothing goes out on
    // the background route meanwhile.
    if p.flag("realInput") {
        guard_agent_point(core, id, point, cx).await?;
        // The menu a real right click opens would close when the
        // user's front app comes back.
        // So would a Control-held left click's.
        let pressed = real_click_button(button);
        if real_click::opens_menu(pressed, modifiers.control) {
            return Err(ControlError::invalid(real_click::NO_MENU_CLICK));
        }
        let click = real_click::Click::new(pressed, count).ok_or_else(|| {
            ControlError::invalid(
                "a click with the user's pointer presses its button one to three times",
            )
        })?;
        return Ok(Arm::Early(
            queued_click(core, connection, id, point, click, modifiers, cx).await,
        ));
    }
    // Check foreign-display ownership and find the top window in one
    // window-list pass. A window of the user's front app, beside the
    // window they work in: a posted click would make it key and take
    // their typing (#110).
    let checked = off(cx, move || crate::mac::input::click_check_at(id, point)).await;
    let hit_window = match checked {
        Err((target, owner_display)) => {
            return Err(foreign_point_error(core, id, target, owner_display, cx));
        }
        Ok(window) => window,
    };
    // A queued element action's click: only to the element's own window,
    // found by the window that hosts what is at the point, as the queue
    // kept it and as the press checks it going out.
    let own = own_windows(p);
    let mut bound_to = None;
    if let Some(own) = &own {
        let host = off(cx, move || {
            crate::mac::input::host_at(point).map(|(w, _)| w)
        })
        .await;
        if !bound_click_reaches(host.map(|h| h.window_id), own) {
            return Err(ControlError::failed(
                "the element's window is no longer at its center, as when it moved or another window covers it, so the queued click was not sent. Take a screenshot and get_window_state before you act again",
            ));
        }
        bound_to = host;
    }
    // A bound click is checked against the user's focus by its host
    // window, so a hosted panel waits beside the user's window too.
    let front_target = match own {
        Some(_) => bound_to,
        None => hit_window.map(|(t, _)| t),
    };
    if let Some(target) = front_target {
        let plain = modifiers == Default::default();
        let left = button == Button::Left;
        let (front, focused) = shared_app::front(target, cx).await;
        // With focus unknown, only an app with a window on the
        // user's screens may have theirs focused.
        let shared = front
            .shared(front == Front::FocusUnknown && shared_app::user_window(target.pid, cx).await);
        match (front, focused) {
            (Front::OtherFocused, Some(focused)) => {
                // Keys go to this window from now on, as after a
                // click. The app is in front, so this sends no
                // focus record.
                show_pointer(core, id, point, true, cx).focus(target).await;
                let beside = Beside { target, focused };
                return Ok(Arm::Early(
                    shared_app::click(id, point, beside, left, count, !plain, cx)
                        .await
                        .and_then(reply),
                ));
            }
            // The user's window may have focus: no focus record,
            // and nothing posted (#110).
            _ if shared => {
                show_pointer(core, id, point, true, cx);
                return Ok(Arm::Early(
                    shared_app::click_unverified(id, point, target, left, count, !plain, cx)
                        .await
                        .and_then(reply),
                ));
            }
            _ => {}
        }
    }
    let forwarder = show_pointer(core, id, point, true, cx);
    // A queued element click checks the user's focus again as it goes out.
    let check_front = own.is_some();
    let clicked = forwarder
        .click_in(point, button, count, modifiers, own, check_front)
        .await;
    let (hit, stopped) = (clicked.hit, clicked.stopped);
    if !stopped {
        match clicked.end() {
            // The user went back to their window of the app as the click
            // was about to go out: nothing was sent, and it waits again
            // (`front_queue`).
            ClickEnd::Wait => {
                if let Some(host) = clicked.front_took {
                    return Err(shared_app::took_focus(host));
                }
            }
            // Cut after its first press: final, never queued again.
            ClickEnd::Partial => {
                if let Some(window) = hit {
                    shared_app::note_click(id, window, point);
                }
                return Err(shared_app::partial_click(count));
            }
            // A queued element click whose window left the point while it
            // went out: nothing was sent.
            ClickEnd::Nothing if bound_to.is_some() => {
                return Err(ControlError::failed(
                    "the element's window is no longer at its center, as when it moved or another window covers it, so the queued click was not sent. Take a screenshot and get_window_state before you act again",
                ));
            }
            ClickEnd::Sent | ClickEnd::Nothing => {}
        }
    }
    if let Some(window) = hit {
        shared_app::note_click(id, window, point);
    }
    if stopped {
        return Err(stopped_input(id, "the click", hit.is_some()));
    }
    Ok(Arm::Finish(match hit {
        Some(window) => Ok(json!({ "windowId": window.window_id })),
        None => Err(ControlError::not_found(format!(
            "nothing at ({}, {}): no window there to click",
            p.number("x").unwrap_or(0.0) as i64,
            p.number("y").unwrap_or(0.0) as i64
        ))),
    }))
}

async fn drag_action(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Arm, ControlError> {
    let id = input_display(core, p, cx).await?;
    let from = global(core, id, p.require("fromX")?, p.require("fromY")?, cx)?;
    let to = global(core, id, p.require("toX")?, p.require("toY")?, cx)?;
    outside_menu_bar(id, from)?;
    guard_agent_point(core, id, from, cx).await?;
    guard_agent_point(core, id, to, cx).await?;
    let modifiers = modifiers(p)?;
    if p.flag("realInput") {
        return Ok(Arm::Early(
            queued_drag(
                core,
                connection,
                id,
                (from, Some(to)),
                modifiers,
                "Requested real-input retry",
                cx,
            )
            .await,
        ));
    }
    // A drag on a title bar moves the window, which posted events
    // can't do.
    if let Some(grab) = title_bar_at(id, from, cx).await {
        show_pointer(core, id, to, false, cx);
        if !drag_to(grab, to, cx).await {
            return Err(ControlError::failed(
                "the app would not let its window move",
            ));
        }
        return Ok(Arm::Early(reply(
            json!({ "movedWindow": true, "route": "placement" }),
        )));
    }
    // A drag from one window to another needs a system drag
    // session, which follows only the real pointer: posted, it
    // only selects what it grabbed (#166, ADR 0013, "Drags
    // between windows").
    let ends = off(cx, move || {
        let at = |p| crate::mac::input::window_at(p).map(|(w, _)| w);
        (at(from), at(to))
    })
    .await;
    if agent_screen_model::real_drag::between_windows(ends.0, ends.1) {
        return Ok(Arm::Early(
            queued_drag(
                core,
                connection,
                id,
                (from, Some(to)),
                modifiers,
                agent_screen_model::real_drag::BETWEEN_WINDOWS,
                cx,
            )
            .await,
        ));
    }
    // A web page takes a drag through the app's DevTools pipe, or
    // not at all (#37, ADR 0010).
    let refused = match web_route(from, cx).await {
        Route::Pipe(page) => {
            return Ok(Arm::Early(
                pipe_drag(core, id, page, from, to, modifiers, cx).await,
            ));
        }
        Route::Refused(app) => Some(app),
        Route::Unmapped(why) => return Err(unmapped(why)),
        Route::Posted => None,
    };
    if let Some((target, _)) = off(cx, move || crate::mac::input::window_at(from)).await {
        let forwarder = core.update(cx, |c, _| c.forwarder(id));
        let guard = forwarder.clone();
        let changed = off(cx, move || {
            ax::drag_slider(target, from, to, || {
                !guard.agent_stopped() && windows::on_display(id).iter().any(|w| w.target == target)
            })
        })
        .await;
        if forwarder.agent_stopped() {
            return Err(ControlError::takeover(
                "input stopped during slider routing; inspect its current value before retrying",
            ));
        }
        let changed = changed.map_err(ControlError::failed)?;
        if changed {
            show_pointer(core, id, to, false, cx);
            return Ok(Arm::Early(reply(json!({ "route": "accessibility" }))));
        }
    }
    if let Some(app) = refused {
        return Ok(Arm::Early(
            queued_drag(
                core,
                connection,
                id,
                (from, Some(to)),
                modifiers,
                &devtools::refused_message(&app),
                cx,
            )
            .await,
        ));
    }
    shared_app::posted_press_allowed(from, "drag", cx).await?;
    let forwarder = show_pointer(core, id, from, true, cx);
    let dragged = forwarder.drag(from, to, modifiers).await;
    if dragged == Sent::Nowhere && forwarder.agent_stopped() {
        return Err(stopped_input(id, "the drag", false));
    }
    let released = match dragged {
        Sent::Stopped(at) => at,
        _ => to,
    };
    show_pointer(core, id, released, false, cx);
    if matches!(dragged, Sent::Stopped(_)) {
        return Err(ControlError::takeover(format!(
            "the user took over display {id} during the drag, which let go short of its target; near the start, the release may have acted on what it grabbed. Call wait_for_control to wait until they hand it back."
        )));
    }
    if dragged == Sent::Nowhere {
        return Err(ControlError::not_found(format!(
            "nothing at ({}, {}) to drag",
            p.number("fromX").unwrap_or(0.0) as i64,
            p.number("fromY").unwrap_or(0.0) as i64
        )));
    }
    Ok(Arm::Finish(Ok(json!({ "route": "posted" }))))
}

async fn mouse_button_action(
    core: &Entity<Core>,
    connection: u64,
    down: bool,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Arm, ControlError> {
    let id = input_display(core, p, cx).await?;
    // Without a point the button acts where the Agent Pointer is.
    let point = match (p.number("x"), p.number("y")) {
        (Some(x), Some(y)) => global(core, id, x, y, cx)?,
        _ => {
            let bounds = cg::display_bounds(id);
            core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .and_then(|d| d.pointer)
                    .map(|p| (p.x, p.y))
            })
            .map(|(x, y)| (bounds.x + x, bounds.y + y))
            .ok_or_else(|| ControlError::invalid("no pointer position yet; give a coordinate"))?
        }
    };
    if down {
        outside_menu_bar(id, point)?;
        guard_agent_point(core, id, point, cx).await?;
    }
    let grabbed = GRAB.with(|g| g.borrow_mut().remove(&id));
    if down && let Some(grab) = title_bar_at(id, point, cx).await {
        show_pointer(core, id, point, true, cx);
        let window = grab.target.window_id;
        GRAB.with(|g| g.borrow_mut().insert(id, grab));
        return Ok(Arm::Early(reply(json!({ "windowId": window }))));
    }
    if !down && let Some(grab) = grabbed {
        show_pointer(core, id, point, false, cx);
        drag_to(grab, point, cx).await;
        return Ok(Arm::Early(reply(
            json!({ "windowId": grab.target.window_id }),
        )));
    }
    let modifiers = modifiers(p)?;
    // A press in a web page goes through the app's DevTools pipe,
    // and so do the moves and the release after it (#37, ADR 0010).
    let held = HOLDS.with(|h| h.borrow_mut().remove(&id));
    if let Some(Hold::Pipe(drag)) = held {
        if down {
            off(cx, move || drag.cancel()).await;
        } else {
            show_pointer(core, id, point, false, cx);
            let window = drag.window();
            off(cx, move || drag.release(point)).await.map_err(|e| {
                ControlError::failed(format!("the release didn't reach the page: {e}"))
            })?;
            return Ok(Arm::Early(reply(json!({ "windowId": window }))));
        }
    }
    if down && p.flag("realInput") {
        return Ok(Arm::Early(
            queued_drag(
                core,
                connection,
                id,
                (point, None),
                modifiers,
                "Requested real-input press",
                cx,
            )
            .await,
        ));
    }
    if down {
        match web_route(point, cx).await {
            Route::Pipe(page) => {
                if forwarder_stopped(core, id, cx) {
                    return Err(ControlError::takeover(format!(
                        "the user took over display {id}, so the press was not sent. Call wait_for_control to wait until they hand it back."
                    )));
                }
                // Keys go to the window pressed, as after a click.
                let window = page.window();
                show_pointer(core, id, point, true, cx).focus(window).await;
                let window = window.window_id;
                let bits = modifier_bits(modifiers);
                let drag = off(cx, move || devtools::Drag::press(page, point, bits))
                    .await
                    .map_err(|e| {
                        ControlError::failed(format!("the press didn't reach the page: {e}"))
                    })?;
                HOLDS.with(|h| h.borrow_mut().insert(id, Hold::Pipe(drag)));
                return Ok(Arm::Early(reply(json!({ "windowId": window }))));
            }
            Route::Unmapped(why) => return Err(unmapped(why)),
            Route::Refused(app) => {
                return Ok(Arm::Early(
                    queued_drag(
                        core,
                        connection,
                        id,
                        (point, None),
                        modifiers,
                        &devtools::refused_message(&app),
                        cx,
                    )
                    .await,
                ));
            }
            Route::Posted => {}
        }
        shared_app::posted_press_allowed(point, "press", cx).await?;
    }
    let hit = show_pointer(core, id, point, down, cx)
        .button(point, down, modifiers)
        .await;
    Ok(Arm::Finish(match hit {
        Some(window) => Ok(json!({ "windowId": window.window_id })),
        None if down && forwarder_stopped(core, id, cx) => Err(ControlError::takeover(format!(
            "the user took over display {id}, so the press was not sent. Call wait_for_control to wait until they hand it back."
        ))),
        None if down => Err(ControlError::not_found(
            "nothing under the pointer to press on",
        )),
        None => Err(ControlError::invalid("the left button is not down")),
    }))
}

async fn type_action(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Arm, ControlError> {
    let id = input_display(core, p, cx).await?;
    let text = p
        .string("text")
        .ok_or_else(|| ControlError::invalid("text is required"))?;
    if p.flag("viaInputMethod") {
        let keys = crate::keychord::keys_for_text(&text).map_err(|c| {
            ControlError::invalid(format!(
                "{c:?} has no key on a US keyboard. Through the input method, type the keys it composes from, such as pinyin letters."
            ))
        })?;
        return Ok(Arm::Early(
            through_input_method(core, id, keys, p.string("inputSource"), cx).await,
        ));
    }
    let forwarder = core.update(cx, |c, _| c.forwarder(id));
    let bounds = cg::display_bounds(id);
    let chars: Vec<char> = crate::mac::input::typed_text(&text).chars().collect();
    let total = chars.len();
    let partial = |done: usize, e: ControlError| match done {
        0 => e,
        _ => ControlError::failed(format!(
            "typed {done} of {total} characters, then stopped: {} If you retry, type only the rest, or replace the whole field with set_value.",
            e.message
        )),
    };
    let typed = async {
        let mut done = 0;
        // In runs of 20, about 100 ms each, so the user switching to
        // the app mid-way stops the rest; the app coming to the
        // front stops a run at the next key.
        while done < total {
            let gate = keys_gate(&forwarder, bounds, cx)
                .await
                .map_err(|e| partial(done, e))?;
            if let Gate::Beside(beside) = gate {
                let rest: String = chars[done..].iter().collect();
                return shared_app::type_text(id, beside, &rest, done, cx).await;
            }
            let run: String = chars[done..].iter().take(20).collect();
            let length = run.chars().count();
            match forwarder.type_text(run, bounds).await {
                Sent::Nowhere => return Err(partial(done, keys_refused(&forwarder, id).await)),
                Sent::Dropped(shown) => {
                    let typed = done + length;
                    let shown: String = shown.chars().take(300).collect();
                    return Err(ControlError::failed(format!(
                        "sent {typed} of {total} characters, but the field holds {shown:?}, without some of them: the app dropped keys that came too fast, or changed the text. Typing stopped there. Select the field's text and type it again in shorter calls, a word or a few characters each, then check the field."
                    )));
                }
                Sent::Typed(typed) => {
                    let typed = done + typed;
                    return Err(ControlError::takeover(format!(
                        "the user took over display {id} after {typed} of {total} characters. Call wait_for_control, then take a screenshot before you type the rest."
                    )));
                }
                Sent::Fronted(typed) => done += typed,
                _ => done += length,
            }
        }
        // Return and Tab move on from what the last click picked.
        if chars.iter().any(|c| matches!(c, '\n' | '\r' | '\t')) {
            shared_app::forget(id);
        }
        Ok(json!({}))
    }
    .await;
    forwarder.keys_done().await;
    Ok(Arm::Finish(typed))
}

async fn key_action(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Arm, ControlError> {
    let id = input_display(core, p, cx).await?;
    let keys = p
        .string("keys")
        .ok_or_else(|| ControlError::invalid("keys is required"))?;
    let chords =
        KeyChord::parse_sequence(&keys).map_err(|e| ControlError::invalid(e.to_string()))?;
    if p.flag("viaInputMethod") {
        return Ok(Arm::Early(
            through_input_method(core, id, chords, p.string("inputSource"), cx).await,
        ));
    }
    let forwarder = core.update(cx, |c, _| c.forwarder(id));
    let repeat = p.int("repeat").unwrap_or(1);
    let bounds = cg::display_bounds(id);
    let total = chords.len();
    let names: Vec<&str> = keys.split_whitespace().collect();
    let partial = |pressed: usize, e: ControlError| match pressed {
        0 => e,
        _ => ControlError::failed(format!(
            "pressed {pressed} of {total} keys, then stopped: {} If you retry, press only the rest.",
            e.message
        )),
    };
    let pressed = async {
        let mut notes = Vec::new();
        // Whether anything changed after the last key that pressed.
        let mut changed = None;
        for (pressed, chord) in chords.into_iter().enumerate() {
            if forwarder.agent_stopped() {
                return Err(ControlError::takeover(format!(
                    "the user took over display {id} after {pressed} of {total} keys. Call wait_for_control, then take a screenshot before you press the rest."
                )));
            }
            let gate = keys_gate(&forwarder, bounds, cx)
                .await
                .map_err(|e| partial(pressed, e))?;
            let name = names.get(pressed).copied().unwrap_or("key");
            if let Gate::Beside(beside) = gate {
                if repeat > 1 {
                    return Err(partial(
                        pressed,
                        shared_app::refusal(beside, &format!("{name} with repeat")),
                    ));
                }
                let (note, after) = shared_app::key(id, beside, chord, name, cx)
                    .await
                    .map_err(|e| partial(pressed, e))?;
                notes.push(note);
                changed = after.or(changed);
                continue;
            }
            // Select All needs a first responder, which an app in the
            // background lacks; set the selection through accessibility.
            if chord == KeyChord::parse("cmd+a").expect("a chord")
                && let Some(target) = forwarder.keys_target(bounds).await
                && cx
                    .background_executor()
                    .spawn(async move { ax::select_all(target) })
                    .await
            {
                continue;
            }
            // A posted key may move focus on from what the last
            // click picked, as Tab does.
            shared_app::forget(id);
            let sent = forwarder.press(chord, repeat, bounds).await;
            if forwarder.agent_stopped() && sent.is_none_or(|n| n < repeat.max(1)) {
                let some = sent.is_some_and(|n| n > 0);
                return Err(partial(pressed, stopped_input(id, "the keys", some)));
            }
            match sent {
                None => return Err(partial(pressed, keys_refused(&forwarder, id).await)),
                // The app came to the front partway through.
                Some(sent) if sent < repeat.max(1) => {
                    let why = match keys_gate(&forwarder, bounds, cx).await {
                        Ok(Gate::Beside(beside)) => {
                            shared_app::refusal(beside, "the rest").message
                        }
                        Err(e) => e.message,
                        Ok(Gate::Post) => "its app came to the front.".into(),
                    };
                    return Err(partial(
                        pressed,
                        ControlError::failed(format!(
                            "{name} went out {sent} of {repeat} times, then stopped: {why}"
                        )),
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(match notes.is_empty() {
            true => json!({}),
            false => {
                let mut reply = json!({ "route": "accessibility", "note": notes.join(" ") });
                if let Some(changed) = changed {
                    reply["changed"] = json!(changed);
                }
                reply
            }
        })
    }
    .await;
    forwarder.keys_done().await;
    Ok(Arm::Finish(pressed))
}

async fn place_window_action(core: &Entity<Core>, p: &Args<'_>, cx: &mut AsyncApp) -> Reply {
    let id = input_display(core, p, cx).await?;
    let wanted = p.int("windowId");
    let app = p.string("app").filter(|a| !a.is_empty());
    if wanted.is_none() && app.is_none() {
        return Err(ControlError::invalid("window_id or app is required"));
    }
    let asked = FrameRequest {
        x: p.number("x"),
        y: p.number("y"),
        width: p.number("width"),
        height: p.number("height"),
    };
    if asked.width.is_some_and(|w| w <= 0.0) || asked.height.is_some_and(|h| h <= 0.0) {
        return Err(ControlError::invalid("width and height must be above 0"));
    }
    // A window already on the display only takes its new frame.
    if let Some(wanted) = wanted {
        let here = off(cx, move || {
            windows::on_display(id)
                .into_iter()
                .find(|w| w.target.window_id as i64 == wanted)
        })
        .await;
        if let Some(window) = here {
            return frame_window(core, id, window.target, asked, cx).await;
        }
    }
    // The app the agent named, resolved once: by a running app's
    // name or bundle id, an installed app's name, or an .app path.
    let named: Option<(NamedApp, Option<access::AppId>)> = match app.clone() {
        Some(text) => Some(
            off(cx, move || {
                let found = app::identify(&text);
                let named = NamedApp::new(&text, found.as_ref().map(|f| f.bundle_id.clone()));
                (named, found)
            })
            .await,
        ),
        None => None,
    };
    let matcher = named.as_ref().map(|(n, _)| n.clone());
    // A window on another of the session's displays moves over as a
    // Placement, with no Window Request (ADR 0016).
    let others: Vec<DisplayId> = core.read_with(cx, |c, _| {
        c.displays
            .get(id)
            .and_then(|d| d.session())
            .map(|key| c.displays.of_session(key))
            .unwrap_or_default()
            .into_iter()
            .filter(|&o| o != id)
            .collect()
    });
    if !others.is_empty() {
        let found = off(cx, {
            let matcher = matcher.clone();
            move || {
                let mut ids = AppIds::default();
                // The app has a window here already: that one is
                // what the agent means, as before ADR 0016.
                if wanted.is_none()
                    && windows::on_display(id)
                        .iter()
                        .any(|w| w.layer == 0 && is_named(w, None, matcher.as_ref(), &mut ids))
                {
                    return Vec::new();
                }
                let mut found = Vec::new();
                for o in others {
                    for w in windows::on_display(o) {
                        if (wanted.is_some() || w.layer == 0)
                            && is_named(&w, wanted, matcher.as_ref(), &mut ids)
                        {
                            found.push((o, w.target));
                        }
                    }
                }
                found
            }
        })
        .await;
        // A window_id names one window: move_between_own says why
        // it can't move. By app name, a window the user took back
        // or made, or one on a display they operate, is skipped for
        // the next (ADR 0016).
        let pick = match (wanted, found.first()) {
            (Some(_), first) => first.copied(),
            (None, _) => core.update(cx, |c, _| {
                for &(from, _) in &found {
                    sync_user_made(c, from);
                }
                c.displays.first_movable(id, &found)
            }),
        };
        if let Some((from, target)) = pick {
            return move_between_own(core, from, id, target, asked, cx).await;
        }
    }
    let words = p
        .string("userInstruction")
        .map(|w| access::user_instruction(&w))
        .transpose()
        .map_err(ControlError::invalid)?;
    let timeout = p.number("timeoutSeconds");
    // A window of an app the agent started is its own to place.
    let mut ids = AppIds::default();
    let stray = strays(core, id, cx)
        .await
        .into_iter()
        .find(|w| is_named(w, wanted, matcher.as_ref(), &mut ids));
    let Some(window) = stray else {
        // Any other window is the user's until they hand it over.
        // The user's windows that match, front to back, with their
        // apps' bundle ids, which Window Access goes by.
        let spares = core.read_with(cx, |c, _| c.spare_ids());
        let mut candidates: Vec<(windows::Window, Option<String>)> = off(cx, {
            let matcher = matcher.clone();
            move || {
                let mut ids = AppIds::default();
                let mut candidates = Vec::new();
                for w in windows::all() {
                    if w.layer == 0
                        && (windows::display_role(&w).accepts_window_request()
                            || windows::on_any(&w.bounds, &spares))
                        && is_named(&w, wanted, matcher.as_ref(), &mut ids)
                    {
                        let bundle = ids.of(w.target.pid).1.clone();
                        candidates.push((w, bundle));
                    }
                }
                candidates
            }
        })
        .await;
        if candidates.is_empty() {
            // Another session's window, or one on a Review Display:
            // only the user gives it (ADR 0017).
            if let Some((theirs, held)) = held_window(core, id, wanted, matcher.clone(), cx).await {
                return take_held(core, id, theirs, held, words, timeout, asked, cx).await;
            }
            // One the user minimized, or whose app they hid: the
            // same consent brings it out onto the display (#221).
            let away = off(cx, {
                let matcher = matcher.clone();
                move || put_away_candidates(wanted, matcher.as_ref())
            })
            .await;
            candidates = match away {
                Ok(away) => away,
                Err(refusal) => {
                    let name = app.clone().unwrap_or_else(|| "the".into());
                    return Err(ControlError::not_found(format!(
                        "could not bring the {name} window out: {}",
                        refusal.reason()
                    )));
                }
            };
        }
        if candidates.is_empty() {
            let text = app.clone().unwrap_or_default();
            let holders = core.read_with(cx, |c, _| Holders::of(c, id));
            let message = off(cx, move || match (wanted, named) {
                (Some(window), _) => no_such_window(window, &holders),
                (None, Some((named, found))) => no_app_window(id, &text, &named, found, &holders),
                (None, None) => unreachable!("window_id or app is required"),
            })
            .await;
            return Err(ControlError::not_found(message));
        }
        // The user's word names its window: the front one. A grant
        // takes the first window it covers, past one the user just
        // took back; with none, the user is asked for the front one.
        let pick = if words.is_some() {
            0
        } else {
            let keys: Vec<(WindowTarget, Option<String>)> = candidates
                .iter()
                .map(|(w, b)| (w.target, b.clone()))
                .collect();
            core.read_with(cx, |c, _| c.displays.grantable(id, &keys))
                .unwrap_or(0)
        };
        let (theirs, bundle_id) = candidates.into_iter().nth(pick).expect("a candidate");
        let target = theirs.target;
        let authority = match &words {
            Some(_) => Authority::Once,
            None => Authority::Grant { bundle_id },
        };
        let allowed = core.read_with(cx, |c, _| c.displays.may_hand_over(id, target, &authority));
        let mut reply = match allowed {
            Ok(()) => hand_over(core, id, theirs, authority, words, cx).await?,
            // Not covered by the grant: the user answers on a card.
            Err(Refusal::NotGranted | Refusal::Excluded) if words.is_none() => {
                request_window(core, id, theirs, None, timeout, cx).await?
            }
            // A stray another session's display records, on the
            // user's screen: the user decides (ADR 0017).
            Err(Refusal::Elsewhere) => {
                let held = core.read_with(cx, |c, _| {
                    c.displays.held_by_at(id, target, None, c.elapsed_ms())
                });
                return match held {
                    Some(held) => {
                        take_held(core, id, theirs, held, words, timeout, asked, cx).await
                    }
                    None => Err(ControlError::failed(
                        Refusal::Elsewhere.message(&theirs.app),
                    )),
                };
            }
            Err(Refusal::Takeover) => {
                return Err(ControlError::takeover(
                    Refusal::Takeover.message(&theirs.app),
                ));
            }
            Err(refusal) => {
                return Err(ControlError::failed(refusal.message(&theirs.app)));
            }
        };
        reply.extend(frame_window(core, id, target, asked, cx).await?);
        return Ok(reply);
    };
    // A window this display recorded before keeps that record if
    // it can't be placed: it stays a stray either way.
    let before = core.read_with(cx, |c, _| {
        c.displays
            .get(id)
            .and_then(|d| d.records.get(&window.target.window_id).cloned())
    });
    // A stray the session's other display recorded goes over with
    // that record, so a Handover stays one (ADR 0016).
    let recorded_by = core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .find(|d| {
                d.id != id
                    && c.displays.same_session(id, d.id)
                    && d.records.contains_key(&window.target.window_id)
            })
            .map(|d| d.id)
    });
    let moved = match recorded_by {
        Some(from) => Some(
            core.update(cx, |c, cx| {
                cx.notify();
                c.displays.move_between(from, id, window.target, false)
            })
            .map_err(ControlError::failed)?,
        ),
        None => {
            claim(core, id, window.target, Origin::Placed, cx);
            None
        }
    };
    if let Err(e) = place_and_hold(window.target, id, cx).await {
        // Still a stray, so the agent can try again.
        core.update(cx, |c, cx| {
            match moved {
                Some(moved) => c.displays.undo_move(moved),
                None => c
                    .displays
                    .restore_record_on(id, window.target.window_id, before),
            }
            cx.notify();
        });
        return Err(ControlError::failed(format!(
            "could not move the window: {e}"
        )));
    }
    frame_window(core, id, window.target, asked, cx).await
}

async fn window_state_action(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let scope = read_scope(p)?;
    let want_image = p.0.get("image") != Some(&Value::Bool(false));
    let id = display(core, p, false, cx)?;
    let window = window_on(id, p, cx).await?;
    let target = window.target;
    let root = read_root(core, id, target, scope.root, cx).await?;
    let area = cg::display_bounds(id);
    let origin = (area.x, area.y);
    let scale = display_scale(core, id, cx);
    let size = (window.bounds.width, window.bounds.height);
    // Walk the accessibility tree and capture the window picture in
    // parallel: both are read-only and run on background threads.
    let started = Instant::now();
    let walk_task = cx.background_executor().spawn(async move {
        let res = ax::snapshot(target, origin, scale, root.map(|r| r.0), scope.max_depth);
        (res, started.elapsed())
    });
    let shot_task = want_image.then(|| {
        cx.background_executor().spawn(async move {
            let shot = screenshot::window(target, size)?;
            Ok::<_, String>(encode_shot(shot))
        })
    });
    let ((snapshot, walked), shot) = match shot_task {
        Some(shot_task) => {
            let (walk, shot) = futures::future::join(walk_task, shot_task).await;
            (walk, Some(shot))
        }
        None => (walk_task.await, None),
    };
    // The window is there, so this isn't `not_found`, which
    // reads as closed: the app is busy, or AX is off.
    let snapshot = snapshot.map_err(|error| match error {
        ax::SnapshotError::WindowUnavailable => ControlError::failed(
            "the app didn't answer accessibility in time, or Accessibility isn't granted; call get_window_state again",
        ),
        ax::SnapshotError::RootUnavailable => ControlError::invalid(
            "the root element no longer belongs to this window; call get_window_state without root to find it again",
        ),
    })?;
    let count = snapshot.elements.len();
    let complete = scope.root.is_none() && !snapshot.truncated && !snapshot.depth_limited;
    let tree = core.update(cx, |c, _| {
        let numbers = c.elements.entry((id, window.target.window_id)).or_default();
        numbers::relabel(
            &snapshot.tree,
            &numbers.refresh(snapshot.elements, complete),
        )
    });
    let mut result = match shot {
        Some(shot) => shot.map_err(ControlError::failed)?.into_json(),
        None => json!({}),
    };
    log::debug!(
        "windowState {}: {count} elements in {} ms, picture by {} ms",
        target.window_id,
        walked.as_millis(),
        started.elapsed().as_millis()
    );
    result["window"] = in_shot(window.json(), scale);
    result["tree"] = json!(tree);
    result["elements"] = json!(count);
    result["truncated"] = json!(snapshot.truncated);
    result["depthLimited"] = json!(snapshot.depth_limited);
    result["root"] = json!(scope.root);
    add_strayed(core, id, &mut result, cx);
    Ok(result)
}

async fn element_action(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Arm, ControlError> {
    let scope = read_scope(p)?;
    let id = input_display(core, p, cx).await?;
    let window = window_on(id, p, cx).await?;
    // Validate the requested result root before any action occurs.
    read_root(core, id, window.target, scope.root, cx).await?;
    let index = p
        .int("element")
        .ok_or_else(|| ControlError::invalid("element is required"))?;
    let element = core.read_with(cx, |c, _| {
        let numbers = c.elements.get(&(id, window.target.window_id));
        numbers.map(|n| (n.get(index as usize).cloned(), n.issued(index as usize)))
    });
    let element = match element {
        None => {
            return Err(ControlError::invalid(
                "call get_window_state for this window first",
            ));
        }
        Some((None, true)) => {
            return Err(ControlError::not_found(format!(
                "element {index} is no longer in the window; call get_window_state again"
            )));
        }
        Some((None, false)) => {
            return Err(ControlError::not_found(format!(
                "no element {index}; call get_window_state again"
            )));
        }
        Some((Some(element), _)) => element,
    };
    let current = ax::Sendable(element.clone());
    let target = window.target;
    match off(cx, move || ax::belongs(current.get(), target, true)).await {
        Some(true) => {}
        Some(false) => {
            return Err(ControlError::not_found(format!(
                "element {index} is no longer in this window; call get_window_state again"
            )));
        }
        None => {
            return Err(ControlError::failed(
                "the app didn't answer accessibility in time; it may be busy. Nothing was done; try again shortly",
            ));
        }
    }
    let action = p.string("action").unwrap_or_else(|| "press".into());
    // A menu bar title lists its items rather than opening a menu
    // on the user's own menu bar.
    if matches!(action.as_str(), "press" | "click" | "show_menu") {
        let item = ax::Sendable(element.clone());
        let menu = cx
            .background_executor()
            .spawn(async move { ax::menu_items(item.get(), 0) })
            .await;
        if let Some(menu) = menu {
            let tree = core.update(cx, |c, _| {
                let numbers = c.elements.entry((id, window.target.window_id)).or_default();
                numbers::relabel(&menu.tree, &numbers.extend(menu.elements))
            });
            return Ok(Arm::Early(reply(json!({ "menu": tree }))));
        }
    }
    // Beside the user's window of the same app, a menu would open
    // in their front app and take their keyboard (#110).
    let item = ax::Sendable(element.clone());
    if let Some(refusal) = shared_app::element_refusal(window.target, &action, &item, cx).await {
        return Err(refusal);
    }
    if action == "read_text" {
        let item = ax::Sendable(element.clone());
        let (value, selected) = cx
            .background_executor()
            .spawn(async move {
                let e = item.get();
                (
                    ax::string_attribute(e, "AXValue"),
                    ax::string_attribute(e, "AXSelectedText"),
                )
            })
            .await;
        if value.is_none() && selected.is_none() {
            return Err(ControlError::not_found("the element has no text to read"));
        }
        return Ok(Arm::Early(reply(
            json!({ "text": value, "selected": selected.filter(|s| !s.is_empty()) }),
        )));
    }
    // Electron takes a menu item pressed through accessibility for
    // its shortcut: AppKit runs the press with no current event, and
    // Electron reads only a mouse event as a click (#39). An item of
    // its menu bar goes through the Input Queue as a real click
    // while a text field has focus, where apps ignore shortcuts, or
    // when the agent asks with real_input.
    let real_input = match p.0.get("realInput") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err(ControlError::invalid("realInput must be true or false")),
    };
    if real_input.is_some() && action != "press" {
        return Err(ControlError::invalid(
            "real_input applies to press on a menu item of an Electron app's menu bar",
        ));
    }
    let menu_note = if action == "press" {
        let (item, target) = (ax::Sendable(element.clone()), window.target);
        let pid = target.pid;
        let (press, path) = off(cx, move || {
            let e = item.get();
            if ax::string_attribute(e, "AXRole").as_deref() != Some("AXMenuItem") {
                return (None, None);
            }
            let electron = app::is_electron(pid);
            let path = electron
                .then(|| ax::menu_bar_path(e))
                .flatten()
                .filter(|(owner, _)| *owner == pid)
                .map(|(_, path)| path);
            let enabled = ax::attribute(e, "AXEnabled")
                .and_then(|v| {
                    v.downcast_ref::<objc2_core_foundation::CFBoolean>()
                        .map(|b| b.as_bool())
                })
                .unwrap_or(true);
            let press = agent_screen_model::menu_press::Press {
                in_menu_bar: path.is_some() && enabled,
                apple_menu: path
                    .as_ref()
                    .and_then(|p| p.first())
                    .is_some_and(|(index, _)| *index == 0),
                electron,
                editing: electron && ax::editing(target),
                real_input,
            };
            (Some((press, app::is_vs_code_based(pid))), path)
        })
        .await;
        if let (Some((press, _)), Some(path)) = (press, &path)
            && press.route() == agent_screen_model::menu_press::Route::RealClick
        {
            let path = path.clone();
            return Ok(Arm::Early(
                queued_menu_press(core, connection, id, window.target, path, cx).await,
            ));
        }
        // While a real click on the item waits or runs, a press
        // through accessibility would choose it a second time.
        if let Some(path) = &path {
            let operation = menu_operation(window.target, path);
            let outstanding = core.read_with(cx, |c, _| {
                c.input_queue.owner(id, connection).is_ok_and(|owner| {
                    c.input_queue.queue.requests().any(|r| {
                        r.owner == owner && r.operation == operation && !r.status.terminal()
                    })
                })
            });
            if outstanding {
                return Err(ControlError::invalid(
                    "a real click on this menu item still waits for the user's approval or runs; use input_queue to withdraw it before pressing the item through accessibility",
                ));
            }
        }
        if real_input == Some(true) {
            return Err(ControlError::invalid(
                "real_input applies to press on an enabled menu item of an Electron app's own menu bar, not the Apple menu",
            ));
        }
        press
            .filter(|(press, _)| press.electron)
            .map(|(_, vs_code)| {
                if vs_code {
                    VS_CODE_MENU_NOTE
                } else {
                    ELECTRON_MENU_NOTE
                }
            })
    } else {
        None
    };
    // A menu bar command acts on the app's key window, the user's
    // while they work in another window of it (#110).
    if let Some(refusal) = shared_app::menu_bar_refusal(window.target, &action, &item, cx).await {
        return Err(refusal);
    }
    if matches!(action.as_str(), "focus" | "set_value" | "press") {
        core.update(cx, |c, _| c.forwarder(id))
            .remember_user_window(window.target.pid)
            .await;
    }
    let action_note = match act(
        core,
        id,
        window.target,
        &action,
        &element,
        p.string("value"),
        cx,
    )
    .await
    {
        // Only the user's front app refused it, before anything was sent:
        // it waits in the Input Queue as a click at the element's center
        // (#219). Element numbers go stale, so the click holds a point.
        Err(e) if e.code == "front_blocked" => {
            let params =
                match element_click(core, id, window.target, &action, &element, p, cx).await {
                    Ok(params) => params,
                    Err(None) => return Err(ControlError::failed(e.message)),
                    Err(Some(why)) => {
                        return Err(ControlError::failed(format!(
                            "{}. It can't wait for the user to leave the app either: {why}.",
                            e.message.trim_end_matches('.')
                        )));
                    }
                };
            return Ok(Arm::Early(
                super::front_queue::queue_if_blocked(
                    core,
                    connection,
                    "click",
                    &Args(&params),
                    Err(e),
                    Some(element_operation(p)),
                    cx,
                )
                .await,
            ));
        }
        other => other?,
    };
    // Typing after focusing a field, or pressing in a window, goes
    // to that window, as it would after a click there.
    if matches!(action.as_str(), "focus" | "set_value" | "press") {
        let forwarder = core.update(cx, |c, _| c.forwarder(id));
        forwarder.focus(window.target).await;
    }
    if let Some(note) = menu_note.map(String::from).or(action_note) {
        return Ok(Arm::Early(reply(json!({ "note": note }))));
    }
    Ok(Arm::Finish(Ok(json!({}))))
}

/// The windows a queued element action's click may go to (`element_click`):
/// None for an agent's own click, which goes wherever its point is.
pub(super) fn own_windows(p: &Args<'_>) -> Option<Vec<u32>> {
    let own = p.0.get("ownWindows")?.as_array()?;
    Some(
        own.iter()
            .filter_map(|w| w.as_u64().map(|w| w as u32))
            .collect(),
    )
}

/// What names an element action in the Input Queue, so the same call
/// again finds the click it waits as (`front_queue::rejoin`).
fn element_operation(p: &Args<'_>) -> String {
    let field = |key: &str| p.0.get(key).cloned().unwrap_or(Value::Null);
    let call = json!({
        "displayId": field("displayId"),
        "windowId": field("windowId"),
        "element": field("element"),
        "action": field("action"),
        "value": field("value"),
        "realInput": field("realInput"),
    });
    format!("elementAction {call}")
}

/// What is at an element's center: the window there, by the window its
/// panels belong to, must be the requested one or the element's own, as a
/// sheet of it, for a click to stand for its press. With the element's own
/// windows, which the click checks again as it goes out.
async fn element_center(
    element: &ax::Element,
    window: WindowTarget,
    center: Option<(f64, f64)>,
    cx: &mut AsyncApp,
) -> (Center, Vec<u32>) {
    let e = ax::Sendable(element.clone());
    let (at_center, enclosing) = off(cx, move || {
        (
            center.and_then(|c| crate::mac::input::host_at(c).map(|(w, _)| w.window_id)),
            ax::enclosing_window_id(e.get()),
        )
    })
    .await;
    let own: Vec<u32> = [Some(window.window_id), enclosing]
        .into_iter()
        .flatten()
        .collect();
    (Center::of(center.is_some(), at_center, &own), own)
}

/// Why a click at an element's center can't reach it.
fn unreachable_center(at: Center) -> &'static str {
    match at {
        Center::OffDisplay => {
            "its center is not on the display. Call scroll_into_view on it, then get_window_state, and try again"
        }
        Center::NoWindow => {
            "no window that takes a click is at its center. Take a screenshot and get_window_state before you retry"
        }
        Center::Covered | Center::Window => {
            "another window covers its center, and the click would land there. Move that window or close it, then try again"
        }
    }
}

/// The `click` call an element action stands for, to wait in the Input
/// Queue: at the element's center, in screenshot pixels, with its button
/// and count. The queue keeps the window at that point and runs the click
/// only while it is still there, so that window must be the element's
/// own (`Center`). Err(None) for an action that is no click, Err(Some)
/// with why the click can't reach the element.
async fn element_click(
    core: &Entity<Core>,
    id: DisplayId,
    window: WindowTarget,
    action: &str,
    element: &ax::Element,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Params, Option<String>> {
    let (button, count) = match action {
        "press" | "click" => ("left", 1),
        "double_click" => ("left", 2),
        "right_click" => ("right", 1),
        _ => return Err(None),
    };
    let e = ax::Sendable(element.clone());
    let bounds = cg::display_bounds(id);
    let center = off(cx, move || ax::frame(e.get()))
        .await
        .map(|f| f.mid())
        .filter(|c| bounds.contains(*c));
    let (at, own) = element_center(element, window, center, cx).await;
    let (Center::Window, Some((x, y))) = (at, center) else {
        return Err(Some(unreachable_center(at).to_string()));
    };
    let scale = display_scale(core, id, cx);
    let mut params = Params::new();
    params.insert("displayId".into(), json!(id));
    // Unrounded, so the point maps back to the element's center itself.
    params.insert("x".into(), json!((x - bounds.x) * scale));
    params.insert("y".into(), json!((y - bounds.y) * scale));
    params.insert("button".into(), json!(button));
    params.insert("count".into(), json!(count));
    // The queue keeps the window at the point only if it is one of these,
    // and the click goes out only to one of them (`own_windows`).
    params.insert("ownWindows".into(), json!(own));
    if let Some(key) = p.0.get("sessionKey") {
        params.insert("sessionKey".into(), key.clone());
    }
    Ok(params)
}

/// Methods that see or drive the display. The rest (ending a display,
/// asking for help, waiting for control) work without permissions.
fn needs_permissions(method: &str) -> bool {
    !matches!(
        method,
        "endDisplay"
            | "requestHelp"
            | "requestControl"
            | "withdrawControlRequest"
            | "waitForControl"
            | "setDisposition"
            | "windowAccess"
            | "listWindows"
            | "agentReady"
            | "reportContext"
            | "fileReport"
            | "inputQueue"
            | "cancelWaitForChange"
            // The way out of a permission macOS won't turn on.
            | "resetPermissions"
    ) && !method.starts_with("debug")
}

/// Fail with what the user has to do when a permission is missing, and put
/// the setup window in front of them once for each state.
async fn permitted(cx: &mut AsyncApp) -> Result<(), ControlError> {
    use std::sync::atomic::Ordering;
    // The permission check's last answer, at most 2 s old. When it says
    // something is missing, look again off the UI thread: the user may have
    // just granted it, and the look can cost a window list.
    let mut missing = crate::mac::permissions::last().missing();
    if !missing.is_empty() {
        missing = cx
            .background_executor()
            .spawn(async { crate::mac::permissions::status().missing() })
            .await;
    }
    let mut reopen = crate::mac::permissions::NEEDS_REOPEN.load(Ordering::Relaxed);
    // macOS can apply the grant a moment after the first probe failed: look
    // again before sending the agent to the user.
    if missing.is_empty() && reopen {
        let works = cx
            .background_executor()
            .spawn(async { crate::mac::screenshot::can_capture() })
            .await;
        if works {
            crate::mac::permissions::NEEDS_REOPEN.store(false, Ordering::Relaxed);
            if let Some(shell) = crate::ui::shell::current() {
                let _ = shell.update(cx, |shell, cx| shell.capture_works(cx));
            }
            reopen = false;
        }
    }
    if missing.is_empty() && !reopen {
        return Ok(());
    }
    if let Some(shell) = crate::ui::shell::current() {
        let state = (missing.clone(), reopen);
        let _ = shell.update(cx, |shell, cx| shell.setup_for_agent(state, cx));
    }
    if missing.is_empty() {
        return Err(ControlError {
            blocked: None,
            code: "permission",
            message: "The user turned on Screen Recording, and macOS applies it only once Agent Screen reopens. Agent Screen has put its Settings window on the user's screen: tell the user to press Reopen Agent Screen there, then retry.".into(),
        });
    }
    let names: Vec<&str> = missing.iter().map(|p| p.english()).collect();
    Err(ControlError {
        blocked: None,
        code: "permission",
        message: format!(
            "Agent Screen is missing the macOS {} permission, and only the user can grant it. \
             Agent Screen has put its Settings window on the user's screen (also reachable from its \
             menu bar icon). Tell the user to press Turn On… beside it in that window: Agent Screen \
             then appears in System Settings' list, where they switch it on. Opened directly, \
             System Settings may not list Agent Screen yet. Retry after that; until then every \
             call fails the same way.",
            names.join(" and ")
        ),
    })
}

/// A display the request's Agent Session may use for `method`: one it
/// owns, and still live unless the method is about ending. The request
/// names its session with `sessionKey`, so one connection serves several
/// sessions (ADR 0016). An MCP server built before that names none; then
/// it is the last session that called createDisplay on the connection.
pub(super) fn usable(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    id: DisplayId,
    method: &str,
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
    let named = p.string("sessionKey").filter(|k| !k.is_empty());
    core.read_with(cx, |c, _| {
        let Some(d) = c.displays.get(id) else {
            // Gone or never there: the method's own lookup says which.
            return Ok(());
        };
        let mine = named.as_ref().or_else(|| c.sessions.get(&connection));
        let owner = d.owner.as_ref().map(|o| &o.session_key);
        let own = mine.is_some() && owner == mine;
        // An ended display says why to anyone, so an agent whose client
        // restarted its MCP server learns its earlier session ended (#326).
        if let Some(reason) = d.ended
            && (!own || !matches!(method, "endDisplay" | "waitForControl"))
        {
            return Err(ended_error(id, reason, own));
        }
        if !own {
            return Err(ControlError::not_found(format!(
                "display {id} belongs to another session; call create_display to get yours"
            )));
        }
        Ok(())
    })
}

/// A method's result as a reply; `return reply(...)` leaves a method early.
fn reply(value: Value) -> Reply {
    Ok(match value {
        Value::Object(map) => map,
        _ => Map::new(),
    })
}

// MARK: Methods

async fn create_display(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let session_key = p
        .string("sessionKey")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ControlError::invalid("sessionKey is required"))?;
    if core.read_with(cx, |c, _| c.closed.contains(&connection)) {
        return Err(ControlError::failed("the connection closed"));
    }
    core.update(cx, |c, _| {
        c.sessions.insert(connection, session_key.clone());
    });
    let asked = (p.number("width"), p.number("height"));
    // Another display for the session, beside the ones it has (ADR 0016).
    let new = p.flag("new");
    let orphans = orphans_param(p);
    if !new && let Some(existing) = reuse(core, connection, &session_key, &orphans, asked, cx) {
        return Ok(existing);
    }
    if new && let Err(cap) = core.read_with(cx, |c, _| c.displays.sessions.may_add(&session_key)) {
        return Err(ControlError::failed(cap.message()));
    }
    let width = asked
        .0
        .unwrap_or(DEFAULT_SIZE.0)
        .clamp(640.0, 2560.0)
        .round();
    let height = asked
        .1
        .unwrap_or(DEFAULT_SIZE.1)
        .clamp(480.0, 1600.0)
        .round();
    let clamped =
        asked.0.is_some_and(|a| a.round() != width) || asked.1.is_some_and(|a| a.round() != height);
    // The model numbers a second display's title as it joins.
    let title = p.string("title").unwrap_or_else(|| "Untitled".into());
    let owner = Owner {
        session_key,
        client: p.string("client").unwrap_or_else(|| "Agent".into()),
        title,
        connection,
    };
    // Where the user's AeroSpace workspaces are, so the guard can put back
    // any the new display takes. With ignored-monitors it takes none.
    // Note where the user's workspaces are while no Agent Display can have
    // taken one. Whether AeroSpace leaves the new display alone shows only
    // once it exists.
    cx.background_executor()
        .spawn(async move {
            if !crate::mac::aerospace::ignores_now()
                && let Some(aerospace) = crate::mac::aerospace::AeroSpace::current()
            {
                aerospace.remember_before_first();
            }
        })
        .await;
    let started = Instant::now();
    // A spare of this size costs macOS nothing; a new display is a
    // reconfiguration every app handles, so agents get only so many.
    // A spare of another size takes this one: a mode change, where a new
    // display would add one more for every later display change to cost.
    let spare_ids = core.read_with(cx, |c, _| c.spare_ids());
    let spare_snap = off(cx, move || Core::probe_spares(&spare_ids)).await;
    let spare = match core.update(cx, |c, _| c.take_spare_with((width, height), &spare_snap)) {
        Some((handle, name, resize)) => {
            let others = core.read_with(cx, |c, _| c.virtual_ids());
            crate::mac::display::reuse_parked(handle, width as u32, height as u32, resize, others)
                .await
                .map(|handle| (handle, name))
        }
        None => None,
    };
    let (made, name) = match spare {
        Some((handle, name)) => (Some(handle), name),
        None => {
            // A new display landed on the user's screen, or parking kept
            // failing: another try would change the user's screen again.
            if let Some(why) = crate::mac::display::creation_refusal() {
                return Err(ControlError::failed(why));
            }
            if let Err(why) = core.update(cx, |c, _| c.budget_allows()) {
                return Err(ControlError::failed(why));
            }
            if let Err(wait) = core.update(cx, |c, _| c.may_make()) {
                return Err(ControlError::failed(format!(
                    "Agent Screen made many displays in the last minutes, and each one makes every app on this Mac reconfigure its screens. Keep using the display you have, or call create_display again in {} s.",
                    wait.as_secs().max(1)
                )));
            }
            core.update(cx, |c, _| c.count_made());
            // macOS takes 300 to 500 ms to make the display: off the main
            // thread, so Peek and the Viewer keep drawing meanwhile.
            let (serial, name) = core.update(cx, |c, _| c.next_name((width as u32, height as u32)));
            let others = core.read_with(cx, |c, _| c.virtual_ids());
            let made = cx
                .background_executor()
                .spawn(crate::mac::display::create_parked(
                    name.clone(),
                    width as u32,
                    height as u32,
                    serial,
                    others,
                ))
                .await;
            let made = match made {
                Ok(made) => Some(made),
                // A display landed on the user's screen, or would: the
                // agent hears not to retry.
                Err(Some(why)) => return Err(ControlError::failed(why)),
                Err(None) => None,
            };
            (made, name)
        }
    };
    // The session may have closed, got a display from a parallel call, or
    // reached its cap from one, while macOS made this one: then this one
    // goes.
    let key = owner.session_key.clone();
    let raced = core.read_with(cx, |c, _| {
        if c.closed.contains(&connection) {
            Some(Err(ControlError::failed("the connection closed")))
        } else if new {
            c.displays
                .sessions
                .may_add(&key)
                .err()
                .map(|cap| Err(ControlError::failed(cap.message())))
        } else {
            None
        }
    });
    if let Some(answer) = raced {
        drop(made);
        return answer;
    }
    // A parallel call made the session's display meanwhile: reuse it as a
    // call after it would, orphans and all.
    if !new && let Some(existing) = reuse(core, connection, &key, &orphans, asked, cx) {
        drop(made);
        return Ok(existing);
    }
    let reply = core
        .update(cx, |c, cx| {
            let id = c.adopt(made?, name, (width, height), Some(owner), cx)?;
            adopt_orphans(c, &key, id, &orphans);
            c.displays.get(id).map(|d| {
                let mut reply = describe(d, false);
                if clamped {
                    reply["note"] = json!(format!(
                        "sizes run from 640×480 to 2560×1600; made it {width}×{height}"
                    ));
                }
                reply
            })
        })
        .ok_or_else(|| {
            ControlError::failed(crate::mac::display::creation_refusal().unwrap_or_else(|| {
                "macOS could not create a virtual display at a verified safe position. No Agent Display was assigned.".into()
            }))
        })?;
    // Answer once the display is where it stays, so a first screenshot
    // shows only the display.
    let id = reply["displayId"].as_u64().unwrap_or_default() as DisplayId;
    let created = started.elapsed().as_millis();
    // Windows macOS put back on the display, of apps adopted above: claimed
    // before AeroSpace takes them for the user's, a second after they show.
    if !orphans.is_empty() {
        let started = core.read_with(cx, |c, _| {
            c.displays
                .get(id)
                .map(|d| d.started.clone())
                .unwrap_or_default()
        });
        off(cx, move || {
            for w in windows::on_display(id) {
                if started.contains(&w.target.pid) {
                    crate::mac::claims::claim(w.target.window_id, id);
                }
            }
        })
        .await;
    }
    // Adoption already verified the position; there is no timeout that
    // can turn a failed parking attempt into a successful reply.
    // Stock AeroSpace may show one of the user's workspaces on the new
    // display until the shell's guard moves it back; their windows must
    // not reach the agent.
    let tucked = started.elapsed().as_millis();
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    // Read AeroSpace's state now that the display exists: the user may have
    // started, quit or reconfigured it since Agent Screen last looked.
    let aerospace_takes = cx
        .background_executor()
        .spawn(async move {
            use crate::mac::aerospace;
            aerospace::ignoring_with_display().is_some_and(|i| i != aerospace::Ignoring::Yes)
        })
        .await;
    if let Some(screen) = screen.filter(|_| aerospace_takes) {
        for _ in 0..30 {
            let clear = cx
                .background_executor()
                .spawn({
                    let screen = screen.clone();
                    async move {
                        let Some(aerospace) = crate::mac::aerospace::AeroSpace::current() else {
                            return true;
                        };
                        aerospace
                            .visible_windows_on(&screen)
                            .ok()
                            .is_none_or(|windows| windows.is_empty())
                    }
                })
                .await;
            if clear {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
    }
    let on_new = off(cx, move || windows::on_display(id)).await;
    if !on_new.is_empty() {
        let targets: Vec<WindowTarget> = on_new.iter().map(|w| w.target).collect();
        let unowned = core.update(cx, |c, _| {
            c.displays.reconcile_new_display_windows(id, &targets)
        });
        if !unowned.is_empty() {
            off(cx, move || {
                for w in on_new {
                    if unowned.contains(&w.target) {
                        crate::core::evacuate_window_to_user(w.target, &w.app, id, "new display");
                    }
                }
            })
            .await;
        }
    }
    log::debug!(
        "createDisplay {id}: created {created} ms, parked {tucked} ms, clear of the user's workspaces {} ms",
        started.elapsed().as_millis()
    );
    Ok(reply)
}

/// createDisplay's `adoptGroups`: the apps the session started on displays
/// that ended when the app quit, by the display they were on. An MCP
/// server built before ADR 0016 sends only `adopt`, bare pids, which go to
/// the display this call makes or reuses.
fn orphans_param(p: &Args) -> Vec<Orphans> {
    let pids = |v: &Value| -> Vec<i32> {
        v.as_array()
            .map(|pids| {
                pids.iter()
                    .filter_map(Value::as_i64)
                    .filter_map(|pid| i32::try_from(pid).ok().filter(|p| *p > 0))
                    .collect()
            })
            .unwrap_or_default()
    };
    // `adoptGroups` names the display each group was on; `adopt` is the
    // same pids flat, for an app older than ADR 0016 (ADR 0005).
    if let Some(groups) = p.0.get("adoptGroups").and_then(Value::as_array) {
        return groups
            .iter()
            .filter_map(Value::as_object)
            .map(|group| Orphans {
                display: group
                    .get("display")
                    .and_then(Value::as_u64)
                    .and_then(|d| DisplayId::try_from(d).ok())
                    .unwrap_or_default(),
                pids: group.get("pids").map(pids).unwrap_or_default(),
            })
            .collect();
    }
    let bare = p.0.get("adopt").map(pids).unwrap_or_default();
    if bare.is_empty() {
        return Vec::new();
    }
    vec![Orphans {
        display: 0,
        pids: bare,
    }]
}

/// Give the session's orphaned apps to its displays (`session::adoption`):
/// each to the display it was on when that one was made again, otherwise
/// to `id`. A pid a display already holds, or one no longer running, stays
/// out. Orphans arrive only from the asking session's own MCP record, so
/// another session's apps never come here.
fn adopt_orphans(c: &mut Core, key: &str, id: DisplayId, orphans: &[Orphans]) {
    if orphans.is_empty() {
        return;
    }
    let live = c.displays.of_session(key);
    let given = {
        let displays = &c.displays;
        agent_screen_model::session::adoption(orphans, &live, id, |pid| {
            displays.list.iter().any(|d| d.started.contains(&pid))
                || unsafe { libc::kill(pid, 0) } != 0
        })
    };
    for (display, pid) in given {
        if let Some(d) = c.displays.get_mut(display) {
            log::info!("display {display}: adopts app {pid}, started on an earlier connection");
            d.started.insert(pid);
        }
    }
}

/// createDisplay without `new` for a session with a live display: its
/// latest, which adopts the session's orphans, with a note naming the
/// session's displays when it has several. None when it has none.
fn reuse(
    core: &Entity<Core>,
    connection: u64,
    key: &str,
    orphans: &[Orphans],
    asked: (Option<f64>, Option<f64>),
    cx: &mut AsyncApp,
) -> Option<Value> {
    let (mut existing, (w, h)) = core.update(cx, |c, cx| {
        let now_ms = c.elapsed_ms();
        let id = c.reuse_session_display(connection, key, now_ms)?;
        adopt_orphans(c, key, id, orphans);
        cx.notify();
        let d = c.displays.get(id)?;
        Some((describe(d, true), (d.width, d.height)))
    })?;
    let mut notes = Vec::new();
    if asked.0.is_some_and(|a| a.round() != w) || asked.1.is_some_and(|a| a.round() != h) {
        notes.push(format!(
            "this session already has a {w}×{h} display, reused as is; end_display first for another size, or pass new: true for another display"
        ));
    }
    let id = existing["displayId"].as_u64().unwrap_or_default() as DisplayId;
    if let Some(several) = core.read_with(cx, |c, _| session_displays(c, key, id)) {
        existing["displays"] = several.0;
        notes.push(several.1);
    }
    if !notes.is_empty() {
        existing["note"] = json!(notes.join(". "));
    }
    Some(existing)
}

/// The session's live displays and a note naming them, when it has more
/// than one: which one a create_display without `new` reused.
fn session_displays(c: &Core, key: &str, reused: DisplayId) -> Option<(Value, String)> {
    let ids = c.displays.of_session(key);
    if ids.len() < 2 {
        return None;
    }
    let list: Vec<Value> = ids
        .iter()
        .filter_map(|&id| c.displays.get(id))
        .map(|d| json!({ "displayId": d.id, "label": d.label() }))
        .collect();
    let named: Vec<String> = ids
        .iter()
        .filter_map(|&id| c.displays.get(id))
        .map(|d| format!("{} ({})", d.id, d.label()))
        .collect();
    Some((
        json!(list),
        format!(
            "this session has {} displays: {}. Reused {reused}, the latest; pass new: true for another one, and use each display_id for the display you mean",
            ids.len(),
            named.join(", ")
        ),
    ))
}

fn describe<H>(d: &crate::model::Display<H>, reused: bool) -> Value {
    let mut out = json!({
        "displayId": d.id,
        "width": d.width as i64,
        "height": d.height as i64,
        "scale": shot_scale((d.width, d.height)),
        "label": d.label(),
        "reused": reused,
        // This app takes `new` and `adoptGroups` (ADR 0016). An MCP server
        // refuses `new` against an app whose reply lacks it.
        "multiDisplay": true
    });
    if let Some(url) = crate::web::preview_url(d.id) {
        out["preview_url"] = json!(url);
    }
    out
}

struct EncodedShot {
    data: String,
    mime_type: &'static str,
    width: usize,
    height: usize,
}

impl EncodedShot {
    fn into_json(self) -> Value {
        json!({
            "data": self.data,
            "mimeType": self.mime_type,
            "width": self.width,
            "height": self.height,
        })
    }
}

fn encode_shot(shot: screenshot::Shot) -> EncodedShot {
    EncodedShot {
        data: base64::engine::general_purpose::STANDARD.encode(&shot.data),
        mime_type: shot.mime_type,
        width: shot.width,
        height: shot.height,
    }
}

/// Note the windows the user made in the display's apps, which the kept
/// catches left alone.
fn sync_user_made(c: &mut Core, id: DisplayId) {
    let made: Vec<u32> = c
        .kept_catches
        .iter()
        .filter(|(display, _)| *display == id)
        .flat_map(|(_, k)| k.skipped())
        .collect();
    if let Some(d) = c.displays.get_mut(id) {
        d.user_made.extend(made);
    }
}

/// Windows of the agent's apps left on the user's screen because both the
/// user and the agent had just acted, or neither, not reported before.
/// `placeable` says whether place_window takes it: it does for apps the
/// agent started; others need a handover.
fn undecided_strays(c: &mut Core, id: DisplayId) -> Vec<Value> {
    sync_user_made(c, id);
    let fresh: Vec<u32> = c
        .kept_catches
        .iter()
        .filter(|(display, _)| *display == id)
        .flat_map(|(_, k)| k.undecided())
        .chain(
            c.left_alone
                .iter()
                .filter(|(display, _)| *display == id)
                .map(|(_, w)| *w),
        )
        .filter(|w| !c.told.contains(&(id, *w)))
        .collect();
    c.left_alone.retain(|(display, _)| *display != id);
    // Most screenshots have nothing to report: skip the window list.
    if fresh.is_empty() {
        return Vec::new();
    }
    let Some(d) = c.displays.get(id) else {
        return Vec::new();
    };
    let shown = windows::all();
    let mut out = Vec::new();
    for window in fresh {
        if !c.told.insert((id, window)) {
            continue;
        }
        let Some(w) = shown.iter().find(|w| w.target.window_id == window) else {
            continue;
        };
        if d.returned.contains(&window) || d.user_made.contains(&window) {
            continue;
        }
        // Moved onto an Agent Display meanwhile, by a catch or a handover.
        if c.displays
            .list
            .iter()
            .any(|a| cg::display_bounds(a.id).contains(w.bounds.mid()))
        {
            continue;
        }
        out.push(json!({
            "windowId": window,
            "app": w.app,
            "placeable": d.is_stray(w.target, false),
        }));
    }
    out
}

/// Name in a screenshot's result the windows an action may have opened that
/// stayed on the user's screen.
fn add_strayed(core: &Entity<Core>, id: DisplayId, result: &mut Value, cx: &mut AsyncApp) {
    let strayed = core.update(cx, |c, _| undecided_strays(c, id));
    if !strayed.is_empty() {
        result["strayed"] = json!(strayed);
    }
    add_given(core, id, result, cx);
}

/// Name in `given`, once, the windows the user gave from this display to
/// another Agent Session since its session last heard (ADR 0017).
fn add_given(
    core: &Entity<Core>,
    id: DisplayId,
    result: &mut Value,
    cx: &mut AsyncApp,
) -> Option<Vec<(u32, String)>> {
    let given = core.update(cx, |c, _| c.displays.given_news(id));
    if given.is_empty() {
        return None;
    }
    result["given"] = json!(
        given
            .iter()
            .map(|(window, app)| json!({ "windowId": window, "app": app }))
            .collect::<Vec<_>>()
    );
    Some(given)
}

/// Whether a kept catch on the display left this window alone as nobody's
/// for sure.
fn undecided(c: &Core, id: DisplayId, window: u32) -> bool {
    c.kept_catches
        .iter()
        .any(|(display, k)| *display == id && k.undecided().contains(&window))
}

/// Whether a window that opened late is still the agent's to move: the
/// user hasn't taken its app back or made it themselves, and the catch
/// didn't leave it alone.
fn still_agents(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    cx: &mut AsyncApp,
) -> bool {
    core.update(cx, |c, _| {
        sync_user_made(c, id);
        !undecided(c, id, target.window_id)
            && c.displays
                .get(id)
                .is_some_and(|d| d.is_stray(target, false))
    })
}

/// Catch on display `to` the later windows of apps `pids`, which an ended
/// display of the session started and handed to `to` (ADR 0016).
pub fn catch_again(core: Entity<Core>, to: DisplayId, pids: Vec<i32>, cx: &mut App) {
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(to, mtm));
    cx.spawn(async move |cx: &mut AsyncApp| {
        for pid in pids {
            let catch = crate::mac::catch::Catch::start(pid, to, screen.clone(), None).await;
            keep_catch(&core, to, catch, cx);
        }
    })
    .detach();
}

/// Keep the catch on an app this display's session started, for as long
/// as the display lives. A running app's catch ends here: its later windows
/// may be the user's.
fn keep_catch(
    core: &Entity<Core>,
    id: DisplayId,
    catch: Option<crate::mac::catch::Catch>,
    cx: &mut AsyncApp,
) {
    let Some(catch) = catch else { return };
    catch.keep();
    core.update(cx, |c, _| {
        let ours = c
            .displays
            .get(id)
            .is_some_and(|d| d.started.contains(&catch.pid()));
        let kept = c.kept_catches.iter().any(|(_, k)| k.pid() == catch.pid());
        if ours && !kept {
            c.kept_catches.push((id, catch));
        }
    });
}

/// Windows of the apps launch_app started for this display that sit on no
/// Agent Display and no display's records, such as one that opened on the
/// user's screen before its catch was ready.
async fn strays(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> Vec<windows::Window> {
    let (displays, spares) = core.read_with(cx, |c, _| {
        (
            c.displays.list.iter().map(|d| d.id).collect::<Vec<_>>(),
            c.spare_ids(),
        )
    });
    // AeroSpace may answer late: off the main thread.
    let off_displays: Vec<(windows::Window, bool, crate::model::DisplayRole)> =
        off(cx, move || {
            let placed = windows::kept_on(&displays);
            windows::all()
                .into_iter()
                .filter(|w| w.layer == 0)
                .map(|w| {
                    let held = placed.contains(&w.target.window_id);
                    let role = if windows::on_any(&w.bounds, &spares) {
                        crate::model::DisplayRole::User
                    } else {
                        windows::display_role(&w)
                    };
                    (w, held, role)
                })
                .collect()
        })
        .await;
    core.update(cx, |c, _| {
        let own = c
            .displays
            .get(id)
            .and_then(|d| d.session())
            .map(|key| c.displays.of_session(key))
            .unwrap_or_else(|| vec![id]);
        for &display in &own {
            sync_user_made(c, display);
        }
        // The session's own records count as its strays once off its
        // displays; another session's records don't (ADR 0016).
        let recorded: std::collections::HashSet<u32> = c
            .displays
            .list
            .iter()
            .filter(|d| {
                d.id != id
                    && !own.contains(&d.id)
                    && cg::window_display_role(d.id) == crate::model::DisplayRole::Agent
            })
            .flat_map(|d| d.records.keys().copied())
            .collect();
        if c.displays.get(id).is_none() {
            return Vec::new();
        }
        off_displays
            .into_iter()
            .filter(|(w, held, role)| {
                c.displays.is_session_stray(
                    id,
                    w.target,
                    recorded.contains(&w.target.window_id),
                    *role,
                    *held,
                )
            })
            .map(|(w, _, _)| w)
            .collect()
    })
}

/// Apps' names and bundle ids, looked up once per pid.
#[derive(Default)]
struct AppIds(std::collections::HashMap<i32, (Option<String>, Option<String>)>);

impl AppIds {
    fn of(&mut self, pid: i32) -> &(Option<String>, Option<String>) {
        self.0
            .entry(pid)
            .or_insert_with(|| (app::name(pid), app::bundle_id(pid)))
    }
}

/// Whether `w` is the window an agent named for place_window: by id, or
/// by its app's owner name, name or bundle id (#133).
fn is_named(
    w: &windows::Window,
    wanted: Option<i64>,
    named: Option<&NamedApp>,
    ids: &mut AppIds,
) -> bool {
    match (wanted, named) {
        (Some(wanted), _) => w.target.window_id as i64 == wanted,
        (None, Some(named)) => {
            named.is_owner(&w.app) || {
                let (name, bundle_id) = ids.of(w.target.pid);
                named.is_app(&w.app, name.as_deref(), bundle_id.as_deref())
            }
        }
        (None, None) => false,
    }
}

/// Who has each of this app's Agent Displays, and its Display Label, as the
/// session using display `asking` sees them, read on the main thread for a
/// `not_found` worked out off it.
struct Holders(std::collections::HashMap<DisplayId, (Holder, String)>);

impl Holders {
    fn of(core: &Core, asking: DisplayId) -> Holders {
        let now_ms = core.elapsed_ms();
        let session = core.displays.get(asking).and_then(|d| d.session());
        Holders(
            core.displays
                .list
                .iter()
                .map(|d| {
                    let label = core
                        .displays
                        .holder_label_at(d.id, now_ms)
                        .unwrap_or_else(|| d.label());
                    (d.id, (d.holder(session), label))
                })
                .collect(),
        )
    }

    /// Who has the Agent Display owning `window_id` at `bounds` under the
    /// centralized ownership rule (`effective_agent_display`, ADR 0018), and
    /// its label when known or derivable from a cross-instance `0x5353` serial
    /// (#189, L10). Blocking.
    fn at_named(&self, window_id: u32, bounds: &Rect) -> (Holder, Option<String>) {
        let online = windows::online_display_areas();
        let recorded = windows::recorded_owner(window_id);
        let Some((id, _)) =
            agent_screen_model::arrangement::effective_agent_display(bounds, &online, recorded)
        else {
            return (Holder::Other, None);
        };
        if let Some((h, label)) = self.0.get(&id) {
            return (*h, Some(label.clone()));
        }
        if cg::is_agent_screen(id) {
            let serial = cg::serial(id);
            let label = if serial > 0 {
                format!("display {id} [serial {serial}, another Agent Screen instance]")
            } else {
                format!("display {id} [another Agent Screen instance]")
            };
            return (Holder::Other, Some(label));
        }
        (Holder::Other, None)
    }
}

/// The user's document windows that match and that place_window can bring
/// out onto a display: minimized ones, where they would show again on the
/// user's screen (#221), the oldest first. Those whose app may not hide
/// unseen are left out; when that leaves none, why. Blocking.
fn put_away_candidates(
    wanted: Option<i64>,
    matcher: Option<&NamedApp>,
) -> Result<Vec<(windows::Window, Option<String>)>, BringOutRefusal> {
    let mut ids = AppIds::default();
    let away = windows::put_away(|w| {
        w.layer == 0
            && windows::display_role(w).accepts_window_request()
            && is_named(w, wanted, matcher, &mut ids)
    });
    let keys: Vec<(u32, PutAway)> = away
        .iter()
        .map(|(w, why)| (w.target.window_id, *why))
        .collect();
    let mut refused = None;
    let mut states: std::collections::HashMap<i32, Result<(), BringOutRefusal>> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    for i in agent_screen_model::bring_out::preference(&keys) {
        let w = &away[i].0;
        let allowed = *states.entry(w.target.pid).or_insert_with(|| {
            agent_screen_model::bring_out::may_bring_out(true, &windows::app_state(w.target, None))
        });
        match allowed {
            Ok(()) => out.push((w.clone(), ids.of(w.target.pid).1.clone())),
            Err(refusal) => refused = refused.or(Some(refusal)),
        }
    }
    match refused {
        Some(refusal) if out.is_empty() => Err(refusal),
        _ => Ok(out),
    }
}

/// Why the off-screen windows `away` returned aren't on screen: the most
/// telling reason among them.
fn why_off_screen(away: &[(windows::Window, PutAway)]) -> Option<PutAway> {
    away.iter()
        .map(|(_, why)| *why)
        .min_by_key(|why| why.rank())
}

/// place_window's `not_found` for a window id that is no stray and no
/// window of the user's on screen: where the window is. It names the app
/// only for a window on the user's screen. Blocking.
fn no_such_window(id: i64, holders: &Holders) -> String {
    let on_screen = windows::all()
        .into_iter()
        .find(|w| w.target.window_id as i64 == id);
    let mut label = None;
    let sighting = match on_screen {
        Some(w) if w.layer != 0 && windows::display_role(&w).accepts_window_request() => {
            WindowSighting::NotDocument { app: w.app }
        }
        Some(w) if !windows::display_role(&w).accepts_window_request() => {
            let (holder, name) = holders.at_named(w.target.window_id, &w.bounds);
            label = name;
            WindowSighting::OnAgentDisplay(holder)
        }
        // It left the list between the two reads.
        Some(_) => WindowSighting::OffScreen(PutAway::Unknown),
        None => match cg::on_screen_windows()
            .into_iter()
            .find(|w| w.target.window_id as i64 == id)
        {
            // On screen, but a window place_window never offers: a menu,
            // a floating window above the panel levels, a small one, or
            // Agent Screen's own.
            Some(w) => {
                let window = windows::Window {
                    target: w.target,
                    app: w.owner_name,
                    title: w.title,
                    bounds: w.bounds,
                    layer: w.layer,
                    off_display: false,
                };
                if windows::display_role(&window).accepts_window_request() {
                    WindowSighting::NotDocument { app: window.app }
                } else {
                    let (holder, name) = holders.at_named(window.target.window_id, &window.bounds);
                    label = name;
                    WindowSighting::OnAgentDisplay(holder)
                }
            }
            None if u32::try_from(id).is_ok_and(|i| cg::all_ids().contains(&i)) => {
                let away = windows::put_away(|w| w.target.window_id as i64 == id);
                WindowSighting::OffScreen(why_off_screen(&away).unwrap_or(PutAway::Unknown))
            }
            None => WindowSighting::Gone,
        },
    };
    placement::no_window_named(id, &sighting, label.as_deref())
}

/// place_window's `not_found` for an app no stray and no window of the
/// user's on screen matched: why its windows on screen weren't offered,
/// or what was searched. Blocking.
fn no_app_window(
    display: DisplayId,
    text: &str,
    named: &NamedApp,
    found: Option<access::AppId>,
    holders: &Holders,
) -> String {
    let mut ids = AppIds::default();
    let mine: Vec<windows::Window> = windows::all()
        .into_iter()
        .filter(|w| is_named(w, None, Some(named), &mut ids))
        .collect();
    let online = windows::online_display_areas();
    // A document window that matched yet wasn't offered is on an Agent
    // Display.
    let documents: Vec<&windows::Window> = mine
        .iter()
        .filter(|w| w.layer == 0 && !windows::display_role(w).accepts_window_request())
        .collect();
    let mut holder_labels: Option<(String, usize)> = None;
    let sighting = if let Some(here) = documents.iter().find(|w| {
        agent_screen_model::arrangement::effective_agent_display(
            &w.bounds,
            &online,
            windows::recorded_owner(w.target.window_id),
        )
        .is_some_and(|(owner, _)| owner == display)
    }) {
        AppSighting::OnThisDisplay {
            window_id: here.target.window_id,
        }
    } else if let Some(w) = mine
        .iter()
        .find(|w| windows::display_role(w).accepts_window_request())
    {
        AppSighting::OnlyPanels {
            name: w.app.clone(),
        }
    } else if !mine.is_empty() {
        let resolved: Vec<(Holder, Option<String>)> = mine
            .iter()
            .map(|w| holders.at_named(w.target.window_id, &w.bounds))
            .collect();
        holder_labels =
            placement::join_holder_labels(resolved.iter().filter_map(|(_, l)| l.as_deref()));
        AppSighting::OnAgentDisplay(Holder::of_all(resolved.into_iter().map(|(h, _)| h)))
    } else {
        // A window anywhere, hidden or on another Space, names its app by
        // its owner name, which the app's own name may not be.
        let running = found
            .as_ref()
            .filter(|f| app::is_running(&f.bundle_id))
            .map(|f| f.name.clone())
            .or_else(|| {
                cg::all_windows()
                    .into_iter()
                    .find(|w| named.is_owner(&w.owner_name))
                    .map(|w| w.owner_name)
            });
        // Its windows that exist off screen say why none is on it (#221).
        let away = running.as_ref().map(|_| {
            let mut ids = AppIds::default();
            windows::put_away(|w| w.layer == 0 && is_named(w, None, Some(named), &mut ids))
        });
        match (running, found) {
            (Some(name), _) => match away.as_deref().and_then(why_off_screen) {
                Some(why) => AppSighting::PutAway { name, why },
                None => AppSighting::NoWindow { name },
            },
            (None, Some(found)) => AppSighting::NotRunning { name: found.name },
            (None, None) => AppSighting::Unknown,
        }
    };
    placement::no_app_window_named(
        text,
        &sighting,
        holder_labels.as_ref().map(|(s, n)| (s.as_str(), *n)),
    )
}

/// Ask the user for one of their windows with a Window Request, and wait
/// for the answer. Granted, the window goes over as a Handover. `held`:
/// another Agent Session or a Review Display holds it, the card names the
/// holder, and Hand Over moves it from there as a Placement (ADR 0017).
async fn request_window(
    core: &Entity<Core>,
    id: DisplayId,
    window: windows::Window,
    held: Option<Held>,
    timeout: Option<f64>,
    cx: &mut AsyncApp,
) -> Result<Map<String, Value>, ControlError> {
    let app = window.app.clone();
    let mut held = held;
    // The user consents to the holder the card names. When the window has
    // moved to another holder by the time they answer, a new card names
    // that one (ADR 0017).
    for _ in 0..3 {
        match ask_window(core, id, window.clone(), held.clone(), timeout, cx).await? {
            Asked::Done(reply) => return Ok(reply),
            Asked::Again(now) => {
                log::info!(
                    "display {id}: window {} ({app}) is with {} now; asking the user again",
                    window.target.window_id,
                    now.label
                );
                held = Some(now);
            }
        }
    }
    Err(ControlError::failed(format!(
        "the {app} window kept moving between agent sessions while the user decided, so nothing moved. Call place_window again"
    )))
}

/// How one Window Request ended for the asker.
enum Asked {
    Done(Map<String, Value>),
    /// The user said Hand Over, but another holder has the window now: ask
    /// again, naming it.
    Again(Held),
}

/// Where a window place_window asked for is when the user answers.
enum Now {
    Gone,
    /// On the asking display already.
    Here,
    Held(Held),
    /// On the user's screen, and no other session holds it.
    Users(windows::Window),
    /// On one of the asker's own other displays, or a display no session
    /// uses.
    Unheld,
}

/// Where window `target` is now, as the session of display `id` sees it.
async fn where_now(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    cx: &mut AsyncApp,
) -> Now {
    let (known, spares): (Vec<DisplayId>, Vec<DisplayId>) = core.read_with(cx, |c, _| {
        (
            c.displays.list.iter().map(|d| d.id).collect(),
            c.spare_ids(),
        )
    });
    let window = target.window_id;
    let (on, found) = off(cx, move || {
        for o in known {
            if let Some(w) = windows::on_display(o)
                .into_iter()
                .find(|w| w.target.window_id == window)
            {
                return (Some(o), Some(w));
            }
        }
        let w = windows::all()
            .into_iter()
            .find(|w| w.target.window_id == window)
            .filter(|w| {
                windows::display_role(w).accepts_window_request()
                    || windows::on_any(&w.bounds, &spares)
            });
        (None, w)
    })
    .await;
    match (on, found) {
        (Some(o), _) if o == id => Now::Here,
        (Some(o), Some(_)) => core
            .read_with(cx, |c, _| {
                c.displays.held_by_at(id, target, Some(o), c.elapsed_ms())
            })
            .map_or(Now::Unheld, Now::Held),
        (None, Some(w)) => core
            .read_with(cx, |c, _| {
                c.displays.held_by_at(id, target, None, c.elapsed_ms())
            })
            .map_or(Now::Users(w), Now::Held),
        _ => Now::Gone,
    }
}

/// After the user's Hand Over: move the window from where it is now. The
/// holder the card named still has it: a Placement from there. The holder
/// let it go, as when its display ended and sent a Handover home: it is the
/// user's again, and goes over as their Handover, keeping its return
/// location. Another holder has it: ask again.
async fn after_hand_over(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    app: &str,
    named: Option<&Held>,
    cx: &mut AsyncApp,
) -> Result<Asked, ControlError> {
    let state = match where_now(core, id, target, cx).await {
        Now::Here => held::NowState::Here,
        Now::Held(now) => held::NowState::Held(now),
        Now::Users(window) => held::NowState::Users(window),
        Now::Unheld => held::NowState::Unheld,
        Now::Gone => held::NowState::Gone,
    };
    match held::decide_after_hand_over(state, named) {
        held::HandOverAction::AlreadyHere => Ok(Asked::Done(Map::new())),
        held::HandOverAction::Transfer(now) => transfer_window(core, &now, id, target, app, cx)
            .await
            .map(Asked::Done),
        held::HandOverAction::AskAgain(now) => Ok(Asked::Again(now)),
        held::HandOverAction::FromUser(window) => {
            hand_over(core, id, window, Authority::User, None, cx)
                .await
                .map(Asked::Done)
        }
        held::HandOverAction::MovedUnheld => Err(ControlError::failed(format!(
            "the {app} window moved to another display while the user decided, so nothing moved. Call list_windows on your displays, then place_window again"
        ))),
        held::HandOverAction::Closed => Err(ControlError::failed(format!(
            "the {app} window closed while the user decided"
        ))),
    }
}

/// One Window Request, and what came of the user's answer.
async fn ask_window(
    core: &Entity<Core>,
    id: DisplayId,
    window: windows::Window,
    held: Option<Held>,
    timeout: Option<f64>,
    cx: &mut AsyncApp,
) -> Result<Asked, ControlError> {
    let target = window.target;
    let app = window.app.clone();
    let named = NamedWindow {
        target,
        app: window.app,
        title: window.title,
    };
    let (request, answer) = core
        .update(cx, |c, cx| {
            let asked = c.displays.request_window(id, named, held.clone());
            cx.notify();
            asked
        })
        .ok_or_else(|| ControlError::failed(format!("display {id} has ended")))?;
    let seconds = timeout.unwrap_or(120.0).clamp(1.0, 600.0);
    let timer = cx
        .background_executor()
        .timer(Duration::from_secs_f64(seconds));
    let mut answer = answer.fuse();
    let resolution = futures::select! {
        resolution = answer => resolution.unwrap_or(Resolution::Ended),
        _ = FutureExt::fuse(timer) => {
            let timed_out = core.update(cx, |c, cx| {
                let timed_out = c.displays.time_out_window(id, request);
                if timed_out {
                    cx.notify();
                }
                timed_out
            });
            // An answer that came with the timer stands.
            if timed_out {
                Resolution::TimedOut
            } else {
                answer.await.unwrap_or(Resolution::Ended)
            }
        }
    };
    match (resolution, &held) {
        (Resolution::HandedOver(_), Some(held)) => {
            return after_hand_over(core, id, target, &app, Some(held), cx).await;
        }
        // The user's window, which another session took while they decided.
        (Resolution::Claimed, _) => {
            return after_hand_over(core, id, target, &app, None, cx).await;
        }
        (Resolution::HandedOver(_), None) => {}
        (Resolution::Declined, Some(held)) => {
            return Err(ControlError::failed(held::declined_message(&app, held)));
        }
        (Resolution::Declined, None) => {
            return Err(ControlError::failed(format!(
                "the user declined to hand over the {app} window; it stays theirs. Work without it, or launch the app yourself with launch_app"
            )));
        }
        (Resolution::TimedOut, _) => {
            return Err(ControlError::failed(held::timed_out_message(
                &app,
                seconds,
                held.as_ref(),
            )));
        }
        (Resolution::Replaced, _) => {
            return Err(ControlError::failed(format!(
                "your request for the {app} window was withdrawn: you asked for another window"
            )));
        }
        _ => {
            return Err(ControlError::failed(format!(
                "your request for the {app} window was withdrawn: the display ended"
            )));
        }
    }
    // The Handover moves it off the main thread; wait until it is there.
    for _ in 0..30 {
        let there = off(cx, move || {
            windows::on_display(id).iter().any(|w| w.target == target)
        })
        .await;
        if there {
            return Ok(Asked::Done(Map::from_iter([(
                "handedOver".into(),
                Value::Bool(true),
            )])));
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    Err(ControlError::failed(format!(
        "the user handed over the {app} window, but it didn't reach the display"
    )))
}

/// The window place_window names that another Agent Session holds on its
/// display, or that waits on a Review Display, with its holder: the first
/// match, display by display.
async fn held_window(
    core: &Entity<Core>,
    id: DisplayId,
    wanted: Option<i64>,
    matcher: Option<NamedApp>,
    cx: &mut AsyncApp,
) -> Option<(windows::Window, Held)> {
    let holders: Vec<DisplayId> = core.read_with(cx, |c, _| {
        let probe = WindowTarget {
            pid: 0,
            window_id: 0,
        };
        c.displays
            .list
            .iter()
            .filter(|d| c.displays.held_by(id, probe, Some(d.id)).is_some())
            .map(|d| d.id)
            .collect()
    });
    if holders.is_empty() {
        return None;
    }
    let found = off(cx, move || {
        let mut ids = AppIds::default();
        let mut found = Vec::new();
        for o in holders {
            for w in windows::on_display(o) {
                if (wanted.is_some() || w.layer == 0)
                    && is_named(&w, wanted, matcher.as_ref(), &mut ids)
                {
                    found.push((o, w));
                }
            }
        }
        found
    })
    .await;
    core.read_with(cx, |c, _| {
        let now_ms = c.elapsed_ms();
        found.into_iter().find_map(|(on, w)| {
            let held = c.displays.held_by_at(id, w.target, Some(on), now_ms)?;
            Some((w, held))
        })
    })
}

/// place_window for a window another session holds (ADR 0017). The user's
/// words relayed from the conversation move it at once only when they name
/// the window or its app; otherwise a Window Request naming the holder asks
/// the user. A Window Authorization never covers it.
#[allow(clippy::too_many_arguments)]
async fn take_held(
    core: &Entity<Core>,
    id: DisplayId,
    window: windows::Window,
    held: Held,
    words: Option<String>,
    timeout: Option<f64>,
    asked: FrameRequest,
    cx: &mut AsyncApp,
) -> Reply {
    let target = window.target;
    let app = window.app.clone();
    let named = match words.clone() {
        Some(text) => {
            let owner = window.app.clone();
            off(cx, move || {
                let name = app::name(target.pid).unwrap_or_default();
                let bundle = app::bundle_id(target.pid).unwrap_or_default();
                held::names_window(&text, &[&owner, &name, &bundle])
            })
            .await
        }
        None => false,
    };
    let mut reply = match words {
        Some(text) if named => {
            core.update(cx, |c, _| {
                if let Some(access) = c.displays.access_mut(id) {
                    access.last = Some(Instruction {
                        action: ChatAction::Place,
                        text: text.clone(),
                    });
                }
            });
            log::info!(
                "display {id}: window {} ({app}) taken from {} on the user's words {text:?}",
                target.window_id,
                held.label
            );
            let mut reply = transfer_window(core, &held, id, target, &app, cx).await?;
            reply.insert("by".into(), json!("chat"));
            reply
        }
        _ => request_window(core, id, window, Some(held), timeout, cx).await?,
    };
    reply.extend(frame_window(core, id, target, asked, cx).await?);
    Ok(reply)
}

/// Move a window another session holds onto display `to`, on the user's
/// answer or their words: the holder's records let it go and its session
/// hears it on its next call; a Handover the user gave the holder still
/// returns to the user (ADR 0017).
async fn transfer_window(
    core: &Entity<Core>,
    held: &Held,
    to: DisplayId,
    target: WindowTarget,
    app: &str,
    cx: &mut AsyncApp,
) -> Result<Map<String, Value>, ControlError> {
    let from = held.display;
    let moved = core
        .update(cx, |c, cx| {
            cx.notify();
            c.displays.transfer(from, to, target, app)
        })
        .map_err(ControlError::failed)?;
    let handed_over = moved.record.origin == Origin::HandedOver;
    if let Err(e) = place_and_hold(target, to, cx).await {
        core.update(cx, |c, cx| {
            c.displays.undo_transfer(moved);
            cx.notify();
        });
        return Err(ControlError::failed(format!(
            "the user gave you the {app} window, but it could not move from {}: {e}",
            held.label
        )));
    }
    core.update(cx, |c, _| c.displays.finish_transfer(&moved));
    // The window is the asker's now: the holder's drag in it, held or
    // queued, stops, as it would act on the window on the asker's display.
    release_hold_on(from, target.window_id);
    let stopped = core.update(cx, |c, cx| {
        let stopped = c.input_queue.window_given(from, target.window_id);
        cx.notify();
        stopped
    });
    if stopped > 0 {
        log::info!(
            "display {from}: {stopped} input request(s) on window {} stopped, the window given away",
            target.window_id
        );
    }
    log::info!(
        "window {} ({app}) moved from display {from} ({}) to display {to}, another session's, on the user's word",
        target.window_id,
        held.label
    );
    guard_placed(core, target, to, cx);
    let mut reply = Map::from_iter([("takenFrom".into(), json!(held.label))]);
    if handed_over {
        reply.insert("handedOver".into(), Value::Bool(true));
    }
    Ok(reply)
}

/// Hand one of the user's windows over on their word or under the
/// session's Window Access: claimed with the check in one step, checked
/// again right before it moves, and given up if a revoke, a Takeover, a
/// Take Back or the display ending came in between.
async fn hand_over(
    core: &Entity<Core>,
    id: DisplayId,
    window: windows::Window,
    authority: Authority,
    words: Option<String>,
    cx: &mut AsyncApp,
) -> Result<Map<String, Value>, ControlError> {
    let target = window.target;
    let app_name = window.app.clone();
    let title = window.title.clone();
    // A window off screen, minimized, comes home where it would have
    // shown, and back into the Dock (#221).
    let bounds = window.bounds;
    let by = match (&authority, &words) {
        (Authority::User, _) => "user",
        (_, Some(_)) => "chat",
        _ => "grant",
    };
    let home = off(cx, move || {
        crate::dismissal::home_of(target, None)
            .or_else(|| crate::dismissal::home_of(target, Some(bounds)))
    })
    .await;
    let from_dock = home.as_ref().is_some_and(|h| h.minimized);
    let ticket = core
        .update(cx, |c, cx| {
            let ticket = c.displays.hand_over(id, target, &authority, home)?;
            if let (Some(access), Some(text)) = (c.displays.access_mut(id), words.clone()) {
                access.last = Some(Instruction {
                    action: ChatAction::Place,
                    text,
                });
            }
            cx.notify();
            Ok::<_, Refusal>(ticket)
        })
        .map_err(|r| ControlError::failed(r.message(&app_name)))?;
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    // The last word before the move, on the main thread, where revokes,
    // Takeovers and Take Backs happen.
    let confirmed = core.update(cx, |c, _| {
        let confirmed = c.displays.confirm_hand_over(id, target, &ticket);
        if confirmed.is_err() {
            c.displays.abandon_hand_over(id, &ticket);
        }
        confirmed
    });
    // Read with the confirmation: a Take Back after it bumps the count,
    // and the move then stays home.
    let seen = windows::times_returned(target.window_id);
    if let Err(refusal) = confirmed {
        core.update(cx, |_, cx| cx.notify());
        return Err(match refusal {
            Refusal::Takeover => ControlError::takeover(refusal.message(&app_name)),
            _ => ControlError::failed(refusal.message(&app_name)),
        });
    }
    log::info!(
        "display {id}: window {} ({app_name}) handed over by {by}{}",
        target.window_id,
        words
            .as_deref()
            .map(|w| format!(" on the user's words {w:?}"))
            .unwrap_or_default()
    );
    let placed = off(cx, move || {
        windows::place_handed_over(target, id, screen.as_deref(), seen)
    })
    .await;
    if let Err(e) = placed {
        core.update(cx, |c, cx| {
            c.displays.abandon_hand_over(id, &ticket);
            cx.notify();
        });
        return Err(ControlError::failed(format!(
            "could not move the {app_name} window: {e}"
        )));
    }
    let landed = core.update(cx, |c, cx| {
        let landed = c.displays.commit_hand_over(id, &ticket, None);
        if landed {
            // The app's late windows are the agent's to have, not to send
            // back, as after the user's own Hand Over.
            c.returning
                .retain(|(d, k)| !(*d == id && k.pid() == target.pid));
            c.pending_returns.hand_over(id, target.pid);
        }
        cx.notify();
        landed
    });
    if !landed {
        return Err(ControlError::failed(format!(
            "the user took the {app_name} window back while it moved; it stays theirs"
        )));
    }
    guard_placed(core, target, id, cx);
    Ok(Map::from_iter([
        ("handedOver".into(), Value::Bool(true)),
        ("by".into(), json!(by)),
        // Which of the app's windows came, and from where.
        ("windowId".into(), json!(target.window_id)),
        ("title".into(), json!(title)),
        ("fromDock".into(), json!(from_dock)),
    ]))
}

/// requestControl on the user's word: hand the display back, or take it
/// over for them. The same actions as the Viewer's buttons.
fn chat_control(
    core: &Entity<Core>,
    id: DisplayId,
    words: &str,
    control: Option<&str>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let words = access::user_instruction(words).map_err(ControlError::invalid)?;
    let to_user = match control {
        None | Some("agent") => false,
        Some("user") => true,
        Some(_) => return Err(ControlError::invalid("control must be agent or user")),
    };
    let action = if to_user {
        ChatAction::Pause
    } else {
        ChatAction::HandBack
    };
    let changed = core.update(cx, |c, cx| {
        let changed = if to_user {
            c.take_over(id, cx)
        } else {
            c.hand_back(id, cx)
        };
        if let Some(access) = c.displays.access_mut(id) {
            access.last = Some(Instruction {
                action,
                text: words.clone(),
            });
        }
        cx.notify();
        changed
    });
    log::info!(
        "display {id}: {} on the user's words as the agent relayed them: {words:?} (changed: {changed})",
        action.as_str()
    );
    Ok(json!({
        "control": if to_user { "user" } else { "agent" },
        "changed": changed,
    }))
}

/// The session's Window Access as the agent and scripts read it from
/// display `id`, with that display's Handovers and pause.
fn access_status(c: &Core, id: DisplayId) -> Value {
    let none = agent_screen_model::access::Access::default();
    let access = c.displays.access(id).unwrap_or(&none);
    let Some(d) = c.displays.get(id) else {
        return json!({ "scope": "none", "apps": [], "paused": false, "excluded": [], "handovers": [], "lastInstruction": null });
    };
    let shown = windows::all();
    let handovers: Vec<Value> = d
        .handovers()
        .into_iter()
        .map(|t| {
            let w = shown.iter().find(|w| w.target.window_id == t.window_id);
            json!({
                "windowId": t.window_id,
                "app": w.map(|w| w.app.clone()),
                "title": w.map(|w| w.title.clone()),
            })
        })
        .collect();
    json!({
        "scope": access.scope_name(),
        "apps": access.apps().iter().map(|a| json!({ "bundleId": a.bundle_id, "name": a.name })).collect::<Vec<_>>(),
        "paused": d.access_paused(),
        "excluded": access.excluded,
        "handovers": handovers,
        "lastInstruction": access.last.as_ref().map(|i| json!({ "action": i.action.as_str(), "text": i.text })),
    })
}

/// window_access: grant, revoke, return or status, on the user's word.
async fn window_access(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = display(core, p, true, cx)?;
    let action = p
        .string("action")
        .ok_or_else(|| ControlError::invalid("action is required"))?;
    let words = p
        .string("userInstruction")
        .map(|w| access::user_instruction(&w))
        .transpose()
        .map_err(ControlError::invalid)?;
    let all = p.flag("all");
    let apps: Vec<String> =
        p.0.get("apps")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
    let needs_words = || {
        ControlError::invalid(format!(
            "{action} needs userInstruction: the user's words from the chat"
        ))
    };
    let mut extra = Map::new();
    match action.as_str() {
        "status" => {}
        "grant" => {
            let words = words.clone().ok_or_else(needs_words)?;
            let resolved = if all {
                None
            } else {
                if apps.is_empty() {
                    return Err(ControlError::invalid("grant needs apps or all"));
                }
                let names = apps.clone();
                let found = off(cx, move || {
                    names
                        .into_iter()
                        .map(|n| app::identify(&n).ok_or(n))
                        .collect::<Vec<_>>()
                })
                .await;
                let missing: Vec<String> = found.iter().filter_map(|f| f.clone().err()).collect();
                if !missing.is_empty() {
                    return Err(ControlError::not_found(format!(
                        "no app named {} on this Mac; nothing was granted. Use the app's name as the Dock shows it, or its bundle id",
                        missing.join(", ")
                    )));
                }
                Some(found.into_iter().filter_map(Result::ok).collect::<Vec<_>>())
            };
            let changed = core.update(cx, |c, cx| {
                let access = c.displays.access_mut(id)?;
                let before = access.scope.clone();
                match resolved.clone() {
                    None => access.grant_all(),
                    Some(list) => access.grant_apps(list),
                }
                access.last = Some(Instruction {
                    action: ChatAction::Grant,
                    text: words.clone(),
                });
                let changed = access.scope != before;
                cx.notify();
                Some(changed)
            });
            log::info!(
                "display {id}: window access granted for {} on the user's words as the agent relayed them: {words:?}",
                if all {
                    "all apps".to_string()
                } else {
                    apps.join(", ")
                }
            );
            extra.insert("changed".into(), json!(changed.unwrap_or(false)));
        }
        "revoke" => {
            let granted: Vec<(String, Option<String>)> = core.read_with(cx, |c, _| {
                let access = c.displays.access(id).cloned();
                apps.iter()
                    .map(|a| (a.clone(), access.as_ref().and_then(|x| x.granted_id(a))))
                    .collect()
            });
            // Names the grant doesn't hold by name: their bundle ids.
            let ids: Vec<String> = off(cx, move || {
                granted
                    .into_iter()
                    .map(|(name, id)| {
                        id.or_else(|| app::identify(&name).map(|a| a.bundle_id))
                            .unwrap_or(name)
                    })
                    .collect()
            })
            .await;
            let changed = core.update(cx, |c, cx| {
                let access = c
                    .displays
                    .access_mut(id)
                    .ok_or_else(|| ControlError::failed("the display has ended"))?;
                let changed = access
                    .revoke((!ids.is_empty()).then_some(ids.as_slice()))
                    .map_err(ControlError::invalid)?;
                if let Some(text) = words.clone() {
                    access.last = Some(Instruction {
                        action: ChatAction::Revoke,
                        text,
                    });
                }
                cx.notify();
                Ok::<_, ControlError>(changed)
            })?;
            log::info!("display {id}: window access revoked by the agent (changed: {changed})");
            extra.insert("changed".into(), json!(changed));
        }
        "return" => {
            let words = words.clone().ok_or_else(needs_words)?;
            let wanted: Vec<u32> = match p.int("windowId") {
                Some(w) if !all => vec![w as u32],
                None if all => core.read_with(cx, |c, _| {
                    c.displays
                        .get(id)
                        .map(|d| d.handovers().iter().map(|t| t.window_id).collect())
                        .unwrap_or_default()
                }),
                _ => {
                    return Err(ControlError::invalid(
                        "return needs windowId or all, one of them",
                    ));
                }
            };
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let mut returned = Vec::new();
            let mut stuck = Vec::new();
            // One at a time, each after the input queued before it.
            for window in wanted {
                let given = core.update(cx, |c, cx| {
                    let given = c.displays.return_handover(id, window);
                    if given.is_ok() {
                        if let Some(access) = c.displays.access_mut(id) {
                            access.last = Some(Instruction {
                                action: ChatAction::Return,
                                text: words.clone(),
                            });
                        }
                        cx.notify();
                    }
                    given
                });
                let given = match given {
                    Ok(given) => given,
                    Err(e) if !all => return Err(ControlError::not_found(e)),
                    Err(_) => continue,
                };
                let task = shell
                    .update(cx, |shell, cx| {
                        shell.send_home(given.target, id, Some(given), cx)
                    })
                    .map_err(|e| ControlError::failed(e.to_string()))?;
                if !task.await {
                    if !all {
                        return Err(ControlError::failed(format!(
                            "window {window} didn't leave your display, so it is still a Handover there. Tell the user; return can try again."
                        )));
                    }
                    stuck.push(window);
                    continue;
                }
                log::info!(
                    "display {id}: window {window} returned on the user's words as the agent relayed them: {words:?}"
                );
                returned.push(window);
            }
            extra.insert("returned".into(), json!(returned));
            if !stuck.is_empty() {
                extra.insert("stuck".into(), json!(stuck));
                // An older MCP passes this through beside its own note,
                // which says every window went back.
                extra.insert(
                    "warning".into(),
                    json!("The windows in stuck didn't leave your display and are still Handovers there. Tell the user; return can try again."),
                );
            }
        }
        other => {
            return Err(ControlError::invalid(format!(
                "action must be grant, revoke, return or status, not {other}"
            )));
        }
    }
    let mut status = core.read_with(cx, |c, _| access_status(c, id));
    if let Value::Object(map) = &mut status {
        map.extend(extra);
    }
    Ok(status)
}

/// Apply only an explicit frame request. The callback is the real AX writer
/// in frame_window and an offline capture in the geometry regression tests.
fn apply_frame_request(
    asked: FrameRequest,
    current: Rect,
    area: Rect,
    scale: f64,
    reserved_top: f64,
    set: impl FnOnce(Rect) -> Result<(), String>,
) -> Result<bool, String> {
    if asked.is_empty() {
        return Ok(false);
    }
    let frame = asked.resolve_with_top_inset(current, area, scale, reserved_top);
    set(frame)?;
    Ok(frame != asked.resolve(current, area, scale))
}

/// Give a window on display `id` the frame the agent asked for, if any, and
/// say where it ended up, in screenshot pixels: apps hold to their own
/// minimum and maximum sizes.
async fn frame_window(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    asked: FrameRequest,
    cx: &mut AsyncApp,
) -> Result<Map<String, Value>, ControlError> {
    let scale = display_scale(core, id, cx);
    // A window the user took back is theirs: no frame of the agent's
    // follows it home. A Take Back after this read bumps the count, and
    // the write in the window's lane then doesn't happen.
    let taken_back = core.read_with(cx, |c, _| {
        c.displays
            .get(id)
            .is_none_or(|d| d.returned.contains(&target.window_id))
    });
    if taken_back && !asked.is_empty() {
        return Err(ControlError::failed(
            "the user took the window back; it stays where they put it",
        ));
    }
    let seen = windows::times_returned(target.window_id);
    // Use the same main-thread policy as outside_menu_bar, not a second
    // hard-coded strip height. Empty frame requests still leave it unchanged.
    let reserved_top = crate::mac::menubar_height(id);
    let frame = off(cx, move || {
        let area = cg::display_bounds(id);
        let now = || windows::find(target.window_id).map(|w| w.bounds);
        let closed = || "the window closed".to_string();
        let mut bounds = now().ok_or_else(closed)?;
        let before = bounds;
        // The frame written, when the app answered the write with an error.
        let mut refused = None;
        let constrained = apply_frame_request(asked, bounds, area, scale, reserved_top, |frame| {
            windows::write_frame_unless_returned(target.window_id, seen, || {
                match ax::set_frame_asked(target, frame) {
                    Err(ax::MoveError::Rejected) => refused = Some(frame),
                    Err(e) => return Err(format!("could not set the window's frame: {e}")),
                    Ok(()) => {}
                }
                Ok(())
            })
        })?;
        if !asked.is_empty() {
            // Some apps, Electron's among them, answer a resize a moment
            // later: wait until two reads agree, up to half a second. An app
            // that answered with an error may be busy, with writes still
            // queued while its window stands still, so for it wait up to
            // 1.5 s while the window still has the frame it had.
            let reads = if refused.is_some() { 30 } else { 10 };
            let mut last = None;
            for _ in 0..reads {
                std::thread::sleep(Duration::from_millis(50));
                let read = now().ok_or_else(closed)?;
                let steady = last == Some(read);
                last = Some(read);
                let unchanged = refused.is_some_and(|frame| {
                    agent_screen_model::placement::frame_write(before, frame, read)
                        == agent_screen_model::placement::FrameWrite::Unchanged
                });
                if steady && !unchanged {
                    break;
                }
            }
            bounds = last.unwrap_or(bounds);
            let centre = (
                bounds.x + bounds.width / 2.0,
                bounds.y + bounds.height / 2.0,
            );
            if !area.contains(centre) {
                return Err(
                    "the app moved the window off the display after the frame was set".to_string(),
                );
            }
        }
        // An error from the app says only that some write failed: the frame
        // the window settled at says what it did (#167).
        if let Some(frame) = refused {
            let write = agent_screen_model::placement::frame_write(before, frame, bounds);
            log::info!(
                "window {}: the app answered a frame write with an error; {write:?}, at {bounds:?} for {frame:?}",
                target.window_id
            );
            if write == agent_screen_model::placement::FrameWrite::Unchanged {
                let kept = frame_in_shot(bounds, area, scale);
                return Err(format!(
                    "could not set the window's frame: the app rejected the change, and the window kept its frame x {}, y {}, width {}, height {}",
                    kept.x, kept.y, kept.width, kept.height
                ));
            }
        }
        Ok::<_, String>((frame_in_shot(bounds, area, scale), constrained))
    })
    .await;
    let (frame, constrained) = frame.map_err(ControlError::failed)?;
    Ok(Map::from_iter([
        ("frameConstrained".into(), json!(constrained)),
        (
            "window".into(),
            json!({
                "x": frame.x as i64,
                "y": frame.y as i64,
                "width": frame.width as i64,
                "height": frame.height as i64,
            }),
        ),
    ]))
}

/// Move a window onto display `id` and keep it there. AeroSpace can lay
/// the window out again in the user's workspace right after a display
/// appears (`windows::kept_on`), and an app can put it back on its own
/// screen: watch it for 1.5 s and move it back each time. Done once it has
/// stayed 0.5 s since the last move (#47).
async fn place_and_hold(
    target: WindowTarget,
    id: DisplayId,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let mut outcome = place(target, id, cx).await;
    let mut moves = 1;
    let mut checks = 0;
    let mut stayed = 0;
    while outcome.is_ok() && !(checks >= 6 && stayed >= 2) {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        checks += 1;
        if held(target, id, cx).await {
            stayed += 1;
            continue;
        }
        if moves == 4 {
            return Err(
                "it kept going back to the user's screen, moved there by its app or by AeroSpace"
                    .into(),
            );
        }
        log::info!(
            "window {} left display {id} after place_window; placing again",
            target.window_id
        );
        moves += 1;
        stayed = 0;
        outcome = place(target, id, cx).await;
    }
    outcome
}

/// place_window for a window on `from`, another live display of the same
/// Agent Session: a Placement between the session's own displays, never a
/// Window Request. The window's record goes with it, so a Handover stays a
/// Handover and still goes home when its new display ends (ADR 0016).
async fn move_between_own(
    core: &Entity<Core>,
    from: DisplayId,
    to: DisplayId,
    target: WindowTarget,
    asked: FrameRequest,
    cx: &mut AsyncApp,
) -> Reply {
    if core.read_with(cx, |c, _| c.displays.get(from).is_some_and(|d| d.takeover)) {
        return Err(ControlError::takeover(format!(
            "window {} is on your display {from}, which the user is operating. Call wait_for_control on display {from}, or ask the user to hand it back",
            target.window_id
        )));
    }
    let moved = core
        .update(cx, |c, cx| {
            cx.notify();
            c.displays.move_between(from, to, target, true)
        })
        .map_err(ControlError::failed)?;
    if let Err(e) = place_and_hold(target, to, cx).await {
        core.update(cx, |c, cx| {
            c.displays.undo_move(moved);
            cx.notify();
        });
        return Err(ControlError::failed(format!(
            "could not move window {} from display {from}: {e}",
            target.window_id
        )));
    }
    log::info!(
        "window {} moved from display {from} to display {to}, both of one session",
        target.window_id
    );
    guard_placed(core, target, to, cx);
    let mut reply = frame_window(core, to, target, asked, cx).await?;
    reply.insert("movedFrom".into(), json!(from));
    Ok(reply)
}

/// Record a window Agent Screen put on a display.
fn claim(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    origin: Origin,
    cx: &mut AsyncApp,
) {
    core.update(cx, |c, cx| {
        if c.displays.get(id).is_some() {
            c.displays.claim_on(id, target, origin, None);
            cx.notify();
        }
    });
}

/// Refuse agent pointer input at `at` on `id` if the visibly topmost window
/// there is recorded on another display, naming that holder instead of
/// operating its window or clicking through to a hidden window below (`M2`).
async fn guard_agent_point(
    core: &Entity<Core>,
    id: DisplayId,
    at: (f64, f64),
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
    let Some((target, owner_display)) =
        off(cx, move || crate::mac::input::foreign_window_at(id, at)).await
    else {
        return Ok(());
    };
    Err(foreign_point_error(core, id, target, owner_display, cx))
}

fn foreign_point_error(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    owner_display: DisplayId,
    cx: &AsyncApp,
) -> ControlError {
    let holder_label = core.read_with(cx, |c, _| {
        let now_ms = c.elapsed_ms();
        c.displays
            .held_by_at(id, target, Some(owner_display), now_ms)
            .map(|h| h.agent_label())
            .or_else(|| c.displays.holder_label_at(owner_display, now_ms))
    });
    ControlError::failed(held::foreign_top_window_message(
        id,
        owner_display,
        holder_label.as_deref(),
    ))
}

/// Move a window onto a display, off the main thread.
async fn held(target: WindowTarget, id: DisplayId, cx: &mut AsyncApp) -> bool {
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    cx.background_executor()
        .spawn(async move { windows::held(target, id, screen.as_deref()) })
        .await
}

/// Move a window onto a display for the agent, off the main thread. A
/// window the user took back stays where it went.
pub async fn place(target: WindowTarget, id: DisplayId, cx: &mut AsyncApp) -> Result<(), String> {
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    cx.background_executor()
        .spawn(async move { windows::place_for_agent(target, id, screen.as_deref()) })
        .await
}

/// One action on an accessibility element. AX actions reach background apps
/// without focusing them; clicks go through the input forwarder at the
/// element's center.
async fn act(
    core: &Entity<Core>,
    id: DisplayId,
    window: WindowTarget,
    action: &str,
    element: &ax::Element,
    value: Option<String>,
    cx: &mut AsyncApp,
) -> Result<Option<String>, ControlError> {
    let mut note = None;
    let bounds = cg::display_bounds(id);
    // Each AX call blocks until the app answers, up to the 1 s timeout,
    // and an action that opens a modal holds it all that time.
    let off_main = async |cx: &mut AsyncApp, work: Box<dyn FnOnce(&ax::Element) -> bool + Send>| {
        let element = ax::Sendable(element.clone());
        cx.background_executor()
            .spawn(async move { work(element.get()) })
            .await
    };
    let frame = {
        let element = ax::Sendable(element.clone());
        cx.background_executor()
            .spawn(async move { ax::frame(element.get()) })
            .await
    };
    let center = frame.map(|f| f.mid()).filter(|c| bounds.contains(*c));
    let perform = async |name: &'static str, cx: &mut AsyncApp| {
        if let Some(c) = center {
            show_pointer(core, id, c, true, cx);
        }
        off_main(cx, Box::new(move |e| ax::try_action(e, name))).await
    };
    // The window the click went to, if any.
    // `own`: the element's own windows, for a click that stands for its
    // press, which goes out only to one of them.
    let click = async |button: Button,
                       count: i64,
                       own: Option<Vec<u32>>,
                       cx: &mut AsyncApp|
           -> Result<Option<WindowTarget>, ControlError> {
        let c = center.ok_or_else(|| {
            ControlError::failed("the element has no visible frame on the display")
        })?;
        shared_app::posted_press_allowed(c, "click", cx).await?;
        let clicked = show_pointer(core, id, c, true, cx)
            .click_in(c, button, count, Default::default(), own, true)
            .await;
        if clicked.stopped {
            return Err(stopped_input(id, "the click", clicked.hit.is_some()));
        }
        match clicked.end() {
            // The user went back to their window of the app during the
            // wait before the press: nothing was sent, and it waits in the
            // Input Queue as a click (`element_action`).
            ClickEnd::Wait => Err(shared_app::took_focus(clicked.front_took.unwrap_or(window))),
            // A double click cut after its first press: final, never
            // queued, so it is never pressed more times than asked.
            ClickEnd::Partial => Err(shared_app::partial_click(count)),
            ClickEnd::Sent | ClickEnd::Nothing => Ok(clicked.hit),
        }
    };
    // A click that stands for a press: it goes out only while the window
    // at the element's center is the element's own, checked as the press
    // goes out, and None means nothing was sent.
    let press_click = async |cx: &mut AsyncApp| -> Result<(), ControlError> {
        let (_, own) = element_center(element, window, center, cx).await;
        if click(Button::Left, 1, Some(own), cx).await?.is_none() {
            return Err(ControlError::failed(
                "as the click went out, the window at the element's center was not the element's own, or there was none, so nothing was sent and the element was not pressed. Take a screenshot and get_window_state before you retry",
            ));
        }
        Ok(())
    };
    match action {
        "press" => {
            // Dia answers AXPress with success on an element that lists
            // none and does nothing (#219): such a press is a click.
            let listed = off_main(
                cx,
                Box::new(|e| ax::actions(e).iter().any(|a| a == "AXPress")),
            )
            .await;
            let (front, _) = shared_app::front(window, cx).await;
            let user_window = !listed
                && front == Front::FocusUnknown
                && shared_app::user_window(window.pid, cx).await;
            let at = match listed {
                true => Center::Window,
                false => element_center(element, window, center, cx).await.0,
            };
            match element_press(listed, front, user_window, at) {
                ElementPress::Ax => {
                    if !perform("AXPress", cx).await {
                        press_click(cx).await?;
                    }
                }
                ElementPress::PostedClick => {
                    press_click(cx).await?;
                    note = Some(UNLISTED_PRESS_NOTE.to_string());
                }
                ElementPress::WaitForFront => {
                    return Err(shared_app::unlisted_press(window, cx).await);
                }
                ElementPress::Refuse(at) => {
                    return Err(ControlError::failed(format!(
                        "the element lists no accessibility press, so it takes a click at its center, and nothing was sent: {}.",
                        unreachable_center(at)
                    )));
                }
            }
        }
        "click" => _ = click(Button::Left, 1, None, cx).await?,
        "double_click" => _ = click(Button::Left, 2, None, cx).await?,
        "right_click" => _ = click(Button::Right, 1, None, cx).await?,
        "show_menu" => {
            if !perform("AXShowMenu", cx).await {
                click(Button::Right, 1, None, cx).await?;
            }
        }
        "focus" => {
            let focused = off_main(
                cx,
                Box::new(|e| {
                    ax::set_attribute(e, "AXFocused", objc2_core_foundation::CFBoolean::new(true))
                }),
            )
            .await;
            if !focused {
                return Err(ControlError::failed("the element cannot take focus"));
            }
        }
        "set_value" => {
            let value =
                value.ok_or_else(|| ControlError::invalid("value is required for set_value"))?;
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            let mut access = AxValues {
                element,
                window,
                forwarder,
                bounds,
                id,
                cx,
            };
            note = super::values::set_value(&mut access, id, &value).await?;
        }
        "insert_text" => {
            let value =
                value.ok_or_else(|| ControlError::invalid("value is required for insert_text"))?;
            // Replacing the selection through accessibility is a paste that
            // needs no focus and no pasteboard.
            let set = off_main(
                cx,
                Box::new(move |e| {
                    let before = ax::string_attribute(e, "AXValue");
                    ax::settable(e, "AXSelectedText")
                        && ax::set_attribute(
                            e,
                            "AXSelectedText",
                            &objc2_core_foundation::CFString::from_str(&value),
                        )
                        && (value.is_empty()
                            || ax::string_attribute(e, "AXValue").is_none_or(|now| {
                                Some(&now) != before.as_ref() && now.contains(value.trim())
                            }))
                }),
            )
            .await;
            if !set {
                return Err(ControlError::failed(
                    "the element takes no inserted text; focus it and type instead",
                ));
            }
        }
        "increment" | "decrement" => {
            let name = if action == "increment" {
                "AXIncrement"
            } else {
                "AXDecrement"
            };
            let confirmed = off_main(cx, Box::new(move |e| ax::perform(e, name))).await;
            if !confirmed {
                return Err(ControlError::failed(format!(
                    "Accessibility did not confirm {action}. The control may not support it, or the app may have acted before its response timed out. Inspect the current value before retrying; no fallback click was sent."
                )));
            }
        }
        "scroll_into_view" => {
            if !perform("AXScrollToVisible", cx).await {
                return Err(ControlError::failed("the element cannot scroll into view"));
            }
        }
        other => return Err(ControlError::invalid(format!("unknown action {other}"))),
    }
    Ok(note)
}

/// The window an element is in, and its host process. Falls back to the
/// inspected `fallback` window when the element sits inside an `AXSheet` or a
/// hosted `.xpc`/`.appex` view whose `AXWindow` is unnumbered or off-screen
/// (#205).
fn popup_window(element: &ax::Element, fallback: WindowTarget) -> Option<WindowTarget> {
    let window_id = ax::enclosing_window_id(element)
        .or_else(|| ax::element_attribute(element, "AXWindow").and_then(|w| ax::window_id(&w)));
    if let Some(window_id) = window_id
        && cg::window_bounds(window_id).is_some()
        && let Some(pid) = ax::pid_of(element)
    {
        if pid != fallback.pid && cg::is_remote_view_service(pid) {
            return Some(fallback);
        }
        return Some(WindowTarget { pid, window_id });
    }
    if ax::belongs(element, fallback, false) == Some(true) {
        return Some(fallback);
    }
    None
}

/// set_value's AX and input calls on one element, each off the main
/// thread, as an app may take up to the 1 s timeout to answer.
struct AxValues<'a> {
    element: &'a ax::Element,
    window: WindowTarget,
    forwarder: crate::mac::input::Forwarder,
    bounds: Rect,
    id: DisplayId,
    cx: &'a mut AsyncApp,
}

impl super::values::ValueAccess for AxValues<'_> {
    async fn role(&mut self) -> Option<String> {
        let item = ax::Sendable(self.element.clone());
        off(self.cx, move || ax::string_attribute(item.get(), "AXRole")).await
    }

    fn stopped(&self) -> bool {
        self.forwarder.agent_stopped()
    }

    async fn popup_facts(&mut self) -> super::values::PopupFacts {
        let item = ax::Sendable(self.element.clone());
        let win_pid = self.window.pid;
        let mut facts = off(self.cx, move || {
            let elem_pid = ax::pid_of(item.get());
            super::values::PopupFacts {
                in_web_page: ax::in_web_area(item.get()),
                engine: elem_pid
                    .and_then(devtools::engine)
                    .or_else(|| devtools::engine(win_pid)),
                cocoa: elem_pid
                    .and_then(app::is_cocoa)
                    .or_else(|| app::is_cocoa(win_pid)),
                posts: true,
                shared: false,
                holds_menu: false,
            }
        })
        .await;
        // Only a select in a Chromium page reads these (`web_select`): a
        // native pop-up button gets no extra accessibility reads, as its
        // children or its app's focus, before its keys, which must keep
        // type-ahead's cadence.
        if facts.in_web_page == Some(true)
            && facts.engine == Some(agent_screen_model::devtools::Engine::Chromium)
        {
            // Whether keys typed into a select without the pipe reach the
            // agent's window only (#110).
            let (front, _) = shared_app::front(self.window, self.cx).await;
            let user_window =
                front == Front::FocusUnknown && shared_app::user_window(win_pid, self.cx).await;
            let item = ax::Sendable(self.element.clone());
            facts.posts = front.posts();
            facts.shared = front.shared(user_window);
            facts.holds_menu = off(self.cx, move || {
                crate::mac::shared_app::holds_menu(item.get())
            })
            .await;
        }
        facts
    }

    async fn choose_in_page(&mut self, title: &str, shown: &str) -> super::values::PageChoice {
        use super::values::PageChoice;
        use devtools::SelectChoice;
        let item = ax::Sendable(self.element.clone());
        let fallback = self.window;
        let title = title.to_owned();
        let shown = shown.to_owned();
        let forwarder = self.forwarder.clone();
        let within = self.bounds;
        off(self.cx, move || {
            let Some(target) = popup_window(item.get(), fallback) else {
                return PageChoice::Unpiped;
            };
            let (Some(bounds), Some(frame)) =
                (cg::window_bounds(target.window_id), ax::frame(item.get()))
            else {
                return PageChoice::Unpiped;
            };
            let at = frame.mid();
            if !within.contains(at) {
                return PageChoice::Unpiped;
            }
            match devtools::choose_option(target, bounds, at, &title, &shown, || {
                forwarder.agent_stopped()
            }) {
                SelectChoice::Unpiped => PageChoice::Unpiped,
                SelectChoice::NotSelect => PageChoice::NotSelect,
                SelectChoice::Elsewhere => PageChoice::Elsewhere,
                SelectChoice::Chosen => PageChoice::Chosen,
                SelectChoice::Already => PageChoice::Already,
                SelectChoice::Missing { shown, near } => PageChoice::Missing { shown, near },
                SelectChoice::Disabled => PageChoice::Disabled,
                SelectChoice::Unknown(why) => PageChoice::Unknown(why),
                SelectChoice::Stopped => PageChoice::Stopped,
            }
        })
        .await
    }

    async fn type_popup(&mut self, keys: &str) -> super::values::Typed {
        use super::values::Typed;
        use crate::mac::input::PopupTyped;
        let id = self.id;
        let fallback = self.window;
        let item = ax::Sendable(self.element.clone());
        let Some(target) = off(self.cx, move || popup_window(item.get(), fallback)).await else {
            return Typed::NotSent(ControlError::failed(
                "Accessibility names no window for the pop-up button, so nothing was typed. Refresh get_window_state and retry",
            ));
        };
        let typed = self
            .forwarder
            .choose_closed_popup(
                target,
                ax::Sendable(self.element.clone()),
                keys.to_owned(),
                self.bounds,
            )
            .await;
        match typed {
            PopupTyped::Typed => Typed::All,
            PopupTyped::Stopped(done) => Typed::Stopped(done),
            PopupTyped::Interrupted(done) => Typed::Interrupted(done),
            PopupTyped::Slow(done) => Typed::Slow(done),
            PopupTyped::Gone => Typed::NotSent(ControlError::failed(format!(
                "the pop-up button's window is no longer on display {id}, so nothing was typed. Call list_windows"
            ))),
            PopupTyped::Unfocused => Typed::NotSent(ControlError::failed(
                "the pop-up button didn't become its app's focused element, so keys could have gone to another control. Nothing was typed and no menu was opened. Refresh get_window_state and retry, or call request_help",
            )),
            PopupTyped::Spaced => Typed::NotSent(ControlError::invalid(
                "Agent Screen never types a space into a closed pop-up. Nothing was sent",
            )),
            PopupTyped::Refused => {
                let app = self
                    .forwarder
                    .take_refused()
                    .await
                    .map(|r| r.app)
                    .unwrap_or_else(|| "the app".into());
                Typed::NotSent(ControlError::failed(format!(
                    "{app}'s focused window is not on an Agent Display, so keys for the pop-up button could reach the user's window instead of yours on display {id}. Nothing was typed and no menu was opened. Click inside your window once and retry"
                )))
            }
        }
    }

    async fn set(&mut self, value: &str) -> super::values::Set {
        use super::values::{Set, unless_stopped};
        let item = ax::Sendable(self.element.clone());
        let value = value.to_owned();
        let forwarder = self.forwarder.clone();
        off(self.cx, move || {
            let stopped = || forwarder.agent_stopped();
            if stopped() {
                return Set::Stopped;
            }
            if !ax::settable(item.get(), "AXValue") {
                return Set::NotSettable;
            }
            match unless_stopped(stopped, || {
                ax::set_attribute(
                    item.get(),
                    "AXValue",
                    &objc2_core_foundation::CFString::from_str(&value),
                )
            }) {
                Some(true) => Set::Sent,
                Some(false) => Set::Refused,
                None => Set::Stopped,
            }
        })
        .await
    }

    async fn read(&mut self) -> Option<String> {
        let item = ax::Sendable(self.element.clone());
        off(self.cx, move || ax::string_attribute(item.get(), "AXValue")).await
    }

    async fn confirm(&mut self) -> super::values::Confirm {
        use super::values::{Confirm, unless_stopped};
        let item = ax::Sendable(self.element.clone());
        let forwarder = self.forwarder.clone();
        off(self.cx, move || {
            if !ax::actions(item.get()).iter().any(|a| a == "AXConfirm") {
                return Confirm::Unsupported;
            }
            match unless_stopped(
                || forwarder.agent_stopped(),
                || ax::try_action(item.get(), "AXConfirm"),
            ) {
                Some(true) => Confirm::Done,
                Some(false) => Confirm::Refused,
                None => Confirm::Stopped,
            }
        })
        .await
    }

    async fn pause(&mut self, duration: Duration) {
        self.cx.background_executor().timer(duration).await;
    }
}

/// Open an app in the background and place its new window on a display.
/// A URL with a scheme, or an existing file; `~/` means the home folder.
fn open_target(text: &str) -> Result<String, ControlError> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        return open_target(&format!("{home}/{rest}"));
    }
    if text.starts_with('/') {
        return if std::path::Path::new(text).exists() {
            Ok(text.to_string())
        } else {
            Err(ControlError::not_found(format!("no file at {text}")))
        };
    }
    let scheme = text.split_once(':').map(|(s, _)| s);
    if scheme.is_some_and(|s| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    }) {
        Ok(text.to_string())
    } else {
        Err(ControlError::invalid(
            "open takes a URL such as https://example.com or an absolute file path",
        ))
    }
}

/// Test builds may turn off the fallback for a browser that refuses the
/// agents' instance (ADR 0022), so a test beside the user's own browser
/// fails there, records nothing and never opens a window in theirs.
fn no_shared_fallback_for_tests() -> bool {
    cfg!(debug_assertions) && std::env::var("AGENTSCREEN_NO_SHARED_FALLBACK").as_deref() == Ok("1")
}

/// Test builds may hold the launch, so a test can start another copy of
/// the app while it waits (issue #35).
fn hold_launch_for_tests() {
    if cfg!(debug_assertions)
        && let Some(ms) = std::env::var("AGENTSCREEN_OPEN_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// `open -g`: without activating, like OpenConfiguration.activates = false.
async fn open_in_background(
    app: std::path::PathBuf,
    target: Option<String>,
    hidden: bool,
    start: &Start,
    cx: &mut AsyncApp,
) -> Option<std::sync::mpsc::Receiver<bool>> {
    let new_instance = start.new_instance();
    let arguments = start.open_arguments();
    let environment = start.environment.clone();
    cx.background_executor()
        .spawn(async move {
            hold_launch_for_tests();
            let mut open = std::process::Command::new("/usr/bin/open");
            open.arg("-g");
            if new_instance {
                open.arg("-n");
            }
            for (name, value) in &environment {
                open.arg("--env").arg(format!("{name}={value}"));
            }
            // A starting app draws its first window wherever macOS puts it,
            // on the user's screen; hidden, it draws nothing until moved.
            if hidden {
                open.arg("-j");
            }
            match target {
                Some(target) => open.arg("-a").arg(&app).arg(target),
                None => open.arg(&app),
            };
            open.arg("--args").args(&arguments);
            // `open` returns once the app has finished starting, a second
            // or more; the caller watches for the process meanwhile, and
            // hears whether `open` succeeded when it ends.
            let mut child = open.spawn().ok()?;
            let (done, outcome) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let ok = child.wait().is_ok_and(|s| s.success());
                if !ok {
                    log::warn!("open failed for {}", app.display());
                }
                let _ = done.send(ok);
            });
            Some(outcome)
        })
        .await
}

/// Wait, at most 10 s, until the browser window's page has loaded. A
/// window without a page, as another app's, returns at once.
async fn wait_for_page(target: WindowTarget, cx: &mut AsyncApp) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let page = cx
            .background_executor()
            .spawn(async move { ax::page(target) })
            .await;
        let done = match page {
            ax::Page::Loading => false,
            ax::Page::NoWindow | ax::Page::Blank => started.elapsed() > Duration::from_secs(3),
            ax::Page::Loaded | ax::Page::Unknown => true,
        };
        if done {
            log::debug!(
                "launch: page {page:?} at {} ms",
                started.elapsed().as_millis()
            );
            return;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    log::info!("launch: page still loading after 10 s");
}

/// Wait until `target`, a running browser's new window made at
/// `created`, shows the page the agent's URL opened: that URL, or any
/// address after the one it showed first, as a redirect to https or www
/// shows. A browser sent a URL with `open` opens it in its key window:
/// until the new window has it, handing key focus back to the user's
/// window could make the browser open it there (#110). After 2 s from the
/// window's start, the URL has gone out and a slow page doesn't need the
/// user's focus: whether the page came by then. Blocks.
fn shows_url(target: WindowTarget, url: &str, created: Instant) -> bool {
    // What the window showed first, as a new tab page, before the URL.
    let mut first = None;
    while created.elapsed() < Duration::from_secs(2) {
        if let Some(shown) = ax::web_url(target) {
            let first = first.get_or_insert_with(|| shown.clone());
            if agent_screen_model::launch::same_page(url, &shown) || shown != *first {
                log::debug!(
                    "launch: the new window shows {shown:?} at {} ms",
                    created.elapsed().as_millis()
                );
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    log::info!("launch: the new window didn't show the agent's page within 2 s");
    false
}

/// The user's window of app `pid` that has its key focus, when it is off
/// the Agent Displays: the one a new window of the agent's takes it from.
fn user_key_window(pid: i32) -> Option<WindowTarget> {
    let id = ax::focused_window_id(pid)?;
    let window = windows::find(id)?;
    (!crate::mac::input::on_agent_display_at(window.bounds)).then_some(window.target)
}

/// What `launch_app` did.
struct Launched {
    window: Option<windows::Window>,
    was_running: bool,
    /// Why the launch returned this window or none. For an app Agent Screen started:
    /// `still_starting`, none yet; one it opens soon moves onto the display.
    /// For a running app: `existing_window`, the file shows in a window it
    /// had; `still_opening`, the file's window hasn't come yet;
    /// `just_started`, the user or another session started it moments ago
    /// and it isn't ready; `no_new_window`, its menus have no New Window;
    /// `app_busy`, that item is greyed out; `menus_unreadable`, its menus
    /// can't be read or used. With `display_window`, a running app that
    /// couldn't open a new window opened the URL in a window this display holds.
    reason: Option<&'static str>,
    /// The process, when this launch started it.
    pid: Option<i32>,
    /// The window opened in the user's own process of a browser that
    /// refuses the agents' instance (ADR 0022).
    shared: bool,
}

/// How long a cold start's windows count as the agent's, and the launch
/// watches for them (ADR 0007).
const LATE_WINDOW_WAIT: Duration = agent_screen_model::new_window::COLD_START;

/// How long a running app may take to make the window Agent Screen asked
/// for. A window it makes later may be the user's.
const ASKED_WINDOW_WAIT: Duration = Duration::from_millis(3000);

/// How long a launch waits, from asking, for a busy running app to say
/// what the window it just made is (#270).
const UNDESCRIBED_WAIT: Duration = Duration::from_secs(5);

/// How long after a launch gave up a busy app's window is still moved to
/// the display once the app describes it, as ADR 0007's watch waits.
const UNDESCRIBED_ADOPT: Duration = Duration::from_secs(60);

/// What accessibility says, now, of the window busy app `pid` made for a
/// launch but couldn't describe then (#270): `known` by the number the app
/// gave, or else its one new window not in `before`, as
/// `launch::undescribed_window` picks it. Blocking.
enum Undescribed {
    /// The app doesn't answer yet.
    Busy,
    /// No window is surely the one: gone, held by a display, or one of
    /// several new.
    Unsure,
    Window {
        id: u32,
        described: bool,
    },
}

fn find_undescribed(
    pid: i32,
    known: Option<u32>,
    before: &std::collections::HashSet<u32>,
) -> Undescribed {
    let Some(windows) = ax::windows_quick(pid) else {
        return Undescribed::Busy;
    };
    let listed: Vec<_> = windows
        .iter()
        .map(|w| agent_screen_model::launch::Listed {
            window: w.id,
            before: before.contains(&w.id),
            claimed: crate::mac::claims::display_of(w.id).is_some(),
            on_agent_display: w.frame.is_some_and(crate::mac::input::on_agent_display_at),
        })
        .collect();
    match agent_screen_model::launch::undescribed_window(known, &listed) {
        Some(id) => Undescribed::Window {
            id,
            described: windows.iter().any(|w| w.id == id && w.described),
        },
        None => Undescribed::Unsure,
    }
}

/// Whether the user pressed something in app `pid` since the launch asked
/// at `asked_at`, or acted towards it just now (ADR 0007): then a window
/// it made is left where it is, as it may be theirs.
fn user_acted_in(pid: i32, asked_at: Instant) -> bool {
    crate::mac::catch::user_acted_since(pid, asked_at)
        || crate::mac::catch::user_acted_recently(pid)
}

/// Move the window app `pid` made that it couldn't describe before the
/// launch gave up onto display `id` once the app says it is a window,
/// within `UNDESCRIBED_ADOPT`. `window` by its number, or else found on the
/// first tick the app answers while no launch of it runs, as the one new
/// window not in `before`, which holds all the app had when the launch
/// asked at `asked_at`, on screen or not. Not if the user has acted in the
/// app since, nor once it is gone or isn't surely the one: then it stays
/// where it is (ADR 0007). Once it is placed, `user_window` gets its key
/// focus back.
#[allow(clippy::too_many_arguments)]
fn adopt_undescribed(
    core: Entity<Core>,
    id: DisplayId,
    pid: i32,
    path: std::path::PathBuf,
    window: Option<u32>,
    before: std::collections::HashSet<u32>,
    asked_at: Instant,
    user_window: Option<WindowTarget>,
    cx: &mut AsyncApp,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let started = Instant::now();
        let mut window = window;
        while started.elapsed() < UNDESCRIBED_ADOPT {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            if user_acted_in(pid, asked_at) {
                log::info!(
                    "launch: the user acted in pid {pid} before its window {window:?} could be described; left where it is"
                );
                return;
            }
            // Another launch's window could pass for the new one.
            if window.is_none() && core.update(cx, |c, _| c.launching.contains(&path)) {
                continue;
            }
            let before = before.clone();
            let known = window;
            match off(cx, move || find_undescribed(pid, known, &before)).await {
                Undescribed::Busy => continue,
                Undescribed::Unsure => {
                    log::info!(
                        "launch: no window of pid {pid} is surely the one it made, {window:?}; left where it is"
                    );
                    return;
                }
                Undescribed::Window { id: found, described } => {
                    window = Some(found);
                    if !described {
                        continue;
                    }
                    // The look may have waited on the app.
                    if user_acted_in(pid, asked_at) {
                        log::info!(
                            "launch: the user acted in pid {pid} as its window {found} was described; left where it is"
                        );
                        return;
                    }
                    let target = WindowTarget {
                        pid,
                        window_id: found,
                    };
                    let placed = settle(&core, target, id, cx).await;
                    if placed.is_ok() {
                        // Placing it made it key; the user's window gets
                        // its key focus back.
                        // The agent's keys since the launch used up the
                        // note of it.
                        let forwarder = core.update(cx, |c, _| c.forwarder(id));
                        if let Some(user) = user_window {
                            forwarder.remember_window(user).await;
                        }
                        forwarder.keys_done().await;
                    }
                    log::info!(
                        "launch: window {found} of pid {pid}, described late, moved onto display {id}: {}",
                        placed.is_ok()
                    );
                    return;
                }
            }
        }
        log::info!(
            "launch: window {window:?} of pid {pid} still undescribed after {} s; left where it is",
            UNDESCRIBED_ADOPT.as_secs()
        );
    })
    .detach();
}
/// The same, counted from when `open` returns: it returns once the app has
/// the file.
const OPENED_WINDOW_WAIT: Duration = Duration::from_millis(2000);
/// However long `open` takes, a running app's turn ends this long after
/// Agent Screen's request.
const RUNNING_APP_TURN: Duration = Duration::from_millis(4000);

/// RUNNING_APP_TURN, or in test builds AGENTSCREEN_RUNNING_TURN_MS, which
/// sends every file down the path of one that opens late.
fn running_app_turn() -> Duration {
    std::env::var("AGENTSCREEN_RUNNING_TURN_MS")
        .ok()
        .filter(|_| super::debug::enabled())
        .and_then(|ms| ms.parse().ok())
        .map_or(RUNNING_APP_TURN, Duration::from_millis)
}

/// Arguments and environment for a new instance of an app.
#[derive(Clone, Default)]
struct Start {
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    /// The agents' instance of a browser the user runs, which takes the
    /// new window (ADR 0022).
    agents: Option<AgentsInstance>,
    /// This start is the agents' instance's (ADR 0022), shared by every
    /// session: no display holds it as an app it started.
    agents_start: bool,
}

impl Start {
    fn parse(p: &Args) -> Result<Start, ControlError> {
        let arguments = match p.0.get("arguments") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .ok_or_else(|| ControlError::invalid("arguments are strings"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(ControlError::invalid("arguments is a list of strings")),
        };
        let environment = match p.0.get("environment") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(map)) => map
                .iter()
                .map(|(k, v)| {
                    v.as_str()
                        .map(|v| (k.clone(), v.to_string()))
                        .ok_or_else(|| ControlError::invalid("environment values are strings"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(ControlError::invalid(
                    "environment maps variable names to strings",
                ));
            }
        };
        if environment
            .iter()
            .any(|(k, _)| k.is_empty() || k.contains('='))
        {
            return Err(ControlError::invalid(
                "environment names are non-empty, without =",
            ));
        }
        Ok(Start {
            arguments,
            environment,
            agents: None,
            agents_start: false,
        })
    }

    /// What `open` passes after `--args`. A document app that starts
    /// with nothing to open shows an Open panel, owned by a system service
    /// and left on the user's screen, instead of a window: ask for an
    /// untitled document. Only a starting app reads its arguments. Nor
    /// bring back windows from its last run, unsaved text included, which
    /// may be the user's. Given arguments, the app gets exactly those:
    /// Electron takes the first one that isn't a flag for its app folder.
    fn open_arguments(&self) -> Vec<String> {
        if self.arguments.is_empty() {
            [
                "-NSShowAppCentricOpenPanelInsteadOfUntitledFile",
                "NO",
                "-ApplePersistenceIgnoreState",
                "YES",
            ]
            .map(String::from)
            .to_vec()
        } else {
            self.arguments.clone()
        }
    }

    /// A running instance would ignore both: a new one starts.
    fn new_instance(&self) -> bool {
        !self.arguments.is_empty() || !self.environment.is_empty()
    }

    /// The same start with `switches` ahead of the agent's arguments.
    fn with_switches(&self, switches: Vec<String>) -> Start {
        Start {
            arguments: switches
                .into_iter()
                .chain(self.arguments.iter().cloned())
                .collect(),
            environment: self.environment.clone(),
            agents: None,
            agents_start: false,
        }
    }
}

/// The agents' own instance of a Chromium browser (ADR 0022), running as
/// `pid` with Agent Screen's pipe.
#[derive(Clone)]
struct AgentsInstance {
    pid: i32,
}

/// How to start the browser at `path`: a Chromium browser the user already
/// runs gets the agents' instance (ADR 0022).
fn browser_plan(
    path: &std::path::Path,
    arguments: &[String],
) -> (
    agent_screen_model::agent_browser::Plan,
    Option<std::path::PathBuf>,
) {
    use agent_screen_model::agent_browser::{Plan, plan};
    let profile = (devtools::is_chromium(path) && app::handles_web(path))
        .then(|| app::agent_browser_profile(path))
        .flatten();
    let Some(profile) = profile else {
        return (Plan::AsAsked, None);
    };
    let running: Vec<(i32, Vec<String>)> = app::running_pids(path)
        .into_iter()
        .map(|pid| (pid, app::arguments(pid).unwrap_or_default()))
        .collect();
    let piped: Vec<i32> = running
        .iter()
        .map(|(pid, _)| *pid)
        .filter(|&pid| devtools::has_pipe(pid))
        .collect();
    let refused = refusal(path, &profile).is_some_and(|(_, known)| known);
    (
        plan(
            true,
            app::bundle_id_at(path)
                .is_some_and(|id| agent_screen_model::agent_browser::verified(&id)),
            arguments,
            &running,
            &piped,
            &profile,
            refused,
        ),
        Some(profile),
    )
}

/// The refusals file beside the agents' profile for the browser at `path`,
/// and whether it lists this version of the browser (ADR 0022).
fn refusal(
    path: &std::path::Path,
    profile: &std::path::Path,
) -> Option<(std::path::PathBuf, bool)> {
    use agent_screen_model::agent_browser::{REFUSALS, refuses};
    let file = profile.parent()?.join(REFUSALS);
    let record = std::fs::read_to_string(&file).unwrap_or_default();
    let known = refuses(
        &record,
        &app::bundle_id_at(path)?,
        &app::bundle_version_at(path).unwrap_or_default(),
    );
    Some((file, known))
}

/// Remember that the browser at `path` refused the agents' instance, so
/// later launches open in the user's process without trying again. The
/// profile folder goes to the Trash when this launch made it, since the
/// browser only wrote its first-run files there.
fn record_refusal(path: &std::path::Path, profile: &std::path::Path, made_profile: bool) {
    use agent_screen_model::agent_browser::with_refusal;
    if let (Some((file, _)), Some(id)) = (refusal(path, profile), app::bundle_id_at(path)) {
        let version = app::bundle_version_at(path).unwrap_or_default();
        let record = std::fs::read_to_string(&file).unwrap_or_default();
        if let Err(e) = std::fs::write(&file, with_refusal(&record, &id, &version)) {
            log::warn!("launch: couldn't record that {id} refuses a second instance: {e}");
        }
    }
    if made_profile && profile.exists() {
        match app::trash(profile) {
            Ok(()) => log::info!(
                "launch: moved the unused profile {} to the Trash",
                profile.display()
            ),
            Err(e) => log::warn!(
                "launch: couldn't move {} to the Trash: {e}",
                profile.display()
            ),
        }
    }
}

/// Ask the agents' instance for a new window on the display at `bounds`,
/// showing `url` if given, through its DevTools pipe, which opens it
/// without activating the browser. The page's target, and whether it went
/// into a window that was open already, as Dia puts it, which then is on
/// this display. A tab in a window elsewhere, such as another agent's
/// display, is closed, and so is the launch.
/// Where the page the agents' instance's pipe opened for a launch went.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PageWindow {
    /// A new window, which Chromium gives a frame.
    New,
    /// A tab in a window it had open on this display, with a frame.
    Open,
    /// None: the browser keeps its pages in windows Chromium gives no
    /// frame for, as Dia does. The page goes into the agents' window on this
    /// display through its address field.
    Frameless,
}

fn new_window_in(
    agents: &AgentsInstance,
    url: Option<&str>,
    bounds: crate::model::Rect,
) -> Result<(String, PageWindow), String> {
    use agent_screen_model::agent_browser::{TabLanding, tab_landing};
    let margin = agent_screen_model::launch::WINDOW_MARGIN;
    let channel = devtools::channel(agents.pid, Duration::from_secs(2)).ok_or_else(|| {
        format!(
            "(pid {}) doesn't answer on its pipe; call launch_app again",
            agents.pid
        )
    })?;
    match channel.new_window(url.unwrap_or("about:blank"), bounds.inset(margin, margin)) {
        Ok(devtools::NewPage::Window { target }) => Ok((target, PageWindow::New)),
        Ok(devtools::NewPage::Frameless) => Ok((String::new(), PageWindow::Frameless)),
        Ok(devtools::NewPage::Tab {
            target,
            bounds: window,
        }) => match tab_landing(window, bounds) {
            TabLanding::ThisDisplay => Ok((target, PageWindow::Open)),
            TabLanding::Elsewhere => {
                channel.close_target(&target);
                Err("opened the page as a tab in its window on another display, not in a new window, and the tab was closed; open the page in your window on this display with computer, or call launch_app again".into())
            }
        },
        Ok(devtools::NewPage::Unknown) => Err("didn't say where it opened the page".into()),
        Err(e) => Err(format!("made no window through its pipe: {e}")),
    }
}

/// The launch's window, fitted to display `id` through the agents'
/// instance's pipe when it reaches past the display.
async fn fit_agents_window(launched: Launched, id: DisplayId, cx: &AsyncApp) -> Launched {
    let Some(window) = launched.window.clone() else {
        return launched;
    };
    let display = cg::display_bounds(id);
    let size = (window.bounds.width, window.bounds.height);
    if !agent_screen_model::agent_browser::overflows(size, (display.width, display.height)) {
        return launched;
    }
    let (target, current) = (window.target, window.bounds);
    let margin = agent_screen_model::launch::WINDOW_MARGIN;
    let fitted = off(cx, move || {
        let set = devtools::channel(target.pid, Duration::from_secs(2))
            .is_some_and(|c| c.fit_window(current, display.inset(margin, margin)));
        log::info!(
            "launch: window {} of the agents' instance was {}x{}, past the display; fitted through its pipe: {set}",
            target.window_id,
            size.0,
            size.1
        );
        std::thread::sleep(Duration::from_millis(300));
        windows::on_display(id).into_iter().find(|w| w.target == target)
    })
    .await;
    Launched {
        window: fitted.or(Some(window)),
        ..launched
    }
}

/// Whether a process of the browser at `path` runs on a profile other than
/// the agents', as the user's does.
async fn users_run(path: &std::path::Path, profile: &std::path::Path, cx: &AsyncApp) -> bool {
    let (path, profile) = (path.to_path_buf(), profile.to_path_buf());
    off(cx, move || {
        app::running_pids(&path).into_iter().any(|pid| {
            !app::arguments(pid)
                .is_some_and(|argv| agent_screen_model::agent_browser::is_agents(&argv, &profile))
        })
    })
    .await
}

/// Whether a process of the browser at `path` runs on the agents' profile.
async fn agents_run(path: &std::path::Path, profile: &std::path::Path, cx: &AsyncApp) -> bool {
    let (path, profile) = (path.to_path_buf(), profile.to_path_buf());
    off(cx, move || {
        app::processes_of(&path).into_iter().any(|pid| {
            app::arguments(pid)
                .is_some_and(|argv| agent_screen_model::agent_browser::is_agents(&argv, &profile))
        })
    })
    .await
}

async fn launch(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    target: Option<&str>,
    start: &Start,
    cx: &mut AsyncApp,
) -> Result<Launched, ControlError> {
    let resolving = Instant::now();
    let asked = name.to_string();
    let path = off(cx, move || app::resolve(&asked)).await.ok_or_else(|| {
        ControlError::not_found(format!(
            "no app named {name}; give its bundle id, or the absolute path of its .app bundle if Launch Services doesn't list it, as for a build under /tmp"
        ))
    })?;
    // A plain file URL is its file, except for a browser, which opens it as
    // a page: then the URL route gives it a window of its own first. So
    // does a file path: a running browser opens one as a tab of its front
    // window, which may be the user's, and makes no window (#168).
    let browser = match target.filter(|t| t.starts_with("file://") || t.starts_with('/')) {
        Some(_) => {
            let path = path.clone();
            cx.background_executor()
                .spawn(async move { app::handles_web(&path) })
                .await
        }
        None => false,
    };
    let file = file_target(target, browser)?;
    let target = file.as_deref().or(target);
    log::debug!(
        "launch: {name} found in {} ms",
        resolving.elapsed().as_millis()
    );
    // Wait for another session's launch of the same app to finish.
    let mut free = false;
    for _ in 0..450 {
        free = core.update(cx, |c, _| c.launching.insert(path.clone()));
        if free {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    if !free {
        return Err(ControlError::failed(format!(
            "another session is still launching {name}; try again shortly"
        )));
    }
    // A catch kept on this app would race the launch's own for its window:
    // on every process of it, since the user's browser and the agents'
    // instance run from one bundle.
    let pids = off(cx, {
        let path = path.clone();
        move || app::processes_of(&path)
    })
    .await;
    let pause = |c: &mut Core, on: bool| {
        for (_, kept) in &c.kept_catches {
            if pids.contains(&kept.pid()) {
                kept.pause(on);
            }
        }
    };
    core.update(cx, |c, _| pause(c, true));
    // The idle quit never closes the agents' instance while this launch
    // may open a window in it; a quit that started ends first, so the plan
    // below finds the instance gone.
    let _hold = off(cx, {
        let path = path.clone();
        move || devtools::is_chromium(&path).then(devtools::hold_agents)
    })
    .await;
    // Decided while this launch holds the app, so two sessions don't both
    // start the agents' instance.
    let (plan, profile) = off(cx, {
        let (path, arguments) = (path.clone(), start.arguments.clone());
        move || browser_plan(&path, &arguments)
    })
    .await;
    use agent_screen_model::agent_browser::{Plan, was_refused, without_debugging_port};
    // One left from an earlier run of Agent Screen has no pipe, the only
    // way to open its window without activating it: it quits, and a new one
    // starts.
    let plan = match (plan, profile.clone()) {
        (Plan::Replace(old), Some(profile)) => {
            log::info!("launch: the agents' {name} {old} has no pipe; quitting it to start anew");
            let gone = off(cx, move || {
                app::terminate(old);
                (0..50).any(|_| {
                    std::thread::sleep(Duration::from_millis(100));
                    (unsafe { libc::kill(old, 0) }) != 0
                })
            })
            .await;
            if !gone {
                core.update(cx, |c, _| {
                    pause(c, false);
                    c.launching.remove(&path);
                });
                return Err(ControlError::failed(format!(
                    "the agents' own {name} (pid {old}), left from an earlier run of Agent Screen, didn't quit within 5 s; ask the user to quit it, then call launch_app again"
                )));
            }
            Plan::Start(agent_screen_model::agent_browser::switches(&profile))
        }
        (plan, _) => plan,
    };
    let asked = start;
    let mut shared = plan == Plan::Shared;
    // Beside the user's own browser, a test never opens a window in it.
    if shared && no_shared_fallback_for_tests() {
        core.update(cx, |c, _| {
            pause(c, false);
            c.launching.remove(&path);
        });
        return Err(ControlError::failed(format!(
            "{name} would open in the user's own process, and the shared fallback is off for this test"
        )));
    }
    let starting = matches!(plan, Plan::Start(_));
    let made_profile = profile.as_ref().is_some_and(|p| !p.exists());
    let start = match (plan, profile.clone()) {
        (Plan::Start(switches), _) => {
            log::info!("launch: starting the agents' instance of {name}");
            let mut start = Start {
                arguments: without_debugging_port(&start.arguments),
                ..start.clone()
            }
            .with_switches(switches);
            start.agents_start = true;
            start
        }
        (Plan::Reuse(pid), Some(_)) => Start {
            agents: Some(AgentsInstance { pid }),
            ..start.clone()
        },
        _ => start.clone(),
    };
    let result = match &start.agents {
        Some(agents) if start.new_instance() => Err(ControlError::failed(format!(
            "the agents' own {name} already runs (pid {}), separate from the user's, and arguments and environment apply only when it starts. Call launch_app without them for a new window in it, or pass --user-data-dir with another folder for a separate instance",
            agents.pid
        ))),
        _ => launch_now(core, id, name, &path, target, &start, cx).await,
    };
    // A browser such as one that allows one instance quits at once on a
    // second start, whatever its profile, while the user's keeps running.
    // Only that is a refusal: a timeout or a crash is not remembered.
    let quit_at_once = matches!(&result, Ok(l) if l.reason == Some("already_running"));
    let refused = starting
        && was_refused(
            quit_at_once,
            match profile.as_deref() {
                Some(profile) => users_run(&path, profile, cx).await,
                None => false,
            },
        );
    let result = match profile.as_deref() {
        Some(_) if refused && no_shared_fallback_for_tests() => {
            log::info!(
                "launch: {name} refused the agents' instance; the fallback is off for this test"
            );
            Err(ControlError::failed(format!(
                "{name} quit at once beside the user's: it refused the agents' instance, and the shared fallback is off for this test"
            )))
        }
        Some(profile) if refused && !agents_run(&path, profile, cx).await => {
            log::info!(
                "launch: {name} refuses a second instance; opening the window in the user's"
            );
            off(cx, {
                let (path, profile) = (path.clone(), profile.to_path_buf());
                move || record_refusal(&path, &profile, made_profile)
            })
            .await;
            shared = true;
            launch_now(core, id, name, &path, target, asked, cx).await
        }
        _ => result,
    };
    let result = result.map(|l| Launched { shared, ..l });
    // The agents' instance's window, too big for the display, as Dia's
    // first one is: fit it through Chromium, which Dia follows.
    let agents_window = starting || start.agents.is_some();
    let result = match result {
        Ok(l) if agents_window && !shared => Ok(fit_agents_window(l, id, cx).await),
        other => other,
    };
    core.update(cx, |c, _| {
        pause(c, false);
        c.launching.remove(&path);
    });
    result
}

async fn start_key_hold(
    user: WindowTarget,
    cx: &mut AsyncApp,
) -> Option<crate::mac::key_hold::Hold> {
    off(cx, move || {
        crate::mac::input::is_front(user.pid)
            .then(|| crate::mac::key_hold::start(user))
            .flatten()
    })
    .await
}

async fn finish_key_hold(
    hold: &mut Option<crate::mac::key_hold::Hold>,
    end: agent_screen_model::key_hold::End,
    cx: &mut AsyncApp,
) {
    if let Some(id) = hold.take().map(|h| h.detach()) {
        off(cx, move || crate::mac::key_hold::finish(id, end)).await;
    }
}

async fn unclaim_and_already_running(
    core: &Entity<Core>,
    id: DisplayId,
    pid: i32,
    name: &str,
    instances_before: &[i32],
    cx: &mut AsyncApp,
) -> Result<Launched, ControlError> {
    core.update(cx, |c, _| {
        if let Some(d) = c.displays.get_mut(id) {
            d.started.remove(&pid);
        }
    });
    already_running(core, id, name, instances_before, cx).await
}

/// Start a Chromium or Electron app with a private DevTools pipe (ADR 0010,
/// ADR 0022). Returns the spawned pid, whether it starts with no window,
/// and a one-shot outcome channel on success; `None` falls back to
/// `open_in_background`.
async fn spawn_piped_browser(
    core: &Entity<Core>,
    name: &str,
    path: &std::path::Path,
    bounds: Rect,
    first: Option<&str>,
    start: &Start,
    cx: &mut AsyncApp,
) -> Option<(i32, bool, std::sync::mpsc::Receiver<bool>)> {
    let path = path.to_path_buf();
    // A browser opens its windows on the display, never on the user's
    // screen first (#123).
    let browser = off(cx, {
        let path = path.clone();
        move || app::handles_web(&path)
    })
    .await;
    // Started while a spare display is being made, it would lose
    // that place: let the spare come first, up to 2 s.
    for _ in 0..100 {
        if !browser || !core.read_with(cx, |c, _| c.making_spare()) {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    let mut arguments = if browser {
        agent_screen_model::launch::browser_window_switches(bounds)
    } else {
        Vec::new()
    };
    arguments.extend(start.open_arguments());
    // The agents' instance shows no window as it starts: Chromium shows
    // its startup window with activation, which takes the user's front
    // app. Its first window comes through the pipe, which opens windows
    // without activating it (ADR 0022). A browser that keeps its pages
    // in windows of its own, as Dia, opens its startup window without
    // activation and makes none through the pipe.
    let through_pipe = start.agents_start && {
        let path = path.clone();
        off(cx, move || {
            app::bundle_id_at(&path)
                .is_none_or(|id| agent_screen_model::agent_browser::pipe_makes_windows(&id))
        })
        .await
    };
    let first_page = if through_pipe {
        arguments.push(agent_screen_model::agent_browser::NO_STARTUP_WINDOW.into());
        Some(first.unwrap_or("about:blank").to_string())
    } else {
        arguments.extend(first.map(String::from));
        None
    };
    let environment = start.environment.clone();
    let started = off(cx, move || {
        hold_launch_for_tests();
        devtools::spawn(&path, &arguments, &environment)
    })
    .await;
    match started {
        Ok(pid) => {
            if start.agents_start
                && let Some(profile) = start
                    .arguments
                    .iter()
                    .find(|a| a.starts_with("--user-data-dir="))
            {
                devtools::mark_agents(pid, profile);
            }
            if let Some(url) = first_page {
                let margin = agent_screen_model::launch::WINDOW_MARGIN;
                let frame = bounds.inset(margin, margin);
                cx.background_executor()
                    .spawn(async move {
                        let made = devtools::channel(pid, Duration::from_secs(15))
                            .ok_or_else(|| "no pipe within 15 s".to_string())
                            .and_then(|c| {
                                // Its launch must be over before its first
                                // window shows, or Chromium brings that
                                // window to the front (#241).
                                app::finish_launch(pid);
                                c.new_window(&url, frame)
                            });
                        log::info!(
                            "launch: the agents' instance {pid}'s first window through its pipe: {made:?}"
                        );
                    })
                    .detach();
            }
            let (done, outcome) = std::sync::mpsc::channel();
            let _ = done.send(true);
            Some((pid, through_pipe, outcome))
        }
        Err(e) => {
            log::warn!("launch: couldn't start {name} with a pipe ({e}); opening it");
            None
        }
    }
}

enum RunningWindowStop {
    Failed(ControlError),
    Missing(&'static str),
}

/// The window this display holds that a running app's URL may open in,
/// when the app has no window outside the display's records (#309).
async fn held_url_window(
    core: &Entity<Core>,
    id: DisplayId,
    pid: i32,
    cx: &mut AsyncApp,
) -> Option<windows::Window> {
    let held = core.read_with(cx, |c, _| {
        c.displays
            .get(id)
            .map(|d| d.records.keys().copied().collect())
            .unwrap_or_default()
    });
    off(cx, move || {
        let app_windows: Vec<_> = cg::windows_of(pid)
            .into_iter()
            .filter(|w| w.layer == 0 && w.bounds.width > 40.0 && w.bounds.height > 40.0)
            .map(|w| w.target.window_id)
            .collect();
        if !agent_screen_model::launch::url_into_held_window(&app_windows, &held) {
            return None;
        }
        windows::on_display(id)
            .into_iter()
            .find(|w| w.target.pid == pid && w.layer == 0 && held.contains(&w.target.window_id))
    })
    .await
}

/// Ask a running app for a new window: through the agents' instance pipe
/// (ADR 0022), or through the app's New Window menu item with retries for an
/// app that just started.
#[allow(clippy::too_many_arguments)]
async fn request_running_window(
    core: &Entity<Core>,
    id: DisplayId,
    pid: i32,
    name: &str,
    bounds: Rect,
    target: Option<&str>,
    is_url: bool,
    agents: Option<&AgentsInstance>,
    cx: &mut AsyncApp,
) -> Result<Option<(String, PageWindow)>, RunningWindowStop> {
    // An app that has just started may not have its menus yet.
    let mut asked = ax::NewWindow::NotReady;
    let mut pipe_page = None;
    // The agents' instance gets its window, and the URL in it, through
    // its pipe, never through its menus or LaunchServices (ADR 0022).
    if let Some(agents) = agents.cloned() {
        let url = target.filter(|_| is_url).map(String::from);
        let url_for_page = url.clone();
        let opened = {
            let agents = agents.clone();
            off(cx, move || new_window_in(&agents, url.as_deref(), bounds)).await
        };
        let failed = match opened {
            Ok((_, PageWindow::Frameless)) => {
                // Into the agents' window on this display, through the
                // page it shows.
                let (only, url) = (agents.pid, url_for_page.clone());
                let into = off(cx, move || {
                    let window = windows::on_display(id)
                        .into_iter()
                        .find(|w| w.target.pid == only && w.layer == 0)
                        .ok_or("keeps its pages in its own windows, which its pipe can't open, and has no window on this display; call launch_app again once its window is here".to_string())?;
                    if let Some(url) = url {
                        devtools::channel(only, Duration::from_secs(1))
                            .ok_or("doesn't answer on its pipe".to_string())?
                            .navigate_visible_page(bounds, &url)
                            .map_err(|e| format!("couldn't open the page in its window on this display: {e}"))?;
                    }
                    Ok::<_, String>(window.target.window_id)
                })
                .await;
                match into {
                    Ok(_) => {
                        asked = ax::NewWindow::Pressed;
                        pipe_page = Some((String::new(), PageWindow::Frameless));
                        None
                    }
                    Err(e) => Some(e),
                }
            }
            Ok(page) => {
                asked = ax::NewWindow::Pressed;
                pipe_page = Some(page);
                None
            }
            Err(e) => Some(e),
        };
        if let Some(e) = failed {
            return Err(RunningWindowStop::Failed(ControlError::failed(format!(
                "the agents' {name} {e}"
            ))));
        }
    }
    for _ in 0..10 {
        if agents.is_some() {
            break;
        }
        asked = cx
            .background_executor()
            .spawn(async move { ax::press_new_window(pid) })
            .await;
        if asked != ax::NewWindow::NotReady {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
    }
    // Just started and no window yet: its menus may not have the item
    // yet. Give them up to 3 s.
    let just_started = || {
        off(cx, move || {
            app::launched_within(pid, LATE_WINDOW_WAIT)
                && !windows::all()
                    .iter()
                    .any(|w| w.target.pid == pid && w.layer == 0)
        })
    };
    // Only this display's own launch has a watch that moves the window.
    let started_here = core.read_with(cx, |c, _| {
        c.displays.get(id).is_some_and(|d| d.started.contains(&pid))
    });
    // This display's own launch answers at once: its watch moves the
    // window when it comes.
    if asked == ax::NewWindow::Missing && !started_here && just_started().await {
        for _ in 0..20 {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            asked = cx
                .background_executor()
                .spawn(async move { ax::press_new_window(pid) })
                .await;
            if asked != ax::NewWindow::Missing {
                break;
            }
        }
    }
    let starting = asked == ax::NewWindow::Missing && just_started().await;
    let missing = match asked {
        ax::NewWindow::Pressed => None,
        ax::NewWindow::Disabled => Some("app_busy"),
        // The first launch's watch moves its window when it comes.
        ax::NewWindow::Missing if started_here && starting => Some("still_starting"),
        // Started by the user or another session moments ago.
        ax::NewWindow::Missing if starting => Some("just_started"),
        ax::NewWindow::Missing => Some("no_new_window"),
        ax::NewWindow::NotReady => Some("menus_unreadable"),
    };
    match missing {
        Some(reason) => Err(RunningWindowStop::Missing(reason)),
        None => Ok(pipe_page),
    }
}

/// Note that display `id` is launching the running app `pid`, before the
/// window the launch opens, a new one or a document's, shows on the
/// user's screen: another display's catch or cold-start watch that sees it
/// there leaves it to this launch (`new_window::another_may_have_made`).
/// The mark holds until the launch, with its watch for a late document,
/// ends and drops the guard, at most `new_window::LAUNCH_MARK_MAX` from
/// the launch call or the watch's start.
fn note_launching(id: DisplayId, pid: i32) -> crate::mac::catch::Launching {
    crate::mac::catch::launching(id, pid)
}

/// Send a URL to a running app's newly opened window before placing it,
/// trying its address field first and falling back to `open_in_background`.
async fn open_url_in_new_window(
    new_window: WindowTarget,
    path: &std::path::Path,
    target: Option<&str>,
    was_running: bool,
    is_url: bool,
    uses_agents_instance: bool,
    cx: &mut AsyncApp,
) -> Option<bool> {
    match (was_running && is_url && !uses_agents_instance)
        .then_some(target)
        .flatten()
    {
        // The agents' instance opened the URL in the window itself.
        None if uses_agents_instance && is_url => Some(true),
        Some(target) => {
            // Into the new window's address field where there is
            // one Agent Screen knows; else the system sends the URL.
            let url = target.to_string();
            let in_field = cx
                .background_executor()
                .spawn(async move { ax::open_in_address_field(new_window, &url) })
                .await;
            log::debug!("launch: URL through the address field: {in_field}");
            Some(
                in_field
                    || match open_in_background(
                        path.to_path_buf(),
                        Some(target.to_string()),
                        false,
                        &Start::default(),
                        cx,
                    )
                    .await
                    {
                        Some(outcome) => {
                            cx.background_executor()
                                .spawn(async move {
                                    outcome
                                        .recv_timeout(Duration::from_secs(10))
                                        .unwrap_or(false)
                                })
                                .await
                        }
                        None => false,
                    },
            )
        }
        None => None,
    }
}

/// Watch for the URL to show in `window`, then hand key focus back to the
/// user's window and release any held keys (#110).
fn spawn_url_handback(
    window: WindowTarget,
    url: String,
    detected: Instant,
    hold: Option<crate::mac::key_hold::Hold>,
    user_window: Option<WindowTarget>,
    cx: &mut AsyncApp,
) -> gpui::Task<bool> {
    let held = hold.map(|h| h.detach());
    cx.background_executor().spawn(async move {
        let shown = shows_url(window, &url, detected);
        // The hold makes the user's window key, then sends
        // on what it held. Without one going on, focus goes
        // back here.
        let ran = held.is_some_and(|id| {
            crate::mac::key_hold::finish(id, agent_screen_model::key_hold::End::HandedBack)
        });
        if !ran
            && let Some(user) = user_window
            && crate::mac::input::is_front(user.pid)
            && ax::focused_window_id(user.pid) == Some(window.window_id)
        {
            let given = ax::make_main(user);
            log::debug!("launch: key focus back to the user's window: {given}");
        }
        shown
    })
}

/// Check whether `document` is already shown in a pre-existing window; if not,
/// watch for a late window showing it so it does not stay on the user's screen.
/// `launching`, the launch's mark (`note_launching`), lasts as long as the
/// watch, up to 30 s from the watch's start.
/// Returns `(shown_already, watching_started)`.
#[allow(clippy::too_many_arguments)]
async fn watch_late_document(
    core: &Entity<Core>,
    id: DisplayId,
    pid: i32,
    before: &std::collections::HashSet<u32>,
    document: Option<std::path::PathBuf>,
    document_catch: Option<crate::mac::catch::Catch>,
    launching: Option<crate::mac::catch::Launching>,
    cx: &mut AsyncApp,
) -> (bool, bool) {
    let shown_already = match document.clone() {
        Some(d) => {
            let before = before.clone();
            cx.background_executor()
                .spawn(async move {
                    windows::all().iter().any(|w| {
                        w.target.pid == pid
                            && before.contains(&w.target.window_id)
                            && shows_document(w.target, &d)
                    })
                })
                .await
        }
        None => false,
    };
    let watching = document
        .filter(|_| !shown_already)
        .filter(|d| core.update(cx, |c, _| c.watching.insert((pid, d.clone()))));
    let Some(document) = watching else {
        return (shown_already, false);
    };
    // A catch for the file's window moves it as it appears; the poll
    // covers an app that names the document only after.
    let core = core.clone();
    let before = before.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        // The mark counts from here, past the launch call's own wait, and
        // the watch ends with it.
        let marked_until = launching.as_ref().and_then(|l| l.renew());
        for _ in 0..200 {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            if marked_until.is_some_and(|until| Instant::now() >= until) {
                break;
            }
            let taken = document_catch.as_ref().and_then(|c| c.taken());
            let (seen, file) = (before.clone(), document.clone());
            let late = cx
                .background_executor()
                .spawn(async move {
                    windows::all().into_iter().find(|w| {
                        w.target.pid == pid
                            && w.layer == 0
                            && !seen.contains(&w.target.window_id)
                            && (taken == Some(w.target.window_id)
                                || shows_document(w.target, &file))
                    })
                })
                .await;
            if let Some(late) = late {
                let _ = settle(&core, late.target, id, cx).await;
                break;
            }
            if !takes_windows(&core, id, cx) {
                break;
            }
        }
        drop(document_catch);
        drop(launching);
        core.update(cx, |c, _| c.watching.remove(&(pid, document)));
    })
    .detach();
    (shown_already, true)
}

async fn launch_now(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    path: &std::path::Path,
    target: Option<&str>,
    start: &Start,
    cx: &mut AsyncApp,
) -> Result<Launched, ControlError> {
    let path = path.to_path_buf();
    let agents = start.agents.clone();
    let started = Instant::now();
    // Windows already on screen, or claimed by any display, aren't new.
    let mut before: std::collections::HashSet<u32> = off(cx, || {
        cg::on_screen_ids_where(|_, layer, bounds| {
            (0..=8).contains(&layer) && bounds.width > 40.0 && bounds.height > 40.0
        })
        .into_iter()
        .collect()
    })
    .await;
    before.extend(core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .flat_map(|d| d.records.keys().copied())
            .collect::<Vec<_>>()
    }));
    // Every window then, minimized, hidden and on other Spaces too, as the
    // app's own list names them: a busy app's undescribed window is a new
    // one (#270), never the user's minimized one.
    let mut all_before = off(cx, cg::all_ids).await;
    all_before.extend(before.iter().copied());
    // A new instance is a starting app, whatever else runs from the bundle.
    let fresh = start.new_instance();
    // The agents' instance of a browser the user runs is the only one this
    // launch may use (ADR 0022).
    let only = agents.as_ref().map(|a| a.pid);
    let process_watch =
        std::sync::Arc::new(std::sync::Mutex::new(app::ProcessWatch::new(&path, only)));
    let poll_pids = {
        let watch = std::sync::Arc::clone(&process_watch);
        move || {
            watch
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .poll_running_pids()
        }
    };
    let instances_before = off(cx, poll_pids.clone()).await;
    let was_running = !fresh && !instances_before.is_empty();
    // New windows go straight onto the display, before the user sees them
    // or AeroSpace files them in the user's workspace.
    let bounds = cg::display_bounds(id);
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    // A running app may be the user's: take only the window Agent Screen
    // opens. A starting app's windows are all Agent Screen's.
    let limit = was_running.then_some(1);
    let catch_from = |pid: i32| crate::mac::catch::Catch::start(pid, id, screen.clone(), limit);
    let running = instances_before.first().copied();
    // The agents' running instance is shared by every session: its window
    // is the one its pipe made for this page, found below, and a catch
    // could take another session's window first.
    let mut catch = match running.filter(|_| !fresh && agents.is_none()) {
        Some(pid) => catch_from(pid).await,
        None => None,
    };

    // A starting app opens the URL or file in its first window. A running
    // one opens a file in a window of its own, but a URL as a tab of its
    // front window, which may be the user's. Ask for a new window first.
    // If it can't open one, send the URL only when this display holds
    // all of the app's windows.
    let is_url = target.is_some_and(|t| !t.starts_with('/'));
    // A running app given a file: its window is the one that shows the
    // file, whenever it comes, and never one the user opens meanwhile.
    let document = target
        .filter(|t| was_running && t.starts_with('/'))
        .and_then(|t| std::fs::canonicalize(t).ok());
    let first = (!was_running || !is_url)
        .then(|| target.map(String::from))
        .flatten();
    // Opening a running app with nothing to open only reopens it, and
    // TextEdit and others answer a reopen with a window of their own, on
    // the user's screen, besides the one Command-N asks for below.
    let could_not_open = || ControlError::failed(format!("could not open {name}"));
    let requested_at = Instant::now();
    // The new window may take the app's key focus from the user's window
    // before Agent Screen sees it: note theirs now, to hand it back after.
    let mut user_window = None;
    if let Some(pid) = running.filter(|_| !fresh) {
        core.update(cx, |c, _| c.forwarder(id))
            .remember_user_window(pid)
            .await;
        user_window = off(cx, move || user_key_window(pid)).await;
    }
    // A Chromium or Electron app that starts here gets a private DevTools
    // pipe, so drags in its pages work (ADR 0010). `open` can't hand it
    // one: Agent Screen starts it. Only a browser takes a URL or file as
    // an argument; another app given one starts through `open`, without.
    let piped = !was_running
        && off(cx, {
            let (path, given) = (path.clone(), first.is_some());
            move || devtools::is_chromium(&path) && (!given || app::handles_web(&path))
        })
        .await;
    let mut spawned = None;
    // The agents' instance starts with no window, and its first comes
    // through its pipe at the display's frame (ADR 0022).
    let mut windowless = false;
    let opening = if piped {
        spawn_piped_browser(core, name, &path, bounds, first.as_deref(), start, cx)
            .await
            .map(|(pid, through_pipe, outcome)| {
                spawned = Some(pid);
                windowless = through_pipe;
                outcome
            })
    } else {
        None
    };
    // The running app's document window opens on the user's screen too.
    // The mark lasts until this launch returns.
    let mut launching = running
        .filter(|_| !fresh && document.is_some())
        .map(|pid| note_launching(id, pid));
    let opening = match opening {
        Some(o) => Some(o),
        None if !was_running || first.is_some() => Some(
            open_in_background(path.clone(), first, !was_running, start, cx)
                .await
                .ok_or_else(could_not_open)?,
        ),
        None => None,
    };
    // How `open` ended: None while it runs, or without one.
    let mut opened: Option<(bool, Instant)> = None;
    let poll_open = |opened: &mut Option<(bool, Instant)>| {
        if opened.is_none()
            && let Some(Ok(ok)) = opening.as_ref().map(|o| o.try_recv())
        {
            *opened = Some((ok, Instant::now()));
        }
        matches!(opened, Some((false, _)))
    };
    let mut pid = None;
    // A new process of the bundle may be another tool's, started from a
    // shell meanwhile (issue #35): the new instance is the one launchd
    // started with these arguments. One that restarted itself under
    // another pid matches neither, so the first new one is it after all.
    let asked = start.open_arguments();
    let mut other = None;
    // A new instance of an app that already runs may be one that allows
    // only one: it hands its launch to the running instance and quits.
    let single_instance_risk = fresh && !instances_before.is_empty();
    let quit = |pid: i32| {
        let path = path.clone();
        move || {
            let here = spawned == Some(pid);
            let alive =
                !here && only.is_none_or(|o| o == pid) && app::is_bundle_pid_running(pid, &path);
            agent_screen_model::launch::has_quit(
                here,
                here && unsafe { libc::kill(pid, 0) } == 0,
                alive,
            )
        }
    };
    for _ in 0..200 {
        // Started here: its pid is known.
        if spawned.is_some() {
            pid = spawned;
            break;
        }
        let now = off(cx, poll_pids.clone()).await;
        pid = if fresh {
            let new: Vec<i32> = now
                .into_iter()
                .filter(|p| !instances_before.contains(p))
                .collect();
            other = other.or(new.first().copied());
            let asked = asked.clone();
            off(cx, move || {
                new.into_iter().find(|&p| {
                    let parent = app::parent_pid(p).unwrap_or(0);
                    let argv = app::arguments(p).unwrap_or_default();
                    agent_screen_model::launch::is_opened(parent, &argv, &asked)
                })
            })
            .await
        } else {
            now.into_iter().next()
        };
        if pid.is_some() {
            break;
        }
        if poll_open(&mut opened) {
            return Err(could_not_open());
        }
        // `open` came back and no new process runs: an app that allows one
        // instance passed the launch to the running one and quit (#29).
        if single_instance_risk
            && other.is_none()
            && matches!(opened, Some((true, at)) if at.elapsed() > Duration::from_millis(500))
        {
            return already_running(core, id, name, &instances_before, cx).await;
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    if pid.is_none() && other.is_some() {
        log::info!("launch: no new instance of {name} matched its launch; taking {other:?}");
    }
    let pid = pid
        .or(other)
        .ok_or_else(|| ControlError::failed(format!("{name} did not start")))?;
    if single_instance_risk && off(cx, quit(pid)).await {
        return already_running(core, id, name, &instances_before, cx).await;
    }
    // Two sessions launching an app at once both see it start; it's the
    // first one's. Its cold start counts from here.
    let launched_at = Instant::now();
    if !was_running {
        // The agents' instance is every session's: no display holds it,
        // keeps a catch on it, or hands it to the session's next display.
        let ours = start.agents_start
            || core.update(cx, |c, _| {
                let claimed = c.displays.list.iter().any(|d| d.started.contains(&pid));
                !claimed
                    && c.displays
                        .get_mut(id)
                        .is_some_and(|d| d.started.insert(pid))
            });
        // Its windows are the agent's for 30 s from now unless the user
        // acts on it, whenever launch_app answers.
        if ours {
            crate::mac::catch::agent_acted_for(id, pid, LATE_WINDOW_WAIT);
        }
    }
    // A starting app takes accessibility observers only once it has set
    // up; until then, try again, and move any window that beats it.
    for _ in 0..100 {
        if catch.is_some() || single_instance_risk && off(cx, quit(pid)).await {
            break;
        }
        catch = catch_from(pid).await;
        let seen = before.clone();
        // `open -j` starts an app hidden; one Agent Screen started itself
        // is hidden as soon as it can be, so its first window doesn't
        // draw on the user's screen.
        let hide = spawned.is_some() && !windowless;
        cx.background_executor()
            .spawn(async move {
                if hide && !app::is_hidden(pid) {
                    app::hide(pid);
                }
                for window_id in cg::on_screen_ids_where(|p, layer, bounds| {
                    p == pid && layer == 0 && bounds.width > 40.0 && bounds.height > 40.0
                }) {
                    if !seen.contains(&window_id) {
                        let _ = ax::move_into(WindowTarget { pid, window_id }, bounds);
                    }
                }
            })
            .await;
        if catch.is_none() {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
    }
    log::debug!(
        "launch: {name} is pid {pid} at {} ms, catching new windows: {}",
        started.elapsed().as_millis(),
        catch.is_some()
    );
    if single_instance_risk && off(cx, quit(pid)).await {
        return unclaim_and_already_running(core, id, pid, name, &instances_before, cx).await;
    }
    // Started hidden: once its windows sit on the display, show them there.
    if !was_running && !windowless {
        for _ in 0..150 {
            // Browsers show themselves as they start, which draws their
            // first window on the user's screen: hide them again until it
            // sits on the display.
            let moved = off(cx, move || {
                if !app::is_hidden(pid) {
                    app::hide(pid);
                }
                ax::move_all_into(pid, bounds)
            })
            .await;
            if moved > 0 {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        off(cx, move || app::unhide(pid)).await;
    }

    // A running app opens no window on launch; ask it for a new one through
    // the menu item that opens one. Without such an item, as in Calculator
    // or Notes, there is none to give: say so now.
    let asked_for_window = was_running && (target.is_none() || is_url);
    // The new window takes the app's key focus from the user's window, in
    // their front app, until Agent Screen hands it back: hold their keys
    // meanwhile and send them on to their window after (#110).
    let mut hold = match user_window {
        Some(user) if asked_for_window => start_key_hold(user, cx).await,
        _ => None,
    };
    // The page the agents' instance's pipe opened for this launch, and
    // whether it went into a window on the display that was open already.
    let mut pipe_page: Option<(String, PageWindow)> = None;
    if asked_for_window {
        launching = Some(note_launching(id, pid));
        match request_running_window(
            core,
            id,
            pid,
            name,
            bounds,
            target,
            is_url,
            agents.as_ref(),
            cx,
        )
        .await
        {
            Ok(page) => pipe_page = page,
            Err(stop) => {
                // No window will take the user's focus: what was held goes to
                // their window, which still has it.
                finish_key_hold(&mut hold, agent_screen_model::key_hold::End::NoWindow, cx).await;
                if let RunningWindowStop::Missing("no_new_window" | "menus_unreadable") = stop
                    && is_url
                    && agents.is_none()
                    && let Some(url) = target
                    && let Some(window) = held_url_window(core, id, pid, cx).await
                {
                    let opened = match open_in_background(
                        path.clone(),
                        Some(url.to_string()),
                        false,
                        &Start::default(),
                        cx,
                    )
                    .await
                    {
                        Some(outcome) => {
                            cx.background_executor()
                                .spawn(async move {
                                    outcome
                                        .recv_timeout(Duration::from_secs(10))
                                        .unwrap_or(false)
                                })
                                .await
                        }
                        None => false,
                    };
                    // Give the catch time to place a new window the URL opens.
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    drop(catch);
                    if !opened {
                        return Err(ControlError::failed(format!(
                            "{name} could not open the URL in its window on this display"
                        )));
                    }
                    log::info!(
                        "launch: {name} has no new window; opened the URL in its window {} on this display",
                        window.target.window_id
                    );
                    return Ok(Launched {
                        window: Some(window),
                        was_running: true,
                        reason: Some("display_window"),
                        pid: None,
                        shared: false,
                    });
                }
                drop(catch);
                return match stop {
                    RunningWindowStop::Failed(err) => Err(err),
                    RunningWindowStop::Missing(reason) if is_url => {
                        let next = match reason {
                            "just_started" | "still_starting" => {
                                "Wait a few seconds and call launch_app again".to_string()
                            }
                            "app_busy" => format!(
                                "Ask the user to finish what {name} is showing, then call launch_app again"
                            ),
                            _ => format!(
                                "Ask the user to hand its window over with place_window, then call launch_app again with the same open to open the URL in that window. Or ask the user to quit {name}, then call launch_app again"
                            ),
                        };
                        Err(ControlError::failed(format!(
                            "{name} is running and can't open a new window now ({reason}), so the URL was not opened. {next}."
                        )))
                    }
                    RunningWindowStop::Missing(reason) => Ok(Launched {
                        window: None,
                        was_running,
                        reason: Some(reason),
                        pid: None,
                        shared: false,
                    }),
                };
            }
        }
    }
    let asked_at = Instant::now();

    // A running app may be the user's: its new window is the one the catch
    // took, never one the user opens meanwhile. A starting app's windows
    // are all Agent Screen's.
    // Off the main thread: reading a window's document asks the app, which
    // may be busy.
    let find_ours = |taken: Option<u32>| {
        let (before, document, page) = (before.clone(), document.clone(), pipe_page.clone());
        let url = target.filter(|_| is_url).map(String::from);
        async move {
            // The frame of the window that shows the pipe's page now, read
            // beside the system's list so a window being moved still matches.
            let page_frame = page.as_ref().and_then(|(target, _)| {
                devtools::channel(pid, Duration::from_secs(1))?
                    .window_of(target)
                    .map(|(_, frame)| frame)
            });
            let kind = page.as_ref().map(|(_, kind)| *kind);
            // Whether `w` is the window that shows the pipe's page.
            let page_window =
                |w: &windows::Window| match kind? {
                    PageWindow::New | PageWindow::Open => Some(page_frame.is_some_and(|f| {
                        agent_screen_model::agent_browser::same_frame(f, w.bounds)
                    })),
                    // Found by the page it shows, on this display.
                    PageWindow::Frameless => Some(
                        bounds.contains(w.bounds.mid())
                            && url.as_deref().is_none_or(|url| {
                                ax::web_url(w.target).is_some_and(|shown| {
                                    agent_screen_model::launch::same_page(url, &shown)
                                })
                            }),
                    ),
                };
            let in_open_window = matches!(kind, Some(PageWindow::Open | PageWindow::Frameless));
            windows::all().into_iter().find(|w| {
                w.target.pid == pid
                    && w.layer == 0
                    && (in_open_window || !before.contains(&w.target.window_id))
                    && agent_screen_model::launch::answers_launch(
                        was_running,
                        page_window(w),
                        taken == Some(w.target.window_id),
                        // Asked only when nothing else decides: it asks the app.
                        || {
                            document
                                .as_deref()
                                .is_some_and(|d| shows_document(w.target, d))
                        },
                    )
            })
        }
    };
    let mut gave_up = false;
    // A window the busy app still couldn't describe when the launch gave
    // up waiting for it (#270), by its number once known: moved to the
    // display once the app describes it.
    let mut undescribed: Option<Option<u32>> = None;
    // Done with that window: taken, or left where it is.
    let mut passed_undescribed = false;
    // Takes over from `catch` for a file still opening, with no gap between.
    let mut document_catch = None;
    // Every 25 ms for 6 s: the caught window is answered within a tick.
    for tick in 0..240 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        if single_instance_risk && tick % 8 == 0 && off(cx, quit(pid)).await {
            drop(catch);
            return unclaim_and_already_running(core, id, pid, name, &instances_before, cx).await;
        }
        let mut taken = catch.as_ref().and_then(|c| c.taken());
        // A window the busy app couldn't describe as it showed is the
        // catch's too, once the app says it is a window (#270), unless the
        // user has acted in the app since (ADR 0007).
        if taken.is_none()
            && !passed_undescribed
            && let Some(kept) = catch.as_ref().and_then(|c| c.undescribed())
        {
            let before = all_before.clone();
            match off(cx, move || find_undescribed(pid, kept, &before)).await {
                Undescribed::Busy if asked_at.elapsed() < UNDESCRIBED_WAIT => continue,
                // A busy app lists the window only once it answers.
                Undescribed::Busy => {
                    undescribed = Some(kept);
                    break;
                }
                Undescribed::Unsure => {
                    log::info!(
                        "launch: {name} answered, but no window is surely the one it made; left where it is"
                    );
                    passed_undescribed = true;
                }
                Undescribed::Window { id: window, .. } if user_acted_in(pid, asked_at) => {
                    log::info!(
                        "launch: {name} window {window} came while the user acted in the app; left where it is"
                    );
                    passed_undescribed = true;
                }
                Undescribed::Window {
                    id: window,
                    described: true,
                } => taken = Some(window),
                // Waiting for the app to describe it: the launch doesn't
                // give up meanwhile.
                Undescribed::Window { .. } if asked_at.elapsed() < UNDESCRIBED_WAIT => continue,
                Undescribed::Window { id: window, .. } => {
                    undescribed = Some(Some(window));
                    break;
                }
            }
        }
        let open_failed = poll_open(&mut opened);
        // A window already caught is the app's answer; keep it even if
        // `open` reports a failure, so it isn't left unclaimed.
        if open_failed && taken.is_none() {
            return Err(could_not_open());
        }
        // A running app had its chance: stop taking its windows, which may
        // now be the user's.
        if was_running
            && taken.is_none()
            && waited_long_enough(
                asked_for_window,
                asked_at,
                requested_at,
                opened.map(|(_, at)| at),
            )
        {
            document_catch = match document.clone() {
                Some(d) => crate::mac::catch::Catch::for_document(pid, id, screen.clone(), d).await,
                None => None,
            };
            log::debug!(
                "launch: {name} file catch armed: {}",
                document_catch.is_some()
            );
            catch = None;
            gave_up = true;
            break;
        }
        if let Some(fresh) = cx.background_executor().spawn(find_ours(taken)).await {
            let found = started.elapsed().as_millis();
            let detected = Instant::now();
            // A hold that ended before the window came, on a click or its
            // time limit, left the window free to take the user's keys:
            // hold them again until the hand-back.
            if let Some(user) = user_window
                && asked_for_window
                && !hold.as_ref().is_some_and(|h| h.holding())
            {
                if let Some(ended) = hold.take() {
                    ended.detach();
                }
                hold = start_key_hold(user, cx).await;
                log::debug!(
                    "launch: keys held again for the late window: {}",
                    hold.is_some()
                );
            }
            // A running app's new window is its key window from the start,
            // where the user's typing goes until Agent Screen hands focus
            // back (#110): send the URL now, before placing the window, so
            // that comes sooner. The browser opens it in its key window.
            let opened = open_url_in_new_window(
                fresh.target,
                &path,
                target,
                was_running,
                is_url,
                agents.is_some(),
                cx,
            )
            .await;
            // While the window is placed, watch for the URL, and give the
            // user's window its key focus back the moment it shows: placing
            // takes most of a second, all of it with the user typing into
            // the new window (#110). The focus record is refused in the
            // front app, so it goes through accessibility, as a click would.
            let handback = match (opened, target) {
                (Some(true), Some(url)) => Some(spawn_url_handback(
                    fresh.target,
                    url.to_string(),
                    detected,
                    hold.take(),
                    user_window,
                    cx,
                )),
                _ => None,
            };
            let placed = settle(core, fresh.target, id, cx).await?;
            log::debug!(
                "launch: {name} window at {found} ms, placed at {} ms",
                started.elapsed().as_millis()
            );
            // Whether the running app's new window may give its key focus
            // back to the user's window before the call ends: one asked for
            // with nothing to open, or one that shows the agent's URL.
            let mut settled = was_running && target.is_none();
            if let (Some(opened), Some(target)) = (opened, target) {
                if !opened {
                    return Err(ControlError::failed(format!(
                        "{name} opened a window on the display but not {target}; open it there yourself"
                    )));
                }
                // `open` returns once the URL is sent, not handled; the
                // hand-back comes within 2 s whether the page does or not.
                if let Some(task) = handback {
                    task.await;
                    settled = true;
                }
            }
            // The new window took the app's key focus from the user's
            // window, where they may be typing, as in a browser they use
            // (#110): hand it back now rather than after the page loads.
            // Otherwise the call's end hands it back, as after any input.
            if settled {
                core.update(cx, |c, _| c.forwarder(id)).keys_done().await;
                finish_key_hold(&mut hold, agent_screen_model::key_hold::End::HandedBack, cx).await;
            }
            // A window the agents' instance opens through its pipe shows
            // behind the windows already on the display, such as another
            // Chrome's, which then takes the agent's clicks. Raise it until
            // it is in front at its middle; a raise activates nothing
            // (ADR 0022).
            if windowless || agents.is_some() {
                let target = placed.target;
                let front = off(cx, move || {
                    // `placed.bounds` is relative to the display.
                    let mid = cg::window_bounds(target.window_id)?.mid();
                    let mut front = crate::mac::input::window_at(mid).map(|(w, _)| w);
                    let before = front;
                    for _ in 0..5 {
                        if front == Some(target) {
                            break;
                        }
                        if let Some(w) = ax::window_element(target) {
                            ax::perform(&w, "AXRaise");
                        }
                        std::thread::sleep(Duration::from_millis(100));
                        front = crate::mac::input::window_at(mid).map(|(w, _)| w);
                    }
                    Some((before, front))
                })
                .await;
                log::debug!(
                    "launch: the agents' window {target:?}; in front of it before and after raising: {front:?}"
                );
            }
            // An app started in the background drops the first click on its
            // window, which only makes the window key: spend it here.
            if !was_running {
                core.update(cx, |c, _| c.forwarder(id))
                    .wake(placed.target, placed.bounds);
            }
            // A page's buttons work only once its scripts have run: an
            // agent clicking straight after would click a dead page.
            if is_url {
                wait_for_page(placed.target, cx).await;
            }
            keep_catch(core, id, catch, cx);
            // A starting app's later windows, such as a main window after a
            // loading one, may come while it is too busy for the catch.
            if !was_running && !start.agents_start {
                let mut seen = before.clone();
                seen.insert(placed.target.window_id);
                watch_cold_start(core, id, pid, seen, true, launched_at, screen.clone(), cx);
            }
            log::debug!(
                "launch: {name} answered at {} ms",
                started.elapsed().as_millis()
            );
            return Ok(Launched {
                window: Some(placed),
                was_running,
                reason: None,
                pid: (!was_running).then_some(pid),
                shared: false,
            });
        }
    }
    keep_catch(core, id, catch, cx);
    log::debug!("launch: {name} made no window of Agent Screen's (gave up early: {gave_up})");
    if let Some(window) = undescribed {
        finish_key_hold(&mut hold, agent_screen_model::key_hold::End::NoWindow, cx).await;
        adopt_undescribed(
            core.clone(),
            id,
            pid,
            path.clone(),
            window,
            all_before.clone(),
            asked_at,
            user_window,
            cx,
        );
        return Err(ControlError::failed(format!(
            "{name} made a new window but was too busy to say what it is within {} s{}. Agent Screen moves that window onto this display once {name} answers, unless the user acts in {name} first: call list_windows in a few seconds, then act in it",
            UNDESCRIBED_WAIT.as_secs(),
            if target.is_some() && is_url {
                ", so the URL was not opened in it"
            } else {
                ""
            }
        )));
    }
    finish_key_hold(&mut hold, agent_screen_model::key_hold::End::NoWindow, cx).await;
    // A slow launch can open its window after we stop waiting. Keep watching
    // so it does not stay on the user's screen: for an app Agent Screen
    // started, any new window; for a running app, the one showing the file.
    let (shown_already, watching) = watch_late_document(
        core,
        id,
        pid,
        &before,
        document,
        document_catch.take(),
        launching.take(),
        cx,
    )
    .await;
    if !watching && !was_running {
        // The kept catch takes the first window the moment it opens, unless
        // the user is clicking or typing then; the watch is for an app that
        // won't take observers yet. The cold start counts from the launch.
        watch_cold_start(
            core,
            id,
            pid,
            before,
            false,
            launched_at,
            screen.clone(),
            cx,
        );
    }
    // The URL waits for the new window, which never came: opening it now
    // would add a tab to whatever window the user has in front.
    if was_running && is_url {
        return Err(ControlError::failed(format!(
            "{name} opened no new window, so the URL was not opened. Ask the user to hand its window over with place_window, then call launch_app again with the same open. Or ask the user to quit {name}, then call launch_app again."
        )));
    }
    Ok(Launched {
        window: None,
        was_running,
        pid: (!was_running).then_some(pid),
        shared: false,
        reason: Some(if !was_running {
            "still_starting"
        } else if asked_for_window {
            "no_new_window"
        } else if shown_already {
            "existing_window"
        } else {
            "still_opening"
        }),
    })
}

/// The answer when the instance a launch started quit at once, as an app
/// that allows one instance does, passing its arguments to the running one
/// (#29): that instance's window on the display, or an error that says the
/// app is already running.
async fn already_running(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    running: &[i32],
    cx: &mut AsyncApp,
) -> Result<Launched, ControlError> {
    log::info!(
        "launch: {name} quit at once; it allows one instance, already running as {running:?}"
    );
    let on_display: std::collections::HashSet<u32> = core.read_with(cx, |c, _| {
        c.displays
            .get(id)
            .map(|d| d.records.keys().copied().collect())
            .unwrap_or_default()
    });
    let running = running.to_vec();
    let theirs = running.clone();
    let window = off(cx, move || {
        windows::on_display(id).into_iter().find(|w| {
            theirs.contains(&w.target.pid)
                && w.layer == 0
                && on_display.contains(&w.target.window_id)
        })
    })
    .await;
    match window {
        Some(window) => Ok(Launched {
            window: Some(window),
            was_running: true,
            reason: Some("already_running"),
            pid: None,
            shared: false,
        }),
        None => Err(ControlError::failed(format!(
            "{name} is already running (pid {}) and allows only one instance: the new instance quit at once and passed its arguments to the running one, which has no window on this display. Ask the user to quit {name}, then call launch_app again; or call launch_app without arguments and environment to ask the running app for a new window",
            running.first().copied().unwrap_or_default()
        ))),
    }
}

/// What a launch opens in place of `target`, if anything: a file URL's file
/// for an app that isn't a browser, and a file path's URL for a browser,
/// which opens either as a page. A file URL's file must exist; open_target
/// checked a path's.
fn file_target(target: Option<&str>, browser: bool) -> Result<Option<String>, ControlError> {
    if let Some(path) = target.filter(|t| browser && t.starts_with('/')) {
        return Ok(Some(agent_screen_model::launch::file_url(path)));
    }
    let Some(url) = target.filter(|t| t.starts_with("file://")) else {
        return Ok(None);
    };
    let path = ax::file_url_path(url)
        .ok_or_else(|| ControlError::invalid(format!("{url} isn't a file URL")))?;
    if !path.exists() {
        return Err(ControlError::not_found(format!(
            "no file at {}",
            path.display()
        )));
    }
    Ok((!browser).then(|| path.to_string_lossy().into_owned()))
}

/// Whether a window shows a file, by its AXDocument URL.
fn shows_document(target: WindowTarget, path: &std::path::Path) -> bool {
    ax::window_element(target)
        .and_then(|w| ax::document_path(&w))
        .is_some_and(|p| p == path)
}

/// Whether a running app has had its time to make the window Agent Screen
/// asked for: counted from Command-N, or from when `open` returned, and
/// never longer than RUNNING_APP_TURN from the request.
fn waited_long_enough(
    asked_for_window: bool,
    asked_at: Instant,
    requested_at: Instant,
    opened_at: Option<Instant>,
) -> bool {
    if asked_for_window {
        asked_at.elapsed() > ASKED_WINDOW_WAIT
    } else {
        requested_at.elapsed() > running_app_turn()
            || opened_at.is_some_and(|at| at.elapsed() > OPENED_WINDOW_WAIT)
    }
}

/// Place a new window and wait until it is really on the display. A window
/// manager may not know about a window for a moment after it appears.
async fn settle(
    core: &Entity<Core>,
    mut target: WindowTarget,
    id: DisplayId,
    cx: &mut AsyncApp,
) -> Result<windows::Window, ControlError> {
    if !takes_windows(core, id, cx) {
        return Err(ended_during(core, id, cx));
    }
    claim(core, id, target, Origin::Launched, cx);
    // AeroSpace may take a placed window back to its own layout a moment
    // later; then look again a while after, and `guard_placed` keeps
    // looking for 3 s past the answer. Even one that ignores Agent Displays
    // does: it files a new window that opened on the user's screen in their
    // workspace, and a layout pass begun before the window moved puts it
    // back there. Without AeroSpace a window placed is a window held.
    let aerospace_takes = cx
        .background_executor()
        .spawn(async { crate::mac::aerospace::AeroSpace::current().is_some() })
        .await;
    let look = if aerospace_takes { 200 } else { 30 };
    let mut last_error = None;
    for _ in 0..12 {
        if !takes_windows(core, id, cx) {
            return Err(ended_during(core, id, cx));
        }
        // An app given a file may close the window it opened first and show
        // the document in another, as Pages does importing text: follow the
        // app to that one.
        if let Some(other) = replacement(core, target, cx).await {
            log::info!(
                "window {} closed while placing; following {} of the same app",
                target.window_id,
                other.window_id
            );
            target = other;
            claim(core, id, target, Origin::Launched, cx);
        }
        if let Err(e) = place(target, id, cx).await {
            last_error = Some(e);
        }
        cx.background_executor()
            .timer(Duration::from_millis(look))
            .await;
        if !held(target, id, cx).await {
            continue;
        }
        let on_display = move || {
            windows::on_display(id)
                .into_iter()
                .find(|w| w.target == target)
        };
        if let Some(placed) = off(cx, on_display).await {
            if aerospace_takes {
                guard_placed(core, target, id, cx);
            }
            // The new app's window takes the keys, not the last one clicked.
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            forwarder.focus(target).await;
            // A window bigger than the display gets pulled inside.
            let moved = cx
                .background_executor()
                .spawn(async move { windows::keep_inside(id) })
                .await;
            if moved && let Some(kept) = off(cx, on_display).await {
                return Ok(kept);
            }
            return Ok(placed);
        }
    }
    // Left where it opened, it would sit on the user's screen.
    let minimized = cx
        .background_executor()
        .spawn(async move { ax::minimize(target) })
        .await;
    Err(ControlError::failed(format!(
        "the window opened but could not be placed on the display{}; {}",
        last_error.map(|e| format!(": {e}")).unwrap_or_default(),
        if minimized {
            "it was minimized so it stays off the user's screen"
        } else {
            "it may be on the user's screen"
        }
    )))
}

/// Another window of `target`'s app, once `target` has closed: one no
/// display has, the newest first.
async fn replacement(
    core: &Entity<Core>,
    target: WindowTarget,
    cx: &mut AsyncApp,
) -> Option<WindowTarget> {
    let claimed: std::collections::HashSet<u32> = core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .flat_map(|d| d.records.keys().copied())
            .collect()
    });
    cx.background_executor()
        .spawn(async move {
            let all = windows::all();
            if all.iter().any(|w| w.target == target) || ax::window_element(target).is_some() {
                return None;
            }
            all.into_iter()
                .filter(|w| {
                    w.target.pid == target.pid
                        && w.layer == 0
                        && !claimed.contains(&w.target.window_id)
                })
                .map(|w| w.target)
                .max_by_key(|t| t.window_id)
        })
        .await
}

/// Watch a window just placed for 3 s more, and place it again if it leaves
/// the display. AeroSpace files a window that opened on the user's screen in
/// their workspace, and a layout pass that began before the window moved
/// puts it back there, a second or more later when the pass is slow. Nothing
/// moves it again after that, so it would stay on the user's screen. Stops
/// once the user takes the window back.
fn guard_placed(core: &Entity<Core>, target: WindowTarget, id: DisplayId, cx: &mut AsyncApp) {
    let core = core.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        // 3 s of looking, not counting while displays change, up to 10 s.
        let mut looks = 0;
        for _ in 0..40 {
            if looks == 12 {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            // While another display comes or goes, this one moves and its
            // windows follow a moment apart: the window only seems to have
            // left. Putting it back then would put it on another display.
            if cg::displays_changed_within(Duration::from_millis(1000)) {
                continue;
            }
            looks += 1;
            // Ended, taken back by the user, or closed: no longer ours to keep.
            // Pause only while the Viewer is actively dragging or watching this
            // window (`keep_on_display`), not for the entire Takeover (`L5`).
            let recorded = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .is_some_and(|d| d.records.contains_key(&target.window_id))
            });
            if !recorded
                || !takes_windows(&core, id, cx)
                || off(cx, move || windows::find(target.window_id).is_none()).await
            {
                return;
            }
            if crate::mac::input::viewer_watching_window(target.window_id) {
                continue;
            }
            // Displays move as others come and go, and their windows a
            // moment apart, so a window may seem to sit on the display next
            // door. Only one on the user's screen has really left.
            // AeroSpace that tiles agent displays may still hide it there.
            let (on_virtual, aerospace_tiles) = off(cx, move || {
                (
                    windows::on_virtual_display(target.window_id),
                    !crate::mac::aerospace::ignores_now()
                        && crate::mac::aerospace::AeroSpace::current().is_some(),
                )
            })
            .await;
            if on_virtual && !aerospace_tiles {
                continue;
            }
            if !held(target, id, cx).await {
                // Taken back while we looked: home is where it goes.
                let still = core.read_with(cx, |c, _| {
                    c.displays.get(id).is_some_and(|d| {
                        d.records.contains_key(&target.window_id)
                            && !d.returned.contains(&target.window_id)
                    })
                });
                if !still {
                    return;
                }
                let now = off(cx, move || {
                    windows::find(target.window_id).map(|w| (w.app, w.bounds))
                })
                .await;
                log::info!(
                    "window {} left display {id} after placing, for {now:?}; placing again",
                    target.window_id,
                );
                let _ = place(target, id, cx).await;
            }
        }
    })
    .detach();
}

/// Wait for the display to stop changing after an action, then bring back
/// any window the action grew past the display's edges.
async fn settle_display(core: &Entity<Core>, id: DisplayId, size: (f64, f64), cx: &mut AsyncApp) {
    wait_until_still(id, size, cx).await;
    adopt_opened(core, id, cx).await;
    let moved = cx
        .background_executor()
        .spawn(async move { windows::keep_inside(id) })
        .await;
    if moved {
        wait_until_still(id, size, cx).await;
    }
    // Some apps resize after the display looked still: Calculator takes
    // over 300 ms to switch to Scientific. Look again a little later.
    let core = core.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        for wait in [400, 800] {
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            adopt_opened(&core, id, cx).await;
            cx.background_executor()
                .spawn(async move { windows::keep_inside(id) })
                .await;
        }
    })
    .detach();
}

/// Wait until the display stops changing after an action, so the
/// screenshot shows its result: no new frame for 120 ms, after the first
/// change, or 500 ms with none; at most 1.5 s.
/// While agents act, the display streams at 30 fps at thumbnail size, and
/// ScreenCaptureKit only delivers a frame when something changed.
/// The small stream that counts a display's changes, at the size it runs.
fn change_pixels(size: (f64, f64)) -> (u32, u32) {
    (416, (416.0 * size.1 / size.0) as u32)
}

/// Before an action: have the display's changes counted, so the stream is
/// running by the time the screenshot after it waits for the display to
/// settle. A stream started only then misses changes while it starts.
/// Keeps `"agent-warm"` subscribed for 30 s so the capture stream stays warm
/// without leaving CPU luma counting (`"agent"`) on between settles.
fn watch_for_changes(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) {
    let Some(streams) = crate::ui::stream::global() else {
        return;
    };
    let size = core
        .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
        .unwrap_or(DEFAULT_SIZE);
    let px = change_pixels(size);
    streams.update(cx, |s, cx| {
        s.want(id, "agent-warm", 30, px, cx);
        s.want(id, "agent", 30, px, cx);
    });
    let at = Instant::now();
    ACTION_BASE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, at);
    AGENT_COUNT_AT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, at);
    // Safety valve if an action is sent without a follow-up `wait_until_still`
    // (for example, direct control-socket `click`): stop CPU luma scanning
    // after 2 s once no settle wait or in-flight press check holds counting,
    // while keeping `"agent-warm"` alive for 30 s.
    let streams_weak = streams.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let still_same = AGENT_COUNT_AT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&id)
                .copied()
                == Some(at);
            match safety_timer_step(still_same, count_holds(id)) {
                SafetyTimerStep::Superseded => break,
                SafetyTimerStep::KeepWaiting => continue,
                SafetyTimerStep::StopCounting => {
                    AGENT_COUNT_AT
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id);
                    let _ = streams_weak.update(cx, |s, cx| s.want(id, "agent", 0, (0, 0), cx));
                    break;
                }
            }
        }
    })
    .detach();
    keep_counting(id, cx);
}

/// Process-wide RAII hold that keeps `"agent"` luma change counting active on
/// `id` while a settle wait or an in-flight `shared_app::Watcher` press check
/// runs, so `watch_for_changes`'s 2 s safety timer does not drop the display
/// signal if `AXPress` blocks for more than 2 s. Safe to create and drop on
/// different threads.
pub(super) struct ChangeCountHold {
    id: DisplayId,
}

impl Drop for ChangeCountHold {
    fn drop(&mut self) {
        let mut map = AGENT_COUNT_HOLDS.lock().unwrap_or_else(|e| e.into_inner());
        let next = map.get(&self.id).copied().unwrap_or(1).saturating_sub(1);
        if next == 0 {
            map.remove(&self.id);
        } else {
            map.insert(self.id, next);
        }
    }
}

pub(super) fn hold_change_counting(id: DisplayId) -> ChangeCountHold {
    let mut map = AGENT_COUNT_HOLDS.lock().unwrap_or_else(|e| e.into_inner());
    *map.entry(id).or_default() += 1;
    ChangeCountHold { id }
}

fn count_holds(id: DisplayId) -> usize {
    AGENT_COUNT_HOLDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .copied()
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SafetyTimerStep {
    Superseded,
    KeepWaiting,
    StopCounting,
}

fn safety_timer_step(still_same: bool, holds: usize) -> SafetyTimerStep {
    if !still_same {
        SafetyTimerStep::Superseded
    } else if holds > 0 {
        SafetyTimerStep::KeepWaiting
    } else {
        SafetyTimerStep::StopCounting
    }
}

/// Keep the display's capture stream warm for 30 s without holding `"agent"`
/// CPU luma counting active between settles.
fn keep_counting(id: DisplayId, cx: &mut AsyncApp) {
    let Some(streams) = crate::ui::stream::global() else {
        return;
    };
    let wanted = Instant::now();
    AGENT_ACTIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, wanted);
    let streams = streams.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor()
            .timer(Duration::from_secs(30))
            .await;
        let mut active = AGENT_ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if active.get(&id).copied() == Some(wanted) {
            active.remove(&id);
            drop(active);
            let _ = streams.update(cx, |s, cx| s.want(id, "agent-warm", 0, (0, 0), cx));
        }
    })
    .detach();
}

async fn wait_until_still(id: DisplayId, size: (f64, f64), cx: &mut AsyncApp) {
    let Some(streams) = crate::ui::stream::global() else {
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        return;
    };
    let px = change_pixels(size);
    let latest = streams.update(cx, |s, cx| {
        s.want(id, "agent-warm", 30, px, cx);
        s.want(id, "agent", 30, px, cx);
        s.latest(id)
    });
    let hold = hold_change_counting(id);
    let start = Instant::now();
    // The stream has been counting since the action, so no change after it
    // is lost to a stream still starting. An app may draw what an action
    // did a moment after its first change, as a page that turns red 400 ms
    // after a click whose counter updates at once (issue #16): so whatever
    // changed, the wait lasts half a second from the action, then until
    // the display has been still for 120 ms.
    let since = ACTION_BASE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
        .filter(|at| at.elapsed() < Duration::from_secs(5))
        .unwrap_or(start);
    // Watch `Latest::changes` off the main thread instead of hopping onto the
    // UI thread every 30 ms. Uses the executor's timer so no worker thread blocks.
    let bg = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            let read_changes = || {
                latest
                    .as_ref()
                    .map(|l| l.changes.load(std::sync::atomic::Ordering::Relaxed))
            };
            let mut last = read_changes();
            let mut quiet_since = start;
            while start.elapsed() < Duration::from_millis(1500) {
                let now = read_changes();
                if now != last {
                    last = now;
                    quiet_since = Instant::now();
                } else if quiet_since.elapsed() >= Duration::from_millis(120)
                    && since.elapsed() >= Duration::from_millis(500)
                {
                    break;
                }
                bg.timer(Duration::from_millis(30)).await;
            }
        })
        .await;
    drop(hold);
    if count_holds(id) == 0 {
        AGENT_COUNT_AT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        streams.update(cx, |s, cx| s.want(id, "agent", 0, (0, 0), cx));
    }
    keep_counting(id, cx);
}

/// act_on_element's press on an element that lists no AXPress (#219).
const UNLISTED_PRESS_NOTE: &str = "The element lists no accessibility press, as a page element in Dia doesn't, so the press went in as a background click at its center. Take a screenshot to see what it did.";

/// What an agent hears after pressing a menu item of an app built on VS
/// Code, such as Cursor: with the terminal focused, Edit > Find opened the
/// terminal's find box (`cargo e2e cursormenu`).
const VS_CODE_MENU_NOTE: &str = "This app is built on VS Code. It ran this menu item as the item's keyboard shortcut in whatever has focus, not as the menu's own command, because Electron reports a menu item pressed through accessibility as its shortcut. With the terminal, a panel or an input box focused, the result can differ: Edit > Find opened the terminal's find box. Check the window. To run the command itself, open the Command Palette with key cmd+shift+p, type the command's name and press Return.";

/// What an agent hears after pressing an Electron app's menu item.
const ELECTRON_MENU_NOTE: &str = "This is an Electron app. Electron reports a menu item pressed through accessibility to the app as its keyboard shortcut (event.triggeredByAccelerator is true), and some apps ignore shortcuts while a text field has focus. If the window shows no change, press the item again with real_input: true: once the user allows it, Agent Screen chooses it with a real click on the display's menu bar. Or move focus out of the text field, with a click on an empty part of the window, then press it again.";

/// Process-wide change-counting state per display (not `thread_local!`, so
/// holds created or dropped on background threads stay visible to main-thread
/// timers without count drift).
static AGENT_ACTIVE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<DisplayId, Instant>>,
> = std::sync::LazyLock::new(Default::default);
static ACTION_BASE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<DisplayId, Instant>>,
> = std::sync::LazyLock::new(Default::default);
static AGENT_COUNT_AT: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<DisplayId, Instant>>,
> = std::sync::LazyLock::new(Default::default);
static AGENT_COUNT_HOLDS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<DisplayId, usize>>,
> = std::sync::LazyLock::new(Default::default);

/// Screenshot pixels per display point: below 1 on displays whose
/// screenshot is scaled down.
fn display_scale(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> f64 {
    let size = core
        .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
        .unwrap_or(DEFAULT_SIZE);
    shot_scale(size)
}

/// A window's bounds, in display points, as screenshot pixels: the space
/// the computer tool clicks in.
fn in_shot(mut window: Value, scale: f64) -> Value {
    for key in ["x", "y", "width", "height"] {
        if let Some(n) = window[key].as_f64() {
            window[key] = json!((n * scale).round() as i64);
        }
    }
    window
}

/// Where an agent's keys go: posted to the app, or, when it is the user's
/// front app with their window focused, beside that window (#110).
enum Gate {
    Post,
    Beside(Beside),
}

async fn keys_gate(
    forwarder: &crate::mac::input::Forwarder,
    within: Rect,
    cx: &mut AsyncApp,
) -> Result<Gate, ControlError> {
    let Some(target) = forwarder.keys_target(within).await else {
        return Err(no_keyboard_target());
    };
    match shared_app::front(target, cx).await {
        // In front with the agent's window focused, as when an app it
        // started activates itself, keys go to that window: fine.
        (Front::No | Front::TargetFocused, _) => Ok(Gate::Post),
        (Front::OtherFocused, Some(focused)) => Ok(Gate::Beside(Beside { target, focused })),
        _ => {
            let name = windows::find(target.window_id).map_or_else(|| "that app".into(), |w| w.app);
            Err(shared_app::unknown_focus(target, &name))
        }
    }
}

/// Keys that go out only as posted events, as a held key or the input
/// method's: refused beside the user's window.
async fn keys_allowed(
    forwarder: &crate::mac::input::Forwarder,
    within: Rect,
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
    match keys_gate(forwarder, within, cx).await? {
        Gate::Post => Ok(()),
        Gate::Beside(beside) => Err(shared_app::refusal(beside, "the keys")),
    }
}

fn forwarder_stopped(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> bool {
    core.update(cx, |c, _| c.forwarder(id)).agent_stopped()
}

/// Keys through the user's input method (ADR 0008). The display says the
/// agent has the user's keyboard while the keys go in, and the Red Glow and
/// the Input Bar show on the user's screens: without the glow, no key goes.
async fn through_input_method(
    core: &Entity<Core>,
    id: DisplayId,
    keys: Vec<KeyChord>,
    source: Option<String>,
    cx: &mut AsyncApp,
) -> Result<Params, ControlError> {
    // Refused before the glow shows: no flash for a call that sends nothing.
    crate::mac::ime::may_borrow(keys.len()).map_err(ControlError::failed)?;
    let forwarder = core.update(cx, |c, _| c.forwarder(id));
    let bounds = cg::display_bounds(id);
    keys_allowed(&forwarder, bounds, cx).await?;
    let Some(target) = forwarder.keys_target(bounds).await else {
        forwarder.keys_done().await;
        return Err(no_keyboard_target());
    };
    // Calls can overlap, and one that loses the session's input lock
    // returns while another types: each returns only its own borrow, so
    // the glow stays while any runs. A Stop pressed for a running borrow
    // stays pressed.
    let first = core.update(cx, |c, cx| {
        let first = !c.displays.list.iter().any(|d| d.keyboard_borrowed());
        if let Some(display) = c.displays.get_mut(id) {
            display.borrow_keyboard();
        }
        cx.notify();
        first
    });
    let give_back = |core: &Entity<Core>, cx: &mut AsyncApp| {
        core.update(cx, |c, cx| {
            if let Some(display) = c.displays.get_mut(id) {
                display.return_keyboard();
            }
            cx.notify();
        })
    };
    if first {
        crate::mac::ime::clear_stop();
    }
    if !sync_glow(cx) {
        give_back(core, cx);
        sync_glow(cx);
        forwarder.keys_done().await;
        return Err(ControlError::failed(
            "Agent Screen couldn't show the red glow on the user's screens, which must show while their keyboard is borrowed, so nothing was typed",
        ));
    }
    let total = keys.len();
    let waker = forwarder.clone();
    let wake = move || {
        if let Some(bounds) = cg::window_bounds(target.window_id) {
            futures::executor::block_on(waker.wake_now(target, bounds));
        }
    };
    let typed = cx
        .background_executor()
        .spawn(async move { crate::mac::ime::type_keys(target, &keys, source.as_deref(), wake) })
        .await;
    give_back(core, cx);
    sync_glow(cx);
    forwarder.keys_done().await;
    match typed {
        Ok(crate::mac::ime::Typed::All) => Ok(Params::new()),
        Ok(crate::mac::ime::Typed::Halted(sent)) => Err(ControlError::failed(format!(
            "the user pressed Stop after {sent} of {total} keys, so the input method stopped and their app is in front again. Take a screenshot, and don't send the rest unless they ask."
        ))),
        Ok(crate::mac::ime::Typed::Stopped(sent)) => Err(ControlError::failed(format!(
            "the user touched the keyboard or pointer after {sent} of {total} keys, so the input method stopped and their app is in front again. Take a screenshot, then send the rest once they've stopped."
        ))),
        Err(reason) => Err(ControlError::failed(reason)),
    }
}

/// Show the Red Glow as the facts now call for, at once. Whether every
/// user screen shows it; false without a UI.
pub(crate) fn sync_glow(cx: &mut AsyncApp) -> bool {
    crate::ui::shell::current()
        .and_then(|shell| shell.update(cx, |shell, cx| shell.sync_glow(cx)).ok())
        .unwrap_or(false)
}

/// Why keys found nowhere to go: their app's focused window was off the
/// Agent Displays, so they were refused (#120), or no window on the
/// display takes keys.
async fn keys_refused(forwarder: &crate::mac::input::Forwarder, id: DisplayId) -> ControlError {
    match forwarder.take_refused().await {
        Some(refused) => refused_error(&refused, id),
        None => no_keyboard_target(),
    }
}

fn refused_error(refused: &crate::mac::input::Refused, id: DisplayId) -> ControlError {
    let app = &refused.app;
    let focused = match &refused.window {
        Some(title) if !title.is_empty() => {
            format!("{app}'s focused window, {title:?}, is not on an Agent Display")
        }
        Some(_) => format!("{app}'s focused window is untitled and not on an Agent Display"),
        None => format!(
            "Accessibility could not say which window of {app} has focus, and {app} has a window that is not on an Agent Display"
        ),
    };
    ControlError::failed(format!(
        "{focused}, so keys posted to {app} could reach the user's window instead of yours on display {id}. Agent Screen refused the keys before sending any, and won't bring {app} to the front to fix that. Use act_on_element insert_text or set_value on the field, or click inside your window once and retry."
    ))
}

fn no_keyboard_target() -> ControlError {
    ControlError::not_found(
        "no window on the display can take keyboard input; launch an app or click a field first",
    )
}

/// For 30 s after a cold start, decide each new window of the app that the
/// catch didn't, by ADR 0007's rule (`new_window::watch`): one the app made
/// before it took observers, or while too busy to answer Accessibility, as
/// WeCom's main window, 3.4 s after its loading window, was. Without a kept
/// catch yet, it tries for one again. A window the app can't describe yet
/// waits, up to 60 s from the launch. `shown`: launch_app answered with a
/// window of the app. `launched_at`: when the cold start began.
#[allow(clippy::too_many_arguments)]
fn watch_cold_start(
    core: &Entity<Core>,
    id: DisplayId,
    pid: i32,
    mut seen: std::collections::HashSet<u32>,
    mut shown: bool,
    launched_at: Instant,
    screen: Option<String>,
    cx: &mut AsyncApp,
) {
    use agent_screen_model::new_window::{
        On, Role, Seen, Step, Tick, Watch, retries_catch, step, watch,
    };
    let core = core.clone();
    let bounds = cg::display_bounds(id);
    cx.spawn(async move |cx: &mut AsyncApp| {
        // Each window the app didn't describe yet: what was known as it
        // first showed, and when, so the user's input since counts.
        let mut first_seen: std::collections::HashMap<u32, (Seen, Instant)> = Default::default();
        for tick in 0u32.. {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let started = takes_windows(&core, id, cx)
                && core.read_with(cx, |c, _| {
                    c.displays.get(id).is_some_and(|d| d.started.contains(&pid))
                });
            let hidden = off(cx, move || app::is_hidden(pid)).await;
            // Only asked when it would move the hidden app's windows.
            let theirs = hidden && !shown && {
                let users = core.update(cx, |c, _| users_windows(c, id));
                off(cx, move || {
                    ax::elements_attribute(&ax::application(pid), "AXWindows")
                        .iter()
                        .any(|w| ax::window_id(w).is_some_and(|w| users.contains(&w)))
                })
                .await
            };
            let now = step(Tick {
                elapsed: launched_at.elapsed(),
                started,
                hidden,
                shown,
                theirs,
                waiting: !first_seen.is_empty(),
            });
            match now {
                Step::Stop => return,
                Step::GiveUp => {
                    for window in first_seen.keys() {
                        log::info!(
                            "launch: pid {pid} never said what window {window} is; left where it is"
                        );
                    }
                    core.update(cx, |c, _| {
                        c.left_alone
                            .extend(first_seen.keys().map(|window| (id, *window)))
                    });
                    return;
                }
                // Its windows are listed nowhere: move them in, then show.
                Step::ShowHidden => {
                    off(cx, move || {
                        if ax::move_all_into(pid, bounds) > 0 {
                            app::unhide(pid);
                        }
                    })
                    .await;
                    continue;
                }
                Step::Skip => continue,
                Step::Look | Step::Resolve => {}
            }
            // No catch yet, as for an app that took no observers while it
            // started: try for one again now and then.
            let caught = |c: &Core| c.kept_catches.iter().any(|(_, k)| k.pid() == pid);
            if now == Step::Look && retries_catch(tick) && !core.read_with(cx, |c, _| caught(c)) {
                let catch = crate::mac::catch::Catch::start(pid, id, screen.clone(), None);
                keep_catch(&core, id, catch.await, cx);
            }
            let others: Vec<Rect> = core.read_with(cx, |c, _| {
                c.displays
                    .list
                    .iter()
                    .filter(|d| d.id != id)
                    .map(|d| cg::display_bounds(d.id))
                    .collect()
            });
            let listed = seen.clone();
            let known = first_seen.clone();
            // Past the cold start, only the windows seen in it.
            let only_waiting = now == Step::Resolve;
            // Every window's facts first, then what each is: asking a busy
            // app takes up to 1 s a window.
            let late: Vec<(windows::Window, On, Seen, Instant, Role)> = off(cx, move || {
                let found: Vec<(windows::Window, On, Seen, Instant)> = windows::all()
                    .into_iter()
                    .filter(|w| {
                        let window = w.target.window_id;
                        w.target.pid == pid
                            && w.layer == 0
                            && !listed.contains(&window)
                            && (!only_waiting || known.contains_key(&window))
                    })
                    .map(|w| {
                        let mid = w.bounds.mid();
                        let on = if bounds.contains(mid) {
                            On::ThisDisplay
                        } else if others.iter().any(|d| d.contains(mid)) {
                            On::OtherDisplay
                        } else {
                            On::Elsewhere
                        };
                        let window = w.target.window_id;
                        let (facts, sighted) = known.get(&window).copied().unwrap_or_else(|| {
                            (
                                crate::mac::catch::seen(id, pid, bounds, Some(window)),
                                Instant::now(),
                            )
                        });
                        (w, on, facts, sighted)
                    })
                    .collect();
                found
                    .into_iter()
                    .map(|(w, on, facts, sighted)| {
                        let role = crate::mac::catch::role(w.target);
                        (w, on, facts, sighted, role)
                    })
                    .collect()
            })
            .await;
            shown |= !late.is_empty();
            for (window, on, facts, sighted, role) in late {
                let target = window.target;
                // Read right before deciding, as reading roles and settling
                // other windows take seconds: a window the user clicked
                // into since it showed isn't the agent's, and a taken-back
                // app is the user's.
                let user_since =
                    off(cx, move || crate::mac::catch::user_acted_since(pid, sighted)).await;
                let facts = facts.with_user_now(user_since);
                let decided = !still_agents(&core, id, target, cx);
                let verdict = watch(on, decided, role, facts);
                if verdict == Watch::Later {
                    first_seen.insert(target.window_id, (facts, sighted));
                    continue;
                }
                seen.insert(target.window_id);
                first_seen.remove(&target.window_id);
                if !decided && facts.ends_cold_start() {
                    crate::mac::catch::end_cold_start(id, pid);
                }
                match verdict {
                    Watch::Leave | Watch::Later => {}
                    // As the catch left it: placed and recorded the way
                    // launch_app places its window.
                    Watch::Record => {
                        let _ = settle(&core, target, id, cx).await;
                    }
                    Watch::Place => {
                        log::info!(
                            "launch: pid {pid} made window {} past its catch; the agent's ({facts:?})",
                            target.window_id
                        );
                        let _ = settle(&core, target, id, cx).await;
                    }
                    Watch::Theirs => {
                        log::info!(
                            "launch: pid {pid} made window {} past its catch while the user acted on it; theirs",
                            target.window_id
                        );
                        core.update(cx, |c, _| {
                            if let Some(d) = c.displays.get_mut(id) {
                                d.user_made.insert(target.window_id);
                            }
                        });
                    }
                    Watch::Undecided => {
                        log::info!(
                            "launch: pid {pid} made window {} past its catch ({facts:?}, {role:?}); left where it is",
                            target.window_id
                        );
                        core.update(cx, |c, _| c.left_alone.push((id, target.window_id)));
                    }
                }
            }
        }
    })
    .detach();
}

/// The windows of the display's apps that are the user's: made by them,
/// taken back, or left with them undecided.
fn users_windows(c: &mut Core, id: DisplayId) -> std::collections::HashSet<u32> {
    sync_user_made(c, id);
    let undecided = c
        .kept_catches
        .iter()
        .filter(|(display, _)| *display == id)
        .flat_map(|(_, k)| k.undecided());
    let left = c
        .left_alone
        .iter()
        .filter(|(display, _)| *display == id)
        .map(|(_, w)| *w);
    let mut theirs: std::collections::HashSet<u32> = undecided.chain(left).collect();
    if let Some(d) = c.displays.get(id) {
        theirs.extend(d.user_made.iter().chain(&d.returned).copied());
    }
    theirs
}

/// The display still exists and has not ended, so windows may go onto it.
fn takes_windows(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> bool {
    core.read_with(cx, |c, _| c.displays.get(id).is_some_and(|d| !d.is_ended()))
}

// MARK: Input Queue

async fn input_queue_action(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = display(core, p, true, cx)?;
    let owner = core
        .read_with(cx, |c, _| c.input_queue.owner(id, connection))
        .map_err(ControlError::invalid)?;
    let request = p
        .int("requestId")
        .map(|id| {
            u64::try_from(id)
                .ok()
                .filter(|id| *id != 0)
                .ok_or_else(|| ControlError::invalid("requestId must be a positive integer"))
        })
        .transpose()?;
    // Checked before any change: done forgets results, so a request id
    // with it would fail after the session's requests were already gone.
    let action = Action::parse(
        &p.string("action").unwrap_or_else(|| "status".into()),
        request,
    )
    .map_err(ControlError::invalid)?;
    let wait_for = match action {
        Action::Request => {
            let operation = p
                .string("operation")
                .ok_or_else(|| ControlError::invalid("operation is required"))?;
            let reason = p
                .string("reason")
                .ok_or_else(|| ControlError::invalid("reason is required"))?;
            Some(
                core.update(cx, |c, cx| {
                    let result = c.input_queue.submit(owner.clone(), operation, reason, None);
                    cx.notify();
                    result
                })
                .map_err(ControlError::invalid)?,
            )
        }
        Action::Withdraw(request) => {
            core.update(cx, |c, cx| {
                let result = c.input_queue.withdraw(&owner, request);
                cx.notify();
                result
            })
            .map_err(ControlError::invalid)?;
            Some(request)
        }
        Action::Done => {
            core.update(cx, |c, cx| {
                c.input_queue.done(&owner);
                cx.notify();
            });
            None
        }
        Action::Wait(request) => Some(request),
        Action::Status(request) => request,
    };
    if let Some(request) = wait_for {
        core.read_with(cx, |c, _| c.input_queue.request_for(&owner, request))
            .map_err(ControlError::invalid)?;
    }
    // A front request runs once its app leaves the front, while a call
    // waits for it: this one.
    if let Action::Wait(request) = action
        && core.read_with(cx, |c, _| c.input_queue.front(request).is_some())
    {
        return super::front_queue::wait(core, connection, id, request, cx).await;
    }
    // A wait on a drag makes it runnable again: its result comes back here.
    if let Action::Wait(request) = action {
        core.update(cx, |c, _| c.input_queue.attach(request));
    }
    let waits = matches!(action, Action::Request | Action::Wait(_));
    let timeout = if waits {
        p.number("timeout").unwrap_or(60.0).clamp(0.0, 60.0)
    } else {
        0.0
    };
    let until = Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let state = core.update(cx, |c, cx| -> Result<(Value, bool), ControlError> {
            if !c
                .input_queue
                .owner(id, connection)
                .is_ok_and(|current| current == owner)
            {
                return Err(ControlError::failed(
                    "the input request's display lifetime ended; any running drag is stopping. Inspect its result before retrying",
                ));
            }
            let changed = c.input_queue.tick();
            let paused = c.displays.get(id).is_some_and(|d| d.takeover);
            let state = c.input_queue.status(&owner, wait_for, paused);
            let waiting = waits && wait_for.is_some_and(|r| c.input_queue.awaits(r, paused));
            // A started loan must release its inputs before this call
            // returns, even if the approval wait's deadline has passed.
            let running = wait_for.is_some_and(|r| {
                c.input_queue.queue.request(r).is_some_and(|r| {
                    matches!(r.status, agent_screen_model::input_queue::Status::Running
                        | agent_screen_model::input_queue::Status::Stopping(_))
                })
            });
            if waiting && !running
                && Instant::now() >= until
                && let Some(r) = wait_for
            {
                c.input_queue.detach(r);
            }
            if changed {
                cx.notify();
            }
            Ok((state, waiting && (running || Instant::now() < until)))
        })?;
        let (state, waiting) = state;
        if !waiting {
            return Ok(state);
        }
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
    }
}

async fn queued_drag(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    path: ((f64, f64), Option<(f64, f64)>),
    modifiers: crate::keychord::Modifiers,
    why: &str,
    cx: &mut AsyncApp,
) -> Reply {
    let (from, to) = path;
    let (target, frame) = off(cx, move || crate::mac::input::real_drag_window_at(from))
        .await
        .ok_or_else(|| ControlError::not_found("no window at the drag's start"))?;
    // The window under the end may be another window of the display, as
    // when a file goes from Finder onto a web page (ADR 0013, "Drags
    // between windows").
    let (drop, drop_frame) = match to {
        Some(to) => off(cx, move || crate::mac::input::real_drag_window_at(to))
            .await
            .ok_or_else(|| ControlError::not_found("no window at the drag's end"))?,
        None => (target, frame),
    };
    let area = cg::display_bounds(id);
    let point = |p: (f64, f64)| ((p.0 - area.x).round() as i64, (p.1 - area.y).round() as i64);
    let reason = match to {
        Some(to) => format!(
            "Drag from {:?} to {:?} on this display",
            point(from),
            point(to)
        ),
        None => format!("Begin dragging at {:?} on this display", point(from)),
    };
    // A press held across calls would keep the user's button down while the
    // agent thinks, so only a whole drag runs; a press only asks.
    let plan = to.map(|to| crate::input_queue::Drag {
        target,
        frame,
        drop,
        drop_frame,
        area,
        from,
        to,
        flags: crate::mac::input::flags(modifiers).0,
    });
    let operation = format!(
        "drag:{}:{from:?}:{to:?}:{}",
        target.window_id,
        modifier_bits(modifiers)
    );
    let owner = core
        .read_with(cx, |c, _| c.input_queue.owner(id, connection))
        .map_err(ControlError::invalid)?;
    let request = core
        .update(cx, |c, cx| {
            let result = c.input_queue.submit(
                owner.clone(),
                operation,
                reason,
                plan.map(crate::input_queue::Plan::Drag),
            );
            cx.notify();
            result
        })
        .map_err(ControlError::invalid)?;
    let params = json!({ "displayId": id, "action": "wait", "requestId": request, "timeout": 60 });
    let state = input_queue_action(
        core,
        connection,
        &Args(params.as_object().expect("queue params")),
        cx,
    )
    .await?;
    let r = &state["requests"][0];
    match r["state"].as_str() {
        Some("completed") => {
            show_pointer(core, id, to.unwrap_or(from), false, cx);
            reply(json!({
                "route": "real_input",
                "requestId": request,
                "timing": r["timing"],
                "note": "The drag ran with the user's pointer, which stays on this display until the approval ends. Call input_queue done when you need no more drags with it: the pointer and the user's front app then come back at once, instead of 30 s after your last drag.",
            }))
        }
        Some("stopped") if r["stopReason"] != "Replaced" && r["stopReason"] != "Withdrawn" => {
            Err(ControlError::failed(format!(
                "The drag with the user's pointer stopped partway ({}{}); the button was let go. Take a screenshot before retrying. Input queue status: {state}",
                r["stopReason"].as_str().unwrap_or("stopped"),
                r["failure"]
                    .as_str()
                    .map(|f| format!(": {f}"))
                    .unwrap_or_default(),
            )))
        }
        _ => Err(ControlError {
            blocked: None,
            code: "input_pending",
            message: format!(
                "{why} No drag or press was sent. Input queue status: {state}. {}",
                if plan.is_some() {
                    "The drag runs with the user's pointer once they allow it and while your call waits: retry the same drag with real_input to wait again, or use input_queue to inspect or withdraw it."
                } else {
                    "A press held across calls can't use the user's pointer. Once they allow it, use left_click_drag with real_input for the whole drag."
                }
            ),
        }),
    }
}

fn real_click_button(button: Button) -> real_click::Button {
    match button {
        Button::Left => real_click::Button::Left,
        Button::Right => real_click::Button::Right,
        Button::Middle => real_click::Button::Middle,
    }
}

/// A click with the user's real pointer, once they allow it (ADR 0013,
/// "Clicks"). The agent asks after a background click visibly had no
/// effect; nothing goes out on the background route while the request is
/// outstanding, so the click never runs twice.
async fn queued_click(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    at: (f64, f64),
    click: real_click::Click,
    modifiers: crate::keychord::Modifiers,
    cx: &mut AsyncApp,
) -> Reply {
    let area = cg::display_bounds(id);
    let (x, y) = real_click::display_point(at, area);
    let hit = off(cx, move || {
        let on_disp = windows::on_display(id);
        let targets: Vec<WindowTarget> = on_disp.iter().map(|w| w.target).collect();
        let hit = crate::mac::input::real_click_at(at, &targets)?;
        let window = on_disp.into_iter().find(|w| w.target == hit.window)?;
        let frame = window.bounds.offset(area.x, area.y);
        Some((hit, frame, window.app, window.title))
    })
    .await;
    let Some((hit, frame, app, title)) = hit else {
        return Err(ControlError::not_found(format!(
            "no window of yours at ({x}, {y}) on display {id}, so no click was asked for"
        )));
    };
    let target = hit.window;
    let hit_frame = hit.menu.map_or(frame, |(_, mf)| mf);
    if !real_click::contained(at, area, hit_frame) {
        return Err(ControlError::invalid(
            "the window under that point reaches past your Agent Display, so its click can't use the user's pointer",
        ));
    }
    let ax_pid = hit.ax_pid;
    // What the agent aimed at, to compare just before the loan clicks: a
    // list can reorder while the user decides.
    let (region, look, element) = off(cx, move || {
        // Chromium and Electron publish their pages' elements only once
        // asked.
        if hit.menu.is_none() {
            crate::mac::ax::ensure_web_tree(target);
        }
        let (element, framed) =
            crate::mac::ax::target_at(ax_pid, at, std::time::Duration::from_secs(1), true);
        let region = real_click::look_region(at, area, hit_frame, framed);
        (region, crate::mac::screenshot::samples(id, region), element)
    })
    .await;
    log::info!(
        "input queue: a real click at ({x}, {y}) will compare element {element:?} and region {region:?}"
    );
    let look = look.map_err(|why| {
        ControlError::failed(format!(
            "Agent Screen could not capture what is under ({x}, {y}) to check it before the click ({why}). No click was asked for or sent; try again."
        ))
    })?;
    let app = if app.is_empty() {
        off(cx, move || app::name(target.pid))
            .await
            .unwrap_or_else(|| "the app".into())
    } else {
        app
    };
    let verb = match click.verb() {
        real_click::Verb::Click => "Click",
        real_click::Verb::DoubleClick => "Double-click",
        real_click::Verb::TripleClick => "Triple-click",
        real_click::Verb::RightClick => "Right-click",
        real_click::Verb::MiddleClick => "Middle-click",
    };
    let reason = format!("{verb} at ({x}, {y}) in {app} with the user's pointer");
    let plan = crate::input_queue::Click {
        target,
        frame,
        menu: hit.menu,
        ax_pid,
        area,
        at,
        click,
        flags: crate::mac::input::flags(modifiers).0,
        app,
        title: real_click::short_title(&title, 40),
        region,
        look,
        element: element.map(Box::new),
        asked_ms: 0,
    };
    let operation = real_click::operation(target.window_id, at, click, modifier_bits(modifiers));
    let token = operation.clone();
    let owner = core
        .read_with(cx, |c, _| c.input_queue.owner(id, connection))
        .map_err(ControlError::invalid)?;
    let request = core
        .update(cx, |c, cx| {
            let plan = crate::input_queue::Click {
                asked_ms: c.input_queue.now(),
                ..plan
            };
            let result = c.input_queue.submit(
                owner.clone(),
                operation,
                reason,
                Some(crate::input_queue::Plan::Click(plan)),
            );
            cx.notify();
            result
        })
        .map_err(ControlError::invalid)?;
    let params = json!({ "displayId": id, "action": "wait", "requestId": request, "timeout": 60 });
    let state = input_queue_action(
        core,
        connection,
        &Args(params.as_object().expect("queue params")),
        cx,
    )
    .await?;
    let r = &state["requests"][0];
    let ran = match r["state"].as_str() {
        Some("completed") => true,
        Some("stopped") if r["stopReason"] != "Replaced" && r["stopReason"] != "Withdrawn" => false,
        state_now => {
            let next = if state_now == Some("declined") {
                "The user declined: the same click again returns this answer. Use another way, such as act_on_element, or call request_help."
            } else {
                "The click runs with the user's pointer once they allow it and while your call waits: make the same click with real_input again to wait again, or use input_queue to inspect or withdraw it."
            };
            return Err(ControlError {
                blocked: None,
                code: "input_pending",
                message: format!(
                    "The {} at ({x}, {y}) waits for the user to allow their pointer. No click was sent, not on the background route either. Input queue status: {state}. {next}",
                    verb.to_lowercase()
                ),
            });
        }
    };
    // The session is done with the user's pointer unless it has more
    // waiting: the approval ends and their pointer and app come back.
    let changed = r["failure"].as_str() == Some(real_click::CONTENT_CHANGED);
    let (waited, refusals) = core.update(cx, |c, cx| {
        let asked = match c.input_queue.plan(request) {
            Some(crate::input_queue::Plan::Click(click)) => click.asked_ms,
            _ => c.input_queue.now(),
        };
        let waited = real_click::waited_seconds(asked, c.input_queue.now());
        let refusals = c
            .input_queue
            .content_changed(&owner, changed.then_some(token.as_str()));
        c.input_queue.loan_done(&owner, request);
        cx.notify();
        (waited, refusals)
    });
    if ran {
        show_pointer(core, id, at, true, cx);
        return reply(json!({
            "windowId": target.window_id,
            "route": "real_input",
            "requestId": request,
            "waitedSeconds": waited,
            "timing": r["timing"],
        }));
    }
    if changed && refusals >= 2 {
        return Err(ControlError::failed(format!(
            "Nothing was clicked: what is under ({x}, {y}) changed again while the request waited, {waited} s this time, so that area keeps changing, as a video or an animation does. Asking again would likely end the same way, and asks the user again. Use act_on_element on the element there instead, or call request_help."
        )));
    }
    if changed {
        return Err(ControlError::failed(format!(
            "Nothing was clicked: what is under ({x}, {y}) looks different from when you asked, {waited} s ago, so the click could land on something else, such as another row of a list that reordered. Take a new screenshot, find the target again, and ask again with real_input."
        )));
    }
    Err(ControlError::failed(format!(
        "The {} with the user's pointer stopped ({}{}); every button it pressed was let go. Take a screenshot before retrying. Input queue status: {state}",
        verb.to_lowercase(),
        r["stopReason"].as_str().unwrap_or("stopped"),
        r["failure"]
            .as_str()
            .map(|f| format!(": {f}"))
            .unwrap_or_default(),
    )))
}

/// The Input Queue's token for a real click on a menu item: the same press
/// made again finds its request.
fn menu_operation(target: WindowTarget, path: &[(usize, String)]) -> String {
    let path: Vec<String> = path
        .iter()
        .map(|(index, title)| format!("{index}={title}"))
        .collect();
    format!("menu:{}:{}", target.window_id, path.join("/"))
}

/// Choose an item of an Electron app's menu bar with a real click, once the
/// user allows it (ADR 0013, "Menu items of Electron apps"). Nothing goes
/// through accessibility while the request is outstanding: the item would
/// run twice, once as a shortcut.
async fn queued_menu_press(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    target: WindowTarget,
    path: Vec<(usize, String)>,
    cx: &mut AsyncApp,
) -> Reply {
    let area = cg::display_bounds(id);
    let app = off(cx, move || app::name(target.pid))
        .await
        .unwrap_or_else(|| "the app".into());
    let item = crate::input_queue::MenuItem {
        target,
        area,
        app: app.clone(),
        path,
    };
    let label = agent_screen_model::menu_press::label(&item.titles());
    let reason = format!("Choose {label} in {app} with a click on its menu");
    let operation = menu_operation(target, &item.path);
    let owner = core
        .read_with(cx, |c, _| c.input_queue.owner(id, connection))
        .map_err(ControlError::invalid)?;
    let request = core
        .update(cx, |c, cx| {
            let result = c.input_queue.submit(
                owner.clone(),
                operation,
                reason,
                Some(crate::input_queue::Plan::MenuItem(item)),
            );
            cx.notify();
            result
        })
        .map_err(ControlError::invalid)?;
    let params = json!({ "displayId": id, "action": "wait", "requestId": request, "timeout": 60 });
    let state = input_queue_action(
        core,
        connection,
        &Args(params.as_object().expect("queue params")),
        cx,
    )
    .await?;
    let r = &state["requests"][0];
    let ran = match r["state"].as_str() {
        Some("completed") => true,
        Some("stopped") if r["stopReason"] != "Replaced" && r["stopReason"] != "Withdrawn" => false,
        state_now => {
            let next = if state_now == Some("declined") {
                "The user declined: pressing the same item again returns this answer. Press it with real_input: false to send it as the shortcut, or call request_help."
            } else {
                "The click runs with the user's pointer once they allow it and while your call waits: press the same item again to wait again, or use input_queue to inspect or withdraw it. To send it as the shortcut instead, withdraw the request and press the item with real_input: false."
            };
            return Err(ControlError {
                blocked: None,
                code: "input_pending",
                message: format!(
                    "{label} needs a real click, as Electron takes an item pressed through accessibility for its shortcut. No click was sent, and the item was not pressed through accessibility either. Input queue status: {state}. {next}"
                ),
            });
        }
    };
    // The session is done with the user's pointer unless it has more
    // waiting: the approval ends and their pointer and app come back.
    core.update(cx, |c, cx| {
        c.input_queue.loan_done(&owner, request);
        cx.notify();
    });
    if ran {
        return reply(json!({
            "route": "real_input",
            "requestId": request,
            "note": format!("Chose {label} with a real click on the display's menu bar, as the user allowed. Electron saw a click, not a shortcut."),
        }));
    }
    Err(ControlError::failed(format!(
        "Choosing {label} with the user's pointer stopped ({}{}). No key was sent and any menu it opened was closed. Take a screenshot before retrying. Input queue status: {state}",
        r["stopReason"].as_str().unwrap_or("stopped"),
        r["failure"]
            .as_str()
            .map(|f| format!(": {f}"))
            .unwrap_or_default(),
    )))
}

// MARK: Parameters

pub(super) fn display(
    core: &Entity<Core>,
    p: &Args,
    live: bool,
    cx: &mut AsyncApp,
) -> Result<DisplayId, ControlError> {
    let id = p
        .int("displayId")
        .ok_or_else(|| ControlError::invalid("displayId is required"))? as DisplayId;
    let named = p.string("sessionKey").filter(|k| !k.is_empty());
    let own = |key: Option<&String>| named.is_some() && key == named.as_ref();
    core.read_with(cx, |c, _| match c.displays.get(id) {
        None => match c.displays.endings.get(id) {
            Some(e) => Err(ended_error(id, e.reason, own(e.session_key.as_ref()))),
            None => Err(ControlError::not_found(format!(
                "display {id} does not exist; call create_display again"
            ))),
        },
        Some(d) if live && let Some(reason) = d.ended => Err(ended_error(
            id,
            reason,
            own(d.owner.as_ref().map(|o| &o.session_key)),
        )),
        Some(_) => Ok(id),
    })
}

/// A display the Agent Session may send input to right now.
async fn input_display(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<DisplayId, ControlError> {
    let id = display(core, p, true, cx)?;
    if core.read_with(cx, |c, _| c.displays.get(id).is_some_and(|d| d.takeover)) {
        return Err(ControlError::takeover(format!(
            "the user is operating display {id}. Call wait_for_control to wait until they hand it back."
        )));
    }
    // Remember the windows before the action, so the settle after it can
    // tell which windows the action opened.
    let before = cx
        .background_executor()
        .spawn(async { cg::all_ids() })
        .await;
    BEFORE.with(|b| b.borrow_mut().insert(id, (before, Instant::now())));
    Ok(id)
}

/// The window on display `id` whose title bar is at a global point, held
/// there, for an agent's drag.
async fn title_bar_at(id: DisplayId, at: (f64, f64), cx: &mut AsyncApp) -> Option<Grab> {
    cx.background_executor()
        .spawn(async move { Grab::on_display(id, at) })
        .await
}

/// Move a held window as far as the pointer moved from the press.
async fn drag_to(grab: Grab, to: (f64, f64), cx: &mut AsyncApp) -> bool {
    cx.background_executor()
        .spawn(async move { grab.drag_to(to) })
        .await
}

/// A drag an agent holds on a display across calls, from left_mouse_down
/// to left_mouse_up, in a web page.
enum Hold {
    /// Through the app's DevTools pipe.
    Pipe(devtools::Drag),
}

thread_local! {
    static HOLDS: std::cell::RefCell<std::collections::HashMap<DisplayId, Hold>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The user took the display over: an agent's drag held through a pipe
/// lets go where it is, as a posted one does.
pub fn release_hold(id: DisplayId) {
    if let Some(Hold::Pipe(drag)) = HOLDS.with(|h| h.borrow_mut().remove(&id)) {
        std::thread::spawn(move || drag.cancel());
    }
}

/// The user gave window `window` from display `id` to another session: a
/// drag the agent holds in it lets go, and a title-bar grab of it ends.
fn release_hold_on(id: DisplayId, window: u32) {
    let in_window = HOLDS.with(|h| {
        h.borrow()
            .get(&id)
            .is_some_and(|Hold::Pipe(drag)| drag.window() == window)
    });
    if in_window {
        release_hold(id);
    }
    GRAB.with(|g| {
        let mut grabs = g.borrow_mut();
        if grabs
            .get(&id)
            .is_some_and(|grab| grab.target.window_id == window)
        {
            grabs.remove(&id);
        }
    });
}

/// How a drag starting at a global point goes out (ADR 0010).
async fn web_route(at: (f64, f64), cx: &mut AsyncApp) -> Route {
    off(cx, move || {
        devtools::route(crate::mac::input::window_at(at), at)
    })
    .await
}

fn unmapped(why: String) -> ControlError {
    ControlError::failed(format!(
        "the app shows a web page there, but Agent Screen couldn't tell which of its pages is under the point ({why}), so nothing was sent. Take a screenshot and try again."
    ))
}

fn modifier_bits(m: crate::keychord::Modifiers) -> i64 {
    agent_screen_model::devtools::modifier_bits(m.alt, m.control, m.command, m.shift)
}

/// left_click_drag in a web page, through the app's DevTools pipe: a press,
/// a hold, 20 even steps and a pause before the release, as a posted drag
/// has. Nothing reaches WindowServer, so the session's button state and
/// the user's pointer stay theirs.
async fn pipe_drag(
    core: &Entity<Core>,
    id: DisplayId,
    page: devtools::Page,
    from: (f64, f64),
    to: (f64, f64),
    modifiers: crate::keychord::Modifiers,
    cx: &mut AsyncApp,
) -> Reply {
    if forwarder_stopped(core, id, cx) {
        return Err(ControlError::takeover(format!(
            "the user took over display {id}, so the drag was not sent. Call wait_for_control to wait until they hand it back."
        )));
    }
    // Keys go to the window dragged in, as after a posted drag.
    show_pointer(core, id, from, true, cx)
        .focus(page.window())
        .await;
    let bits = modifier_bits(modifiers);
    let mut drag = off(cx, move || devtools::Drag::press(page, from, bits))
        .await
        .map_err(|e| ControlError::failed(format!("the press didn't reach the page: {e}")))?;
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    for at in agent_screen_model::devtools::path(from, to, 20) {
        if forwarder_stopped(core, id, cx) {
            let at = drag.at();
            off(cx, move || drag.cancel()).await;
            show_pointer(core, id, at, false, cx);
            return Err(ControlError::takeover(format!(
                "the user took over display {id} during the drag, which let go short of its target and dropped nothing. Call wait_for_control to wait until they hand it back."
            )));
        }
        let (back, moved) = off(cx, move || {
            let moved = drag.move_to(at);
            (drag, moved)
        })
        .await;
        drag = back;
        if let Err(e) = moved {
            off(cx, move || drag.cancel()).await;
            return Err(ControlError::failed(format!(
                "the drag stopped partway, as a move didn't reach the page: {e}"
            )));
        }
        show_pointer(core, id, at, false, cx);
        cx.background_executor()
            .timer(Duration::from_millis(16))
            .await;
    }
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    off(cx, move || drag.release(to))
        .await
        .map_err(|e| ControlError::failed(format!("the release didn't reach the page: {e}")))?;
    show_pointer(core, id, to, false, cx);
    reply(json!({ "route": "devtools" }))
}

thread_local! {
    static GRAB: std::cell::RefCell<std::collections::HashMap<DisplayId, Grab>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Window ids before each display's latest agent action.
    static BEFORE: std::cell::RefCell<std::collections::HashMap<DisplayId, (std::collections::HashSet<u32>, Instant)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Windows the latest action opened in an app that has windows on the
/// display, such as TextEdit's new document after New Document, open where
/// the app chooses: the user's screen. Bring them to the display. Windows on
/// any Agent Display, the user's apps and older windows stay where they are.
async fn adopt_opened(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) {
    let Some(before) = BEFORE.with(|b| {
        b.borrow()
            .get(&id)
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(10))
            .map(|(ids, _)| ids.clone())
    }) else {
        return;
    };
    // Most actions open nothing; the id list is cheap to compare.
    let shown = off(cx, cg::on_screen_ids).await;
    if shown.is_subset(&before) {
        return;
    }
    let agent_areas: Vec<Rect> = core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .filter(|d| !d.is_ended())
            .map(|d| cg::display_bounds(d.id))
            .collect()
    });
    // The catches' rule: a window the user may have opened stays with
    // them, as does any window of an app they took back.
    let theirs = core.update(cx, |c, _| {
        sync_user_made(c, id);
        let Some(d) = c.displays.get(id) else {
            return (Default::default(), Default::default());
        };
        let undecided: std::collections::HashSet<u32> = c
            .kept_catches
            .iter()
            .filter(|(display, _)| *display == id)
            .flat_map(|(_, k)| k.undecided())
            .collect();
        (
            d.released.clone(),
            d.user_made
                .iter()
                .chain(&d.returned)
                .copied()
                .chain(undecided)
                .collect::<std::collections::HashSet<u32>>(),
        )
    });
    let (released, kept_out) = theirs;
    let opened = cx
        .background_executor()
        .spawn(async move {
            let pids: std::collections::HashSet<i32> = windows::on_display(id)
                .iter()
                .map(|w| w.target.pid)
                .collect();
            windows::all()
                .into_iter()
                .filter(|w| {
                    pids.contains(&w.target.pid)
                        && !released.contains(&w.target.pid)
                        && !kept_out.contains(&w.target.window_id)
                        && !before.contains(&w.target.window_id)
                        && w.layer == 0
                        && !agent_areas.iter().any(|a| a.contains(w.bounds.mid()))
                })
                .map(|w| w.target)
                .partition::<Vec<_>, _>(|w| !crate::mac::catch::user_acted_recently(w.pid))
        })
        .await;
    let (opened, left) = opened;
    // The user just acted on the app too: leave the window, and tell the
    // agent on its next screenshot.
    if !left.is_empty() {
        core.update(cx, |c, _| {
            c.left_alone.extend(left.iter().map(|w| (id, w.window_id)))
        });
    }
    for target in left.iter().chain(&opened) {
        BEFORE.with(|b| {
            if let Some((ids, _)) = b.borrow_mut().get_mut(&id) {
                ids.insert(target.window_id);
            }
        });
    }
    for target in opened {
        // Seen once is enough: a window that won't move isn't retried.
        BEFORE.with(|b| {
            if let Some((ids, _)) = b.borrow_mut().get_mut(&id) {
                ids.insert(target.window_id);
            }
        });
        if let Err(e) = settle(core, target, id, cx).await {
            log::info!(
                "could not bring an opened window to display {id}: {}",
                e.message
            );
        }
    }
}

/// The error for a call on an ended display: why it ended and what to do
/// (#326). One the user closed means they want the agent to stop. `own`:
/// the asking session is the one the display belonged to.
fn ended_error(id: DisplayId, reason: EndReason, own: bool) -> ControlError {
    ControlError::ended(agent_screen_model::ended::ended_message(id, reason, own))
}

/// The error for the asking session's own display that ended during its
/// call.
fn ended_during(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> ControlError {
    match core.read_with(cx, |c, _| c.displays.ended_reason(id)) {
        Some(reason) => ended_error(id, reason, true),
        None => ControlError::ended(format!(
            "display {id} has ended; call create_display for a new one"
        )),
    }
}

pub(super) async fn target(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<(DisplayId, (f64, f64)), ControlError> {
    let id = input_display(core, p, cx).await?;
    Ok((id, global(core, id, p.require("x")?, p.require("y")?, cx)?))
}

/// Screenshot pixel → global point. Agents aim in the screenshot, which is
/// scaled down for displays over the image budget.
fn global(
    core: &Entity<Core>,
    id: DisplayId,
    x: f64,
    y: f64,
    cx: &mut AsyncApp,
) -> Result<(f64, f64), ControlError> {
    let size = core
        .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
        .unwrap_or(DEFAULT_SIZE);
    let scale = shot_scale(size);
    let (w, h) = ((size.0 * scale).round(), (size.1 * scale).round());
    // Half-open: x == w is the next display's first column.
    if !(0.0..w).contains(&x) || !(0.0..h).contains(&y) {
        return Err(ControlError::invalid(format!(
            "({}, {}) is outside the {}×{} screenshot",
            x as i64, y as i64, w as i64, h as i64
        )));
    }
    let bounds: Rect = cg::display_bounds(id);
    Ok((bounds.x + x / scale, bounds.y + y / scale))
}

/// The strip at the top of the display holds the user's menu bar, left out
/// of screenshots: a click there would reach the user's front app. Main
/// thread.
fn outside_menu_bar(id: DisplayId, (_, y): (f64, f64)) -> Result<(), ControlError> {
    if y - cg::display_bounds(id).y < crate::mac::menubar_height(id) {
        return Err(ControlError::invalid(
            "that is the reserved menu bar strip at the top, which belongs to the user's app. \
             For app menus, press a menubaritem from get_window_state. If a zoomed window's toolbar is here, \
             use place_window with its window_id and x, y, width, height to move or shrink it below the strip; \
             read the returned window frame. If those arguments are missing, collect get_diagnostics in this MCP session before restarting its client.",
        ));
    }
    Ok(())
}

/// Show the Agent Pointer at a global point; returns the display's input queue.
fn show_pointer(
    core: &Entity<Core>,
    id: DisplayId,
    at: (f64, f64),
    click: bool,
    cx: &mut AsyncApp,
) -> crate::mac::input::Forwarder {
    let bounds = cg::display_bounds(id);
    let active_agent = core.read_with(cx, |c, _| {
        c.displays
            .get(id)
            .is_some_and(|d| !d.takeover && !d.is_ended())
    });
    crate::mac::recording::pointer(
        id,
        (at.0 - bounds.x, at.1 - bounds.y),
        (bounds.width, bounds.height),
        click,
        active_agent,
    );
    core.update(cx, |c, cx| {
        c.displays
            .point(id, at.0 - bounds.x, at.1 - bounds.y, click);
        cx.notify();
        c.forwarder(id)
    })
}

async fn window_on(
    id: DisplayId,
    p: &Args<'_>,
    cx: &AsyncApp,
) -> Result<windows::Window, ControlError> {
    let window = p
        .int("windowId")
        .ok_or_else(|| ControlError::invalid("windowId is required"))?;
    let (found, presence) = off(cx, move || {
        let found = windows::on_display(id)
            .into_iter()
            .find(|w| w.target.window_id as i64 == window);
        // A window the display listed that shows nowhere now may still be
        // open: say why, and what to ask the user. Asked only when it isn't
        // on the display; any other window id stays a plain not_found.
        let presence = match (&found, u32::try_from(window)) {
            (None, Ok(w)) => windows::seen_on(id, w).map(|seen| windows::presence(id, seen.target)),
            _ => None,
        };
        (found, presence)
    })
    .await;
    if let Some(found) = found {
        return Ok(found);
    }
    match presence.and_then(|p| p.advice(window, id)) {
        Some(advice) => Err(ControlError::off_screen(advice)),
        None => Err(ControlError::not_found(format!(
            "no window {window} on display {id}; see list_windows"
        ))),
    }
}

fn modifiers(p: &Args) -> Result<crate::keychord::Modifiers, ControlError> {
    let names: Vec<String> = match p.0.get("modifiers") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    };
    KeyChord::parse_modifiers(&names).map_err(|e| ControlError::invalid(e.to_string()))
}

/// Optional traversal scope is strict even on the raw control socket.
fn read_scope(p: &Args<'_>) -> Result<agent_screen_model::tree::Scope, ControlError> {
    let number = |key: &str| match p.0.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| ControlError::invalid(format!("{key} must be a non-negative integer"))),
    };
    Ok(agent_screen_model::tree::Scope {
        root: number("root")?,
        max_depth: number("maxDepth")?,
    })
}

/// A number retained from this window, checked against its live AX parent
/// chain. Failure is an invalid root, never a claim that the window closed.
async fn read_root(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    root: Option<usize>,
    cx: &mut AsyncApp,
) -> Result<Option<ax::Sendable>, ControlError> {
    let Some(number) = root else { return Ok(None) };
    let element = core.read_with(cx, |c, _| {
        c.elements.get(&(id, target.window_id)).and_then(|numbers| numbers.get(number)).cloned()
    }).ok_or_else(|| ControlError::invalid(format!(
        "root element {number} is unavailable in this window; call get_window_state without root and with a small max_depth to find it"
    )))?;
    let retained = ax::Sendable(element.clone());
    match off(cx, move || ax::belongs(retained.get(), target, false)).await {
        Some(true) => {}
        Some(false) => {
            return Err(ControlError::invalid(format!(
                "root element {number} no longer belongs to this window; call get_window_state without root to find it again"
            )));
        }
        None => {
            return Err(ControlError::failed(
                "the app didn't answer accessibility in time; it may be busy. Nothing was done; try again shortly",
            ));
        }
    }
    Ok(Some(ax::Sendable(element)))
}

thread_local! {
    /// Waits for change by connection and the MCP's wait id: running ones,
    /// with the way to stop each when its MCP call is cancelled, and cancels
    /// that came before their wait registered.
    static WAITS: std::cell::RefCell<std::collections::HashMap<(u64, u64), WaitSlot>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

enum WaitSlot {
    Running(futures::channel::oneshot::Sender<()>),
    /// Cancelled before it started, at this time.
    Cancelled(Instant),
}

/// Forget early cancels older than this, as for a wait that never came.
const EARLY_CANCEL: Duration = Duration::from_secs(30);

/// Register a wait: the receiver its cancel fires, or None when the cancel
/// already came.
fn register_wait(key: (u64, u64)) -> Option<futures::channel::oneshot::Receiver<()>> {
    WAITS.with(|w| {
        let mut waits = w.borrow_mut();
        waits.retain(
            |_, slot| !matches!(slot, WaitSlot::Cancelled(at) if at.elapsed() > EARLY_CANCEL),
        );
        if matches!(waits.remove(&key), Some(WaitSlot::Cancelled(_))) {
            return None;
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        waits.insert(key, WaitSlot::Running(tx));
        Some(rx)
    })
}

/// Stop a running wait, or remember the cancel for one not started yet.
fn cancel_wait(key: (u64, u64)) {
    WAITS.with(|w| {
        let mut waits = w.borrow_mut();
        match waits.remove(&key) {
            Some(WaitSlot::Running(stop)) => {
                let _ = stop.send(());
            }
            _ => {
                waits.insert(key, WaitSlot::Cancelled(Instant::now()));
            }
        }
    });
}

/// Takes a running wait out of `WAITS` however it ends.
struct Waiting(Option<(u64, u64)>);

impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(key) = self.0 {
            WAITS.with(|w| {
                let mut waits = w.borrow_mut();
                if matches!(waits.get(&key), Some(WaitSlot::Running(_))) {
                    waits.remove(&key);
                }
            });
        }
    }
}

/// wait_for_change: read one window's tree, off the main thread, until a
/// condition holds against the baseline, the window goes, the display
/// ends, the connection closes, the MCP cancels the call or the time is
/// up. The baseline is the tree the agent last got in this scope, which
/// the MCP sends, or the first read. Reads work in a Takeover, as
/// windowState's do.
async fn wait_for_change(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    use agent_screen_model::watch;
    let cancelled = || ControlError::failed("the wait was cancelled");
    // Registered before anything waits, so a cancel can't slip in first.
    // Without a wait id, from an older MCP, nothing cancels it.
    let key = p.int("waitId").map(|wait| (connection, wait as u64));
    let (_never, idle) = futures::channel::oneshot::channel();
    let mut cancel = match key {
        Some(key) => register_wait(key).ok_or_else(cancelled)?,
        None => idle,
    }
    .fuse();
    let _waiting = Waiting(key);
    let scope = read_scope(p)?;
    let conditions = watch::Conditions::from_json(p.0.get("conditions").unwrap_or(&Value::Null))
        .map_err(ControlError::invalid)?;
    let timeout = watch::timeout(p.number("timeoutSeconds")).map_err(ControlError::invalid)?;
    let id = display(core, p, true, cx)?;
    let window = window_on(id, p, cx).await?;
    let target = window.target;
    let root = read_root(core, id, target, scope.root, cx).await?;
    let area = cg::display_bounds(id);
    let origin = (area.x, area.y);
    let scale = display_scale(core, id, cx);
    let started = Instant::now();
    let deadline = started + timeout;
    // The agent's tree names numbers of this app run only if it read the
    // window since the app started.
    let numbered = core.read_with(cx, |c, _| c.elements.contains_key(&(id, target.window_id)));
    let baseline = p
        .string("baseline")
        .filter(|_| numbered)
        .map(|tree| watch::Read {
            tree,
            truncated: p.flag("baselineTruncated"),
        });
    let from_agent = baseline.is_some();
    let mut watch = watch::Watch::new(conditions, baseline);
    let mut depth_limited = false;
    let mut count = None;
    let mut unanswered = 0;
    let waited = |started: Instant| (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    let outcome = loop {
        let (closed, ended, reason) = core.read_with(cx, |c, _| {
            let d = c.displays.get(id);
            (
                c.closed.contains(&connection),
                d.is_none_or(|d| d.is_ended()),
                c.displays.ended_reason(id),
            )
        });
        if closed {
            return Err(ControlError::failed("the connection closed"));
        }
        if ended {
            return Ok(json!({
                "outcome": "displayEnded",
                "closedByUser": reason == Some(EndReason::ClosedByUser),
                "endedReason": reason.map(EndReason::tag),
                "endedMessage": reason
                    .map(|r| agent_screen_model::ended::ended_message(id, r, true)),
                "waitedSeconds": waited(started),
            }));
        }
        let window_id = target.window_id;
        // Off the display and on screen elsewhere, it left. Off screen, it
        // closed, or it is still open: minimized, its app hidden, or on a
        // Space the display isn't showing. Why is asked only then.
        let (here, elsewhere) = off(cx, move || windows::window_presence(id, window_id)).await;
        if !here {
            let presence = if elsewhere {
                Presence::Elsewhere
            } else {
                off(cx, move || windows::presence(id, target)).await
            };
            // Back on the display meanwhile: go on waiting.
            if let Some(outcome) = presence.outcome() {
                return Ok(json!({
                    "outcome": outcome,
                    "waitedSeconds": waited(started),
                }));
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let read_root = root.as_ref().map(|r| ax::Sendable(r.get().clone()));
        let read_started = Instant::now();
        let read = cx.background_executor().spawn(async move {
            ax::snapshot(
                target,
                origin,
                scale,
                read_root.map(|r| r.0),
                scope.max_depth,
            )
        });
        // A read still running at the deadline is left to finish alone.
        let timer = cx.background_executor().timer(remaining.max(watch::POLL));
        let snapshot = futures::select! {
            snapshot = FutureExt::fuse(read) => snapshot,
            _ = FutureExt::fuse(timer) => break "timedOut",
            _ = cancel => return Err(cancelled()),
        };
        let read_time = read_started.elapsed();
        match snapshot {
            Ok(snapshot) => {
                count = Some(snapshot.elements.len());
                depth_limited |= snapshot.depth_limited;
                let truncated = snapshot.truncated;
                let complete = scope.root.is_none() && !truncated && !snapshot.depth_limited;
                let tree = core.update(cx, |c, _| {
                    let numbers = c.elements.entry((id, target.window_id)).or_default();
                    numbers::relabel(
                        &snapshot.tree,
                        &numbers.refresh(snapshot.elements, complete),
                    )
                });
                let fired = watch.read(watch::Read { tree, truncated });
                if !fired.is_empty() {
                    let names: Vec<&str> = fired.iter().map(|f| f.name()).collect();
                    let matched: Vec<&String> = fired.iter().flat_map(|f| f.elements()).collect();
                    return Ok(json!({
                        "outcome": "fired",
                        "fired": names,
                        "matched": matched,
                        "changes": watch.changes(),
                        "tree": watch.last().map(|r| &r.tree),
                        "treeTruncated": truncated,
                        "elements": count,
                        "truncated": watch.any_cut(),
                        "depthLimited": depth_limited,
                        "baselineFromAgent": from_agent,
                        "takeover": takeover(core, id, cx),
                        "waitedSeconds": waited(started),
                    }));
                }
            }
            Err(ax::SnapshotError::RootUnavailable) => {
                return Ok(json!({ "outcome": "rootGone", "waitedSeconds": waited(started) }));
            }
            // Busy or not answering: try again at the next read.
            Err(ax::SnapshotError::WindowUnavailable) => unanswered += 1,
        }
        if Instant::now() >= deadline {
            break "timedOut";
        }
        let gap = watch::poll_gap(read_time, watch.quiet())
            .min(deadline.saturating_duration_since(Instant::now()));
        let timer = cx.background_executor().timer(gap);
        futures::select! {
            _ = FutureExt::fuse(timer) => {}
            _ = cancel => return Err(cancelled()),
        }
    };
    Ok(json!({
        "outcome": outcome,
        "changes": watch.changes(),
        "tree": watch.last().map(|r| &r.tree),
        "treeTruncated": watch.last().is_some_and(|r| r.truncated),
        "elements": watch.last().and(count),
        "unanswered": unanswered,
        "truncated": watch.any_cut(),
        "depthLimited": depth_limited,
        "disappearsAbsent": watch.disappears_absent(),
        "baselineFromAgent": from_agent,
        "takeover": takeover(core, id, cx),
        "waitedSeconds": waited(started),
    }))
}

/// Whether the user is operating display `id`.
/// The user took over display `id` while the agent's input waited to go
/// out, so the rest of it never went (#287, ADR 0004). `some` when part of
/// it went out before.
fn stopped_input(id: DisplayId, what: &str, some: bool) -> ControlError {
    let sent = if some {
        "after part of it went out"
    } else {
        "before it went out"
    };
    ControlError::takeover(format!(
        "the user took over display {id} {sent}, so the rest of {what} was not sent. Call wait_for_control to wait until they hand it back."
    ))
}

fn takeover(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> bool {
    core.read_with(cx, |c, _| c.displays.get(id).is_some_and(|d| d.takeover))
}

/// Loosely typed request parameters from JSON.
pub(super) struct Args<'a>(pub(super) &'a Params);

impl Args<'_> {
    pub(super) fn string(&self, key: &str) -> Option<String> {
        self.0.get(key)?.as_str().map(String::from)
    }
    pub(super) fn number(&self, key: &str) -> Option<f64> {
        self.0.get(key)?.as_f64()
    }
    pub(super) fn int(&self, key: &str) -> Option<i64> {
        let value = self.0.get(key)?;
        value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
    }
    pub(super) fn bool(&self, key: &str) -> Option<bool> {
        self.0.get(key)?.as_bool()
    }
    pub(super) fn flag(&self, key: &str) -> bool {
        self.bool(key) == Some(true)
    }
    pub(super) fn require(&self, key: &str) -> Result<f64, ControlError> {
        self.number(key)
            .ok_or_else(|| ControlError::invalid(format!("{key} is required")))
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    fn ago(ms: u64) -> Instant {
        Instant::now() - Duration::from_millis(ms)
    }

    #[test]
    fn subtree_socket_scope_rejects_invalid_numbers() {
        for value in [json!(-1), json!(1.5), json!("2"), json!(true)] {
            for key in ["root", "maxDepth"] {
                let mut p = Params::new();
                p.insert(key.into(), value.clone());
                assert!(read_scope(&Args(&p)).is_err());
            }
        }
        let p = json!({"root": 0, "maxDepth": 2})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            read_scope(&Args(&p)).unwrap(),
            agent_screen_model::tree::Scope {
                root: Some(0),
                max_depth: Some(2)
            }
        );
    }

    #[test]
    fn routes_file_urls() {
        let dir = std::env::temp_dir().join(format!("agent-screen-route-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.html");
        std::fs::write(&file, "x").unwrap();
        let url = format!("file://{}", file.to_string_lossy().replace(' ', "%20"));
        let path = Some(file.to_string_lossy().into_owned());
        // Not a browser: the path, with or without a fragment or query.
        assert_eq!(file_target(Some(&url), false).unwrap(), path);
        assert_eq!(file_target(Some(&format!("{url}#x")), false).unwrap(), path);
        assert_eq!(
            file_target(Some(&format!("{url}?q=1")), false).unwrap(),
            path
        );
        // A browser keeps the URL, and gets one for a path (#168).
        assert_eq!(file_target(Some(&format!("{url}#x")), true).unwrap(), None);
        assert_eq!(
            file_target(path.as_deref(), true).unwrap(),
            Some(agent_screen_model::launch::file_url(
                path.as_deref().unwrap()
            ))
        );
        // An app that isn't a browser opens the path as it is.
        assert_eq!(file_target(path.as_deref(), false).unwrap(), None);
        // Missing files fail for both; other targets pass through.
        assert!(file_target(Some("file:///no/such/file.html"), true).is_err());
        assert_eq!(
            file_target(Some("https://example.com"), false).unwrap(),
            None
        );
        assert_eq!(file_target(None, false).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_file_urls() {
        assert_eq!(
            ax::file_url_path("file:///Users/me/My%20Notes/a%C3%A9.txt"),
            Some(std::path::PathBuf::from("/Users/me/My Notes/aé.txt"))
        );
        assert_eq!(
            ax::file_url_path("file://localhost/tmp/x/"),
            Some(std::path::PathBuf::from("/tmp/x"))
        );
        assert_eq!(ax::file_url_path("https://example.com"), None);
        // A page's query and fragment aren't part of its file.
        assert_eq!(
            ax::file_url_path("file:///tmp/a.html#sec"),
            Some(std::path::PathBuf::from("/tmp/a.html"))
        );
        assert_eq!(
            ax::file_url_path("file:///tmp/a%2Fb.html?q=1"),
            Some(std::path::PathBuf::from("/tmp/a/b.html"))
        );
    }

    #[test]
    fn a_running_app_gets_a_short_turn() {
        // Command-N: 3 s from the key press.
        assert!(!waited_long_enough(true, ago(1000), ago(1500), None));
        assert!(waited_long_enough(true, ago(3500), ago(4000), None));
        // A file: 2 s from when `open` returned...
        assert!(!waited_long_enough(
            false,
            ago(0),
            ago(1000),
            Some(ago(500))
        ));
        assert!(waited_long_enough(
            false,
            ago(0),
            ago(3000),
            Some(ago(2500))
        ));
        // ...and 4 s from the request at most, even while `open` runs.
        assert!(!waited_long_enough(false, ago(0), ago(3000), None));
        assert!(waited_long_enough(false, ago(0), ago(4500), None));
    }
}

#[cfg(test)]
mod frame_request_tests {
    use super::*;

    #[test]
    fn actual_frame_writer_receives_usable_geometry_and_no_default_write() {
        let area = Rect::new(4448.0, -1931.0, 1440.0, 900.0);
        let scale = shot_scale((1440.0, 900.0));
        let full = FrameRequest {
            x: Some(0.0),
            y: Some(0.0),
            width: Some(1356.0),
            height: Some(848.0),
        };
        let mut written = None;
        assert!(
            apply_frame_request(full, area, area, scale, 33.0, |frame| {
                written = Some(frame);
                Ok(())
            })
            .unwrap()
        );
        let frame = written.unwrap();
        assert_eq!(frame.y, -1898.0);
        assert_eq!(frame.height, 867.0);
        assert_eq!(frame.max_y(), area.max_y());
        let mut again = None;
        apply_frame_request(full, frame, area, scale, 33.0, |frame| {
            again = Some(frame);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            again, written,
            "repeating the same explicit placement is idempotent"
        );
        assert!(
            !apply_frame_request(
                FrameRequest::default(),
                area,
                area,
                scale,
                33.0,
                |_| panic!("a default placement must not resize a window")
            )
            .unwrap()
        );
        let ordinary = Rect::new(4500.0, -1800.0, 800.0, 600.0);
        let partial = FrameRequest {
            x: Some(80.0),
            ..Default::default()
        };
        assert!(
            !apply_frame_request(partial, ordinary, area, scale, 33.0, |frame| {
                assert_eq!(frame.x, area.x + 80.0 / scale);
                assert_eq!(frame.y, ordinary.y);
                assert_eq!(frame.width, ordinary.width);
                assert_eq!(frame.height, ordinary.height);
                Ok(())
            })
            .unwrap()
        );
        assert!(
            !apply_frame_request(full, area, area, scale, 0.0, |frame| {
                assert_eq!(frame, full.resolve(area, area, scale));
                Ok(())
            })
            .unwrap()
        );
    }
}

#[cfg(test)]
mod key_window_refusal_tests {
    use crate::mac::input::Refused;

    #[test]
    fn a_refusal_names_the_focused_window_and_what_to_do_instead() {
        let refused = Refused {
            app: "TextEdit".into(),
            window: Some("Notes".into()),
        };
        let message = super::refused_error(&refused, 7).message;
        assert!(
            message.contains(r#"TextEdit's focused window, "Notes", is not on an Agent Display"#)
        );
        assert!(message.contains("refused the keys before sending any"));
        assert!(message.contains("insert_text or set_value"));
        assert!(message.contains("display 7"));
    }

    #[test]
    fn a_refusal_says_when_accessibility_named_no_window() {
        let refused = Refused {
            app: "TextEdit".into(),
            window: None,
        };
        let message = super::refused_error(&refused, 7).message;
        assert!(message.contains("Accessibility could not say which window of TextEdit has focus"));
    }
}

#[cfg(test)]
mod handle_one_arm_tests {
    use super::{Arm, ControlError, finish_arm, reply};
    use serde_json::json;

    #[test]
    fn finish_arm_refreshes_only_for_tail_outcomes() {
        let mut refreshed = 0;
        let ok = finish_arm(Ok(Arm::Finish(Ok(json!({ "windowId": 1 })))), || {
            refreshed += 1;
        })
        .unwrap();
        assert_eq!(refreshed, 1);
        assert_eq!(ok.get("windowId"), Some(&json!(1)));

        let err = finish_arm(
            Ok(Arm::Finish(Err(ControlError::not_found("none")))),
            || {
                refreshed += 1;
            },
        );
        assert_eq!(refreshed, 2);
        assert!(err.is_err());

        let early_ok = finish_arm(Ok(Arm::Early(reply(json!({ "route": "queued" })))), || {
            refreshed += 1;
        })
        .unwrap();
        assert_eq!(refreshed, 2);
        assert_eq!(early_ok.get("route"), Some(&json!("queued")));

        let early_err = finish_arm(Ok(Arm::Early(Err(ControlError::takeover("user")))), || {
            refreshed += 1;
        });
        assert_eq!(refreshed, 2);
        assert!(early_err.is_err());

        let propagated_err = finish_arm(Err(ControlError::invalid("bad")), || {
            refreshed += 1;
        });
        assert_eq!(refreshed, 2);
        assert!(propagated_err.is_err());
    }

    #[test]
    fn change_count_hold_keeps_safety_timer_waiting_across_threads_until_released() {
        use super::{SafetyTimerStep, count_holds, hold_change_counting, safety_timer_step};
        use std::sync::mpsc;

        let id = 4242;
        assert_eq!(count_holds(id), 0);
        assert_eq!(
            safety_timer_step(true, count_holds(id)),
            SafetyTimerStep::StopCounting
        );

        // Take a hold on a background thread and verify visibility on the test
        // thread, then move the hold across threads and drop it on the test
        // thread so cross-thread create/drop never drifts.
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let bg_hold = hold_change_counting(id);
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            bg_hold
        });
        ready_rx.recv().unwrap();

        assert_eq!(count_holds(id), 1);
        assert_eq!(
            safety_timer_step(true, count_holds(id)),
            SafetyTimerStep::KeepWaiting
        );
        assert_eq!(
            safety_timer_step(false, count_holds(id)),
            SafetyTimerStep::Superseded
        );

        // Overlapping hold on this thread keeps count at 2 until both drop.
        let settle_hold = hold_change_counting(id);
        assert_eq!(count_holds(id), 2);
        drop(settle_hold);
        assert_eq!(count_holds(id), 1);
        assert_eq!(
            safety_timer_step(true, count_holds(id)),
            SafetyTimerStep::KeepWaiting
        );

        release_tx.send(()).unwrap();
        let moved_hold = worker.join().unwrap();
        assert_eq!(count_holds(id), 1);
        drop(moved_hold);
        assert_eq!(count_holds(id), 0);
        assert_eq!(
            safety_timer_step(true, count_holds(id)),
            SafetyTimerStep::StopCounting
        );
    }
}

#[cfg(test)]
mod permission_tests {
    #[test]
    fn reset_permissions_works_without_the_permissions_it_resets() {
        assert!(!super::needs_permissions("resetPermissions"));
        assert!(super::needs_permissions("createDisplay"));
    }
}
