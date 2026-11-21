//! The Help Card: drops from the menu bar icon while a Help Request is
//! open, with the display's picture, the agent's reason, and its answers:
//! Take Over, Continue, Reply, Give a Window, or Later (ADR 0015). An agent
//! whose MCP server predates those answers gets Take Over and Later only.
//! Reply opens a one-line field in the card, the only time the card takes
//! the keyboard; it gives the keyboard back whenever the field closes. For a Window Request it names the window the agent asks for,
//! with Hand Over or Keep. For a Control Request, during a Takeover, it
//! gives the agent's reason, with Let … Continue or Keep Control. For the
//! Input Queue (ADR 0013) it asks to use the user's mouse, with the path
//! of each waiting drag, or the point of each click, drawn on its
//! display's picture and what approval covers, then shows the step that
//! runs, or the idle grant, with Stop.

use super::pointer;
pub use super::queue_card::{
    Phase, clock, preview_running, preview_steps, queue_height, queue_title, queue_view,
    request_text, tabular,
};
use super::reply_field::{ReplyEvent, ReplyField};
use super::shell::{Shell, tell};
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::i18n::t;
use crate::mac::native;
use crate::model::DisplayId;
use agent_screen_model::held::Place;
use agent_screen_model::request::{
    Answer, Kind as RequestKind, REPLY_CHARS, RequestId, Unanswered,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gpui::{
    AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, MouseButton, ObjectFit,
    ParentElement, Pixels, Render, Role, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, canvas, div, prelude::FluentBuilder, px, surface, svg,
};

pub const WIDTH: f32 = 320.;
pub(super) const PADDING: f32 = 14.;
pub(super) const GAP: f32 = 8.;
pub(super) const TITLE: f32 = 20.;
/// Two lines of the reason; the tooltip has all of it.
pub(super) const REASON: f32 = 36.;
const LABEL: f32 = 16.;
pub(super) const BUTTONS: f32 = 30.;
const MAX_PICTURE: f32 = 184.;
pub(super) const RADIUS: f32 = 18.;

/// The card's height for a Window Request, which has no picture.
pub const REQUEST_HEIGHT: f32 = PADDING * 2. + TITLE + REASON + LABEL + BUTTONS + GAP * 3.;

/// The card's height for a display of `size`: everything but the picture is
/// fixed, so the panel can be placed before it paints. A Help Request's
/// answers take two rows of buttons, or one, Take Over and Later, for an
/// agent that understands no others.
pub fn height(size: (f64, f64), answers: bool) -> f32 {
    let rows = if answers { 2. } else { 1. };
    PADDING * 2. + TITLE + REASON + LABEL + BUTTONS * rows + GAP * (3. + rows) + picture(size).1
}

/// The picture's size inside the card, aspect-fit.
pub(super) fn picture(size: (f64, f64)) -> (f32, f32) {
    let width = WIDTH - PADDING * 2.;
    let fit = (width / size.0 as f32).min(MAX_PICTURE / size.1 as f32);
    (size.0 as f32 * fit, size.1 as f32 * fit)
}

pub struct HelpCard {
    pub(super) shell: WeakEntity<Shell>,
    pub(super) core: Entity<Core>,
    pub(super) streams: Entity<Streams>,
    /// The display whose Help Request the card shows.
    pub display: Option<DisplayId>,
    pub input_queue: bool,
    /// The reply field, while the user types an answer.
    reply: Option<Reply>,
}

/// A reply being typed, for one Help Request: a request the agent replaced
/// since takes the draft with it.
struct Reply {
    display: DisplayId,
    request: RequestId,
    field: Entity<ReplyField>,
}

impl HelpCard {
    pub fn new(
        shell: WeakEntity<Shell>,
        core: Entity<Core>,
        streams: Entity<Streams>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&core, |_, _, cx| cx.notify()).detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(&streams, |card: &mut HelpCard, _, frame: &NewFrame, cx| {
            if card.display == Some(frame.0) {
                cx.notify();
            }
        })
        .detach();
        HelpCard {
            shell,
            core,
            streams,
            display: None,
            input_queue: false,
            reply: None,
        }
    }

    /// Reply: open the field for display `id`'s Help Request, with the
    /// `draft` the user already typed, if any. With `keys` the card takes
    /// the keyboard, as the user's click on Reply or the field does; test
    /// hooks and a draft from a notification leave it with the user's app.
    /// False when that request is gone or offers no Reply.
    pub fn open_reply(
        &mut self,
        id: DisplayId,
        draft: Option<String>,
        keys: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(request) = help_request(&self.core, id, cx) else {
            return false;
        };
        let field = match &self.reply {
            Some(reply) if reply.display == id && reply.request == request => reply.field.clone(),
            _ => {
                self.close_reply(window, cx);
                let theme = theme::theme(window);
                let field = cx.new(|cx| {
                    ReplyField::new(
                        t("Tell it what to do", "告诉它怎么做").into(),
                        (theme.label, theme.secondary, theme.accent),
                        cx,
                    )
                });
                cx.subscribe_in(
                    &field,
                    window,
                    move |card, _, event, window, cx| match event {
                        ReplyEvent::Send(text) => card.send(text.clone(), window, cx),
                        ReplyEvent::Cancel => card.close_reply(window, cx),
                        ReplyEvent::Focus => {
                            card.open_reply(id, None, true, window, cx);
                        }
                    },
                )
                .detach();
                // The count past the limit follows the typing.
                cx.observe(&field, |_, _, cx| cx.notify()).detach();
                self.reply = Some(Reply {
                    display: id,
                    request,
                    field: field.clone(),
                });
                field
            }
        };
        if let Some(draft) = draft {
            field.update(cx, |field, cx| field.set_text(&draft, cx));
        }
        if keys {
            native::card_keys(true);
            if let Some(ns) = native::ns_window(window) {
                native::make_key(&ns);
            }
            window.focus(&field.read(cx).focus.clone(), cx);
        }
        cx.notify();
        true
    }

    /// Put the field away, draft and all, and give the keyboard back to
    /// the user's app. The Shell calls it too whenever the card hides or
    /// shows something else, which it may do without a render.
    pub fn close_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        native::card_keys(false);
        if let Some(ns) = native::ns_window(window) {
            native::give_up_keys(&ns);
        }
        if self.reply.take().is_some() {
            cx.notify();
        }
    }

    /// Send or Return: what the user typed goes to the agent. A blank or
    /// too long reply keeps the field for another try; one for a request
    /// the agent replaced since goes nowhere, and its draft goes too.
    fn send(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(why) = self.send_reply(text, window, cx) {
            window.play_system_bell();
            if why == Unanswered::NotOpen {
                self.close_reply(window, cx);
            }
        }
    }

    fn send_reply(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), Unanswered> {
        let Some((display, request)) = self.reply.as_ref().map(|r| (r.display, r.request)) else {
            return Err(Unanswered::NotOpen);
        };
        let shell = self.shell.clone();
        let answered = shell
            .update(cx, |shell, cx| {
                shell.answer_help(display, Some(request), Answer::Reply(text), cx)
            })
            .unwrap_or(Err(Unanswered::NotOpen));
        if answered.is_ok() {
            self.close_reply(window, cx);
        }
        answered
    }

    /// Test hook: open the field, type `text` into it and press Send.
    pub fn debug_reply(
        &mut self,
        id: DisplayId,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.open_reply(id, Some(text.to_string()), false, window, cx) {
            return false;
        }
        self.send_reply(text.to_string(), window, cx).is_ok()
    }

    /// Whether this card currently displays an unanswered user approval (#206).
    pub fn is_highlighted(&self, cx: &Context<Self>) -> bool {
        let core = self.core.read(cx);
        if self.input_queue {
            matches!(queue_view(core, self.display).phase, Phase::Asking { .. })
        } else {
            self.display
                .and_then(|id| core.displays.get(id))
                .is_some_and(|d| d.requests.any_open())
        }
    }

    /// Test hook: the reply field's state, and whether the card has the
    /// keyboard or may take it.
    pub fn debug_state(&self, window: &Window, cx: &Context<Self>) -> serde_json::Value {
        let key = native::ns_window(window).is_some_and(|ns| native::is_key(&ns));
        serde_json::json!({
            "key": key,
            "cardKeys": native::card_keys_on(),
            "highlighted": self.is_highlighted(cx),
            "reply": self.reply.as_ref().map(|r| serde_json::json!({
                "displayId": r.display,
                "requestId": r.request,
                "chars": r.field.read(cx).content.chars().count(),
            })),
        })
    }
}

