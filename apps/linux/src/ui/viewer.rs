//! GPUI Viewer window for an isolated Linux Agent Display:
//! - Live `wlr-screencopy` -> BGRA `RenderImage` streaming with explicit
//!   `window.drop_image` sprite-atlas reclamation on every frame swap.
//! - Agent Pointer overlay while the agent operates the display.
//! - Takeover pointer/keyboard forwarding + ADR 0014 `viewer_keys` password-field
//!   key hold backed by off-main-thread `Access::focus_secrecy`.
//! - Host <-> display clipboard synchronization while `takeover && active`.
//! - Review Display keep-alive (`Keep Open`) or shutdown (`Close Display`).

use super::frame::FrameSlot;
use crate::coordinator::{Coordinator, UiDisplaySnapshot};
use agent_screen_app_core::ports::{InputOp, MouseButton as PortButton, RawRgbaFrame};
use agent_screen_model::keychord::{KeyChord, Modifiers};
use agent_screen_model::model::{DisplayId, Rect};
use agent_screen_model::pointer_geometry;
use agent_screen_model::viewer_keys::{
    HOLD_MS, Press, Route, Secrecy, Source, holds, moves_focus, route,
};
use agent_screen_ui_common::i18n::t;
use agent_screen_ui_common::{pointer, theme};
use gpui::{
    App, Bounds, ClipboardItem, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, InteractiveElement, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement, Pixels, Render, ScrollDelta, ScrollWheelEvent, Style, Styled, StyledImage as _,
    UTF16Selection, Window, canvas, div, img, point, prelude::FluentBuilder, px, relative,
};
use serde_json::{Map, Value, json};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct HeldKey {
    chord: KeyChord,
    press: Press,
    characters: Option<String>,
    queued_at: Instant,
}

pub struct ViewerView {
    pub display_id: DisplayId,
    coord: Arc<Coordinator>,
    pub focus: FocusHandle,
    pub frame_slot: FrameSlot,
    pending_raw: Option<RawRgbaFrame>,
    snapshot: Option<UiDisplaySnapshot>,
    image_bounds: Option<Bounds<Pixels>>,
    dragging_from: Option<(f64, f64)>,
    last_host_clip: Option<String>,
    // ADR 0014 password key privacy gate:
    pub source: Source,
    pub secrecy: Secrecy,
    pub gave_up: bool,
    marked: Option<String>,
    held_keys: Vec<HeldKey>,
    lookup_seq: u64,
    pub inspections: u64,
    pub main_thread_lookups: u64,
    pub secure_found: u64,
    pub last_lookup_ms: u64,
    pub last_held_ms: u64,
    pub last_route: &'static str,
}

impl ViewerView {
    pub fn new(
        display_id: DisplayId,
        coord: Arc<Coordinator>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let snapshot = coord.ui_snapshot_of(display_id);
        let view = Self {
            display_id,
            coord: coord.clone(),
            focus,
            frame_slot: FrameSlot::default(),
            pending_raw: None,
            snapshot,
            image_bounds: None,
            dragging_from: None,
            last_host_clip: None,
            source: detect_initial_input_source(),
            secrecy: Secrecy::Unknown,
            gave_up: false,
            marked: None,
            held_keys: Vec::new(),
            lookup_seq: 0,
            inspections: 0,
            main_thread_lookups: 0,
            secure_found: 0,
            last_lookup_ms: 0,
            last_held_ms: 0,
            last_route: "none",
        };
        view.spawn_frame_poller(cx);
        view
    }

