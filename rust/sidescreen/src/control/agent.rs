//! Serves control requests from Agent Sessions. Coordinates are in points
//! relative to the display's top-left corner, the same space as screenshots.

use super::server::{ControlError, Reply};
use crate::core::Core;
use crate::keychord::KeyChord;
use crate::mac::input::Button;
use crate::mac::screenshot::{self, Format};
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
    let p = Args(&p);
    if needs_permissions(method) {
        permitted(cx)?;
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
    let result: Result<Value, ControlError> = match method {
        "createDisplay" => create_display(&core, connection, &p, cx).await,
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
                    let (w, h) = (size.0 * scale, size.1 * scale);
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
                grab.drag_to(point, cx).await;
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
                if !grab.drag_to(to, cx).await {
                    return Err(ControlError::failed(
                        "the app would not let its window move",
                    ));
                }
                return reply(json!({ "movedWindow": true }));
            }
            let forwarder = show_pointer(&core, id, from, true, cx);
            let dragged = forwarder.drag(from, to, modifiers).await;
            show_pointer(&core, id, to, false, cx);
            if !dragged {
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
                grab.drag_to(point, cx).await;
                return reply(json!({ "windowId": grab.target.window_id }));
            }
            let modifiers = modifiers(&p)?;
            let hit = show_pointer(&core, id, point, down, cx)
                .button(point, down, modifiers)
                .await;
            match hit {
                Some(window) => Ok(json!({ "windowId": window.window_id })),
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
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            if !forwarder.type_text(text, cg::display_bounds(id)).await {
                return Err(no_keyboard_target());
            }
            Ok(json!({}))
        }
        "key" => {
            let id = input_display(&core, &p, cx)?;
            let keys = p
                .string("keys")
                .ok_or_else(|| ControlError::invalid("keys is required"))?;
            let chords = KeyChord::parse_sequence(&keys)
                .map_err(|e| ControlError::invalid(e.to_string()))?;
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            let repeat = p.int("repeat").unwrap_or(1);
            let bounds = cg::display_bounds(id);
            for chord in chords {
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
            if !forwarder.chord(chord, true, within).await {
                return Err(no_keyboard_target());
            }
            cx.background_executor().timer(held).await;
            forwarder.chord(chord, false, within).await;
            Ok(json!({}))
        }
        "listWindows" => {
            let id = display(&core, &p, false, cx)?;
            let records = core.read_with(cx, |c, _| {
                c.displays
                    .get(id)
                    .map(|d| d.records.clone())
                    .unwrap_or_default()
            });
            let list: Vec<Value> = windows::on_display(id)
                .iter()
                .map(|w| {
                    let mut json = w.json();
                    let record = records.get(&w.target.window_id);
                    json["origin"] = json!(record.map_or("unknown", |r| r.origin.as_str()));
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
            let window = p
                .int("windowId")
                .and_then(|w| windows::find(w as u32))
                .ok_or_else(|| ControlError::not_found("no window with that id"))?;
            claim(&core, id, window.target, Origin::Placed, cx);
            place(window.target, id, cx)
                .await
                .map_err(|e| ControlError::failed(format!("could not move the window: {e}")))?;
            Ok(json!({}))
        }
        "windowState" => {
            let id = display(&core, &p, false, cx)?;
            let window = window_on(id, &p)?;
            let target = window.target;
            let area = cg::display_bounds(id);
            let origin = (area.x, area.y);
            // Walking a tree takes a round trip per element.
            let snapshot = cx
                .background_executor()
                .spawn(async move { ax::snapshot(target, origin) })
                .await
                .ok_or_else(|| {
                    ControlError::not_found(
                        "the window has no accessibility element; is Accessibility granted?",
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
            result["window"] = window.json();
            result["tree"] = json!(tree);
            result["elements"] = json!(count);
            result["truncated"] = json!(snapshot.truncated);
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
            let launched = launch(&core, id, &name, target.as_deref(), cx).await?;
            Ok(match launched.window {
                Some(w) => json!({ "window": w.json(), "wasRunning": launched.was_running }),
                None => json!({ "wasRunning": launched.was_running, "reason": launched.reason }),
            })
        }
        "requestHelp" => {
            let id = display(&core, &p, true, cx)?;
            let reason = p
                .string("reason")
                .filter(|r| !r.is_empty())
                .ok_or_else(|| ControlError::invalid("reason is required"))?;
            core.update(cx, |c, cx| {
                if c.displays.request_help(id, reason) {
                    cx.notify();
                }
            });
            Ok(json!({}))
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
            Ok(json!({ "outcome": outcome.as_str() }))
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
                    cx.notify();
                }
            });
            Ok(json!({}))
        }
        "debugViewer" if debug_methods() => {
            let (frame, image, key) = crate::mac::viewer_input::debug_layout()
                .ok_or_else(|| ControlError::not_found("the Viewer shows no display"))?;
            Ok(json!({ "frame": frame, "image": image, "key": key }))
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
                _ => Err("kind is click or key".into()),
            }
            .map_err(ControlError::failed)?;
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
    result.and_then(reply)
}

/// Methods that see or drive the display. The rest (ending a display,
/// asking for help, waiting for control) work without permissions.
fn needs_permissions(method: &str) -> bool {
    !matches!(
        method,
        "endDisplay" | "requestHelp" | "waitForControl" | "setDisposition" | "listWindows"
    ) && !method.starts_with("debug")
}

/// Fail with what the user has to do when a permission is missing, and put
/// the setup window in front of them.
fn permitted(cx: &mut AsyncApp) -> Result<(), ControlError> {
    let missing = crate::mac::permissions::status().missing();
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(shell) = crate::ui::shell::current() {
        let _ = shell.update(cx, |shell, cx| shell.setup_for_agent(cx));
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
            return Err(ControlError::ended(format!(
                "display {id} has ended; call create_display for a new one"
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
    // Read AeroSpace's state now: the user may have started, quit or
    // reconfigured it since Sidescreen last looked.
    let aerospace_takes = cx
        .background_executor()
        .spawn(async move {
            use crate::mac::aerospace;
            let takes = aerospace::ignoring().is_some_and(|i| i != aerospace::Ignoring::Yes);
            if takes && let Some(aerospace) = aerospace::AeroSpace::current() {
                aerospace.remember_before_first();
            }
            takes
        })
        .await;
    let started = Instant::now();
    let reply = core
        .update(cx, |c, cx| {
            let id = c.create((width, height), Some(owner), cx)?;
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
    for _ in 0..40 {
        if core.read_with(cx, |c, _| c.tucked.contains(&id)) {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    // Stock AeroSpace may show one of the user's workspaces on the new
    // display until the shell's guard moves it back; their windows must
    // not reach the agent.
    let tucked = started.elapsed().as_millis();
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
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
    cx: &mut AsyncApp,
) -> Option<std::sync::mpsc::Receiver<bool>> {
    cx.background_executor()
        .spawn(async move {
            let mut open = std::process::Command::new("/usr/bin/open");
            open.arg("-g");
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
            open.args([
                "--args",
                "-NSShowAppCentricOpenPanelInsteadOfUntitledFile",
                "NO",
                "-ApplePersistenceIgnoreState",
                "YES",
            ]);
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

/// What `launch_app` did.
struct Launched {
    window: Option<windows::Window>,
    was_running: bool,
    /// Why there is no window: `still_starting` (the app Sidescreen started
    /// has none yet; one it opens soon moves onto the display),
    /// `existing_window` (a running app opened the file in a window it
    /// already had), or `no_new_window` (a running app made no window).
    reason: Option<&'static str>,
}

/// How long a running app may take to make the window Sidescreen asked
/// for. A window it makes later may be the user's.
const ASKED_WINDOW_WAIT: Duration = Duration::from_millis(3000);
/// The same, counted from when `open` returns: it returns once the app has
/// the file.
const OPENED_WINDOW_WAIT: Duration = Duration::from_millis(2000);

async fn launch(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    target: Option<&str>,
    cx: &mut AsyncApp,
) -> Result<Launched, ControlError> {
    let path = app::resolve(name)
        .ok_or_else(|| ControlError::not_found(format!("no app named {name}")))?;
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
    let result = launch_now(core, id, name, &path, target, cx).await;
    core.update(cx, |c, _| c.launching.remove(&path));
    result
}

async fn launch_now(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    path: &std::path::Path,
    target: Option<&str>,
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
    let was_running = app::running_pid(&path).is_some();
    // New windows go straight onto the display, before the user sees them
    // or AeroSpace files them in the user's workspace.
    let bounds = cg::display_bounds(id);
    let screen = MainThreadMarker::new().and_then(|mtm| app::screen_name(id, mtm));
    // A running app may be the user's: take only the window Sidescreen
    // opens. A starting app's windows are all Sidescreen's.
    let limit = was_running.then_some(1);
    let catch_from = |pid: i32| crate::mac::catch::Catch::start(pid, id, screen.clone(), limit);
    let mut catch = app::running_pid(&path).and_then(catch_from);

    // A starting app opens the URL or file in its first window. A running
    // one opens a file in a window of its own, but a URL as a tab of its
    // front window, which may be the user's: it gets a new window first,
    // below, and the URL after.
    let is_url = target.is_some_and(|t| !t.starts_with('/'));
    let first = (!was_running || !is_url)
        .then(|| target.map(String::from))
        .flatten();
    // Opening a running app with nothing to open only reopens it, and
    // TextEdit and others answer a reopen with a window of their own, on
    // the user's screen, besides the one Command-N asks for below.
    let could_not_open = || ControlError::failed(format!("could not open {name}"));
    let opening = if !was_running || first.is_some() {
        Some(
            open_in_background(path.clone(), first, !was_running, cx)
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
        pid = app::running_pid(&path);
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
    // A starting app takes accessibility observers only once it has set
    // up; until then, try again, and move any window that beats it.
    for _ in 0..100 {
        if catch.is_some() {
            break;
        }
        catch = catch_from(pid);
        for w in windows::all() {
            if w.target.pid == pid && w.layer == 0 && !before.contains(&w.target.window_id) {
                let _ = ax::move_into(w.target, bounds);
            }
        }
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

    // A running app opens no window on launch; ask it for a new one.
    let asked_for_window = was_running && (target.is_none() || is_url);
    if asked_for_window && let Ok(chord) = KeyChord::parse("cmd+n") {
        crate::mac::input::post_key_to_pid(pid, chord);
    }
    let asked_at = Instant::now();

    // A running app may be the user's: its new window is the one the catch
    // took, never one the user opens meanwhile. A starting app's windows
    // are all Sidescreen's.
    let is_ours = |w: &windows::Window, taken: Option<u32>| {
        w.target.pid == pid
            && w.layer == 0
            && !before.contains(&w.target.window_id)
            && (!was_running || taken == Some(w.target.window_id))
    };
    let mut gave_up = false;
    for _ in 0..120 {
        cx.background_executor()
            .timer(Duration::from_millis(50))
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
            && waited_long_enough(asked_for_window, asked_at, opened.map(|(_, at)| at))
        {
            catch = None;
            gave_up = true;
            break;
        }
        if let Some(fresh) = windows::all().into_iter().find(|w| is_ours(w, taken)) {
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
                let opened =
                    match open_in_background(path.clone(), Some(target.to_string()), false, cx)
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
            }
            return Ok(Launched {
                window: Some(placed),
                was_running,
                reason: None,
            });
        }
    }
    drop(catch);
    log::debug!("launch: {name} made no window of Sidescreen's (gave up early: {gave_up})");
    // A slow launch can open its window after we stop waiting. Keep watching
    // so it does not stay on the user's screen. Only for an app Sidescreen
    // started: a running app's later windows may be the user's.
    let core = core.clone();
    if !was_running {
        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..200 {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if let Some(late) = windows::all().into_iter().find(|w| {
                    w.target.pid == pid && w.layer == 0 && !before.contains(&w.target.window_id)
                }) {
                    let _ = settle(&core, late.target, id, cx).await;
                    return;
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
            "{name} opened no new window, so the URL was not opened; call list_windows, then open it in a window on your display"
        )));
    }
    Ok(Launched {
        window: None,
        was_running,
        reason: Some(if !was_running {
            "still_starting"
        } else if asked_for_window {
            "no_new_window"
        } else {
            "existing_window"
        }),
    })
}

/// Whether a running app has had its time to make the window Sidescreen
/// asked for: counted from Command-N, or from when `open` returned.
fn waited_long_enough(
    asked_for_window: bool,
    asked_at: Instant,
    opened_at: Option<Instant>,
) -> bool {
    if asked_for_window {
        asked_at.elapsed() > ASKED_WINDOW_WAIT
    } else {
        opened_at.is_some_and(|at| at.elapsed() > OPENED_WINDOW_WAIT)
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
/// screenshot shows its result: no new frame for 120 ms, at most 1.5 s.
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
    while start.elapsed() < Duration::from_millis(1500) {
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        let now = streams.read_with(cx, |s, _| s.changes(id));
        if now != last {
            last = now;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= Duration::from_millis(120)
            && start.elapsed() >= Duration::from_millis(150)
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
            Err(ControlError::ended(format!("display {id} has ended")))
        }
        None => Err(ControlError::not_found(format!(
            "display {id} does not exist; call create_display again"
        ))),
        Some(d) if live && d.is_ended() => {
            Err(ControlError::ended(format!("display {id} has ended")))
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

/// A window an agent holds by its title bar, between left_mouse_down and
/// left_mouse_up.
#[derive(Clone, Copy)]
struct Grab {
    target: WindowTarget,
    frame: Rect,
    from: (f64, f64),
    /// The display's global bounds: the window stays inside them.
    area: Rect,
}

impl Grab {
    /// Move the window as far as the pointer moved from the press.
    async fn drag_to(self, to: (f64, f64), cx: &mut AsyncApp) -> bool {
        let frame = self
            .frame
            .offset(to.0 - self.from.0, to.1 - self.from.1)
            .kept_inside(&self.area);
        cx.background_executor()
            .spawn(async move { ax::set_frame(self.target, frame).is_ok() })
            .await
    }
}

/// The window on display `id` whose title bar is at a global point, held
/// there. A user window that only overhangs the display is never taken.
async fn title_bar_at(id: DisplayId, at: (f64, f64), cx: &mut AsyncApp) -> Option<Grab> {
    cx.background_executor()
        .spawn(async move {
            let area = cg::display_bounds(id);
            let window = windows::on_display(id)
                .into_iter()
                .map(|w| windows::Window {
                    bounds: w.bounds.offset(area.x, area.y),
                    ..w
                })
                .find(|w| w.bounds.contains(at))?;
            ax::on_title_bar(window.target, window.bounds, at).then_some(Grab {
                target: window.target,
                frame: window.bounds,
                from: at,
                area,
            })
        })
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
    // The app the user works in is theirs: a window it opens now, such as
    // their own cmd+N, stays with them.
    let active = objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier());
    let agent_areas: Vec<Rect> = core.read_with(cx, |c, _| {
        c.displays
            .list
            .iter()
            .filter(|d| !d.is_ended())
            .map(|d| cg::display_bounds(d.id))
            .collect()
    });
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
                        && Some(w.target.pid) != active
                        && !before.contains(&w.target.window_id)
                        && w.layer == 0
                        && !agent_areas.iter().any(|a| a.contains(w.bounds.mid()))
                })
                .map(|w| w.target)
                .collect::<Vec<_>>()
        })
        .await;
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
    if !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
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
    fn a_running_app_gets_a_short_turn() {
        // Command-N: 3 s from the key press.
        assert!(!waited_long_enough(true, ago(1000), None));
        assert!(waited_long_enough(true, ago(3500), None));
        // A file: 2 s from when `open` returned, however long that took.
        assert!(!waited_long_enough(false, ago(9000), None));
        assert!(!waited_long_enough(false, ago(9000), Some(ago(500))));
        assert!(waited_long_enough(false, ago(9000), Some(ago(2500))));
    }
}
