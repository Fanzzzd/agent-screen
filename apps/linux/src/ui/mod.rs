//! GPUI desktop UI shell for `apps/linux`: coordinates `ViewerView`,
//! `HelpCardView`, `PeekView` (`LayerShell` with `PopUp` fallback), GNOME
//! `StatusWindowView`, `ksni` SNI tray, and desktop notifications, while
//! leaving headless daemon mode untouched when `AGENTSCREEN_HEADLESS=1` or no
//! host display server is configured.

pub mod frame;
pub mod help_card;
pub mod peek;
pub mod recordings;
pub mod tray_and_notify;
pub mod viewer;

use crate::access_atspi::AtspiAccess;
use crate::coordinator::{Coordinator, UiControlBridge};
use crate::sway_display::SwayDisplayManager;
use agent_screen_app_core::server::{ControlError, prepare_socket_path, serve_socket};
use agent_screen_app_core::web_preview::{WebPreviewServer, delete_state_file};
use agent_screen_model::model::DisplayId;
use agent_screen_model::request::{Answer, REPLY_CHARS};
use agent_screen_ui_common::{card_layout, pointer};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    App, AppContext, Bounds, Entity, TitlebarOptions, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, px, size,
};
use help_card::HelpCardView;
use peek::{PeekView, StatusWindowView};
use recordings::RecordingsView;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tray_and_notify::{HostDesktop, TrayAction};
use viewer::ViewerView;

pub fn should_run_gui() -> bool {
    if let Ok(val) = std::env::var("AGENTSCREEN_HEADLESS") {
        let v = val.trim().to_ascii_lowercase();
        if v == "1" || v == "true" || v == "yes" {
            return false;
        }
    }
    let has_wayland = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .is_some_and(|s| !s.trim().is_empty());
    let has_x11 = std::env::var("DISPLAY")
        .ok()
        .is_some_and(|s| !s.trim().is_empty());
    has_wayland || has_x11
}

enum UiCommand {
    ShowSurface {
        params: Map<String, Value>,
        reply: oneshot::Sender<Result<Value, ControlError>>,
    },
    DebugViewer {
        display_id: DisplayId,
        reply: oneshot::Sender<Result<Value, ControlError>>,
    },
    DebugViewerInput {
        params: Map<String, Value>,
        reply: oneshot::Sender<Result<Value, ControlError>>,
    },
    DebugCard {
        reply: oneshot::Sender<Result<Value, ControlError>>,
    },
    DebugIcon {
        reply: oneshot::Sender<Result<Value, ControlError>>,
    },
    Attention {
        display_id: DisplayId,
        title: String,
        body: String,
    },
    Quit,
}

struct LinuxUiBridge {
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
}

impl UiControlBridge for LinuxUiBridge {
    fn on_attention_requested(&self, display_id: DisplayId, title: String, body: String) {
        let _ = self.cmd_tx.unbounded_send(UiCommand::Attention {
            display_id,
            title,
            body,
        });
    }

    fn debug_show<'a>(
        &'a self,
        params: Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, ControlError>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.cmd_tx
                .unbounded_send(UiCommand::ShowSurface { params, reply: tx })
                .map_err(|_| ControlError::failed("UI loop closed"))?;
            rx.await
                .map_err(|_| ControlError::failed("UI loop dropped debugShow"))?
        })
    }

    fn debug_viewer<'a>(
        &'a self,
        display_id: DisplayId,
    ) -> BoxFuture<'a, Result<Value, ControlError>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.cmd_tx
                .unbounded_send(UiCommand::DebugViewer {
                    display_id,
                    reply: tx,
                })
                .map_err(|_| ControlError::failed("UI loop closed"))?;
            rx.await
                .map_err(|_| ControlError::failed("UI loop dropped debugViewer"))?
        })
    }

    fn debug_viewer_input<'a>(
        &'a self,
        params: Map<String, Value>,
    ) -> BoxFuture<'a, Result<Value, ControlError>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.cmd_tx
                .unbounded_send(UiCommand::DebugViewerInput { params, reply: tx })
                .map_err(|_| ControlError::failed("UI loop closed"))?;
            rx.await
                .map_err(|_| ControlError::failed("UI loop dropped debugViewerInput"))?
        })
    }

    fn debug_card(&self) -> BoxFuture<'_, Result<Value, ControlError>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.cmd_tx
                .unbounded_send(UiCommand::DebugCard { reply: tx })
                .map_err(|_| ControlError::failed("UI loop closed"))?;
            rx.await
                .map_err(|_| ControlError::failed("UI loop dropped debugCard"))?
        })
    }

    fn debug_icon(&self) -> BoxFuture<'_, Result<Value, ControlError>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            self.cmd_tx
                .unbounded_send(UiCommand::DebugIcon { reply: tx })
                .map_err(|_| ControlError::failed("UI loop closed"))?;
            rx.await
                .map_err(|_| ControlError::failed("UI loop dropped debugIcon"))?
        })
    }

    fn request_quit(&self) {
        let _ = self.cmd_tx.unbounded_send(UiCommand::Quit);
    }
}