    fn spawn_frame_poller(&self, cx: &mut Context<Self>) {
        let id = self.display_id;
        let coord = self.coord.clone();
        let rt = coord.tokio_handle();
        cx.spawn(async move |this, cx| {
            loop {
                let (rx_tx, rx) = futures::channel::oneshot::channel();
                let c = coord.clone();
                rt.spawn(async move {
                    let snap = c.ui_snapshot_of(id);
                    let raw = match &snap {
                        Some(s) => s.handle.capture_raw().await.ok(),
                        None => None,
                    };
                    let pulled = if snap.as_ref().is_some_and(|s| s.takeover) {
                        c.ui_pull_display_clipboard(id).await
                    } else {
                        None
                    };
                    let _ = rx_tx.send((snap, raw, pulled));
                });

                let Ok((snap, raw, pulled_clip)) = rx.await else {
                    break;
                };
                let alive = this
                    .update(cx, |view, cx| {
                        let Some(snap) = snap else {
                            return false;
                        };
                        view.snapshot = Some(snap);
                        if let Some(raw) = raw {
                            view.pending_raw = Some(raw);
                        }
                        if let Some(text) = pulled_clip
                            && view.last_host_clip.as_ref() != Some(&text)
                        {
                            view.last_host_clip = Some(text.clone());
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                        view.flush_expired_held_keys();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
            }
        })
        .detach();
    }

    pub fn cleanup(&mut self, window: &mut Window) {
        self.coord.ui_set_viewer_focused(self.display_id, false);
        self.frame_slot.clear(window);
    }

    fn trigger_secrecy_lookup(&mut self, at: Option<(f64, f64)>, cx: &mut Context<Self>) {
        self.secrecy = Secrecy::Unknown;
        self.gave_up = false;
        self.lookup_seq += 1;
        self.inspections += 1;
        let seq = self.lookup_seq;
        let id = self.display_id;
        let coord = self.coord.clone();
        let rt = coord.tokio_handle();
        let started = Instant::now();

        cx.spawn(async move |this, cx| {
            let (tx, rx) = futures::channel::oneshot::channel();
            rt.spawn(async move {
                // Two agreeing looks or 100 ms settle per ADR 0014.
                let first = coord.ui_focus_secrecy(id, at).await;
                tokio::time::sleep(Duration::from_millis(90)).await;
                let second = coord.ui_focus_secrecy(id, at).await;
                let settled = if second != Secrecy::Unknown {
                    second
                } else {
                    first
                };
                let _ = tx.send((settled, started.elapsed().as_millis() as u64));
            });
            if let Ok((sec, elapsed_ms)) = rx.await {
                let _ = this.update(cx, |view, cx| {
                    if view.lookup_seq == seq {
                        view.last_lookup_ms = elapsed_ms;
                        view.apply_settled_secrecy(sec);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn apply_settled_secrecy(&mut self, next: Secrecy) {
        self.secrecy = next;
        if next == Secrecy::Secure && self.marked.take().is_some() {
            self.secure_found += 1;
        }
        if next != Secrecy::Unknown {
            let drained = std::mem::take(&mut self.held_keys);
            for hk in drained {
                self.last_held_ms = hk.queued_at.elapsed().as_millis() as u64;
                self.dispatch_routed_key(hk.chord, hk.press, hk.characters);
            }
        }
    }

    fn flush_expired_held_keys(&mut self) {
        if self.held_keys.is_empty() {
            return;
        }
        if self.held_keys[0].queued_at.elapsed().as_millis() as u64 >= HOLD_MS {
            self.gave_up = true;
            let drained = std::mem::take(&mut self.held_keys);
            for hk in drained {
                self.last_held_ms = hk.queued_at.elapsed().as_millis() as u64;
                self.dispatch_routed_key(hk.chord, hk.press, hk.characters);
            }
        }
    }

    fn dispatch_routed_key(
        &mut self,
        chord: KeyChord,
        press: Press,
        characters: Option<String>,
    ) -> &'static str {
        let chosen = route(self.source, self.secrecy, press);
        let route_name = match chosen {
            Route::Raw => {
                self.send_input(InputOp::KeySequence {
                    chords: vec![chord],
                    repeat: 1,
                });
                "raw"
            }
            Route::InputMethod => match characters.filter(|s| !s.is_empty()) {
                Some(text) => {
                    self.send_input(InputOp::TypeText { text });
                    "text"
                }
                None => {
                    self.send_input(InputOp::KeySequence {
                        chords: vec![chord],
                        repeat: 1,
                    });
                    "raw"
                }
            },
        };
        if moves_focus(press) {
            self.secrecy = Secrecy::Unknown;
        }
        self.last_route = route_name;
        route_name
    }

    fn send_input(&self, op: InputOp) {
        let Some(snap) = &self.snapshot else {
            return;
        };
        if !snap.takeover {
            return;
        }
        let handle = snap.handle.clone();
        self.coord.tokio_handle().spawn(async move {
            let _ = handle.act_input(op).await;
        });
    }

    fn map_window_point(&self, pos: gpui::Point<Pixels>) -> Option<(f64, f64)> {
        let bounds = self.image_bounds?;
        let snap = self.snapshot.as_ref()?;
        let container = Rect::new(
            f32::from(bounds.origin.x) as f64,
            f32::from(bounds.origin.y) as f64,
            f32::from(bounds.size.width) as f64,
            f32::from(bounds.size.height) as f64,
        );
        let size = (snap.width as f64, snap.height as f64);
        let fitted = pointer_geometry::fit_image(container, size)?;
        let px_x = f32::from(pos.x) as f64;
        let px_y = f32::from(pos.y) as f64;
        if !fitted.contains((px_x, px_y)) {
            return None;
        }
        let x = ((px_x - fitted.x) / fitted.width * size.0).clamp(0.0, (size.0 - 1.0).max(0.0));
        let y = ((px_y - fitted.y) / fitted.height * size.1).clamp(0.0, (size.1 - 1.0).max(0.0));
        Some((x, y))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(snap) = &self.snapshot else { return };
        if !snap.takeover {
            return;
        }
        let Some(pt) = self.map_window_point(ev.position) else {
            return;
        };
        let btn = match ev.button {
            MouseButton::Left => PortButton::Left,
            MouseButton::Right => PortButton::Right,
            MouseButton::Middle => PortButton::Middle,
            _ => return,
        };
        if btn == PortButton::Left {
            self.dragging_from = Some(pt);
        }
        self.trigger_secrecy_lookup(Some(pt), cx);
        let mods = gpui_modifiers(&ev.modifiers);
        self.send_input(InputOp::Click {
            point: pt,
            button: btn,
            count: (ev.click_count as u32).clamp(1, 3),
            modifiers: mods,
        });
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(from) = self.dragging_from.take() else {
            return;
        };
        let Some(to) = self.map_window_point(ev.position) else {
            return;
        };
        if (to.0 - from.0).hypot(to.1 - from.1) >= 6.0 {
            self.send_input(InputOp::Drag {
                from,
                to,
                modifiers: gpui_modifiers(&ev.modifiers),
            });
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(snap) = &self.snapshot else { return };
        if !snap.takeover {
            return;
        }
        if let Some(pt) = self.map_window_point(ev.position) {
            self.send_input(InputOp::Move { point: pt });
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some(snap) = &self.snapshot else { return };
        if !snap.takeover {
            return;
        }
        let Some(pt) = self.map_window_point(ev.position) else {
            return;
        };
        let (dx, dy) = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x) as f64, f32::from(p.y) as f64),
            ScrollDelta::Lines(l) => (l.x as f64 * 24.0, l.y as f64 * 24.0),
        };
        self.send_input(InputOp::Scroll {
            point: pt,
            delta: (dx, -dy),
            modifiers: gpui_modifiers(&ev.modifiers),
        });
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(snap) = &self.snapshot else { return };
        if !snap.takeover || self.marked.is_some() {
            return;
        }
        let Some(chord) = keystroke_to_chord(&ev.keystroke) else {
            return;
        };
        let press = Press {
            code: chord.code,
            command: ev.keystroke.modifiers.platform,
            control: ev.keystroke.modifiers.control,
            repeat: ev.is_held,
        };
        let chars = ev.keystroke.key_char.clone();
        if holds(self.source, self.secrecy, press, self.gave_up) {
            self.held_keys.push(HeldKey {
                chord,
                press,
                characters: chars,
                queued_at: Instant::now(),
            });
            self.last_route = "held";
            if self.inspections == 0 || self.secrecy == Secrecy::Unknown {
                self.trigger_secrecy_lookup(None, cx);
            }
            cx.stop_propagation();
            return;
        }
        let moved = moves_focus(press);
        self.dispatch_routed_key(chord, press, chars);
        if moved {
            self.trigger_secrecy_lookup(None, cx);
        }
        cx.stop_propagation();
    }

    pub fn debug_snapshot(&mut self, window: &Window) -> Value {
        self.snapshot = self.coord.ui_snapshot_of(self.display_id);
        let win_bounds = window.bounds();
        let img_bounds = self.image_bounds.unwrap_or(win_bounds);
        let secrecy_str = match self.secrecy {
            Secrecy::Unknown => "unknown",
            Secrecy::Secure => "secure",
            Secrecy::Open => "open",
        };
        let source_str = match self.source {
            Source::Layout => "layout",
            Source::InputMethod => "inputMethod",
        };
        json!({
            "displayId": self.display_id,
            "frame": [
                f32::from(win_bounds.origin.x),
                f32::from(win_bounds.origin.y),
                f32::from(win_bounds.size.width),
                f32::from(win_bounds.size.height),
            ],
            "image": [
                f32::from(img_bounds.origin.x),
                f32::from(img_bounds.origin.y),
                f32::from(img_bounds.size.width),
                f32::from(img_bounds.size.height),
            ],
            "key": window.is_window_active(),
            "takeover": self.snapshot.as_ref().is_some_and(|s| s.takeover),
            "ended": self.snapshot.as_ref().is_some_and(|s| s.ended),
            "keptReview": self.snapshot.as_ref().is_some_and(|s| s.kept_review),
            "uploadedFrames": self.frame_slot.uploaded_frames,
            "droppedFrames": self.frame_slot.dropped_frames,
            "residentTextures": self.frame_slot.resident_textures(),
            "source": source_str,
            "secrecy": secrecy_str,
            "secure": self.secrecy == Secrecy::Secure,
            "marked": self.marked,
            "inspections": self.inspections,
            "mainThreadLookups": self.main_thread_lookups,
            "secureFound": self.secure_found,
            "lookupMs": self.last_lookup_ms,
            "heldMs": self.last_held_ms,
            "lastRoute": self.last_route,
        })
    }

    pub fn debug_input(
        &mut self,
        p: &Map<String, Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Value {
        if let Some(src) = p.get("source").and_then(Value::as_str) {
            self.source = if src == "inputMethod" {
                Source::InputMethod
            } else {
                Source::Layout
            };
        }
        let kind = p.get("kind").and_then(Value::as_str).unwrap_or("key");
        match kind {
            "swapFrames" => {
                let count = p
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(20)
                    .clamp(1, 200);
                let w = 64u32;
                let h = 64u32;
                for i in 1..=count {
                    let mut rgba = vec![0u8; (w * h * 4) as usize];
                    for chunk in rgba.as_chunks_mut::<4>().0 {
                        chunk[0] = (i & 0xff) as u8;
                        chunk[1] = 100;
                        chunk[2] = 200;
                        chunk[3] = 255;
                    }
                    let raw = RawRgbaFrame {
                        width: w,
                        height: h,
                        rgba,
                        digest: 1_000_000 + i,
                    };
                    self.frame_slot.swap(&raw, window);
                }
                cx.notify();
                json!({
                    "uploadedFrames": self.frame_slot.uploaded_frames,
                    "droppedFrames": self.frame_slot.dropped_frames,
                    "residentTextures": self.frame_slot.resident_textures(),
                })
            }
            "click" => {
                let x = p.get("x").and_then(Value::as_f64).unwrap_or(100.0);
                let y = p.get("y").and_then(Value::as_f64).unwrap_or(100.0);
                self.snapshot = self.coord.ui_snapshot_of(self.display_id);
                self.coord.ui_set_viewer_focused(self.display_id, true);
                self.trigger_secrecy_lookup(Some((x, y)), cx);
                self.send_input(InputOp::Click {
                    point: (x, y),
                    button: PortButton::Left,
                    count: 1,
                    modifiers: Modifiers::default(),
                });
                json!({ "clicked": [x, y], "secrecy": "unknown" })
            }
            "mark" => {
                let text = p
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if self.secrecy == Secrecy::Secure {
                    self.marked = None;
                    self.secure_found += 1;
                } else {
                    self.marked = (!text.is_empty()).then_some(text);
                }
                cx.notify();
                json!({ "marked": self.marked, "secureFound": self.secure_found })
            }
            "commit" => {
                let text = p
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.marked = None;
                if self.secrecy != Secrecy::Secure && !text.is_empty() {
                    self.send_input(InputOp::TypeText { text });
                    self.last_route = "text";
                } else {
                    self.secure_found += 1;
                    self.last_route = "blockedSecure";
                }
                cx.notify();
                json!({ "route": self.last_route, "secureFound": self.secure_found })
            }
            _ => {
                self.snapshot = self.coord.ui_snapshot_of(self.display_id);
                let key_name = p.get("key").and_then(Value::as_str).unwrap_or("a");
                let chars = p
                    .get("characters")
                    .and_then(Value::as_str)
                    .map(String::from);
                let control = p.get("control").and_then(Value::as_bool).unwrap_or(false);
                let command = p.get("command").and_then(Value::as_bool).unwrap_or(false);
                let shift = p.get("shift").and_then(Value::as_bool).unwrap_or(false);
                let mut chord = KeyChord::parse(key_name).unwrap_or(KeyChord {
                    code: 0,
                    modifiers: Modifiers::default(),
                });
                chord.modifiers.control |= control;
                chord.modifiers.command |= command;
                chord.modifiers.shift |= shift;
                let press = Press {
                    code: chord.code,
                    command: chord.modifiers.command,
                    control: chord.modifiers.control,
                    repeat: p.get("repeat").and_then(Value::as_bool).unwrap_or(false),
                };
                if holds(self.source, self.secrecy, press, self.gave_up) {
                    self.held_keys.push(HeldKey {
                        chord,
                        press,
                        characters: chars,
                        queued_at: Instant::now(),
                    });
                    self.last_route = "held";
                    self.trigger_secrecy_lookup(None, cx);
                    return json!({ "route": "held", "secrecy": "unknown" });
                }
                let route_name = self.dispatch_routed_key(chord, press, chars);
                let secrecy_str = match self.secrecy {
                    Secrecy::Unknown => "unknown",
                    Secrecy::Secure => "secure",
                    Secrecy::Open => "open",
                };
                json!({ "route": route_name, "secrecy": secrecy_str })
            }
        }
    }
}

impl EntityInputHandler for ViewerView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        Some(self.marked.clone().unwrap_or_default())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len: usize = self
            .marked
            .as_deref()
            .unwrap_or_default()
            .chars()
            .map(char::len_utf16)
            .sum();
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let marked = self.marked.as_ref()?;
        let len: usize = marked.chars().map(char::len_utf16).sum();
        (len > 0).then_some(0..len)
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if self.secrecy != Secrecy::Secure && !text.is_empty() {
            self.send_input(InputOp::TypeText {
                text: text.to_string(),
            });
            self.last_route = "text";
        } else if self.secrecy == Secrecy::Secure {
            self.secure_found += 1;
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.secrecy == Secrecy::Secure {
            self.marked = None;
            self.secure_found += 1;
        } else {
            self.marked = (!text.is_empty()).then(|| text.to_string());
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _at: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

impl Render for ViewerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(raw) = self.pending_raw.take() {
            self.frame_slot.swap(&raw, window);
        }
        let active = window.is_window_active();
        self.coord.ui_set_viewer_focused(self.display_id, active);

        if active
            && self.snapshot.as_ref().is_some_and(|s| s.takeover)
            && let Some(clip) = cx.read_from_clipboard().and_then(|c| c.text())
            && self.last_host_clip.as_ref() != Some(&clip)
        {
            self.last_host_clip = Some(clip.clone());
            let coord = self.coord.clone();
            let id = self.display_id;
            coord.tokio_handle().spawn(async move {
                let _ = coord.ui_sync_host_clipboard(id, clip).await;
            });
        }

        let theme = theme::theme(window);
        let snap = self.snapshot.clone();
        let label = snap
            .as_ref()
            .map(|s| s.label.clone())
            .unwrap_or_else(|| format!("Display {}", self.display_id));
        let size = snap
            .as_ref()
            .map(|s| (s.width as f64, s.height as f64))
            .unwrap_or((1280.0, 800.0));
        let takeover = snap.as_ref().is_some_and(|s| s.takeover);
        let recording = snap.as_ref().is_some_and(|s| s.recording);
        let ended = snap.as_ref().is_some_and(|s| s.ended);
        let kept_review = snap.as_ref().is_some_and(|s| s.kept_review);
        let ptr = snap.as_ref().and_then(|s| s.pointer);

        let status_text = if ended {
            if kept_review {
                t("Kept Open", "已保留")
            } else {
                t("Review", "待审阅")
            }
        } else if takeover {
            t("User Control", "已接管")
        } else {
            t("Agent Working", "运行中")
        };

        let id = self.display_id;
        let coord_toggle = self.coord.clone();
        let coord_keep = self.coord.clone();
        let coord_close = self.coord.clone();
        let entity_for_bounds = cx.entity();

        div()
            .id("viewer-root")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.card)
            .text_color(theme.label)
            .child(
                div()
                    .h(px(44.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.hairline)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_sm().child(label))
                            .child(
                                div()
                                    .text_xs()
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .bg(theme.control)
                                    .text_color(theme.secondary)
                                    .child(status_text),
                            )
                            .when(recording, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .px(px(6.))
                                        .py(px(2.))
                                        .rounded(px(6.))
                                        .bg(theme.red)
                                        .text_color(gpui::white())
                                        .child(t("Recording", "录制中")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(!ended, |el| {
                                el.child(
                                    div()
                                        .id("viewer-takeover-btn")
                                        .px(px(10.))
                                        .py(px(4.))
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .bg(if takeover { theme.orange } else { theme.accent })
                                        .text_color(gpui::white())
                                        .text_xs()
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            coord_toggle.ui_set_takeover(id, !takeover);
                                            cx.stop_propagation();
                                        })
                                        .child(if takeover {
                                            t("Hand Control Back", "交回控制权")
                                        } else {
                                            t("Take Over", "接管")
                                        }),
                                )
                            })
                            .when(ended, |el| {
                                el.child(
                                    div()
                                        .id("viewer-keep-btn")
                                        .px(px(10.))
                                        .py(px(4.))
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .bg(theme.control)
                                        .text_xs()
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            coord_keep.ui_keep_review_display(id);
                                            cx.stop_propagation();
                                        })
                                        .child(t("Keep Open", "保留窗口")),
                                )
                                .child(
                                    div()
                                        .id("viewer-dismiss-btn")
                                        .px(px(10.))
                                        .py(px(4.))
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .bg(theme.red)
                                        .text_color(gpui::white())
                                        .text_xs()
                                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                            let c = coord_close.clone();
                                            c.tokio_handle().spawn(async move {
                                                c.ui_dismiss_display(id).await;
                                            });
                                            window.remove_window();
                                            cx.stop_propagation();
                                        })
                                        .child(t("Close Display", "关闭显示器")),
                                )
                            }),
                    ),
            )
            .when(ended && !kept_review, |el| {
                el.child(
                    div()
                        .px(px(14.))
                        .py(px(8.))
                        .bg(theme.asking_card)
                        .border_b_1()
                        .border_color(theme.asking_border)
                        .text_xs()
                        .child(t(
                            "Session ended. Keep this display open to inspect its windows, or close it to stop its compositor.",
                            "会话已结束。可保留此显示器继续查看窗口，或关闭显示器以停止其隔离合成器。",
                        )),
                )
            })
            .child(
                div()
                    .id("viewer-stage")
                    .flex_1()
                    .m(px(10.))
                    .relative()
                    .rounded(px(10.))
                    .bg(theme.screen_empty)
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .child(ViewerInputCanvas {
                        view: entity_for_bounds,
                    })
                    .child(match self.frame_slot.image() {
                        Some(render_img) => img(render_img)
                            .object_fit(ObjectFit::Contain)
                            .size_full()
                            .into_any_element(),
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(t("Waiting for picture...", "正在等待画面…"))
                            .into_any_element(),
                    })
                    .child(theme::round_corners(10., theme.card))
                    .when(!takeover && !ended, |el| {
                        el.child(pointer::layer(id, ptr, size, theme.accent))
                    }),
            )
    }
}

struct ViewerInputCanvas {
    view: Entity<ViewerView>,
}

impl IntoElement for ViewerInputCanvas {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for ViewerInputCanvas {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style {
            position: gpui::Position::Absolute,
            ..Default::default()
        };
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        if self.view.read(cx).secrecy != Secrecy::Secure {
            window.handle_input(
                &focus,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
        }
        self.view.update(cx, |v, _| {
            v.image_bounds = Some(bounds);
        });
        let _ = canvas(|_, _, _| {}, |_, _, _, _| {});
        let _ = point(px(0.), px(0.));
    }
}

fn detect_initial_input_source() -> Source {
    let im = std::env::var("GTK_IM_MODULE")
        .or_else(|_| std::env::var("QT_IM_MODULE"))
        .or_else(|_| std::env::var("XMODIFIERS"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if im.contains("fcitx") || im.contains("ibus") {
        Source::InputMethod
    } else {
        Source::Layout
    }
}

fn gpui_modifiers(m: &gpui::Modifiers) -> Modifiers {
    Modifiers {
        control: m.control,
        alt: m.alt,
        shift: m.shift,
        command: m.platform,
        function: m.function,
    }
}

fn keystroke_to_chord(ks: &gpui::Keystroke) -> Option<KeyChord> {
    let key = match ks.key.as_str() {
        "enter" => "Return",
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "tab" => "Tab",
        "escape" => "Escape",
        "space" | " " => "space",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        other => other,
    };
    let mut chord = KeyChord::parse(key).ok()?;
    chord.modifiers = gpui_modifiers(&ks.modifiers);
    Some(chord)
}
