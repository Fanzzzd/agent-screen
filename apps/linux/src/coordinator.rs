//! Request coordinator for `apps/linux`: binds `agent_screen_model::model::Displays`
//! to the Linux `Display`, `Access`, and `WebPreviewServer` implementations.

use crate::clipboard_wl::ClipboardBridge;
use agent_screen_app_core::devtools_pipe;
use agent_screen_app_core::disconnect_grace::{ConnectionSessions, DISCONNECT_GRACE_MS};
use agent_screen_app_core::ports::{
    Access, ClipboardPayload, ClipboardSync, Display, DisplayManager, DisplaySpec,
    ElementActionReq, ImageFormat, InputOp, LaunchRequest, MouseButton, Secrecy, SnapshotOpts,
    WindowInfo,
};
use agent_screen_app_core::server::{ControlError, Reply, RequestHandler};
use agent_screen_app_core::web_preview::WebPreviewServer;
use agent_screen_model::keychord::KeyChord;
use agent_screen_model::model::{
    ControlAsk, DisplayId, Displays, Disposition, EndReason, Origin, Owner, Rect, Wait, help_steps,
    lowest_free_serial, shot_scale,
};
use agent_screen_model::request::{Answer, HelpAsked, Kind, Resolution, Woken};
use agent_screen_model::session::Orphans;
use agent_screen_model::tree::Scope as TreeScope;
use agent_screen_model::watch;
use base64::Engine as _;
use futures::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const DEFAULT_SIZE: (f64, f64) = (1280.0, 800.0);

struct State {
    displays: Displays<Arc<dyn Display>>,
    /// Surviving displays recovered on daemon startup that await reattachment
    /// by their original `sessionKey` or `adoptGroups` (or user review).
    surviving: BTreeMap<DisplayId, SurvivingEntry>,
    conn_sessions: ConnectionSessions,
    conn_last_session: HashMap<u64, String>,
    last_agent_client: Option<String>,
}

struct SurvivingEntry {
    spec: DisplaySpec,
    started_pids: Vec<i32>,
    display: Arc<dyn Display>,
}

pub struct Coordinator {
    manager: Arc<dyn DisplayManager>,
    access: Option<Arc<dyn Access>>,
    clipboard_bridge: ClipboardBridge,
    web: Arc<WebPreviewServer>,
    quit: Arc<Notify>,
    started_at: Instant,
    state: Mutex<State>,
    waits: Mutex<HashMap<(u64, u64), tokio::sync::oneshot::Sender<()>>>,
    create_lock: tokio::sync::Mutex<()>,
}

