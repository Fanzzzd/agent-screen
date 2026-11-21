//! The Input Queue card: asks to use the user's mouse, draws waiting drag paths
//! and click targets on the display's live picture, and shows active/idle grant
//! status with Stop (ADR 0013).

use super::help_card::{
    BUTTONS, Badge, GAP, HelpCard, PADDING, RADIUS, Tone, card_button, label_row, painted, picture,
    reason_text, title_row, zh,
};
use super::shell::{Shell, tell};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::i18n::t;
use crate::input_queue::Plan;
use crate::model::DisplayId;
use core_video::pixel_buffer::CVPixelBuffer;
use gpui::{
    Bounds, Context, HighlightStyle, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    PathBuilder, Pixels, Role, SharedString, StatefulInteractiveElement, Styled, StyledText,
    WeakEntity, Window, canvas, div, point, prelude::FluentBuilder, px, surface,
};
use std::cell::{Cell, RefCell};

impl HelpCard {
    /// The Input Queue's card, a member of the Help Card family: asking to
    /// use the user's mouse, with each request's path drawn on its
    /// display's picture, or during a grant, the step that runs and Stop.
    /// Its height follows its content: nothing in it shrinks, as GPUI sizes
    /// a shrinkable child of a content-sized column at its minimum.
    pub(super) fn render_queue(&self, theme: &Theme, cx: &Context<Self>) -> gpui::AnyElement {
        let core = self.core.read(cx);
        let view = queue_view(core, self.display);
        let zh = zh();
        let shell = self.shell.clone();
        let title = queue_title(&view);
        let count = view.items.len();
        let shown = self.display.and_then(|id| core.displays.get(id));
        let label = shown.map(|d| d.label()).unwrap_or_default();
        let picture = shown.map(|d| {
            let size = (d.width, d.height);
            let frame = self.streams.read(cx).frame(d.id);
            queue_picture(d.id, size, frame, &view, theme)
        });
        let asking = matches!(view.phase, Phase::Asking { .. });
        let waiting = waiting_rows(core, theme, &self.shell);
        let border_pad = if asking { 4. } else { 2. };
        let root = div()
            .id("help-card")
            .role(Role::Dialog)
            .aria_label(SharedString::from(title.clone()))
            .relative()
            .flex()
            .flex_col()
            .gap(px(if zh { 6. } else { GAP }))
            .p(px(if asking { PADDING - 1. } else { PADDING }))
            .bg(if asking {
                theme.asking_card
            } else {
                theme.card
            })
            .when(asking, |d| d.border_2().border_color(theme.asking_border))
            .when(!asking, |d| d.border_1().border_color(theme.card_border))
            .rounded(px(RADIUS))
            .text_color(theme.label)
            // The window takes the card's height once it is painted. The
            // card has its content's height, wherever the window is.
            .child(
                canvas(
                    move |bounds, _, cx| {
                        // The canvas fills the card inside its border.
                        let height = f32::from(bounds.size.height).ceil() + border_pad;
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

        if let Phase::Asking { can_approve } = view.phase {
            let reason = match (count, view.kind()) {
                (0 | 1, _) => view.items.first().map(one_reason).unwrap_or_default(),
                (n, Kind::Drags) => tf!(
                    "These {n} drags can’t run in the background.",
                    "这 {n} 次拖拽后台做不了，要借用你的指针。"
                ),
                (n, Kind::Clicks) => tf!(
                    "These {n} clicks had no effect in the background.",
                    "这 {n} 次点击在后台没有生效，要借用你的指针。"
                ),
                (n, _) => tf!(
                    "These {n} actions need your pointer.",
                    "这 {n} 个操作要借用你的指针。"
                ),
            };
            let several = view.agent.is_none();
            let rows = view.items.iter().enumerate().map(|(i, item)| {
                let row = list_row(i, item, several, zh, theme).relative();
                // The first and last rows report where they were
                // painted, as buttons do.
                let row = row.when(i == 0, |d| d.child(painted("input-request")));
                row.when(i + 1 == count, |d| d.child(painted("input-request-last")))
            });
            let (allow, decline) = (self.shell.clone(), self.shell.clone());
            let approval = core.input_queue.queue.approval();
            let scope = if can_approve {
                let (strong, rest) = if count > 1 {
                    (
                        t("Allow", "允许"),
                        t(
                            " runs them and any new requests in turn.",
                            "后依次执行，之后的新请求也会执行。",
                        ),
                    )
                } else {
                    (
                        t("Allow", "允许"),
                        t(
                            " runs it and any new requests in turn.",
                            "后依次执行，之后的新请求也会执行。",
                        ),
                    )
                };
                scope_lines(
                    Some((strong, rest)),
                    t(
                        "Ends after 30 s idle. Mouse or keyboard stops it.",
                        "空闲 30 秒自动结束，动鼠标或按键即可收回。",
                    ),
                    zh,
                    theme,
                )
            } else {
                scope_lines(
                    None,
                    t(
                        "You are using this display. Hand it back to allow.",
                        "你正在操作这个显示器，交还后才能允许。",
                    ),
                    zh,
                    theme,
                )
            };
            return queue_window(
                root.child(title_row(
                    "icons/hand.svg",
                    theme.orange,
                    title,
                    Some(Badge::Approval),
                ))
                // With one request, the reason is the request.
                .child(
                    reason_text(reason, zh).when(count == 1, |d| d.child(painted("input-request"))),
                )
                .children(picture)
                .when(count <= 1, |d| {
                    d.child(label_row(label.clone(), None, theme))
                })
                .when(count > 1, |d| {
                    d.child(
                        div()
                            .id("input-requests")
                            .flex_none()
                            .flex()
                            .flex_col()
                            .when(count > LIST_ROWS, |d| {
                                d.h(px(LIST_ROWS as f32 * list_row_height(zh)))
                                    .overflow_y_scroll()
                            })
                            .children(rows),
                    )
                })
                .children(waiting)
                .child(scope)
                .child(
                    div()
                        .flex_none()
                        .h(px(BUTTONS))
                        .mt(px(if zh { 3. } else { 2. }))
                        .flex()
                        .gap(px(8.))
                        .child(
                            card_button("input-allow", t("Allow", "允许"), Tone::Orange, theme)
                                .flex_1()
                                .when(!can_approve, |d| d.opacity(0.5))
                                .on_click(move |_, _, cx| {
                                    if can_approve {
                                        tell(&allow, cx, move |shell, cx| {
                                            shell.answer_input(approval, true, cx)
                                        });
                                    }
                                }),
                        )
                        .child(
                            card_button("input-decline", t("Not Now", "暂不"), Tone::Plain, theme)
                                .px(px(16.))
                                .on_click(move |_, _, cx| {
                                    tell(&decline, cx, move |shell, cx| {
                                        shell.answer_input(approval, false, cx)
                                    })
                                }),
                        ),
                ),
            );
        }

        let (reason, right) = match view.phase {
            Phase::Running { step } => (
                view.items
                    .iter()
                    .find(|i| i.step == Step::Now)
                    .or_else(|| view.items.iter().find(|i| i.step == Step::Pending))
                    .map(|i| i.text.clone())
                    .unwrap_or_else(|| t("Starting the next step.", "马上开始下一步。").into()),
                (count > 0).then(|| {
                    let n = count.max(step);
                    tf!("Step {step} of {n}", "第 {step} / {n} 步")
                }),
            ),
            Phase::Waiting { idle } => (
                t(
                    "Nothing running. Ends after 30 seconds idle.",
                    "当前没有操作，空闲 30 秒后自动结束。",
                )
                .to_string(),
                Some(tf!(
                    "Idle {} / {}",
                    "空闲 {} / {}",
                    clock(idle),
                    clock(IDLE_SECONDS)
                )),
            ),
            Phase::Asking { .. } => unreachable!(),
        };
        let revoke = self.shell.clone();
        queue_window(
            root.child(title_row("icons/pointer.svg", theme.red, title, None))
                .child(reason_text(reason, zh))
                .children(picture)
                .child(label_row(label, right, theme))
                .children(waiting)
                .child(scope_lines(None, stop_hint(), zh, theme))
                .child(
                    div()
                        .flex_none()
                        .h(px(BUTTONS))
                        .mt(px(if zh { 3. } else { 2. }))
                        .flex()
                        .child(
                            card_button(
                                "input-revoke",
                                t("Stop and Take Back", "停止并收回"),
                                Tone::Stop,
                                theme,
                            )
                            .flex_1()
                            .on_click(move |_, _, cx| {
                                tell(&revoke, cx, |shell, cx| shell.revoke_input(cx))
                            }),
                        ),
                ),
        )
    }
}

/// How to take the mouse back, on the card during a grant.
fn stop_hint() -> &'static str {
    t(
        "Move the mouse or press a key to stop.",
        "动鼠标或按任意键即可收回。",
    )
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

/// Past this many requests, the list keeps this many rows' height and
/// scrolls.
const LIST_ROWS: usize = 5;
const IDLE_SECONDS: u64 = agent_screen_model::input_queue::IDLE_MS / 1000;

fn list_row_height(zh: bool) -> f32 {
    if zh { 16. + 6. } else { 17. + 10. }
}

thread_local! {
    /// The queue card's height at its last paint, which its window takes.
    /// Before the first paint, a guess.
    static QUEUE_FIT: Cell<f32> = const { Cell::new(420.) };
    /// A debug preview: present the grant's second step as running,
    /// without running anything, until the grant ends.
    static RUN_PREVIEW: Cell<bool> = const { Cell::new(false) };
    /// A debug preview: steps shown after the queue's own, as if other
    /// requests waited, until nothing waits or runs.
    static PREVIEW_STEPS: RefCell<Vec<(DisplayId, Plan)>> = const { RefCell::new(Vec::new()) };
}

/// The height the queue card's window should have.
pub fn queue_height() -> f32 {
    QUEUE_FIT.with(Cell::get)
}

/// Show the current grant as if its second step ran, for screenshots.
pub fn preview_running() {
    RUN_PREVIEW.with(|p| p.set(true));
}

/// Show `steps` after the queue's own, for screenshots: a session waits on
/// one request at a time, so one display has one request.
pub fn preview_steps(steps: Vec<(DisplayId, Plan)>) {
    PREVIEW_STEPS.with(|p| *p.borrow_mut() = steps);
}

/// The preview's steps as items, or none, forgetting them once the queue
/// is empty.
fn previewed(
    input: &crate::input_queue::InputQueue,
    agent_of: impl Fn(DisplayId) -> String,
) -> Vec<Item> {
    if input.queue.waiting().next().is_none() && input.queue.grant().is_none() {
        PREVIEW_STEPS.with(|p| p.borrow_mut().clear());
    }
    PREVIEW_STEPS.with(|p| {
        p.borrow()
            .iter()
            .map(|(display, plan)| Item {
                display: *display,
                agent: agent_of(*display),
                text: plan_text(plan),
                plan: Some(plan.clone()),
                step: Step::Pending,
                takeover: false,
            })
            .collect()
    })
}

/// What a request says: the menu item a click will choose, or the drag,
/// in the user's language, or the agent's reason for a request that only
/// asks.
pub fn request_text(
    input: &crate::input_queue::InputQueue,
    request: &agent_screen_model::input_queue::Request,
) -> String {
    match input.plan(request.id) {
        Some(plan) => plan_text(plan),
        None => request.reason.clone(),
    }
}

fn plan_text(plan: &Plan) -> String {
    match plan {
        Plan::MenuItem(item) => {
            let label = agent_screen_model::menu_press::label(&item.titles());
            let app = &item.app;
            tf!("Choose \"{label}\" in {app}", "在 {app} 中选择\"{label}\"")
        }
        Plan::Drag(drag) => {
            let at = |p: (f64, f64)| {
                format!(
                    "({}, {})",
                    (p.0 - drag.area.x).round() as i64,
                    (p.1 - drag.area.y).round() as i64
                )
            };
            let (from, to) = (at(drag.from), at(drag.to));
            tf!("Drag from {from} to {to}", "从 {from} 拖到 {to}")
        }
        Plan::Click(click) => click_text(click),
    }
}

/// How much of a window's title a click's text keeps.
const TITLE_CHARS: usize = 20;

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
    // A long title gives way, so the card's two lines keep the reason.
    let title = click
        .title
        .as_deref()
        .and_then(|title| real_click::short_title(title, TITLE_CHARS));
    match title {
        Some(title) if &title != app => tf!(
            "{verb} at ({x}, {y}) in {app}, \"{title}\"",
            "在 {app}「{title}」中{verb} ({x}, {y})"
        ),
        _ => tf!(
            "{verb} at ({x}, {y}) in {app}",
            "在 {app} 中{verb} ({x}, {y})"
        ),
    }
}

/// Where a step of the queue stands.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Step {
    Done,
    Now,
    Pending,
}

/// One request on the card: whose, what it does and where it stands.
pub struct Item {
    pub display: DisplayId,
    pub agent: String,
    pub text: String,
    pub plan: Option<Plan>,
    pub step: Step,
    /// The user has taken over its display, so Allow doesn't cover it.
    pub takeover: bool,
}

pub enum Phase {
    /// Waiting for the user's answer; a Takeover of every asking display
    /// holds approval back.
    Asking { can_approve: bool },
    /// A step runs, or is about to: the `step`th, from 1.
    Running { step: usize },
    /// Granted with nothing to run, for `idle` seconds.
    Waiting { idle: u64 },
}

/// What the queue's card and the Input Bar show.
pub struct QueueView {
    pub phase: Phase,
    /// The agent asking or using the mouse, or None when several agents
    /// ask.
    pub agent: Option<String>,
    /// How many agents the items come from.
    pub agents: usize,
    pub items: Vec<Item>,
}

/// What the card's requests are, when they are all of one kind. A request
/// that only asks counts as a drag, as only drags ask that way.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Drags,
    Clicks,
    Menus,
    Mixed,
}

