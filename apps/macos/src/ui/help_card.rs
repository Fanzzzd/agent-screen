//! The Help Card: drops from the menu bar icon while a Help Request is
//! open, with the display's picture, the agent's reason, and Take Over or
//! Later. For a Window Request it names the window the agent asks for,
//! with Hand Over or Keep. For a Control Request, during a Takeover, it
//! gives the agent's reason, with Let … Continue or Keep Control. For the
//! Input Queue (ADR 0013) it asks to use the user's mouse, with each
//! waiting request and the whole scope of approval, then shows the grant
//! with Stop.

use super::pointer;
use super::shell::{Shell, tell};
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::i18n::t;
use crate::input_queue::Plan;
use crate::model::DisplayId;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gpui::{
    Bounds, Context, Entity, HighlightStyle, InteractiveElement, IntoElement, MouseButton,
    ObjectFit, ParentElement, Pixels, Render, Role, SharedString, StatefulInteractiveElement,
    Styled, StyledText, WeakEntity, Window, canvas, div, prelude::FluentBuilder, px, surface, svg,
};

pub const WIDTH: f32 = 320.;
/// The Input Queue's card is wider: its scope text is shown in full.
pub const QUEUE_WIDTH: f32 = 360.;
const PADDING: f32 = 14.;
const GAP: f32 = 8.;
const TITLE: f32 = 20.;
/// Two lines of the reason; the tooltip has all of it.
const REASON: f32 = 36.;
const LABEL: f32 = 16.;
const BUTTONS: f32 = 30.;
const MAX_PICTURE: f32 = 184.;
const RADIUS: f32 = 18.;

/// The card's height for a Window Request, which has no picture.
pub const REQUEST_HEIGHT: f32 = PADDING * 2. + TITLE + REASON + LABEL + BUTTONS + GAP * 3.;

/// The card's height for a display of `size`: everything but the picture is
/// fixed, so the panel can be placed before it paints.
pub fn height(size: (f64, f64)) -> f32 {
    PADDING * 2. + TITLE + REASON + LABEL + BUTTONS + GAP * 4. + picture(size).1
}

/// The picture's size inside the card, aspect-fit.
fn picture(size: (f64, f64)) -> (f32, f32) {
    let width = WIDTH - PADDING * 2.;
    let fit = (width / size.0 as f32).min(MAX_PICTURE / size.1 as f32);
    (size.0 as f32 * fit, size.1 as f32 * fit)
}

pub struct HelpCard {
    shell: WeakEntity<Shell>,
    core: Entity<Core>,
    streams: Entity<Streams>,
    /// The display whose Help Request the card shows.
    pub display: Option<DisplayId>,
    pub input_queue: bool,
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
        }
    }
}

impl Render for HelpCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let width = if self.input_queue { QUEUE_WIDTH } else { WIDTH };
        CARD_WIDTH.with(|w| w.set(width));
        // Only what this render draws reports its bounds.
        BUTTONS_PAINTED.with(|b| b.borrow_mut().clear());
        if self.input_queue {
            return self.render_queue(&theme, cx);
        }
        let core = self.core.read(cx);
        let Some(display) = self.display.and_then(|id| core.displays.get(id)) else {
            return div().into_any_element();
        };
        let id = display.id;
        let agent = display.agent_name().to_string();
        if let Some(request) = &display.window_request {
            let app = request.app.clone();
            let window = if request.title.is_empty() {
                app.clone()
            } else {
                request.title.clone()
            };
            let (give, keep) = (self.shell.clone(), self.shell.clone());
            return card(
                tf!(
                    "{agent} asks for your {app} window",
                    "{agent} 想要你的 {app} 窗口"
                ),
                tf!(
                    "“{window}” goes to its display. You can take it back from the Windows menu.",
                    "“{window}”会挪到它的显示器上，之后可以从“窗口”菜单收回。"
                ),
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
                        card_button("window-keep", t("Keep", "留着"), Tone::Plain, &theme)
                            .px(px(16.))
                            .on_click(move |_, _, cx| {
                                tell(&keep, cx, move |shell, cx| {
                                    shell.answer_window(id, false, cx);
                                })
                            }),
                    ),
            )
            .into_any_element();
        }
        if let Some(reason) = display.control_request.clone() {
            let (give, keep) = (self.shell.clone(), self.shell.clone());
            return card(
                tf!("{agent} asks to continue", "{agent} 想继续操作"),
                reason,
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
        let reason = display.help_request.clone().unwrap_or_default();
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
        card(
            tf!("{agent} needs your help", "{agent} 需要你帮忙"),
            reason,
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
                .child(
                    card_button("help-later", t("Later", "稍后"), Tone::Plain, &theme)
                        .px(px(16.))
                        .on_click(move |_, _, cx| {
                            tell(&later, cx, move |shell, cx| shell.card_later(id, cx))
                        }),
                ),
        )
        .into_any_element()
    }
}

