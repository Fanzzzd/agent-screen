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
use agent_screen_model::location::Temporary;
use gpui::{
    AnyElement, App, Context, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ObjectFit, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, WeakEntity, Window, div, img,
    prelude::FluentBuilder, px, svg,
};
use std::path::PathBuf;
use std::sync::Arc;

pub const WIDTH: f32 = 520.;
/// The height it opens at; it then fits its content, which grows by a
/// line when a row explains what to do in System Settings.
pub const HEIGHT: f32 = 640.;

pub struct Setup {
    shell: WeakEntity<Shell>,
    /// The window's keys: Tab moves between buttons, Escape closes it.
    focus: FocusHandle,
    focused: bool,
    /// Centered once its height fits the content.
    centered: std::rc::Rc<std::cell::Cell<bool>>,
    pub status: Status,
    /// The permission whose pane the user was sent to.
    pub waiting: Option<Permission>,
    /// That permission is still off a while after, with Agent Screen in
    /// its list.
    pub still_off: bool,
    /// Screen Recording came on while running, and capture still fails:
    /// macOS applies it to a new process.
    pub restart: bool,
    /// The agents that loaded Agent Screen's tools since it started.
    pub agent: Option<String>,
    /// The client that last loaded the tools, maybe before a restart, and
    /// the day it did.
    pub earlier: Option<(String, String)>,
    /// The MCP command was just copied.
    pub copied: bool,
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
    /// Whether agents may report Agent Screen bugs (ADR 0024).
    pub reports_on: bool,
    /// Every display shares one macOS Space ("Displays have separate
    /// Spaces" off): a row says what that does and where to change it.
    pub spaces_shared: bool,
    /// The bundle runs from the disk image, Downloads or a translocated
    /// copy: a banner asks to move it first.
    pub temporary: Option<Temporary>,
    pub moving: MoveStep,
    icon: Arc<gpui::Image>,
}

/// Move to Applications, while the bundle runs from a temporary place.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MoveStep {
    #[default]
    Idle,
    /// Applications has an Agent Screen: replace it?
    AskReplace,
    Copying,
    Failed(String),
}

/// The app icon, flat, from AppIcon.icns. Asking macOS for the Icon
/// Composer icon's pixels crashes ImageIO on macOS 26.
const ICON: &[u8] = include_bytes!("../../assets/app-icon.png");

impl Setup {
    pub fn new(shell: WeakEntity<Shell>, status: Status, cx: &mut Context<Setup>) -> Setup {
        let icon = Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            ICON.to_vec(),
        ));
        Setup {
            shell,
            focus: cx.focus_handle(),
            focused: false,
            centered: Default::default(),
            status,
            waiting: None,
            still_off: false,
            restart: false,
            agent: None,
            earlier: None,
            copied: false,
            connecting: false,
            registered: Vec::new(),
            notifications: notify::Auth::Unknown,
            colorsync: None,
            colorsync_busy: false,
            colorsync_error: None,
            aerospace: None,
            pinned: Vec::new(),
            wallpaper: None,
            reports_on: true,
            spaces_shared: false,
            temporary: None,
            moving: MoveStep::Idle,
            icon,
        }
    }

    /// The permission to grant next, top to bottom.
    fn next(&self) -> Option<Permission> {
        self.status.missing().into_iter().next()
    }
}