/// The open Help Request of display `id` the card would ask about, if its
/// agent understands Reply.
fn help_request(core: &Entity<Core>, id: DisplayId, cx: &Context<HelpCard>) -> Option<RequestId> {
    let d = core.read(cx).displays.get(id)?;
    d.asking()
        .filter(|r| r.kind() == RequestKind::Help && d.requests.help_answers())
        .map(|r| r.id)
}

impl Render for HelpCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        CARD_WIDTH.with(|w| w.set(WIDTH));
        // Only what this render draws reports its bounds.
        BUTTONS_PAINTED.with(|b| b.borrow_mut().clear());
        // The field goes with the Help Request it answers.
        let stale = self.reply.as_ref().is_some_and(|r| {
            self.input_queue
                || Some(r.display) != self.display
                || help_request(&self.core, r.display, cx) != Some(r.request)
        });
        if stale {
            // Not during the render: closing gives up the keyboard, which
            // orders the card's own window out and in.
            cx.defer_in(window, |card, window, cx| card.close_reply(window, cx));
        }
        if self.input_queue {
            return self.render_queue(&theme, cx);
        }
        let reply = self
            .reply
            .as_ref()
            .filter(|_| !stale)
            .map(|r| r.field.clone());
        let core = self.core.read(cx);
        let Some(display) = self.display.and_then(|id| core.displays.get(id)) else {
            return div().into_any_element();
        };
        let id = display.id;
        let agent = display.agent_name().to_string();
        if let Some(request) = display.requests.window() {
            let app = request.app.clone();
            let window = if request.title.is_empty() {
                app.clone()
            } else {
                request.title.clone()
            };
            let (give, keep) = (self.shell.clone(), self.shell.clone());
            // Another session's window, or one waiting for review: the card
            // names who has it (ADR 0017).
            let (title, body, keep_label) = match display.requests.window_held() {
                Some(held) => {
                    let holder = held.label.clone();
                    let holder_agent = held.agent.clone();
                    let (where_it_is, keep_label) = match held.place {
                        Place::Review => (
                            tf!(
                                "It waits for your review on {holder}.",
                                "它在待查看的 {holder} 上。"
                            ),
                            t("Leave It", "保留原处").to_string(),
                        ),
                        Place::Display => (
                            tf!("{holder} is using it.", "{holder} 正在使用。"),
                            tf!("Keep with {holder_agent}", "留给 {holder_agent}"),
                        ),
                        Place::UserScreen => (
                            tf!(
                                "It is on your screen, and {holder} started it.",
                                "它在你的屏幕上，由 {holder} 打开。"
                            ),
                            tf!("Keep with {holder_agent}", "留给 {holder_agent}"),
                        ),
                    };
                    (
                        tf!("{agent} asks for {app}", "{agent} 想要 {app}"),
                        format!(
                            "{where_it_is}{}",
                            tf!(
                                " Hand Over moves “{window}” to {agent}'s display.",
                                "交给它后，“{window}”会挪到 {agent} 的显示器上。"
                            )
                        ),
                        keep_label,
                    )
                }
                None => (
                    tf!(
                        "{agent} asks for your {app} window",
                        "{agent} 想要你的 {app} 窗口"
                    ),
                    tf!(
                        "“{window}” goes to its display. Take Back in the Viewer returns it.",
                        "“{window}”会移到它的显示器上。之后可在 Viewer 中点“取回”。"
                    ),
                    t("Keep", "保留").to_string(),
                ),
            };
            return card(title, body, Badge::Approval, &theme)
                .child(
                    div()
                        .h(px(LABEL))
                        .truncate()
                        .text_size(px(11.))
                        .text_color(theme.secondary)
                        .child(display.label()),
                )
                .child(
                    div()
                        .h(px(BUTTONS))
                        .flex()
                        .gap(px(8.))
                        .child(
                            card_button(
                                "window-give",
                                t("Hand Over", "交给它"),
                                Tone::Orange,
                                &theme,
                            )
                            .flex_1()
                            .on_click(move |_, _, cx| {
                                tell(&give, cx, move |shell, cx| {
                                    shell.answer_window(id, true, cx);
                                })
                            }),
                        )
                        .child(
                            card_button("window-keep", keep_label, Tone::Plain, &theme)
                                .px(px(16.))
                                // One line, however long the holder's name.
                                .max_w(px(180.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .on_click(move |_, _, cx| {
                                    tell(&keep, cx, move |shell, cx| {
                                        shell.answer_window(id, false, cx);
                                    })
                                }),
                        ),
                )
                .into_any_element();
        }
        if let Some(reason) = display.requests.control_reason().map(String::from) {
            let (give, keep) = (self.shell.clone(), self.shell.clone());
            return card(
                tf!("{agent} asks to continue", "{agent} 想继续操作"),
                reason,
                Badge::Waiting,
                &theme,
            )
            .child(
                div()
                    .h(px(LABEL))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(theme.secondary)
                    .child(display.label()),
            )
            .child(
                div()
                    .h(px(BUTTONS))
                    .flex()
                    .gap(px(8.))
                    .child(
                        // The title names the agent: a long name here pushed
                        // Keep Control off the card.
                        card_button(
                            "control-continue",
                            t("Let It Continue", "让它继续"),
                            Tone::Orange,
                            &theme,
                        )
                        .flex_1()
                        .on_click(move |_, _, cx| {
                            tell(&give, cx, move |shell, cx| {
                                shell.answer_control(id, true, cx);
                            })
                        }),
                    )
                    .child(
                        card_button(
                            "control-keep",
                            t("Keep Control", "继续接管"),
                            Tone::Plain,
                            &theme,
                        )
                        .px(px(16.))
                        .on_click(move |_, _, cx| {
                            tell(&keep, cx, move |shell, cx| {
                                shell.answer_control(id, false, cx);
                            })
                        }),
                    ),
            )
            .into_any_element();
        }
        let reason = display
            .requests
            .help()
            .map(|(reason, _)| reason.to_string())
            .unwrap_or_default();
        let label = display.label();
        let size = (display.width, display.height);
        let fit = picture(size);
        let frame = self.streams.read(cx).frame(id);
        let picture = match frame {
            Some(frame) => surface(frame)
                .object_fit(ObjectFit::Contain)
                .size_full()
                .into_any_element(),
            None => super::peek::waiting(id, 10.5).into_any_element(),
        };
        let (open, take_over, later) = (self.shell.clone(), self.shell.clone(), self.shell.clone());
        let (go_on, give) = (self.shell.clone(), self.shell.clone());
        let card_entity = cx.entity().downgrade();
        let answers = display.requests.help_answers();
        let request = display.requests.get(RequestKind::Help).map(|r| r.id);
        let over = reply
            .as_ref()
            .map(|f| f.read(cx).content.chars().count())
            .filter(|&n| n > REPLY_CHARS);
        card(
            tf!("{agent} needs your help", "{agent} 需要你帮忙"),
            reason,
            Badge::Waiting,
            &theme,
        )
        .child(
            div()
                .id("help-picture")
                .role(Role::Button)
                .aria_label(t("Open in Viewer", "在 Viewer 中打开"))
                .tooltip(|_, cx| super::shell::hint(t("Open in Viewer", "在 Viewer 中打开"), cx))
                .tooltip_show_delay(super::shell::HINT_DELAY)
                .flex()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| tell(&open, cx, move |shell, cx| shell.card_open(id, cx)))
                .child(
                    div()
                        .relative()
                        .w(px(fit.0))
                        .h(px(fit.1))
                        .rounded(px(9.))
                        .overflow_hidden()
                        .bg(theme.screen_empty)
                        .child(picture)
                        .child(theme::round_corners(9., theme.card))
                        .child(pointer::layer(id, display.pointer, size, theme.accent))
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(px(9.))
                                .hover(|s| s.border(px(2.)).border_color(theme.orange)),
                        ),
                ),
        )
        .child(
            div()
                .h(px(LABEL))
                .truncate()
                .text_size(px(11.))
                .text_color(theme.secondary)
                .child(label),
        )
        .child(
            div()
                .h(px(BUTTONS))
                .flex()
                .gap(px(8.))
                .child(
                    card_button(
                        "help-take-over",
                        t("Take Over", "接手操作"),
                        Tone::Orange,
                        &theme,
                    )
                    .flex_1()
                    .on_click(move |_, _, cx| {
                        tell(&take_over, cx, move |shell, cx| shell.open_for_help(id, cx))
                    }),
                )
                .when(answers, |row| {
                    row.child(
                        card_button("help-continue", t("Continue", "继续"), Tone::Plain, &theme)
                            .px(px(16.))
                            .on_click(move |_, _, cx| {
                                tell(&go_on, cx, move |shell, cx| {
                                    let _ = shell.answer_help(id, request, Answer::Continue, cx);
                                })
                            }),
                    )
                })
                .when(!answers, |row| {
                    let later = later.clone();
                    row.child(
                        card_button("help-later", t("Later", "稍后"), Tone::Plain, &theme)
                            .px(px(16.))
                            .on_click(move |_, _, cx| {
                                tell(&later, cx, move |shell, cx| shell.card_later(id, cx))
                            }),
                    )
                }),
        )
        .when(answers, |c| {
            c.child(match reply {
                Some(field) => reply_row(field, card_entity, over, &theme),
                None => answer_row(card_entity, give, later, id, &theme),
            })
        })
        .into_any_element()
    }
}