impl HelpCard {
    /// The Input Queue's card: asking for approval, or during a grant,
    /// what runs and how to take control back. Its height follows its
    /// content: nothing in it shrinks, as GPUI sizes a shrinkable child of
    /// a content-sized column at its minimum. Past a few requests the list
    /// gets a fixed height and scrolls.
    fn render_queue(&self, theme: &Theme, cx: &Context<Self>) -> gpui::AnyElement {
        let core = self.core.read(cx);
        let input = &core.input_queue;
        let shell = self.shell.clone();
        let root = div()
            .id("help-card")
            .role(Role::Dialog)
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.card)
            .border_1()
            .border_color(theme.card_border)
            .rounded(px(RADIUS))
            .text_color(theme.label)
            // The window takes the card's height once it is painted. The
            // card has its content's height, wherever the window is.
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let height = f32::from(bounds.size.height).ceil();
                        if (height - QUEUE_FIT.with(Cell::get)).abs() > 0.5 {
                            QUEUE_FIT.with(|f| f.set(height));
                            tell(&shell, cx, |shell, cx| shell.fit_card(cx));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        if let Some(grant) = grant_view(input) {
            let task = input
                .queue
                .active()
                .or_else(|| input.queue.waiting().next())
                .and_then(|r| core.displays.get(r.owner.display))
                .or_else(|| self.display.and_then(|id| core.displays.get(id)))
                .map(|d| d.label())
                .unwrap_or_default();
            let (title, sub) = if grant.running {
                (
                    t("Using your mouse", "正在使用你的鼠标"),
                    t(
                        "The current action runs on an Agent Display. Queued actions follow.",
                        "当前操作在 Agent Display 上执行，完成后继续处理队列。",
                    ),
                )
            } else {
                (
                    t("Waiting for the next action", "等待下一步"),
                    t(
                        "Nothing is running. Approval ends after 30 seconds of inactivity.",
                        "当前没有操作。空闲满 30 秒后，自动结束本次放行。",
                    ),
                )
            };
            let picture = self
                .display
                .and_then(|id| self.streams.read(cx).frame(id))
                .map(|frame| {
                    surface(frame)
                        .object_fit(ObjectFit::Contain)
                        .size_full()
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element());
            let revoke = self.shell.clone();
            return queue_window(
                root.aria_label(SharedString::from(title))
                    .child(queue_header(title, sub, theme).min_h(px(QUEUE_GRANT_HEADER)))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .px(px(QUEUE_PAD))
                            .pb(px(20.))
                            .child(
                                div()
                                    .relative()
                                    .flex_none()
                                    .h(px(116.))
                                    .rounded(px(8.))
                                    .overflow_hidden()
                                    .bg(theme.control)
                                    .child(picture)
                                    .child(theme::round_corners(8., theme.card)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .mt(px(12.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.))
                                    .text_size(px(11.))
                                    .text_color(theme.secondary)
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .when(grant.running, |d| {
                                                d.child(
                                                    div()
                                                        .flex_none()
                                                        .size(px(6.))
                                                        .rounded_full()
                                                        .bg(theme.red),
                                                )
                                            })
                                            .child(div().min_w_0().truncate().child(task)),
                                    )
                                    .child(div().flex_none().font_features(tabular()).child(
                                        if grant.running {
                                            clock(grant.elapsed)
                                        } else {
                                            tf!(
                                                "Idle {} / {}",
                                                "空闲 {} / {}",
                                                clock(grant.idle.unwrap_or(0)),
                                                clock(IDLE_SECONDS)
                                            )
                                        },
                                    )),
                            ),
                    )
                    .child(scope(
                        "icons/pointer.svg",
                        "",
                        t(
                            "Move the mouse or press any key to take control.",
                            "移动鼠标或按任意键即可收回。",
                        ),
                        &format!(
                            "\n{}",
                            t(
                                "New requests may run until the queue is idle for 30 seconds.",
                                "之后的新请求仍可执行，空闲 30 秒后结束。",
                            )
                        ),
                        theme,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .px(px(QUEUE_PAD))
                            .pb(px(QUEUE_PAD))
                            .child(
                                card_button(
                                    "input-revoke",
                                    t("Stop and take control", "停止并收回"),
                                    Tone::Stop,
                                    theme,
                                )
                                .h(px(QUEUE_BUTTONS))
                                .flex_1()
                                .on_click(move |_, _, cx| {
                                    tell(&revoke, cx, |shell, cx| shell.revoke_input(cx))
                                }),
                            ),
                    ),
            );
        }

        let approval = input.queue.approval();
        let mut rows = Vec::new();
        let mut can_approve = false;
        let (mut menus, mut clicks, mut drags) = (false, false, false);
        for request in input.queue.waiting() {
            let Some(display) = core.displays.get(request.owner.display) else {
                continue;
            };
            can_approve |= !display.takeover && !display.is_ended();
            match input.plan(request.id) {
                Some(Plan::MenuItem(_)) => menus = true,
                Some(Plan::Click(_)) => clicks = true,
                _ => drags = true,
            }
            let row = request_row(
                display.label(),
                request_text(input, request),
                display.takeover,
                theme,
            );
            // The first row reports where it was painted, as buttons do.
            rows.push(if rows.is_empty() {
                row.relative().child(painted("input-request"))
            } else {
                row
            });
        }
        let count = rows.len();
        let (title, sub) = if can_approve || count == 0 {
            (
                t("Allow use of your mouse?", "允许使用你的鼠标？"),
                match (menus, clicks, drags) {
                    (true, false, false) => t(
                        "This app takes a menu item chosen in the background as its keyboard shortcut. A click with your real pointer chooses it.",
                        "这个应用会把在后台选择的菜单项当作快捷键处理。用你的真实指针点一下，才能真正选中它。",
                    ),
                    (false, true, false) => t(
                        "A click sent in the background had no effect in this app. It needs your real pointer.",
                        "后台发送的点击在这个应用里没有生效，需要暂时使用你的真实指针。",
                    ),
                    (false, false, _) => t(
                        "These drags cannot run in the background. They need your real pointer.",
                        "这些拖拽无法在后台完成，需要暂时使用你的真实指针。",
                    ),
                    _ => t(
                        "These actions cannot run in the background. They need your real pointer.",
                        "这些操作无法在后台完成，需要暂时使用你的真实指针。",
                    ),
                },
            )
        } else {
            (
                t("Waiting for hand-back", "等待你交还显示器"),
                t(
                    "You are using this display. Hand it back before approving these requests.",
                    "你正在接手此显示器。交还后，才能批准这些请求。",
                ),
            )
        };
        let (allow, decline) = (self.shell.clone(), self.shell.clone());
        queue_window(root.aria_label(SharedString::from(title))
            .child(queue_header(title, sub, theme))
            .child(
                div()
                    .id("input-requests")
                    .flex_none()
                    .when(count > QUEUE_ROWS, |d| d.h(px(QUEUE_LIST)).overflow_y_scroll())
                    .px(px(QUEUE_PAD))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(QUEUE_PAD))
                    .pb(px(12.))
                    .text_size(px(11.))
                    .text_color(theme.secondary)
                    .child(if count == 1 {
                        t("1 request", "1 个请求").to_string()
                    } else {
                        tf!("{count} requests, one at a time", "{count} 个请求，依次执行")
                    }),
            )
            .child(scope(
                "icons/lock.svg",
                t("Approval covers ", "批准包括"),
                t("these and new requests", "当前及之后的新请求"),
                t(
                    " until 30 seconds of inactivity. Move the mouse or press any key to take control.",
                    "。空闲 30 秒后结束。移动鼠标或按任意键即可收回。",
                ),
                theme,
            ))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(8.))
                    .px(px(QUEUE_PAD))
                    .pb(px(QUEUE_PAD))
                    .child(
                        card_button("input-decline", t("Not now", "暂不允许"), Tone::Plain, theme)
                            .h(px(QUEUE_BUTTONS))
                            .px(px(18.))
                            .on_click(move |_, _, cx| {
                                tell(&decline, cx, move |shell, cx| {
                                    shell.answer_input(approval, false, cx)
                                })
                            }),
                    )
                    .child(
                        card_button(
                            "input-allow",
                            if can_approve {
                                t("Allow queue", "允许队列")
                            } else {
                                t("Waiting for hand-back", "等待交还")
                            },
                            Tone::Accent,
                            theme,
                        )
                        .h(px(QUEUE_BUTTONS))
                        .flex_1()
                        .when(!can_approve, |d| d.opacity(0.5))
                        .on_click(move |_, _, cx| {
                            if can_approve {
                                tell(&allow, cx, move |shell, cx| {
                                    shell.answer_input(approval, true, cx)
                                });
                            }
                        }),
                    ),
            )
            )
    }
}

