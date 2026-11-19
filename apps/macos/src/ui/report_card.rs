//! The Bug Report Card: drops from the menu bar icon, as the Help Card
//! does, when an agent's first bug report waits for the user's consent
//! (ADR 0024). It names the agent and the report, says what Open on GitHub
//! does and what was removed, and offers Open on GitHub, Not Now and Turn
//! Off. The Help Card goes first: this card waits while that one shows.

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

/// Three lines of what the button does.
const NOTE: f32 = 48.;

/// The card's height: fixed, so the panel can be placed before it paints.
pub const HEIGHT: f32 = PADDING * 2. + TITLE + REASON + NOTE + BUTTONS + GAP * 3.;

/// What the card shows: the report waiting, who filed it and how many more
/// wait behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub id: u64,
    pub agent: String,
    pub title: String,
    pub more: usize,
}

pub struct ReportCard {
    shell: WeakEntity<Shell>,
    pub shown: Option<Shown>,
}

impl ReportCard {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self { shell, shown: None }
    }
}

impl Render for ReportCard {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let Some(shown) = self.shown.clone() else {
            return div().into_any_element();
        };
        let zh = help_card::zh();
        let id = shown.id;
        let (open, later, off) = (self.shell.clone(), self.shell.clone(), self.shell.clone());
        let title = if shown.more > 0 {
            tf!(
                "Report {} Agent Screen bugs?",
                "报告 {} 个 Agent Screen 问题？",
                shown.more + 1
            )
        } else {
            t("Report an Agent Screen bug?", "报告 Agent Screen 的问题？").to_string()
        };
        let reason = format!("{}: {}", shown.agent, shown.title);
        let note = t(
            "Opens a prefilled public issue on GitHub for you to submit. Window titles, typed text, links and file paths are removed. Later reports open without asking.",
            "在 GitHub 打开预填的公开问题，由你提交。已去掉窗口标题、输入的文字、链接和文件路径。之后的报告直接打开，不再询问。",
        );
        div()
            .id("report-card")
            .role(Role::Dialog)
            .aria_label(SharedString::from(title.clone()))
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
            .child(title_row("icons/alert.svg", theme.accent, title, false))
            .child(reason_text(reason, zh).h(px(REASON)))
            .child(
                div()
                    .h(px(NOTE))
                    .flex_none()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .line_clamp(3)
                    .text_ellipsis()
                    .text_color(theme.secondary)
                    .child(note),
            )
            .child(
                div()
                    .h(px(BUTTONS))
                    .flex()
                    .gap(px(8.))
                    .child(
                        card_button(
                            "report-open",
                            t("Open on GitHub", "在 GitHub 打开"),
                            Tone::Accent,
                            &theme,
                        )
                        .flex_1()
                        .on_click(move |_, _, cx| {
                            tell(&open, cx, move |shell, cx| shell.allow_report(id, cx))
                        }),
                    )
                    .child(
                        card_button("report-later", t("Not Now", "稍后"), Tone::Plain, &theme)
                            .px(px(14.))
                            .on_click(move |_, _, cx| {
                                tell(&later, cx, move |shell, cx| shell.decline_report(id, cx))
                            }),
                    )
                    .child(
                        card_button("report-off", t("Turn Off", "关闭报告"), Tone::Plain, &theme)
                            .px(px(14.))
                            .on_click(move |_, _, cx| {
                                tell(&off, cx, |shell, cx| shell.set_reports_automatic(false, cx))
                            }),
                    ),
            )
            .into_any_element()
    }
}
