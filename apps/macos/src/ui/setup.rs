//! The Settings window: the permissions, the agents, and Agent Displays,
//! each with its state and the one thing to do about it.
//!
//! It opens on its own when a permission is missing at launch, and from
//! Settings… in the menu bar icon's menu after. The shell keeps its state
//! current; each row turns green as the user grants, without a click here.

use super::shell::{Shell, tell};
use super::theme::{self, Theme};
use crate::i18n::t;
use crate::mac::aerospace::Ignoring;
use crate::mac::native::{self, MenuAction, Native};
use crate::mac::notify;
use crate::mac::permissions::{Permission, Status};
use gpui::{
    AnyElement, App, Context, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton,
    ObjectFit, ParentElement, Render, Role, SharedString, StatefulInteractiveElement, Styled,
    StyledImage, WeakEntity, Window, div, img, prelude::FluentBuilder, px, svg,
};
use std::path::PathBuf;
use std::sync::Arc;

pub const WIDTH: f32 = 520.;
/// The height it opens at; it then fits its content, which grows by a
/// line when a row explains what to do in System Settings.
pub const HEIGHT: f32 = 640.;

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
    /// The running AeroSpace and its config, None without AeroSpace.
    pub aerospace: Option<Ignoring>,
    /// The user's workspaces AeroSpace pins to a monitor number an Agent
    /// Display took.
    pub pinned: Vec<String>,
    /// The picture the user chose for Agent Displays.
    pub wallpaper: Option<PathBuf>,
    icon: Arc<gpui::Image>,
}

/// The app icon, flat, from AppIcon.icns. Asking macOS for the Icon
/// Composer icon's pixels crashes ImageIO on macOS 26.
const ICON: &[u8] = include_bytes!("../../assets/app-icon.png");

impl Setup {
    pub fn new(shell: WeakEntity<Shell>, status: Status) -> Setup {
        let icon = Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            ICON.to_vec(),
        ));
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
            aerospace: None,
            pinned: Vec::new(),
            wallpaper: None,
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
        let header = if ready {
            let status = match self.agent.clone() {
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
                    "Connect your agent below, then ask it to create a display.",
                    "在下面接入 agent，再让它创建显示器。",
                )
                .to_string(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(img(self.icon.clone()).flex_none().size(px(52.)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::BOLD)
                                .child("Agent Screen"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.secondary)
                                .child(status),
                        ),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(img(self.icon.clone()).size(px(64.)).mb(px(6.)))
                .child(
                    div()
                        .text_size(px(19.))
                        .font_weight(FontWeight::BOLD)
                        .child(t("Get Agent Screen working", "让 Agent Screen 开始工作")),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme.secondary)
                        .text_center()
                        .max_w(px(380.))
                        .child(t(
                            "Agents use apps on a display beside yours. Agent Screen needs to see that display and to click and type for the agent.",
                            "Agent 在旁边的显示器上操作 app。Agent Screen 要看到那块显示器，也要替 agent 点击和输入。",
                        )),
                )
        };
        let mut permissions: Vec<AnyElement> = Permission::ALL
            .map(|p| self.row(p, &theme).into_any_element())
            .into_iter()
            .collect();
        permissions.push(self.notifications_row(&theme).into_any_element());
        // Until setup is done, only what makes the Mac work well with agents.
        let mut displays = vec![self.colorsync_row(&theme).into_any_element()];
        if ready {
            displays.push(self.wallpaper_row(&theme).into_any_element());
            if self.aerospace.is_some() || !self.pinned.is_empty() {
                displays.push(self.aerospace_row(&theme).into_any_element());
            }
        }
        let shell = self.shell.clone();
        let content = div()
            .flex()
            .flex_col()
            .text_color(theme.label)
            .px(px(32.))
            .pt(px(if ready { 36. } else { 44. }))
            .pb(px(24.))
            .child(header)
            .child(section(t("Permissions", "权限"), permissions, &theme))
            .child(section(
                t("Agents", "Agent"),
                vec![
                    self.agent_row(&theme).into_any_element(),
                    browser_row(&theme).into_any_element(),
                ],
                &theme,
            ))
            .child(section(
                t("Agent Displays", "Agent 显示器"),
                displays,
                &theme,
            ))
            .child(
                div()
                    .mt(px(18.))
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
    let Some(ns) = native::ns_window(window) else {
        return;
    };
    let Some(screen) = ns.screen() else {
        return;
    };
    let area = screen.visibleFrame();
    let mut frame = native::frame(&ns);
    frame.origin.x = area.origin.x + (area.size.width - frame.size.width) / 2.0;
    frame.origin.y = area.origin.y + ((area.size.height - frame.size.height) / 2.0).max(0.0);
    native::set_frame(&ns, frame);
}

fn green() -> Hsla {
    gpui::rgb(0x30d158).into()
}

