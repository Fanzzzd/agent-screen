//! Host desktop integration over the session D-Bus:
//! - `StatusNotifierItem` tray via `ksni` with runtime `StatusNotifierWatcher`
//!   availability tracking so GNOME without AppIndicator falls back to the
//!   compact GPUI status window.
//! - Freedesktop `org.freedesktop.Notifications` client with dynamic
//!   `inline-reply` and `actions` capability probing.

use crate::coordinator::{Coordinator, UiDisplaySnapshot};
use agent_screen_model::model::DisplayId;
use agent_screen_model::request::Answer;
use agent_screen_ui_common::i18n::t;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use ksni::TrayMethods;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use zbus::zvariant::Value as ZValue;

#[derive(Clone, Debug)]
pub enum TrayAction {
    TogglePeek,
    OpenViewer(DisplayId),
    ToggleTakeover(DisplayId),
    OpenHelpReply(DisplayId),
    Quit,
}

pub struct HostDesktop {
    pub sni_online: Arc<AtomicBool>,
    pub notify_actions: AtomicBool,
    pub notify_inline_reply: AtomicBool,
    tray_handle: Mutex<Option<ksni::Handle<AgentScreenTray>>>,
    pending_notifications: Arc<Mutex<HashMap<u32, DisplayId>>>,
    dbus_conn: Mutex<Option<zbus::Connection>>,
}

struct AgentScreenTray {
    sni_online: Arc<AtomicBool>,
    tx: UnboundedSender<TrayAction>,
    displays: Vec<TrayDisplayItem>,
    asking_count: usize,
}

#[derive(Clone)]
struct TrayDisplayItem {
    id: DisplayId,
    label: String,
    takeover: bool,
}

impl ksni::Tray for AgentScreenTray {
    fn id(&self) -> String {
        match agent_screen_mcp::endpoint::instance() {
            Some(inst) => format!("agent-screen-{inst}"),
            None => "agent-screen".into(),
        }
    }

    fn title(&self) -> String {
        "Agent Screen".into()
    }

