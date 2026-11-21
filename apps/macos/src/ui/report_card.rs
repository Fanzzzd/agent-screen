//! The Bug Report Card: drops from the menu bar icon, as the Help Card
//! does, when an agent's bug report waits (ADR 0024). Every report waits
//! here: it names the agent and the report, shows the whole report on Show
//! Report, says what Open on GitHub does, and offers Open on GitHub, Not
//! Now and Turn Off. Open on GitHub opens this report's form and no other.
//! The Help Card goes first: this card waits while that one shows.

use super::help_card::{
    self, BUTTONS, GAP, PADDING, RADIUS, REASON, TITLE, Tone, card_button, reason_text, title_row,
};
use super::shell::{Shell, tell};
use super::theme;
use crate::i18n::t;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, Role,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

/// Three lines of what the button does.
const NOTE: f32 = 48.;
/// The Show Report line.
const TOGGLE: f32 = 16.;
/// The report, when shown: it scrolls.
const BODY: f32 = 260.;

/// The card's height, folded or with the report shown: fixed, so the panel
/// can be placed before it paints.
pub fn height(expanded: bool) -> f32 {
    let folded = PADDING * 2. + TITLE + REASON + NOTE + TOGGLE + BUTTONS + GAP * 4.;
    if expanded {
        folded + BODY + GAP
    } else {
        folded
    }
}

/// What the card shows: the report waiting, who filed it, how many more
/// wait behind it, and whether the whole report shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub id: u64,
    pub agent: String,
    pub title: String,
    /// The report as it would leave the Mac.
    pub body: String,
    pub more: usize,
    pub expanded: bool,
    /// This build files through the relay, so Open on GitHub agrees to
    /// later reports going out without asking, and the card says so.
    pub relay: bool,
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
        let (open, later, off, more) = (
            self.shell.clone(),
            self.shell.clone(),
            self.shell.clone(),
            self.shell.clone(),
        );
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
        let note = if shown.relay {
            t(
                "Files this as a public GitHub issue, and later reports without asking. Agent Screen redacts known patterns, not every private word: review the report first.",
                "将此报告提交为 GitHub 公开问题，之后的报告不再询问。Agent Screen 按已知模式脱敏，无法识别所有隐私内容，请先检查报告。",
            )
        } else {
            t(
                "Opens a prefilled public issue on GitHub for you to submit. Agent Screen redacts known patterns, not every private word: review the report first.",
                "在 GitHub 打开预填的公开问题，由你提交。Agent Screen 按已知模式脱敏，无法识别所有隐私内容，请先检查报告。",
            )
        };
        let toggle = if shown.expanded {
            t("Hide Report", "收起报告")
        } else {
            t("Show Report", "查看报告")
        };
        let body = shown.expanded.then(|| {
            div()
                // Each report starts at its top.
                .id(SharedString::from(format!("report-body-{id}")))
                .h(px(BODY))
                .flex_none()
                .p(px(8.))
                .rounded(px(8.))
                .bg(theme.control)
                .overflow_y_scroll()
                .text_size(px(10.5))
                .line_height(px(14.))
                .text_color(theme.label)
                .child(shown.body.clone())
        });
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
            .child(title_row("icons/alert.svg", theme.accent, title, None))
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
                div().h(px(TOGGLE)).flex_none().flex().child(
                    div()
                        .id("report-more")
                        .role(Role::Button)
                        .aria_label(SharedString::from(toggle))
                        .text_size(px(11.))
                        .line_height(px(TOGGLE))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .child(toggle)
                        .on_click(move |_, _, cx| {
                            tell(&more, cx, |shell, cx| shell.toggle_report_details(cx))
                        }),
                ),
            )
            .children(body)
            .child(
                div()
                    .h(px(BUTTONS))
                    .flex_none()
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
                        card_button("report-later", t("Not Now", "暂不"), Tone::Plain, &theme)
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
