//! The setup window: Screen Recording and Accessibility, each with its
//! state and the one thing to do about it.
//!
//! It opens on its own when a permission is missing at launch, and from the
//! menu bar icon after. The shell keeps its state current; each row turns
//! green as the user grants, without a click here.

use super::shell::{Shell, tell};
use super::theme::{self, Theme};
use crate::i18n::t;
use crate::mac::notify;
use crate::mac::permissions::{Permission, Status};
use gpui::{
    App, Context, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Render, Role, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, img,
    prelude::FluentBuilder, px, svg,
};
use std::sync::Arc;

pub const WIDTH: f32 = 520.;
/// The height it opens at; it then fits its content, which grows by a
/// line when a row explains what to do in System Settings.
pub const HEIGHT: f32 = 496.;

pub struct Setup {
    shell: WeakEntity<Shell>,
    /// Centered once its height fits the content.
    centered: std::rc::Rc<std::cell::Cell<bool>>,
    pub status: Status,
    /// The permission whose pane the user was sent to.
    pub waiting: Option<Permission>,
    /// Screen Recording came on while running, and capture still fails:
    /// macOS applies it to a new process.
    pub restart: bool,
    /// The last agent that loaded Agent Screen's tools, if any did.
    pub agent: Option<String>,
    /// A Connect is running.
    pub connecting: bool,
    /// Clients whose config has Agent Screen's server.
    pub registered: Vec<&'static str>,
    /// Whether macOS lets Agent Screen notify.
    pub notifications: notify::Auth,
    /// The ColorSync helper (ADR 0009), a setup running, its last error.
    pub colorsync: Option<crate::mac::colorsync::Helper>,
    pub colorsync_busy: bool,
    pub colorsync_error: Option<String>,
    icon: Option<Arc<gpui::Image>>,
}

impl Setup {
    pub fn new(shell: WeakEntity<Shell>, status: Status) -> Setup {
        let icon = crate::mac::native::app_icon_png()
            .map(|png| Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)));
        Setup {
            shell,
            centered: Default::default(),
            status,
            waiting: None,
            restart: false,
            agent: None,
            connecting: false,
            registered: Vec::new(),
            notifications: notify::Auth::Unknown,
            colorsync: None,
            colorsync_busy: false,
            colorsync_error: None,
            icon,
        }
    }

    /// The permission to grant next, top to bottom.
    fn next(&self) -> Option<Permission> {
        self.status.missing().into_iter().next()
    }
}