    fn status(&self) -> ksni::Status {
        if self.asking_count > 0 {
            ksni::Status::NeedsAttention
        } else if !self.displays.is_empty() {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![build_tray_icon(
            self.asking_count > 0,
            !self.displays.is_empty(),
        )]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let subtitle = if self.asking_count > 0 {
            t("Waiting for your input", "等待你的处理").to_string()
        } else if self.displays.is_empty() {
            t("Idle", "空闲").to_string()
        } else {
            agent_screen_ui_common::tf_ui!(
                "{} active display(s)",
                "{} 台活动显示器",
                self.displays.len()
            )
        };
        ksni::ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title: "Agent Screen".into(),
            description: subtitle,
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.unbounded_send(TrayAction::TogglePeek);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let mut items: Vec<MenuItem<Self>> = Vec::new();
        items.push(
            StandardItem {
                label: t("Show Peek", "显示概览").into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.unbounded_send(TrayAction::TogglePeek);
                }),
                ..Default::default()
            }
            .into(),
        );
        if !self.displays.is_empty() {
            items.push(MenuItem::Separator);
            for d in &self.displays {
                let id = d.id;
                let open_label = format!("{}: {}", t("Open Viewer", "打开观察窗"), d.label);
                items.push(
                    StandardItem {
                        label: open_label,
                        activate: Box::new(move |tray: &mut Self| {
                            let _ = tray.tx.unbounded_send(TrayAction::OpenViewer(id));
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
                let toggle_label = if d.takeover {
                    format!("  {}", t("Hand Control Back", "交回控制权"))
                } else {
                    format!("  {}", t("Take Over", "接管"))
                };
                items.push(
                    StandardItem {
                        label: toggle_label,
                        activate: Box::new(move |tray: &mut Self| {
                            let _ = tray.tx.unbounded_send(TrayAction::ToggleTakeover(id));
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: t("Quit Agent Screen", "退出 Agent Screen").into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.unbounded_send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }

    fn watcher_online(&self) {
        self.sni_online.store(true, Ordering::Release);
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        self.sni_online.store(false, Ordering::Release);
        true
    }
}

fn build_tray_icon(asking: bool, active: bool) -> ksni::Icon {
    let w = 22i32;
    let h = 22i32;
    let (r, g, b) = if asking {
        (0xffu8, 0x9f, 0x0a)
    } else if active {
        (0x0a, 0x84, 0xff)
    } else {
        (0x98, 0x98, 0x9d)
    };
    let mut argb = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let border = (2..20).contains(&x)
                && (4..18).contains(&y)
                && (x == 2 || x == 19 || y == 4 || y == 17);
            let dot = (9..13).contains(&x) && (9..13).contains(&y);
            if border || dot {
                argb.extend_from_slice(&[255, r, g, b]);
            } else {
                argb.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    ksni::Icon {
        width: w,
        height: h,
        data: argb,
    }
}

impl HostDesktop {
    pub fn start(
        tokio_rt: &tokio::runtime::Handle,
        coord: Arc<Coordinator>,
        tx: UnboundedSender<TrayAction>,
    ) -> Arc<Self> {
        let sni_online = Arc::new(AtomicBool::new(false));
        let desktop = Arc::new(Self {
            sni_online: sni_online.clone(),
            notify_actions: AtomicBool::new(false),
            notify_inline_reply: AtomicBool::new(false),
            tray_handle: Mutex::new(None),
            pending_notifications: Arc::new(Mutex::new(HashMap::new())),
            dbus_conn: Mutex::new(None),
        });

        let d_clone = desktop.clone();
        tokio_rt.spawn(async move {
            // Only connect when DBUS_SESSION_BUS_ADDRESS is present in this process's env.
            let Ok(bus_addr) = std::env::var("DBUS_SESSION_BUS_ADDRESS") else {
                return;
            };
            if bus_addr.trim().is_empty() {
                return;
            }

            if let Ok(builder) = zbus::connection::Builder::address(bus_addr.as_str())
                && let Ok(conn) = builder.build().await
            {
                let has_watcher = probe_sni_watcher(&conn).await;
                d_clone.sni_online.store(has_watcher, Ordering::Release);
                let (actions, inline_reply) = probe_notification_caps(&conn).await;
                d_clone.notify_actions.store(actions, Ordering::Release);
                d_clone
                    .notify_inline_reply
                    .store(inline_reply, Ordering::Release);

                spawn_notification_signal_listener(
                    conn.clone(),
                    d_clone.pending_notifications.clone(),
                    coord,
                    tx.clone(),
                );
                *d_clone.dbus_conn.lock().unwrap() = Some(conn);
            }

            let tray = AgentScreenTray {
                sni_online: d_clone.sni_online.clone(),
                tx,
                displays: Vec::new(),
                asking_count: 0,
            };
            if let Ok(handle) = tray
                .disable_dbus_name(true)
                .assume_sni_available(true)
                .spawn()
                .await
            {
                *d_clone.tray_handle.lock().unwrap() = Some(handle);
            } else {
                d_clone.sni_online.store(false, Ordering::Release);
            }
        });

        desktop
    }

    pub fn sync_snapshots(&self, tokio_rt: &tokio::runtime::Handle, snaps: &[UiDisplaySnapshot]) {
        let Some(handle) = self.tray_handle.lock().unwrap().clone() else {
            return;
        };
        let items: Vec<TrayDisplayItem> = snaps
            .iter()
            .map(|s| TrayDisplayItem {
                id: s.id,
                label: s.label.clone(),
                takeover: s.takeover,
            })
            .collect();
        let asking = snaps
            .iter()
            .filter(|s| s.help.is_some() || s.control_req.is_some())
            .count();
        tokio_rt.spawn(async move {
            let _ = handle
                .update(move |tray| {
                    tray.displays = items;
                    tray.asking_count = asking;
                })
                .await;
        });
    }

    pub fn send_attention_notification(
        &self,
        tokio_rt: &tokio::runtime::Handle,
        display_id: DisplayId,
        summary: String,
        body: String,
    ) {
        let Some(conn) = self.dbus_conn.lock().unwrap().clone() else {
            return;
        };
        let supports_actions = self.notify_actions.load(Ordering::Acquire);
        let supports_inline = self.notify_inline_reply.load(Ordering::Acquire);
        let pending = self.pending_notifications.clone();

        tokio_rt.spawn(async move {
            let mut actions: Vec<&str> = Vec::new();
            if supports_actions {
                actions.extend_from_slice(&[
                    "take-over",
                    t("Take Over", "接管"),
                    "continue",
                    t("Continue", "继续"),
                ]);
                if supports_inline {
                    actions.extend_from_slice(&["inline-reply", t("Reply", "回复")]);
                } else {
                    actions.extend_from_slice(&["reply", t("Reply", "回复")]);
                }
            }
            let hints: HashMap<&str, ZValue<'_>> = HashMap::new();
            let args = (
                "Agent Screen",
                0u32,
                "",
                summary.as_str(),
                body.as_str(),
                actions,
                hints,
                8000i32,
            );
            if let Ok(msg) = conn
                .call_method(
                    Some("org.freedesktop.Notifications"),
                    "/org/freedesktop/Notifications",
                    Some("org.freedesktop.Notifications"),
                    "Notify",
                    &args,
                )
                .await
                && let Ok(nid) = msg.body().deserialize::<u32>()
            {
                pending.lock().unwrap().insert(nid, display_id);
            }
        });
    }
}

async fn probe_sni_watcher(conn: &zbus::Connection) -> bool {
    let args = ("org.kde.StatusNotifierWatcher",);
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &args,
    )
    .await
    .ok()
    .and_then(|m| m.body().deserialize::<bool>().ok())
    .unwrap_or(false)
}

async fn probe_notification_caps(conn: &zbus::Connection) -> (bool, bool) {
    let Ok(msg) = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "GetCapabilities",
            &(),
        )
        .await
    else {
        return (false, false);
    };
    let caps: Vec<String> = msg.body().deserialize().unwrap_or_default();
    let actions = caps.iter().any(|c| c == "actions");
    let inline = caps.iter().any(|c| c == "inline-reply");
    (actions, inline)
}

fn spawn_notification_signal_listener(
    conn: zbus::Connection,
    pending: Arc<Mutex<HashMap<u32, DisplayId>>>,
    coord: Arc<Coordinator>,
    tx: UnboundedSender<TrayAction>,
) {
    tokio::spawn(async move {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.Notifications")
            .ok()
            .map(|b| b.build());
        let mut stream = match rule {
            Some(r) => match zbus::MessageStream::for_match_rule(r, &conn, None).await {
                Ok(s) => s,
                Err(_) => return,
            },
            None => return,
        };
        while let Some(Ok(msg)) = stream.next().await {
            let header = msg.header();
            let Some(member) = header.member().map(|m| m.as_str()) else {
                continue;
            };
            if member == "ActionInvoked"
                && let Ok((nid, key)) = msg.body().deserialize::<(u32, String)>()
                && let Some(&display_id) = pending.lock().unwrap().get(&nid)
            {
                match key.as_str() {
                    "take-over" => {
                        coord.ui_set_takeover(display_id, true);
                        let _ = tx.unbounded_send(TrayAction::OpenViewer(display_id));
                    }
                    "continue" => {
                        let _ = coord.ui_answer_help(display_id, None, Answer::Continue);
                    }
                    "reply" | "default" => {
                        let _ = tx.unbounded_send(TrayAction::OpenHelpReply(display_id));
                    }
                    _ => {}
                }
            } else if member == "NotificationReplied"
                && let Ok((nid, text)) = msg.body().deserialize::<(u32, String)>()
                && let Some(&display_id) = pending.lock().unwrap().get(&nid)
            {
                let _ = coord.ui_answer_help(display_id, None, Answer::Reply(text));
            }
        }
    });
}
