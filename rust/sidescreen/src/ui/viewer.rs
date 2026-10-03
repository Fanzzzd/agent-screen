//! The Viewer: the large panel that mirrors one Agent Display and forwards
//! the user's input to it.

use super::pointer;
use super::shell::{Shell, tell};
use super::status::{Kind, Status};
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::dismissal::Item;
use crate::mac::viewer_input;
use crate::model::{DisplayId, Fate};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ObjectFit, ParentElement, Pixels, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, point,
    prelude::FluentBuilder, px, size, surface, svg,
};
use std::time::Instant;

pub const HEADER: f32 = 52.;
pub const INSET: f32 = 8.;
pub const RADIUS: f32 = 22.;
const SCREEN_RADIUS: f32 = 13.;

/// The Dismissal question, laid over the screen.
pub struct Sheet {
    pub display: DisplayId,
    pub label: String,
    pub agent: String,
    pub items: Vec<Item>,
    /// An Agent Session still uses the display.
    pub live: bool,
}

pub struct Viewer {
    shell: WeakEntity<Shell>,
    core: Entity<Core>,
    streams: Entity<Streams>,
    pub display: Option<DisplayId>,
    /// Windows on the display, refreshed by the shell.
    pub count: usize,
    pub sheet: Option<Sheet>,
    /// A dragged window hovers over the card.
    pub drop_target: bool,
    /// A Handover just landed: a brief accent ring.
    pub flash_until: Option<Instant>,
    pub focus: FocusHandle,
}

impl Viewer {
    pub fn new(
        shell: WeakEntity<Shell>,
        core: Entity<Core>,
        streams: Entity<Streams>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&core, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&streams, |viewer: &mut Viewer, _, frame: &NewFrame, cx| {
            if viewer.display == Some(frame.0) {
                cx.notify();
            }
        })
        .detach();
        Viewer {
            shell,
            core,
            streams,
            display: None,
            count: 0,
            sheet: None,
            drop_target: false,
            flash_until: None,
            focus: cx.focus_handle(),
        }
    }

    /// Card size for a display on a screen with `visible` room, in points.
    pub fn card_size(display: (f64, f64), visible: (f64, f64)) -> (f64, f64) {
        let (header, inset) = (HEADER as f64, INSET as f64);
        let aspect = display.0 / display.1;
        let mut width = (visible.0 * 0.62).min(1500.0);
        let mut height = (width - inset * 2.0) / aspect + header + inset;
        let max_height = visible.1 * 0.86;
        if height > max_height {
            height = max_height;
            width = (height - header - inset) * aspect + inset * 2.0;
        }
        (width.round(), height.round())
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::theme(window);
        let Some(id) = self.display else {
            return div().size_full().into_any_element();
        };
        let core = self.core.read(cx);
        let Some(display) = core.displays.get(id) else {
            return div().size_full().into_any_element();
        };
        let status = Status::of(display, self.count, &theme);
        let label = display.label();
        let agent = display.agent_name().to_string();
        let size = (display.width, display.height);
        let pointer = (!display.takeover && !display.is_ended())
            .then_some(display.pointer)
            .flatten();
        let frame = self.streams.read(cx).frame(id);
        let flashing = self.flash_until.is_some_and(|t| t > Instant::now());
        let outline = if self.drop_target || flashing {
            Some((theme.accent, 4.))
        } else {
            None
        };
        let status_line = if self.drop_target {
            div()
                .text_size(px(12.))
                .text_color(theme.accent)
                .truncate()
                .child(format!("松手把窗口交给 {agent}"))
                .into_any_element()
        } else {
            status.line(&status.long, 12.).into_any_element()
        };

        let header = header(
            id,
            &label,
            status_line,
            status.kind,
            &agent,
            self.count,
            &theme,
            &self.shell,
        );
        let screen = self.screen(id, size, frame, pointer, outline, &theme);
        div()
            .id("viewer")
            .role(Role::Group)
            .aria_label(SharedString::from(label.clone()))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.card)
            .border_1()
            .border_color(theme.card_border)
            .rounded(px(RADIUS))
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .px(px(INSET))
                    .pb(px(INSET))
                    .child(screen)
                    .when_some(self.sheet.as_ref(), |d, sheet| {
                        d.child(self.sheet_view(sheet, &theme, cx))
                    }),
            )
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn header(
    id: DisplayId,
    label: &str,
    status: gpui::AnyElement,
    kind: Kind,
    agent: &str,
    count: usize,
    theme: &Theme,
    shell: &WeakEntity<Shell>,
) -> impl IntoElement {
    let primary = match kind {
        Kind::Manual => None,
        Kind::Agent => Some(("接手操作".to_string(), None, None)),
        Kind::Help => Some((
            "接手操作".to_string(),
            Some("icons/hand.svg"),
            Some(theme.orange),
        )),
        Kind::Takeover => Some((
            format!("让 {agent} 继续"),
            Some("icons/play.svg"),
            Some(theme.accent),
        )),
        Kind::Review | Kind::Disconnected => {
            Some(("处理窗口…".to_string(), None, Some(theme.accent)))
        }
    };
    let primary_shell = shell.clone();
    let windows_shell = shell.clone();
    let close_shell = shell.clone();
    div()
        .id("header")
        .h(px(HEADER))
        .flex_none()
        .flex()
        .items_center()
        .pl(px(16.))
        .pr(px(12.))
        .gap(px(8.))
        // Dragging the header moves the panel.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.label)
                        .truncate()
                        .child(label.to_string()),
                )
                .child(status),
        )
        .when_some(primary, |d, (title, icon, color)| {
            d.child(button("primary", title, icon, color, theme, move |cx| {
                tell(&primary_shell, cx, move |shell, cx| shell.primary(id, cx))
            }))
        })
        .child(button(
            "windows",
            format!("窗口 {count}"),
            Some("icons/windows.svg"),
            None,
            theme,
            move |cx| {
                tell(&windows_shell, cx, move |shell, cx| {
                    shell.windows_menu(id, cx)
                })
            },
        ))
        .child(icon_button(
            "collapse",
            "icons/chevron-up.svg",
            "收起到菜单栏",
            theme,
            move |cx| tell(&close_shell, cx, |shell, cx| shell.close_viewer(cx)),
        ))
}

