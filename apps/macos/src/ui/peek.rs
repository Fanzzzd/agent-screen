//! Peek: the strip of live thumbnails that drops out of the menu bar icon.

use super::pointer;
use super::shell::{Shell, tell};
use super::status::{AccessLine, Status};
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::i18n::t;
use crate::model::{DisplayId, Rect, WindowTarget};
use gpui::{
    AnimationExt, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ObjectFit, ParentElement, Render, Role, StatefulInteractiveElement, Styled,
    WeakEntity, Window, div, prelude::FluentBuilder, px, surface, svg,
};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

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
/// Room around the card in Peek's window, top, left, bottom, right, for
/// the shadow Peek draws itself (`native::own_shadow`).
pub const MARGIN: [f64; 4] = [8., 28., 40., 28.];
const SHADOW: [(f32, f32, f32); 2] = [
    // Opacity, drop, blur: a tight contact shadow and a soft wide one.
    (0.12, 1., 2.5),
    (0.28, 10., 28.),
];
/// A tile grows in or shrinks out over this, as Peek's frame changes with
/// it (`Shell::show_peek`).
pub const RESIZE: Duration = Duration::from_millis(260);

/// The card sliding to a new width inside Peek's window, which spans both
/// the card's old and new frames meanwhile: resizing the window itself
/// every frame costs WindowServer a new backing store each time, about
/// 40 ms a step, where GPUI redraws at the display's rate. Peek draws the
/// card's shadow itself, so it moves with the card in the same frame. Left
/// edge and width, in the window less its margin. Peek times it itself: a `with_animation` around
/// the card would put its id in every tile's element path, so the tiles'
/// own grow and shrink would start over when the slide ends.
#[derive(Clone, Copy, PartialEq)]
pub struct Slide {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub started: std::time::Instant,
}

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
    /// Opened by a click, so the keyboard drives it.
    pub keys: bool,
    /// Displays being made for "+": a tile each, waiting.
    pub pending: usize,
    /// The card is sliding to a new width.
    pub slide: Option<Slide>,
    /// Peek came down to say where an app's windows are.
    pub remind: Option<Remind>,
    /// Displays that take the place of a pending tile: they show without
    /// growing in again.
    pub arrived: HashSet<DisplayId>,
    /// The displays the last render showed, while Peek was up.
    known: Vec<DisplayId>,
    /// Peek was up for the last render too.
    following: bool,
    /// Displays that came while Peek was up, which grow in.
    entering: HashSet<DisplayId>,
    /// Displays that went while Peek was up, by where their tile stood,
    /// shrinking out.
    leaving: Vec<(DisplayId, usize)>,
    /// How each tile looked when last drawn: a closed display's tile
    /// shrinks out as it was, not as a blank box.
    looks: HashMap<DisplayId, Look>,
}

/// Where an app the user went to has its windows: Peek came down to show
/// it, and rings that display's tile and outlines the windows on it.
#[derive(Clone)]
pub struct Remind {
    pub display: DisplayId,
    pub app: String,
    /// The app's windows on the display, with their frames in the
    /// display's points.
    pub windows: Vec<(WindowTarget, Rect)>,
    /// Counts reminders, so each one's glow plays from its start.
    pub seq: u64,
}