/// The window around the queue card. GPUI gives the window's root the
/// window's size, so the card inside it keeps its own height, which the
/// window then takes: a root measured instead follows the window down.
fn queue_window(card: gpui::Stateful<gpui::Div>) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(card.flex_none())
        .into_any_element()
}

const QUEUE_PAD: f32 = 24.;
/// The grant's header with two lines of text, running or not, so the card
/// keeps its height when a drag starts.
const QUEUE_GRANT_HEADER: f32 = 144.;
/// Past this many requests, the list takes QUEUE_LIST and scrolls.
const QUEUE_ROWS: usize = 3;
const QUEUE_LIST: f32 = 220.;
const QUEUE_BUTTONS: f32 = 36.;
const IDLE_SECONDS: u64 = agent_screen_model::input_queue::IDLE_MS / 1000;

thread_local! {
    /// The queue card's height at its last paint, which its window takes.
    /// Before the first paint, a guess.
    static QUEUE_FIT: Cell<f32> = const { Cell::new(420.) };
}

/// The height the queue card's window should have.
pub fn queue_height() -> f32 {
    QUEUE_FIT.with(Cell::get)
}

/// What a request row says: the menu item a click will choose, in the
/// user's language, or the agent's reason for a drag.
pub fn request_text(
    input: &crate::input_queue::InputQueue,
    request: &agent_screen_model::input_queue::Request,
) -> String {
    match input.plan(request.id) {
        Some(Plan::MenuItem(item)) => {
            let label = agent_screen_model::menu_press::label(&item.titles());
            let app = &item.app;
            tf!("Choose \"{label}\" in {app}", "在 {app} 中选择\"{label}\"")
        }
        Some(Plan::Click(click)) => click_text(click),
        _ => request.reason.clone(),
    }
}