struct ShellWindows {
    coord: Arc<Coordinator>,
    desktop: Arc<HostDesktop>,
    tray_tx: mpsc::UnboundedSender<TrayAction>,
    viewers: HashMap<DisplayId, WindowHandle<ViewerView>>,
    peek: Option<(WindowHandle<PeekView>, bool)>,
    card: Option<WindowHandle<HelpCardView>>,
    recordings_win: Option<WindowHandle<RecordingsView>>,
    status_win: Option<WindowHandle<StatusWindowView>>,
    last_auto_prompt: Option<(DisplayId, u64)>,
}

impl ShellWindows {
    fn open_viewer(
        &mut self,
        id: DisplayId,
        cx: &mut App,
    ) -> Result<WindowHandle<ViewerView>, ControlError> {
        if let Some(existing) = self.viewers.get(&id).copied()
            && existing
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return Ok(existing);
        }
        let snap = self
            .coord
            .ui_snapshot_of(id)
            .ok_or_else(|| ControlError::not_found(format!("no display {id}")))?;
        let w = (snap.width as f32 * 0.72).clamp(640.0, 1180.0);
        let h = (snap.height as f32 * 0.72 + 54.0).clamp(460.0, 860.0);
        let coord = self.coord.clone();
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(w), px(h)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("Agent Screen: {}", snap.label).into()),
                ..Default::default()
            }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let handle = cx
            .open_window(opts, move |window, cx| {
                cx.new(|cx| ViewerView::new(id, coord, window, cx))
            })
            .map_err(|e| ControlError::failed(format!("open Viewer failed: {e}")))?;
        self.viewers.insert(id, handle);
        Ok(handle)
    }

    fn close_viewer(&mut self, id: DisplayId, cx: &mut App) {
        if let Some(handle) = self.viewers.remove(&id) {
            let _ = handle.update(cx, |v, window, _| {
                v.cleanup(window);
                window.remove_window();
            });
        }
        pointer::forget(id);
    }

    fn open_peek(&mut self, cx: &mut App) -> Result<bool, ControlError> {
        if let Some((existing, is_layer)) = self.peek
            && existing.read_with(cx, |_, _| ()).is_ok()
        {
            return Ok(is_layer);
        }
        let coord = self.coord.clone();
        let tx = self.tray_tx.clone();
        let layer_opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                gpui::point(px(0.), px(0.)),
                size(px(300.), px(260.)),
            ))),
            focus: false,
            show: true,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "agent-screen-peek".into(),
                layer: Layer::Top,
                anchor: Anchor::TOP | Anchor::RIGHT,
                exclusive_zone: None,
                exclusive_edge: None,
                margin: Some((px(12.), px(12.), px(0.), px(0.))),
                keyboard_interactivity: KeyboardInteractivity::None,
            }),
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let c1 = coord.clone();
        let tx1 = tx.clone();
        if let Ok(handle) = cx.open_window(layer_opts, move |_, cx| {
            cx.new(|cx| PeekView::new(c1, tx1, true, cx))
        }) {
            self.peek = Some((handle, true));
            return Ok(true);
        }

        // GNOME Mutter / compositors without zwlr_layer_shell_v1: fall back to PopUp/Normal.
        let fallback_opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(300.), px(260.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Screen Peek".into()),
                ..Default::default()
            }),
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let handle = cx
            .open_window(fallback_opts, move |_, cx| {
                cx.new(|cx| PeekView::new(coord, tx, false, cx))
            })
            .map_err(|e| ControlError::failed(format!("open Peek failed: {e}")))?;
        self.peek = Some((handle, false));
        Ok(false)
    }

    fn hide_peek(&mut self, cx: &mut App) {
        if let Some((h, _)) = self.peek.take() {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn open_card(
        &mut self,
        display_id: Option<DisplayId>,
        cx: &mut App,
    ) -> Result<WindowHandle<HelpCardView>, ControlError> {
        if let Some(existing) = self.card
            && existing
                .update(cx, |card, _, cx| {
                    if display_id.is_some() {
                        card.set_display(display_id, cx);
                    }
                })
                .is_ok()
        {
            return Ok(existing);
        }
        let h_px = card_layout::height((1280.0, 800.0), true);
        let coord = self.coord.clone();
        let layer_opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                gpui::point(px(0.), px(0.)),
                size(px(card_layout::WIDTH), px(h_px)),
            ))),
            focus: false,
            show: true,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "agent-screen-help-card".into(),
                layer: Layer::Top,
                anchor: Anchor::TOP | Anchor::RIGHT,
                exclusive_zone: None,
                exclusive_edge: None,
                margin: Some((px(16.), px(16.), px(0.), px(0.))),
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
            }),
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let c1 = coord.clone();
        if let Ok(handle) = cx.open_window(layer_opts, move |_, cx| {
            cx.new(|cx| HelpCardView::new(display_id, c1, cx))
        }) {
            self.card = Some(handle);
            return Ok(handle);
        }

        let fallback_opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(card_layout::WIDTH), px(h_px)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Screen".into()),
                ..Default::default()
            }),
            focus: false,
            show: true,
            kind: WindowKind::Normal,
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let handle = cx
            .open_window(fallback_opts, move |_, cx| {
                cx.new(|cx| HelpCardView::new(display_id, coord, cx))
            })
            .map_err(|e| ControlError::failed(format!("open HelpCard failed: {e}")))?;
        self.card = Some(handle);
        Ok(handle)
    }

    fn ensure_status_window(&mut self, cx: &mut App) -> Result<bool, ControlError> {
        if let Some(existing) = self.status_win
            && existing.read_with(cx, |_, _| ()).is_ok()
        {
            return Ok(true);
        }
        let coord = self.coord.clone();
        let tx = self.tray_tx.clone();
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(300.), px(58.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Screen".into()),
                ..Default::default()
            }),
            focus: false,
            show: true,
            kind: WindowKind::Normal,
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let handle = cx
            .open_window(opts, move |_, cx| {
                cx.new(|cx| StatusWindowView::new(coord, tx, cx))
            })
            .map_err(|e| ControlError::failed(format!("open StatusWindow failed: {e}")))?;
        self.status_win = Some(handle);
        Ok(true)
    }

    fn close_status_window(&mut self, cx: &mut App) {
        if let Some(h) = self.status_win.take() {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn open_recordings(
        &mut self,
        cx: &mut App,
    ) -> Result<WindowHandle<RecordingsView>, ControlError> {
        if let Some(existing) = self.recordings_win
            && existing
                .update(cx, |v, window, cx| {
                    v.refresh_library(cx);
                    window.activate_window();
                })
                .is_ok()
        {
            return Ok(existing);
        }
        let rt = self.coord.tokio_handle();
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(880.), px(560.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Agent Screen: Recordings".into()),
                ..Default::default()
            }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            app_id: Some("app.agentscreen.AgentScreen".into()),
            ..Default::default()
        };
        let handle = cx
            .open_window(opts, move |window, cx| {
                cx.new(|cx| RecordingsView::new(rt, window, cx))
            })
            .map_err(|e| ControlError::failed(format!("open Recordings failed: {e}")))?;
        self.recordings_win = Some(handle);
        Ok(handle)
    }

    fn close_recordings(&mut self, cx: &mut App) {
        if let Some(h) = self.recordings_win.take() {
            let _ = h.update(cx, |v, window, _| {
                v.cleanup(window);
                window.remove_window();
            });
        }
    }

    fn handle_debug_show(
        &mut self,
        p: &Map<String, Value>,
        cx: &mut App,
    ) -> Result<Value, ControlError> {
        let surface = p.get("surface").and_then(Value::as_str).unwrap_or("viewer");
        let id = p.get("displayId").and_then(Value::as_u64).unwrap_or(0) as DisplayId;

        match surface {
            "viewer" | "select" | "focus" => {
                if id == 0 {
                    return Err(ControlError::invalid("displayId is required"));
                }
                self.open_viewer(id, cx)?;
                Ok(json!({ "surface": surface, "displayId": id }))
            }
            "collapse" => {
                self.close_viewer(id, cx);
                Ok(json!({ "closed": true }))
            }
            "peek" | "click" => {
                let is_layer = self.open_peek(cx)?;
                Ok(json!({ "surface": "peek", "layerShell": is_layer }))
            }
            "hide" => {
                self.hide_peek(cx);
                Ok(json!({ "hidden": true }))
            }
            "status-window" => {
                self.ensure_status_window(cx)?;
                Ok(json!({ "surface": "status-window", "open": true }))
            }
            "recordings" => {
                let handle = self.open_recordings(cx)?;
                let count = handle.read_with(cx, |v, _| v.entries.len()).unwrap_or(0);
                Ok(json!({ "surface": "recordings", "open": true, "entries": count }))
            }
            "recordings-close" => {
                self.close_recordings(cx);
                Ok(json!({ "surface": "recordings-close", "open": false }))
            }
            "card-open" => {
                let target = (id > 0).then_some(id);
                self.open_card(target, cx)?;
                Ok(json!({ "surface": "card-open" }))
            }
            "card-later" => {
                if id > 0 {
                    self.coord.ui_put_off_help(id);
                }
                if let Some(card) = self.card.take() {
                    let _ = card.update(cx, |c, window, _| {
                        c.cleanup(window);
                        window.remove_window();
                    });
                }
                Ok(json!({ "putOff": true }))
            }
            "card-take-over" => {
                let target = self.resolve_card_display(id)?;
                self.coord.ui_set_takeover(target, true);
                self.open_viewer(target, cx)?;
                Ok(json!({ "answered": "takeOver", "displayId": target }))
            }
            "card-help-continue" => {
                let target = self.resolve_card_display(id)?;
                self.coord
                    .ui_answer_help(target, None, Answer::Continue)
                    .map_err(ControlError::failed)?;
                Ok(json!({ "answered": "continue", "displayId": target }))
            }
            "card-reply" | "card-reply-long" => {
                let target = self.resolve_card_display(id)?;
                let handle = self.open_card(Some(target), cx)?;
                let draft = (surface == "card-reply-long").then(|| "x".repeat(REPLY_CHARS + 25));
                let opened = handle
                    .update(cx, |card, window, cx| {
                        card.open_reply(target, draft, false, window, cx)
                    })
                    .unwrap_or(false);
                Ok(json!({ "replyOpen": opened, "displayId": target }))
            }
            "card-reply-cancel" => {
                if let Some(card) = self.card {
                    let _ = card.update(cx, |c, _, cx| c.close_reply(cx));
                }
                Ok(json!({ "replyOpen": false }))
            }
            "card-continue" => {
                let target = self.resolve_card_display(id)?;
                self.coord
                    .ui_answer_control(target, None, true)
                    .map_err(ControlError::failed)?;
                Ok(json!({ "answered": "continue" }))
            }
            "card-keep-control" => {
                let target = self.resolve_card_display(id)?;
                self.coord
                    .ui_answer_control(target, None, false)
                    .map_err(ControlError::failed)?;
                Ok(json!({ "answered": "keep" }))
            }
            "keep-review" => {
                if id == 0 {
                    return Err(ControlError::invalid("displayId is required"));
                }
                let kept = self.coord.ui_keep_review_display(id);
                Ok(json!({ "keptReview": kept }))
            }
            "dismiss" | "dismiss-confirm" => {
                if id == 0 {
                    return Err(ControlError::invalid("displayId is required"));
                }
                self.close_viewer(id, cx);
                let coord = self.coord.clone();
                coord.tokio_handle().spawn(async move {
                    coord.ui_dismiss_display(id).await;
                });
                Ok(json!({ "dismissed": true }))
            }
            "quit" => {
                cx.quit();
                Ok(json!({}))
            }
            other => Err(ControlError::invalid(format!(
                "unsupported Linux debugShow surface {other:?}"
            ))),
        }
    }

    fn resolve_card_display(&self, explicit: DisplayId) -> Result<DisplayId, ControlError> {
        if explicit > 0 {
            return Ok(explicit);
        }
        self.coord
            .ui_snapshots()
            .into_iter()
            .find(|s| s.help.is_some() || s.control_req.is_some())
            .map(|s| s.id)
            .ok_or_else(|| ControlError::not_found("no display has an open request"))
    }
}

