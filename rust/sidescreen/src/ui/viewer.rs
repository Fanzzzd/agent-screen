//! The Viewer: the large panel that mirrors one Agent Display and forwards
//! the user's input to it.

use super::pointer;
use super::shell::{Shell, tell};
use super::status::{Kind, Status};
use super::stream::{NewFrame, Streams};
use super::theme::{self, Theme};
use crate::core::Core;
use crate::dismissal::Item;
use crate::i18n::t;
use crate::mac::viewer_input;
use crate::model::{DisplayId, Fate};
use gpui::{
    Animation, AnimationExt, App, Bounds, Context, Entity, FocusHandle, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ObjectFit, ParentElement, Pixels, Render, Role,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, point,
    prelude::FluentBuilder, px, size, surface, svg,
};
use std::cell::Cell;
use std::time::Instant;

pub const HEADER: f32 = 52.;
pub const INSET: f32 = 8.;
pub const RADIUS: f32 = 22.;
const SCREEN_RADIUS: f32 = 13.;
/// The header fades in after the card lands.
const HEADER_IN: std::time::Duration = std::time::Duration::from_millis(120);
/// The Dismissal sheet fades in, and out once answered.
const SHEET_IN: std::time::Duration = std::time::Duration::from_millis(160);
pub const SHEET_OUT: std::time::Duration = std::time::Duration::from_millis(120);

/// The Dismissal question, laid over the screen.
#[derive(Clone)]
pub struct Sheet {
    pub display: DisplayId,
    pub label: String,
    pub agent: String,
    pub items: Vec<Item>,
    /// An Agent Session still uses the display.
    pub live: bool,
}