impl Render for Setup {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let ready = self.status.complete() && !self.restart;
        let (title, subtitle) = if ready {
            (
                t("Agent Screen is ready", "Agent Screen 可以用了"),
                match self.agent.clone() {
                    Some(agent) => tf!(
                        "{agent} loaded Agent Screen's tools. Ask your agent to create a display: its apps run there, off your screen.",
                        "{agent} 已加载 Agent Screen 的工具。让 agent 创建显示器，它的 app 会在旁边运行，不占你的屏幕。"
                    ),
                    None if !self.registered.is_empty() => t(
                        "Start a new agent session and ask it to create a display. Its apps run there, off your screen.",
                        "开一个新的 agent 会话，让它创建显示器。它的 app 会在旁边运行，不占你的屏幕。",
                    )
                    .to_string(),
                    None => t(
                        "Connect your agent below, then start a new session and ask it to create a display. Its apps run there, off your screen.",
                        "在下面接入 agent，再开一个新会话，让它创建显示器。它的 app 会在旁边运行，不占你的屏幕。",
                    )
                    .to_string(),
                },
            )
        } else {
            (
                t("Get Agent Screen working", "让 Agent Screen 开始工作"),
                t(
                    "Agents use apps on a display beside yours. Agent Screen needs to see that display and to click and type for the agent.",
                    "Agent 在旁边的显示器上操作 app。Agent Screen 要看到那块显示器，也要替 agent 点击和输入。",
                )
                .to_string(),
            )
        };
        let mut rows: Vec<gpui::AnyElement> = Permission::ALL
            .map(|p| self.row(p, &theme).into_any_element())
            .into_iter()
            .collect();
        rows.push(self.notifications_row(&theme).into_any_element());
        rows.push(self.colorsync_row(&theme).into_any_element());
        rows.push(self.agent_row(&theme).into_any_element());
        let shell = self.shell.clone();
        let content = div()
            .flex()
            .flex_col()
            .text_color(theme.label)
            .px(px(32.))
            .pt(px(44.))
            .pb(px(24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .when_some(self.icon.clone(), |d, icon| {
                        d.child(img(icon).size(px(64.)).mb(px(6.)))
                    })
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_weight(FontWeight::BOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.secondary)
                            .text_center()
                            .max_w(px(380.))
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .mt(px(22.))
                    .rounded(px(12.))
                    .bg(theme.control)
                    .flex()
                    .flex_col()
                    .children(rows.into_iter().enumerate().map(|(i, row)| {
                        div()
                            .when(i > 0, |d| d.border_t_1().border_color(theme.card_border))
                            .child(row)
                    })),
            )
            .child(
                div()
                    .mt(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            // Wraps instead of pushing the button out.
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .child(t(
                                "Used only on this Mac. Turn off anytime in System Settings.",
                                "权限只用在这台 Mac 上，随时可以在系统设置里关掉。",
                            )),
                    )
                    .child(button(
                        "setup-close",
                        if ready {
                            t("Done", "完成")
                        } else {
                            t("Not Now", "稍后")
                        }
                        .into(),
                        // Connect is the next step until an agent loads the tools.
                        (ready && (self.agent.is_some() || !self.registered.is_empty()))
                            .then_some(theme.accent),
                        &theme,
                        move |cx| tell(&shell, cx, |shell, cx| shell.close_setup(cx)),
                    )),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.card)
            .on_children_prepainted({
                let centered = self.centered.clone();
                move |bounds, window, _| {
                    let Some(content) = bounds.first() else {
                        return;
                    };
                    let height = content.size.height.ceil();
                    if (window.viewport_size().height - height).abs() > px(1.) {
                        window.resize(gpui::size(px(WIDTH), height));
                        return;
                    }
                    // It opens at a guessed height and then fits its
                    // content: once it has, center it on its screen.
                    if !centered.replace(true) {
                        center(window);
                    }
                }
            })
            .child(content)
    }
}

/// Center the window on its screen, below the menu bar.
fn center(window: &Window) {
    let Some(ns) = crate::mac::native::ns_window(window) else {
        return;
    };
    let Some(screen) = ns.screen() else {
        return;
    };
    let area = screen.visibleFrame();
    let mut frame = crate::mac::native::frame(&ns);
    frame.origin.x = area.origin.x + (area.size.width - frame.size.width) / 2.0;
    frame.origin.y = area.origin.y + ((area.size.height - frame.size.height) / 2.0).max(0.0);
    crate::mac::native::set_frame(&ns, frame);
}