/// A capsule button. `prominent` fills it with that color.
fn button(
    id: &'static str,
    title: String,
    icon: Option<&'static str>,
    prominent: Option<Hsla>,
    theme: &Theme,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let (bg, hover, fg) = match prominent {
        Some(color) => (color, color.opacity(0.85), gpui::white()),
        None => (theme.control, theme.control_hover, theme.label),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(SharedString::from(title.clone()))
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(fg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| action(cx))
        .when_some(icon, |d, path| {
            d.child(svg().path(path).size(px(13.)).text_color(fg))
        })
        .child(title)
}

fn icon_button(
    id: &'static str,
    icon: &'static str,
    hint: &'static str,
    theme: &Theme,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(hint)
        .tooltip(move |_, cx| super::shell::hint(hint, cx))
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.control)
        .hover(|s| s.bg(theme.control_hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| action(cx))
        .child(svg().path(icon).size(px(14.)).text_color(theme.label))
}

/// Where the display image sits in `bounds`: aspect-fit, centered.
fn content_rect(bounds: Bounds<Pixels>, display: (f64, f64)) -> Bounds<Pixels> {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let scale = (w / display.0 as f32).min(h / display.1 as f32);
    let fit = size(px(display.0 as f32 * scale), px(display.1 as f32 * scale));
    Bounds::new(
        point(
            bounds.origin.x + (bounds.size.width - fit.width) / 2.,
            bounds.origin.y + (bounds.size.height - fit.height) / 2.,
        ),
        fit,
    )
}

impl Viewer {
    fn screen(
        &self,
        id: DisplayId,
        size: (f64, f64),
        frame: Option<core_video::pixel_buffer::CVPixelBuffer>,
        pointer: Option<crate::model::AgentPointer>,
        outline: Option<(Hsla, f32)>,
        theme: &Theme,
    ) -> impl IntoElement {
        let picture = match frame {
            Some(frame) => surface(frame)
                .object_fit(ObjectFit::Contain)
                .size_full()
                .into_any_element(),
            None => div().size_full().into_any_element(),
        };
        // The native input monitor needs to know where the image is.
        let blocked = self.sheet.is_some();
        let input = canvas(
            |_, _, _| {},
            move |bounds, _, _: &mut Window, _| {
                let c = content_rect(bounds, size);
                let rect = [c.origin.x, c.origin.y, c.size.width, c.size.height]
                    .map(|v| f32::from(v) as f64);
                viewer_input::layout(Some(rect), blocked);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        div()
            .id("screen")
            .role(Role::Image)
            .aria_label("显示器画面")
            .relative()
            .size_full()
            .rounded(px(SCREEN_RADIUS))
            .overflow_hidden()
            .bg(theme.screen_empty)
            .child(picture)
            .child(theme::round_corners(SCREEN_RADIUS, theme.card))
            .child(pointer::layer(id, pointer, size, theme.accent))
            .when_some(outline, |d, (color, width)| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(SCREEN_RADIUS))
                        .border(px(width))
                        .border_color(color),
                )
            })
            .child(input)
    }

    fn sheet_view(&self, sheet: &Sheet, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let closes_window = sheet.items.iter().any(|i| i.fate == Fate::Close);
        let confirm_color = if closes_window || sheet.live {
            theme.red
        } else {
            theme.accent
        };
        let mut lines = vec![
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.label)
                .child(format!("关闭「{}」？", sheet.label))
                .into_any_element(),
        ];
        if sheet.live {
            lines.push(note(
                format!(
                    "{} 仍在使用这块显示器，关闭后它将无法继续操作。",
                    sheet.agent
                ),
                theme.orange,
            ));
        }
        let explain = if sheet.items.is_empty() {
            "这块显示器上没有窗口。"
        } else {
            "选择这些窗口的去向。"
        };
        lines.push(note(explain.into(), theme.secondary));

        let rows: Vec<_> = sheet
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let fate = if item.locked {
                    // The user's own window always comes back.
                    div()
                        .text_size(px(12.))
                        .text_color(theme.secondary)
                        .child("取回原位置")
                        .into_any_element()
                } else {
                    segmented(index, item.fate, theme, cx).into_any_element()
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.label)
                                    .truncate()
                                    .child(item.name()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.secondary)
                                    .child(item.reason.clone()),
                            ),
                    )
                    .child(div().flex_none().child(fate))
            })
            .collect();

        let cancel_shell = self.shell.clone();
        let confirm = cx.listener(|viewer: &mut Viewer, _, _, cx| viewer.answer(true, cx));
        let cancel = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            tell(&cancel_shell, cx, |shell, cx| {
                shell.answer_dismiss(None, cx)
            });
        };
        div()
            .id("sheet")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|viewer: &mut Viewer, e: &KeyDownEvent, _, cx| {
                // Return alone could close windows by accident; ⌘Return confirms.
                match e.keystroke.key.as_str() {
                    "escape" => viewer.answer(false, cx),
                    "enter" if e.keystroke.modifiers.platform => viewer.answer(true, cx),
                    _ => {}
                }
            }))
            .absolute()
            .top_0()
            .bottom(px(INSET))
            .left(px(INSET))
            .right(px(INSET))
            .rounded(px(SCREEN_RADIUS))
            .bg(theme.scrim)
            .flex()
            .items_center()
            .justify_center()
            // Clicks and scrolls stay here instead of reaching the display.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w(px(420.))
                    .max_h_full()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(20.))
                    .pt(px(18.))
                    .pb(px(16.))
                    .rounded(px(16.))
                    .bg(theme.sheet)
                    .border_1()
                    .border_color(theme.card_border)
                    .children(lines)
                    .when(!rows.is_empty(), |d| {
                        d.child(
                            div()
                                .id("rows")
                                .mt(px(8.))
                                .max_h(px(300.))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap(px(10.))
                                .children(rows),
                        )
                    })
                    .child(
                        div()
                            .mt(px(12.))
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(sheet_button("cancel", "取消", None, theme).on_click(cancel))
                            .child(
                                sheet_button("confirm", "关闭显示器", Some(confirm_color), theme)
                                    .on_click(confirm),
                            ),
                    ),
            )
    }

    /// Answer the open sheet: the adjusted plan, or a cancel.
    pub fn answer(&mut self, confirmed: bool, cx: &mut Context<Self>) {
        let Some(sheet) = self.sheet.as_ref() else {
            return;
        };
        let items = confirmed.then(|| sheet.items.clone());
        tell(&self.shell, cx, move |shell, cx| {
            shell.answer_dismiss(items, cx)
        });
    }
}

