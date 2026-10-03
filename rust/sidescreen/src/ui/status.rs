//! What the user reads about a display's state. Peek and the Viewer show the
//! same words.

use super::theme::Theme;
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
        let windows = format!("{window_count} 个窗口");
        match display.ended {
            Some(EndReason::Disconnected) => Status {
                kind: Kind::Disconnected,
                short: format!("连接中断 · {windows}"),
                long: format!("{agent} 的连接中断了，窗口还在这里"),
                icon: Some("icons/alert.svg"),
                color: theme.secondary,
            },
            Some(EndReason::Finished) => Status {
                kind: Kind::Review,
                short: format!("待查看 · {windows}"),
                long: format!("{agent} 已停止操作，窗口等你查看"),
                icon: Some("icons/tray.svg"),
                color: theme.secondary,
            },
            None if display.takeover => Status {
                kind: Kind::Takeover,
                short: "你在操作".into(),
                long: format!("你在操作，{agent} 在等你交还"),
                icon: None,
                color: theme.accent,
            },
            None => match (&display.help_request, &display.owner) {
                (Some(reason), _) => Status {
                    kind: Kind::Help,
                    short: format!("需要帮助：{reason}"),
                    long: format!("需要你帮助：{reason}"),
                    icon: Some("icons/hand.svg"),
                    color: theme.orange,
                },
                (None, None) => Status {
                    kind: Kind::Manual,
                    short: "你新建的".into(),
                    long: "你新建的显示器，没有 agent 在用".into(),
                    icon: None,
                    color: theme.secondary,
                },
                (None, Some(_)) => Status {
                    kind: Kind::Agent,
                    short: format!("由 {agent} 控制"),
                    long: format!("由 {agent} 控制"),
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