impl Render for Setup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        if !self.focused {
            window.focus(&self.focus, cx);
            self.focused = true;
        }
        let ready = self.status.complete() && !self.restart;
        let header = if ready {
            let status = match self.agent.clone() {
                Some(agent) => tf!(
                    "{agent} loaded Agent Screen's tools. Ask your agent to create a display: its apps run there, off your screen.",
                    "{agent} 已加载 Agent Screen 的工具。让 agent 创建显示器，它的 app 将在其中运行，不占用你的屏幕。"
                ),
                None if !self.registered.is_empty() => t(
                    "Start a new agent session and ask it to create a display. Its apps run there, off your screen.",
                    "新建一个 agent 会话，让它创建显示器。它的 app 将在其中运行，不占用你的屏幕。",
                )
                .to_string(),
                None => t(
                    "Connect your agent below, then ask it to create a display.",
                    "先在下方接入 agent，再让它创建显示器。",
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
                            "Agent 在独立的显示器上操作 app。Agent Screen 需要读取该显示器的画面，并代 agent 点击和输入。",
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
            if self.spaces_shared {
                displays.push(self.spaces_row(&theme).into_any_element());
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
            .when_some(self.temporary, |d, why| {
                d.child(self.location_banner(why, &theme))
            })
            .child(header)
            .child(section(t("Permissions", "权限"), permissions, &theme))
            .child(section(
                t("Agents", "Agent"),
                vec![
                    self.agent_row(&theme, cx.weak_entity()).into_any_element(),
                    self.reports_row(&theme).into_any_element(),
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
                                "权限仅在本机使用，可随时在系统设置中关闭。",
                            )),
                    )
                    .child(key_button(
                        "setup-close",
                        if ready {
                            t("Done", "完成")
                        } else {
                            t("Later", "稍后")
                        }
                        .into(),
                        // Connect is the next step until an agent loads the tools.
                        (ready && (self.agent.is_some() || !self.registered.is_empty()))
                            .then_some(theme.accent),
                        &theme,
                        move |cx| tell(&shell, cx, |shell, cx| shell.close_setup(cx)),
                    )),
            );
        let close = self.shell.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                match e.keystroke.key.as_str() {
                    "tab" if e.keystroke.modifiers.shift => window.focus_prev(cx),
                    "tab" => window.focus_next(cx),
                    "escape" => tell(&close, cx, |shell, cx| shell.close_setup(cx)),
                    _ => return,
                }
                cx.stop_propagation();
            })
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
    item_with(lead, title, detail, note, None, action, theme)
}

/// A row with `extra` under its text, such as links.
fn item_with(
    lead: impl IntoElement,
    title: SharedString,
    detail: SharedString,
    note: Option<(String, Hsla)>,
    extra: Option<AnyElement>,
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
                })
                .children(extra),
        )
        .child(action)
}

/// A text button under a row's detail.
fn link(
    id: &'static str,
    label: &'static str,
    theme: &Theme,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.accent)
        .hover(|s| s.opacity(0.75))
        .active(|s| s.opacity(0.6))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .keyboard(action, theme)
        .child(label)
}

/// Agent Screen's server as an MCP client's JSON config takes it, with the
/// command and its arguments apart.
pub fn mcp_config() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    mcp_config_for(&exe.display().to_string())
}

fn mcp_config_for(exe: &str) -> String {
    let server = serde_json::json!({ "command": exe, "args": ["mcp"] });
    let body = serde_json::to_string_pretty(&server).unwrap_or_default();
    format!("\"agent-screen\": {body}")
}

/// docs/clients.md: each client's config.
pub fn open_client_guide() {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("https://github.com/Fanzzzd/agent-screen/blob/main/docs/clients.md")
        .spawn();
}