pub fn run_gui_daemon() -> anyhow::Result<()> {
    let socket_path = prepare_socket_path()?;
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded::<UiCommand>();
    let (tray_tx, mut tray_rx) = mpsc::unbounded::<TrayAction>();
    let (boot_tx, boot_rx) = std::sync::mpsc::sync_channel(1);

    let quit_notify = Arc::new(tokio::sync::Notify::new());
    let quit_for_thread = quit_notify.clone();
    let cmd_tx_boot = cmd_tx.clone();
    let tray_tx_boot = tray_tx.clone();

    std::thread::Builder::new()
        .name("agent-screen-tokio".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = boot_tx.send(Err(anyhow::anyhow!(e)));
                    return;
                }
            };
            rt.block_on(async move {
                let web_server = match WebPreviewServer::start().await {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = boot_tx.send(Err(e));
                        return;
                    }
                };
                let manager = Arc::new(SwayDisplayManager::new());
                let access = Arc::new(AtspiAccess::new());
                let coord =
                    Coordinator::new(manager, Some(access), web_server, quit_for_thread.clone())
                        .await;
                let bridge = Arc::new(LinuxUiBridge {
                    cmd_tx: cmd_tx_boot.clone(),
                });
                coord.attach_ui_bridge(bridge);
                let desktop = HostDesktop::start(
                    &tokio::runtime::Handle::current(),
                    coord.clone(),
                    tray_tx_boot,
                );

                let _ = boot_tx.send(Ok((coord.clone(), desktop)));

                let quit_sig = quit_for_thread.clone();
                let cmd_quit = cmd_tx_boot.clone();
                tokio::spawn(async move {
                    let mut sigterm =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("SIGTERM handler");
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = sigterm.recv() => {}
                    }
                    quit_sig.notify_waiters();
                    let _ = cmd_quit.unbounded_send(UiCommand::Quit);
                });

                let _ = serve_socket(socket_path, coord.clone(), quit_for_thread).await;
                delete_state_file();
                if std::env::var_os("AGENTSCREEN_KEEP_SCOPES_ON_QUIT").is_none() {
                    coord.shutdown_all().await;
                }
                let _ = cmd_tx_boot.unbounded_send(UiCommand::Quit);
            });
        })?;

    let (coord, desktop) = boot_rx
        .recv()
        .map_err(|e| anyhow::anyhow!("Tokio bootstrap failed: {e}"))??;

    let quit_on_exit = quit_notify.clone();
    let app = gpui_platform::application().with_assets(agent_screen_ui_common::theme::Assets);
    app.run(move |cx: &mut App| {
        let shell = Arc::new(Mutex::new(ShellWindows {
            coord: coord.clone(),
            desktop: desktop.clone(),
            tray_tx: tray_tx.clone(),
            viewers: HashMap::new(),
            peek: None,
            card: None,
            recordings_win: None,
            status_win: None,
            last_auto_prompt: None,
        }));

        let shell_cmds = shell.clone();
        cx.spawn(async move |cx| {
            while let Some(cmd) = cmd_rx.next().await {
                match cmd {
                    UiCommand::ShowSurface { params, reply } => {
                        let res = cx.update(|cx| {
                            shell_cmds.lock().unwrap().handle_debug_show(&params, cx)
                        });
                        let _ = reply.send(res);
                    }
                    UiCommand::DebugViewer { display_id, reply } => {
                        let res = cx.update(|cx| {
                            let mut sh = shell_cmds.lock().unwrap();
                            let handle = sh.open_viewer(display_id, cx)?;
                            handle
                                .update(cx, |v, window, _| v.debug_snapshot(window))
                                .map_err(|e| ControlError::failed(e.to_string()))
                        });
                        let _ = reply.send(res);
                    }
                    UiCommand::DebugViewerInput { params, reply } => {
                        let res = cx.update(|cx| {
                            let id = params
                                .get("displayId")
                                .and_then(Value::as_u64)
                                .unwrap_or(0) as DisplayId;
                            if id == 0 {
                                return Err(ControlError::invalid("displayId is required"));
                            }
                            let mut sh = shell_cmds.lock().unwrap();
                            let handle = sh.open_viewer(id, cx)?;
                            handle
                                .update(cx, |v, window, cx| v.debug_input(&params, window, cx))
                                .map_err(|e| ControlError::failed(e.to_string()))
                        });
                        let _ = reply.send(res);
                    }
                    UiCommand::DebugCard { reply } => {
                        let res = cx.update(|cx| {
                            let mut sh = shell_cmds.lock().unwrap();
                            let handle = match sh.card {
                                Some(h) if h.read_with(cx, |_, _| ()).is_ok() => h,
                                _ => sh.open_card(None, cx)?,
                            };
                            handle
                                .update(cx, |c, window, cx| c.debug_snapshot(window, cx))
                                .map_err(|e| ControlError::failed(e.to_string()))
                        });
                        let _ = reply.send(res);
                    }
                    UiCommand::DebugIcon { reply } => {
                        let res: Result<Value, ControlError> = cx.update(|cx| {
                            let sh = shell_cmds.lock().unwrap();
                            let snaps = sh.coord.ui_snapshots();
                            let asking = snaps
                                .iter()
                                .filter(|s| s.help.is_some() || s.control_req.is_some())
                                .count();
                            let state = if asking > 0 {
                                "asking"
                            } else if !snaps.is_empty() {
                                "working"
                            } else {
                                "idle"
                            };
                            let status_open = sh
                                .status_win
                                .is_some_and(|w| w.read_with(cx, |_, _| ()).is_ok());
                            Ok(json!({
                                "state": state,
                                "askingCount": asking,
                                "sniOnline": sh.desktop.sni_online.load(Ordering::Acquire),
                                "statusWindowOpen": status_open,
                                "notifyActions": sh.desktop.notify_actions.load(Ordering::Acquire),
                                "notifyInlineReply": sh.desktop.notify_inline_reply.load(Ordering::Acquire),
                            }))
                        });
                        let _ = reply.send(res);
                    }
                    UiCommand::Attention {
                        display_id,
                        title,
                        body,
                    } => {
                        cx.update(|cx| {
                            let mut sh = shell_cmds.lock().unwrap();
                            sh.desktop.send_attention_notification(
                                &sh.coord.tokio_handle(),
                                display_id,
                                title,
                                body,
                            );
                            let _ = sh.open_card(Some(display_id), cx);
                        });
                    }
                    UiCommand::Quit => {
                        cx.update(|cx| cx.quit());
                        break;
                    }
                }
            }
        })
        .detach();

        let shell_tray = shell.clone();
        cx.spawn(async move |cx| {
            while let Some(action) = tray_rx.next().await {
                cx.update(|cx| {
                    let mut sh = shell_tray.lock().unwrap();
                    match action {
                        TrayAction::TogglePeek => {
                            if sh
                                .peek
                                .is_some_and(|(h, _)| h.read_with(cx, |_, _| ()).is_ok())
                            {
                                sh.hide_peek(cx);
                            } else {
                                let _ = sh.open_peek(cx);
                            }
                        }
                        TrayAction::OpenRecordings => {
                            let _ = sh.open_recordings(cx);
                        }
                        TrayAction::OpenViewer(id) => {
                            let _ = sh.open_viewer(id, cx);
                        }
                        TrayAction::ToggleTakeover(id) => {
                            let cur = sh
                                .coord
                                .ui_snapshot_of(id)
                                .is_some_and(|s| s.takeover);
                            sh.coord.ui_set_takeover(id, !cur);
                            if !cur {
                                let _ = sh.open_viewer(id, cx);
                            }
                        }
                        TrayAction::OpenHelpReply(id) => {
                            if let Ok(handle) = sh.open_card(Some(id), cx) {
                                let _ = handle.update(cx, |card, window, cx| {
                                    card.open_reply(id, None, true, window, cx);
                                });
                            }
                        }
                        TrayAction::Quit => {
                            cx.quit();
                        }
                    }
                });
            }
        })
        .detach();

        let shell_sup = shell.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.update(|cx| {
                    let mut sh = shell_sup.lock().unwrap();
                    let snaps = sh.coord.ui_snapshots();
                    sh.desktop.sync_snapshots(&sh.coord.tokio_handle(), &snaps);

                    let sni = sh.desktop.sni_online.load(Ordering::Acquire);
                    if !sni && !snaps.is_empty() {
                        let _ = sh.ensure_status_window(cx);
                    } else if snaps.is_empty() || sni {
                        sh.close_status_window(cx);
                    }

                    if let Some(asking) = snaps.iter().find(|s| {
                        s.help.as_ref().is_some_and(|h| !h.put_off) || s.control_req.is_some()
                    }) {
                        let req_id = asking
                            .help
                            .as_ref()
                            .map(|h| h.request_id)
                            .or_else(|| asking.control_req.as_ref().map(|c| c.request_id))
                            .unwrap_or(0);
                        if sh.last_auto_prompt != Some((asking.id, req_id)) {
                            sh.last_auto_prompt = Some((asking.id, req_id));
                            let _ = sh.open_card(Some(asking.id), cx);
                        }
                    }
                });
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
        })
        .detach();
        let _ = Entity::<ViewerView>::downgrade;
    });

    quit_on_exit.notify_waiters();
    std::thread::sleep(Duration::from_millis(80));
    Ok(())
}