impl Coordinator {
    pub async fn new(
        manager: Arc<dyn DisplayManager>,
        access: Option<Arc<dyn Access>>,
        web: Arc<WebPreviewServer>,
        quit: Arc<Notify>,
    ) -> Arc<Self> {
        let recovered = manager.reattach_surviving().await;
        let mut surviving = BTreeMap::new();
        for rec in recovered {
            surviving.insert(
                rec.spec.id,
                SurvivingEntry {
                    spec: rec.spec,
                    started_pids: rec.started_pids,
                    display: rec.display,
                },
            );
        }

        let coord = Arc::new(Self {
            manager,
            access,
            clipboard_bridge: ClipboardBridge::new(),
            web,
            quit,
            started_at: Instant::now(),
            state: Mutex::new(State {
                displays: Displays::default(),
                surviving,
                conn_sessions: ConnectionSessions::new(),
                conn_last_session: HashMap::new(),
                last_agent_client: None,
            }),
            waits: Mutex::new(HashMap::new()),
            create_lock: tokio::sync::Mutex::new(()),
        });

        let weak = Arc::downgrade(&coord);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let Some(c) = weak.upgrade() else {
                    break;
                };
                c.tick_disconnect_grace().await;
            }
        });

        coord
    }

    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    async fn tick_disconnect_grace(&self) {
        let now = self.now_ms();
        let expired: Vec<(DisplayId, Arc<dyn Display>)> = {
            let mut st = self.state.lock().unwrap();
            let ids = st.displays.expire_disconnected(now, DISCONNECT_GRACE_MS);
            ids.into_iter()
                .filter_map(|id| st.displays.get(id).map(|d| (id, d.handle.clone())))
                .collect()
        };
        for (id, handle) in expired {
            self.finalize_ended_display(id, handle).await;
        }
    }

    async fn finalize_ended_display(&self, id: DisplayId, handle: Arc<dyn Display>) {
        // ADR 0019: expire Web Preview token as soon as the session stops using the display.
        self.web.expire_display(id);
        self.clipboard_bridge.disconnect(id);
        // ADR 0004 & ADR 0020: an empty ended display closes immediately; one
        // with open windows stays alive as a Review Display.
        let windows = handle.list_windows().await.unwrap_or_default();
        if windows.is_empty() {
            {
                let mut st = self.state.lock().unwrap();
                let _ = st.displays.remove(id);
            }
            if let Some(ax) = &self.access {
                ax.remove_display(id);
            }
            self.clipboard_bridge.remove_display(id);
            let _ = handle.shutdown().await;
        }
    }

    pub async fn connection_closed(&self, connection: u64) {
        {
            let mut waits = self.waits.lock().unwrap();
            let keys: Vec<(u64, u64)> = waits
                .keys()
                .filter(|&&(c, _)| c == connection)
                .copied()
                .collect();
            for k in keys {
                if let Some(tx) = waits.remove(&k) {
                    let _ = tx.send(());
                }
            }
        }
        let now = self.now_ms();
        let mut st = self.state.lock().unwrap();
        st.conn_last_session.remove(&connection);
        let _ = st.conn_sessions.on_closed(connection);
        st.displays.connection_disconnected(connection, now);
    }

    pub async fn shutdown_all(&self) {
        let handles: Vec<Arc<dyn Display>> = {
            let mut st = self.state.lock().unwrap();
            let ids: Vec<DisplayId> = st.displays.of_all_ids();
            let mut list: Vec<Arc<dyn Display>> = ids
                .into_iter()
                .filter_map(|id| st.displays.remove(id).map(|d| d.handle))
                .collect();
            for (_, entry) in std::mem::take(&mut st.surviving) {
                list.push(entry.display);
            }
            list
        };
        for h in handles {
            let _ = h.shutdown().await;
        }
    }

    pub async fn dispatch(
        &self,
        connection: u64,
        method: &str,
        params: Map<String, Value>,
    ) -> Result<Map<String, Value>, ControlError> {
        let p = Params(&params);
        let is_debug = method.starts_with("debug");
        if is_debug && !debug_methods_enabled() {
            return Err(ControlError::invalid(format!("unknown method {method}")));
        }

        let session_key = p.string("sessionKey").or_else(|| {
            self.state
                .lock()
                .unwrap()
                .conn_last_session
                .get(&connection)
                .cloned()
        });

        if !is_debug
            && method != "agentReady"
            && let Some(ref key) = session_key
        {
            let now = self.now_ms();
            let mut st = self.state.lock().unwrap();
            st.conn_sessions.bind(connection, key.clone());
            st.conn_last_session.insert(connection, key.clone());
            st.displays.rebind_disconnected(key, connection, now);
            st.displays.sessions.note_call_start(key, now);
        }

        let outcome = self
            .handle_method(connection, method, &p, session_key.as_deref())
            .await;

        if !is_debug
            && method != "agentReady"
            && let Some(ref key) = session_key
        {
            let now = self.now_ms();
            self.state
                .lock()
                .unwrap()
                .displays
                .sessions
                .note_call_end(key, now);
        }

        outcome.map(|v| match v {
            Value::Object(map) => map,
            _ => Map::new(),
        })
    }

    async fn handle_method(
        &self,
        connection: u64,
        method: &str,
        p: &Params<'_>,
        session_key: Option<&str>,
    ) -> Result<Value, ControlError> {
        match method {
            "agentReady" => {
                let client = p.string("client").unwrap_or_else(|| "Agent".into());
                self.state.lock().unwrap().last_agent_client = Some(client);
                Ok(self.manager.capabilities().to_json())
            }
            "createDisplay" => self.create_display(connection, p).await,
            "endDisplay" => {
                let (id, handle) = self.require_display(p, session_key, false, true)?;
                {
                    let mut st = self.state.lock().unwrap();
                    st.displays.end(id, EndReason::Finished);
                }
                self.finalize_ended_display(id, handle).await;
                Ok(json!({}))
            }
            "settle" => {
                let (_, handle) = self.require_display(p, session_key, false, false)?;
                handle.settle().await?;
                Ok(json!({}))
            }
            "screenshot" => {
                let (id, handle) = self.require_display(p, session_key, false, false)?;
                let m = handle.metrics();
                let size_f = (m.width as f64, m.height as f64);
                let scale = shot_scale(size_f);
                let base_pixels = (
                    (size_f.0 * scale).round() as u32,
                    (size_f.1 * scale).round() as u32,
                );
                let format = if p.string("format").as_deref() == Some("png") {
                    ImageFormat::Png
                } else {
                    ImageFormat::Jpeg
                };
                let region_points = match p.0.get("region").and_then(Value::as_array) {
                    Some(arr) => {
                        let nums: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
                        let [x0, y0, x1, y1] = nums[..] else {
                            return Err(ControlError::invalid("region is [x0, y0, x1, y1]"));
                        };
                        let (w, h) = (base_pixels.0 as f64, base_pixels.1 as f64);
                        if !(0.0 <= x0 && x0 < x1 && x1 <= w && 0.0 <= y0 && y0 < y1 && y1 <= h) {
                            return Err(ControlError::invalid(format!(
                                "region must lie within the {}×{} screenshot, x0 < x1 and y0 < y1",
                                base_pixels.0, base_pixels.1
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
                let target_pixels = match region_points {
                    Some(r) => {
                        let zoom = (2.0f64).min(1568.0 / r.width.max(r.height));
                        (
                            (r.width * zoom).round().max(1.0) as u32,
                            (r.height * zoom).round().max(1.0) as u32,
                        )
                    }
                    None => base_pixels,
                };
                let shot = handle
                    .screenshot(SnapshotOpts {
                        format,
                        region_points,
                        target_pixels,
                        settle: p.bool("settle"),
                    })
                    .await?;
                Ok(json!({
                    "displayId": id,
                    "data": base64::engine::general_purpose::STANDARD.encode(&shot.data),
                    "mimeType": shot.mime_type,
                    "width": shot.width,
                    "height": shot.height,
                    "scale": scale,
                }))
            }
            "click" => {
                let (id, handle) = self.require_display(p, session_key, true, false)?;
                let pt = self.display_point(&handle, p.require_num("x")?, p.require_num("y")?)?;
                let button = MouseButton::parse(p.string("button").as_deref().unwrap_or("left"))
                    .ok_or_else(|| ControlError::invalid("unknown button"))?;
                let count = p.int("count").unwrap_or(1).clamp(1, 3) as u32;
                let modifiers = p.modifiers()?;
                self.note_pointer(id, pt, true);
                let out = handle
                    .act_input(InputOp::Click {
                        point: pt,
                        button,
                        count,
                        modifiers,
                    })
                    .await?;
                let mut reply = json!({});
                if let Some(wid) = out.window_id {
                    reply["windowId"] = json!(wid);
                }
                if let Some(route) = out.route {
                    reply["route"] = json!(route);
                }
                if let Some(note) = out.note {
                    reply["note"] = json!(note);
                }
                Ok(reply)
            }
            "move" => {
                let (id, handle) = self.require_display(p, session_key, true, false)?;
                let pt = self.display_point(&handle, p.require_num("x")?, p.require_num("y")?)?;
                self.note_pointer(id, pt, false);
                handle.act_input(InputOp::Move { point: pt }).await?;
                Ok(json!({}))
            }
            "mouseDown" | "mouseUp" => {
                let (id, handle) = self.require_display(p, session_key, true, false)?;
                let down = method == "mouseDown";
                let pt = match (p.num("x"), p.num("y")) {
                    (Some(x), Some(y)) => {
                        let resolved = self.display_point(&handle, x, y)?;
                        self.note_pointer(id, resolved, down);
                        Some(resolved)
                    }
                    _ => None,
                };
                let modifiers = p.modifiers()?;
                handle
                    .act_input(InputOp::MouseButton {
                        point: pt,
                        button: MouseButton::Left,
                        down,
                        modifiers,
                    })
                    .await?;
                Ok(json!({}))
            }
            "drag" => {
                let (id, handle) = self.require_display(p, session_key, true, false)?;
                let from =
                    self.display_point(&handle, p.require_num("fromX")?, p.require_num("fromY")?)?;
                let to =
                    self.display_point(&handle, p.require_num("toX")?, p.require_num("toY")?)?;
                let modifiers = p.modifiers()?;
                self.note_pointer(id, to, false);

                // Route drags inside piped Chromium/Electron pages over DevTools pipe (ADR 0010).
                if let Ok(wins) = handle.list_windows().await
                    && let Some(hit) = wins.into_iter().find(|w| w.bounds.contains(from))
                {
                    let web_area = match &self.access {
                        Some(ax) => ax.web_area_at(handle.as_ref(), &hit, from).await,
                        None => None,
                    };
                    let target = hit.target;
                    let bounds = hit.bounds;
                    let cdp_drag = tokio::task::spawn_blocking(move || {
                        devtools_pipe::try_drag_via_pipe(
                            id, target, bounds, web_area, 1.0, from, to, modifiers,
                        )
                    })
                    .await
                    .map_err(|e| ControlError::failed(e.to_string()))?;
                    match cdp_drag {
                        Ok(true) => return Ok(json!({ "route": "devtools" })),
                        Ok(false) => {}
                        Err(e) => return Err(ControlError::failed(e)),
                    }
                }

                let out = handle
                    .act_input(InputOp::Drag {
                        from,
                        to,
                        modifiers,
                    })
                    .await?;
                Ok(json!({
                    "route": out.route.unwrap_or("wayland_seat")
                }))
            }
            "scroll" => {
                let (id, handle) = self.require_display(p, session_key, true, false)?;
                let pt = self.display_point(&handle, p.require_num("x")?, p.require_num("y")?)?;
                let dx = p.num("deltaX").unwrap_or(0.0);
                let dy = p.num("deltaY").unwrap_or(0.0);
                let modifiers = p.modifiers()?;
                self.note_pointer(id, pt, false);
                handle
                    .act_input(InputOp::Scroll {
                        point: pt,
                        delta: (dx, dy),
                        modifiers,
                    })
                    .await?;
                Ok(json!({}))
            }
            "cursorPosition" => {
                let (id, handle) = self.require_display(p, session_key, false, false)?;
                let m = handle.metrics();
                let scale = shot_scale((m.width as f64, m.height as f64));
                let ptr = self
                    .state
                    .lock()
                    .unwrap()
                    .displays
                    .get(id)
                    .and_then(|d| d.pointer);
                Ok(match ptr {
                    Some(pt) => json!({
                        "x": (pt.x * scale).round(),
                        "y": (pt.y * scale).round(),
                    }),
                    None => json!({}),
                })
            }
            "type" => {
                let (_, handle) = self.require_display(p, session_key, true, false)?;
                let text = p
                    .string("text")
                    .ok_or_else(|| ControlError::invalid("text is required"))?;
                handle.act_input(InputOp::TypeText { text }).await?;
                Ok(json!({}))
            }
            "key" => {
                let (_, handle) = self.require_display(p, session_key, true, false)?;
                let keys = p
                    .string("keys")
                    .ok_or_else(|| ControlError::invalid("keys is required"))?;
                let chords = KeyChord::parse_sequence(&keys)
                    .map_err(|e| ControlError::invalid(e.to_string()))?;
                let repeat = p.int("repeat").unwrap_or(1).max(1) as u32;
                handle
                    .act_input(InputOp::KeySequence { chords, repeat })
                    .await?;
                Ok(json!({}))
            }
            "holdKey" => {
                let (_, handle) = self.require_display(p, session_key, true, false)?;
                let keys = p
                    .string("keys")
                    .ok_or_else(|| ControlError::invalid("keys is required"))?;
                let chord =
                    KeyChord::parse(&keys).map_err(|e| ControlError::invalid(e.to_string()))?;
                let secs = p.num("seconds").unwrap_or(1.0).clamp(0.05, 10.0);
                handle
                    .act_input(InputOp::HoldKey {
                        chord,
                        duration_ms: (secs * 1000.0).round() as u64,
                    })
                    .await?;
                Ok(json!({}))
            }
            "launchApp" => {
                let (id, handle) = self.require_display(p, session_key, true, false)?;
                let app = p
                    .string("app")
                    .ok_or_else(|| ControlError::invalid("app is required"))?;
                let arguments = match p.0.get("arguments") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(arr)) => arr
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(String::from)
                                .ok_or_else(|| ControlError::invalid("arguments must be strings"))
                        })
                        .collect::<Result<_, _>>()?,
                    Some(_) => return Err(ControlError::invalid("arguments must be an array")),
                };
                let environment = match p.0.get("environment") {
                    None | Some(Value::Null) => BTreeMap::new(),
                    Some(Value::Object(obj)) => obj
                        .iter()
                        .map(|(k, v)| {
                            v.as_str()
                                .map(|s| (k.clone(), s.to_string()))
                                .ok_or_else(|| {
                                    ControlError::invalid("environment values must be strings")
                                })
                        })
                        .collect::<Result<_, _>>()?,
                    Some(_) => return Err(ControlError::invalid("environment must be an object")),
                };
                let launched = handle
                    .launch_app(LaunchRequest {
                        app,
                        open: p.string("open"),
                        arguments,
                        environment,
                    })
                    .await?;
                if let Some(pid) = launched.pid {
                    let mut st = self.state.lock().unwrap();
                    if let Some(d) = st.displays.get_mut(id) {
                        d.started.insert(pid);
                        if let Some(ref win) = launched.window {
                            d.claim(win.target, Origin::Launched, None);
                        }
                    }
                }
                let m = handle.metrics();
                let scale = shot_scale((m.width as f64, m.height as f64));
                let mut reply = json!({
                    "wasRunning": launched.was_running,
                    "reason": launched.reason,
                });
                if let Some(win) = launched.window {
                    reply["window"] = window_json(&win, scale);
                }
                if let Some(pid) = launched.pid {
                    reply["pid"] = json!(pid);
                }
                Ok(reply)
            }
            "listWindows" => {
                let (id, handle) = self.require_display(p, session_key, false, false)?;
                if p.bool("strays") {
                    return Ok(json!({ "windows": [] }));
                }
                let m = handle.metrics();
                let scale = shot_scale((m.width as f64, m.height as f64));
                let wins = handle.list_windows().await?;
                let dispositions: HashMap<u32, Option<Disposition>> = {
                    let st = self.state.lock().unwrap();
                    st.displays
                        .get(id)
                        .map(|d| {
                            d.records
                                .iter()
                                .map(|(&wid, r)| (wid, r.disposition))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let list: Vec<Value> = wins
                    .iter()
                    .map(|w| {
                        let mut j = window_json(w, scale);
                        j["origin"] = json!("launched");
                        j["disposition"] = json!(
                            dispositions
                                .get(&w.target.window_id)
                                .copied()
                                .flatten()
                                .map(Disposition::as_str)
                        );
                        j
                    })
                    .collect();
                Ok(json!({ "windows": list, "scale": scale }))
            }
            "placeWindow" => {
                let (_, handle) = self.require_display(p, session_key, true, false)?;
                let wanted = p.int("windowId");
                let app = p.string("app").filter(|a| !a.is_empty());
                if wanted.is_none() && app.is_none() {
                    return Err(ControlError::invalid("window_id or app is required"));
                }
                let wins = handle.list_windows().await?;
                let matched = if let Some(wid) = wanted {
                    wins.into_iter().find(|w| w.target.window_id as i64 == wid)
                } else if let Some(ref name) = app {
                    let q = name.to_ascii_lowercase();
                    wins.into_iter().find(|w| {
                        w.app.to_ascii_lowercase().contains(&q)
                            || w.title.to_ascii_lowercase().contains(&q)
                    })
                } else {
                    None
                };
                let Some(win) = matched else {
                    return Err(ControlError::not_found(
                        "no matching window on this display; on Linux windows cannot move from the host desktop or across displays (use launch_app to start apps on this display)",
                    ));
                };
                let m = handle.metrics();
                let scale = shot_scale((m.width as f64, m.height as f64));
                let ax = p.num("x").map(|v| v / scale);
                let ay = p.num("y").map(|v| v / scale);
                let aw = p.num("width").map(|v| v / scale);
                let ah = p.num("height").map(|v| v / scale);
                if aw.is_some_and(|w| w <= 0.0) || ah.is_some_and(|h| h <= 0.0) {
                    return Err(ControlError::invalid("width and height must be above 0"));
                }
                let frame_req = if ax.is_some() || ay.is_some() || aw.is_some() || ah.is_some() {
                    Some((ax, ay, aw, ah))
                } else {
                    None
                };
                let outcome = handle.place_window(win.target, frame_req).await?;
                Ok(json!({
                    "window": window_json(&outcome.window, scale),
                    "frameConstrained": outcome.constrained,
                }))
            }
            "setDisposition" => {
                let (id, handle) = self.require_display(p, session_key, false, false)?;
                let wid = p
                    .int("windowId")
                    .ok_or_else(|| ControlError::invalid("windowId is required"))?;
                let disp = p
                    .string("disposition")
                    .and_then(|s| Disposition::parse(&s))
                    .ok_or_else(|| ControlError::invalid("disposition must be keep or close"))?;
                let win = handle
                    .list_windows()
                    .await?
                    .into_iter()
                    .find(|w| w.target.window_id as i64 == wid)
                    .ok_or_else(|| {
                        ControlError::not_found(format!("no window {wid} on display {id}"))
                    })?;
                {
                    let mut st = self.state.lock().unwrap();
                    if let Some(d) = st.displays.get_mut(id) {
                        d.set_disposition(win.target, disp);
                    }
                }
                Ok(json!({ "handedOver": false }))
            }
            "requestHelp" => {
                let (id, _) = self.require_display(p, session_key, true, false)?;
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
                let steps = help_steps(steps).map_err(ControlError::invalid)?;
                let answers = p.bool("answers");
                let asked = self
                    .state
                    .lock()
                    .unwrap()
                    .displays
                    .request_help(id, reason, steps, answers);
                Ok(match asked {
                    Some(HelpAsked::Answered(woken)) => json!({ "answered": woken.to_json(false) }),
                    Some(HelpAsked::Opened(req_id)) => json!({ "requestId": req_id }),
                    None => json!({ "requestId": null }),
                })
            }
            "withdrawHelp" => {
                let (id, _) = self.require_display(p, session_key, false, false)?;
                let withdrawn = self.state.lock().unwrap().displays.withdraw_help(id);
                Ok(json!({ "withdrawn": withdrawn }))
            }
            "answerHelp" => {
                let (id, _) = self.require_display(p, session_key, true, false)?;
                let _words = p
                    .string("userInstruction")
                    .filter(|w| !w.trim().is_empty())
                    .ok_or_else(|| ControlError::invalid("userInstruction is required"))?;
                let mut st = self.state.lock().unwrap();
                st.displays
                    .answer(id, Kind::Help, None, Answer::Continue)
                    .map_err(|why| ControlError::failed(why.message()))?;
                st.displays.forget_unheard(id);
                Ok(json!({ "answered": true }))
            }
            "requestControl" => {
                if let Some(_words) = p.string("userInstruction") {
                    let (id, _) = self.require_display(p, session_key, false, false)?;
                    let to = p.string("control").unwrap_or_else(|| "agent".into());
                    let mut st = self.state.lock().unwrap();
                    let changed = if to == "user" {
                        !st.displays.begin_takeover(id).is_empty()
                    } else {
                        !st.displays.end_takeover(id).is_empty()
                    };
                    return Ok(json!({ "changed": changed }));
                }
                let (id, _) = self.require_display(p, session_key, false, false)?;
                let reason = p
                    .string("reason")
                    .filter(|r| !r.is_empty())
                    .ok_or_else(|| ControlError::invalid("reason is required"))?;
                let asked = self
                    .state
                    .lock()
                    .unwrap()
                    .displays
                    .request_control(id, reason);
                Ok(json!({
                    "requested": asked != ControlAsk::NotNeeded,
                    "alreadyAsked": asked == ControlAsk::AlreadyAsked,
                }))
            }
            "withdrawControlRequest" => {
                let (id, _) = self.require_display(p, session_key, false, false)?;
                let withdrawn = self
                    .state
                    .lock()
                    .unwrap()
                    .displays
                    .withdraw_control_request(id);
                Ok(json!({ "withdrawn": withdrawn }))
            }
            "waitForControl" => {
                let (id, _) = self.require_display(p, session_key, false, true)?;
                let secs = p.num("timeoutSeconds").unwrap_or(100.0).clamp(1.0, 600.0);
                let wait = self.state.lock().unwrap().displays.wait_for_control(id);
                let woken = match wait {
                    Wait::Now(w) => w,
                    Wait::Later(rx) => {
                        match tokio::time::timeout(Duration::from_secs_f64(secs), rx).await {
                            Ok(Ok(w)) => w,
                            Ok(Err(_)) => Woken::of(None, Resolution::Ended),
                            Err(_) => {
                                let open = self.state.lock().unwrap().displays.waiting_on(id);
                                Woken::of(open, Resolution::TimedOut)
                            }
                        }
                    }
                };
                let closed = woken.resolution == Resolution::Ended && {
                    let st = self.state.lock().unwrap();
                    st.displays.closed_by_user.contains(&id)
                        || st
                            .displays
                            .get(id)
                            .is_some_and(|d| d.ended == Some(EndReason::ClosedByUser))
                };
                Ok(woken.to_json(closed))
            }
            "windowState" => {
                let (_, handle) = self.require_display(p, session_key, false, false)?;
                let access = self
                    .access
                    .as_ref()
                    .ok_or_else(|| ControlError::failed("accessibility port is not initialized"))?;
                let window_id = p
                    .int("windowId")
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| ControlError::invalid("windowId is required"))?;
                let scope = TreeScope {
                    root: p.int("root").and_then(|r| usize::try_from(r).ok()),
                    max_depth: p.int("maxDepth").and_then(|d| usize::try_from(d).ok()),
                };
                let snap = access
                    .window_state(handle.as_ref(), window_id, scope)
                    .await
                    .map_err(|e| {
                        if e.code == "root_gone" {
                            ControlError::invalid(e.message)
                        } else {
                            ControlError::from(e)
                        }
                    })?;
                let m = handle.metrics();
                let scale = shot_scale((m.width as f64, m.height as f64));
                Ok(json!({
                    "window": window_json(&snap.window, scale),
                    "tree": snap.tree,
                    "elements": snap.elements,
                    "truncated": snap.truncated,
                    "depthLimited": snap.depth_limited,
                    "root": snap.root,
                    "data": base64::engine::general_purpose::STANDARD.encode(&snap.image.data),
                    "mimeType": snap.image.mime_type,
                }))
            }
            "waitForChange" => self.wait_for_change(connection, p, session_key).await,
            "cancelWaitForChange" => {
                let wait_id = p
                    .int("waitId")
                    .and_then(|v| u64::try_from(v).ok())
                    .ok_or_else(|| ControlError::invalid("waitId is required"))?;
                let cancelled = self
                    .waits
                    .lock()
                    .unwrap()
                    .remove(&(connection, wait_id))
                    .is_some_and(|tx| tx.send(()).is_ok());
                Ok(json!({ "cancelled": cancelled }))
            }
            "elementAction" => {
                let (_, handle) = self.require_display(p, session_key, true, false)?;
                let access = self
                    .access
                    .as_ref()
                    .ok_or_else(|| ControlError::failed("accessibility port is not initialized"))?;
                let window_id = p
                    .int("windowId")
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| ControlError::invalid("windowId is required"))?;
                let element = p
                    .int("element")
                    .and_then(|v| usize::try_from(v).ok())
                    .ok_or_else(|| ControlError::invalid("element is required"))?;
                let action = p
                    .string("action")
                    .ok_or_else(|| ControlError::invalid("action is required"))?;
                let scope = TreeScope {
                    root: p.int("root").and_then(|r| usize::try_from(r).ok()),
                    max_depth: p.int("maxDepth").and_then(|d| usize::try_from(d).ok()),
                };
                let reply = access
                    .act_on_element(
                        handle.as_ref(),
                        ElementActionReq {
                            window_id,
                            element,
                            action,
                            value: p.string("value"),
                            scope,
                        },
                    )
                    .await?;
                let mut out = json!({});
                if let Some(t) = reply.text {
                    out["text"] = json!(t);
                }
                if let Some(s) = reply.selected {
                    out["selected"] = json!(s);
                }
                if let Some(m) = reply.menu {
                    out["menu"] = json!(m);
                }
                if let Some(n) = reply.note {
                    out["note"] = json!(n);
                }
                Ok(out)
            }
            // Debug hooks
            "debugWebPreview" => Ok(self.web.debug_status()),
            "debugAgentReady" => Ok(json!({
                "agent": self.state.lock().unwrap().last_agent_client
            })),
            "debugTakeover" => {
                let id = p.require_display_id()?;
                let active = p.bool("active");
                {
                    let mut st = self.state.lock().unwrap();
                    if active {
                        st.displays.begin_takeover(id);
                    } else {
                        st.displays.end_takeover(id);
                    }
                }
                if active {
                    if p.0.contains_key("viewerFocused") {
                        self.clipboard_bridge
                            .set_viewer_focused(id, p.bool("viewerFocused"));
                    }
                } else {
                    self.clipboard_bridge.disconnect(id);
                }
                Ok(json!({ "active": active }))
            }
            "debugAnswerHelp" => {
                let id = p.require_display_id()?;
                let ans = p.string("answer").unwrap_or_else(|| "continue".into());
                let mut st = self.state.lock().unwrap();
                match ans.as_str() {
                    "takeOver" => {
                        st.displays.begin_takeover(id);
                    }
                    "reply" => {
                        let text = p.string("text").unwrap_or_default();
                        st.displays
                            .answer(id, Kind::Help, None, Answer::Reply(text))
                            .map_err(|e| ControlError::failed(e.message()))?;
                    }
                    _ => {
                        st.displays
                            .answer(id, Kind::Help, None, Answer::Continue)
                            .map_err(|e| ControlError::failed(e.message()))?;
                    }
                }
                Ok(json!({ "answered": true }))
            }
            "debugFocusSecrecy" => {
                let id = p.require_display_id()?;
                let handle = self.find_handle_any(id)?;
                let access = self
                    .access
                    .as_ref()
                    .ok_or_else(|| ControlError::failed("accessibility port is not initialized"))?;
                let sec = access.focus_secrecy(handle.as_ref(), None).await;
                let name = match sec {
                    Secrecy::Open => "open",
                    Secrecy::Secure => "secure",
                    Secrecy::Unknown => "unknown",
                };
                Ok(json!({ "secrecy": name }))
            }
            "debugClipboard" => {
                let id = p.require_display_id()?;
                let handle = self.find_handle_any(id)?;
                if let Some(text) = p.string("write") {
                    handle.clipboard(ClipboardSync::WriteText(text)).await?;
                    Ok(json!({ "written": true }))
                } else if let Some(b64) = p.string("writePngBase64") {
                    let png = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| ControlError::invalid(format!("invalid base64: {e}")))?;
                    handle.clipboard(ClipboardSync::WriteImagePng(png)).await?;
                    Ok(json!({ "written": true }))
                } else if p.bool("clear") {
                    handle.clipboard(ClipboardSync::Clear).await?;
                    Ok(json!({ "cleared": true }))
                } else {
                    match handle.clipboard(ClipboardSync::Read).await? {
                        ClipboardPayload::Empty => Ok(json!({ "kind": "empty", "text": null })),
                        ClipboardPayload::Text(t) => Ok(json!({ "kind": "text", "text": t })),
                        ClipboardPayload::ImagePng(png) => Ok(json!({
                            "kind": "image/png",
                            "pngBase64": base64::engine::general_purpose::STANDARD.encode(png),
                        })),
                    }
                }
            }
            "debugClipboardBridge" => {
                let id = p.require_display_id()?;
                let handle = self.find_handle_any(id)?;
                if p.0.contains_key("viewerFocused") {
                    self.clipboard_bridge
                        .set_viewer_focused(id, p.bool("viewerFocused"));
                }
                let takeover_active = self
                    .state
                    .lock()
                    .unwrap()
                    .displays
                    .get(id)
                    .is_some_and(|d| d.takeover);
                let dir = p.string("direction").unwrap_or_else(|| "status".into());
                match dir.as_str() {
                    "hostToDisplay" => {
                        let payload = if let Some(b64) = p.string("pngBase64") {
                            let png = base64::engine::general_purpose::STANDARD
                                .decode(b64)
                                .map_err(|e| ControlError::invalid(e.to_string()))?;
                            ClipboardPayload::ImagePng(png)
                        } else if let Some(text) = p.string("text") {
                            ClipboardPayload::Text(text)
                        } else {
                            ClipboardPayload::Empty
                        };
                        let synced = self
                            .clipboard_bridge
                            .sync_host_to_display(handle.as_ref(), takeover_active, payload)
                            .await?;
                        Ok(json!({
                            "synced": synced,
                            "maySync": self.clipboard_bridge.may_sync(id, takeover_active),
                        }))
                    }
                    "displayToHost" => {
                        let pulled = self
                            .clipboard_bridge
                            .sync_display_to_host(handle.as_ref(), takeover_active)
                            .await?;
                        Ok(match pulled {
                            None => json!({ "synced": false, "kind": "none" }),
                            Some(ClipboardPayload::Empty) => {
                                json!({ "synced": false, "kind": "empty" })
                            }
                            Some(ClipboardPayload::Text(t)) => {
                                json!({ "synced": true, "kind": "text", "text": t })
                            }
                            Some(ClipboardPayload::ImagePng(png)) => json!({
                                "synced": true,
                                "kind": "image/png",
                                "pngBase64": base64::engine::general_purpose::STANDARD.encode(png),
                            }),
                        })
                    }
                    _ => Ok(json!({
                        "maySync": self.clipboard_bridge.may_sync(id, takeover_active),
                        "takeover": takeover_active,
                    })),
                }
            }
            "debugDismissDisplay" => {
                let id = p.require_display_id()?;
                let removed = {
                    let mut st = self.state.lock().unwrap();
                    st.displays
                        .remove(id)
                        .map(|d| d.handle)
                        .or_else(|| st.surviving.remove(&id).map(|s| s.display))
                };
                self.web.expire_display(id);
                self.clipboard_bridge.remove_display(id);
                if let Some(ax) = &self.access {
                    ax.remove_display(id);
                }
                if let Some(h) = removed {
                    let _ = h.shutdown().await;
                }
                Ok(json!({ "dismissed": true }))
            }
            "debugQuit" => {
                let quit = self.quit.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    quit.notify_waiters();
                });
                Ok(json!({}))
            }
            other => Err(ControlError::invalid(format!("unknown method {other}"))),
        }
    }

    async fn wait_for_change(
        &self,
        connection: u64,
        p: &Params<'_>,
        session_key: Option<&str>,
    ) -> Result<Value, ControlError> {
        let access = self
            .access
            .as_ref()
            .ok_or_else(|| ControlError::failed("accessibility port is not initialized"))?;

        let wait_key = p
            .int("waitId")
            .and_then(|v| u64::try_from(v).ok())
            .map(|id| (connection, id));
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
        if let Some(key) = wait_key {
            let mut waits = self.waits.lock().unwrap();
            if waits.contains_key(&key) {
                return Err(ControlError::failed("duplicate waitId"));
            }
            waits.insert(key, cancel_tx);
        } else {
            // Keep `cancel_tx` alive inside this scope when no waitId was sent.
            std::mem::forget(cancel_tx);
        }
        struct WaitGuard<'a> {
            waits: &'a Mutex<HashMap<(u64, u64), tokio::sync::oneshot::Sender<()>>>,
            key: Option<(u64, u64)>,
        }
        impl Drop for WaitGuard<'_> {
            fn drop(&mut self) {
                if let Some(k) = self.key {
                    self.waits.lock().unwrap().remove(&k);
                }
            }
        }
        let _guard = WaitGuard {
            waits: &self.waits,
            key: wait_key,
        };

        let scope = TreeScope {
            root: p.int("root").and_then(|r| usize::try_from(r).ok()),
            max_depth: p.int("maxDepth").and_then(|d| usize::try_from(d).ok()),
        };
        let conditions =
            watch::Conditions::from_json(p.0.get("conditions").unwrap_or(&Value::Null))
                .map_err(ControlError::invalid)?;
        let timeout = watch::timeout(p.num("timeoutSeconds")).map_err(ControlError::invalid)?;
        let (id, handle) = self.require_display(p, session_key, false, false)?;
        let window_id = p
            .int("windowId")
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| ControlError::invalid("windowId is required"))?;

        // Verify the window (and optional subtree root) exists at call start.
        let wins = handle.list_windows().await?;
        if !wins.iter().any(|w| w.target.window_id == window_id) {
            return Err(ControlError::not_found(format!(
                "no window {window_id} on display {id}"
            )));
        }

        let numbered = access.has_numbers(id, window_id);
        let baseline = p
            .string("baseline")
            .filter(|_| numbered)
            .map(|tree| watch::Read {
                tree,
                truncated: p.bool("baselineTruncated"),
            });
        let from_agent = baseline.is_some();
        let mut watcher = watch::Watch::new(conditions, baseline);
        let mut depth_limited = false;
        let mut count: Option<usize> = None;
        let mut unanswered: u64 = 0;

        let started = Instant::now();
        let deadline = started + timeout;
        let waited = |s: Instant| (s.elapsed().as_secs_f64() * 10.0).round() / 10.0;

        loop {
            let (ended, by_user, in_takeover) = {
                let st = self.state.lock().unwrap();
                let d = st.displays.get(id);
                (
                    d.is_none_or(|d| d.is_ended()),
                    st.displays.closed_by_user.contains(&id)
                        || d.is_some_and(|d| d.ended == Some(EndReason::ClosedByUser)),
                    d.is_some_and(|d| d.takeover),
                )
            };
            if ended {
                return Ok(json!({
                    "outcome": "displayEnded",
                    "closedByUser": by_user,
                    "waitedSeconds": waited(started),
                }));
            }

            let here = handle
                .list_windows()
                .await
                .unwrap_or_default()
                .iter()
                .any(|w| w.target.window_id == window_id);
            if !here {
                return Ok(json!({
                    "outcome": "windowClosed",
                    "waitedSeconds": waited(started),
                }));
            }

            let read_started = Instant::now();
            let remaining = deadline.saturating_duration_since(Instant::now());
            let read_fut = access.window_tree(handle.as_ref(), window_id, scope);
            let snap_res = tokio::select! {
                res = read_fut => res,
                _ = tokio::time::sleep(remaining.max(watch::POLL)) => break,
                _ = &mut cancel_rx => {
                    return Err(ControlError::failed("the wait was cancelled"));
                }
            };
            let read_elapsed = read_started.elapsed();

            match snap_res {
                Ok(snap) => {
                    count = Some(snap.elements);
                    depth_limited |= snap.depth_limited;
                    let truncated = snap.truncated;
                    let fired = watcher.read(watch::Read {
                        tree: snap.tree,
                        truncated,
                    });
                    if !fired.is_empty() {
                        let names: Vec<&str> = fired.iter().map(|f| f.name()).collect();
                        let matched: Vec<&String> =
                            fired.iter().flat_map(|f| f.elements()).collect();
                        return Ok(json!({
                            "outcome": "fired",
                            "fired": names,
                            "matched": matched,
                            "changes": watcher.changes(),
                            "tree": watcher.last().map(|r| &r.tree),
                            "treeTruncated": truncated,
                            "elements": count,
                            "truncated": watcher.any_cut(),
                            "depthLimited": depth_limited,
                            "baselineFromAgent": from_agent,
                            "takeover": in_takeover,
                            "waitedSeconds": waited(started),
                        }));
                    }
                }
                Err(err) if err.code == "root_gone" => {
                    if watcher.baseline().is_none() && !from_agent {
                        return Err(ControlError::invalid(err.message));
                    }
                    return Ok(json!({
                        "outcome": "rootGone",
                        "waitedSeconds": waited(started),
                    }));
                }
                Err(err) if err.code == "invalid" => {
                    return Err(ControlError::invalid(err.message));
                }
                Err(err) if err.code == "not_found" => {
                    return Ok(json!({
                        "outcome": "windowClosed",
                        "waitedSeconds": waited(started),
                    }));
                }
                Err(_) => {
                    unanswered += 1;
                }
            }

            if Instant::now() >= deadline {
                break;
            }
            // On Linux AT-SPI over local Unix socket is fast, so allow responsive
            // 150ms minimum polling on early ticks while respecting `watch::poll_gap`
            // backoff on longer quiet waits.
            let gap = if watcher.quiet() <= 1 {
                Duration::from_millis(150).max(read_elapsed * 3)
            } else {
                watch::poll_gap(read_elapsed, watcher.quiet())
            }
            .min(deadline.saturating_duration_since(Instant::now()));

            tokio::select! {
                _ = tokio::time::sleep(gap) => {}
                _ = &mut cancel_rx => {
                    return Err(ControlError::failed("the wait was cancelled"));
                }
            }
        }

        let in_takeover = self
            .state
            .lock()
            .unwrap()
            .displays
            .get(id)
            .is_some_and(|d| d.takeover);
        Ok(json!({
            "outcome": "timedOut",
            "changes": watcher.changes(),
            "tree": watcher.last().map(|r| &r.tree),
            "treeTruncated": watcher.last().is_some_and(|r| r.truncated),
            "elements": watcher.last().and(count),
            "unanswered": unanswered,
            "truncated": watcher.any_cut(),
            "depthLimited": depth_limited,
            "disappearsAbsent": watcher.disappears_absent(),
            "baselineFromAgent": from_agent,
            "takeover": in_takeover,
            "waitedSeconds": waited(started),
        }))
    }

    async fn create_display(&self, connection: u64, p: &Params<'_>) -> Result<Value, ControlError> {
        let _guard = self.create_lock.lock().await;
        let session_key = p
            .string("sessionKey")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ControlError::invalid("sessionKey is required"))?;
        let new = p.bool("new");
        let orphans = parse_orphans(p);
        let asked_w = p.num("width");
        let asked_h = p.num("height");

        // 1. Check surviving scopes recovered on daemon startup that match
        //    `adoptGroups` or `sessionKey`!
        let adopted_surviving: Vec<SurvivingEntry> = {
            let mut st = self.state.lock().unwrap();
            st.conn_last_session.insert(connection, session_key.clone());
            let wanted_ids: BTreeSet<DisplayId> = orphans
                .iter()
                .map(|o| o.display)
                .filter(|&d| d > 0)
                .collect();
            let matching_ids: Vec<DisplayId> = st
                .surviving
                .iter()
                .filter(|&(id, entry)| {
                    wanted_ids.contains(id) || entry.spec.session_key == session_key
                })
                .map(|(&id, _)| id)
                .collect();
            matching_ids
                .into_iter()
                .filter_map(|id| st.surviving.remove(&id))
                .collect()
        };

        if !adopted_surviving.is_empty() {
            let mut last_id = 0;
            {
                let mut st = self.state.lock().unwrap();
                for entry in adopted_surviving {
                    let id = entry.spec.id;
                    last_id = id;
                    let owner = Owner {
                        session_key: session_key.clone(),
                        client: p.string("client").unwrap_or(entry.spec.client),
                        title: entry.spec.title,
                        connection,
                    };
                    st.displays.add(
                        id,
                        format!("Agent Display {id}"),
                        (entry.spec.width as f64, entry.spec.height as f64),
                        Some(owner),
                        entry.display.clone(),
                    );
                    if let Some(d) = st.displays.get_mut(id) {
                        for pid in entry.started_pids {
                            d.started.insert(pid);
                        }
                        let label = d.label();
                        self.web.register_display(entry.display.clone(), label);
                    }
                    entry.display.record_started_pids(
                        &orphans
                            .iter()
                            .filter(|g| g.display == id || g.display == 0)
                            .flat_map(|g| g.pids.iter().copied())
                            .collect::<Vec<_>>(),
                    );
                }
            }
            if !new {
                let st = self.state.lock().unwrap();
                let mut reply = self.describe_display(&st, last_id, &session_key, true)?;
                reply["note"] = json!(
                    "Agent Screen restarted and reattached your surviving Agent Display with its open windows intact."
                );
                return Ok(reply);
            }
        }

        // 2. Reuse current live display of this session when `new` is false.
        if !new {
            let mut st = self.state.lock().unwrap();
            st.displays
                .rebind_disconnected(&session_key, connection, self.now_ms());
            if let Some(existing_id) = st.displays.for_session(&session_key).map(|d| d.id) {
                return self.describe_display(&st, existing_id, &session_key, true);
            }
        }

        // 3. Enforce 4-display per-session cap when adding a new display.
        let id = {
            let st = self.state.lock().unwrap();
            if let Err(cap) = st.displays.sessions.may_add(&session_key) {
                return Err(ControlError::failed(cap.message()));
            }
            let mut taken: BTreeSet<DisplayId> = st.displays.of_all_ids().into_iter().collect();
            taken.extend(st.surviving.keys().copied());
            lowest_free_serial(1, &taken)
        };

        let width = asked_w
            .unwrap_or(DEFAULT_SIZE.0)
            .clamp(640.0, 2560.0)
            .round();
        let height = asked_h
            .unwrap_or(DEFAULT_SIZE.1)
            .clamp(480.0, 1600.0)
            .round();
        let clamped = asked_w.is_some_and(|a| a.round() != width)
            || asked_h.is_some_and(|a| a.round() != height);
        let client = p.string("client").unwrap_or_else(|| "Agent".into());
        let raw_title = p.string("title").unwrap_or_else(|| "Untitled".into());
        let distinct_title = self
            .state
            .lock()
            .unwrap()
            .displays
            .distinct_title(&session_key, &raw_title);

        let handle = self
            .manager
            .create(
                DisplaySpec {
                    id,
                    width: width as u32,
                    height: height as u32,
                    session_key: session_key.clone(),
                    client: client.clone(),
                    title: distinct_title.clone(),
                },
                &orphans,
            )
            .await?;

        let mut st = self.state.lock().unwrap();
        let owner = Owner {
            session_key: session_key.clone(),
            client,
            title: raw_title,
            connection,
        };
        st.displays.add(
            id,
            format!("Agent Display {id}"),
            (width, height),
            Some(owner),
            handle.clone(),
        );
        let label = st
            .displays
            .get(id)
            .map(|d| d.label())
            .unwrap_or_else(|| distinct_title);
        self.web.register_display(handle, label);

        let mut reply = self.describe_display(&st, id, &session_key, false)?;
        if clamped {
            reply["note"] = json!(format!(
                "sizes run from 640×480 to 2560×1600; made it {}×{}",
                width as i64, height as i64
            ));
        }
        Ok(reply)
    }

    fn describe_display(
        &self,
        st: &State,
        id: DisplayId,
        session_key: &str,
        reused: bool,
    ) -> Result<Value, ControlError> {
        let d = st
            .displays
            .get(id)
            .ok_or_else(|| ControlError::not_found(format!("no display {id}")))?;
        let mut out = json!({
            "displayId": d.id,
            "width": d.width as i64,
            "height": d.height as i64,
            "scale": shot_scale((d.width, d.height)),
            "label": d.label(),
            "reused": reused,
            "multiDisplay": true,
        });
        if let Some(url) = self.web.preview_url(d.id) {
            out["preview_url"] = json!(url);
        }
        let ids = st.displays.of_session(session_key);
        if ids.len() >= 2 {
            let list: Vec<Value> = ids
                .iter()
                .filter_map(|&did| st.displays.get(did))
                .map(|disp| json!({ "displayId": disp.id, "label": disp.label() }))
                .collect();
            out["displays"] = json!(list);
        }
        Ok(out)
    }

    fn require_display(
        &self,
        p: &Params<'_>,
        session_key: Option<&str>,
        refuse_takeover: bool,
        allow_ended: bool,
    ) -> Result<(DisplayId, Arc<dyn Display>), ControlError> {
        let id = p.require_display_id()?;
        let st = self.state.lock().unwrap();
        let Some(d) = st.displays.get(id) else {
            if st.displays.closed_by_user.contains(&id) {
                return Err(ControlError::ended(format!(
                    "the user closed display {id}; stop working on screen and ask before creating another"
                )));
            }
            if st.displays.retired.contains(&id) {
                return Err(ControlError::ended(format!("display {id} has ended")));
            }
            return Err(ControlError::not_found(format!("no display {id}")));
        };
        if let (Some(owner_session), Some(asking)) = (d.session(), session_key)
            && owner_session != asking
        {
            return Err(ControlError::invalid(format!(
                "display {id} belongs to another Agent Session"
            )));
        }
        if d.is_ended() && !allow_ended {
            if d.ended == Some(EndReason::ClosedByUser) {
                return Err(ControlError::ended(format!(
                    "the user closed display {id}; stop working on screen and ask before creating another"
                )));
            }
            return Err(ControlError::ended(format!("display {id} has ended")));
        }
        if refuse_takeover && d.takeover {
            return Err(ControlError::takeover(format!(
                "the user is operating display {id} (Takeover). Call wait_for_control to wait until they hand it back."
            )));
        }
        Ok((id, d.handle.clone()))
    }

    fn find_handle_any(&self, id: DisplayId) -> Result<Arc<dyn Display>, ControlError> {
        let st = self.state.lock().unwrap();
        st.displays
            .get(id)
            .map(|d| d.handle.clone())
            .or_else(|| st.surviving.get(&id).map(|s| s.display.clone()))
            .ok_or_else(|| ControlError::not_found(format!("no display {id}")))
    }

    fn display_point(
        &self,
        handle: &Arc<dyn Display>,
        shot_x: f64,
        shot_y: f64,
    ) -> Result<(f64, f64), ControlError> {
        let m = handle.metrics();
        let scale = shot_scale((m.width as f64, m.height as f64));
        let x = shot_x / scale;
        let y = shot_y / scale;
        if x < 0.0 || y < 0.0 || x >= m.width as f64 || y >= m.height as f64 {
            return Err(ControlError::invalid(format!(
                "coordinate ({shot_x}, {shot_y}) lies outside the {}×{} display",
                (m.width as f64 * scale).round() as i64,
                (m.height as f64 * scale).round() as i64,
            )));
        }
        Ok((x, y))
    }

    fn note_pointer(&self, id: DisplayId, pt: (f64, f64), click: bool) {
        self.state
            .lock()
            .unwrap()
            .displays
            .point(id, pt.0, pt.1, click);
    }
}

