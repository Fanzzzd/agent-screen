//! Serves control requests from Agent Sessions. Coordinates are in points
//! relative to the display's top-left corner, the same space as screenshots.

use super::server::{ControlError, Reply};
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
use agent_screen_model::input_queue::Action;
use base64::Engine;
use futures::FutureExt;
use gpui::{AsyncApp, Entity};
use objc2::MainThreadMarker;
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

type Params = Map<String, Value>;

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
    if matches!(method, "click" | "mouseDown" | "key" | "type")
        && let Some(id) = shown
    {
        menu_still_open(id, method, cx).await?;
    }
    let reply = handle_one(core.clone(), connection, method, p, cx).await;
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
        let menus = off(cx, move || windows::menus_on(id)).await;
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
}

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
    let open = off(cx, move || windows::menus_on(id)).await;
    if seen.iter().any(|w| open.contains(w)) {
        return Ok(());
    }
    MENUS.with(|m| m.borrow_mut().insert(id, open));
    let action = match method {
        "type" => "typing",
        "key" => "key",
        _ => "click",
    };
    Err(ControlError::failed(format!(
        "the menu open on display {id} closed before this {action}, with no choice made, so nothing was sent. macOS closes an app's open menu at the user's next click or key anywhere, on their own screen too. Take a screenshot, open the menu again and choose its item in the next call, with act_on_element press on the item or a click on it; or focus the closed control and type the option's text"
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
        usable(&core, connection, id as DisplayId, method, cx)?;
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
    if let Some(id) = acting {
        crate::mac::catch::agent_acted(id);
        if method != "launchApp" {
            watch_for_changes(&core, id, cx);
        }
    }
    let result: Result<Value, ControlError> = match method {
        "createDisplay" => create_display(&core, connection, &p, cx).await,
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
            Ok(json!({}))
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
            if p.0.get("settle") == Some(&Value::Bool(true)) {
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
                .spawn(async move { screenshot::display(id, pixels, format, region) })
                .await
                .map_err(ControlError::failed)?;
            let mut result = shot_json(&shot);
            result["scale"] = json!(scale);
            add_strayed(&core, id, &mut result, cx);
            Ok(result)
        }
        "click" => {
            let (id, point) = target(&core, &p, cx).await?;
            outside_menu_bar(id, point)?;
            let button = Button::parse(p.string("button").as_deref().unwrap_or("left"))
                .ok_or_else(|| {
                    ControlError::invalid(format!(
                        "unknown button {}",
                        p.string("button").unwrap_or_default()
                    ))
                })?;
            let modifiers = modifiers(&p)?;
            let forwarder = show_pointer(&core, id, point, true, cx);
            let hit = forwarder
                .click(point, button, p.int("count").unwrap_or(1), modifiers)
                .await;
            match hit {
                Some(window) => Ok(json!({ "windowId": window.window_id })),
                None => Err(ControlError::not_found(format!(
                    "nothing at ({}, {}): no window there to click",
                    p.number("x").unwrap_or(0.0) as i64,
                    p.number("y").unwrap_or(0.0) as i64
                ))),
            }
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
            show_pointer(&core, id, point, false, cx)
                .move_to(point)
                .await;
            Ok(json!({}))
        }
        "drag" => {
            let id = input_display(&core, &p, cx).await?;
            let from = global(&core, id, p.require("fromX")?, p.require("fromY")?, cx)?;
            let to = global(&core, id, p.require("toX")?, p.require("toY")?, cx)?;
            outside_menu_bar(id, from)?;
            let modifiers = modifiers(&p)?;
            if p.0.get("realInput") == Some(&Value::Bool(true)) {
                return queued_drag(
                    &core,
                    connection,
                    id,
                    (from, Some(to)),
                    modifiers,
                    "Requested real-input retry",
                    cx,
                )
                .await;
            }
            // A drag on a title bar moves the window, which posted events
            // can't do.
            if let Some(grab) = title_bar_at(id, from, cx).await {
                show_pointer(&core, id, to, false, cx);
                if !drag_to(grab, to, cx).await {
                    return Err(ControlError::failed(
                        "the app would not let its window move",
                    ));
                }
                return reply(json!({ "movedWindow": true, "route": "placement" }));
            }
            // A web page takes a drag through the app's DevTools pipe, or
            // not at all (#37, ADR 0010).
            let refused = match web_route(from, cx).await {
                Route::Pipe(page) => {
                    return pipe_drag(&core, id, page, from, to, modifiers, cx).await;
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
                        !guard.agent_stopped()
                            && windows::on_display(id).iter().any(|w| w.target == target)
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
                    show_pointer(&core, id, to, false, cx);
                    return reply(json!({ "route": "accessibility" }));
                }
            }
            if let Some(app) = refused {
                return queued_drag(
                    &core,
                    connection,
                    id,
                    (from, Some(to)),
                    modifiers,
                    &devtools::refused_message(&app),
                    cx,
                )
                .await;
            }
            let forwarder = show_pointer(&core, id, from, true, cx);
            let dragged = forwarder.drag(from, to, modifiers).await;
            let released = match dragged {
                Sent::Stopped(at) => at,
                _ => to,
            };
            show_pointer(&core, id, released, false, cx);
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
            Ok(json!({ "route": "posted" }))
        }
        "mouseDown" | "mouseUp" => {
            let down = method == "mouseDown";
            let id = input_display(&core, &p, cx).await?;
            // Without a point the button acts where the Agent Pointer is.
            let point = match (p.number("x"), p.number("y")) {
                (Some(x), Some(y)) => global(&core, id, x, y, cx)?,
                _ => {
                    let bounds = cg::display_bounds(id);
                    core.read_with(cx, |c, _| {
                        c.displays
                            .get(id)
                            .and_then(|d| d.pointer)
                            .map(|p| (p.x, p.y))
                    })
                    .map(|(x, y)| (bounds.x + x, bounds.y + y))
                    .ok_or_else(|| {
                        ControlError::invalid("no pointer position yet; give a coordinate")
                    })?
                }
            };
            if down {
                outside_menu_bar(id, point)?;
            }
            let grabbed = GRAB.with(|g| g.borrow_mut().remove(&id));
            if down && let Some(grab) = title_bar_at(id, point, cx).await {
                show_pointer(&core, id, point, true, cx);
                let window = grab.target.window_id;
                GRAB.with(|g| g.borrow_mut().insert(id, grab));
                return reply(json!({ "windowId": window }));
            }
            if !down && let Some(grab) = grabbed {
                show_pointer(&core, id, point, false, cx);
                drag_to(grab, point, cx).await;
                return reply(json!({ "windowId": grab.target.window_id }));
            }
            let modifiers = modifiers(&p)?;
            // A press in a web page goes through the app's DevTools pipe,
            // and so do the moves and the release after it (#37, ADR 0010).
            let held = HOLDS.with(|h| h.borrow_mut().remove(&id));
            if let Some(Hold::Pipe(drag)) = held {
                if down {
                    off(cx, move || drag.cancel()).await;
                } else {
                    show_pointer(&core, id, point, false, cx);
                    let window = drag.window();
                    off(cx, move || drag.release(point)).await.map_err(|e| {
                        ControlError::failed(format!("the release didn't reach the page: {e}"))
                    })?;
                    return reply(json!({ "windowId": window }));
                }
            }
            if down && p.0.get("realInput") == Some(&Value::Bool(true)) {
                return queued_drag(
                    &core,
                    connection,
                    id,
                    (point, None),
                    modifiers,
                    "Requested real-input press",
                    cx,
                )
                .await;
            }
            if down {
                match web_route(point, cx).await {
                    Route::Pipe(page) => {
                        if forwarder_stopped(&core, id, cx) {
                            return Err(ControlError::takeover(format!(
                                "the user took over display {id}, so the press was not sent. Call wait_for_control to wait until they hand it back."
                            )));
                        }
                        // Keys go to the window pressed, as after a click.
                        let window = page.window();
                        show_pointer(&core, id, point, true, cx).focus(window).await;
                        let window = window.window_id;
                        let bits = modifier_bits(modifiers);
                        let drag = off(cx, move || devtools::Drag::press(page, point, bits))
                            .await
                            .map_err(|e| {
                                ControlError::failed(format!(
                                    "the press didn't reach the page: {e}"
                                ))
                            })?;
                        HOLDS.with(|h| h.borrow_mut().insert(id, Hold::Pipe(drag)));
                        return reply(json!({ "windowId": window }));
                    }
                    Route::Unmapped(why) => return Err(unmapped(why)),
                    Route::Refused(app) => {
                        return queued_drag(
                            &core,
                            connection,
                            id,
                            (point, None),
                            modifiers,
                            &devtools::refused_message(&app),
                            cx,
                        )
                        .await;
                    }
                    Route::Posted => {}
                }
            }
            let hit = show_pointer(&core, id, point, down, cx)
                .button(point, down, modifiers)
                .await;
            match hit {
                Some(window) => Ok(json!({ "windowId": window.window_id })),
                None if down && forwarder_stopped(&core, id, cx) => {
                    Err(ControlError::takeover(format!(
                        "the user took over display {id}, so the press was not sent. Call wait_for_control to wait until they hand it back."
                    )))
                }
                None if down => Err(ControlError::not_found(
                    "nothing under the pointer to press on",
                )),
                None => Err(ControlError::invalid("the left button is not down")),
            }
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
            let id = input_display(&core, &p, cx).await?;
            let text = p
                .string("text")
                .ok_or_else(|| ControlError::invalid("text is required"))?;
            if p.0.get("viaInputMethod") == Some(&Value::Bool(true)) {
                let keys = crate::keychord::keys_for_text(&text).map_err(|c| {
                    ControlError::invalid(format!(
                        "{c:?} has no key on a US keyboard. Through the input method, type the keys it composes from, such as pinyin letters."
                    ))
                })?;
                return through_input_method(&core, id, keys, p.string("inputSource"), cx).await;
            }
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            let bounds = cg::display_bounds(id);
            let chars: Vec<char> = crate::mac::input::typed_text(&text).chars().collect();
            let total = chars.len();
            let typed = async {
                let mut done = 0;
                // In runs of 20, about 100 ms each, so the user switching to
                // the app mid-way stops the rest.
                for run in chars.chunks(20) {
                    if let Err(e) = keys_allowed(&forwarder, bounds, cx).await {
                        return Err(match done {
                            0 => e,
                            _ => ControlError::failed(format!(
                                "typed {done} of {total} characters, then stopped: {} If you retry, type only the rest, or replace the whole field with set_value.",
                                e.message
                            )),
                        });
                    }
                    match forwarder.type_text(run.iter().collect(), bounds).await {
                        Sent::Nowhere => return Err(no_keyboard_target()),
                        Sent::Dropped(shown) => {
                            let typed = done + run.len();
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
                        _ => done += run.len(),
                    }
                }
                Ok(json!({}))
            }
            .await;
            forwarder.keys_done().await;
            typed
        }
        "key" => {
            let id = input_display(&core, &p, cx).await?;
            let keys = p
                .string("keys")
                .ok_or_else(|| ControlError::invalid("keys is required"))?;
            let chords = KeyChord::parse_sequence(&keys)
                .map_err(|e| ControlError::invalid(e.to_string()))?;
            if p.0.get("viaInputMethod") == Some(&Value::Bool(true)) {
                return through_input_method(&core, id, chords, p.string("inputSource"), cx).await;
            }
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            let repeat = p.int("repeat").unwrap_or(1);
            let bounds = cg::display_bounds(id);
            let total = chords.len();
            let pressed = async {
                for (pressed, chord) in chords.into_iter().enumerate() {
                    if forwarder.agent_stopped() {
                        return Err(ControlError::takeover(format!(
                            "the user took over display {id} after {pressed} of {total} keys. Call wait_for_control, then take a screenshot before you press the rest."
                        )));
                    }
                    if let Err(e) = keys_allowed(&forwarder, bounds, cx).await {
                        return Err(match pressed {
                            0 => e,
                            _ => ControlError::failed(format!(
                                "pressed {pressed} of {total} keys, then stopped: {} If you retry, press only the rest.",
                                e.message
                            )),
                        });
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
                    if !forwarder.press(chord, repeat, bounds).await {
                        return Err(no_keyboard_target());
                    }
                }
                Ok(json!({}))
            }
            .await;
            forwarder.keys_done().await;
            pressed
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
                forwarder.keys_done().await;
                return Err(no_keyboard_target());
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
        "listWindows" if p.0.get("strays").and_then(Value::as_bool) == Some(true) => {
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
            let (records, started) = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .map(|d| (d.records.clone(), d.started.clone()))
                    .unwrap_or_default()
            });
            let scale = display_scale(&core, id, cx);
            let list: Vec<Value> = off(cx, move || windows::on_display(id))
                .await
                .iter()
                .map(|w| {
                    let mut json = in_shot(w.json(), scale);
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
                .collect();
            Ok(json!({ "windows": list, "scale": scale }))
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
                ControlError::not_found(format!("no window with that id on display {id}"))
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
        "placeWindow" => {
            let id = input_display(&core, &p, cx).await?;
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
                    return frame_window(&core, id, window.target, asked, cx).await;
                }
            }
            let matches = |w: &windows::Window| match (wanted, &app) {
                (Some(wanted), _) => w.target.window_id as i64 == wanted,
                (None, Some(app)) => w.app.eq_ignore_ascii_case(app),
                (None, None) => false,
            };
            // A window of an app the agent started is its own to place.
            let stray = strays(&core, id, cx).await.into_iter().find(|w| matches(w));
            let Some(window) = stray else {
                // Any other window is the user's until they hand it over.
                // The user's windows that match, front to back, with their
                // apps' bundle ids, which Window Access goes by.
                let candidates: Vec<(windows::Window, Option<String>)> = off(cx, {
                    let app = app.clone();
                    move || {
                        windows::all()
                            .into_iter()
                            .filter(|w| {
                                w.layer == 0
                                    && match (wanted, &app) {
                                        (Some(wanted), _) => w.target.window_id as i64 == wanted,
                                        (None, Some(app)) => w.app.eq_ignore_ascii_case(app),
                                        (None, None) => false,
                                    }
                                    && windows::display_role(w).accepts_window_request()
                            })
                            .map(|w| {
                                let bundle = app::bundle_id(w.target.pid);
                                (w, bundle)
                            })
                            .collect()
                    }
                })
                .await;
                if candidates.is_empty() {
                    return Err(ControlError::not_found(match &app {
                        Some(app) => format!("no {app} window on the user's screen"),
                        None => "no window with that id on the user's screen".into(),
                    }));
                }
                let words = p
                    .string("userInstruction")
                    .map(|w| access::user_instruction(&w))
                    .transpose()
                    .map_err(ControlError::invalid)?;
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
                let allowed =
                    core.read_with(cx, |c, _| c.displays.may_hand_over(id, target, &authority));
                let mut reply = match allowed {
                    Ok(()) => hand_over(&core, id, theirs, authority, words, cx).await?,
                    // Not covered by the grant: the user answers on a card.
                    Err(Refusal::NotGranted | Refusal::Excluded) if words.is_none() => {
                        request_window(&core, id, theirs, p.number("timeoutSeconds"), cx).await?
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
                reply.extend(frame_window(&core, id, target, asked, cx).await?);
                return Ok(reply);
            };
            // A window this display recorded before keeps that record if
            // it can't be placed: it stays a stray either way.
            let before = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .and_then(|d| d.records.get(&window.target.window_id).cloned())
            });
            claim(&core, id, window.target, Origin::Placed, cx);
            // AeroSpace can lay the window out again in the user's
            // workspace right after a display appears (`windows::kept_on`),
            // and an app can put it back on its own screen: watch it for
            // 1.5 s and move it back each time. The answer waits until it
            // has stayed 0.5 s since the last move (#47).
            let mut outcome = place(window.target, id, cx).await;
            let mut moves = 1;
            let mut checks = 0;
            let mut stayed = 0;
            while outcome.is_ok() && !(checks >= 6 && stayed >= 2) {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                checks += 1;
                if held(window.target, id, cx).await {
                    stayed += 1;
                    continue;
                }
                if moves == 4 {
                    outcome = Err(
                        "it kept going back to the user's screen, moved there by its app or by AeroSpace"
                            .into(),
                    );
                    break;
                }
                log::info!(
                    "window {} left display {id} after place_window; placing again",
                    window.target.window_id
                );
                moves += 1;
                stayed = 0;
                outcome = place(window.target, id, cx).await;
            }
            if let Err(e) = outcome {
                // Still a stray, so the agent can try again.
                core.update(cx, |c, _| {
                    if let Some(d) = c.displays.get_mut(id) {
                        match before {
                            Some(record) => {
                                d.records.insert(window.target.window_id, record);
                            }
                            None => d.forget(window.target.window_id),
                        }
                    }
                });
                return Err(ControlError::failed(format!(
                    "could not move the window: {e}"
                )));
            }
            frame_window(&core, id, window.target, asked, cx)
                .await
                .map(Value::Object)
        }
        "windowState" => {
            let id = display(&core, &p, false, cx)?;
            let window = window_on(id, &p, cx).await?;
            let target = window.target;
            let area = cg::display_bounds(id);
            let origin = (area.x, area.y);
            let scale = display_scale(&core, id, cx);
            // Walking a tree takes a round trip per element.
            let started = Instant::now();
            let snapshot = cx
                .background_executor()
                .spawn(async move { ax::snapshot(target, origin, scale) })
                .await
                // The window is there, so this isn't `not_found`, which
                // reads as closed: the app is busy, or AX is off.
                .ok_or_else(|| {
                    ControlError::failed(
                        "the app didn't answer accessibility in time, or Accessibility isn't granted; call get_window_state again",
                    )
                })?;
            let count = snapshot.elements.len();
            let walked = started.elapsed();
            let tree = core.update(cx, |c, _| {
                let numbers = c.elements.entry((id, window.target.window_id)).or_default();
                numbers::relabel(&snapshot.tree, &numbers.renumber(snapshot.elements))
            });
            let size = (window.bounds.width, window.bounds.height);
            let shot = cx
                .background_executor()
                .spawn(async move { screenshot::window(target, size) })
                .await
                .map_err(ControlError::failed)?;
            log::debug!(
                "windowState {}: {count} elements in {} ms, picture by {} ms",
                target.window_id,
                walked.as_millis(),
                started.elapsed().as_millis()
            );
            let mut result = shot_json(&shot);
            result["window"] = in_shot(window.json(), scale);
            result["tree"] = json!(tree);
            result["elements"] = json!(count);
            result["truncated"] = json!(snapshot.truncated);
            add_strayed(&core, id, &mut result, cx);
            Ok(result)
        }
        "elementAction" => {
            let id = input_display(&core, &p, cx).await?;
            let window = window_on(id, &p, cx).await?;
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
                    return reply(json!({ "menu": tree }));
                }
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
                return reply(
                    json!({ "text": value, "selected": selected.filter(|s| !s.is_empty()) }),
                );
            }
            if matches!(action.as_str(), "focus" | "set_value" | "press") {
                core.update(cx, |c, _| c.forwarder(id))
                    .remember_user_window(window.target.pid)
                    .await;
            }
            act(&core, id, &action, &element, p.string("value"), cx).await?;
            // Typing after focusing a field, or pressing in a window, goes
            // to that window, as it would after a click there.
            if matches!(action.as_str(), "focus" | "set_value" | "press") {
                let forwarder = core.update(cx, |c, _| c.forwarder(id));
                forwarder.focus(window.target).await;
            }
            // Electron takes a menu item pressed through accessibility for
            // its shortcut: AppKit runs the press with no current event,
            // and Electron reads only a mouse event as a click (#39).
            if action == "press" {
                let (item, pid) = (ax::Sendable(element.clone()), window.target.pid);
                let note = off(cx, move || {
                    let menu =
                        ax::string_attribute(item.get(), "AXRole").as_deref() == Some("AXMenuItem");
                    if !menu || !app::is_electron(pid) {
                        None
                    } else if app::is_vs_code_based(pid) {
                        Some(VS_CODE_MENU_NOTE)
                    } else {
                        Some(ELECTRON_MENU_NOTE)
                    }
                })
                .await;
                if let Some(note) = note {
                    return reply(json!({ "note": note }));
                }
            }
            Ok(json!({}))
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
        // The MCP server of an agent session listed the tools.
        "agentReady" => {
            let client = p.string("client").unwrap_or_else(|| "An agent".into());
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
            core.update(cx, |c, cx| {
                if c.displays.request_help(id, reason, steps) {
                    cx.notify();
                }
            });
            Ok(json!({}))
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
            let outcome = match wait {
                Wait::Now(outcome) => outcome,
                Wait::Later(rx) => {
                    let timer = cx
                        .background_executor()
                        .timer(Duration::from_secs_f64(seconds));
                    futures::select! {
                        outcome = rx.fuse() => outcome.unwrap_or(crate::model::ControlWait::Ended),
                        _ = FutureExt::fuse(timer) => crate::model::ControlWait::TimedOut,
                    }
                }
            };
            // Ended because the user closed it: the agent should stop.
            let closed = outcome == crate::model::ControlWait::Ended
                && core.read_with(cx, |c, _| {
                    c.displays.closed_by_user.contains(&id)
                        || c.displays
                            .get(id)
                            .is_some_and(|d| d.ended == Some(EndReason::ClosedByUser))
                });
            // Timed out with the Help Request unanswered: the user may not
            // have come at all.
            let help_open = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .is_some_and(|d| d.help_request.is_some() && !d.takeover)
            });
            let outcome = if closed {
                "closedByUser"
            } else {
                outcome.as_str()
            };
            let control_asked = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .is_some_and(|d| d.control_request.is_some())
            });
            Ok(json!({ "outcome": outcome, "helpOpen": help_open, "controlAsked": control_asked }))
        }
        // Test hook: stands in for the user's input in the Viewer.
        "debugTakeover" if debug_methods() => {
            let id = display(&core, &p, false, cx)?;
            let active = p.0.get("active") == Some(&Value::Bool(true));
            core.update(cx, |c, cx| {
                if active {
                    c.take_over(id, cx)
                } else {
                    c.hand_back(id, cx)
                }
            });
            Ok(json!({}))
        }
        // Move any window, by pid and id, to a global point: a test puts a
        // system alert on an agent display with it.
        "debugMoveWindow" if debug_methods() => {
            let target = WindowTarget {
                pid: p.int("pid").unwrap_or(0) as i32,
                window_id: p.int("windowId").unwrap_or(0) as u32,
            };
            let (x, y) = (p.number("x").unwrap_or(0.0), p.number("y").unwrap_or(0.0));
            let moved = cx
                .background_executor()
                .spawn(async move {
                    ax::window_element(target).is_some_and(|w| ax::set_position(&w, x, y))
                })
                .await;
            Ok(json!({ "moved": moved }))
        }
        "debugViewer" if debug_methods() => {
            let (frame, image, key) = crate::mac::viewer_input::debug_layout()
                .ok_or_else(|| ControlError::not_found("the Viewer shows no display"))?;
            // Whether the shown display's stream showed another display.
            let misrouted = cx.update(|cx| {
                crate::ui::shell::current()
                    .and_then(|shell| shell.upgrade())
                    .and_then(|shell| shell.read(cx).viewer_display())
                    .zip(crate::ui::stream::global())
                    .is_some_and(|(id, streams)| streams.read(cx).misrouted(id))
            });
            // The header's Take Back button, in points from the Viewer's
            // top-left corner, as last painted.
            let take_back = crate::ui::viewer::WindowMenu::TakeBack.button().map(|b| {
                [
                    f64::from(b.origin.x),
                    f64::from(b.origin.y),
                    f64::from(b.size.width),
                    f64::from(b.size.height),
                ]
            });
            Ok(json!({
                "frame": frame,
                "image": image,
                "key": key,
                "misrouted": misrouted,
                "takeBack": take_back,
            }))
        }
        "debugViewerInput" if debug_methods() => {
            use crate::mac::viewer_input;
            // A frame from the display's stream leaves the Viewer to draw,
            // as it nearly always is while the user works in it.
            if p.int("key") == Some(1) {
                cx.update(|cx| cx.refresh_windows());
            }
            match p.string("kind").as_deref() {
                Some("click") => viewer_input::inject_click(
                    p.number("x").unwrap_or(0.5),
                    p.number("y").unwrap_or(0.5),
                    p.int("key").unwrap_or(0) == 1,
                ),
                Some("key") => viewer_input::inject_key(
                    p.int("code").unwrap_or(0) as u16,
                    p.int("shift").unwrap_or(0) == 1,
                    p.int("command").unwrap_or(0) == 1,
                ),
                // A press, a drag in steps and a release, as a hand on
                // the trackpad makes them.
                Some("drag") => {
                    use objc2_app_kit::NSEventType;
                    let from = (p.number("x").unwrap_or(0.5), p.number("y").unwrap_or(0.5));
                    let to = (
                        p.number("toX").unwrap_or(0.5),
                        p.number("toY").unwrap_or(0.5),
                    );
                    viewer_input::inject_mouse(NSEventType::LeftMouseDown, from.0, from.1)
                        .map_err(ControlError::failed)?;
                    for step in 1..=20 {
                        cx.background_executor()
                            .timer(Duration::from_millis(16))
                            .await;
                        let t = step as f64 / 20.0;
                        viewer_input::inject_mouse(
                            NSEventType::LeftMouseDragged,
                            from.0 + (to.0 - from.0) * t,
                            from.1 + (to.1 - from.1) * t,
                        )
                        .map_err(ControlError::failed)?;
                    }
                    viewer_input::inject_mouse(NSEventType::LeftMouseUp, to.0, to.1)
                }
                _ => Err("kind is click, key or drag".into()),
            }
            .map_err(ControlError::failed)?;
            Ok(json!({}))
        }
        // Test hook: the user hands a window over from the Windows menu.
        "debugHandOver" if debug_methods() => {
            let id = display(&core, &p, false, cx)?;
            let window = p
                .int("windowId")
                .and_then(|w| windows::find(w as u32))
                .ok_or_else(|| ControlError::not_found("no window with that id"))?;
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            shell
                .update(cx, |shell, cx| {
                    shell.hand_over(window.target, Some(window.bounds), id, cx)
                })
                .map_err(|e| ControlError::failed(e.to_string()))?;
            Ok(json!({}))
        }
        // Test hook: the user answers the display's Window Request, as the
        // card's buttons do, once one is open.
        "debugAnswerWindow" if debug_methods() => {
            let id = display(&core, &p, false, cx)?;
            let give = p.0.get("give") == Some(&Value::Bool(true));
            for _ in 0..50 {
                let open = core.read_with(cx, |c, _| {
                    c.displays
                        .get(id)
                        .is_some_and(|d| d.window_request.is_some())
                });
                if open {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let answered = shell
                .update(cx, |shell, cx| shell.answer_window(id, give, cx))
                .map_err(|e| ControlError::failed(e.to_string()))?;
            Ok(json!({ "answered": answered }))
        }
        // Test hook: the user takes a window back from the Windows menu.
        "debugTakeBack" if debug_methods() => {
            let id = display(&core, &p, false, cx)?;
            let target = p
                .int("windowId")
                .and_then(|w| windows::find(w as u32))
                .ok_or_else(|| ControlError::not_found("no window with that id"))?
                .target;
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            shell
                .update(cx, |shell, cx| shell.take_back(target, id, cx))
                .map_err(|e| ControlError::failed(e.to_string()))?
                .await;
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            Ok(json!({}))
        }
        // Test hook: the session that holds the user stand-in stage gives
        // up a window it launched there, so the window counts as the user's
        // own, as on a real screen of theirs. The claim stays, so AeroSpace
        // leaves the window on the stage.
        #[cfg(debug_assertions)]
        "debugUserWindow" if debug_methods() => {
            let id = p.int("displayId").unwrap_or(0) as DisplayId;
            usable(&core, connection, id, "debugUserWindow", cx)?;
            if !cg::window_display_role(id).accepts_window_request() {
                return Err(ControlError::invalid(
                    "debugUserWindow needs this session's user stand-in stage (debugShow userStage)",
                ));
            }
            let window = p
                .int("windowId")
                .ok_or_else(|| ControlError::invalid("windowId is required"))?
                as u32;
            let here = off(cx, move || {
                windows::on_display(id)
                    .iter()
                    .any(|w| w.target.window_id == window)
            })
            .await;
            if !here {
                return Err(ControlError::not_found(
                    "no window with that id on the stage",
                ));
            }
            // Only a window the claim file holds on the stage: AeroSpace
            // would take any other for the user's workspace.
            let claimed = off(cx, move || crate::mac::claims::claim_confirmed(window, id)).await;
            if !claimed {
                return Err(ControlError::failed(
                    "the window's claim for the stage wasn't published",
                ));
            }
            core.update(cx, |c, _| {
                if let Some(d) = c.displays.get_mut(id) {
                    d.forget(window);
                    d.user_made.insert(window);
                }
            });
            Ok(json!({}))
        }
        // Test hook: which agent the setup window says loaded the tools.
        "debugAgentReady" if debug_methods() => {
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let agent = shell
                .read_with(cx, |shell, _| shell.last_agent_ready())
                .map_err(|e| ControlError::failed(e.to_string()))?;
            Ok(json!({ "agent": agent }))
        }
        // Test hook: macOS last named `pid` the front app, as when the user
        // had just used it, without making it active.
        "debugFront" if debug_methods() => {
            crate::mac::input::note_activation(p.int("pid").map(|pid| pid as i32));
            Ok(json!({}))
        }
        // Test hook: the user brought `pid` to the front, as from the Dock:
        // the displays the reminder of where its windows are named.
        "debugActivated" if debug_methods() => {
            let pid = p.int("pid").unwrap_or(0) as i32;
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let task = shell
                .update(cx, |shell, cx| shell.remind_where(pid, false, cx))
                .map_err(|e| ControlError::failed(e.to_string()))?;
            let reminded = task.await;
            let peek = shell
                .update(cx, |shell, cx| shell.reminder(cx))
                .ok()
                .flatten();
            Ok(json!({ "reminded": reminded, "peek": peek }))
        }
        // Test hooks for updates: check the feed now without a window, and
        // what is waiting to be installed.
        "debugCheckForUpdates" if debug_methods() => {
            crate::mac::update::check_in_background();
            Ok(json!({ "available": crate::mac::update::available() }))
        }
        "debugUpdateState" if debug_methods() => Ok(json!({
            "available": crate::mac::update::available(),
            "pending": crate::mac::update::pending(),
        })),
        // Test hook: the user's left click at a global point, as the global
        // monitor reports it, such as on an app's Dock icon.
        "debugUserClick" if debug_methods() => {
            let at = (p.require("x")?, p.require("y")?);
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let task = shell
                .update(cx, |shell, cx| shell.user_clicked(at, cx))
                .map_err(|e| ControlError::failed(e.to_string()))?;
            let reminded = task.await;
            let peek = shell
                .update(cx, |shell, cx| shell.reminder(cx))
                .ok()
                .flatten();
            Ok(json!({ "reminded": reminded, "peek": peek }))
        }
        // Test hook: a key press of the user's that went to `pid`, as the
        // global key monitor reports one typed into Spotlight.
        "debugUserKey" if debug_methods() => {
            let pid = p.int("pid").unwrap_or(0) as i32;
            cx.background_executor()
                .spawn(async move { crate::mac::catch::user_key_to(pid) })
                .await;
            Ok(json!({}))
        }
        // A panic on the main thread after the reply, as one in an AppKit
        // callback: the app aborts.
        "debugPanic" if debug_methods() => {
            cx.spawn(async |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                panic!("debugPanic");
            })
            .detach();
            Ok(json!({}))
        }
        "debugQuit" if debug_methods() => {
            // Quits the way the menu does, after this reply goes out.
            cx.spawn(async |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
            Ok(json!({}))
        }
        // The Help Card's width and its buttons' bounds at the last paint.
        "debugCard" if debug_methods() => {
            let (width, buttons) = crate::ui::help_card::painted_buttons();
            let buttons: Map<String, Value> = buttons
                .into_iter()
                .map(|(id, r)| (id.to_string(), json!(r)))
                .collect();
            Ok(json!({ "width": width, "buttons": buttons }))
        }
        // Which picture the menu bar icon shows: idle, working, asking or setup.
        "debugIcon" if debug_methods() => {
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            let state = shell
                .update(cx, |shell, _| shell.icon_state())
                .map_err(|e| ControlError::failed(e.to_string()))?;
            Ok(json!({ "state": state }))
        }
        "debugShow" if debug_methods() => {
            let surface = p.string("surface").unwrap_or_else(|| "viewer".into());
            let id = p.int("displayId").unwrap_or(0) as DisplayId;
            #[cfg(debug_assertions)]
            if surface == "userStage" {
                usable(&core, connection, id, "debugShow", cx)?;
                let shell =
                    crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
                let selected = shell
                    .update(cx, |shell, _| shell.stage_display() == Some(id))
                    .unwrap_or(false);
                let held = core.read_with(cx, |c, _| c.displays.get(id).is_some());
                if !held || !selected || !cg::is_agent_screen(id) || !cg::is_online(id) {
                    return Err(ControlError::invalid(
                        "userStage requires this session's selected live stage display",
                    ));
                }
                cg::debug_user_stage(id, connection);
                return reply(json!({}));
            }
            if matches!(
                surface.as_str(),
                "input-queue"
                    | "input-queue-approved"
                    | "input-queue-allow"
                    | "input-queue-decline"
            ) {
                if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") {
                    return Err(ControlError::invalid(
                        "input queue previews require the isolated e2e instance",
                    ));
                }
                usable(&core, connection, id, "debugShow", cx)?;
            }
            // Allow All itself, through the shell, comes after this.
            if matches!(surface.as_str(), "input-queue" | "input-queue-approved") {
                core.update(cx, |c, cx| -> Result<(), ControlError> {
                    let owner = c
                        .input_queue
                        .owner(id, connection)
                        .map_err(ControlError::invalid)?;
                    c.input_queue
                        .submit(
                            owner,
                            format!("preview:{id}"),
                            "Preview a queued drag on this display".into(),
                            None,
                        )
                        .map_err(ControlError::invalid)?;
                    cx.notify();
                    Ok(())
                })?;
            }
            // A Window Request for no real window, so scripts can look at
            // the card with any `app` and `title`. Keep answers it.
            if surface == "window-request" {
                let app = p.string("app").unwrap_or_else(|| "TextEdit".into());
                let title = p.string("title").unwrap_or_default();
                let target = WindowTarget {
                    pid: 0,
                    window_id: 0,
                };
                let _answer = core
                    .update(cx, |c, cx| {
                        let rx = c.displays.request_window(id, target, app, title);
                        cx.notify();
                        rx
                    })
                    .ok_or_else(|| ControlError::failed(format!("display {id} has ended")))?;
                cx.background_executor()
                    .timer(Duration::from_millis(900))
                    .await;
                return reply(json!({}));
            }
            let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
            shell
                .update(cx, |shell, cx| shell.debug_show(&surface, id, cx))
                .map_err(|e| ControlError::failed(e.to_string()))?
                .map_err(ControlError::invalid)?;
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            Ok(json!({}))
        }
        other => Err(ControlError::invalid(format!("unknown method {other}"))),
    };
    if let Some(id) = acting {
        crate::mac::catch::agent_acted(id);
    }
    result.and_then(reply)
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
            | "inputQueue"
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
            code: "permission",
            message: "The user turned on Screen Recording, and macOS applies it only once Agent Screen reopens. Agent Screen has put its Settings window on the user's screen: tell the user to press Reopen Agent Screen there, then retry.".into(),
        });
    }
    let names: Vec<&str> = missing.iter().map(|p| p.english()).collect();
    Err(ControlError {
        code: "permission",
        message: format!(
            "Agent Screen is missing the macOS {} permission, and only the user can grant it. \
             Agent Screen has put its Settings window on the user's screen (also reachable from its \
             menu bar icon). Tell the user to turn it on there, then retry; until then every call \
             fails the same way.",
            names.join(" and ")
        ),
    })
}

