//! Edge Peek surface (`WindowKind::LayerShell` on Sway/Hyprland/KDE Plasma,
//! falling back to `WindowKind::PopUp` on GNOME Mutter) and the compact
//! GNOME Status Window shown when `StatusNotifierWatcher` is absent.

use super::tray_and_notify::TrayAction;
use crate::coordinator::{Coordinator, UiDisplaySnapshot};
use agent_screen_ui_common::i18n::t;
use agent_screen_ui_common::theme;
use futures::channel::mpsc::UnboundedSender;
use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Render, Styled, Window,
    div, prelude::FluentBuilder, px,
};
use std::sync::Arc;
use std::time::Duration;

pub struct PeekView {
    action_tx: UnboundedSender<TrayAction>,
    snapshots: Vec<UiDisplaySnapshot>,
    pub layer_shell_active: bool,
}

impl PeekView {
    pub fn new(
        coord: Arc<Coordinator>,
        action_tx: UnboundedSender<TrayAction>,
        layer_shell_active: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshots = coord.ui_snapshots();
        let view = Self {
            action_tx,
            snapshots,
            layer_shell_active,
        };
        cx.spawn(async move |this, cx| {
            loop {
                let snaps = coord.ui_snapshots();
                let alive = this
                    .update(cx, |v, cx| {
                        v.snapshots = snaps;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
            }
        })
        .detach();
        view
    }
}

impl Render for PeekView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        div()
            .id("peek-root")
            .size_full()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(theme.card)
            .border_1()
            .border_color(theme.card_border)
            .text_color(theme.label)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child(if self.layer_shell_active {
                        "Agent Screen"
                    } else {
                        "Agent Screen · Peek"
                    }))
                    .child(
                        div()
                            .id("peek-close")
                            .text_xs()
                            .text_color(theme.secondary)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, window, _| {
                                window.remove_window();
                            })
                            .child(t("Hide", "隐藏")),
                    ),
            )
            .when(self.snapshots.is_empty(), |el| {
                el.child(div().text_xs().text_color(theme.secondary).child(t(
                    "No active Agent Displays",
                    "当前没有活动的 Agent Display",
                )))
            })
            .children(self.snapshots.iter().map(|s| {
                let id = s.id;
                let tx_open = self.action_tx.clone();
                let tx_take = self.action_tx.clone();
                let badge = if s.ended {
                    t("Review", "待审阅")
                } else if s.takeover {
                    t("User Control", "已接管")
                } else if s.help.is_some() || s.control_req.is_some() {
                    t("Needs Help", "等待处理")
                } else {
                    t("Working", "运行中")
                };
                div()
                    .id(("peek-row", id))
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(theme.control)
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        let _ = tx_open.unbounded_send(TrayAction::OpenViewer(id));
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_xs().child(s.label.clone()))
                            .child(div().text_xs().text_color(theme.secondary).child(badge)),
                    )
                    .when(!s.ended, |row| {
                        let takeover = s.takeover;
                        row.child(
                            div()
                                .id(("peek-takeover", id))
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(6.))
                                .bg(if takeover { theme.orange } else { theme.accent })
                                .text_color(gpui::white())
                                .text_xs()
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    let _ = tx_take.unbounded_send(TrayAction::ToggleTakeover(id));
                                    cx.stop_propagation();
                                })
                                .child(if takeover {
                                    t("Hand Back", "交回")
                                } else {
                                    t("Take Over", "接管")
                                }),
                        )
                    })
            }))
    }
}

/// Compact persistent status window shown on vanilla GNOME (or any Wayland
/// session without `StatusNotifierWatcher`) whenever displays are active or
/// awaiting review.
pub struct StatusWindowView {
    action_tx: UnboundedSender<TrayAction>,
    snapshots: Vec<UiDisplaySnapshot>,
}

impl StatusWindowView {
    pub fn new(
        coord: Arc<Coordinator>,
        action_tx: UnboundedSender<TrayAction>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshots = coord.ui_snapshots();
        let view = Self {
            action_tx,
            snapshots,
        };
        cx.spawn(async move |this, cx| {
            loop {
                let snaps = coord.ui_snapshots();
                let alive = this
                    .update(cx, |v, cx| {
                        v.snapshots = snaps;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
        })
        .detach();
        view
    }
}

impl Render for StatusWindowView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let count = self.snapshots.len();
        let asking = self
            .snapshots
            .iter()
            .filter(|s| s.help.is_some() || s.control_req.is_some())
            .count();
        let tx_peek = self.action_tx.clone();
        let first_id = self.snapshots.first().map(|s| s.id);
        let tx_view = self.action_tx.clone();

        div()
            .id("gnome-status-root")
            .size_full()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .bg(if asking > 0 {
                theme.asking_card
            } else {
                theme.card
            })
            .text_color(theme.label)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_xs().child("Agent Screen"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(if asking > 0 {
                                agent_screen_ui_common::tf_ui!(
                                    "{asking} waiting · {count} display(s)",
                                    "{asking} 待处理 · {count} 台显示器"
                                )
                            } else {
                                agent_screen_ui_common::tf_ui!(
                                    "{count} display(s)",
                                    "{count} 台显示器"
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("status-open-peek")
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .bg(theme.control)
                            .text_xs()
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                let _ = tx_peek.unbounded_send(TrayAction::TogglePeek);
                            })
                            .child(t("Peek", "概览")),
                    )
                    .when_some(first_id, |el, id| {
                        el.child(
                            div()
                                .id("status-open-viewer")
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(6.))
                                .cursor_pointer()
                                .bg(theme.accent)
                                .text_color(gpui::white())
                                .text_xs()
                                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                    let _ = tx_view.unbounded_send(TrayAction::OpenViewer(id));
                                })
                                .child(t("Viewer", "观察窗")),
                        )
                    }),
            )
    }
}