/// A tile's picture and words, kept for its shrinking out.
#[derive(Clone)]
struct Look {
    label: String,
    status: Status,
    frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    size: (f64, f64),
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
            keys: false,
            pending: 0,
            slide: None,
            remind: None,
            arrived: HashSet::new(),
            known: Vec::new(),
            following: false,
            entering: HashSet::new(),
            leaving: Vec::new(),
            looks: HashMap::new(),
        }
    }

    /// ←/→ move between tiles, Return opens one, 1-9 jump to a display,
    /// ⌘N adds one and Esc closes Peek.
    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // Hidden, or shown again by hover before it left: the keys aren't
        // meant for it.
        if !self.keys {
            return;
        }
        let ids = ordered(self.core.read(cx));
        let count = ids.len() + 1;
        // A display may have gone since the arrows moved.
        let current = self
            .focused
            .filter(|&i| i < count)
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

    /// Which displays grow in and which shrink out: those that came or went
    /// since the last render while Peek was up.
    fn follow(&mut self, ids: &[DisplayId], cx: &mut Context<Self>) {
        if !self.shown {
            self.known.clear();
            self.entering.clear();
            self.leaving.clear();
            self.looks.clear();
            self.arrived.clear();
            self.following = false;
            return;
        }
        if self.following {
            for id in ids.iter().filter(|id| !self.known.contains(id)) {
                if !self.arrived.remove(id) {
                    self.entering.insert(*id);
                }
            }
        }
        for (index, id) in self.known.iter().enumerate() {
            if !ids.contains(id) && !self.leaving.iter().any(|(l, _)| l == id) {
                let id = *id;
                self.leaving.push((id, index));
                self.entering.remove(&id);
                cx.spawn(async move |peek, cx| {
                    cx.background_executor()
                        .timer(RESIZE + Duration::from_millis(40))
                        .await;
                    let _ = peek.update(cx, |peek, cx| {
                        peek.leaving.retain(|(l, _)| *l != id);
                        peek.looks.remove(&id);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        self.known = ids.to_vec();
        self.following = true;
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
        let ids = ordered(self.core.read(cx));
        self.follow(&ids, cx);
        let core = self.core.read(cx);
        let streams = self.streams.read(cx);
        let mut tiles: Vec<gpui::AnyElement> = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            let Some(display) = core.displays.get(*id) else {
                continue;
            };
            let count = self.counts.get(id).copied().unwrap_or(0);
            let status = Status::of(display, count, &theme);
            let pointer = (!display.takeover && !display.is_ended())
                .then_some(display.pointer)
                .flatten();
            let frame = streams.frame(*id);
            self.looks.insert(
                *id,
                Look {
                    label: display.label(),
                    status: status.clone(),
                    frame: frame.clone(),
                    size: (display.width, display.height),
                },
            );
            let built = tile(
                *id,
                display.label(),
                crate::mac::recording::is_recording(*id),
                AccessLine::of(display),
                status,
                frame,
                pointer,
                (display.width, display.height),
                self.active == Some(*id),
                self.focused == Some(index),
                self.remind.as_ref().filter(|r| r.display == *id),
                self.tile,
                &theme,
                &self.shell,
            );
            tiles.push(if self.entering.contains(id) {
                grow(("tile-in", *id), self.tile, true, built).into_any_element()
            } else {
                built.into_any_element()
            });
        }
        for (id, index) in &self.leaving {
            // As it last looked, deaf to clicks: its display is gone.
            let look = self.looks.get(id).cloned();
            if cfg!(debug_assertions) {
                log::debug!("peek leaving {id} as drawn {}", look.is_some());
            }
            let ghost = match look {
                Some(look) => tile(
                    *id,
                    look.label,
                    false,
                    None,
                    look.status,
                    look.frame,
                    None,
                    look.size,
                    false,
                    false,
                    None,
                    self.tile,
                    &theme,
                    &WeakEntity::new_invalid(),
                )
                .into_any_element(),
                None => div()
                    .w(px(self.tile))
                    .h(px(thumb_height(self.tile)))
                    .rounded(px(9.))
                    .bg(theme.screen_empty)
                    .into_any_element(),
            };
            let at = (*index).min(tiles.len());
            tiles.insert(
                at,
                grow(("tile-out", *id), self.tile, false, ghost).into_any_element(),
            );
        }
        for index in 0..self.pending {
            let waiting = div().w(px(self.tile)).flex().flex_col().child(
                div()
                    .h(px(thumb_height(self.tile)))
                    .rounded(px(9.))
                    .overflow_hidden()
                    .bg(theme.screen_empty)
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.5))
                            .text_color(theme.secondary)
                            .child(t("Preparing…", "准备中…")),
                    ),
            );
            tiles
                .push(grow(("pending", index as u32), self.tile, true, waiting).into_any_element());
        }
        let narrow = !ids.is_empty() || self.pending > 0;
        let shell = self.shell.clone();
        let add_focused = self.focused == Some(ids.len());
        let [top, left, bottom, right] = MARGIN.map(|m| m as f32);
        let card = div()
            .id("card")
            .absolute()
            .top(px(top))
            .bottom(px(bottom))
            .shadow(
                SHADOW
                    .iter()
                    .map(|&(opacity, drop, blur)| gpui::BoxShadow {
                        color: gpui::black().opacity(opacity),
                        offset: gpui::point(px(0.), px(drop)),
                        blur_radius: px(blur),
                        spread_radius: px(0.),
                        inset: false,
                    })
                    .collect(),
            )
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
            ));
        let viewport = f32::from(window.viewport_size().width) - left - right;
        // A slide that has ended keeps the card where it stopped until the
        // window takes the card's frame.
        let slid = self.slide.filter(|slide| {
            slide.started.elapsed() < RESIZE || (viewport - slide.to.1).abs() > 0.5
        });
        let card = match slid {
            Some(slide) => {
                let t = (slide.started.elapsed().as_secs_f32() / RESIZE.as_secs_f32()).min(1.);
                if t < 1. {
                    window.request_animation_frame();
                }
                let t = gpui::ease_out_quint()(t);
                let mix = |a: f32, b: f32| a + (b - a) * t;
                let width = mix(slide.from.1, slide.to.1);
                if self.shown {
                    log_card(width);
                }
                card.left(px(left + mix(slide.from.0, slide.to.0)))
                    .w(px(width))
            }
            None => {
                if self.shown {
                    log_card(viewport);
                }
                card.left(px(left)).right(px(right))
            }
        };
        div()
            .id("peek")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|peek: &mut Peek, e: &KeyDownEvent, _, cx| peek.key(e, cx)))
            .role(Role::Group)
            .aria_label("Agent Screen")
            .size_full()
            .relative()
            .child(card)
    }
}