/// The second row of a Help Request's answers: Reply, Give a Window, Later.
fn answer_row(
    card: WeakEntity<HelpCard>,
    give: WeakEntity<Shell>,
    later: WeakEntity<Shell>,
    id: DisplayId,
    theme: &Theme,
) -> gpui::AnyElement {
    div()
        .h(px(BUTTONS))
        .flex()
        .gap(px(8.))
        .child(
            card_button("help-reply", t("Reply", "回复"), Tone::Plain, theme)
                .flex_1()
                .on_click(move |_, window, cx| {
                    let _ = card.update(cx, |card, cx| card.open_reply(id, None, true, window, cx));
                }),
        )
        .child(
            card_button(
                "help-give",
                t("Give a Window", "给它窗口"),
                Tone::Plain,
                theme,
            )
            .flex_1()
            .on_click(move |_, _, cx| {
                let button = painted_button("help-give");
                tell(&give, cx, move |shell, cx| shell.card_give(id, button, cx))
            }),
        )
        .child(
            card_button("help-later", t("Later", "稍后"), Tone::Plain, theme)
                .px(px(16.))
                .on_click(move |_, _, cx| {
                    tell(&later, cx, move |shell, cx| shell.card_later(id, cx))
                }),
        )
        .into_any_element()
}

/// Reply's row: the field, Send and Cancel. Return sends, Escape cancels.
/// `over`: the reply's length, once it passes the limit.
fn reply_row(
    field: Entity<ReplyField>,
    card: WeakEntity<HelpCard>,
    over: Option<usize>,
    theme: &Theme,
) -> gpui::AnyElement {
    let (send, cancel) = (card.clone(), card);
    let typed = field.clone();
    div()
        .h(px(BUTTONS))
        .flex()
        .gap(px(8.))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(BUTTONS))
                .flex()
                .items_center()
                .px(px(12.))
                .rounded_full()
                .bg(theme.control)
                .border_1()
                .border_color(theme.accent.opacity(0.6))
                .text_size(px(13.))
                .text_color(theme.label)
                .child(field)
                .when_some(over, |d, n| {
                    d.child(
                        div()
                            .flex_none()
                            .pl(px(6.))
                            .text_size(px(11.))
                            .text_color(theme.orange)
                            .child(format!("{n}/{REPLY_CHARS}")),
                    )
                })
                .child(painted("reply-field")),
        )
        .child(
            card_button("reply-send", t("Send", "发送"), Tone::Plain, theme)
                .px(px(14.))
                .on_click(move |_, window, cx| {
                    let text = typed.read(cx).content.clone();
                    let _ = send.update(cx, |card, cx| card.send(text, window, cx));
                }),
        )
        .child(
            card_button("reply-cancel", t("Cancel", "取消"), Tone::Plain, theme)
                .px(px(14.))
                .on_click(move |_, window, cx| {
                    let _ = cancel.update(cx, |card, cx| card.close_reply(window, cx));
                }),
        )
        .into_any_element()
}

