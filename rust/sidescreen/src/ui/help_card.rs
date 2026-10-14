//! The Help Card: drops from the menu bar icon while a Help Request is
//! open, with the display's picture, the agent's reason, and Take Over or
//! Later.

use super::pointer;
use super::shell::{Shell, tell};
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::i18n::t;
use crate::model::DisplayId;
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement,
    Render, Role, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
    prelude::FluentBuilder, px, surface, svg,
};

pub const WIDTH: f32 = 320.;
const PADDING: f32 = 14.;
const GAP: f32 = 8.;
const TITLE: f32 = 20.;
/// Two lines of the reason; the tooltip has all of it.
const REASON: f32 = 36.;
const LABEL: f32 = 16.;
const BUTTONS: f32 = 30.;
const MAX_PICTURE: f32 = 184.;
const RADIUS: f32 = 18.;

/// The card's height for a display of `size`: everything but the picture is
/// fixed, so the panel can be placed before it paints.
pub fn height(size: (f64, f64)) -> f32 {
    PADDING * 2. + TITLE + REASON + LABEL + BUTTONS + GAP * 4. + picture(size).1
}

/// The picture's size inside the card, aspect-fit.
fn picture(size: (f64, f64)) -> (f32, f32) {
    let width = WIDTH - PADDING * 2.;
    let fit = (width / size.0 as f32).min(MAX_PICTURE / size.1 as f32);
    (size.0 as f32 * fit, size.1 as f32 * fit)
}

pub struct HelpCard {
    shell: WeakEntity<Shell>,
    core: Entity<Core>,
    streams: Entity<Streams>,
    /// The display whose Help Request the card shows.
    pub display: Option<DisplayId>,
}

impl HelpCard {
    pub fn new(
        shell: WeakEntity<Shell>,
        core: Entity<Core>,
        streams: Entity<Streams>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&core, |_, _, cx| cx.notify()).detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(&streams, |card: &mut HelpCard, _, frame: &NewFrame, cx| {
            if card.display == Some(frame.0) {
                cx.notify();
            }
        })
        .detach();
        HelpCard {
            shell,
            core,
            streams,
            display: None,
        }
    }
}

impl Render for HelpCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let core = self.core.read(cx);
        let Some(display) = self.display.and_then(|id| core.displays.get(id)) else {
            return div().into_any_element();
        };
        let id = display.id;
        let agent = display.agent_name().to_string();
        let reason = display.help_request.clone().unwrap_or_default();
        let label = display.label();
        let size = (display.width, display.height);
        let fit = picture(size);
        let frame = self.streams.read(cx).frame(id);
        let picture = match frame {
            Some(frame) => surface(frame)
                .object_fit(ObjectFit::Contain)
                .size_full()
                .into_any_element(),
            None => super::peek::waiting(id, 10.5).into_any_element(),
        };
        let (open, take_over, later) = (self.shell.clone(), self.shell.clone(), self.shell.clone());
        let title = tf!("{agent} needs your help", "{agent} 需要你帮忙");
        div()
            .id("help-card")
            .role(Role::Dialog)
            .aria_label(SharedString::from(title.clone()))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .p(px(PADDING))
            .bg(theme.card)
            .border_1()
            .border_color(theme.card_border)
            .rounded(px(RADIUS))
            .text_color(theme.label)
            .child(
                div()
                    .h(px(TITLE))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        svg()
                            .path("icons/hand.svg")
                            .flex_none()
                            .size(px(15.))
                            .text_color(theme.orange),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .id("help-reason")
                    .h(px(REASON))
                    .text_size(px(12.5))
                    .line_height(px(18.))
                    .line_clamp(2)
                    .tooltip({
                        let reason = reason.clone();
                        move |_, cx| super::shell::hint(reason.clone(), cx)
                    })
                    .tooltip_show_delay(super::shell::HINT_DELAY)
                    .child(reason),
            )
            .child(
                div()
                    .id("help-picture")
                    .role(Role::Button)
                    .aria_label(t("Open in Viewer", "在 Viewer 中打开"))
                    .tooltip(|_, cx| {
                        super::shell::hint(t("Open in Viewer", "在 Viewer 中打开"), cx)
                    })
                    .tooltip_show_delay(super::shell::HINT_DELAY)
                    .flex()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        tell(&open, cx, move |shell, cx| shell.card_open(id, cx))
                    })
                    .child(
                        div()
                            .relative()
                            .w(px(fit.0))
                            .h(px(fit.1))
                            .rounded(px(9.))
                            .overflow_hidden()
                            .bg(theme.screen_empty)
                            .child(picture)
                            .child(theme::round_corners(9., theme.card))
                            .child(pointer::layer(id, display.pointer, size, theme.accent))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(9.))
                                    .hover(|s| s.border(px(2.)).border_color(theme.orange)),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(LABEL))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(theme.secondary)
                    .child(label),
            )
            .child(
                div()
                    .h(px(BUTTONS))
                    .flex()
                    .gap(px(8.))
                    .child(
                        card_button("help-take-over", t("Take Over", "接手操作"), true, &theme)
                            .flex_1()
                            .on_click(move |_, _, cx| {
                                tell(&take_over, cx, move |shell, cx| shell.open_for_help(id, cx))
                            }),
                    )
                    .child(
                        card_button("help-later", t("Later", "稍后"), false, &theme)
                            .px(px(16.))
                            .on_click(move |_, _, cx| {
                                tell(&later, cx, move |shell, cx| shell.card_later(id, cx))
                            }),
                    ),
            )
            .into_any_element()
    }
}

fn card_button(
    id: &'static str,
    title: &'static str,
    prominent: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (bg, hover, fg) = if prominent {
        (theme.orange.opacity(0.9), theme.orange, gpui::white())
    } else {
        (theme.control, theme.control_hover, theme.label)
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(title)
        .h(px(BUTTONS))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.7))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(fg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(prominent, |d| {
            d.child(svg().path("icons/hand.svg").size(px(13.)).text_color(fg))
        })
        .child(title)
}
