//! The Input Bar: while an Input Grant lets agents use the pointer (ADR
//! 0013), a pill at the bottom centre of the user's screen says which
//! agent uses it, with how to take control back, or while the grant
//! idles, how long until it ends. It
//! stays clear of the queue card under the menu bar icon, never takes the
//! keyboard, and its Stop ends the grant as the card's does.

use super::help_card::{self, Tone};
use super::shell::{Shell, tell};
use super::theme;
use crate::core::Core;
use crate::i18n::t;
use std::cell::Cell;

use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, prelude::FluentBuilder,
    px,
};

thread_local! {
    /// The bar's size at its last paint, which its window takes. Before
    /// the first paint, a guess.
    static FIT: Cell<(f32, f32)> = const { Cell::new((420., HEIGHT)) };
}

/// The pill's height; its ends are round.
const HEIGHT: f32 = 40.;

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
        let core = self.core.read(cx);
        if core.input_queue.queue.grant().is_none() {
            return div().into_any_element();
        }
        let view = help_card::queue_view(core, None);
        let title = help_card::queue_title(&view);
        let (how, clock) = match view.phase {
            help_card::Phase::Waiting { idle } => (
                None,
                Some(format!(
                    "{} / {}",
                    help_card::clock(idle),
                    help_card::clock(agent_screen_model::input_queue::IDLE_MS / 1000)
                )),
            ),
            _ => (Some(t("Move the mouse to stop", "动鼠标即可收回")), None),
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
                    .aria_label(SharedString::from(title.clone()))
                    .relative()
                    .flex_none()
                    .h(px(HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(14.))
                    .pr(px(5.))
                    .bg(theme.card)
                    .border_1()
                    .border_color(theme.card_border)
                    .rounded(px(HEIGHT / 2.))
                    .text_size(px(12.))
                    .text_color(theme.label)
                    .whitespace_nowrap()
                    // A red dot in a soft ring of its color.
                    .child(
                        div()
                            .flex_none()
                            .size(px(13.))
                            .rounded_full()
                            .bg(theme.red.opacity(0.25))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(px(7.)).rounded_full().bg(theme.red)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(300.))
                            .truncate()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when_some(how, |d, how| {
                        d.child(div().flex_none().text_color(theme.secondary).child(how))
                    })
                    .when_some(clock, |d, clock| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(theme.secondary)
                                .font_features(help_card::tabular())
                                .child(clock),
                        )
                    })
                    .child(
                        help_card::button("input-bar-stop", t("Stop", "停止"), Tone::Stop, &theme)
                            .flex_none()
                            .h(px(30.))
                            .px(px(13.))
                            .on_click(move |_, _, cx| {
                                tell(&stop, cx, |shell, cx| shell.revoke_input(cx))
                            }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                // The canvas fills the bar inside its
                                // border, a point on each side.
                                let fit = (
                                    f32::from(bounds.size.width).ceil() + 2.,
                                    f32::from(bounds.size.height).ceil() + 2.,
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