/// A click the user is asked to allow: what it does, where, and in which
/// app and window.
fn click_text(click: &crate::input_queue::Click) -> String {
    use agent_screen_model::real_click::{self, Verb};
    let (x, y) = real_click::display_point(click.at, click.area);
    let app = &click.app;
    let verb = match click.click.verb() {
        Verb::Click => t("Click", "点击"),
        Verb::DoubleClick => t("Double-click", "双击"),
        Verb::TripleClick => t("Triple-click", "三击"),
        Verb::RightClick => t("Right-click", "右键点击"),
        Verb::MiddleClick => t("Middle-click", "中键点击"),
    };
    match &click.title {
        Some(title) if title != app => tf!(
            "{verb} at ({x}, {y}) in {app}, \"{title}\"",
            "在 {app}「{title}」中{verb} ({x}, {y})"
        ),
        _ => tf!(
            "{verb} at ({x}, {y}) in {app}",
            "在 {app} 中{verb} ({x}, {y})"
        ),
    }
}

/// What the card and the status bar show of a grant.
pub struct GrantView {
    /// A request runs or waits to run; otherwise the grant idles toward
    /// its end.
    pub running: bool,
    pub elapsed: u64,
    pub idle: Option<u64>,
}

pub fn grant_view(input: &crate::input_queue::InputQueue) -> Option<GrantView> {
    input.queue.grant()?;
    let idle = input.idle_seconds();
    Some(GrantView {
        running: input.queue.active().is_some() || idle.is_none(),
        elapsed: input.elapsed_seconds().unwrap_or(0),
        idle,
    })
}