trait DisplaysAllIdsExt {
    fn of_all_ids(&self) -> Vec<DisplayId>;
}

impl<H> DisplaysAllIdsExt for Displays<H> {
    fn of_all_ids(&self) -> Vec<DisplayId> {
        self.list.iter().map(|d| d.id).collect()
    }
}

impl RequestHandler for Coordinator {
    fn dispatch<'a>(
        &'a self,
        connection: u64,
        method: &'a str,
        params: Map<String, Value>,
    ) -> BoxFuture<'a, Reply> {
        Box::pin(async move { Coordinator::dispatch(self, connection, method, params).await })
    }

    fn connection_closed(&self, connection: u64) -> BoxFuture<'_, ()> {
        Box::pin(async move { Coordinator::connection_closed(self, connection).await })
    }
}

fn debug_methods_enabled() -> bool {
    cfg!(debug_assertions) || std::env::var_os("AGENTSCREEN_DEBUG").is_some()
}

fn window_json(w: &WindowInfo, scale: f64) -> Value {
    json!({
        "windowId": w.target.window_id,
        "pid": w.target.pid,
        "app": w.app,
        "title": w.title,
        "x": (w.bounds.x * scale).round() as i64,
        "y": (w.bounds.y * scale).round() as i64,
        "width": (w.bounds.width * scale).round().max(1.0) as i64,
        "height": (w.bounds.height * scale).round().max(1.0) as i64,
        "kind": "window",
    })
}

