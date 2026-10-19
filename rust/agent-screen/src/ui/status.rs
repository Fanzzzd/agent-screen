//! What the user reads about a display's state. Peek and the Viewer show the
//! same words.

use super::theme::Theme;
use crate::i18n::{Lang, t};
use crate::model::{Display, EndReason};
use gpui::{Hsla, IntoElement, ParentElement, Styled, div, px, svg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Manual,
    Agent,
    Help,
    Takeover,
    Review,
    Disconnected,
}

pub struct Status {
    pub kind: Kind,
    /// Short form for Peek: "需要帮助：输入验证码".
    pub short: String,
    /// Long form for the Viewer header.
    pub long: String,
    pub icon: Option<&'static str>,
    pub color: Hsla,
}

impl Status {
    pub fn of<H>(display: &Display<H>, window_count: usize, theme: &Theme) -> Status {
        let agent = display.agent_name();
        let windows = match (crate::i18n::lang(), window_count) {
            (Lang::En, 1) => "1 window".to_string(),
            _ => tf!("{window_count} windows", "{window_count} 个窗口"),
        };
        if display.keyboard_borrowed && !display.is_ended() {
            return Status {
                kind: Kind::Agent,
                short: t("Using your keyboard", "正在用你的键盘").into(),
                long: tf!(
                    "{agent} is typing through your input method. Your keys go to its app until it’s done.",
                    "{agent} 正在通过你的输入法打字，完成前你的按键会进入它的应用"
                ),
                icon: Some("icons/alert.svg"),
                color: theme.orange,
            };
        }
        match display.ended {
            Some(EndReason::Disconnected) => Status {
                kind: Kind::Disconnected,
                short: tf!("Disconnected · {windows}", "连接中断 · {windows}"),
                long: tf!(
                    "{agent} disconnected. Its windows are still here.",
                    "{agent} 的连接中断了，窗口还在这里"
                ),
                icon: Some("icons/alert.svg"),
                color: theme.secondary,
            },
            Some(EndReason::Finished | EndReason::ClosedByUser) => Status {
                kind: Kind::Review,
                short: tf!("Needs Review · {windows}", "待查看 · {windows}"),
                long: tf!(
                    "{agent} stopped. Its windows wait for your review.",
                    "{agent} 已停止操作，窗口等你查看"
                ),
                icon: Some("icons/tray.svg"),
                color: theme.secondary,
            },
            None if display.takeover => Status {
                kind: Kind::Takeover,
                short: t("You’re in control", "你在操作").into(),
                // The reason stays up while the user answers it.
                long: match &display.help_request {
                    Some(reason) => tf!(
                        "You’re in control. {agent} asked: {reason}",
                        "你在操作。{agent} 请你：{reason}"
                    ),
                    None => tf!(
                        "You’re in control. {agent} is waiting to continue.",
                        "你在操作，{agent} 在等你交还"
                    ),
                },
                icon: None,
                color: theme.accent,
            },
            None => match (&display.help_request, &display.owner) {
                (Some(reason), _) => Status {
                    kind: Kind::Help,
                    short: tf!("Needs help: {reason}", "需要帮助：{reason}"),
                    long: tf!("Needs your help: {reason}", "需要你帮助：{reason}"),
                    icon: Some("icons/hand.svg"),
                    color: theme.orange,
                },
                (None, None) => Status {
                    kind: Kind::Manual,
                    short: t("Created by you", "你新建的").into(),
                    long: t(
                        "Created by you. No agent is using it.",
                        "你新建的显示器，没有 agent 在用",
                    )
                    .into(),
                    icon: None,
                    color: theme.secondary,
                },
                (None, Some(_)) => Status {
                    kind: Kind::Agent,
                    short: tf!("Controlled by {agent}", "由 {agent} 控制"),
                    long: tf!("Controlled by {agent}", "由 {agent} 控制"),
                    icon: None,
                    color: theme.secondary,
                },
            },
        }
    }

    /// The status line: its icon, then the text, truncated at the end.
    pub fn line(&self, text: &str, size: f32) -> impl IntoElement + use<> {
        let icon = self.icon.map(|path| {
            svg()
                .path(path)
                .size(px(size))
                .flex_none()
                .text_color(self.color)
        });
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .min_w_0()
            .text_size(px(size))
            .text_color(self.color)
            .children(icon)
            .child(div().min_w_0().truncate().child(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DEFAULT_SIZE, Displays, Owner};

    fn display(owner: bool) -> Displays<()> {
        let mut d = Displays::default();
        let owner = owner.then(|| Owner {
            session_key: "s".into(),
            client: "Codex".into(),
            title: "订机票".into(),
            connection: 1,
        });
        d.add(7, "Agent Screen 1".into(), DEFAULT_SIZE, owner, ());
        d
    }

    fn status(d: &Displays<()>, windows: usize) -> Status {
        Status::of(
            d.get(7).unwrap(),
            windows,
            &super::super::theme::palette(true),
        )
    }

    #[test]
    fn copy_follows_the_display_state() {
        crate::i18n::set_for_test(Lang::ZhHans);
        let manual = display(false);
        assert_eq!(status(&manual, 0).short, "你新建的");

        let mut d = display(true);
        assert_eq!(status(&d, 0).long, "由 Codex 控制");

        d.request_help(7, "输入验证码".into(), Vec::new());
        let help = status(&d, 0);
        assert_eq!(
            (help.kind, help.short.as_str()),
            (Kind::Help, "需要帮助：输入验证码")
        );

        // A Takeover outranks the Help Request it answers, and keeps its reason.
        d.begin_takeover(7);
        assert_eq!(status(&d, 0).long, "你在操作。Codex 请你：输入验证码");
        d.withdraw_help(7);
        assert_eq!(status(&d, 0).long, "你在操作，Codex 在等你交还");

        d.end_takeover(7);
        d.end(7, EndReason::Finished);
        assert_eq!(status(&d, 2).short, "待查看 · 2 个窗口");

        let mut lost = display(true);
        lost.end(7, EndReason::Disconnected);
        assert_eq!(status(&lost, 1).long, "Codex 的连接中断了，窗口还在这里");
    }

    #[test]
    fn english_copy_counts_windows() {
        crate::i18n::set_for_test(Lang::En);
        let mut d = display(true);
        assert_eq!(status(&d, 0).long, "Controlled by Codex");
        d.end(7, EndReason::Finished);
        assert_eq!(status(&d, 1).short, "Needs Review · 1 window");
        assert_eq!(status(&d, 2).short, "Needs Review · 2 windows");
    }
}
