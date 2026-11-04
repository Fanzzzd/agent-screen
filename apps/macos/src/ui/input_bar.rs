//! The Input Bar: while an Input Grant lets agents use the pointer (ADR
//! 0013), a small bar at the bottom centre of the user's screen says so,
//! with how to take control back and how long the grant has lasted. It
//! stays clear of the queue card under the menu bar icon, never takes the
//! keyboard, and its Stop ends the grant as the card's does.

use super::help_card::{self, Tone};
use super::shell::{Shell, tell};
use super::theme;
use crate::core::Core;
use crate::i18n::t;
use std::cell::Cell;

use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Role,
    StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, px, svg,
};

thread_local! {
    /// The bar's size at its last paint, which its window takes. Before
    /// the first paint, a guess.
    static FIT: Cell<(f32, f32)> = const { Cell::new((520., 58.)) };
}

/// The size the bar's window should have.
pub fn size() -> (f32, f32) {
    FIT.with(Cell::get)
}

pub struct InputBar {
    shell: WeakEntity<Shell>,
    core: Entity<Core>,
}

impl InputBar {
    pub fn new(
        shell: WeakEntity<Shell>,
        core: Entity<Core>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&core, |_, _, cx| cx.notify()).detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        InputBar { shell, core }
    }
}

impl Render for InputBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let Some(grant) = help_card::grant_view(&self.core.read(cx).input_queue) else {
            return div().into_any_element();
        };
        let (state, time) = if grant.running {
            (
                t("Agent is using your mouse", "Agent 正在使用鼠标"),
                help_card::clock(grant.elapsed),
            )
        } else {
            (
                t("Approved, waiting for next action", "放行中，等待下一步"),
                format!(
                    "{} / {}",
                    help_card::clock(grant.idle.unwrap_or(0)),
                    help_card::clock(agent_screen_model::input_queue::IDLE_MS / 1000)
                ),
            )
        };
        let (shell, stop) = (self.shell.clone(), self.shell.clone());
        // The root fills the window; the bar keeps its own size, which
        // the window then takes. Stretched to the root's height, it would
        // follow the window down a point a paint.
        div()
            .flex()
            .items_start()
            .child(
                div()
                    .id("input-bar")
                    .role(Role::Status)
                    .relative()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .pl(px(12.))
                    .pr(px(10.))
                    .py(px(10.))
                    .bg(theme.card)
                    .border_1()
                    .border_color(theme.card_border)
                    .rounded(px(14.))
                    .text_color(theme.label)
                    .child(
                        div()
                            .flex_none()
                            .size(px(32.))
                            .rounded(px(9.))
                            .bg(theme.red.opacity(0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path("icons/pointer.svg")
                                    .size(px(15.))
                                    .text_color(theme.red),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(250.))
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(state),
                            )
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_size(px(11.))
                                    .text_color(theme.secondary)
                                    .child(t(
                                        "Move the mouse or press a key to take control",
                                        "移动鼠标或按键即可收回",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            // Wide enough for either clock, so the bar
                            // keeps its width when a drag starts.
                            .min_w(px(92.))
                            .flex()
                            .justify_center()
                            .whitespace_nowrap()
                            .px(px(10.))
                            .border_l_1()
                            .border_color(theme.card_border)
                            .text_size(px(11.))
                            .text_color(theme.secondary)
                            .font_features(help_card::tabular())
                            .child(time),
                    )
                    .child(
                        help_card::button(
                            "input-bar-stop",
                            t("Stop", "停止并收回"),
                            Tone::Stop,
                            &theme,
                        )
                        .flex_none()
                        .h(px(30.))
                        .px(px(12.))
                        .text_size(px(12.))
                        .on_click(move |_, _, cx| {
                            tell(&stop, cx, |shell, cx| shell.revoke_input(cx))
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                let fit = (
                                    f32::from(bounds.size.width).ceil(),
                                    f32::from(bounds.size.height).ceil(),
                                );
                                let old = FIT.with(Cell::get);
                                if (fit.0 - old.0).abs() > 0.5 || (fit.1 - old.1).abs() > 0.5 {
                                    FIT.with(|f| f.set(fit));
                                    tell(&shell, cx, |shell, cx| shell.fit_bar(cx));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    ),
            )
            .into_any_element()
    }
}
