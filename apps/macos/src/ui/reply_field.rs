//! The Help Card's one-line reply field: what the user types goes to the
//! agent as their words (ADR 0015). Text and input-method composition come
//! through the field's input handler; Return sends, Escape cancels. The
//! words go exactly as typed: a pasted line break stays in the reply and
//! shows as a space in the one line.

use gpui::{
    App, Bounds, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, GlobalElementId, Hsla, KeyDownEvent, LayoutId, Pixels, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, fill, point, prelude::*,
    px, relative, size,
};
use std::ops::Range;

/// What the field asks of the card.
pub enum ReplyEvent {
    Send(String),
    Cancel,
    /// The user clicked the field: the card takes the keyboard for it.
    Focus,
}

pub struct ReplyField {
    pub focus: FocusHandle,
    pub content: String,
    /// The caret, as a byte offset into `content`.
    caret: usize,
    /// What the input method is composing.
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    colors: (Hsla, Hsla, Hsla),
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// How far the line last moved left to keep the caret in view.
    last_scroll: Pixels,
}

impl EventEmitter<ReplyEvent> for ReplyField {}

impl ReplyField {
    /// `colors`: the text, the placeholder and the caret.
    pub fn new(
        placeholder: SharedString,
        colors: (Hsla, Hsla, Hsla),
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: String::new(),
            caret: 0,
            marked: None,
            placeholder,
            colors,
            last_layout: None,
            last_bounds: None,
            last_scroll: px(0.),
        }
    }

    /// Type `text` as if the user had, for the debug hook.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = text.to_string();
        self.caret = self.content.len();
        self.marked = None;
        cx.notify();
    }

    fn previous(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, offset: usize) -> usize {
        self.content[offset..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8())
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // The input method owns the keys while it composes.
        if self.marked.is_some() {
            return;
        }
        let keys = &event.keystroke;
        let command = keys.modifiers.platform;
        match keys.key.as_str() {
            "enter" => cx.emit(ReplyEvent::Send(self.content.clone())),
            "escape" => cx.emit(ReplyEvent::Cancel),
            "backspace" if self.caret > 0 => {
                let from = if command {
                    0
                } else {
                    self.previous(self.caret)
                };
                self.content.replace_range(from..self.caret, "");
                self.caret = from;
            }
            "delete" if self.caret < self.content.len() => {
                let to = self.next(self.caret);
                self.content.replace_range(self.caret..to, "");
            }
            "left" => {
                self.caret = if command {
                    0
                } else {
                    self.previous(self.caret)
                }
            }
            "right" => {
                self.caret = if command {
                    self.content.len()
                } else {
                    self.next(self.caret)
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.content.insert_str(self.caret, &text);
                    self.caret += text.len();
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn utf16_of(&self, offset: usize) -> usize {
        self.content[..offset.min(self.content.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn offset_of_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (i, c) in self.content.char_indices() {
            if utf16 >= offset {
                return i;
            }
            utf16 += c.len_utf16();
        }
        self.content.len()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_of_utf16(range.start)..self.offset_of_utf16(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_of(range.start)..self.utf16_of(range.end)
    }

    fn replace(&mut self, range: Option<Range<usize>>, text: &str) -> Range<usize> {
        let range = range
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.clone())
            .unwrap_or(self.caret..self.caret);
        self.content.replace_range(range.clone(), text);
        range.start..range.start + text.len()
    }
}

impl EntityInputHandler for ReplyField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self.utf16_of(self.caret);
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.replace(range, text);
        self.caret = inserted.end;
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.replace(range, text);
        self.marked = (!inserted.is_empty()).then_some(inserted.clone());
        // The input method's caret is in UTF-16 units inside the marked
        // text.
        self.caret = match selected {
            Some(selected) => {
                let mut utf16 = 0;
                let mut caret = inserted.start;
                for c in self.content[inserted.clone()].chars() {
                    if utf16 >= selected.end {
                        break;
                    }
                    utf16 += c.len_utf16();
                    caret += c.len_utf8();
                }
                caret
            }
            None => inserted.end,
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range);
        let left = bounds.left() - self.last_scroll;
        Some(Bounds::from_corners(
            point(left + line.x_for_index(range.start), bounds.top()),
            point(left + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        at: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let index = line.closest_index_for_x(at.x - bounds.left() + self.last_scroll);
        Some(self.utf16_of(index))
    }
}

impl Render for ReplyField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .id("reply-text")
            .key_context("ReplyField")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(ReplyEvent::Focus)),
            )
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(Line { field: cx.entity() })
    }
}

/// The text, the composition's underline and the caret, painted from the
/// field's state.
struct Line {
    field: Entity<ReplyField>,
}

struct Painted {
    line: ShapedLine,
    caret: Option<gpui::PaintQuad>,
    /// How far the line moved left so the caret stays in view.
    scroll: Pixels,
}

impl IntoElement for Line {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Line {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Painted {
        let field = self.field.read(cx);
        let style = window.text_style();
        // A line break shows as a space, a byte for a byte, so offsets
        // into the content hold for the line.
        let (text, color): (SharedString, Hsla) = if field.content.is_empty() {
            (field.placeholder.clone(), field.colors.1)
        } else {
            (
                field.content.replace(['\n', '\r'], " ").into(),
                field.colors.0,
            )
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match field.marked.clone().filter(|_| !field.content.is_empty()) {
            Some(marked) => [
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);
        let caret_x = if field.content.is_empty() {
            px(0.)
        } else {
            line.x_for_index(field.caret)
        };
        let scroll = (caret_x - bounds.size.width + px(2.)).max(px(0.));
        let caret = Some(fill(
            Bounds::new(
                point(bounds.left() + caret_x - scroll, bounds.top()),
                size(px(1.5), bounds.size.height),
            ),
            field.colors.2,
        ));
        Painted {
            line,
            caret,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        let origin = point(bounds.origin.x - painted.scroll, bounds.origin.y);
        let _ = painted.line.paint(
            origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
        if focus.is_focused(window)
            && let Some(caret) = painted.caret.take()
        {
            window.paint_quad(caret);
        }
        let line = painted.line.clone();
        let scroll = painted.scroll;
        self.field.update(cx, |field, _| {
            field.last_layout = Some(line);
            field.last_bounds = Some(bounds);
            field.last_scroll = scroll;
        });
    }
}