fn kind(plan: Option<&Plan>) -> Kind {
    match plan {
        Some(Plan::Click(_)) => Kind::Clicks,
        Some(Plan::MenuItem(_)) => Kind::Menus,
        _ => Kind::Drags,
    }
}

impl QueueView {
    fn kind(&self) -> Kind {
        let mut kinds = self.items.iter().map(|i| kind(i.plan.as_ref()));
        let Some(first) = kinds.next() else {
            return Kind::Drags;
        };
        if kinds.all(|k| k == first) {
            first
        } else {
            Kind::Mixed
        }
    }
}

/// The reason for a card with one request: what it does, and why it needs
/// the user's pointer.
fn one_reason(item: &Item) -> String {
    let text = &item.text;
    match &item.plan {
        Some(Plan::Drag(_)) => tf!(
            "{text}. Can’t run in the background.",
            "{text}，后台做不了。"
        ),
        Some(Plan::Click(_)) => tf!(
            "{text}. A click in the background had no effect.",
            "{text}，后台点击无效。"
        ),
        Some(Plan::MenuItem(_)) => tf!(
            "{text}. Chosen in the background, it acts as a shortcut.",
            "{text}，后台选择只会当作快捷键。"
        ),
        None => text.clone(),
    }
}

/// The queue as the card shows it. `display` is the card's display, whose
/// agent speaks when no request names one; None takes the one the card
/// would show.
pub fn queue_view(core: &Core, display: Option<DisplayId>) -> QueueView {
    let input = &core.input_queue;
    let agent_of = |id: DisplayId| {
        core.displays
            .get(id)
            .map_or_else(|| "Agent".to_string(), |d| d.agent_name().to_string())
    };
    let fallback = display
        .or_else(|| input.card_display(&core.displays))
        .map_or_else(|| "Agent".to_string(), agent_of);
    let Some(grant) = input.queue.grant() else {
        RUN_PREVIEW.with(|p| p.set(false));
        let mut can_approve = false;
        let mut items = Vec::new();
        for request in input.queue.waiting() {
            let Some(shown) = core.displays.get(request.owner.display) else {
                continue;
            };
            can_approve |= !shown.takeover && !shown.is_ended();
            items.push(Item {
                display: request.owner.display,
                agent: shown.agent_name().to_string(),
                text: request_text(input, request),
                plan: input.plan(request.id).cloned(),
                step: Step::Pending,
                takeover: shown.takeover,
            });
        }
        items.extend(previewed(input, agent_of));
        let (agent, agents) = one_agent(&items, fallback);
        return QueueView {
            phase: Phase::Asking { can_approve },
            agent,
            agents,
            items,
        };
    };
    let active = input.queue.active().map(|r| r.id);
    let mut items: Vec<Item> = input
        .ran()
        .iter()
        .map(|(id, shown, plan)| Item {
            display: *shown,
            agent: agent_of(*shown),
            text: plan_text(plan),
            plan: Some(plan.clone()),
            step: if Some(*id) == active {
                Step::Now
            } else {
                Step::Done
            },
            takeover: false,
        })
        .collect();
    items.extend(
        input
            .queue
            .waiting()
            // Only what can run: not a request on a display in Takeover,
            // nor one whose call stopped waiting, except the preview's.
            .filter(|r| {
                grant.admits(&r.owner) && (input.queue.runnable(r) || RUN_PREVIEW.with(Cell::get))
            })
            .filter_map(|r| {
                Some(Item {
                    display: r.owner.display,
                    agent: agent_of(r.owner.display),
                    text: request_text(input, r),
                    plan: Some(input.plan(r.id)?.clone()),
                    step: Step::Pending,
                    takeover: false,
                })
            }),
    );
    // A request runs or waits to run; otherwise the grant idles toward
    // its end.
    items.extend(previewed(input, agent_of));
    let mut running = active.is_some() || input.idle_seconds().is_none();
    if RUN_PREVIEW.with(Cell::get) && active.is_none() && !items.is_empty() {
        let now = 1.min(items.len() - 1);
        for (i, item) in items.iter_mut().enumerate() {
            item.step = match i.cmp(&now) {
                std::cmp::Ordering::Less => Step::Done,
                std::cmp::Ordering::Equal => Step::Now,
                std::cmp::Ordering::Greater => Step::Pending,
            };
        }
        running = true;
    }
    let phase = if running {
        let done = items.iter().filter(|i| i.step == Step::Done).count();
        Phase::Running { step: done + 1 }
    } else {
        Phase::Waiting {
            idle: input.idle_seconds().unwrap_or(0),
        }
    };
    let (agent, agents) = match phase {
        // The agent whose step runs is the one using the mouse.
        Phase::Running { .. } => {
            let current = items
                .iter()
                .find(|i| i.step == Step::Now)
                .or_else(|| items.iter().find(|i| i.step == Step::Pending))
                .map_or(fallback, |i| i.agent.clone());
            (Some(current), 1)
        }
        _ => one_agent(&items, fallback),
    };
    QueueView {
        phase,
        agent,
        agents,
        items,
    }
}

