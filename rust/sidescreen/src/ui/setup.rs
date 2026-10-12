//! The setup window: Screen Recording and Accessibility, each with its
//! state and the one thing to do about it.
//!
//! It opens on its own when a permission is missing at launch, and from the
//! menu bar icon after. The shell keeps its state current; each row turns
//! green as the user grants, without a click here.

use super::shell::{Shell, tell};
use super::theme::{self, Theme};
use crate::i18n::t;
use crate::mac::permissions::{Permission, Status};
use gpui::{
    App, Context, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Render, Role, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, img,
    prelude::FluentBuilder, px, svg,
};
use std::sync::Arc;

pub const WIDTH: f32 = 480.;
pub const HEIGHT: f32 = 432.;

pub struct Setup {
    shell: WeakEntity<Shell>,
    pub status: Status,
    /// The permission whose pane the user was sent to.
    pub waiting: Option<Permission>,
    /// Screen Recording came on while running, and capture still fails:
    /// macOS applies it to a new process.
    pub restart: bool,
    /// The last agent that loaded Sidescreen's tools, if any did.
    pub agent: Option<String>,
    icon: Option<Arc<gpui::Image>>,
}

impl Setup {
    pub fn new(shell: WeakEntity<Shell>, status: Status) -> Setup {
        let icon = crate::mac::native::app_icon_png()
            .map(|png| Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)));
        Setup {
            shell,
            status,
            waiting: None,
            restart: false,
            agent: None,
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
                t("Sidescreen is ready", "Sidescreen 可以用了"),
                match self.agent.clone() {
                    Some(agent) => tf!(
                        "{agent} loaded Sidescreen's tools. Ask it to create a display: its apps run there, off your screen.",
                        "{agent} 已加载 Sidescreen 的工具。让它创建虚拟屏，它的 app 会在旁边运行，不占你的屏幕。"
                    ),
                    None => t(
                        "Ask an agent to create a display. Its apps run there, off your screen. No agent has loaded the tools yet: start a new session after connecting one.",
                        "让 agent 创建虚拟屏，它的 app 会在旁边运行，不占你的屏幕。还没有 agent 加载工具：接入后开一个新会话。",
                    )
                    .to_string(),
                },
            )
        } else {
            (
                t("Get Sidescreen working", "让 Sidescreen 开始工作"),
                t(
                    "Agents use apps on a display beside yours. Sidescreen needs to see that display and to click and type for the agent.",
                    "Agent 在旁边的虚拟屏上操作 app。Sidescreen 要看到那块屏，也要替 agent 点击和输入。",
                )
                .to_string(),
            )
        };
        let rows = Permission::ALL.map(|p| self.row(p, &theme));
        let shell = self.shell.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.card)
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
            .child(div().flex_1())
            .child(
                div()
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
                        ready.then_some(theme.accent),
                        &theme,
                        move |cx| tell(&shell, cx, |shell, cx| shell.close_setup(cx)),
                    )),
            )
    }
}

impl Setup {
    fn row(&self, p: Permission, theme: &Theme) -> impl IntoElement {
        let granted = self.status.has(p);
        let (icon, detail) = match p {
            Permission::ScreenRecording => (
                "icons/display.svg",
                t(
                    "Show agent displays and take screenshots",
                    "看到虚拟屏上的画面，给你看，也给 agent 截图",
                ),
            ),
            Permission::Accessibility => (
                "icons/pointer.svg",
                t(
                    "Click, type and place windows for agents",
                    "替 agent 点击、输入，把窗口摆到虚拟屏上",
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
                t("Reopen Sidescreen", "重新打开 Sidescreen").into(),
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
                    "macOS applies this after Sidescreen reopens",
                    "macOS 要重新打开 Sidescreen 才能录屏",
                )
                .to_string(),
                theme.orange,
            ))
        } else if waiting {
            Some((
                t(
                    "Turn on Sidescreen in System Settings. This row updates by itself.",
                    "在系统设置里打开 Sidescreen 的开关，这里会自动更新",
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
