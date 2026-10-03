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
};
use base64::Engine;
use futures::FutureExt;
use gpui::{AsyncApp, Entity};
use objc2::MainThreadMarker;
use serde_json::{Map, Value, json};
use std::time::Duration;

type Params = Map<String, Value>;

pub async fn handle(
    core: Entity<Core>,
    connection: u64,
    method: &str,
    p: Params,
    cx: &mut AsyncApp,
) -> Reply {
    let p = Args(&p);
    let result: Result<Value, ControlError> = match method {
        "createDisplay" => create_display(&core, connection, &p, cx),
        "endDisplay" => {
            let id = display(&core, &p, false, cx)?;
            core.update(cx, |core, cx| core.end(id, EndReason::Finished, cx));
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
            let shot = cx
                .background_executor()
                .spawn(async move { screenshot::display(id, size, format) })
                .await
                .map_err(ControlError::failed)?;
            Ok(shot_json(&shot))
        }
        "click" => {
            let (id, point) = target(&core, &p, cx)?;
            let button = Button::parse(p.string("button").as_deref().unwrap_or("left"))
                .ok_or_else(|| {
                    ControlError::invalid(format!(
                        "unknown button {}",
                        p.string("button").unwrap_or_default()
                    ))
                })?;
            let modifiers = modifiers(&p)?;
            let forwarder = show_pointer(&core, id, point, true, cx);
            forwarder
                .click(point, button, p.int("count").unwrap_or(1), modifiers)
                .await;
            Ok(json!({}))
        }
        "move" => {
            let (id, point) = target(&core, &p, cx)?;
            show_pointer(&core, id, point, false, cx)
                .move_to(point)
                .await;
            Ok(json!({}))
        }
        "drag" => {
            let id = input_display(&core, &p, cx)?;
            let from = global(&core, id, p.require("fromX")?, p.require("fromY")?, cx)?;
            let to = global(&core, id, p.require("toX")?, p.require("toY")?, cx)?;
            let modifiers = modifiers(&p)?;
            let forwarder = show_pointer(&core, id, from, true, cx);
            forwarder.drag(from, to, modifiers).await;
            show_pointer(&core, id, to, false, cx);
            Ok(json!({}))
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
            forwarder.type_text(text, cg::display_bounds(id)).await;
            Ok(json!({}))
        }
        "key" => {
            let id = input_display(&core, &p, cx)?;
            let keys = p
                .string("keys")
                .ok_or_else(|| ControlError::invalid("keys is required"))?;
            let chord = KeyChord::parse(&keys).map_err(|e| ControlError::invalid(e.to_string()))?;
            let forwarder = core.update(cx, |c, _| c.forwarder(id));
            forwarder
                .press(chord, p.int("repeat").unwrap_or(1), cg::display_bounds(id))
                .await;
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
            let snapshot = ax::snapshot(window.target).ok_or_else(|| {
                ControlError::not_found(
                    "the window has no accessibility element; is Accessibility granted?",
                )
            })?;
            let count = snapshot.elements.len();
            core.update(cx, |c, _| {
                c.elements
                    .insert((id, window.target.window_id), snapshot.elements)
            });
            let target = window.target;
            let size = (window.bounds.width, window.bounds.height);
            let shot = cx
                .background_executor()
                .spawn(async move { screenshot::window(target, size) })
                .await
                .map_err(ControlError::failed)?;
            let mut result = shot_json(&shot);
            result["window"] = window.json();
            result["tree"] = json!(snapshot.tree);
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
                let list = c.elements.get(&(id, window.target.window_id));
                list.map(|l| l.get(index as usize).cloned())
            });
            let element = match element {
                None => {
                    return Err(ControlError::invalid(
                        "call get_window_state for this window first",
                    ));
                }
                Some(None) => {
                    return Err(ControlError::not_found(format!(
                        "no element {index}; call get_window_state again"
                    )));
                }
                Some(Some(element)) => element,
            };
            act(
                &core,
                id,
                &p.string("action").unwrap_or_else(|| "press".into()),
                &element,
                p.string("value"),
                cx,
            )
            .await?;
            Ok(json!({}))
        }
        "launchApp" => {
            let id = input_display(&core, &p, cx)?;
            let name = p
                .string("app")
                .ok_or_else(|| ControlError::invalid("app is required"))?;
            let window = launch(&core, id, &name, cx).await?;
            Ok(window.map_or(json!({}), |w| json!({ "window": w.json() })))
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
    result.map(|value| match value {
        Value::Object(map) => map,
        _ => Map::new(),
    })
}

// MARK: Methods