/// A titled group of rows.
fn section(title: &str, rows: Vec<AnyElement>, theme: &Theme) -> impl IntoElement {
    div()
        .mt(px(16.))
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(4.))
                .pb(px(5.))
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.secondary)
                .child(title.to_string()),
        )
        .child(
            div()
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
}

/// Agent Screen's extension in the user's browser (ADR 0012): drags in
/// pages signed in as them.
fn browser_row(theme: &Theme) -> impl IntoElement {
    let connected = crate::mac::browser::connected();
    let on = !connected.is_empty();
    let detail: SharedString = if on {
        tf!(
            "Connected in {}. Agents drag in its pages, signed in as you",
            "已在 {} 里接入。agent 可以在网页里拖动，用的是你的登录",
            connected.join(", ")
        )
        .into()
    } else {
        t(
            "Add Agent Screen's extension to Chrome, Dia, Arc or Edge so agents can drag in pages signed in as you",
            "把 Agent Screen 的扩展加到 Chrome、Dia、Arc 或 Edge，agent 就能在你登录的网页里拖动",
        )
        .into()
    };
    let action = button(
        "setup-browser",
        if on {
            t("Add Again…", "再次添加…")
        } else {
            t("Add…", "添加…")
        }
        .into(),
        None,
        theme,
        |_| native::send(Native::Menu(MenuAction::AddBrowserExtension)),
    )
    .into_any_element();
    item(
        badge(
            "icons/pointer.svg",
            if on { green() } else { theme.accent },
            on,
        ),
        t("Your Browser", "你的浏览器").into(),
        detail,
        None,
        action,
        theme,
    )
}

/// A row's icon on a tint of its color: green once it's on.
fn badge(icon: &'static str, color: Hsla, on: bool) -> gpui::Div {
    div()
        .flex_none()
        .size(px(34.))
        .rounded(px(9.))
        .flex()
        .items_center()
        .justify_center()
        .bg(color.opacity(if on { 0.18 } else { 0.16 }))
        .child(svg().path(icon).size(px(18.)).text_color(color))
}

/// A row's state when there's nothing to do: a green check and `label`.
fn done(label: &'static str) -> AnyElement {
    let green = green();
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
        .child(label)
        .into_any_element()
}

/// What a row says while something runs.
fn busy(label: &'static str, theme: &Theme) -> AnyElement {
    div()
        .text_size(px(12.))
        .text_color(theme.secondary)
        .child(label)
        .into_any_element()
}