/// The one agent the items come from, or None, with how many there are.
fn one_agent(items: &[Item], fallback: String) -> (Option<String>, usize) {
    let mut names: Vec<&str> = items.iter().map(|i| i.agent.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    match names[..] {
        [] => (Some(fallback), 1),
        [one] => (Some(one.to_string()), 1),
        _ => (None, names.len()),
    }
}

/// The card's title, which the Input Bar shares during a grant.
pub fn queue_title(view: &QueueView) -> String {
    let n = view.items.len();
    let k = view.agents;
    let kind = view.kind();
    match (&view.phase, view.agent.as_deref()) {
        (Phase::Asking { .. }, Some(agent)) if n <= 1 => {
            tf!("{agent} wants to use your mouse", "{agent} 想用你的鼠标")
        }
        (Phase::Asking { .. }, Some(agent)) if kind == Kind::Drags => tf!(
            "{agent} wants your mouse for {n} drags",
            "{agent} 想用你的鼠标做 {n} 次拖拽"
        ),
        (Phase::Asking { .. }, Some(agent)) if kind == Kind::Clicks => tf!(
            "{agent} wants your mouse for {n} clicks",
            "{agent} 想用你的鼠标做 {n} 次点击"
        ),
        (Phase::Asking { .. }, Some(agent)) => tf!(
            "{agent} wants your mouse for {n} actions",
            "{agent} 想用你的鼠标做 {n} 个操作"
        ),
        (Phase::Asking { .. }, None) if kind == Kind::Drags => tf!(
            "{k} agents want your mouse for {n} drags",
            "{k} 个 agent 想用你的鼠标做 {n} 次拖拽"
        ),
        (Phase::Asking { .. }, None) if kind == Kind::Clicks => tf!(
            "{k} agents want your mouse for {n} clicks",
            "{k} 个 agent 想用你的鼠标做 {n} 次点击"
        ),
        (Phase::Asking { .. }, None) => tf!(
            "{k} agents want your mouse for {n} actions",
            "{k} 个 agent 想用你的鼠标做 {n} 个操作"
        ),
        (Phase::Running { .. }, agent) => {
            let agent = agent.unwrap_or("Agent");
            tf!("{agent} is using your mouse", "{agent} 正在用你的鼠标")
        }
        (Phase::Waiting { .. }, Some(agent)) => {
            tf!(
                "{agent} can still use your mouse",
                "{agent} 还可以用你的鼠标"
            )
        }
        (Phase::Waiting { .. }, None) => tf!(
            "{k} agents can still use your mouse",
            "{k} 个 agent 还可以用你的鼠标"
        ),
    }
}

/// Minutes and seconds: 01:05.
pub fn clock(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// Digits of one width, so a ticking clock doesn't jitter.
pub fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// What approval covers, or how to stop: secondary lines, the first
/// opening with a word in the label's color.
fn scope_lines(
    first: Option<(&'static str, &'static str)>,
    last: &'static str,
    zh: bool,
    theme: &Theme,
) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .text_size(px(11.))
        .line_height(px(if zh { 15. } else { 16. }))
        .text_color(theme.secondary)
        .when_some(first, |d, (strong, rest)| {
            let text = format!("{strong}{rest}");
            d.child(div().child(StyledText::new(text).with_highlights([(
                0..strong.len(),
                HighlightStyle {
                    color: Some(theme.label),
                    font_weight: Some(gpui::FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            )])))
        })
        .child(div().child(last))
}

/// One request of several, numbered as its path on the picture is.
fn list_row(i: usize, item: &Item, several: bool, zh: bool, theme: &Theme) -> gpui::Div {
    let (number, text) = match item.step {
        _ if item.takeover => (theme.secondary, theme.secondary),
        Step::Done => (theme.secondary, theme.secondary),
        Step::Now => (theme.red, theme.label),
        Step::Pending => (theme.orange, theme.label),
    };
    let leading = if zh { 16. } else { 17. };
    div()
        .flex_none()
        .flex()
        .items_start()
        .gap(px(8.))
        .py(px(if zh { 3. } else { 5. }))
        .when(i > 0, |d| d.border_t_1().border_color(theme.hairline))
        .text_size(px(12.))
        .line_height(px(leading))
        .child(
            div()
                .flex_none()
                .w(px(16.))
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .font_features(tabular())
                .text_color(number)
                .child((i + 1).to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .line_clamp(2)
                        .text_ellipsis()
                        .text_color(text)
                        .child(item.text.clone()),
                )
                .when(item.takeover, |d| {
                    d.child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .child(t("You are using this display", "你正在操作这个显示器")),
                    )
                }),
        )
        .when(several, |d| {
            d.child(
                div()
                    .flex_none()
                    .max_w(px(96.))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(theme.secondary)
                    .child(item.agent.clone()),
            )
        })
}

/// A drag's path or a click's point on the picture, in the picture's
/// points.
struct Mark {
    from: (f32, f32),
    /// Where a drag ends; a click has none.
    to: Option<(f32, f32)>,
    /// How far above the higher end the arc's control point sits.
    bow: f32,
    /// A click's presses, one ring each.
    presses: u8,
    step: Step,
    /// On a display in Takeover: Allow doesn't cover it.
    held: bool,
    number: Option<usize>,
}

/// The rings around a click's point: the first one's radius, and how far
/// apart they are.
const RING: f32 = 4.5;
const RING_STEP: f32 = 3.;

impl Mark {
    fn to(&self) -> (f32, f32) {
        self.to.unwrap_or(self.from)
    }

    /// The outermost ring's radius, or a drag's end ring's.
    fn reach(&self) -> f32 {
        RING + RING_STEP * f32::from(self.presses.max(1) - 1)
    }

    fn control(&self) -> (f32, f32) {
        let to = self.to();
        ((self.from.0 + to.0) / 2., self.from.1.min(to.1) - self.bow)
    }

    /// The point at `t` along the arc.
    fn at(&self, t: f32) -> (f32, f32) {
        let (c, to) = (self.control(), self.to());
        let u = 1. - t;
        (
            u * u * self.from.0 + 2. * u * t * c.0 + t * t * to.0,
            u * u * self.from.1 + 2. * u * t * c.1 + t * t * to.1,
        )
    }
}

/// How high a path's arc bows, in display points.
const BOW: f32 = 72.;

/// The display's picture with each drag and click of its own drawn on it,
/// a drag as an arc from a dot to a ring and a click as a dot in a ring
/// for each press: done ones grey, a done path dashed, the running one red
/// with the pointer on it, the rest orange, numbered when there are
/// several.
fn queue_picture(
    id: DisplayId,
    size: (f64, f64),
    frame: Option<CVPixelBuffer>,
    view: &QueueView,
    theme: &Theme,
) -> gpui::AnyElement {
    let fit = picture(size);
    let numbered = view.items.len() > 1;
    let marks: Vec<Mark> = view
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.display == id)
        .filter_map(|(i, item)| {
            let (area, from, to, presses) = match &item.plan {
                Some(Plan::Drag(drag)) => (drag.area, drag.from, Some(drag.to), 0),
                Some(Plan::Click(click)) => (click.area, click.at, None, click.click.count),
                // A menu item's place on the menu bar is known only once
                // its app's menus show there, when it runs.
                _ => return None,
            };
            if area.width <= 0. || area.height <= 0. {
                return None;
            }
            let sx = fit.0 / area.width as f32;
            let sy = fit.1 / area.height as f32;
            let at = |p: (f64, f64)| ((p.0 - area.x) as f32 * sx, (p.1 - area.y) as f32 * sy);
            Some(Mark {
                from: at(from),
                to: to.map(at),
                bow: BOW * sy,
                presses,
                step: item.step,
                held: item.takeover,
                number: numbered.then_some(i + 1),
            })
        })
        .collect();
    let color = |m: &Mark| match m.step {
        _ if m.held => theme.secondary.opacity(0.7),
        Step::Done => theme.secondary.opacity(0.7),
        Step::Now => theme.red,
        Step::Pending => theme.orange,
    };
    let badges: Vec<gpui::AnyElement> = marks
        .iter()
        .filter_map(|m| {
            let number = m.number?;
            let r = 6.5;
            // Above a drag's start, or a click's outer ring.
            let lift = if m.to.is_some() {
                12.
            } else {
                m.reach() + 3. + r
            };
            Some(
                div()
                    .absolute()
                    .left(px(m.from.0 - if m.to.is_some() { 1. } else { 0. } - r))
                    .top(px(m.from.1 - lift - r))
                    .size(px(r * 2.))
                    .rounded_full()
                    .bg(color(m))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .line_height(px(r * 2.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(gpui::white())
                    .child(number.to_string())
                    .into_any_element(),
            )
        })
        .collect();
    let colors: Vec<_> = marks.iter().map(color).collect();
    let picture = match frame {
        Some(frame) => surface(frame)
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        None => super::peek::waiting(id, 10.5).into_any_element(),
    };
    div()
        .flex_none()
        .flex()
        .justify_center()
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
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut gpui::App| {
                            paint_marks(bounds, &marks, &colors, window)
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
                .children(badges),
        )
        .into_any_element()
}

/// Paints each path: done ones first, so the running one lies on top.
fn paint_marks(bounds: Bounds<Pixels>, marks: &[Mark], colors: &[gpui::Hsla], window: &mut Window) {
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let p = |(x, y): (f32, f32)| point(px(ox + x), px(oy + y));
    let circle = |c: (f32, f32), r: f32| -> Vec<gpui::Point<Pixels>> {
        (0..32)
            .map(|i| {
                let a = i as f32 / 32. * std::f32::consts::TAU;
                p((c.0 + r * a.cos(), c.1 + r * a.sin()))
            })
            .collect()
    };
    let rank = |s: Step| match s {
        Step::Done => 0,
        Step::Pending => 1,
        Step::Now => 2,
    };
    let mut order: Vec<usize> = (0..marks.len()).collect();
    order.sort_by_key(|&i| rank(marks[i].step));
    for i in order {
        let (m, color) = (&marks[i], colors[i]);
        let Some(to) = m.to else {
            // A click: a dot, and a ring for each press.
            let mut dot = PathBuilder::fill();
            dot.add_polygon(&circle(m.from, 2.5), true);
            if let Ok(path) = dot.build() {
                window.paint_path(path, color);
            }
            for press in 0..m.presses.max(1) {
                let mut ring = PathBuilder::stroke(px(if press == 0 { 1.6 } else { 1.2 }));
                ring.add_polygon(&circle(m.from, RING + RING_STEP * f32::from(press)), true);
                if let Ok(path) = ring.build() {
                    window.paint_path(path, color.opacity(1. - 0.25 * f32::from(press)));
                }
            }
            if m.step == Step::Now {
                paint_pointer(m.from, &p, window);
            }
            continue;
        };
        let mut arc = PathBuilder::stroke(px(1.6));
        if m.step == Step::Done {
            arc = arc.dash_array(&[px(2.), px(3.)]);
        }
        arc.move_to(p(m.from));
        arc.curve_to(p(to), p(m.control()));
        if let Ok(path) = arc.build() {
            window.paint_path(path, color);
        }
        let mut dot = PathBuilder::fill();
        dot.add_polygon(&circle(m.from, 3.), true);
        if let Ok(path) = dot.build() {
            window.paint_path(path, color);
        }
        let mut ring = PathBuilder::stroke(px(1.6));
        ring.add_polygon(&circle(to, RING), true);
        if let Ok(path) = ring.build() {
            window.paint_path(path, color);
        }
        if m.step == Step::Now {
            paint_pointer(m.at(0.55), &p, window);
        }
    }
}

/// The arrow of `icons/pointer.svg`, white with a dark outline, its tip
/// near `at`.
fn paint_pointer(
    at: (f32, f32),
    p: &impl Fn((f32, f32)) -> gpui::Point<Pixels>,
    window: &mut Window,
) {
    const ARROW: [(f32, f32); 12] = [
        (5.2, 2.6),
        (6.3, 1.9),
        (19.7, 8.2),
        (19.6, 9.7),
        (14.0, 11.4),
        (18.2, 18.0),
        (17.9, 19.1),
        (15.8, 20.3),
        (14.7, 20.0),
        (10.6, 13.4),
        (6.6, 17.7),
        (5.2, 17.2),
    ];
    let points: Vec<_> = ARROW
        .iter()
        .map(|(x, y)| p((at.0 - 2. + x * 0.62, at.1 - 2. + y * 0.62)))
        .collect();
    let mut fill = PathBuilder::fill();
    fill.add_polygon(&points, true);
    if let Ok(path) = fill.build() {
        window.paint_path(path, gpui::white());
    }
    let mut outline = PathBuilder::stroke(px(0.9));
    outline.add_polygon(&points, true);
    if let Ok(path) = outline.build() {
        window.paint_path(path, gpui::black());
    }
}

/// The actions waiting for the user to leave an app, as the tiles' waiting
/// lines say, while the card shows for something else: a row per display,
/// each with Let the agent continue, whose press runs that display's
/// actions waiting on its app (`control::front_hold`). Empty while none
/// waits.
fn waiting_rows(
    core: &Core,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> Vec<gpui::Stateful<gpui::Div>> {
    core.displays
        .list
        .iter()
        .filter(|d| d.is_live() && !d.takeover)
        .filter_map(|d| {
            let app = core.input_queue.waiting_line(d.id)?;
            Some(waiting_row(d.id, d.agent_name(), &app, theme, shell))
        })
        .collect()
}

fn waiting_row(
    display: DisplayId,
    agent: &str,
    app: &str,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> gpui::Stateful<gpui::Div> {
    let text = tf!(
        "{agent} waits for you to leave {app}",
        "{agent} 等你离开 {app}"
    );
    let shell = shell.clone();
    // Its own id, as each display's row has the same button.
    div()
        .id(("waiting-row", display as usize))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .pt(px(6.))
        .border_t_1()
        .border_color(theme.hairline)
        .child(
            div()
                .flex_1()
                .min_w_0()
                // Two lines, so the app's name shows beside the button.
                .line_clamp(2)
                .text_ellipsis()
                .text_size(px(12.))
                .text_color(theme.label)
                .child(text),
        )
        .child(
            card_button(
                "input-let-continue",
                t("Let the agent continue", "让 Agent 继续"),
                Tone::Accent,
                theme,
            )
            .flex_none()
            .h(px(24.))
            .px(px(10.))
            .text_size(px(11.))
            .on_click(move |_, window, cx| {
                super::shell::let_continue(&shell, display, window, cx);
            }),
        )
}
