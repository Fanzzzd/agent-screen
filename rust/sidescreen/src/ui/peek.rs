//! Peek: the strip of live thumbnails that drops out of the menu bar icon.

use super::pointer;
use super::shell::{Shell, tell};
use super::status::Status;
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::model::DisplayId;
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement,
    Render, Role, StatefulInteractiveElement, Styled, WeakEntity, Window, div,
    prelude::FluentBuilder, px, surface, svg,
};
use std::collections::HashMap;

pub const TILE_WIDTH: f32 = 208.;
pub const THUMB_HEIGHT: f32 = 130.;
pub const LABEL_HEIGHT: f32 = 38.;
pub const PADDING: f32 = 10.;
pub const SPACING: f32 = 10.;
/// With displays around, adding one is a narrow "+" at the end.
pub const NARROW: f32 = 44.;
pub const RADIUS: f32 = 18.;

pub struct Peek {
    shell: WeakEntity<Shell>,
    core: Entity<Core>,
    streams: Entity<Streams>,
    pub shown: bool,
    /// The display open in the Viewer, ringed.
    pub active: Option<DisplayId>,
    /// Windows on each display, refreshed by the shell.
    pub counts: HashMap<DisplayId, usize>,
}

impl Peek {
    pub fn new(
        shell: WeakEntity<Shell>,
        core: Entity<Core>,
        streams: Entity<Streams>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&core, |_, _, cx| cx.notify()).detach();
        // Light and dark switch while the panel is up.
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(&streams, |peek: &mut Peek, _, _: &NewFrame, cx| {
            if peek.shown {
                cx.notify();
            }
        })
        .detach();
        Peek {
            shell,
            core,
            streams,
            shown: false,
            active: None,
            counts: HashMap::new(),
        }
    }

    /// Card size for `displays` tiles plus the "+".
    pub fn size(displays: usize) -> (f64, f64) {
        let add = if displays == 0 { TILE_WIDTH } else { NARROW };
        let width = PADDING * 2. + displays as f32 * (TILE_WIDTH + SPACING) + add;
        let height = PADDING * 2. + THUMB_HEIGHT + LABEL_HEIGHT;
        (width as f64, height as f64)
    }
}

/// Displays in use first, then the ones waiting for review.
pub fn ordered(core: &Core) -> Vec<DisplayId> {
    let live = core.displays.list.iter().filter(|d| !d.is_ended());
    let ended = core.displays.list.iter().filter(|d| d.is_ended());
    live.chain(ended).map(|d| d.id).collect()
}

impl Render for Peek {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let core = self.core.read(cx);
        let streams = self.streams.read(cx);
        let ids = ordered(core);
        let mut tiles = Vec::new();
        for id in &ids {
            let Some(display) = core.displays.get(*id) else {
                continue;
            };
            let count = self.counts.get(id).copied().unwrap_or(0);
            let status = Status::of(display, count, &theme);
            let pointer = (!display.takeover && !display.is_ended())
                .then_some(display.pointer)
                .flatten();
            tiles.push(tile(
                *id,
                display.label(),
                status,
                streams.frame(*id),
                pointer,
                (display.width, display.height),
                self.active == Some(*id),
                &theme,
                &self.shell,
            ));
        }
        let narrow = !ids.is_empty();
        let shell = self.shell.clone();
        div()
            .id("peek")
            .role(Role::Group)
            .aria_label("Sidescreen")
            .size_full()
            .flex()
            .gap(px(SPACING))
            .p(px(PADDING))
            .bg(theme.card)
            .border_1()
            .border_color(theme.card_border)
            .rounded(px(RADIUS))
            .on_hover(move |hovered, _, cx| {
                let hovered = *hovered;
                tell(&shell, cx, move |shell, cx| shell.card_hover(hovered, cx));
            })
            .children(tiles)
            .child(add_tile(narrow, &theme, &self.shell))
    }
}