fn parse_orphans(p: &Params<'_>) -> Vec<Orphans> {
    let pids = |v: &Value| -> Vec<i32> {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_i64)
                    .filter_map(|pid| i32::try_from(pid).ok().filter(|&p| p > 0))
                    .collect()
            })
            .unwrap_or_default()
    };
    if let Some(groups) = p.0.get("adoptGroups").and_then(Value::as_array) {
        return groups
            .iter()
            .filter_map(Value::as_object)
            .map(|g| Orphans {
                display: g
                    .get("display")
                    .and_then(Value::as_u64)
                    .and_then(|d| DisplayId::try_from(d).ok())
                    .unwrap_or(0),
                pids: g.get("pids").map(pids).unwrap_or_default(),
            })
            .collect();
    }
    let bare = p.0.get("adopt").map(pids).unwrap_or_default();
    if bare.is_empty() {
        Vec::new()
    } else {
        vec![Orphans {
            display: 0,
            pids: bare,
        }]
    }
}

struct Params<'a>(&'a Map<String, Value>);

impl Params<'_> {
    fn string(&self, key: &str) -> Option<String> {
        self.0.get(key)?.as_str().map(String::from)
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.0.get(key).and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
        })
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.0.get(key)?.as_f64().filter(|f| f.is_finite())
    }

    fn require_num(&self, key: &str) -> Result<f64, ControlError> {
        self.num(key)
            .ok_or_else(|| ControlError::invalid(format!("{key} is required")))
    }

    fn bool(&self, key: &str) -> bool {
        self.0.get(key) == Some(&Value::Bool(true))
    }

    fn require_display_id(&self) -> Result<DisplayId, ControlError> {
        self.int("displayId")
            .and_then(|v| DisplayId::try_from(v).ok())
            .filter(|&v| v > 0)
            .ok_or_else(|| ControlError::invalid("displayId is required"))
    }

    fn modifiers(&self) -> Result<agent_screen_model::keychord::Modifiers, ControlError> {
        let names: Vec<String> = self
            .0
            .get("modifiers")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        KeyChord::parse_modifiers(&names).map_err(|e| ControlError::invalid(e.to_string()))
    }
}
