//! GPUI Help Card & Control Request surface for Linux:
//! - Displays the live thumbnail (`FrameSlot` with `window.drop_image`), the
//!   agent's reason and numbered steps, and the 3 Linux Help Request answers:
//!   `Take Over`, `Continue`, and `Reply` (inline `ReplyField`), plus `Later`.
//! - Never offers `Give a Window` because isolated Wayland compositors do not
//!   migrate host windows (`host_window_handover: false`, ADR 0020).

use super::frame::FrameSlot;
use crate::coordinator::{Coordinator, UiDisplaySnapshot};
use agent_screen_app_core::ports::RawRgbaFrame;
use agent_screen_model::model::DisplayId;
use agent_screen_model::request::{Answer, REPLY_CHARS, RequestId};
use agent_screen_ui_common::i18n::t;
use agent_screen_ui_common::reply_field::{ReplyEvent, ReplyField};
use agent_screen_ui_common::{card_layout, pointer, theme};
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, MouseButton, ObjectFit,
    ParentElement, Render, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder, px,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct ReplyDraft {
    display_id: DisplayId,
    request_id: RequestId,
    field: Entity<ReplyField>,
}

pub struct HelpCardView {
    pub display_id: Option<DisplayId>,
    coord: Arc<Coordinator>,
    frame_slot: FrameSlot,
    pending_raw: Option<RawRgbaFrame>,
    snapshot: Option<UiDisplaySnapshot>,
    reply: Option<ReplyDraft>,
}

impl HelpCardView {
    pub fn new(
        display_id: Option<DisplayId>,
        coord: Arc<Coordinator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshot = display_id.and_then(|id| coord.ui_snapshot_of(id));
        let view = Self {
            display_id,
            coord,
            frame_slot: FrameSlot::default(),
            pending_raw: None,
            snapshot,
            reply: None,
        };
        view.spawn_poller(cx);
        view
    }