#[allow(clippy::too_many_arguments)]
fn tile(
    id: DisplayId,
    label: String,
    status: Status,
    frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    pointer: Option<crate::model::AgentPointer>,
    size: (f64, f64),
    active: bool,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> impl IntoElement {
    let group = format!("tile-{id}");
    let picture = match frame {
        Some(frame) => surface(frame)
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        None => div().size_full().into_any_element(),
    };
    let select = shell.clone();
    let dismiss = shell.clone();
    let ring = if active {
        theme.accent
    } else {
        gpui::transparent_black()
    };
    div()
        .id(gpui::ElementId::Integer(id as u64))
        .group(group.clone())
        .role(Role::Button)
        .aria_label(format!("{label}，{}", status.long))
        .tooltip({
            let text = format!("{label}\n{}", status.long);
            move |_, cx| super::shell::hint(text.clone(), cx)
        })
        .w(px(TILE_WIDTH))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(move |_, _, cx| tell(&select, cx, move |shell, cx| shell.select(id, cx)))
        .child(
            div()
                .relative()
                .w_full()
                .h(px(THUMB_HEIGHT))
                .rounded(px(9.))
                .overflow_hidden()
                .bg(theme.screen_empty)
                .child(picture)
                .child(theme::round_corners(9., theme.card))
                .child(pointer::layer(id, pointer, size, theme.accent))
                // Hover and "currently open" both show as a ring.
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(9.))
                        .border(px(if active { 2. } else { 0. }))
                        .border_color(ring)
                        .when(!active, |ring| {
                            ring.group_hover(group.clone(), |s| {
                                s.border(px(1.5)).border_color(theme.label.opacity(0.55))
                            })
                        }),
                )
                .child(
                    div()
                        .id(gpui::ElementId::Name(format!("close-{id}").into()))
                        .role(Role::Button)
                        .aria_label("关闭显示器")
                        .tooltip(|_, cx| super::shell::hint("关闭显示器", cx))
                        .tooltip_show_delay(super::shell::HINT_DELAY)
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .size(px(20.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.55))
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            tell(&dismiss, cx, move |shell, cx| shell.dismiss(id, cx));
                        })
                        .child(
                            svg()
                                .path("icons/close.svg")
                                .size(px(11.))
                                .text_color(gpui::white()),
                        ),
                ),
        )
        .child(
            div()
                .h(px(LABEL_HEIGHT))
                .flex()
                .flex_col()
                .justify_center()
                .items_center()
                .gap(px(1.))
                .child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.secondary)
                        .group_hover(group, |s| s.text_color(theme.label))
                        .child(label),
                )
                .child(
                    div()
                        .max_w_full()
                        .flex()
                        .justify_center()
                        .child(status.line(&status.short, 10.5)),
                ),
        )
}

fn add_tile(narrow: bool, theme: &Theme, shell: &WeakEntity<Shell>) -> impl IntoElement {
    let shell = shell.clone();
    let width = if narrow { NARROW } else { TILE_WIDTH };
    div()
        .id("add")
        .group("add")
        .role(Role::Button)
        .aria_label("新建显示器")
        .when(narrow, |d| {
            d.tooltip(|_, cx| super::shell::hint("新建显示器", cx))
                .tooltip_show_delay(super::shell::HINT_DELAY)
        })
        .w(px(width))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(move |_, _, cx| tell(&shell, cx, |shell, cx| shell.create_display(cx)))
        .child(
            div()
                .w_full()
                .h(px(THUMB_HEIGHT))
                .rounded(px(9.))
                .bg(theme.label.opacity(0.06))
                .border(px(0.))
                .group_hover("add", |s| {
                    s.border(px(1.5)).border_color(theme.label.opacity(0.55))
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("icons/plus.svg")
                        .size(px(20.))
                        .text_color(theme.secondary),
                ),
        )
        .child(
            div()
                .h(px(LABEL_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.secondary)
                .group_hover("add", |s| s.text_color(theme.label))
                .when(!narrow, |d| d.child("新建显示器")),
        )
}