impl Setup {
    /// The bundle runs from a temporary place: what breaks, and Move to
    /// Applications, which copies it there and opens the copy.
    fn location_banner(&self, why: Temporary, theme: &Theme) -> impl IntoElement {
        let shell = self.shell.clone();
        let detail = match why {
            Temporary::DiskImage => t(
                "It runs from the disk image. Permissions and agent connections set up here stop working once it moves.",
                "当前从磁盘映像运行。在此授予的权限和 agent 接入在移动后会失效。",
            ),
            Temporary::Downloads => t(
                "It runs from Downloads. Permissions and agent connections set up here stop working once it moves.",
                "当前从“下载”运行。在此授予的权限和 agent 接入在移动后会失效。",
            ),
            Temporary::Translocated => t(
                "macOS runs it from a temporary copy. Permissions and agent connections set up here stop working.",
                "macOS 正从临时副本运行它。在此授予的权限和 agent 接入会失效。",
            ),
        };
        let note = match &self.moving {
            MoveStep::AskReplace => Some((
                t(
                    "Applications already has Agent Screen. Replace it?",
                    "“应用程序”中已有 Agent Screen，是否替换？",
                )
                .to_string(),
                theme.label,
            )),
            MoveStep::Failed(error) => Some((
                tf!(
                    "Didn't work: {error}. Drag it to Applications in Finder instead.",
                    "未成功：{error}。请在访达中将它拖到“应用程序”。"
                ),
                theme.orange,
            )),
            _ => None,
        };
        let action = match &self.moving {
            MoveStep::Copying => busy(t("Copying…", "正在拷贝…"), theme),
            MoveStep::AskReplace => {
                let cancel = shell.clone();
                div()
                    .flex()
                    .gap(px(8.))
                    .child(key_button(
                        "setup-move-cancel",
                        t("Cancel", "取消").into(),
                        None,
                        theme,
                        move |cx| tell(&cancel, cx, |shell, cx| shell.cancel_move(cx)),
                    ))
                    .child(key_button(
                        "setup-move-replace",
                        t("Replace", "替换").into(),
                        Some(theme.orange),
                        theme,
                        move |cx| {
                            tell(&shell, cx, |shell, cx| shell.move_to_applications(true, cx))
                        },
                    ))
                    .into_any_element()
            }
            step => key_button(
                "setup-move",
                if matches!(step, MoveStep::Failed(_)) {
                    t("Try Again", "重试")
                } else {
                    t("Move to Applications…", "移至应用程序…")
                }
                .into(),
                Some(theme.orange),
                theme,
                move |cx| {
                    tell(&shell, cx, |shell, cx| {
                        shell.move_to_applications(false, cx)
                    })
                },
            )
            .into_any_element(),
        };
        div()
            .mb(px(18.))
            .rounded(px(12.))
            .bg(theme.orange.opacity(0.12))
            .border_1()
            .border_color(theme.orange.opacity(0.35))
            .child(item(
                badge("icons/alert.svg", theme.orange, false),
                t(
                    "Move Agent Screen to Applications first",
                    "请先将 Agent Screen 移至“应用程序”",
                )
                .into(),
                detail.into(),
                note,
                action,
                theme,
            ))
    }

