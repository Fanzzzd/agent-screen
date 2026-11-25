//! The Spaces card: drops from the menu bar icon once, in the Help Card's
//! family, when Agent Screen first sees every display share one macOS
//! Space ("Displays have separate Spaces" off, `agent_screen_model::spaces`).
//! Then a desktop switch on the user's screen takes the agents' windows off
//! their displays too. It names the setting and where it is, opens Desktop
//! & Dock on the user's click, and never changes the setting. It waits
//! while the Help Card, the Bug Report Card or Peek shows.

use super::help_card::{
    self, BUTTONS, GAP, PADDING, RADIUS, REASON, TITLE, Tone, card_button, reason_text, title_row,
};
use super::shell::{Shell, tell};
use super::theme;
use crate::i18n::t;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

/// Three lines of what to change.
const NOTE: f32 = 48.;

/// The card's height: fixed, so the panel can be placed before it paints.
pub fn height() -> f32 {
    PADDING * 2. + TITLE + REASON + NOTE + BUTTONS + GAP * 3.
}

pub struct SpacesCard {
    shell: WeakEntity<Shell>,
}

impl SpacesCard {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self { shell }
    }
}

impl Render for SpacesCard {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let zh = help_card::zh();
        let (open, later) = (self.shell.clone(), self.shell.clone());
        let title = t("Your displays share one Space", "所有显示器共用一个空间");
        div()
            .id("spaces-card")
            .role(Role::Dialog)
            .aria_label(SharedString::from(title))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .p(px(PADDING - 1.))
            .bg(theme.card)
            .border_2()
            .border_color(theme.card_border)
            .rounded(px(RADIUS))
            .text_color(theme.label)
            .child(title_row("icons/alert.svg", theme.orange, title.into(), None))
            .child(
                reason_text(
                    t(
                        "“Displays have separate Spaces” is off: a desktop switch hides agents' windows too.",
                        "“显示器具有单独的空间”已关闭，切换桌面时 agent 的窗口也会离开其显示器。",
                    )
                    .into(),
                    zh,
                )
                .h(px(REASON)),
            )
            .child(
                div()
                    .h(px(NOTE))
                    .flex_none()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .line_clamp(3)
                    .text_ellipsis()
                    .text_color(theme.secondary)
                    .child(t(
                        "To keep them there, turn it on in System Settings > Desktop & Dock > Mission Control, then log out and back in. Agent Screen doesn't change it.",
                        "如需保持原位，请在“系统设置 > 桌面与程序坞 > 调度中心”中打开此项，然后注销并重新登录。Agent Screen 不会自行更改。",
                    )),
            )
            .child(
                div()
                    .h(px(BUTTONS))
                    .flex_none()
                    .flex()
                    .gap(px(8.))
                    .child(
                        card_button(
                            "spaces-open",
                            t("Open Desktop & Dock", "打开桌面与程序坞"),
                            Tone::Accent,
                            &theme,
                        )
                        .flex_1()
                        .on_click(move |_, _, cx| {
                            tell(&open, cx, |shell, cx| shell.open_spaces_settings(cx))
                        }),
                    )
                    .child(
                        card_button("spaces-later", t("Not Now", "暂不"), Tone::Plain, &theme)
                            .px(px(14.))
                            .on_click(move |_, _, cx| {
                                tell(&later, cx, |shell, cx| shell.dismiss_spaces_card(cx))
                            }),
                    ),
            )
    }
}