/// One row: its icon, a title, what it does, an optional note under it,
/// and its state or button on the right.
fn item(
    lead: impl IntoElement,
    title: SharedString,
    detail: SharedString,
    note: Option<(String, Hsla)>,
    action: AnyElement,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .child(lead)
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
                        .child(title),
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

impl Setup {
    /// Notifications: optional, so setup is complete without them.
    fn notifications_row(&self, theme: &Theme) -> impl IntoElement {
        let shell = self.shell.clone();
        let on = self.notifications == notify::Auth::Allowed;
        let action = match self.notifications {
            notify::Auth::Allowed => done(t("On", "已开启")),
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
                "When an agent needs your help or is done",
                "agent 需要你帮忙或用完显示器时提醒你",
            )
        };
        let color = if on { green() } else { theme.accent };
        item(
            badge("icons/bell.svg", color, on),
            t("Notifications", "通知").into(),
            detail.into(),
            None,
            action,
            theme,
        )
    }

    /// The ColorSync helper: optional, like notifications (ADR 0009).
    fn colorsync_row(&self, theme: &Theme) -> impl IntoElement {
        use crate::mac::colorsync::Helper;
        let shell = self.shell.clone();
        let on = self.colorsync == Some(Helper::Ready);
        let action = if on {
            done(t("On", "已开启"))
        } else if self.colorsync_busy {
            busy(t("Waiting for your password…", "等你输入密码…"), theme)
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
                "Keeps many displays from making your Mac stutter",
                "屏幕多了 Mac 也不卡",
            )
            .to_string(),
            None => t(
                "Keeps many displays from making your Mac stutter. Asks for your administrator password once",
                "屏幕多了 Mac 也不卡。需要输入一次管理员密码",
            )
            .to_string(),
        };
        let color = if on { green() } else { theme.accent };
        item(
            badge("icons/display.svg", color, on),
            t("Smooth Display Changes", "显示器切换不卡顿").into(),
            detail.into(),
            None,
            action,
            theme,
        )
    }

    /// The picture Agent Displays show, the user's or Agent Screen's own.
    fn wallpaper_row(&self, theme: &Theme) -> impl IntoElement {
        let lead = match self.wallpaper.clone() {
            Some(path) => div()
                .flex_none()
                .size(px(34.))
                .rounded(px(9.))
                .overflow_hidden()
                .child(img(path).size(px(34.)).object_fit(ObjectFit::Cover))
                .into_any_element(),
            None => badge("icons/image.svg", theme.accent, false).into_any_element(),
        };
        let detail = if self.wallpaper.is_some() {
            t(
                "Your picture, in the Viewer and recordings. Agents' screenshots leave it out",
                "你选的图片，出现在 Viewer 和录屏里，agent 截图不含它",
            )
        } else {
            t(
                "Agent Screen's, which names each display. Agents' screenshots leave it out",
                "Agent Screen 自带的，标出每块显示器。agent 截图不含它",
            )
        };
        let choose = button(
            "setup-wallpaper",
            t("Choose…", "选择…").into(),
            None,
            theme,
            |_| native::send(Native::Menu(MenuAction::ChooseWallpaper)),
        );
        let action = div()
            .flex()
            .gap(px(8.))
            .when(self.wallpaper.is_some(), |d| {
                d.child(button(
                    "setup-wallpaper-default",
                    t("Use Default", "改回默认").into(),
                    None,
                    theme,
                    |_| native::send(Native::Menu(MenuAction::DefaultWallpaper)),
                ))
            })
            .child(choose)
            .into_any_element();
        item(
            lead,
            t("Wallpaper", "壁纸").into(),
            detail.into(),
            None,
            action,
            theme,
        )
    }

    /// AeroSpace, when it runs: whether it leaves Agent Displays alone.
    fn aerospace_row(&self, theme: &Theme) -> impl IntoElement {
        let ignoring = self.aerospace == Some(Ignoring::Yes);
        let (color, action) = if !self.pinned.is_empty() {
            (
                theme.orange,
                button(
                    "setup-aerospace-config",
                    t("Open Config…", "打开配置…").into(),
                    None,
                    theme,
                    |_| native::send(Native::Menu(MenuAction::OpenAeroSpaceConfig)),
                )
                .into_any_element(),
            )
        } else if ignoring {
            (green(), done(t("On", "已开启")))
        } else {
            (
                theme.accent,
                button(
                    "setup-aerospace",
                    t("Set Up…", "设置…").into(),
                    None,
                    theme,
                    |_| native::send(Native::Menu(MenuAction::IgnoreInAeroSpace)),
                )
                .into_any_element(),
            )
        };
        let detail = if ignoring {
            t(
                "Leaves Agent Screen's displays alone",
                "不管 Agent Screen 的显示器",
            )
        } else {
            t(
                "Otherwise it puts your workspaces on agents' displays",
                "不然它会把你的工作区放到 agent 的显示器上",
            )
        };
        let note = (!self.pinned.is_empty()).then(|| {
            (
                tf!(
                    "Workspace {} is pinned to a monitor number an agent's display took",
                    "工作区 {} 按编号固定到了一块显示器，这个编号被 agent 的显示器占了",
                    self.pinned.join(", ")
                ),
                theme.orange,
            )
        });
        item(
            badge("icons/tiles.svg", color, ignoring && self.pinned.is_empty()),
            t(
                "Keep AeroSpace Off Agent Displays",
                "让 AeroSpace 不管 Agent 显示器",
            )
            .into(),
            detail.into(),
            note,
            action,
            theme,
        )
    }

    /// Connecting Claude Code and Codex, the step after the permissions.
    fn agent_row(&self, theme: &Theme) -> impl IntoElement {
        let shell = self.shell.clone();
        let loaded = self.agent.is_some();
        let action = if loaded {
            done(t("Connected", "已接入"))
        } else if self.connecting {
            busy(t("Connecting…", "正在接入…"), theme)
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
                "Add Agent Screen to both, then start a new session",
                "把 Agent Screen 加到这两个 agent，再开一个新会话",
            )
            .to_string(),
            (None, clients) => tf!(
                "Added to {}. Start a new session to load the tools",
                "已加到 {}。开一个新会话就能加载工具",
                clients.join(t(" and ", " 和 "))
            ),
        };
        let color = if loaded { green() } else { theme.accent };
        item(
            badge("icons/plus.svg", color, loaded),
            t("Claude Code and Codex", "Claude Code 和 Codex").into(),
            detail.into(),
            None,
            action,
            theme,
        )
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
        let shell = self.shell.clone();
        let restart_here = p == Permission::ScreenRecording && granted && self.restart;
        let waiting = self.waiting == Some(p) && !granted;
        let action = if restart_here {
            button(
                "setup-restart",
                t("Reopen Agent Screen", "重新打开 Agent Screen").into(),
                Some(theme.accent),
                theme,
                move |cx| tell(&shell, cx, |shell, cx| shell.relaunch(cx)),
            )
            .into_any_element()
        } else if granted {
            done(t("On", "已开启"))
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
        let color = if granted { green() } else { theme.accent };
        item(
            badge(icon, color, granted),
            p.name().into(),
            detail.into(),
            note,
            action,
            theme,
        )
    }
}

pub(crate) fn button(
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
