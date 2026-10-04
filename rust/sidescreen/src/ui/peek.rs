//! Peek: the strip of live thumbnails that drops out of the menu bar icon.

use super::pointer;
use super::shell::{Shell, tell};
use super::status::Status;
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::model::DisplayId;
use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ObjectFit, ParentElement, Render, Role, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, prelude::FluentBuilder, px, surface, svg,
};
use std::collections::HashMap;

pub const TILE_WIDTH: f32 = 208.;
pub const THUMB_HEIGHT: f32 = 130.;
/// The narrowest a tile gets when many displays share the card.
pub const MIN_TILE: f32 = 120.;

/// Thumbnail height for a tile width, at the 208×130 proportions.
pub fn thumb_height(tile: f32) -> f32 {
    tile * THUMB_HEIGHT / TILE_WIDTH
}
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
    /// Tile width: TILE_WIDTH unless that many tiles would not fit.
    pub tile: f32,
    /// A Peek opened by a click takes the keyboard.
    pub focus: FocusHandle,
    /// The tile the arrow keys reached; the + tile is the last index.
    pub focused: Option<usize>,
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
            tile: TILE_WIDTH,
            focus: cx.focus_handle(),
            focused: None,
        }
    }

    /// ←/→ move between tiles, Return opens one, 1-9 jump to a display,
    /// ⌘N adds one and Esc closes Peek.
    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let ids = ordered(self.core.read(cx));
        let count = ids.len() + 1;
        let current = self
            .focused
            .or_else(|| self.active.and_then(|a| ids.iter().position(|&id| id == a)));
        let keys = &event.keystroke;
        let shell = self.shell.clone();
        let open = move |index: usize, cx: &mut Context<Self>| match ids.get(index) {
            Some(&id) => tell(&shell, cx, move |shell, cx| shell.select(id, cx)),
            None => tell(&shell, cx, |shell, cx| shell.create_display(cx)),
        };
        let displays = count - 1;
        match keys.key.as_str() {
            "left" => self.focused = Some(current.map_or(0, |i| (i + count - 1) % count)),
            "right" => self.focused = Some(current.map_or(0, |i| (i + 1) % count)),
            "enter" => {
                if let Some(index) = current {
                    open(index, cx);
                }
            }
            "escape" => tell(&self.shell, cx, |shell, cx| shell.hide_peek(cx)),
            "n" if keys.modifiers.platform => open(displays, cx),
            digit if !keys.modifiers.modified() => match digit.parse::<usize>() {
                Ok(n) if (1..=9).contains(&n) && n <= displays => open(n - 1, cx),
                _ => return,
            },
            _ => return,
        }
        cx.notify();
    }

    /// Card size and tile width for `displays` tiles plus the "+", no
    /// wider than `room`: tiles shrink down to MIN_TILE to fit.
    pub fn size(displays: usize, room: f64) -> (f64, f64, f32) {
        let add = if displays == 0 { TILE_WIDTH } else { NARROW };
        let fixed = PADDING * 2. + add + displays as f32 * SPACING;
        let tile = if displays == 0 {
            TILE_WIDTH
        } else {
            ((room as f32 - fixed) / displays as f32).clamp(MIN_TILE, TILE_WIDTH)
        };
        let width = fixed + displays as f32 * tile;
        let height = PADDING * 2. + thumb_height(tile) + LABEL_HEIGHT;
        (width as f64, height as f64, tile)
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
        for (index, id) in ids.iter().enumerate() {
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
                self.focused == Some(index),
                self.tile,
                &theme,
                &self.shell,
            ));
        }
        let narrow = !ids.is_empty();
        let shell = self.shell.clone();
        let add_focused = self.focused == Some(ids.len());
        div()
            .id("peek")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|peek: &mut Peek, e: &KeyDownEvent, _, cx| peek.key(e, cx)))
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
            .child(add_tile(
                narrow,
                self.tile,
                add_focused,
                &theme,
                &self.shell,
            ))
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
    focused: bool,
    width: f32,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> impl IntoElement {
    let group = format!("tile-{id}");
    let picture = match frame {
        Some(frame) => surface(frame)
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        None => waiting(10.5).into_any_element(),
    };
    // The thumbnail takes the display's shape inside the tile's box, so a
    // portrait display gets round corners and a ring of its own size.
    let thumb = thumb_height(width);
    let fit = (width / size.0 as f32).min(thumb / size.1 as f32);
    let fit = (size.0 as f32 * fit, size.1 as f32 * fit);
    let select = shell.clone();
    let dismiss = shell.clone();
    // Open in the Viewer: a full ring. Reached with the arrow keys: a
    // fainter one.
    let ring = if active {
        theme.accent
    } else if focused {
        theme.accent.opacity(0.6)
    } else {
        gpui::transparent_black()
    };
    let ringed = active || focused;
    div()
        .id(gpui::ElementId::Integer(id as u64))
        .group(group.clone())
        .role(Role::Button)
        .aria_label(format!("{label}，{}", status.long))
        .tooltip({
            let text = format!("{label}\n{}", status.long);
            move |_, cx| super::shell::hint(text.clone(), cx)
        })
        .tooltip_show_delay(super::shell::TILE_HINT_DELAY)
        .w(px(width))
        .flex()
        .flex_col()
        .active(|s| s.opacity(0.7))
        .on_click(move |_, _, cx| tell(&select, cx, move |shell, cx| shell.select(id, cx)))
        .child(
            div()
                .w_full()
                .h(px(thumb))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .relative()
                        .w(px(fit.0))
                        .h(px(fit.1))
                        .rounded(px(9.))
                        .overflow_hidden()
                        .bg(theme.screen_empty)
                        .child(picture)
                        // The user's own menu bar, which macOS draws on
                        // every display, stays covered here as in the Viewer.
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(
                                    crate::mac::menubar_height(id) as f32 * fit.0 / size.0 as f32
                                ))
                                .bg(theme.menu_strip),
                        )
                        .child(theme::round_corners(9., theme.card))
                        .child(pointer::layer(id, pointer, size, theme.accent))
                        // Hover and "currently open" both show as a ring.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(px(9.))
                                .border(px(if ringed { 2. } else { 0. }))
                                .border_color(ring)
                                .when(!ringed, |ring| {
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
                                // A 17 pt disc in a 22 pt hit area.
                                .absolute()
                                .top(px(4.))
                                .right(px(4.))
                                .size(px(22.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.)
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .group(format!("close-{id}"))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    tell(&dismiss, cx, move |shell, cx| shell.dismiss(id, cx));
                                })
                                .child(
                                    div()
                                        .size(px(17.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(gpui::black().opacity(0.55))
                                        .group_hover(format!("close-{id}"), |s| {
                                            s.bg(gpui::black().opacity(0.75))
                                        })
                                        .child(
                                            svg()
                                                .path("icons/close.svg")
                                                .size(px(9.))
                                                .text_color(gpui::white()),
                                        ),
                                ),
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

fn add_tile(
    narrow: bool,
    tile: f32,
    focused: bool,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> impl IntoElement {
    let shell = shell.clone();
    let width = if narrow { NARROW } else { tile };
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
        .active(|s| s.opacity(0.7))
        .on_click(move |_, _, cx| tell(&shell, cx, |shell, cx| shell.create_display(cx)))
        .child(
            div()
                .w_full()
                .h(px(thumb_height(tile)))
                .rounded(px(9.))
                .bg(theme.label.opacity(0.06))
                .border(px(if focused { 2. } else { 0. }))
                .border_color(theme.accent.opacity(0.6))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_displays_get_full_size_tiles() {
        let (width, _, tile) = Peek::size(3, 1424.0);
        assert_eq!(tile, TILE_WIDTH);
        assert_eq!(width, (20. + 3. * 218. + 44.) as f64);
    }

    #[test]
    fn many_displays_shrink_their_tiles_to_fit() {
        // Seven full-size tiles need 1590 pt on a 1424 pt screen.
        let (width, height, tile) = Peek::size(7, 1424.0);
        assert!(tile < TILE_WIDTH && tile >= MIN_TILE);
        assert!(width <= 1424.0 + 0.5);
        assert!(height < (PADDING * 2. + THUMB_HEIGHT + LABEL_HEIGHT) as f64);
    }

    #[test]
    fn tiles_stop_shrinking_at_the_minimum() {
        let (_, _, tile) = Peek::size(30, 1424.0);
        assert_eq!(tile, MIN_TILE);
    }
}

/// What a screen shows before its first frame arrives.
pub fn waiting(size: f32) -> gpui::Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size))
        .text_color(gpui::white().opacity(0.7))
        .child("等待画面…")
}