/// Chinese sets a little tighter: its glyphs fill the em, so Latin
/// leading reads loose.
pub(super) fn zh() -> bool {
    crate::i18n::lang() == crate::i18n::Lang::ZhHans
}

/// The family's title row: an icon, one line of semibold text, and an optional
/// badge that says the card waits for the user (#206).
pub(super) fn title_row(
    icon: &'static str,
    color: gpui::Hsla,
    title: String,
    badge: Option<Badge>,
) -> gpui::Div {
    div()
        .flex_none()
        .h(px(TITLE))
        .flex()
        .items_center()
        .gap(px(7.))
        .child(svg().path(icon).flex_none().size(px(15.)).text_color(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .when_some(badge, |d, badge| {
            d.child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(6.))
                    .bg(color.opacity(0.16))
                    .text_color(color)
                    .text_size(px(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(match badge {
                        Badge::Approval => t("Approval needed", "待批准"),
                        Badge::Waiting => t("Waiting for you", "待处理"),
                    }),
            )
        })
}

/// The family's reason: up to two lines, the rest in its tooltip.
pub(super) fn reason_text(reason: String, zh: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id("help-reason")
        .relative()
        .flex_none()
        .text_size(px(12.5))
        .line_height(px(if zh { 17. } else { 18. }))
        // Without an ellipsis GPUI puts the rest of the text on the last
        // line, past the card's edge.
        .line_clamp(2)
        .text_ellipsis()
        .tooltip({
            let reason = reason.clone();
            move |_, cx| super::shell::hint(reason.clone(), cx)
        })
        .tooltip_show_delay(super::shell::HINT_DELAY)
        .child(reason)
}

/// The secondary line under the picture: the display, and on the right
/// the step or the idle clock.
pub(super) fn label_row(label: String, right: Option<String>, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .h(px(LABEL))
        .flex()
        .justify_between()
        .gap(px(12.))
        .text_size(px(11.))
        .line_height(px(LABEL))
        .text_color(theme.secondary)
        .child(div().min_w_0().truncate().child(label))
        .when_some(right, |d, right| {
            d.child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .font_features(tabular())
                    .child(right),
            )
        })
}

/// What a waiting card's badge says: a request for the user's approval
/// (a window, the mouse), or any other request that waits for them.
#[derive(Clone, Copy)]
pub enum Badge {
    Approval,
    Waiting,
}

/// The card with its title and two lines of text; the rest follows.
fn card(title: String, reason: String, badge: Badge, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id("help-card")
        .role(Role::Dialog)
        .aria_label(SharedString::from(title.clone()))
        .size_full()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .p(px(PADDING - 1.))
        .bg(theme.asking_card)
        .border_2()
        .border_color(theme.asking_border)
        .rounded(px(RADIUS))
        .text_color(theme.label)
        .child(title_row(
            "icons/hand.svg",
            theme.orange,
            title,
            Some(badge),
        ))
        .child(reason_text(reason, false).h(px(REASON)))
}

/// How a button stands out.
#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    /// Orange, with the hand: the Help Card's step-in buttons.
    Orange,
    Plain,
    /// Red text with a stop mark: ending an Input Grant.
    Stop,
    /// The accent color, with no mark: the Bug Report Card's Open on GitHub.
    Accent,
}