/// For peekanim, in test builds: the card's width as a frame draws it,
/// each time it changes.
fn log_card(width: f32) {
    thread_local!(static DRAWN: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) });
    if cfg!(debug_assertions) && DRAWN.replace(Some(width)) != Some(width) {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        log::debug!("peek drawn {width} at {ms}");
    }
}

/// For peekanim, in test builds: how far a tile has grown in, 0 to 1.
fn log_tile((kind, id): (&'static str, DisplayId), shown: f32) {
    if cfg!(debug_assertions) {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        log::debug!("peek {kind} {id} {shown} at {ms}");
    }
}

/// `content`, `width` wide, growing in from nothing, or shrinking out to
/// nothing, over `RESIZE`, as Peek's frame follows.
fn grow(
    key: (&'static str, DisplayId),
    width: f32,
    entering: bool,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex_none()
        .overflow_hidden()
        .child(content)
        .with_animation(
            gpui::ElementId::NamedInteger(key.0.into(), key.1 as u64),
            gpui::Animation::new(RESIZE).with_easing(gpui::ease_out_quint()),
            move |d, t| {
                let t = if entering { t } else { 1. - t };
                log_tile(key, t);
                // The gap beside it closes with it.
                d.w(px(width * t)).mr(px((t - 1.) * SPACING)).opacity(t)
            },
        )
}

#[allow(clippy::too_many_arguments)]
fn tile(
    id: DisplayId,
    label: String,
    recording: bool,
    access: Option<AccessLine>,
    status: Status,
    frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
    pointer: Option<crate::model::AgentPointer>,
    size: (f64, f64),
    active: bool,
    focused: bool,
    remind: Option<&Remind>,
    width: f32,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> impl IntoElement {
    let group = format!("tile-{id}");
    let status_line = match remind {
        Some(remind) => div()
            .text_size(px(10.5))
            .text_color(theme.orange)
            .font_weight(gpui::FontWeight::MEDIUM)
            .truncate()
            .child(tf!("{} is here", "{}在这里", remind.app))
            .into_any_element(),
        None => status.line(&status.short, 10.5).into_any_element(),
    };
    let picture = match frame {
        Some(frame) => surface(frame)
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        None => waiting(id, 10.5).into_any_element(),
    };
    // The thumbnail takes the display's shape inside the tile's box, so a
    // portrait display gets round corners and a ring of its own size.
    let thumb = thumb_height(width);
    let fit = (width / size.0 as f32).min(thumb / size.1 as f32);
    let fit = (size.0 as f32 * fit, size.1 as f32 * fit);
    let select = shell.clone();
    let dismiss = shell.clone();
    let revoke = shell.clone();
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
        .aria_label(tf!("{label}, {}", "{label}，{}", status.long))
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
                        .child(theme::round_corners(9., theme.card))
                        .child(pointer::layer(id, pointer, size, theme.accent))
                        .when_some(remind, |d, remind| {
                            d.child(reminder(remind, size, fit, theme, shell))
                        })
                        .when_some(access, |d, access| {
                            d.child(access_pill(id, access, theme, revoke))
                        })
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
                                .aria_label(t("Close Display", "关闭显示器"))
                                .tooltip(|_, cx| {
                                    super::shell::hint(t("Close Display", "关闭显示器"), cx)
                                })
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
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        // Being recorded: a red dot, as in the Viewer.
                        .when(recording, |d| {
                            d.child(div().flex_none().size(px(6.)).rounded_full().bg(theme.red))
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.secondary)
                                .group_hover(group, |s| s.text_color(theme.label))
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .max_w_full()
                        .flex()
                        .justify_center()
                        .child(status_line),
                ),
        )
}

/// The session's Window Access on its tile: a pill at the bottom-left of
/// the picture, with the scope on hover and × to revoke it.
fn access_pill(
    id: DisplayId,
    access: AccessLine,
    theme: &Theme,
    shell: WeakEntity<Shell>,
) -> impl IntoElement {
    let color = if access.paused {
        gpui::white().opacity(0.75)
    } else {
        theme.orange
    };
    let hint = access.long.clone();
    div()
        .id(gpui::ElementId::Name(format!("access-{id}").into()))
        .absolute()
        .left(px(5.))
        .bottom(px(5.))
        .max_w(px(150.))
        .h(px(18.))
        .pl(px(6.))
        .pr(px(2.))
        .flex()
        .items_center()
        .gap(px(3.))
        .rounded_full()
        .bg(gpui::black().opacity(0.62))
        .text_size(px(10.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(color)
        .tooltip(move |_, cx| super::shell::hint(hint.clone(), cx))
        .tooltip_show_delay(super::shell::HINT_DELAY)
        .child(
            svg()
                .path("icons/give.svg")
                .flex_none()
                .size(px(10.))
                .text_color(color),
        )
        .child(div().min_w_0().truncate().child(access.short))
        .child(
            div()
                .id(gpui::ElementId::Name(format!("revoke-{id}").into()))
                .role(Role::Button)
                .aria_label(t("Revoke Window Access", "收回窗口授权"))
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .hover(|s| s.bg(gpui::white().opacity(0.2)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    tell(&shell, cx, move |shell, cx| shell.revoke_access(id, cx));
                })
                .child(
                    svg()
                        .path("icons/close.svg")
                        .size(px(7.))
                        .text_color(gpui::white()),
                ),
        )
}

/// The reminded app on a tile, `fit` big for a display `size` points: its
/// windows outlined, a ring that glows twice, and Take Back.
fn reminder(
    remind: &Remind,
    size: (f64, f64),
    fit: (f32, f32),
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> impl IntoElement {
    let scale = fit.0 / size.0 as f32;
    let orange = theme.orange;
    let outlines = remind.windows.iter().map(move |(_, frame)| {
        div()
            .absolute()
            .left(px(frame.x as f32 * scale))
            .top(px(frame.y as f32 * scale))
            .w(px(frame.width as f32 * scale))
            .h(px(frame.height as f32 * scale))
            .rounded(px(3.))
            .border(px(1.5))
            .border_color(orange)
    });
    let targets: Vec<WindowTarget> = remind.windows.iter().map(|(t, _)| *t).collect();
    let (display, shell) = (remind.display, shell.clone());
    let seq = remind.seq;
    div()
        .absolute()
        .inset_0()
        .children(outlines)
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(9.))
                .border(px(2.))
                .border_color(orange)
                // A glow that swells twice, then stays as a ring.
                .with_animation(
                    gpui::ElementId::NamedInteger("remind".into(), seq),
                    gpui::Animation::new(Duration::from_millis(2400)),
                    move |d, t| {
                        let swell = (t * 2. * std::f32::consts::TAU).cos() * -0.5 + 0.5;
                        d.shadow(vec![gpui::BoxShadow {
                            color: orange.opacity(0.85 * swell),
                            offset: gpui::point(px(0.), px(0.)),
                            blur_radius: px(4. + 10. * swell),
                            spread_radius: px(2. * swell),
                            inset: true,
                        }])
                    },
                ),
        )
        .child(
            div()
                .id(gpui::ElementId::Name(format!("take-back-{display}").into()))
                .role(Role::Button)
                .aria_label(t("Take Back", "取回"))
                .absolute()
                .bottom(px(6.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let targets = targets.clone();
                    tell(&shell, cx, move |shell, cx| {
                        shell.take_back_reminded(display, targets, cx)
                    });
                })
                .child(
                    div()
                        .px(px(10.))
                        .py(px(3.))
                        .rounded_full()
                        .bg(gpui::black().opacity(0.6))
                        .hover(|s| s.bg(gpui::black().opacity(0.8)))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(gpui::white())
                        .child(t("Take Back", "取回")),
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
        .aria_label(t("New Display", "新建显示器"))
        .when(narrow, |d| {
            d.tooltip(|_, cx| super::shell::hint(t("New Display", "新建显示器"), cx))
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
                .flex_col()
                .gap(px(8.))
                .child(
                    svg()
                        .path("icons/plus.svg")
                        .size(px(20.))
                        .text_color(theme.secondary),
                )
                // With no displays yet: agents make their own, and the menu
                // is a right-click away.
                .when(!narrow, |d| {
                    d.child(
                        div()
                            .px(px(10.))
                            .text_center()
                            .text_size(px(10.5))
                            .text_color(theme.secondary)
                            .child(t(
                                "Agents create their own displays. Right-click the icon for the menu.",
                                "Agent 会自己创建显示器。右键点图标打开菜单。",
                            )),
                    )
                }),
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
                .when(!narrow, |d| d.child(t("New Display", "新建显示器"))),
        )
}

/// What a screen shows before its first frame arrives.
/// What stands where display `id`'s picture goes until a frame comes. A
/// display that shares its unit number with another isn't captured
/// (`cg::unit_twin`); say so rather than wait.
pub fn waiting(id: DisplayId, size: f32) -> gpui::Div {
    let text = if crate::mac::cg::unit_twin(id).is_some() {
        t(
            "macOS mixes this display up with another one, so its picture stays hidden",
            "macOS 把这块显示器和另一块混淆了，画面先隐藏",
        )
    } else {
        t("Waiting for picture…", "等待画面…")
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(size))
        .text_center()
        .text_size(px(size))
        .text_color(gpui::white().opacity(0.85))
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_displays_get_full_size_tiles() {
        let (width, _, tile) = Peek::size(3, 1424.0);
        assert_eq!(tile, TILE_WIDTH);
        assert_eq!(width, 20. + 3. * 218. + 44.);
    }

    #[test]
    fn many_displays_shrink_their_tiles_to_fit() {
        // Seven full-size tiles need 1590 pt on a 1424 pt screen.
        let (width, height, tile) = Peek::size(7, 1424.0);
        assert!((MIN_TILE..TILE_WIDTH).contains(&tile));
        assert!(width <= 1424.0 + 0.5);
        assert!(height < (PADDING * 2. + THUMB_HEIGHT + LABEL_HEIGHT) as f64);
    }

    #[test]
    fn tiles_stop_shrinking_at_the_minimum() {
        let (_, _, tile) = Peek::size(30, 1424.0);
        assert_eq!(tile, MIN_TILE);
    }
}