impl Setup {
    /// Notifications: optional, so setup is complete without them.
    fn notifications_row(&self, theme: &Theme) -> impl IntoElement {
        let green: Hsla = gpui::rgb(0x30d158).into();
        let shell = self.shell.clone();
        let on = self.notifications == notify::Auth::Allowed;
        let action: gpui::AnyElement = match self.notifications {
            notify::Auth::Allowed => div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(green)
                .child(
                    svg()
                        .path("icons/check.svg")
                        .size(px(14.))
                        .text_color(green),
                )
                .child(t("On", "已开启"))
                .into_any_element(),
            notify::Auth::Unknown => div().into_any_element(),
            auth => button(
                "setup-notifications",
                if auth == notify::Auth::Denied {
                    t("Open Settings…", "打开设置…")
                } else {
                    t("Allow…", "允许…")
                }
                .into(),
                (self.status.complete() && auth == notify::Auth::NotAsked).then_some(theme.accent),
                theme,
                move |cx| tell(&shell, cx, |shell, _| shell.allow_notifications()),
            )
            .into_any_element(),
        };
        let detail = if self.notifications == notify::Auth::Denied {
            t(
                "Off in System Settings. Turn on Allow Notifications for Agent Screen.",
                "在系统设置里关着。打开 Agent Screen 的“允许通知”。",
            )
        } else {
            t(
                "Tells you when an agent needs your help or is done with its display",
                "agent 需要你帮忙、或用完显示器时提醒你",
            )
        };
        let color = if on { green } else { theme.accent };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(34.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color.opacity(if on { 0.18 } else { 0.16 }))
                    .child(svg().path("icons/bell.svg").size(px(18.)).text_color(color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("Notifications", "通知")),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.secondary)
                            .child(detail),
                    ),
            )
            .child(action)
    }

    /// The ColorSync helper: optional, like notifications (ADR 0009).
    fn colorsync_row(&self, theme: &Theme) -> impl IntoElement {
        use crate::mac::colorsync::Helper;
        let green: Hsla = gpui::rgb(0x30d158).into();
        let shell = self.shell.clone();
        let on = self.colorsync == Some(Helper::Ready);
        let action: gpui::AnyElement = if on {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(green)
                .child(
                    svg()
                        .path("icons/check.svg")
                        .size(px(14.))
                        .text_color(green),
                )
                .child(t("On", "已开启"))
                .into_any_element()
        } else if self.colorsync_busy {
            div()
                .text_size(px(12.))
                .text_color(theme.secondary)
                .child(t("Waiting for your password…", "等你输入密码…"))
                .into_any_element()
        } else {
            button(
                "setup-colorsync",
                if self.colorsync == Some(Helper::Outdated) {
                    t("Update…", "更新…")
                } else {
                    t("Set Up…", "设置…")
                }
                .into(),
                None,
                theme,
                move |cx| tell(&shell, cx, |shell, cx| shell.set_up_colorsync(cx)),
            )
            .into_any_element()
        };
        let detail = match &self.colorsync_error {
            Some(error) => tf!("Didn't work: {error}", "没有成功：{error}"),
            None if on => t(
                "Keeps macOS's list of displays short, so many displays don't make your Mac stutter",
                "精简 macOS 的显示器列表，屏幕多了 Mac 也不卡",
            )
            .to_string(),
            None => t(
                "macOS rereads every display it has ever seen whenever displays change, and with many it stutters. Agent Screen keeps that list short. Asks for your administrator password once.",
                "显示器每次变化，macOS 都会重读见过的所有显示器，屏幕多了就卡。Agent Screen 帮你精简这张表。需要输入一次管理员密码。",
            )
            .to_string(),
        };
        let color = if on { green } else { theme.accent };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(34.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color.opacity(if on { 0.18 } else { 0.16 }))
                    .child(
                        svg()
                            .path("icons/display.svg")
                            .size(px(18.))
                            .text_color(color),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("Smooth Display Changes", "显示器切换不卡顿")),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.secondary)
                            .child(detail),
                    ),
            )
            .child(action)
    }

    /// Connecting Claude Code and Codex, the step after the permissions.
    fn agent_row(&self, theme: &Theme) -> impl IntoElement {
        let green: Hsla = gpui::rgb(0x30d158).into();
        let shell = self.shell.clone();
        let loaded = self.agent.is_some();
        let action: gpui::AnyElement = if loaded {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(green)
                .child(
                    svg()
                        .path("icons/check.svg")
                        .size(px(14.))
                        .text_color(green),
                )
                .child(t("Connected", "已接入"))
                .into_any_element()
        } else if self.connecting {
            div()
                .text_size(px(12.))
                .text_color(theme.secondary)
                .child(t("Connecting…", "正在接入…"))
                .into_any_element()
        } else {
            let again = !self.registered.is_empty();
            button(
                "setup-connect",
                if again {
                    t("Connect Again…", "重新接入…")
                } else {
                    t("Connect…", "接入…")
                }
                .into(),
                (self.status.complete() && !again).then_some(theme.accent),
                theme,
                move |cx| tell(&shell, cx, |shell, cx| shell.connect_agents(cx)),
            )
            .into_any_element()
        };
        let detail = match (&self.agent, self.registered.as_slice()) {
            (Some(agent), _) => tf!("{agent} loaded the tools", "{agent} 已加载工具"),
            (None, []) => t(
                "Add Agent Screen to Claude Code and Codex, then start a new session",
                "把 Agent Screen 加到 Claude Code 和 Codex，再开一个新会话",
            )
            .to_string(),
            (None, clients) => tf!(
                "Added to {}. Start a new session to load the tools",
                "已加到 {}。开一个新会话就能加载工具",
                clients.join(t(" and ", " 和 "))
            ),
        };
        let color = if loaded { green } else { theme.accent };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(34.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color.opacity(if loaded { 0.18 } else { 0.16 }))
                    .child(svg().path("icons/plus.svg").size(px(18.)).text_color(color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("Your agents", "你的 agent")),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.secondary)
                            .child(detail),
                    ),
            )
            .child(action)
    }

    fn row(&self, p: Permission, theme: &Theme) -> impl IntoElement {
        let granted = self.status.has(p);
        let (icon, detail) = match p {
            Permission::ScreenRecording => (
                "icons/display.svg",
                t(
                    "Show agent displays and take screenshots",
                    "看到显示器上的画面，给你看，也给 agent 截图",
                ),
            ),
            Permission::Accessibility => (
                "icons/pointer.svg",
                t(
                    "Click, type and place windows for agents",
                    "替 agent 点击、输入，把窗口摆到显示器上",
                ),
            ),
        };
        let green: Hsla = gpui::rgb(0x30d158).into();
        let shell = self.shell.clone();
        let restart_here = p == Permission::ScreenRecording && granted && self.restart;
        let waiting = self.waiting == Some(p) && !granted;
        let action: gpui::AnyElement = if restart_here {
            button(
                "setup-restart",
                t("Reopen Agent Screen", "重新打开 Agent Screen").into(),
                Some(theme.accent),
                theme,
                move |cx| tell(&shell, cx, |shell, cx| shell.relaunch(cx)),
            )
            .into_any_element()
        } else if granted {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(green)
                .child(
                    svg()
                        .path("icons/check.svg")
                        .size(px(14.))
                        .text_color(green),
                )
                .child(t("On", "已开启"))
                .into_any_element()
        } else {
            let next = self.next() == Some(p);
            button(
                match p {
                    Permission::ScreenRecording => "setup-grant-screen",
                    Permission::Accessibility => "setup-grant-ax",
                },
                if waiting {
                    t("Open Settings Again", "重新打开设置")
                } else {
                    t("Turn On…", "开启…")
                }
                .into(),
                (next && !waiting).then_some(theme.accent),
                theme,
                move |cx| tell(&shell, cx, move |shell, cx| shell.grant(p, cx)),
            )
            .into_any_element()
        };
        let note = if restart_here {
            Some((
                t(
                    "macOS applies this after Agent Screen reopens",
                    "macOS 要重新打开 Agent Screen 才能录屏",
                )
                .to_string(),
                theme.orange,
            ))
        } else if waiting {
            Some((
                t(
                    "Turn on Agent Screen in System Settings. This row updates by itself.",
                    "在系统设置里打开 Agent Screen 的开关，这里会自动更新",
                )
                .to_string(),
                theme.secondary,
            ))
        } else {
            None
        };
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(34.))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if granted {
                        green.opacity(0.18)
                    } else {
                        theme.accent.opacity(0.16)
                    })
                    .child(svg().path(icon).size(px(18.)).text_color(if granted {
                        green
                    } else {
                        theme.accent
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(p.name()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.secondary)
                            .child(detail),
                    )
                    .when_some(note, |d, (text, color)| {
                        d.child(div().text_size(px(11.)).text_color(color).child(text))
                    }),
            )
            .child(action)
    }
}

fn button(
    id: &'static str,
    title: SharedString,
    prominent: Option<Hsla>,
    theme: &Theme,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let (bg, hover, fg) = match prominent {
        Some(color) => (color.opacity(0.9), color, gpui::white()),
        None => (theme.control, theme.control_hover, theme.label),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(title.clone())
        .flex_none()
        .h(px(28.))
        .px(px(14.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.7))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| action(cx))
        .child(title)
}