/// A pill-shaped button. The Help Card's buttons record where they were
/// painted; see `painted_buttons`.
pub(super) fn card_button(
    id: &'static str,
    title: impl Into<SharedString>,
    tone: Tone,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    button(id, title, tone, theme).child(painted(id))
}

/// Records its parent's bounds under `id` at each paint.
pub(super) fn painted(id: &'static str) -> impl IntoElement {
    canvas(
        move |bounds, _, _| {
            BUTTONS_PAINTED.with(|b| b.borrow_mut().insert(id, bounds));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub fn button(
    id: &'static str,
    title: impl Into<SharedString>,
    tone: Tone,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let title: SharedString = title.into();
    let (bg, hover, fg) = match tone {
        Tone::Orange => (theme.orange.opacity(0.9), theme.orange, gpui::white()),
        Tone::Plain => (theme.control, theme.control_hover, theme.label),
        Tone::Stop => (theme.red.opacity(0.12), theme.red.opacity(0.2), theme.red),
        Tone::Accent => (theme.accent.opacity(0.9), theme.accent, gpui::white()),
    };
    let icon = match tone {
        Tone::Orange => Some("icons/hand.svg"),
        Tone::Stop => Some("icons/stop.svg"),
        Tone::Plain | Tone::Accent => None,
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(title.clone())
        .relative()
        .h(px(BUTTONS))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.7))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(fg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when_some(icon, |d, icon| {
            let size = if tone == Tone::Stop { 10. } else { 13. };
            d.child(svg().path(icon).flex_none().size(px(size)).text_color(fg))
        })
        .child(div().whitespace_nowrap().child(title))
}

thread_local! {
    /// Each card button's bounds in the card at its last paint, for the
    /// debug check that every button fits the card.
    static BUTTONS_PAINTED: RefCell<HashMap<&'static str, Bounds<Pixels>>> =
        RefCell::new(HashMap::new());
    /// The width of the card last rendered.
    static CARD_WIDTH: Cell<f32> = const { Cell::new(WIDTH) };
}

/// Where button `id` was last painted on the card, in points from its
/// corner.
pub fn painted_button(id: &str) -> Option<Bounds<Pixels>> {
    BUTTONS_PAINTED.with(|b| b.borrow().get(id).copied())
}

/// The card's width and its buttons as last painted, with the queue card's
/// reason (`input-request`): id and `[x, y, w, h]` in points from the card's
/// corner. Only what the last render drew is listed.
pub fn painted_buttons() -> (f32, Vec<(&'static str, [f32; 4])>) {
    let buttons = BUTTONS_PAINTED.with(|b| {
        b.borrow()
            .iter()
            .map(|(id, b)| {
                let r = [b.origin.x, b.origin.y, b.size.width, b.size.height].map(f32::from);
                (*id, r)
            })
            .collect()
    });
    (CARD_WIDTH.with(Cell::get), buttons)
}