fn create_display(
    core: &Entity<Core>,
    connection: u64,
    p: &Args,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let session_key = p
        .string("sessionKey")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ControlError::invalid("sessionKey is required"))?;
    if core.read_with(cx, |c, _| c.closed.contains(&connection)) {
        return Err(ControlError::failed("the connection closed"));
    }
    if let Some(existing) = core.read_with(cx, |c, _| {
        c.displays
            .for_session(&session_key)
            .map(|d| describe(d, true))
    }) {
        return Ok(existing);
    }
    let width = p
        .number("width")
        .unwrap_or(DEFAULT_SIZE.0)
        .clamp(640.0, 2560.0)
        .round();
    let height = p
        .number("height")
        .unwrap_or(DEFAULT_SIZE.1)
        .clamp(480.0, 1600.0)
        .round();
    let owner = Owner {
        session_key,
        client: p.string("client").unwrap_or_else(|| "Agent".into()),
        title: p.string("title").unwrap_or_else(|| "Untitled".into()),
        connection,
    };
    core.update(cx, |c, cx| {
        let id = c.create((width, height), Some(owner), cx)?;
        c.displays.get(id).map(|d| describe(d, false))
    })
    .ok_or_else(|| ControlError::failed("macOS refused to create a virtual display"))
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
    let center = ax::frame(element)
        .map(|f| f.mid())
        .filter(|c| bounds.contains(*c));
    let perform = |name: &str, cx: &mut AsyncApp| {
        if let Some(c) = center {
            show_pointer(core, id, c, true, cx);
        }
        ax::perform(element, name)
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
            if !perform("AXPress", cx) {
                click(Button::Left, 1, cx).await?;
            }
        }
        "click" => click(Button::Left, 1, cx).await?,
        "double_click" => click(Button::Left, 2, cx).await?,
        "right_click" => click(Button::Right, 1, cx).await?,
        "show_menu" => {
            if !perform("AXShowMenu", cx) {
                click(Button::Right, 1, cx).await?;
            }
        }
        "focus" => {
            if !ax::set_attribute(
                element,
                "AXFocused",
                objc2_core_foundation::CFBoolean::new(true),
            ) {
                return Err(ControlError::failed("the element cannot take focus"));
            }
        }
        "set_value" => {
            let value =
                value.ok_or_else(|| ControlError::invalid("value is required for set_value"))?;
            if !ax::set_attribute(
                element,
                "AXValue",
                &objc2_core_foundation::CFString::from_str(&value),
            ) {
                return Err(ControlError::failed(
                    "the element's value cannot be set; focus it and type instead",
                ));
            }
        }
        "scroll_into_view" => {
            if !perform("AXScrollToVisible", cx) {
                return Err(ControlError::failed("the element cannot scroll into view"));
            }
        }
        other => return Err(ControlError::invalid(format!("unknown action {other}"))),
    }
    Ok(())
}

/// Open an app in the background and place its new window on a display.
async fn launch(
    core: &Entity<Core>,
    id: DisplayId,
    name: &str,
    cx: &mut AsyncApp,
) -> Result<Option<windows::Window>, ControlError> {
    let path = app::resolve(name)
        .ok_or_else(|| ControlError::not_found(format!("no app named {name}")))?;
    let before: std::collections::HashSet<u32> =
        windows::all().iter().map(|w| w.target.window_id).collect();
    let was_running = app::running_pid(&path).is_some();

    // `open -g` launches without activating, like OpenConfiguration.activates = false.
    let launch_path = path.clone();
    let opened = cx
        .background_executor()
        .spawn(async move {
            std::process::Command::new("/usr/bin/open")
                .arg("-g")
                .arg(&launch_path)
                .status()
        })
        .await;
    if !opened.is_ok_and(|s| s.success()) {
        return Err(ControlError::failed(format!("could not open {name}")));
    }
    let mut pid = None;
    for _ in 0..40 {
        pid = app::running_pid(&path);
        if pid.is_some() {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    let pid = pid.ok_or_else(|| ControlError::failed(format!("{name} did not start")))?;

    // A running app opens no window on launch; ask it for a new one.
    if was_running && let Ok(chord) = KeyChord::parse("cmd+n") {
        crate::mac::input::post_key_to_pid(pid, chord);
    }

    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(150))
            .await;
        if let Some(fresh) = windows::all()
            .into_iter()
            .find(|w| w.target.pid == pid && !before.contains(&w.target.window_id))
        {
            return settle(core, fresh.target, id, cx).await.map(Some);
        }
    }
    // A slow launch can open its window after we stop waiting. Keep watching
    // so it does not stay on the user's screen.
    let core = core.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        for _ in 0..200 {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            if let Some(late) = windows::all()
                .into_iter()
                .find(|w| w.target.pid == pid && !before.contains(&w.target.window_id))
            {
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
    Ok(None)
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
    let mut last_error = None;
    for _ in 0..12 {
        if !takes_windows(core, id, cx) {
            return Err(ended());
        }
        if let Err(e) = place(target, id, cx).await {
            last_error = Some(e);
        }
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
        if let Some(placed) = windows::on_display(id)
            .into_iter()
            .find(|w| w.target == target)
        {
            return Ok(placed);
        }
    }
    Err(ControlError::failed(format!(
        "the window opened but could not be placed on the display{}",
        last_error.map(|e| format!(": {e}")).unwrap_or_default()
    )))
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
    Ok(id)
}

fn target(
    core: &Entity<Core>,
    p: &Args,
    cx: &mut AsyncApp,
) -> Result<(DisplayId, (f64, f64)), ControlError> {
    let id = input_display(core, p, cx)?;
    Ok((id, global(core, id, p.require("x")?, p.require("y")?, cx)?))
}

/// Display point → global point.
fn global(
    core: &Entity<Core>,
    id: DisplayId,
    x: f64,
    y: f64,
    cx: &mut AsyncApp,
) -> Result<(f64, f64), ControlError> {
    let (w, h) = core
        .read_with(cx, |c, _| c.displays.get(id).map(|d| (d.width, d.height)))
        .unwrap_or(DEFAULT_SIZE);
    if !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
        return Err(ControlError::invalid(format!(
            "({}, {}) is outside the {}×{} display",
            x as i64, y as i64, w as i64, h as i64
        )));
    }
    let bounds: Rect = cg::display_bounds(id);
    Ok((bounds.x + x, bounds.y + y))
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