thread_local! {
    /// The 窗口 button's bounds in the window, from the last paint.
    static WINDOWS_BUTTON: Cell<Option<Bounds<Pixels>>> = const { Cell::new(None) };
    /// The display image within the screen area, from the last paint:
    /// left and top offsets and width, in window points.
    static IMAGE: Cell<Option<(f32, f32, f32)>> = const { Cell::new(None) };
    /// Each menu bar title's bounds in the window, by title index.
    static TITLES: std::cell::RefCell<Vec<[f64; 4]>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The menu bar title under window x, with its bounds as x, y, width, height.
pub fn title_at(x: f64) -> Option<(usize, [f64; 4])> {
    TITLES.with(|t| {
        t.borrow()
            .iter()
            .enumerate()
            .find(|(_, b)| x >= b[0] && x < b[0] + b[2])
            .map(|(i, b)| (i, *b))
    })
}

pub struct Viewer {
    /// The menu bar of the display's front app, drawn over the strip where
    /// macOS shows the user's own app.
    pub menu_bar: Option<crate::mac::menubar::Bar>,
    shell: WeakEntity<Shell>,
    core: Entity<Core>,
    streams: Entity<Streams>,
    pub display: Option<DisplayId>,
    /// Windows on the display, refreshed by the shell.
    pub count: usize,
    pub sheet: Option<Sheet>,
    /// An answered sheet, fading out.
    pub leaving: Option<Sheet>,
    /// The card is growing out of or shrinking into the menu bar; its
    /// header would crowd a small card, so it hides until the card lands.
    pub morphing: bool,
    /// Counts landings, so the header fades in once per landing.
    pub landed: u64,
    /// Counts switches between displays, so each switch crossfades.
    pub switches: u64,
    /// Counts clicks on the menu bar strip that reached no menu, so each
    /// one flashes the strip red.
    pub refused: u64,
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&core, |_, _, cx| cx.notify()).detach();
        // Light and dark switch while the panel is up.
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
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
            menu_bar: None,
            count: 0,
            sheet: None,
            leaving: None,
            morphing: false,
            landed: 0,
            switches: 0,
            refused: 0,
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
                .child(tf!(
                    "Drop to hand it over to {agent}",
                    "松手把窗口交给 {agent}"
                ))
                .into_any_element()
        } else if status.kind == Kind::Help {
            // The reason is the one line the user must read; one line
            // can't hold a long one, so it shows in full on hover.
            let reason = status.long.clone();
            div()
                .id("status")
                .min_w_0()
                .tooltip(move |_, cx| super::shell::hint(reason.clone(), cx))
                .tooltip_show_delay(super::shell::HINT_DELAY)
                .child(status.line(&status.long, 12.))
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
            .child({
                let morphing = self.morphing;
                div().flex_none().child(header).with_animation(
                    gpui::ElementId::NamedInteger("header-in".into(), self.landed),
                    Animation::new(HEADER_IN),
                    move |header, t| header.opacity(if morphing { 0. } else { t }),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .px(px(INSET))
                    .pb(px(INSET))
                    // Switching displays crossfades instead of swapping.
                    .child(div().size_full().child(screen).with_animation(
                        gpui::ElementId::NamedInteger("screen-in".into(), self.switches),
                        Animation::new(std::time::Duration::from_millis(200)),
                        |screen, t| screen.opacity(t),
                    ))
                    .when_some(self.sheet.as_ref(), |d, sheet| {
                        d.child(self.sheet_view(sheet, &theme, cx).with_animation(
                            "sheet-in",
                            Animation::new(SHEET_IN).with_easing(gpui::ease_out_quint()),
                            |sheet, t| sheet.opacity(t),
                        ))
                    })
                    .when(self.sheet.is_none(), |d| {
                        d.when_some(self.leaving.as_ref(), |d, sheet| {
                            d.child(self.sheet_view(sheet, &theme, cx).with_animation(
                                "sheet-out",
                                Animation::new(SHEET_OUT).with_easing(gpui::quadratic),
                                |sheet, t| sheet.opacity(1. - t),
                            ))
                        })
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
        Kind::Agent => Some((t("Take Over", "接手操作").to_string(), None, None)),
        Kind::Help => Some((
            t("Take Over", "接手操作").to_string(),
            Some("icons/hand.svg"),
            Some(theme.orange),
        )),
        Kind::Takeover => Some((
            tf!("Let {agent} Continue", "让 {agent} 继续"),
            Some("icons/play.svg"),
            Some(theme.accent),
        )),
        Kind::Review | Kind::Disconnected => Some((
            t("Review Windows…", "处理窗口…").to_string(),
            None,
            Some(theme.accent),
        )),
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
                .min_w(px(120.))
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
        .child(
            div()
                .relative()
                .flex_none()
                .child(button(
                    "windows",
                    tf!("Windows {count}", "窗口 {count}"),
                    Some("icons/windows.svg"),
                    None,
                    theme,
                    move |cx| {
                        let below = WINDOWS_BUTTON.with(Cell::get);
                        tell(&windows_shell, cx, move |shell, cx| {
                            shell.windows_menu(id, below, cx)
                        })
                    },
                ))
                // Where the button is, so its menu drops from it.
                .child(
                    canvas(
                        |bounds, _, _| WINDOWS_BUTTON.with(|b| b.set(Some(bounds))),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                ),
        )
        .child(icon_button(
            "collapse",
            "icons/chevron-up.svg",
            t("Collapse to Menu Bar", "收起到菜单栏"),
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
    // Tinted buttons rest a little faded and light up on hover.
    let (bg, hover, fg) = match prominent {
        Some(color) => (color.opacity(0.88), color, gpui::white()),
        None => (theme.control, theme.control_hover, theme.label),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(SharedString::from(title.clone()))
        .flex_none()
        // A long agent name truncates instead of squeezing the title.
        .max_w(px(220.))
        .h(px(28.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded_full()
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.7))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(fg)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| action(cx))
        .when_some(icon, |d, path| {
            d.child(svg().path(path).flex_none().size(px(13.)).text_color(fg))
        })
        .child(div().min_w_0().truncate().child(title))
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
        .tooltip_show_delay(super::shell::HINT_DELAY)
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.control)
        .hover(|s| s.bg(theme.control_hover))
        .active(|s| s.opacity(0.7))
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
            None => super::peek::waiting(13.).into_any_element(),
        };
        // The native input monitor needs to know where the image is.
        // Without Screen Recording the picture stops; the notice over it
        // takes the clicks.
        let denied = !crate::mac::permissions::last().screen;
        let blocked = self.sheet.is_some() || denied;
        let input = canvas(
            |_, _, _| {},
            move |bounds, _, _: &mut Window, _| {
                let c = content_rect(bounds, size);
                IMAGE.with(|i| {
                    i.set(Some((
                        f32::from(c.origin.x - bounds.origin.x),
                        f32::from(c.origin.y - bounds.origin.y),
                        f32::from(c.size.width),
                    )))
                });
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
            .aria_label(t("Display picture", "显示器画面"))
            .relative()
            .size_full()
            .rounded(px(SCREEN_RADIUS))
            .overflow_hidden()
            .bg(theme.screen_empty)
            .child(picture)
            .child(theme::round_corners(SCREEN_RADIUS, theme.card))
            // The capture leaves out the user's menu bar; the agent app's
            // own menus go in its place, where a click opens them.
            .when_some(IMAGE.with(Cell::get), |d, image| {
                d.child(menu_strip(
                    self.menu_bar.clone(),
                    image,
                    id,
                    size,
                    self.refused,
                    theme,
                ))
            })
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
            .when(denied, |d| {
                d.child(denied_notice(self.shell.clone(), theme))
            })
    }

    fn sheet_view(
        &self,
        sheet: &Sheet,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
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
                .child(tf!("Close “{}”?", "关闭「{}」？", sheet.label))
                .into_any_element(),
        ];
        if sheet.live {
            lines.push(note(
                tf!(
                    "{} is still using this display and can’t go on once it closes.",
                    "{} 仍在使用这块显示器，关闭后它将无法继续操作。",
                    sheet.agent
                ),
                theme.orange,
            ));
        }
        let explain = if sheet.items.is_empty() {
            t(
                "There are no windows on this display.",
                "这块显示器上没有窗口。",
            )
        } else {
            t("Choose where these windows go.", "选择这些窗口的去向。")
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
                        .child(t("Goes back where it was", "取回原位置"))
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
                            .child(
                                sheet_button("cancel", t("Cancel", "取消"), "esc", None, theme)
                                    .on_click(cancel),
                            )
                            .child(
                                sheet_button(
                                    "confirm",
                                    t("Close Display", "关闭显示器"),
                                    "⌘↩",
                                    Some(confirm_color),
                                    theme,
                                )
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

/// Over the picture while Screen Recording is off.
fn denied_notice(shell: WeakEntity<Shell>, theme: &Theme) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .bg(theme.scrim)
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(gpui::white())
                .child(t("Screen Recording is off", "屏幕录制权限已关闭")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(gpui::white().opacity(0.8))
                .child(t(
                    "The picture stopped updating, and the agent can’t take screenshots.",
                    "画面不再更新，agent 也截不到图。",
                )),
        )
        .child(div().mt(px(6.)).child(button(
            "denied-fix",
            t("Open Settings", "打开设置").into(),
            None,
            Some(theme.accent),
            theme,
            move |cx| tell(&shell, cx, |shell, cx| shell.open_setup(cx)),
        )))
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
    shortcut: &'static str,
    color: Option<Hsla>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (bg, hover, fg) = match color {
        Some(c) => (c.opacity(0.88), c, gpui::white()),
        None => (theme.control, theme.control_hover, theme.label),
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
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.7))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(fg)
        .gap(px(6.))
        .child(title)
        .child(div().text_size(px(10.5)).opacity(0.6).child(shortcut))
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
            .when(selected, |d| {
                d.bg(theme.segment)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .shadow(vec![gpui::BoxShadow {
                        color: gpui::black().opacity(0.12),
                        offset: point(px(0.), px(1.)),
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
            })
            .when(!selected, |d| d.hover(|s| s.bg(theme.control_hover)))
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
        .child(segment(t("Take Back", "取回"), Fate::GiveBack, cx))
        .child(segment(t("Close Window", "关闭窗口"), Fate::Close, cx))
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

/// The display's front app menu bar, laid over the image's menu bar strip
/// at the strip's own size. Clicks on it reach the shell through the native
/// input monitor, which looks titles up with `title_at`.
/// The strip where macOS draws the user's menu bar, left out of the
/// picture. With an agent app in front it holds that app's menus; without,
/// it stays clear and turns red under the pointer, since nothing there can
/// be clicked. A click that reaches no menu flashes it red.
fn menu_strip(
    bar: Option<crate::mac::menubar::Bar>,
    (left, top, width): (f32, f32, f32),
    id: DisplayId,
    size: (f64, f64),
    refused: u64,
    theme: &Theme,
) -> impl IntoElement {
    let empty = bar.is_none();
    let scale = width / size.0 as f32;
    let height = crate::mac::menubar_height(id) as f32 * scale;
    let font = (13.0 * scale).max(8.0);
    TITLES.with(|t| t.borrow_mut().clear());
    let title = |index: usize, text: String, bold: bool| {
        div()
            .relative()
            .h_full()
            .flex()
            .items_center()
            .px(px(7.0 * scale))
            .rounded(px(4.0 * scale))
            .cursor(gpui::CursorStyle::Arrow)
            .hover(|s| s.bg(theme.control_hover))
            .when(bold, |d| d.font_weight(gpui::FontWeight::BOLD))
            .child(text)
            .child(
                canvas(
                    move |b, _, _| {
                        let rect = [b.origin.x, b.origin.y, b.size.width, b.size.height]
                            .map(|v| f32::from(v) as f64);
                        TITLES.with(|t| {
                            let mut t = t.borrow_mut();
                            if t.len() <= index {
                                t.resize(index + 1, [0.0; 4]);
                            }
                            t[index] = rect;
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    };
    div()
        // Full width: in the first frame of a resize the image position is
        // the last paint's, and a strip only as wide as that would let the
        // user's menu bar show at the edge.
        .absolute()
        .left_0()
        .top(px(top))
        .w_full()
        .h(px(height))
        .flex()
        .items_center()
        .pl(px(left + 6.0 * scale))
        .pr(px(6.0 * scale))
        .group("menu-strip")
        .cursor(gpui::CursorStyle::OperationNotAllowed)
        .when(!empty, |d| d.bg(theme.menu_strip))
        .when(empty, |d| d.hover(|s| s.bg(theme.red.opacity(0.75))))
        .text_size(px(font))
        .text_color(theme.label)
        .overflow_hidden()
        // The first menu is the app's own, named in bold as on a Mac.
        .children(
            bar.map(|b| b.titles)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(i, t)| title(i, t, i == 0)),
        )
        .when(empty, |d| {
            d.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(font.max(11.)))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(gpui::white())
                    .opacity(0.)
                    .group_hover("menu-strip", |s| s.opacity(1.))
                    .child(t(
                        "No menus here: no app is open on this display yet",
                        "这里没有可用的菜单：屏幕上还没有 app",
                    )),
            )
        })
        .when(refused > 0, |d| {
            d.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(theme.red.opacity(0.9))
                    .with_animation(
                        gpui::ElementId::NamedInteger("menu-refused".into(), refused),
                        Animation::new(std::time::Duration::from_millis(600)),
                        |flash, t| flash.opacity(1. - t),
                    ),
            )
        })
}