/// A display this connection may use for `method`: one its Agent Session
/// owns, and still live unless the method is about ending.
fn usable(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    method: &str,
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
    core.read_with(cx, |c, _| {
        let Some(d) = c.displays.get(id) else {
            // Gone or never there: the method's own lookup says which.
            return Ok(());
        };
        let mine = c.sessions.get(&connection);
        let owner = d.owner.as_ref().map(|o| &o.session_key);
        if mine.is_none() || owner != mine {
            return Err(ControlError::not_found(format!(
                "display {id} belongs to another session; call create_display to get yours"
            )));
        }
        if d.is_ended() && !matches!(method, "endDisplay" | "waitForControl") {
            return Err(ended_error(id, d.ended == Some(EndReason::ClosedByUser)));
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
    let adopt: Vec<i32> =
        p.0.get("adopt")
            .and_then(Value::as_array)
            .map(|pids| {
                pids.iter()
                    .filter_map(Value::as_i64)
                    .filter_map(|pid| i32::try_from(pid).ok().filter(|p| *p > 0))
                    .collect()
            })
            .unwrap_or_default();
    if let Some(mut existing) = core.read_with(cx, |c, _| {
        c.displays.for_session(&session_key).map(|d| {
            let size = (d.width, d.height);
            (describe(d, true), size)
        })
    }) {
        let (w, h) = existing.1;
        if asked.0.is_some_and(|a| a.round() != w) || asked.1.is_some_and(|a| a.round() != h) {
            existing.0["note"] = json!(format!(
                "this session already has a {w}×{h} display, reused as is; end_display first for another size"
            ));
        }
        return Ok(existing.0);
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
    let owner = Owner {
        session_key,
        client: p.string("client").unwrap_or_else(|| "Agent".into()),
        title: p.string("title").unwrap_or_else(|| "Untitled".into()),
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
    let spare = match core.update(cx, |c, _| c.take_spare((width, height))) {
        Some((mut handle, name, true)) => {
            let (w, h) = (width as u32, height as u32);
            let (handle, resized) = crate::mac::display::configure(move || {
                let resized = handle.resize(w, h);
                (handle, resized)
            })
            .await;
            if resized {
                Some((handle, name))
            } else {
                crate::lifecycle::note("released", handle.id, "spare could not resize");
                None
            }
        }
        Some((handle, name, false)) => Some((handle, name)),
        None => None,
    };
    let (made, name) = match spare {
        Some((handle, name)) => (Some(handle), name),
        None => {
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
            let (serial, name) = core.update(cx, |c, _| c.next_name());
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
            (made, name)
        }
    };
    // The session may have closed, or got a display from a parallel call,
    // while macOS made this one: then this one goes.
    let key = owner.session_key.clone();
    let raced = core.read_with(cx, |c, _| {
        if c.closed.contains(&connection) {
            Some(Err(ControlError::failed("the connection closed")))
        } else {
            c.displays.for_session(&key).map(|d| Ok(describe(d, true)))
        }
    });
    if let Some(answer) = raced {
        drop(made);
        return answer;
    }
    let adopted = adopt.clone();
    let reply = core
        .update(cx, |c, cx| {
            let id = c.adopt(made?, name, (width, height), Some(owner), cx);
            // Apps the session's displays started before this app quit:
            // their windows, which macOS moved to the user's screen, are
            // strays of this display. A pid another display holds, or one
            // no longer running, stays out.
            for pid in adopt {
                let held = c.displays.list.iter().any(|d| d.started.contains(&pid));
                let alive = unsafe { libc::kill(pid, 0) } == 0;
                if !held
                    && alive
                    && let Some(d) = c.displays.get_mut(id)
                {
                    d.started.insert(pid);
                }
            }
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
        .ok_or_else(|| ControlError::failed("macOS refused to create a virtual display"))?;
    // Answer once the display is where it stays, so a first screenshot
    // shows only the display.
    let id = reply["displayId"].as_u64().unwrap_or_default() as DisplayId;
    let created = started.elapsed().as_millis();
    // Windows macOS put back on the display, of apps adopted above: claimed
    // before AeroSpace takes them for the user's, a second after they show.
    if !adopted.is_empty() {
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
    for _ in 0..100 {
        if core.read_with(cx, |c, _| c.tucked.contains(&id)) {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
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
    log::debug!(
        "createDisplay {id}: created {created} ms, parked {tucked} ms, clear of the user's workspaces {} ms",
        started.elapsed().as_millis()
    );
    Ok(reply)
}

fn describe<H>(d: &crate::model::Display<H>, reused: bool) -> Value {
    json!({
        "displayId": d.id,
        "width": d.width as i64,
        "height": d.height as i64,
        "scale": shot_scale((d.width, d.height)),
        "label": d.label(),
        "reused": reused
    })
}

fn shot_json(shot: &screenshot::Shot) -> Value {
    json!({
        "data": base64::engine::general_purpose::STANDARD.encode(&shot.data),
        "mimeType": shot.mime_type,
        "width": shot.width,
        "height": shot.height
    })
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
    let displays = core.read_with(cx, |c, _| {
        c.displays.list.iter().map(|d| d.id).collect::<Vec<_>>()
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
                    let role = windows::display_role(&w);
                    (w, held, role)
                })
                .collect()
        })
        .await;
    core.update(cx, |c, _| {
        sync_user_made(c, id);
        // This display's own records count as its strays once off it.
        let recorded: std::collections::HashSet<u32> = c
            .displays
            .list
            .iter()
            .filter(|d| {
                d.id != id && cg::window_display_role(d.id) == crate::model::DisplayRole::Agent
            })
            .flat_map(|d| d.records.keys().copied())
            .collect();
        let Some(display) = c.displays.get(id) else {
            return Vec::new();
        };
        off_displays
            .into_iter()
            .filter(|(w, held, role)| {
                display.is_stray_from(
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

/// Ask the user for one of their windows with a Window Request, and wait
/// for the answer. Granted, the window goes over as a Handover.
async fn request_window(
    core: &Entity<Core>,
    id: DisplayId,
    window: windows::Window,
    timeout: Option<f64>,
    cx: &mut AsyncApp,
) -> Result<Map<String, Value>, ControlError> {
    let target = window.target;
    let app = window.app.clone();
    let answer = core
        .update(cx, |c, cx| {
            let rx = c
                .displays
                .request_window(id, target, window.app, window.title);
            cx.notify();
            rx
        })
        .ok_or_else(|| ControlError::failed(format!("display {id} has ended")))?;
    let seconds = timeout.unwrap_or(120.0).clamp(1.0, 600.0);
    let timer = cx
        .background_executor()
        .timer(Duration::from_secs_f64(seconds));
    let given = futures::select! {
        given = answer.fuse() => given.ok(),
        _ = FutureExt::fuse(timer) => {
            core.update(cx, |c, cx| {
                if let Some(d) = c.displays.get_mut(id)
                    && d.window_request.as_ref().is_some_and(|r| r.target == target)
                {
                    d.window_request = None;
                    cx.notify();
                }
            });
            return Err(ControlError::failed(format!(
                "the user didn't answer your request for the {app} window within {seconds:.0} s, so it's withdrawn. Ask again when they're around, or work without it"
            )));
        }
    };
    match given {
        Some(true) => {}
        Some(false) => {
            return Err(ControlError::failed(format!(
                "the user declined to hand over the {app} window; it stays theirs. Work without it, or launch the app yourself with launch_app"
            )));
        }
        None => {
            return Err(ControlError::failed(format!(
                "your request for the {app} window was withdrawn: the display ended or you asked for another window"
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
            return Ok(Map::from_iter([("handedOver".into(), Value::Bool(true))]));
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    Err(ControlError::failed(format!(
        "the user handed over the {app} window, but it didn't reach the display"
    )))
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
    let by = if words.is_some() { "chat" } else { "grant" };
    let home = off(cx, move || crate::dismissal::home_of(target, None)).await;
    let ticket = core
        .update(cx, |c, cx| {
            let ticket = c.displays.hand_over(id, target, &authority, home)?;
            if let (Some(d), Some(text)) = (c.displays.get_mut(id), words.clone()) {
                d.access.last = Some(Instruction {
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
        let landed = c.displays.commit_hand_over(id, &ticket);
        if landed {
            // The app's late windows are the agent's to have, not to send
            // back, as after the user's own Hand Over.
            c.returning
                .retain(|(d, k)| !(*d == id && k.pid() == target.pid));
            c.pending_returns.remove(&(id, target.pid));
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
        if let Some(d) = c.displays.get_mut(id) {
            d.access.last = Some(Instruction {
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

/// The display's Window Access as the agent and scripts read it.
fn access_status(c: &Core, id: DisplayId) -> Value {
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
        "scope": d.access.scope_name(),
        "apps": d.access.apps().iter().map(|a| json!({ "bundleId": a.bundle_id, "name": a.name })).collect::<Vec<_>>(),
        "paused": d.access_paused(),
        "excluded": d.access.excluded,
        "handovers": handovers,
        "lastInstruction": d.access.last.as_ref().map(|i| json!({ "action": i.action.as_str(), "text": i.text })),
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
    let all = p.0.get("all") == Some(&Value::Bool(true));
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
                let d = c.displays.get_mut(id)?;
                let before = d.access.scope.clone();
                match resolved.clone() {
                    None => d.access.grant_all(),
                    Some(list) => d.access.grant_apps(list),
                }
                d.access.last = Some(Instruction {
                    action: ChatAction::Grant,
                    text: words.clone(),
                });
                cx.notify();
                Some(d.access.scope != before)
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
                let access = c.displays.get(id).map(|d| d.access.clone());
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
                let d = c
                    .displays
                    .get_mut(id)
                    .ok_or_else(|| ControlError::failed("the display has ended"))?;
                let changed = d
                    .access
                    .revoke((!ids.is_empty()).then_some(ids.as_slice()))
                    .map_err(ControlError::invalid)?;
                if let Some(text) = words.clone() {
                    d.access.last = Some(Instruction {
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
            // One at a time, each after the input queued before it.
            for window in wanted {
                let record = core.update(cx, |c, cx| {
                    let record = c.displays.return_handover(id, window);
                    if record.is_ok() {
                        if let Some(d) = c.displays.get_mut(id) {
                            d.access.last = Some(Instruction {
                                action: ChatAction::Return,
                                text: words.clone(),
                            });
                        }
                        cx.notify();
                    }
                    record
                });
                let record = match record {
                    Ok(record) => record,
                    Err(e) if !all => return Err(ControlError::not_found(e)),
                    Err(_) => continue,
                };
                let task = shell
                    .update(cx, |shell, cx| {
                        shell.send_home(record.target, id, record.home.clone(), cx)
                    })
                    .map_err(|e| ControlError::failed(e.to_string()))?;
                task.await;
                log::info!(
                    "display {id}: window {window} returned on the user's words as the agent relayed them: {words:?}"
                );
                returned.push(window);
            }
            extra.insert("returned".into(), json!(returned));
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
        let constrained = apply_frame_request(asked, bounds, area, scale, reserved_top, |frame| {
            windows::write_frame_unless_returned(target.window_id, seen, || {
                ax::set_frame_asked(target, frame)
                    .map_err(|e| format!("could not set the window's frame: {e}"))
            })
        })?;
        if !asked.is_empty() {
            // Some apps, Electron's among them, answer a resize a moment
            // later: wait until two reads agree, up to half a second.
            let mut last = None;
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(50));
                let read = now().ok_or_else(closed)?;
                if last == Some(read) {
                    break;
                }
                last = Some(read);
            }
            bounds = last.unwrap_or(bounds);
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

/// Record a window Agent Screen put on a display.
fn claim(
    core: &Entity<Core>,
    id: DisplayId,
    target: WindowTarget,
    origin: Origin,
    cx: &mut AsyncApp,
) {
    core.update(cx, |c, cx| {
        if let Some(d) = c.displays.get_mut(id) {
            d.claim(target, origin, None);
            cx.notify();
        }
    });
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
    action: &str,
    element: &ax::Element,
    value: Option<String>,
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
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
    let click = async |button: Button, count: i64, cx: &mut AsyncApp| -> Result<(), ControlError> {
        let c = center.ok_or_else(|| {
            ControlError::failed("the element has no visible frame on the display")
        })?;
        show_pointer(core, id, c, true, cx)
            .click(c, button, count, Default::default())
            .await;
        Ok(())
    };
    match action {
        "press" => {
            if !perform("AXPress", cx).await {
                click(Button::Left, 1, cx).await?;
            }
        }
        "click" => click(Button::Left, 1, cx).await?,
        "double_click" => click(Button::Left, 2, cx).await?,
        "right_click" => click(Button::Right, 1, cx).await?,
        "show_menu" => {
            if !perform("AXShowMenu", cx).await {
                click(Button::Right, 1, cx).await?;
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
            // Some elements, static text among them, report success and
            // keep their value: read it back.
            let set = off_main(
                cx,
                Box::new(move |e| {
                    ax::settable(e, "AXValue")
                        && ax::set_attribute(
                            e,
                            "AXValue",
                            &objc2_core_foundation::CFString::from_str(&value),
                        )
                        && ax::string_attribute(e, "AXValue")
                            .is_none_or(|now| now.replace('\r', "\n") == value.replace('\r', "\n"))
                }),
            )
            .await;
            if !set {
                return Err(ControlError::failed(
                    "the element's value cannot be set; focus it and type instead",
                ));
            }
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
    Ok(())
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

/// What `launch_app` did.
struct Launched {
    window: Option<windows::Window>,
    was_running: bool,
    /// Why there is no window. For an app Agent Screen started:
    /// `still_starting`, none yet; one it opens soon moves onto the display.
    /// For a running app: `existing_window`, the file shows in a window it
    /// had; `still_opening`, the file's window hasn't come yet;
    /// `just_started`, the user or another session started it moments ago
    /// and it isn't ready; `no_new_window`, its menus have no New Window;
    /// `app_busy`, that item is greyed out; `menus_unreadable`, its menus
    /// can't be read or used.
    reason: Option<&'static str>,
    /// The process, when this launch started it.
    pid: Option<i32>,
}

/// How long a launch watches for a window that comes after it gave up.
const LATE_WINDOW_WAIT: Duration = Duration::from_secs(30);

/// How long a running app may take to make the window Agent Screen asked
/// for. A window it makes later may be the user's.
const ASKED_WINDOW_WAIT: Duration = Duration::from_millis(3000);
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
        .filter(|_| debug_methods())
        .and_then(|ms| ms.parse().ok())
        .map_or(RUNNING_APP_TURN, Duration::from_millis)
}

/// Arguments and environment for a new instance of an app.
#[derive(Default)]
struct Start {
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
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
}

/// Run `f`, which waits on another process, off the UI thread.
async fn off<R: Send + 'static>(cx: &AsyncApp, f: impl FnOnce() -> R + Send + 'static) -> R {
    cx.background_executor().spawn(async move { f() }).await
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
    // a page: then the URL route gives it a window of its own first.
    let browser = match target.filter(|t| t.starts_with("file://")) {
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
    // A catch kept on this app would race the launch's own for its window.
    let pid = off(cx, {
        let path = path.clone();
        move || app::running_pid(&path)
    })
    .await;
    let pause = |c: &mut Core, on: bool| {
        for (_, kept) in &c.kept_catches {
            if Some(kept.pid()) == pid {
                kept.pause(on);
            }
        }
    };
    core.update(cx, |c, _| pause(c, true));
    let result = launch_now(core, id, name, &path, target, start, cx).await;
    core.update(cx, |c, _| {
        pause(c, false);
        c.launching.remove(&path);
    });
    result
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
    let started = Instant::now();
    // Windows already on screen, or claimed by any display, aren't new.
    let mut before: std::collections::HashSet<u32> = off(cx, || {
        windows::all().iter().map(|w| w.target.window_id).collect()
    })
    .await;
    before.extend(core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .flat_map(|d| d.records.keys().copied())
            .collect::<Vec<_>>()
    }));
    // A new instance is a starting app, whatever else runs from the bundle.
    let fresh = start.new_instance();
    let pids = {
        let path = path.clone();
        move || app::running_pids(&path)
    };
    let instances_before = off(cx, pids.clone()).await;
    let was_running = !fresh && !instances_before.is_empty();
    // New windows go straight onto the display, before the user sees them
    // or AeroSpace files them in the user's workspace.
    let bounds = cg::display_bounds(id);
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    // A running app may be the user's: take only the window Agent Screen
    // opens. A starting app's windows are all Agent Screen's.
    let limit = was_running.then_some(1);
    let catch_from = |pid: i32| crate::mac::catch::Catch::start(pid, id, screen.clone(), limit);
    let running = off(cx, pids.clone()).await.into_iter().next();
    let mut catch = match running.filter(|_| !fresh) {
        Some(pid) => catch_from(pid).await,
        None => None,
    };

    // A starting app opens the URL or file in its first window. A running
    // one opens a file in a window of its own, but a URL as a tab of its
    // front window, which may be the user's: it gets a new window first,
    // below, and the URL after.
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
    if let Some(pid) = running.filter(|_| !fresh) {
        core.update(cx, |c, _| c.forwarder(id))
            .remember_user_window(pid)
            .await;
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
    let opening = if piped {
        let (path, first) = (path.clone(), first.clone());
        let mut arguments = start.open_arguments();
        arguments.extend(first);
        let environment = start.environment.clone();
        let started = off(cx, move || {
            hold_launch_for_tests();
            devtools::spawn(&path, &arguments, &environment)
        })
        .await;
        match started {
            Ok(pid) => {
                spawned = Some(pid);
                let (done, outcome) = std::sync::mpsc::channel();
                let _ = done.send(true);
                Some(outcome)
            }
            Err(e) => {
                log::warn!("launch: couldn't start {name} with a pipe ({e}); opening it");
                None
            }
        }
    } else {
        None
    };
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
        let pids = pids.clone();
        move || !pids().contains(&pid)
    };
    for _ in 0..200 {
        // Started here: its pid is known.
        if spawned.is_some() {
            pid = spawned;
            break;
        }
        let now = off(cx, pids.clone()).await;
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
    // first one's.
    if !was_running {
        core.update(cx, |c, _| {
            let claimed = c.displays.list.iter().any(|d| d.started.contains(&pid));
            if !claimed && let Some(d) = c.displays.get_mut(id) {
                d.started.insert(pid);
            }
        });
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
        let hide = spawned.is_some();
        cx.background_executor()
            .spawn(async move {
                if hide && !app::is_hidden(pid) {
                    app::hide(pid);
                }
                for w in windows::all() {
                    if w.target.pid == pid && w.layer == 0 && !seen.contains(&w.target.window_id) {
                        let _ = ax::move_into(w.target, bounds);
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
        core.update(cx, |c, _| {
            if let Some(d) = c.displays.get_mut(id) {
                d.started.remove(&pid);
            }
        });
        return already_running(core, id, name, &instances_before, cx).await;
    }
    // Started hidden: once its windows sit on the display, show them there.
    if !was_running {
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
    if asked_for_window {
        // An app that has just started may not have its menus yet.
        let mut asked = ax::NewWindow::NotReady;
        for _ in 0..10 {
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
        if let Some(reason) = missing {
            drop(catch);
            if is_url {
                let next = if matches!(reason, "just_started" | "still_starting") {
                    "wait a few seconds and call launch_app again".to_string()
                } else {
                    format!("ask the user to quit {name}, then call launch_app again")
                };
                return Err(ControlError::failed(format!(
                    "{name} is running and can't open a new window now ({reason}), so the URL was not opened; {next}"
                )));
            }
            return Ok(Launched {
                window: None,
                was_running,
                reason: Some(reason),
                pid: None,
            });
        }
    }
    let asked_at = Instant::now();

    // A running app may be the user's: its new window is the one the catch
    // took, never one the user opens meanwhile. A starting app's windows
    // are all Agent Screen's.
    // Off the main thread: reading a window's document asks the app, which
    // may be busy.
    let find_ours = |taken: Option<u32>| {
        let (before, document) = (before.clone(), document.clone());
        async move {
            windows::all().into_iter().find(|w| {
                w.target.pid == pid
                    && w.layer == 0
                    && !before.contains(&w.target.window_id)
                    && (!was_running
                        || taken == Some(w.target.window_id)
                        || document
                            .as_deref()
                            .is_some_and(|d| shows_document(w.target, d)))
            })
        }
    };
    let mut gave_up = false;
    // Takes over from `catch` for a file still opening, with no gap between.
    let mut document_catch = None;
    // Every 25 ms for 6 s: the caught window is answered within a tick.
    for tick in 0..240 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        if single_instance_risk && tick % 8 == 0 && off(cx, quit(pid)).await {
            drop(catch);
            core.update(cx, |c, _| {
                if let Some(d) = c.displays.get_mut(id) {
                    d.started.remove(&pid);
                }
            });
            return already_running(core, id, name, &instances_before, cx).await;
        }
        let taken = catch.as_ref().and_then(|c| c.taken());
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
            let placed = settle(core, fresh.target, id, cx).await?;
            log::debug!(
                "launch: {name} window at {found} ms, placed at {} ms",
                started.elapsed().as_millis()
            );
            // The new window is the app's front one now, where it opens the
            // URL or file.
            if was_running
                && is_url
                && let Some(target) = target
            {
                // The title before the URL, to tell when it has arrived.
                let window_id = placed.target.window_id;
                let before = off(cx, move || windows::find(window_id).map(|w| w.title)).await;
                // Into the new window's address field where there is one
                // Agent Screen knows; else the system sends the URL, and the
                // browser opens it in its front window, the new one.
                let (new_window, url) = (placed.target, target.to_string());
                let in_field = cx
                    .background_executor()
                    .spawn(async move { ax::open_in_address_field(new_window, &url) })
                    .await;
                log::debug!("launch: URL through the address field: {in_field}");
                let opened = in_field
                    || match open_in_background(
                        path.clone(),
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
                    };
                if !opened {
                    return Err(ControlError::failed(format!(
                        "{name} opened a window on the display but not {target}; open it there yourself"
                    )));
                }
                // `open` returns once the URL is sent, not handled: wait for
                // the window to show it, at most 2 s, so focus handed back
                // after doesn't make the app open it in the user's window.
                let (window_id, title) = (placed.target.window_id, before);
                for _ in 0..40 {
                    let title = title.clone();
                    if off(cx, move || {
                        windows::find(window_id).is_none_or(|w| Some(w.title) != title)
                    })
                    .await
                    {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }
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
            log::debug!(
                "launch: {name} answered at {} ms",
                started.elapsed().as_millis()
            );
            return Ok(Launched {
                window: Some(placed),
                was_running,
                reason: None,
                pid: (!was_running).then_some(pid),
            });
        }
    }
    keep_catch(core, id, catch, cx);
    log::debug!("launch: {name} made no window of Agent Screen's (gave up early: {gave_up})");
    // A slow launch can open its window after we stop waiting. Keep watching
    // so it does not stay on the user's screen: for an app Agent Screen
    // started, any new window; for a running app, the one showing the file.
    let core = core.clone();
    // The app showed the file in a window it had before; otherwise its
    // window is still to come, and the watcher above moves it.
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
        .clone()
        .filter(|_| !shown_already)
        .filter(|d| core.update(cx, |c, _| c.watching.insert((pid, d.clone()))));
    if let Some(document) = watching {
        // A catch for the file's window moves it as it appears; the poll
        // covers an app that names the document only after.
        let catch = document_catch.take();
        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..200 {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let taken = catch.as_ref().and_then(|c| c.taken());
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
            drop(catch);
            core.update(cx, |c, _| c.watching.remove(&(pid, document)));
        })
        .detach();
    } else if !was_running {
        // The kept catch takes the first window the moment it opens, unless
        // the user is clicking or typing then; the poll is for an app that
        // won't take observers yet.
        crate::mac::catch::agent_acted_for(id, pid, LATE_WINDOW_WAIT);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut seen = before;
            for _ in 0..200 {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                // An app that finished starting after the unhide above, as
                // an Electron development build does, is still hidden, and
                // its windows are listed nowhere: move them in, then show.
                let hidden = off(cx, move || {
                    app::is_hidden(pid) && {
                        if ax::move_all_into(pid, bounds) > 0 {
                            app::unhide(pid);
                        }
                        true
                    }
                })
                .await;
                if hidden {
                    continue;
                }
                let listed = seen.clone();
                let late: Vec<_> = off(cx, move || {
                    windows::all()
                        .into_iter()
                        .filter(|w| {
                            w.target.pid == pid
                                && w.layer == 0
                                && !listed.contains(&w.target.window_id)
                        })
                        .collect()
                })
                .await;
                for window in late {
                    seen.insert(window.target.window_id);
                    // Unless the user took the app back or made it.
                    if still_agents(&core, id, window.target, cx) {
                        let _ = settle(&core, window.target, id, cx).await;
                        return;
                    }
                }
                // Ended or removed meanwhile: the window stays where it opened.
                if !takes_windows(&core, id, cx) {
                    return;
                }
            }
        })
        .detach();
    }
    // The URL waits for the new window, which never came: opening it now
    // would add a tab to whatever window the user has in front.
    if was_running && is_url {
        return Err(ControlError::failed(format!(
            "{name} opened no new window, so the URL was not opened; ask the user to quit {name}, then call launch_app again"
        )));
    }
    Ok(Launched {
        window: None,
        was_running,
        pid: (!was_running).then_some(pid),
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
        }),
        None => Err(ControlError::failed(format!(
            "{name} is already running (pid {}) and allows only one instance: the new instance quit at once and passed its arguments to the running one, which has no window on this display. Ask the user to quit {name}, then call launch_app again; or call launch_app without arguments and environment to ask the running app for a new window",
            running.first().copied().unwrap_or_default()
        ))),
    }
}

/// What to open for a `file://` target: its path, unless the app is a
/// browser, which opens it as a page in a window of its own. The file must
/// exist either way.
fn file_target(target: Option<&str>, browser: bool) -> Result<Option<String>, ControlError> {
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
    let ended = || ControlError::ended(format!("display {id} has ended"));
    if !takes_windows(core, id, cx) {
        return Err(ended());
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
            return Err(ended());
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
fn watch_for_changes(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) {
    let Some(streams) = crate::ui::stream::global() else {
        return;
    };
    let size = core
        .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
        .unwrap_or(DEFAULT_SIZE);
    streams.update(cx, |s, cx| s.want(id, "agent", 30, change_pixels(size), cx));
    ACTION_BASE.with(|b| b.borrow_mut().insert(id, Instant::now()));
    keep_counting(id, cx);
}

/// Keep the display's change count running for a while: the next action
/// usually follows, and a stream that is already counting sees its change.
fn keep_counting(id: DisplayId, cx: &mut AsyncApp) {
    let Some(streams) = crate::ui::stream::global() else {
        return;
    };
    let wanted = Instant::now();
    AGENT_ACTIVE.with(|a| a.borrow_mut().insert(id, wanted));
    let streams = streams.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor()
            .timer(Duration::from_secs(30))
            .await;
        let latest = AGENT_ACTIVE.with(|a| a.borrow().get(&id).copied());
        if latest == Some(wanted) {
            AGENT_ACTIVE.with(|a| a.borrow_mut().remove(&id));
            let _ = streams.update(cx, |s, cx| s.want(id, "agent", 0, (0, 0), cx));
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
    streams.update(cx, |s, cx| s.want(id, "agent", 30, change_pixels(size), cx));
    let start = Instant::now();
    // The stream has been counting since the action, so no change after it
    // is lost to a stream still starting. An app may draw what an action
    // did a moment after its first change, as a page that turns red 400 ms
    // after a click whose counter updates at once (issue #16): so whatever
    // changed, the wait lasts half a second from the action, then until
    // the display has been still for 120 ms.
    let since = ACTION_BASE
        .with(|b| b.borrow_mut().remove(&id))
        .filter(|at| at.elapsed() < Duration::from_secs(5))
        .unwrap_or(start);
    let mut last = streams.read_with(cx, |s, _| s.changes(id));
    let mut quiet_since = start;
    while start.elapsed() < Duration::from_millis(1500) {
        let now = streams.read_with(cx, |s, _| s.changes(id));
        if now != last {
            last = now;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= Duration::from_millis(120)
            && since.elapsed() >= Duration::from_millis(500)
        {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
    }
    keep_counting(id, cx);
}

/// What an agent hears after pressing a menu item of an app built on VS
/// Code, such as Cursor: with the terminal focused, Edit > Find opened the
/// terminal's find box (`cargo e2e cursormenu`).
const VS_CODE_MENU_NOTE: &str = "This app is built on VS Code. It ran this menu item as the item's keyboard shortcut in whatever has focus, not as the menu's own command, because Electron reports a menu item pressed through accessibility as its shortcut. With the terminal, a panel or an input box focused, the result can differ: Edit > Find opened the terminal's find box. Check the window. To run the command itself, open the Command Palette with key cmd+shift+p, type the command's name and press Return.";

/// What an agent hears after pressing an Electron app's menu item.
const ELECTRON_MENU_NOTE: &str = "This is an Electron app. Electron reports a menu item pressed through accessibility to the app as its keyboard shortcut (event.triggeredByAccelerator is true), and some apps ignore shortcuts while a text field has focus. If the window shows no change, move focus out of the text field, with a click on an empty part of the window, then press the item again.";

thread_local! {
    /// When each display last settled after an action.
    static AGENT_ACTIVE: std::cell::RefCell<std::collections::HashMap<DisplayId, std::time::Instant>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// When each display's last action began.
    static ACTION_BASE: std::cell::RefCell<std::collections::HashMap<DisplayId, Instant>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

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

/// Keys posted to the app the user is in reach its key window, which is
/// the user's, not the agent's.
async fn keys_allowed(
    forwarder: &crate::mac::input::Forwarder,
    within: Rect,
    cx: &mut AsyncApp,
) -> Result<(), ControlError> {
    let Some(target) = forwarder.keys_target(within).await else {
        return Err(no_keyboard_target());
    };
    // In front with the agent's window focused, as when an app it started
    // activates itself, keys go to that window: fine.
    let pid = target.pid;
    if crate::mac::input::front() != Some(pid) {
        return Ok(());
    }
    let focused = cx
        .background_executor()
        .spawn(async move { ax::focused_window_id(pid) })
        .await;
    if focused != Some(target.window_id) {
        let name = windows::find(target.window_id).map_or_else(|| "that app".into(), |w| w.app);
        return Err(key_route_error(&name, focused.is_some()));
    }
    Ok(())
}

/// Explain routing evidence without claiming to have observed user activity.
fn key_route_error(app: &str, focused_known: bool) -> ControlError {
    let reason = if focused_known {
        "its focused window is different from the agent's keyboard target"
    } else {
        "Accessibility could not verify its focused window"
    };
    ControlError::failed(format!(
        "{app} is the foreground app, but {reason}, so posted keys could reach the user's window. User idleness does not make this routing safe. The pending keys were refused. Use act_on_element set_value or a supported press action, or ask the user to switch their foreground app before retrying. Agent Screen will not switch it for them."
    ))
}

fn forwarder_stopped(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> bool {
    core.update(cx, |c, _| c.forwarder(id)).agent_stopped()
}

/// Keys through the user's input method (ADR 0008). The display says the
/// agent has the user's keyboard while the keys go in.
async fn through_input_method(
    core: &Entity<Core>,
    id: DisplayId,
    keys: Vec<KeyChord>,
    source: Option<String>,
    cx: &mut AsyncApp,
) -> Result<Params, ControlError> {
    let forwarder = core.update(cx, |c, _| c.forwarder(id));
    let bounds = cg::display_bounds(id);
    keys_allowed(&forwarder, bounds, cx).await?;
    let Some(target) = forwarder.keys_target(bounds).await else {
        forwarder.keys_done().await;
        return Err(no_keyboard_target());
    };
    let borrow = |core: &Entity<Core>, on: bool, cx: &mut AsyncApp| {
        core.update(cx, |c, cx| {
            if let Some(display) = c.displays.get_mut(id) {
                display.keyboard_borrowed = on;
            }
            cx.notify();
        })
    };
    borrow(core, true, cx);
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
    borrow(core, false, cx);
    forwarder.keys_done().await;
    match typed {
        Ok(crate::mac::ime::Typed::All) => Ok(Params::new()),
        Ok(crate::mac::ime::Typed::Stopped(sent)) => Err(ControlError::failed(format!(
            "the user touched the keyboard or pointer after {sent} of {total} keys, so the input method stopped and their app is in front again. Take a screenshot, then send the rest once they've stopped."
        ))),
        Err(reason) => Err(ControlError::failed(reason)),
    }
}

fn no_keyboard_target() -> ControlError {
    ControlError::not_found(
        "no window on the display can take keyboard input; launch an app or click a field first",
    )
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
    let (target, frame) = off(cx, move || crate::mac::input::window_at(from))
        .await
        .ok_or_else(|| ControlError::not_found("no window at the drag's start"))?;
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
    let plan = to.map(|to| crate::input_queue::Plan {
        target,
        frame,
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
            let result = c.input_queue.submit(owner.clone(), operation, reason, plan);
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
            reply(json!({ "route": "real_input", "requestId": request }))
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

// MARK: Parameters

fn display(
    core: &Entity<Core>,
    p: &Args,
    live: bool,
    cx: &mut AsyncApp,
) -> Result<DisplayId, ControlError> {
    let id = p
        .int("displayId")
        .ok_or_else(|| ControlError::invalid("displayId is required"))? as DisplayId;
    core.read_with(cx, |c, _| match c.displays.get(id) {
        None if c.displays.retired.contains(&id) => {
            Err(ended_error(id, c.displays.closed_by_user.contains(&id)))
        }
        None => Err(ControlError::not_found(format!(
            "display {id} does not exist; call create_display again"
        ))),
        Some(d) if live && d.is_ended() => {
            Err(ended_error(id, d.ended == Some(EndReason::ClosedByUser)))
        }
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

/// The error for a call on an ended display. One the user closed means
/// they want the agent to stop.
fn ended_error(id: DisplayId, by_user: bool) -> ControlError {
    ControlError::ended(if by_user {
        format!(
            "the user closed display {id}, so stop working on screen; ask the user before you call create_display again"
        )
    } else {
        format!("display {id} has ended; call create_display for a new one")
    })
}

async fn target(
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
    off(cx, move || windows::on_display(id))
        .await
        .into_iter()
        .find(|w| w.target.window_id as i64 == window)
        .ok_or_else(|| {
            ControlError::not_found(format!(
                "no window {window} on display {id}; see list_windows"
            ))
        })
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

/// Loosely typed request parameters from JSON.
struct Args<'a>(&'a Params);

impl Args<'_> {
    fn string(&self, key: &str) -> Option<String> {
        self.0.get(key)?.as_str().map(String::from)
    }
    fn number(&self, key: &str) -> Option<f64> {
        self.0.get(key)?.as_f64()
    }
    fn int(&self, key: &str) -> Option<i64> {
        let value = self.0.get(key)?;
        value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
    }
    fn require(&self, key: &str) -> Result<f64, ControlError> {
        self.number(key)
            .ok_or_else(|| ControlError::invalid(format!("{key} is required")))
    }
}

/// Test hooks: in debug builds, or with AGENTSCREEN_DEBUG=1 for measuring a
/// release build.
fn debug_methods() -> bool {
    cfg!(debug_assertions) || std::env::var_os("AGENTSCREEN_DEBUG").is_some_and(|v| v == "1")
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    fn ago(ms: u64) -> Instant {
        Instant::now() - Duration::from_millis(ms)
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
        // A browser keeps the URL.
        assert_eq!(file_target(Some(&format!("{url}#x")), true).unwrap(), None);
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
mod keyboard_refusal_tests {
    #[test]
    fn refusal_distinguishes_routing_evidence_from_user_activity() {
        let different = super::key_route_error("Dia", true).message;
        assert!(different.contains("focused window is different"));
        assert!(different.contains("User idleness does not make this routing safe"));
        assert!(different.contains("ask the user to switch"));
        assert!(!different.contains("user is working"));
        let unknown = super::key_route_error("Dia", false).message;
        assert!(unknown.contains("Accessibility could not verify"));
        assert!(!unknown.contains("focused window is different"));
    }
}