/// Minutes and seconds: 01:05.
pub fn clock(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// Digits of one width, so a ticking clock doesn't jitter.
pub fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// The queue card's top: where it comes from, the question and why.
fn queue_header(title: &str, sub: &str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .px(px(QUEUE_PAD))
        .pt(px(20.))
        .pb(px(20.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .mb(px(14.))
                .text_size(px(11.))
                .text_color(theme.secondary)
                .child(
                    svg()
                        .path("icons/display.svg")
                        .flex_none()
                        .size(px(14.))
                        .text_color(theme.secondary),
                )
                .child("Agent Screen"),
        )
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(26.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .mb(px(8.))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(theme.secondary)
                .child(sub.to_string()),
        )
}

/// One waiting request: whose display, and what the agent wants to do.
fn request_row(label: String, reason: String, takeover: bool, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .gap(px(12.))
        .py(px(14.))
        .border_t_1()
        .border_color(theme.card_border)
        .child(
            div()
                .flex_none()
                .size(px(36.))
                .rounded(px(10.))
                .bg(theme.accent.opacity(0.14))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("icons/display.svg")
                        .size(px(18.))
                        .text_color(theme.accent),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(theme.secondary)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(reason),
                )
                .when(takeover, |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .child(t("You are using this display", "你正在操作此显示器")),
                    )
                }),
        )
}

/// What approval allows, in full: `strong` stands out between `before`
/// and `after`.
fn scope(icon: &'static str, before: &str, strong: &str, after: &str, theme: &Theme) -> gpui::Div {
    let text = format!("{before}{strong}{after}");
    let strong = before.len()..before.len() + strong.len();
    div()
        .flex_none()
        .mx(px(QUEUE_PAD))
        .mt(px(4.))
        .mb(px(20.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(10.))
        .bg(theme.control)
        .flex()
        .gap(px(8.))
        .child(
            svg()
                .path(icon)
                .flex_none()
                .mt(px(2.))
                .size(px(13.))
                .text_color(theme.secondary),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(11.5))
                .line_height(px(18.))
                .text_color(theme.secondary)
                .child(StyledText::new(text).with_highlights([(
                    strong,
                    HighlightStyle {
                        color: Some(theme.label),
                        font_weight: Some(gpui::FontWeight::MEDIUM),
                        ..Default::default()
                    },
                )])),
        )
}

/// The card with its title and two lines of text; the rest follows.
fn card(title: String, reason: String, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id("help-card")
        .role(Role::Dialog)
        .aria_label(SharedString::from(title.clone()))
        .size_full()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .p(px(PADDING))
        .bg(theme.card)
        .border_1()
        .border_color(theme.card_border)
        .rounded(px(RADIUS))
        .text_color(theme.label)
        .child(
            div()
                .h(px(TITLE))
                .flex()
                .items_center()
                .gap(px(7.))
                .child(
                    svg()
                        .path("icons/hand.svg")
                        .flex_none()
                        .size(px(15.))
                        .text_color(theme.orange),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                ),
        )
        .child(
            div()
                .id("help-reason")
                .h(px(REASON))
                .text_size(px(12.5))
                .line_height(px(18.))
                // Without an ellipsis GPUI puts the rest of the text on
                // the last line, past the card's edge.
                .line_clamp(2)
                .text_ellipsis()
                .tooltip({
                    let reason = reason.clone();
                    move |_, cx| super::shell::hint(reason.clone(), cx)
                })
                .tooltip_show_delay(super::shell::HINT_DELAY)
                .child(reason),
        )
}

/// How a button stands out.
#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    /// Orange, with the hand: the Help Card's step-in buttons.
    Orange,
    /// The accent color: the Input Queue's approval.
    Accent,
    Plain,
    /// Red text with a stop mark: ending an Input Grant.
    Stop,
}

/// A pill-shaped button. The Help Card's buttons record where they were
/// painted; see `painted_buttons`.
fn card_button(
    id: &'static str,
    title: impl Into<SharedString>,
    tone: Tone,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    button(id, title, tone, theme).child(painted(id))
}

/// Records its parent's bounds under `id` at each paint.
fn painted(id: &'static str) -> impl IntoElement {
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
        Tone::Accent => (theme.accent.opacity(0.92), theme.accent, gpui::white()),
        Tone::Plain => (theme.control, theme.control_hover, theme.label),
        Tone::Stop => (theme.red.opacity(0.12), theme.red.opacity(0.2), theme.red),
    };
    let icon = match tone {
        Tone::Orange => Some("icons/hand.svg"),
        Tone::Stop => Some("icons/stop.svg"),
        Tone::Accent | Tone::Plain => None,
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

/// The card's width and its buttons as last painted, with the queue card's
/// first request row: id and `[x, y, w, h]` in points from the card's
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