fn note(text: String, color: Hsla) -> gpui::AnyElement {
    div()
        .text_size(px(12.))
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn sheet_button(
    id: &'static str,
    title: &'static str,
    color: Option<Hsla>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (bg, fg) = match color {
        Some(c) => (c, gpui::white()),
        None => (theme.control, theme.label),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(title)
        .h(px(28.))
        .px(px(14.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.opacity(0.85))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(fg)
        .child(title)
}

/// 取回 | 关闭窗口 for one window.
fn segmented(
    index: usize,
    fate: Fate,
    theme: &Theme,
    cx: &mut Context<Viewer>,
) -> impl IntoElement {
    let segment = |label: &'static str, value: Fate, cx: &mut Context<Viewer>| {
        let selected = fate == value;
        div()
            .id(gpui::ElementId::Name(
                format!("fate-{index}-{label}").into(),
            ))
            .role(Role::Button)
            .aria_label(label)
            .px(px(9.))
            .h(px(22.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_size(px(11.5))
            .text_color(if selected {
                theme.label
            } else {
                theme.secondary
            })
            .when(selected, |d| d.bg(theme.control_hover))
            .on_click(cx.listener(move |viewer: &mut Viewer, _, _, cx| {
                if let Some(item) = viewer.sheet.as_mut().and_then(|s| s.items.get_mut(index)) {
                    item.fate = value;
                    cx.notify();
                }
            }))
            .child(label)
    };
    div()
        .flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(8.))
        .bg(theme.control)
        .child(segment("取回", Fate::GiveBack, cx))
        .child(segment("关闭窗口", Fate::Close, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_fits_the_screen() {
        // A 16:10 display on a laptop screen: 62% of the width.
        let (w, h) = Viewer::card_size((1440., 900.), (1728., 1080.));
        assert_eq!(w, 1071.);
        assert!(h <= 1080. * 0.86);
        // A portrait display is limited by the height instead.
        let (w, h) = Viewer::card_size((1000., 1500.), (1728., 1084.));
        assert_eq!(h, (1084. * 0.86f64).round());
        assert!(w < h);
    }

    #[test]
    fn content_is_centered_and_aspect_fit() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.)));
        let fit = content_rect(bounds, (1440., 900.));
        assert_eq!(fit.size.height, px(100.));
        assert_eq!(fit.size.width, px(160.));
        assert_eq!(fit.origin.x, px(20.));
    }
}