    fn spawn_poller(&self, cx: &mut Context<Self>) {
        let coord = self.coord.clone();
        let rt = coord.tokio_handle();
        cx.spawn(async move |this, cx| {
            loop {
                let target_id = this.read_with(cx, |v, _| v.display_id).ok().flatten();
                let (tx, rx) = futures::channel::oneshot::channel();
                let c = coord.clone();
                rt.spawn(async move {
                    let snap = match target_id {
                        Some(id) => c.ui_snapshot_of(id),
                        None => c.ui_snapshots().into_iter().find(|s| {
                            s.help.as_ref().is_some_and(|h| !h.put_off) || s.control_req.is_some()
                        }),
                    };
                    let raw = match &snap {
                        Some(s) => s.handle.capture_raw().await.ok(),
                        None => None,
                    };
                    let _ = tx.send((snap, raw));
                });
                let Ok((snap, raw)) = rx.await else {
                    break;
                };
                let alive = this
                    .update(cx, |v, cx| {
                        if let Some(s) = &snap {
                            v.display_id = Some(s.id);
                        }
                        v.snapshot = snap;
                        if let Some(r) = raw {
                            v.pending_raw = Some(r);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
            }
        })
        .detach();
    }

    pub fn set_display(&mut self, display_id: Option<DisplayId>, cx: &mut Context<Self>) {
        self.display_id = display_id;
        self.snapshot = display_id.and_then(|id| self.coord.ui_snapshot_of(id));
        cx.notify();
    }

    pub fn open_reply(
        &mut self,
        id: DisplayId,
        draft: Option<String>,
        focus_keys: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let snap = self.coord.ui_snapshot_of(id);
        let Some(help) = snap.as_ref().and_then(|s| s.help.as_ref()) else {
            return false;
        };
        if !help.answers {
            return false;
        }
        let req_id = help.request_id;
        self.display_id = Some(id);
        self.snapshot = snap;
        let theme = theme::theme(window);
        let field = cx.new(|cx| {
            ReplyField::new(
                t("Tell it what to do", "告诉它怎么做").into(),
                (theme.label, theme.secondary, theme.accent),
                cx,
            )
        });
        cx.subscribe_in(&field, window, move |card, _, ev, window, cx| match ev {
            ReplyEvent::Send(text) => {
                card.submit_reply(text.clone(), window, cx);
            }
            ReplyEvent::Cancel => {
                card.close_reply(cx);
            }
            ReplyEvent::Focus => {
                if let Some(r) = &card.reply {
                    window.focus(&r.field.read(cx).focus.clone(), cx);
                }
            }
        })
        .detach();
        if let Some(draft) = draft {
            field.update(cx, |f, cx| f.set_text(&draft, cx));
        }
        if focus_keys {
            window.focus(&field.read(cx).focus.clone(), cx);
        }
        self.reply = Some(ReplyDraft {
            display_id: id,
            request_id: req_id,
            field,
        });
        cx.notify();
        true
    }

    pub fn close_reply(&mut self, cx: &mut Context<Self>) {
        if self.reply.take().is_some() {
            cx.notify();
        }
    }

    pub fn submit_reply(
        &mut self,
        text: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() || text.chars().count() > REPLY_CHARS {
            return false;
        }
        let Some((id, req_id)) = self.reply.as_ref().map(|r| (r.display_id, r.request_id)) else {
            return false;
        };
        if self
            .coord
            .ui_answer_help(id, Some(req_id), Answer::Reply(text))
            .is_ok()
        {
            self.reply = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    pub fn cleanup(&mut self, window: &mut Window) {
        self.frame_slot.clear(window);
    }

    pub fn debug_snapshot(&self, window: &Window, cx: &Context<Self>) -> Value {
        let highlighted = self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.help.is_some() || s.control_req.is_some());
        let mut buttons = serde_json::Map::new();
        if let Some(snap) = &self.snapshot {
            if let Some(help) = &snap.help {
                buttons.insert("take-over".into(), json!([14, 240, 140, 30]));
                buttons.insert("later".into(), json!([162, 240, 140, 30]));
                if help.answers {
                    buttons.insert("continue".into(), json!([14, 278, 140, 30]));
                    buttons.insert("reply".into(), json!([162, 278, 140, 30]));
                }
            } else if snap.control_req.is_some() {
                buttons.insert("let-continue".into(), json!([14, 120, 140, 30]));
                buttons.insert("keep-control".into(), json!([162, 120, 140, 30]));
            }
        }
        let reply_json = self.reply.as_ref().map(|r| {
            let chars = r.field.read(cx).content.chars().count();
            json!({
                "displayId": r.display_id,
                "requestId": r.request_id,
                "chars": chars,
            })
        });
        json!({
            "width": card_layout::WIDTH,
            "displayId": self.display_id,
            "highlighted": highlighted,
            "buttons": buttons,
            "reply": reply_json,
            "key": window.is_window_active(),
            "residentTextures": self.frame_slot.resident_textures(),
        })
    }
}

impl Render for HelpCardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(raw) = self.pending_raw.take() {
            self.frame_slot.swap(&raw, window);
        }
        let theme = theme::theme(window);
        let Some(snap) = self.snapshot.clone() else {
            return div()
                .id("help-card-empty")
                .size_full()
                .p(px(card_layout::PADDING))
                .bg(theme.card)
                .text_color(theme.secondary)
                .text_xs()
                .child(t("No open requests", "当前没有待处理请求"));
        };

        let asking = snap.help.is_some() || snap.control_req.is_some();
        let bg = if asking {
            theme.asking_card
        } else {
            theme.card
        };
        let border = if asking {
            theme.asking_border
        } else {
            theme.card_border
        };
        let (pic_w, pic_h) = card_layout::picture((snap.width as f64, snap.height as f64));
        let id = snap.id;

        div()
            .id("help-card-root")
            .size_full()
            .p(px(card_layout::PADDING))
            .flex()
            .flex_col()
            .gap(px(card_layout::GAP))
            .bg(bg)
            .border_1()
            .border_color(border)
            .text_color(theme.label)
            .child(div().text_sm().child(if snap.control_req.is_some() {
                agent_screen_ui_common::tf_ui!(
                    "{} wants control back",
                    "{} 请求收回控制权",
                    snap.agent_name
                )
            } else {
                agent_screen_ui_common::tf_ui!("{} needs help", "{} 需要协助", snap.agent_name)
            }))
            .child(
                div().text_xs().text_color(theme.secondary).child(
                    snap.help
                        .as_ref()
                        .map(|h| h.reason.clone())
                        .or_else(|| snap.control_req.as_ref().map(|c| c.reason.clone()))
                        .unwrap_or_else(|| snap.label.clone()),
                ),
            )
            .when_some(
                snap.help
                    .as_ref()
                    .filter(|h| !h.steps.is_empty())
                    .map(|h| h.steps.clone()),
                |el, steps| {
                    el.child(div().flex().flex_col().gap(px(2.)).children(
                        steps.into_iter().enumerate().map(|(idx, step)| {
                            div()
                                .text_xs()
                                .text_color(theme.secondary)
                                .child(format!("{}. {step}", idx + 1))
                        }),
                    ))
                },
            )
            .when(snap.help.is_some(), |el| {
                el.child(
                    div()
                        .w(px(pic_w))
                        .h(px(pic_h))
                        .relative()
                        .rounded(px(9.))
                        .bg(theme.screen_empty)
                        .overflow_hidden()
                        .child(match self.frame_slot.image() {
                            Some(im) => img(im)
                                .object_fit(ObjectFit::Contain)
                                .size_full()
                                .into_any_element(),
                            None => div().size_full().into_any_element(),
                        })
                        .child(theme::round_corners(9., bg))
                        .child(pointer::layer(
                            id,
                            snap.pointer,
                            (snap.width as f64, snap.height as f64),
                            theme.accent,
                        )),
                )
            })
            .when_some(self.reply.as_ref().map(|r| r.field.clone()), |el, field| {
                let chars = field.read(cx).content.chars().count();
                let over = chars > REPLY_CHARS;
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(7.))
                        .bg(theme.control)
                        .child(field.clone())
                        .when(over, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.red)
                                    .child(format!("{chars}/{REPLY_CHARS}")),
                            )
                        })
                        .child(
                            div()
                                .id("card-reply-send")
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(5.))
                                .cursor_pointer()
                                .bg(theme.accent)
                                .text_color(gpui::white())
                                .text_xs()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |card, _, window, cx| {
                                        let txt = field.read(cx).content.clone();
                                        card.submit_reply(txt, window, cx);
                                    }),
                                )
                                .child(t("Send", "发送")),
                        ),
                )
            })
            .when_some(snap.help.clone(), |el, help| {
                let c_take = self.coord.clone();
                let c_later = self.coord.clone();
                let c_cont = self.coord.clone();
                let req_id = help.request_id;
                el.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .id("card-btn-take-over")
                                .flex_1()
                                .py(px(5.))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .bg(theme.accent)
                                .text_color(gpui::white())
                                .text_xs()
                                .flex()
                                .justify_center()
                                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                    c_take.ui_set_takeover(id, true);
                                })
                                .child(t("Take Over", "接管")),
                        )
                        .child(
                            div()
                                .id("card-btn-later")
                                .flex_1()
                                .py(px(5.))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .bg(theme.control)
                                .text_xs()
                                .flex()
                                .justify_center()
                                .on_mouse_down(MouseButton::Left, move |_, window, _| {
                                    c_later.ui_put_off_help(id);
                                    window.remove_window();
                                })
                                .child(t("Later", "稍后")),
                        ),
                )
                .when(help.answers, |el2| {
                    el2.child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                div()
                                    .id("card-btn-continue")
                                    .flex_1()
                                    .py(px(5.))
                                    .rounded(px(7.))
                                    .cursor_pointer()
                                    .bg(theme.control)
                                    .text_xs()
                                    .flex()
                                    .justify_center()
                                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                        let _ = c_cont.ui_answer_help(
                                            id,
                                            Some(req_id),
                                            Answer::Continue,
                                        );
                                    })
                                    .child(t("Continue", "继续")),
                            )
                            .child(
                                div()
                                    .id("card-btn-reply")
                                    .flex_1()
                                    .py(px(5.))
                                    .rounded(px(7.))
                                    .cursor_pointer()
                                    .bg(theme.control)
                                    .text_xs()
                                    .flex()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |card, _, window, cx| {
                                            card.open_reply(id, None, true, window, cx);
                                        }),
                                    )
                                    .child(t("Reply", "回复")),
                            ),
                    )
                })
            })
            .when_some(snap.control_req, |el, creq| {
                let c_yes = self.coord.clone();
                let c_keep = self.coord.clone();
                let req_id = creq.request_id;
                el.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .id("card-btn-let-continue")
                                .flex_1()
                                .py(px(5.))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .bg(theme.accent)
                                .text_color(gpui::white())
                                .text_xs()
                                .flex()
                                .justify_center()
                                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                    let _ = c_yes.ui_answer_control(id, Some(req_id), true);
                                })
                                .child(t("Let It Continue", "让它继续")),
                        )
                        .child(
                            div()
                                .id("card-btn-keep-control")
                                .flex_1()
                                .py(px(5.))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .bg(theme.control)
                                .text_xs()
                                .flex()
                                .justify_center()
                                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                    let _ = c_keep.ui_answer_control(id, Some(req_id), false);
                                })
                                .child(t("Keep Control", "保持控制")),
                        ),
                )
            })
    }
}
