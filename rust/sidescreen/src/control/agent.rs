//! Serves control requests from Agent Sessions. Coordinates are in points
//! relative to the display's top-left corner, the same space as screenshots.

use super::server::{ControlError, Reply};
use crate::core::Core;
use crate::keychord::KeyChord;
use crate::mac::input::{Button, Sent};
use crate::mac::screenshot::{self, Format};
use crate::mac::windows::Grab;
use crate::mac::{app, ax, cg, windows};
use crate::model::{
    DEFAULT_SIZE, DisplayId, Disposition, EndReason, Origin, Owner, Rect, Wait, WindowTarget,
    shot_scale,
};
use crate::numbers;
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
    let reply = handle_one(core.clone(), connection, method, p, cx).await;
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
        if cg::screen_locked() {
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
    }
    let result: Result<Value, ControlError> = match method {
        "createDisplay" => create_display(&core, connection, &p, cx).await,
        "recordStart" => {
            let id = display(&core, &p, true, cx)?;
            let label = core.read_with(cx, |c, _| {
                c.displays.get(id).map(|d| d.label()).unwrap_or_default()
            });
            let (video, timeline) = cx
                .background_executor()
                .spawn(async move { crate::mac::recording::start(id, &label) })
                .await
                .map_err(ControlError::failed)?;
            core.update(cx, |_, cx| cx.notify());
            Ok(json!({ "video": video, "timeline": timeline }))
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
            let (id, point) = target(&core, &p, cx)?;
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
            let (id, point) = target(&core, &p, cx)?;
            if let Some(grab) = GRAB.with(|g| g.borrow().get(&id).copied()) {
                show_pointer(&core, id, point, false, cx);
                drag_to(grab, point, cx).await;
                return reply(json!({}));
            }
            show_pointer(&core, id, point, false, cx)
                .move_to(point)
                .await;
            Ok(json!({}))
        }
        "drag" => {
            let id = input_display(&core, &p, cx)?;
            let from = global(&core, id, p.require("fromX")?, p.require("fromY")?, cx)?;
            let to = global(&core, id, p.require("toX")?, p.require("toY")?, cx)?;
            outside_menu_bar(id, from)?;
            let modifiers = modifiers(&p)?;
            // A drag on a title bar moves the window, which posted events
            // can't do.
            if let Some(grab) = title_bar_at(id, from, cx).await {
                show_pointer(&core, id, to, false, cx);
                if !drag_to(grab, to, cx).await {
                    return Err(ControlError::failed(
                        "the app would not let its window move",
                    ));
                }
                return reply(json!({ "movedWindow": true }));
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
            Ok(json!({}))
        }
        "mouseDown" | "mouseUp" => {
            let down = method == "mouseDown";
            let id = input_display(&core, &p, cx)?;
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
            let (id, point) = target(&core, &p, cx)?;
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
            let id = input_display(&core, &p, cx)?;
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
            let id = input_display(&core, &p, cx)?;
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
            let id = input_display(&core, &p, cx)?;
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
            let list: Vec<Value> = strays(&core, id, cx).iter().map(|w| w.json()).collect();
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
            let list: Vec<Value> = windows::on_display(id)
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
            Ok(json!({ "windows": list }))
        }
        "setDisposition" => {
            let id = display(&core, &p, true, cx)?;
            let window = p
                .int("windowId")
                .and_then(|w| {
                    windows::on_display(id)
                        .into_iter()
                        .find(|x| x.target.window_id as i64 == w)
                })
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
            let id = input_display(&core, &p, cx)?;
            let wanted = p.int("windowId").unwrap_or(-1);
            let returned = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .is_some_and(|d| d.returned.contains(&(wanted as u32)))
            });
            if returned {
                return Err(ControlError::failed(
                    "the user took that window back; it stays theirs. Ask them to hand it over again if you need it",
                ));
            }
            let released = windows::find(wanted as u32).is_some_and(|w| {
                core.read_with(cx, |c, _| {
                    c.displays
                        .get(id)
                        .is_some_and(|d| d.released.contains(&w.target.pid))
                })
            });
            if released {
                return Err(ControlError::failed(
                    "the user took back a window of that app, which makes the app theirs: its windows stay on their screen. Ask the user to hand over the window you need",
                ));
            }
            let made = core.update(cx, |c, _| {
                sync_user_made(c, id);
                c.displays
                    .get(id)
                    .is_some_and(|d| d.user_made.contains(&(wanted as u32)))
            });
            if made {
                return Err(ControlError::failed(
                    "the user opened that window themselves, so it stays on their screen. Ask them to hand it over if you need it",
                ));
            }
            let window = strays(&core, id, cx)
                .into_iter()
                .find(|w| w.target.window_id as i64 == wanted)
                .ok_or_else(|| {
                    ControlError::not_found(
                        "that window isn't a stray of an app you started: call list_windows with strays: true. For one of the user's windows, ask the user to hand it over",
                    )
                })?;
            claim(&core, id, window.target, Origin::Placed, cx);
            if let Err(e) = place(window.target, id, cx).await {
                // Still a stray, so the agent can try again.
                core.update(cx, |c, _| {
                    if let Some(d) = c.displays.get_mut(id) {
                        d.forget(window.target.window_id);
                    }
                });
                return Err(ControlError::failed(format!(
                    "could not move the window: {e}"
                )));
            }
            Ok(json!({}))
        }
        "windowState" => {
            let id = display(&core, &p, false, cx)?;
            let window = window_on(id, &p)?;
            let target = window.target;
            let area = cg::display_bounds(id);
            let origin = (area.x, area.y);
            let scale = display_scale(&core, id, cx);
            // Walking a tree takes a round trip per element.
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
            let mut result = shot_json(&shot);
            result["window"] = in_shot(window.json(), scale);
            result["tree"] = json!(tree);
            result["elements"] = json!(count);
            result["truncated"] = json!(snapshot.truncated);
            add_strayed(&core, id, &mut result, cx);
            Ok(result)
        }
        "elementAction" => {
            let id = input_display(&core, &p, cx)?;
            let window = window_on(id, &p)?;
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
            Ok(json!({}))
        }
        "launchApp" => {
            let id = input_display(&core, &p, cx)?;
            let name = p
                .string("app")
                .ok_or_else(|| ControlError::invalid("app is required"))?;
            let target = p.string("open").map(|t| open_target(&t)).transpose()?;
            let start = Start::parse(&p)?;
            let launched = launch(&core, id, &name, target.as_deref(), &start, cx).await?;
            Ok(match launched.window {
                Some(w) => json!({
                    "window": in_shot(w.json(), display_scale(&core, id, cx)),
                    "wasRunning": launched.was_running
                }),
                None => json!({ "wasRunning": launched.was_running, "reason": launched.reason }),
            })
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
                sidescreen_model::model::help_steps(steps).map_err(ControlError::invalid)?;
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
            Ok(json!({ "outcome": outcome, "helpOpen": help_open }))
        }
        // Test hook: stands in for the user's input in the Viewer.
        "debugTakeover" if debug_methods() => {
            let id = display(&core, &p, false, cx)?;
            let active = p.0.get("active") == Some(&Value::Bool(true));
            core.update(cx, |c, cx| {
                let changed = if active {
                    c.displays.begin_takeover(id)
                } else {
                    c.displays.end_takeover(id)
                };
                if changed {
                    c.stop_agent_input(id, active);
                    cx.notify();
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
            Ok(json!({ "frame": frame, "image": image, "key": key, "misrouted": misrouted }))
        }
        "debugViewerInput" if debug_methods() => {
            use crate::mac::viewer_input;
            match p.string("kind").as_deref() {
                Some("click") => viewer_input::inject_click(
                    p.number("x").unwrap_or(0.5),
                    p.number("y").unwrap_or(0.5),
                ),
                Some("key") => viewer_input::inject_key(
                    p.int("code").unwrap_or(0) as u16,
                    p.int("shift").unwrap_or(0) == 1,
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
                .map_err(|e| ControlError::failed(e.to_string()))?;
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
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
        "debugShow" if debug_methods() => {
            let surface = p.string("surface").unwrap_or_else(|| "viewer".into());
            let id = p.int("displayId").unwrap_or(0) as DisplayId;
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
            | "waitForControl"
            | "setDisposition"
            | "listWindows"
            | "agentReady"
    ) && !method.starts_with("debug")
}

/// Fail with what the user has to do when a permission is missing, and put
/// the setup window in front of them once for each state.
async fn permitted(cx: &mut AsyncApp) -> Result<(), ControlError> {
    use std::sync::atomic::Ordering;
    let missing = crate::mac::permissions::status().missing();
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
            message: "The user turned on Screen Recording, and macOS applies it only once Sidescreen reopens. Sidescreen has put its setup window on the user's screen: tell the user to press Reopen Sidescreen there, then retry.".into(),
        });
    }
    let names: Vec<&str> = missing.iter().map(|p| p.english()).collect();
    Err(ControlError {
        code: "permission",
        message: format!(
            "Sidescreen is missing the macOS {} permission, and only the user can grant it. \
             Sidescreen has put its setup window on the user's screen (also reachable from its \
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
            if let Some(aerospace) = crate::mac::aerospace::AeroSpace::current() {
                aerospace.remember_before_first();
            }
        })
        .await;
    let started = Instant::now();
    // A spare of this size costs macOS nothing; a new display is a
    // reconfiguration every app handles, so agents get only so many.
    let (made, name) = match core.update(cx, |c, _| c.take_spare((width, height))) {
        Some((handle, name)) => (Some(handle), name),
        None => {
            if let Err(why) = core.update(cx, |c, _| c.budget_allows()) {
                return Err(ControlError::failed(why));
            }
            if let Err(wait) = core.update(cx, |c, _| c.may_make()) {
                return Err(ControlError::failed(format!(
                    "Sidescreen made many displays in the last minutes, and each one makes every app on this Mac reconfigure its screens. Keep using the display you have, or call create_display again in {} s.",
                    wait.as_secs().max(1)
                )));
            }
            core.update(cx, |c, _| c.count_made());
            // macOS takes 300 to 500 ms to make the display: off the main
            // thread, so Peek and the Viewer keep drawing meanwhile.
            let (serial, name) = core.update(cx, |c, _| c.next_name());
            let made = cx
                .background_executor()
                .spawn({
                    let name = name.clone();
                    async move {
                        crate::mac::display::VirtualDisplay::create(
                            &name,
                            width as u32,
                            height as u32,
                            serial,
                        )
                    }
                })
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
    let reply = core
        .update(cx, |c, cx| {
            let id = c.adopt(made?, name, (width, height), Some(owner), cx);
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
    // Off the reply's path: macOS takes up to seconds the first time.
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let _ = cx.update(|_| {
            let mtm = MainThreadMarker::new()?;
            let labelling = std::time::Instant::now();
            let labelled = app::label_display(id, mtm);
            log::debug!(
                "display {id}: desktop picture {labelled} in {} ms",
                labelling.elapsed().as_millis()
            );
            Some(())
        });
    })
    .detach();
    // Read AeroSpace's state now that the display exists: the user may have
    // started, quit or reconfigured it since Sidescreen last looked.
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
    json!({ "displayId": d.id, "width": d.width as i64, "height": d.height as i64, "label": d.label(), "reused": reused })
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
fn strays(core: &Entity<Core>, id: DisplayId, cx: &mut AsyncApp) -> Vec<windows::Window> {
    let displays = core.read_with(cx, |c, _| {
        c.displays.list.iter().map(|d| d.id).collect::<Vec<_>>()
    });
    let placed: std::collections::HashSet<u32> = displays
        .into_iter()
        .flat_map(windows::on_display)
        .map(|w| w.target.window_id)
        .collect();
    let off_displays: Vec<windows::Window> = windows::all()
        .into_iter()
        .filter(|w| w.layer == 0 && !placed.contains(&w.target.window_id))
        .collect();
    core.update(cx, |c, _| {
        sync_user_made(c, id);
        let recorded: std::collections::HashSet<u32> = c
            .displays
            .list
            .iter()
            .flat_map(|d| d.records.keys().copied())
            .collect();
        let Some(display) = c.displays.get(id) else {
            return Vec::new();
        };
        off_displays
            .into_iter()
            .filter(|w| display.is_stray(w.target, recorded.contains(&w.target.window_id)))
            .collect()
    })
}

/// Record a window Sidescreen put on a display.
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

pub async fn place(target: WindowTarget, id: DisplayId, cx: &mut AsyncApp) -> Result<(), String> {
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    cx.background_executor()
        .spawn(async move { windows::place(target, id, screen.as_deref()) })
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

/// `open -g`: without activating, like OpenConfiguration.activates = false.
async fn open_in_background(
    app: std::path::PathBuf,
    target: Option<String>,
    hidden: bool,
    start: &Start,
    cx: &mut AsyncApp,
) -> Option<std::sync::mpsc::Receiver<bool>> {
    let new_instance = start.new_instance();
    let arguments = start.arguments.clone();
    let environment = start.environment.clone();
    cx.background_executor()
        .spawn(async move {
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
            // A document app that starts with nothing to open shows an Open
            // panel, owned by a system service and left on the user's
            // screen, instead of a window. Ask for an untitled document.
            // Only a starting app reads its arguments.
            // Nor bring back windows from its last run, unsaved text
            // included, which may be the user's.
            // Given arguments, the app gets exactly those: Electron takes
            // the first one that isn't a flag for its app folder.
            open.arg("--args");
            if arguments.is_empty() {
                open.args([
                    "-NSShowAppCentricOpenPanelInsteadOfUntitledFile",
                    "NO",
                    "-ApplePersistenceIgnoreState",
                    "YES",
                ]);
            }
            open.args(&arguments);
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
    /// Why there is no window. For an app Sidescreen started:
    /// `still_starting`, none yet; one it opens soon moves onto the display.
    /// For a running app: `existing_window`, the file shows in a window it
    /// had; `still_opening`, the file's window hasn't come yet;
    /// `just_started`, the user or another session started it moments ago
    /// and it isn't ready; `no_new_window`, its menus have no New Window;
    /// `app_busy`, that item is greyed out; `menus_unreadable`, its menus
    /// can't be read or used.
    reason: Option<&'static str>,
}

/// How long a launch watches for a window that comes after it gave up.
const LATE_WINDOW_WAIT: Duration = Duration::from_secs(30);

/// How long a running app may take to make the window Sidescreen asked
/// for. A window it makes later may be the user's.
const ASKED_WINDOW_WAIT: Duration = Duration::from_millis(3000);
/// The same, counted from when `open` returns: it returns once the app has
/// the file.
const OPENED_WINDOW_WAIT: Duration = Duration::from_millis(2000);
/// However long `open` takes, a running app's turn ends this long after
/// Sidescreen's request.
const RUNNING_APP_TURN: Duration = Duration::from_millis(4000);

/// RUNNING_APP_TURN, or in test builds SIDESCREEN_RUNNING_TURN_MS, which
/// sends every file down the path of one that opens late.
fn running_app_turn() -> Duration {
    std::env::var("SIDESCREEN_RUNNING_TURN_MS")
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

    /// A running instance would ignore both: a new one starts.
    fn new_instance(&self) -> bool {
        !self.arguments.is_empty() || !self.environment.is_empty()
    }
}

async fn launch(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    target: Option<&str>,
    start: &Start,
    cx: &mut AsyncApp,
) -> Result<Launched, ControlError> {
    let path = app::resolve(name).ok_or_else(|| {
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
    let pid = app::running_pid(&path);
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
    let mut before: std::collections::HashSet<u32> =
        windows::all().iter().map(|w| w.target.window_id).collect();
    before.extend(core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .flat_map(|d| d.records.keys().copied())
            .collect::<Vec<_>>()
    }));
    // A new instance is a starting app, whatever else runs from the bundle.
    let fresh = start.new_instance();
    let instances_before = app::running_pids(&path);
    let was_running = !fresh && !instances_before.is_empty();
    // New windows go straight onto the display, before the user sees them
    // or AeroSpace files them in the user's workspace.
    let bounds = cg::display_bounds(id);
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    // A running app may be the user's: take only the window Sidescreen
    // opens. A starting app's windows are all Sidescreen's.
    let limit = was_running.then_some(1);
    let catch_from = |pid: i32| crate::mac::catch::Catch::start(pid, id, screen.clone(), limit);
    let mut catch = if fresh {
        None
    } else {
        app::running_pid(&path).and_then(catch_from)
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
    // before Sidescreen sees it: note theirs now, to hand it back after.
    if let Some(pid) = app::running_pid(&path).filter(|_| !fresh) {
        core.update(cx, |c, _| c.forwarder(id))
            .remember_user_window(pid)
            .await;
    }
    let opening = if !was_running || first.is_some() {
        Some(
            open_in_background(path.clone(), first, !was_running, start, cx)
                .await
                .ok_or_else(could_not_open)?,
        )
    } else {
        None
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
    for _ in 0..200 {
        pid = if fresh {
            app::running_pids(&path)
                .into_iter()
                .find(|p| !instances_before.contains(p))
        } else {
            app::running_pid(&path)
        };
        if pid.is_some() {
            break;
        }
        if poll_open(&mut opened) {
            return Err(could_not_open());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    let pid = pid.ok_or_else(|| ControlError::failed(format!("{name} did not start")))?;
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
        if catch.is_some() {
            break;
        }
        catch = catch_from(pid);
        let seen = before.clone();
        cx.background_executor()
            .spawn(async move {
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
    // Started hidden: once its windows sit on the display, show them there.
    if !was_running {
        for _ in 0..150 {
            let moved = cx
                .background_executor()
                .spawn(async move { ax::move_all_into(pid, bounds) })
                .await;
            if moved > 0 {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        app::unhide(pid);
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
            app::launched_within(pid, LATE_WINDOW_WAIT)
                && !windows::all()
                    .iter()
                    .any(|w| w.target.pid == pid && w.layer == 0)
        };
        // Only this display's own launch has a watch that moves the window.
        let started_here = core.read_with(cx, |c, _| {
            c.displays.get(id).is_some_and(|d| d.started.contains(&pid))
        });
        // This display's own launch answers at once: its watch moves the
        // window when it comes.
        if asked == ax::NewWindow::Missing && !started_here && just_started() {
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
        let missing = match asked {
            ax::NewWindow::Pressed => None,
            ax::NewWindow::Disabled => Some("app_busy"),
            // The first launch's watch moves its window when it comes.
            ax::NewWindow::Missing if started_here && just_started() => Some("still_starting"),
            // Started by the user or another session moments ago.
            ax::NewWindow::Missing if just_started() => Some("just_started"),
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
            });
        }
    }
    let asked_at = Instant::now();

    // A running app may be the user's: its new window is the one the catch
    // took, never one the user opens meanwhile. A starting app's windows
    // are all Sidescreen's.
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
    for _ in 0..240 {
        cx.background_executor()
            .timer(Duration::from_millis(25))
            .await;
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
            document_catch = document
                .clone()
                .and_then(|d| crate::mac::catch::Catch::for_document(pid, id, screen.clone(), d));
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
                let before = windows::find(placed.target.window_id).map(|w| w.title);
                // Into the new window's address field where there is one
                // Sidescreen knows; else the system sends the URL, and the
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
                    if windows::find(window_id).is_none_or(|w| Some(w.title) != title) {
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
                let forwarder = core.update(cx, |c, _| c.forwarder(id));
                forwarder.wake(placed.target, placed.bounds).await;
            }
            // A page's buttons work only once its scripts have run: an
            // agent clicking straight after would click a dead page.
            if is_url {
                wait_for_page(placed.target, cx).await;
            }
            keep_catch(core, id, catch, cx);
            return Ok(Launched {
                window: Some(placed),
                was_running,
                reason: None,
            });
        }
    }
    keep_catch(core, id, catch, cx);
    log::debug!("launch: {name} made no window of Sidescreen's (gave up early: {gave_up})");
    // A slow launch can open its window after we stop waiting. Keep watching
    // so it does not stay on the user's screen: for an app Sidescreen
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
                if app::is_hidden(pid) {
                    let moved = cx
                        .background_executor()
                        .spawn(async move { ax::move_all_into(pid, bounds) })
                        .await;
                    if moved > 0 {
                        app::unhide(pid);
                    }
                    continue;
                }
                let late: Vec<_> = windows::all()
                    .into_iter()
                    .filter(|w| {
                        w.target.pid == pid && w.layer == 0 && !seen.contains(&w.target.window_id)
                    })
                    .collect();
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

/// Whether a running app has had its time to make the window Sidescreen
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
    target: WindowTarget,
    id: DisplayId,
    cx: &mut AsyncApp,
) -> Result<windows::Window, ControlError> {
    let ended = || ControlError::ended(format!("display {id} has ended"));
    if !takes_windows(core, id, cx) {
        return Err(ended());
    }
    claim(core, id, target, Origin::Launched, cx);
    // Stock AeroSpace may take a placed window back to its own layout a
    // moment later; then look twice, a while apart. Otherwise a window
    // placed is a window held.
    let aerospace_takes = !crate::mac::aerospace::ignores_now()
        && cx
            .background_executor()
            .spawn(async { crate::mac::aerospace::AeroSpace::current().is_some() })
            .await;
    let (first_look, second_look) = if aerospace_takes {
        (200, Some(300))
    } else {
        (30, None)
    };
    let mut last_error = None;
    for _ in 0..12 {
        if !takes_windows(core, id, cx) {
            return Err(ended());
        }
        if let Err(e) = place(target, id, cx).await {
            last_error = Some(e);
        }
        cx.background_executor()
            .timer(Duration::from_millis(first_look))
            .await;
        if !held(target, id, cx).await {
            continue;
        }
        if let Some(wait) = second_look {
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            if !held(target, id, cx).await {
                log::info!(
                    "window {} left display {id} after placing; placing again",
                    target.window_id
                );
                continue;
            }
        }
        if let Some(placed) = windows::on_display(id)
            .into_iter()
            .find(|w| w.target == target)
        {
            // The new app's window takes the keys, not the last one clicked.
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            forwarder.focus(target).await;
            // A window bigger than the display gets pulled inside.
            let moved = cx
                .background_executor()
                .spawn(async move { windows::keep_inside(id) })
                .await;
            if moved
                && let Some(kept) = windows::on_display(id)
                    .into_iter()
                    .find(|w| w.target == target)
            {
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
async fn wait_until_still(id: DisplayId, size: (f64, f64), cx: &mut AsyncApp) {
    use std::time::Instant;
    let Some(streams) = crate::ui::stream::global() else {
        cx.background_executor()
            .timer(Duration::from_millis(350))
            .await;
        return;
    };
    let pixels = (416, (416.0 * size.1 / size.0) as u32);
    streams.update(cx, |s, cx| s.want(id, "agent", 30, pixels, cx));
    let start = Instant::now();
    let mut last = streams.read_with(cx, |s, _| s.changes(id));
    let mut quiet_since = start;
    // An app can take a while to show what an action opened, such as a
    // file panel drawn by another process: give a first change longer.
    let mut changed = false;
    while start.elapsed() < Duration::from_millis(1500) {
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        let now = streams.read_with(cx, |s, _| s.changes(id));
        if now != last {
            last = now;
            changed = true;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= Duration::from_millis(120)
            && start.elapsed() >= Duration::from_millis(if changed { 150 } else { 500 })
        {
            break;
        }
    }
    // Keep the fast stream for a moment: the next action usually follows.
    let wanted = Instant::now();
    AGENT_ACTIVE.with(|a| a.borrow_mut().insert(id, wanted));
    let streams = streams.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(Duration::from_secs(5)).await;
        let latest = AGENT_ACTIVE.with(|a| a.borrow().get(&id).copied());
        if latest == Some(wanted) {
            AGENT_ACTIVE.with(|a| a.borrow_mut().remove(&id));
            let _ = streams.update(cx, |s, cx| s.want(id, "agent", 0, (0, 0), cx));
        }
    })
    .detach();
}

thread_local! {
    /// When each display last settled after an action.
    static AGENT_ACTIVE: std::cell::RefCell<std::collections::HashMap<DisplayId, std::time::Instant>> =
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
    let elsewhere = app::frontmost_pid() == Some(pid)
        && cx
            .background_executor()
            .spawn(async move { ax::focused_window_id(pid) })
            .await
            != Some(target.window_id);
    if elsewhere {
        let name = windows::find(target.window_id).map_or_else(|| "that app".into(), |w| w.app);
        return Err(ControlError::failed(format!(
            "the user is working in {name} right now, so keys would reach their window, not yours. Set text with act_on_element (set_value) instead, or wait and try again."
        )));
    }
    Ok(())
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
    let typed = cx
        .background_executor()
        .spawn(async move { crate::mac::ime::type_keys(target, &keys, source.as_deref()) })
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
fn input_display(
    core: &Entity<Core>,
    p: &Args,
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
    let before = cg::all_ids();
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
    if cg::on_screen_ids().is_subset(&before) {
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

fn target(
    core: &Entity<Core>,
    p: &Args,
    cx: &mut AsyncApp,
) -> Result<(DisplayId, (f64, f64)), ControlError> {
    let id = input_display(core, p, cx)?;
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
            "that is the menu bar strip at the top, which belongs to the user's app, not yours; \
             press a menubaritem from get_window_state to use your app's menus",
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
    core.update(cx, |c, cx| {
        c.displays
            .point(id, at.0 - bounds.x, at.1 - bounds.y, click);
        cx.notify();
        c.forwarder(id)
    })
}

fn window_on(id: DisplayId, p: &Args) -> Result<windows::Window, ControlError> {
    let window = p
        .int("windowId")
        .ok_or_else(|| ControlError::invalid("windowId is required"))?;
    windows::on_display(id)
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

/// Test hooks: in debug builds, or with SIDESCREEN_DEBUG=1 for measuring a
/// release build.
fn debug_methods() -> bool {
    cfg!(debug_assertions) || std::env::var_os("SIDESCREEN_DEBUG").is_some_and(|v| v == "1")
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    fn ago(ms: u64) -> Instant {
        Instant::now() - Duration::from_millis(ms)
    }

    #[test]
    fn routes_file_urls() {
        let dir = std::env::temp_dir().join(format!("sidescreen-route-{}", std::process::id()));
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