    /// Notifications: optional, so setup is complete without them.
    fn notifications_row(&self, theme: &Theme) -> impl IntoElement {
        let shell = self.shell.clone();
        let on = self.notifications == notify::Auth::Allowed;
        let action = match self.notifications {
            notify::Auth::Allowed => done(t("On", "已开启")),
            notify::Auth::Unknown => div().into_any_element(),
            auth => key_button(
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
                "已在系统设置中关闭。请开启 Agent Screen 的“允许通知”。",
            )
        } else {
            t(
                "When an agent needs your help or is done",
                "Agent 需要协助或结束使用显示器时提醒你",
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
            busy(t("Waiting for your password…", "等待输入密码…"), theme)
        } else {
            key_button(
                "setup-colorsync",
                if self.colorsync == Some(Helper::Outdated) {
                    t("Update…", "更新…")
                } else {
                    t("Install…", "安装…")
                }
                .into(),
                None,
                theme,
                move |cx| tell(&shell, cx, |shell, cx| shell.set_up_colorsync(cx)),
            )
            .into_any_element()
        };
        let detail = match &self.colorsync_error {
            Some(error) => tf!("Didn't work: {error}", "未成功：{error}"),
            None if on => t(
                "Keeps many displays from making your Mac stutter",
                "多块显示器时保持系统流畅",
            )
            .to_string(),
            None => t(
                "Prevents stutter. Needs admin password once",
                "防止卡顿。需输入一次管理员密码",
            )
            .to_string(),
        };
        let color = if on { green() } else { theme.accent };
        item(
            badge("icons/display.svg", color, on),
            t("Smooth Display Changes", "流畅切换显示器").into(),
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
                "Your picture. Not in agents' screenshots",
                "你选的图片，agent 截图中不含",
            )
        } else {
            t(
                "Names each display. Not in screenshots",
                "标出每块显示器，截图中不含",
            )
        };
        let choose = key_button(
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
                d.child(key_button(
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
                key_button(
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
                key_button(
                    "setup-aerospace",
                    t("Set Up…", "配置…").into(),
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
                "忽略 Agent Screen 的显示器",
            )
        } else {
            t(
                "Otherwise it puts your workspaces on agents' displays",
                "否则会把你的工作区放到 agent 的显示器上",
            )
        };
        let note = (!self.pinned.is_empty()).then(|| {
            (
                tf!(
                    "Workspace {} is pinned to a monitor number an agent uses",
                    "工作区 {} 固定的显示器编号已被 agent 占用",
                    self.pinned.join(", ")
                ),
                theme.orange,
            )
        });
        item(
            badge("icons/tiles.svg", color, ignoring && self.pinned.is_empty()),
            t(
                "Keep AeroSpace Off Agent Displays",
                "让 AeroSpace 忽略 Agent 显示器",
            )
            .into(),
            detail.into(),
            note,
            action,
            theme,
        )
    }

    /// "Displays have separate Spaces" off, while it is: a desktop switch
    /// takes agents' windows off their displays. Open Settings… opens
    /// Desktop & Dock; Agent Screen never changes the setting.
    fn spaces_row(&self, theme: &Theme) -> impl IntoElement {
        let shell = self.shell.clone();
        let action = key_button(
            "setup-spaces",
            t("Open Settings…", "打开设置…").into(),
            None,
            theme,
            move |cx| tell(&shell, cx, |shell, cx| shell.open_spaces_settings(cx)),
        )
        .into_any_element();
        item(
            badge("icons/display.svg", theme.orange, false),
            t("Displays Have Separate Spaces", "显示器具有单独的空间").into(),
            t(
                "Off: switching desktops moves agents' windows away",
                "已关闭：切换桌面会移走 agent 的窗口",
            )
            .into(),
            Some((
                t(
                    "Turn it on in Desktop & Dock > Mission Control, then log out",
                    "请在“桌面与程序坞 > 调度中心”中打开，然后注销",
                )
                .to_string(),
                theme.orange,
            )),
            action,
            theme,
        )
    }

    /// Connecting agents, the step after the permissions: Connect sets up
    /// the clients the registry finds, and any other takes the command.
    fn agent_row(&self, theme: &Theme, this: WeakEntity<Setup>) -> impl IntoElement {
        let shell = self.shell.clone();
        let loaded = self.agent.is_some();
        let action = if loaded {
            done(t("Connected", "已接入"))
        } else if self.connecting {
            busy(t("Connecting…", "正在接入…"), theme)
        } else {
            let again = !self.registered.is_empty() || self.earlier.is_some();
            key_button(
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
        let detail = match (&self.agent, &self.earlier, self.registered.as_slice()) {
            (Some(agent), _, _) => tf!("{agent} loaded the tools", "{agent} 已加载工具"),
            (None, Some((client, day)), _) => tf!(
                "{client} last loaded the tools on {day}",
                "{client} 上次于 {day}加载工具"
            ),
            (None, None, []) => t(
                "Connect sets up the agent clients on this Mac",
                "“接入”会配置这台 Mac 上的 agent 客户端",
            )
            .to_string(),
            (None, None, clients) => tf!(
                "Added to {}. Start a new session to load the tools",
                "已添加到 {}。新建会话即可加载工具",
                clients.join(t(" and ", " 和 "))
            ),
        };
        let color = if loaded { green() } else { theme.accent };
        let copied = self.copied;
        let others = div()
            .flex()
            .gap(px(12.))
            .mt(px(2.))
            .child(link(
                "setup-copy-command",
                if copied {
                    t("Copied", "已复制")
                } else {
                    t("Copy MCP Config", "复制 MCP 配置")
                },
                theme,
                move |cx| {
                    native::copy(&mcp_config());
                    let _ = this.update(cx, |setup, cx| {
                        setup.copied = true;
                        cx.notify();
                    });
                },
            ))
            .child(link(
                "setup-client-guide",
                t("Client Guide", "客户端说明"),
                theme,
                |_| open_client_guide(),
            ));
        item_with(
            badge("icons/plus.svg", color, loaded),
            t("MCP Clients", "MCP 客户端").into(),
            detail.into(),
            None,
            Some(others.into_any_element()),
            action,
            theme,
        )
    }

    /// Agents' bug reports: on, each waiting on the card for the user, or
    /// off.
    fn reports_row(&self, theme: &Theme) -> impl IntoElement {
        let on = self.reports_on;
        let detail = if on {
            t(
                "Agents' reports wait on a card for your review",
                "Agent 的报告先显示在卡片上供你审阅",
            )
        } else {
            t(
                "Off. Agents only show you their report",
                "已关闭。Agent 仅向你展示报告",
            )
        };
        let action = key_button(
            "setup-reports",
            if on {
                t("Turn Off", "关闭")
            } else {
                t("Turn On", "开启")
            }
            .into(),
            None,
            theme,
            |_| native::send(Native::Menu(MenuAction::ToggleReports)),
        )
        .into_any_element();
        item(
            badge(
                "icons/alert.svg",
                if on { green() } else { theme.secondary },
                on,
            ),
            t("Bug Reports", "问题报告").into(),
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
                    "显示 Agent 显示器的画面并截图",
                ),
            ),
            Permission::Accessibility => (
                "icons/pointer.svg",
                t(
                    "Click, type and place windows for agents",
                    "代 agent 点击、输入和摆放窗口",
                ),
            ),
        };
        let shell = self.shell.clone();
        let restart_here = p == Permission::ScreenRecording && granted && self.restart;
        let waiting = self.waiting == Some(p) && !granted;
        let action = if restart_here {
            key_button(
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
            key_button(
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
                    "Takes effect once Agent Screen reopens",
                    "重新打开 Agent Screen 后生效",
                )
                .to_string(),
                theme.orange,
            ))
        } else if waiting && self.still_off {
            Some((
                t(
                    "Already on? Remove with −, then re-add",
                    "已开启仍无效？点 − 移除后重新添加",
                )
                .to_string(),
                theme.orange,
            ))
        } else if waiting {
            Some((
                t(
                    "Turn on Agent Screen in System Settings. This row updates by itself.",
                    "在系统设置中打开 Agent Screen 的开关，此处会自动更新",
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
    button_base(id, title.clone(), prominent, theme)
        .on_click(move |_, _, cx| action(cx))
        .child(title)
}

/// A Settings button: as `button`, and the keyboard reaches it.
fn key_button(
    id: &'static str,
    title: SharedString,
    prominent: Option<Hsla>,
    theme: &Theme,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    button_base(id, title.clone(), prominent, theme)
        .keyboard(action, theme)
        .child(title)
}

fn button_base(
    id: &'static str,
    title: SharedString,
    prominent: Option<Hsla>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (bg, hover, fg) = match prominent {
        Some(color) => (color.opacity(0.9), color, gpui::white()),
        None => (theme.control, theme.control_hover, theme.label),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(title)
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
}

/// A button the keyboard reaches: Tab stops on it, Return or Space
/// presses it, and a ring shows it has the keyboard.
trait Keyboard: StatefulInteractiveElement + Styled + Sized {
    fn keyboard(self, action: impl Fn(&mut App) + 'static, theme: &Theme) -> Self {
        let action = std::rc::Rc::new(action);
        let press = action.clone();
        let ring = theme.accent;
        self.tab_index(0)
            // A ring outside the button, so the layout doesn't move.
            .focus_visible(move |s| {
                s.shadow(vec![gpui::BoxShadow {
                    color: ring,
                    offset: gpui::point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .on_click(move |_, _, cx| action(cx))
            .on_key_down(move |e: &KeyDownEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space")
                    && !e.keystroke.modifiers.modified()
                {
                    press(cx);
                    cx.stop_propagation();
                }
            })
    }
}

impl Keyboard for gpui::Stateful<gpui::Div> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mcp_config_keeps_command_and_args_apart() {
        let config = mcp_config_for("/Applications/Agent Screen.app/Contents/MacOS/AgentScreen");
        let value: serde_json::Value =
            serde_json::from_str(&format!("{{{config}}}")).expect("valid JSON in braces");
        let server = &value["agent-screen"];
        assert_eq!(
            server["command"],
            "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen"
        );
        assert_eq!(server["args"], serde_json::json!(["mcp"]));
    }
}
