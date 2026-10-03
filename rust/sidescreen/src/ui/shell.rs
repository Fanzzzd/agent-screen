//! The shell around the Agent Displays: the menu bar icon, Peek, the
//! Viewer, Dismissal and Handover, and the guards that keep the user's
//! cursor and AeroSpace workspaces off Agent Displays.

use super::peek::{self, Peek};
use super::pointer;
use super::stream::Streams;
use super::viewer::{Sheet, Viewer};
use crate::core::Core;
use crate::dismissal::{self, Item};
use crate::mac::aerospace::AeroSpace;
use crate::mac::input::Forwarder;
use crate::mac::native::{self, MenuAction, Native, StatusIcon};
use crate::mac::{app, ax, cg, menubar, windows};
use crate::model::{DEFAULT_SIZE, DisplayId, EndReason, Origin, Rect, WindowTarget};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{
    AnyView, App, AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, ParentElement,
    Render, SharedString, Styled, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, point, px, size,
};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Run `f` on the shell once the current update is done. Views call this
/// from their listeners, since the shell in turn updates the views.
pub fn tell(
    shell: &WeakEntity<Shell>,
    cx: &mut App,
    f: impl FnOnce(&mut Shell, &mut Context<Shell>) + 'static,
) {
    let shell = shell.clone();
    cx.defer(move |cx| {
        let _ = shell.update(cx, f);
    });
}

thread_local! {
    /// The shell lives as long as the app.
    static CURRENT: std::cell::RefCell<Option<Entity<Shell>>> = const { std::cell::RefCell::new(None) };
}

/// The running shell, for debug control methods.
pub fn current() -> Option<WeakEntity<Shell>> {
    CURRENT.with(|c| c.borrow().as_ref().map(Entity::downgrade))
}

/// A hover hint.
struct Hint(SharedString);

impl Render for Hint {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = super::theme::theme(window);
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(theme.sheet)
            .border_1()
            .border_color(theme.card_border)
            .text_size(px(11.5))
            .text_color(theme.label)
            .child(self.0.clone())
    }
}

pub fn hint(text: &'static str, cx: &mut App) -> AnyView {
    cx.new(|_| Hint(text.into())).into()
}

/// A window being dragged by its title bar, from the user's desktop.
struct Drag {
    target: WindowTarget,
    frame: Rect,
    last_check: Instant,
}

pub struct Shell {
    core: Entity<Core>,
    streams: Entity<Streams>,
    icon: StatusIcon,
    peek: Option<(WindowHandle<Peek>, Retained<NSWindow>)>,
    viewer: Option<(WindowHandle<Viewer>, Retained<NSWindow>)>,

    peek_shown: bool,
    /// Opened by a click: stays until another click.
    latched: bool,
    over_icon: bool,
    over_card: bool,
    hover_generation: u64,
    peek_generation: u64,
    /// No hover Peek until then: the pointer is still on its way.
    settle_until: Option<Instant>,

    viewer_display: Option<DisplayId>,
    /// The input queue of the display in the Viewer.
    forwarder: Option<Forwarder>,
    viewer_open: bool,
    viewer_generation: u64,
    /// Where the card rests, so it comes back where the user dragged it.
    resting: Option<NSRect>,

    known: HashSet<DisplayId>,
    drag: Option<Drag>,
    drop_hover: bool,
    /// AeroSpace workspaces pinned to a monitor number an Agent Display took.
    conflicts: Vec<(String, String)>,
    guard_generation: u64,
    asking: bool,
}

/// Pixel size a display is captured at.
fn pixels(display: (f64, f64)) -> (u32, u32) {
    ((display.0 * 2.0) as u32, (display.1 * 2.0) as u32)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

fn contains(r: NSRect, (x, y): (f64, f64)) -> bool {
    x >= r.origin.x
        && x <= r.origin.x + r.size.width
        && y >= r.origin.y
        && y <= r.origin.y + r.size.height
}

fn intersects(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

fn offset(r: NSRect, dy: f64) -> NSRect {
    rect(r.origin.x, r.origin.y + dy, r.size.width, r.size.height)
}

fn panel_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(400.), px(200.)),
        ))),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: false,
        show: false,
        is_movable: true,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

/// Start the shell: the menu bar icon, Peek, the Viewer and the guards.
pub fn start(core: Entity<Core>, cx: &mut App) -> anyhow::Result<Entity<Shell>> {
    let mtm = MainThreadMarker::new().expect("main thread");
    let (events, mut native_events) = mpsc::unbounded::<Native>();
    native::set_sender(events);
    let icon = StatusIcon::install(mtm);
    native::watch_mouse();
    native::watch_keys();

    let streams = Streams::new(cx);
    let shell = cx.new(|cx| {
        cx.observe(&core, |shell: &mut Shell, _, cx| shell.sync(cx))
            .detach();
        Shell {
            core: core.clone(),
            streams: streams.clone(),
            icon,
            peek: None,
            viewer: None,
            peek_shown: false,
            latched: false,
            over_icon: false,
            over_card: false,
            hover_generation: 0,
            peek_generation: 0,
            settle_until: None,
            viewer_display: None,
            forwarder: None,
            viewer_open: false,
            viewer_generation: 0,
            resting: None,
            known: HashSet::new(),
            drag: None,
            drop_hover: false,
            conflicts: Vec::new(),
            guard_generation: 0,
            asking: false,
        }
    });
    let weak = shell.downgrade();
    CURRENT.with(|c| *c.borrow_mut() = Some(shell.clone()));

    let peek = {
        let (weak, core, streams) = (weak.clone(), core.clone(), streams.clone());
        cx.open_window(panel_options(), move |_, cx| {
            cx.new(|cx| Peek::new(weak, core, streams, cx))
        })?
    };
    let viewer = {
        let (weak, core, streams) = (weak.clone(), core.clone(), streams.clone());
        cx.open_window(panel_options(), move |_, cx| {
            cx.new(|cx| Viewer::new(weak, core, streams, cx))
        })?
    };
    let peek_ns = peek
        .update(cx, |_, window, _| native::ns_window(window))?
        .expect("Peek window");
    let viewer_ns = viewer
        .update(cx, |_, window, _| native::ns_window(window))?
        .expect("Viewer window");
    native::style_panel(&peek_ns, native::LEVEL_POP_UP_MENU);
    native::style_panel(&viewer_ns, native::LEVEL_FLOATING);
    shell.update(cx, |shell, _| {
        shell.peek = Some((peek, peek_ns));
        shell.viewer = Some((viewer, viewer_ns));
    });

    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Some(event) = native_events.next().await {
            if weak
                .update(cx, |shell, cx| shell.native(event, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    cursor_guard(core, cx);
    // Windows come and go without telling us; recount once a second while
    // Peek or the Viewer shows the counts.
    let counted = shell.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if counted
                .update(cx, |shell, cx| shell.refresh_counts(cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    Ok(shell)
}

impl Shell {
    fn native(&mut self, event: Native, cx: &mut Context<Self>) {
        match event {
            Native::IconHover(over) => {
                self.over_icon = over;
                self.hover_changed(cx);
            }
            Native::IconClicked { right: true } => self.status_menu(),
            Native::IconClicked { right: false } => {
                if self.peek_shown && self.latched {
                    self.hide_peek(cx);
                } else {
                    self.latched = true;
                    self.show_peek(cx);
                }
            }
            Native::MouseDown { x, y } => {
                // A click anywhere else closes a latched Peek.
                let on_icon = self.icon.frame().is_some_and(|f| contains(f, (x, y)));
                let on_card = self
                    .peek
                    .as_ref()
                    .is_some_and(|(_, w)| contains(native::frame(w), (x, y)));
                if self.peek_shown && !on_icon && !on_card {
                    self.hide_peek(cx);
                }
            }
            Native::LeftDown { x, y } => self.drag_began((x, y)),
            Native::LeftDragged { x, y } => self.drag_moved((x, y), cx),
            Native::LeftUp { x, y } => self.drag_ended((x, y), cx),
            Native::Menu(action) => self.menu(action, cx),
            Native::Key {
                code,
                down,
                modifiers,
            } => {
                let Some(id) = self.viewer_display else {
                    return;
                };
                if down {
                    self.user_input(id, cx);
                }
                if let Some(forwarder) = self.forwarder() {
                    forwarder.key(code, down, modifiers);
                }
            }
            Native::FlagsChanged { code, modifiers } => {
                if let Some(forwarder) = self.forwarder() {
                    forwarder.flags_changed(code, modifiers);
                }
            }
        }
    }

    fn forwarder(&self) -> Option<Forwarder> {
        self.viewer_display.and(self.forwarder.clone())
    }

    // MARK: Following the displays

    /// The displays changed: refresh what shows them.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let ids: HashSet<DisplayId> = core.displays.list.iter().map(|d| d.id).collect();
        let asking = core
            .displays
            .list
            .iter()
            .any(|d| d.help_request.is_some() && !d.is_ended());
        if asking != self.asking {
            self.asking = asking;
            self.icon.set_asking(asking);
        }
        let gone: Vec<DisplayId> = self.known.difference(&ids).copied().collect();
        let added = ids.difference(&self.known).count() > 0;
        self.known = ids.clone();
        for id in &gone {
            self.streams.update(cx, |s, _| s.drop_display(*id));
            pointer::forget(*id);
        }
        if let Some(id) = self.viewer_display
            && !ids.contains(&id)
        {
            // The Viewer moves on to the next display, or collapses.
            self.viewer_display = None;
            self.forwarder = None;
            let next = peek::ordered(self.core.read(cx)).first().copied();
            match next {
                Some(next) if self.viewer_open => self.open_viewer(next, cx),
                _ => self.close_viewer(cx),
            }
        }
        if !gone.is_empty() || added {
            self.schedule_guard(cx);
            if self.peek_shown {
                self.show_peek(cx);
            }
        }
        self.refresh_counts(cx);
    }

    /// Window counts for the status lines.
    fn refresh_counts(&mut self, cx: &mut Context<Self>) {
        if !self.peek_shown && !self.viewer_open {
            return;
        }
        let counts: HashMap<DisplayId, usize> = self
            .known
            .iter()
            .map(|&id| (id, windows::on_display(id).len()))
            .collect();
        let viewer_count = self
            .viewer_display
            .and_then(|id| counts.get(&id).copied())
            .unwrap_or(0);
        if let Some((peek, _)) = &self.peek {
            let counts = counts.clone();
            let _ = peek.update(cx, |peek, _, cx| {
                if peek.counts != counts {
                    peek.counts = counts;
                    cx.notify();
                }
            });
        }
        if let Some((viewer, _)) = &self.viewer {
            let _ = viewer.update(cx, |viewer, _, cx| {
                if viewer.count != viewer_count {
                    viewer.count = viewer_count;
                    cx.notify();
                }
            });
        }
    }

    // MARK: Peek

    fn hover_changed(&mut self, cx: &mut Context<Self>) {
        self.hover_generation += 1;
        let generation = self.hover_generation;
        let over = self.over_icon || self.over_card;
        if over && !self.peek_shown {
            if self.settle_until.is_some_and(|t| t > Instant::now()) {
                return;
            }
            self.after(Duration::from_millis(120), cx, move |shell, cx| {
                if shell.hover_generation == generation && (shell.over_icon || shell.over_card) {
                    shell.show_peek(cx);
                }
            });
        } else if !over && self.peek_shown && !self.latched {
            self.after(Duration::from_millis(280), cx, move |shell, cx| {
                if shell.hover_generation == generation
                    && !shell.over_icon
                    && !shell.over_card
                    && !shell.latched
                {
                    shell.hide_peek(cx);
                }
            });
        }
    }

    pub fn card_hover(&mut self, over: bool, cx: &mut Context<Self>) {
        self.over_card = over;
        self.hover_changed(cx);
    }

    fn after(
        &self,
        delay: Duration,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Shell, &mut Context<Shell>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, f);
        })
        .detach();
    }

    /// The user's own screens: frame and visible frame. Our panels never
    /// go on an Agent Display, where the Viewer would mirror itself.
    fn user_screens(&self) -> Vec<(NSRect, NSRect)> {
        native::screens()
            .into_iter()
            .filter(|(id, _, _)| !self.known.contains(id))
            .map(|(_, frame, visible)| (frame, visible))
            .collect()
    }

    /// The icon's frame, or None when it is not visible. Menu bar managers
    /// such as Ice park hidden items far off-screen, the notch hides
    /// overflowing ones, and while an app on an Agent Display is frontmost,
    /// macOS draws the icon in that display's menu bar.
    fn icon_rect(&self) -> Option<NSRect> {
        let icon = self.icon.frame()?;
        self.user_screens()
            .iter()
            .any(|(frame, _)| intersects(*frame, icon))
            .then_some(icon)
    }

    /// Where the panels drop from: the icon, or the top center of the
    /// user's screen under the pointer.
    fn anchor(&self) -> NSRect {
        if let Some(icon) = self.icon_rect() {
            return icon;
        }
        let screens = self.user_screens();
        let pointer = native::mouse_location();
        let (frame, visible) = screens
            .iter()
            .find(|(frame, _)| contains(*frame, pointer))
            .or(screens.first())
            .copied()
            .unwrap_or((rect(0.0, 0.0, 1440.0, 900.0), rect(0.0, 0.0, 1440.0, 875.0)));
        rect(
            frame.origin.x + frame.size.width / 2.0 - 12.0,
            visible.origin.y + visible.size.height - 24.0,
            24.0,
            24.0,
        )
    }

    /// The user's screen showing the anchor: frame and visible frame.
    fn host_screen(&self) -> Option<(NSRect, NSRect)> {
        let anchor = self.anchor();
        let screens = self.user_screens();
        screens
            .iter()
            .find(|(frame, _)| intersects(*frame, anchor))
            .or(screens.first())
            .copied()
    }

    fn show_peek(&mut self, cx: &mut Context<Self>) {
        let Some((peek, window)) = self.peek.clone() else {
            return;
        };
        if !self.peek_shown {
            // Ended displays whose windows are all gone close before Peek
            // would show them.
            self.core.update(cx, |core, cx| core.sweep_ended(cx));
        }
        let ids = peek::ordered(self.core.read(cx));
        let (w, h) = Peek::size(ids.len());
        let Some((screen, _)) = self.host_screen() else {
            return;
        };
        let anchor = self.anchor();
        let (mid, top) = (
            anchor.origin.x + anchor.size.width / 2.0,
            anchor.origin.y - 6.0,
        );
        let x = (mid - w / 2.0).clamp(
            screen.origin.x + 8.0,
            screen.origin.x + screen.size.width - w - 8.0,
        );
        let frame = rect(x.round(), (top - h).round(), w, h);

        let active = self.viewer_open.then_some(self.viewer_display).flatten();
        let _ = peek.update(cx, |peek, _, cx| {
            peek.shown = true;
            peek.active = active;
            cx.notify();
        });
        for id in &ids {
            let size = self
                .core
                .read(cx)
                .displays
                .get(*id)
                .map_or(DEFAULT_SIZE, |d| (d.width, d.height));
            self.streams
                .update(cx, |s, cx| s.want(*id, "peek", 12, pixels(size), cx));
        }
        self.peek_generation += 1;
        if self.peek_shown {
            native::set_frame(&window, frame);
        } else {
            // It slides 10 pt down out of the menu bar and fades in.
            native::set_alpha(&window, 0.0);
            native::set_frame(&window, offset(frame, 10.0));
            native::order_front(&window);
            native::animate(&window, frame, 1.0, 0.24);
        }
        self.peek_shown = true;
        self.refresh_counts(cx);
    }

    fn hide_peek(&mut self, cx: &mut Context<Self>) {
        self.latched = false;
        if !self.peek_shown {
            return;
        }
        self.peek_shown = false;
        self.peek_generation += 1;
        let generation = self.peek_generation;
        let Some((_, window)) = self.peek.clone() else {
            return;
        };
        native::animate(&window, offset(native::frame(&window), 6.0), 0.0, 0.14);
        self.after(Duration::from_millis(160), cx, move |shell, cx| {
            if shell.peek_generation != generation {
                return;
            }
            native::order_out(&window);
            let ids: Vec<DisplayId> = shell.known.iter().copied().collect();
            for id in ids {
                shell
                    .streams
                    .update(cx, |s, cx| s.want(id, "peek", 0, (0, 0), cx));
            }
            if let Some((peek, _)) = &shell.peek {
                let _ = peek.update(cx, |peek, _, _| peek.shown = false);
            }
        });
    }

    /// A tile was clicked.
    pub fn select(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        self.hide_peek(cx);
        self.open_viewer(id, cx);
    }

    pub fn create_display(&mut self, cx: &mut Context<Self>) {
        let id = self
            .core
            .update(cx, |core, cx| core.create(DEFAULT_SIZE, None, cx));
        let Some(id) = id else {
            beep();
            return;
        };
        self.settle_until = Some(Instant::now() + Duration::from_millis(1200));
        self.hide_peek(cx);
        self.after(Duration::from_millis(500), cx, move |shell, cx| {
            shell.open_viewer(id, cx)
        });
    }

    // MARK: Viewer

    pub fn open_viewer(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        let Some((viewer, window)) = self.viewer.clone() else {
            return;
        };
        let Some(size) = self
            .core
            .read(cx)
            .displays
            .get(id)
            .map(|d| (d.width, d.height))
        else {
            return;
        };
        if self.viewer_open && self.viewer_display == Some(id) {
            native::order_front(&window);
            return;
        }
        if let Some(old) = self.viewer_display
            && old != id
        {
            self.streams
                .update(cx, |s, cx| s.want(old, "viewer", 0, (0, 0), cx));
        }
        let forwarder = self.core.update(cx, |core, _| core.forwarder(id));
        self.viewer_display = Some(id);
        self.forwarder = Some(forwarder.clone());
        let count = windows::on_display(id).len();
        let _ = viewer.update(cx, |viewer, _, cx| {
            if viewer.sheet.as_ref().is_some_and(|s| s.display != id) {
                viewer.sheet = None;
            }
            viewer.display = Some(id);
            viewer.forwarder = Some(forwarder);
            viewer.count = count;
            viewer.drop_target = false;
            cx.notify();
        });
        native::forward_keys(None);

        let Some((_, visible)) = self.host_screen() else {
            return;
        };
        let (w, h) = Viewer::card_size(size, (visible.size.width, visible.size.height));
        let frame = match self.resting {
            // Keep the top edge and center where the user left the card.
            Some(r) => rect(
                r.origin.x + r.size.width / 2.0 - w / 2.0,
                r.origin.y + r.size.height - h,
                w,
                h,
            ),
            None => rect(
                (visible.origin.x + (visible.size.width - w) / 2.0).round(),
                (visible.origin.y + (visible.size.height - h) / 2.0).round(),
                w,
                h,
            ),
        };
        self.viewer_generation += 1;
        if self.viewer_open {
            native::set_frame(&window, frame);
        } else {
            // Watching only: the keyboard stays with the user's app until
            // they click into the display.
            native::set_alpha(&window, 0.0);
            native::set_frame(&window, offset(frame, 8.0));
            native::order_front(&window);
            native::animate(&window, frame, 1.0, 0.2);
        }
        self.viewer_open = true;
        self.streams
            .update(cx, |s, cx| s.want(id, "viewer", 60, pixels(size), cx));
        if let Some((peek, _)) = &self.peek {
            let _ = peek.update(cx, |peek, _, cx| {
                peek.active = Some(id);
                cx.notify();
            });
        }
    }

    /// Collapse the Viewer. A Takeover stays: only 让 … 继续 hands the
    /// display back.
    pub fn close_viewer(&mut self, cx: &mut Context<Self>) {
        let Some((viewer, window)) = self.viewer.clone() else {
            return;
        };
        native::forward_keys(None);
        let _ = viewer.update(cx, |viewer, _, cx| {
            viewer.sheet = None;
            cx.notify();
        });
        if let Some((peek, _)) = &self.peek {
            let _ = peek.update(cx, |peek, _, cx| {
                peek.active = None;
                cx.notify();
            });
        }
        if !self.viewer_open {
            return;
        }
        self.viewer_open = false;
        self.resting = Some(native::frame(&window));
        if let Some(id) = self.viewer_display {
            self.streams
                .update(cx, |s, cx| s.want(id, "viewer", 0, (0, 0), cx));
        }
        self.viewer_generation += 1;
        let generation = self.viewer_generation;
        native::animate(&window, offset(native::frame(&window), 8.0), 0.0, 0.16);
        self.after(Duration::from_millis(180), cx, move |shell, _| {
            if shell.viewer_generation == generation {
                native::order_out(&window);
            }
        });
    }

    /// The screen got a click (`true`) or something else in the Viewer did.
    pub fn screen_keys(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some((viewer, window)) = self.viewer.clone() else {
            return;
        };
        let sheet = viewer.read(cx).is_ok_and(|v| v.sheet.is_some());
        if on && !sheet {
            native::make_key(&window);
            native::forward_keys(Some(&window));
        } else {
            native::forward_keys(None);
        }
    }

    /// The user's first input takes the display over from its agent.
    pub fn user_input(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| {
            if core.displays.begin_takeover(id) {
                cx.notify();
            }
        });
    }

    /// The header's main button: step in, hand back, or review.
    pub fn primary(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        let Some((ended, takeover)) = self
            .core
            .read(cx)
            .displays
            .get(id)
            .map(|d| (d.is_ended(), d.takeover))
        else {
            return;
        };
        if ended {
            self.dismiss(id, cx);
        } else if takeover {
            self.core.update(cx, |core, cx| {
                if core.displays.end_takeover(id) {
                    cx.notify();
                }
            });
            native::forward_keys(None);
        } else {
            self.user_input(id, cx);
            self.screen_keys(true, cx);
        }
    }

    /// Open the display that asked for help, ready to operate.
    pub fn open_for_help(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        self.hide_peek(cx);
        self.open_viewer(id, cx);
        self.user_input(id, cx);
    }

    /// Drive the UI from the control socket in debug builds, the way a
    /// click would.
    pub fn debug_show(
        &mut self,
        surface: &str,
        id: DisplayId,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        match surface {
            "peek" => self.show_peek(cx),
            "click" => self.native(Native::IconClicked { right: false }, cx),
            "menu" => self.status_menu(),
            "hide" => self.hide_peek(cx),
            "collapse" => self.close_viewer(cx),
            "create" => self.create_display(cx),
            "dismiss" => self.dismiss(id, cx),
            "primary" => self.primary(id, cx),
            "windows" => self.windows_menu(id, cx),
            "help" => self.open_for_help(id, cx),
            "answer" => {
                let Some((viewer, _)) = self.viewer.clone() else {
                    return Err("no Viewer".into());
                };
                let _ = viewer.update(cx, |viewer, _, cx| viewer.answer(id != 0, cx));
            }
            "focus" => self.screen_keys(true, cx),
            "viewer" => self.open_viewer(id, cx),
            other => return Err(format!("unknown surface {other}")),
        }
        Ok(())
    }

    // MARK: Menus

    fn status_menu(&self) {
        let menu = native::Menu::new("Sidescreen");
        let create = menu.action("新建显示器", MenuAction::CreateDisplay);
        native::set_key_equivalent(&create, "n", true, false, false, false);
        menu.separator();
        if !self.conflicts.is_empty() {
            for (workspace, display) in &self.conflicts {
                menu.disabled(&format!(
                    "AeroSpace 把工作区 {workspace} 固定到了 {display}"
                ));
            }
            let fix = menu.action("打开 AeroSpace 配置…", MenuAction::OpenAeroSpaceConfig);
            fix.setToolTip(Some(&objc2_foundation::NSString::from_str(
                "workspace-to-monitor-force-assignment 里的显示器编号会被虚拟屏占用。改用显示器名称，例如 et2 = 'DELL'。",
            )));
            menu.separator();
        }
        let screen = objc2_core_graphics::CGPreflightScreenCaptureAccess();
        let access = ax::is_trusted();
        if !screen || !access {
            let missing: Vec<&str> = [
                (!screen).then_some("屏幕录制"),
                (!access).then_some("辅助功能"),
            ]
            .into_iter()
            .flatten()
            .collect();
            menu.action(
                &format!("授予权限：{}", missing.join("、")),
                MenuAction::GrantPermissions,
            );
            menu.separator();
        }
        let quit = menu.action("退出 Sidescreen", MenuAction::Quit);
        native::set_key_equivalent(&quit, "q", true, false, false, false);
        self.icon.pop_up(menu.into_native());
    }

    /// 窗口 menu: hand one of the user's windows over, take one back, or
    /// close the display.
    pub fn windows_menu(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let Some(display) = core.displays.get(id) else {
            return;
        };
        let agent_screens: Vec<Rect> = core
            .displays
            .list
            .iter()
            .map(|d| cg::display_bounds(d.id))
            .collect();
        let menu = native::Menu::new("窗口");

        let hand = native::Menu::new("");
        for window in windows::all()
            .into_iter()
            .filter(|w| !agent_screens.iter().any(|s| s.intersects(&w.bounds)))
        {
            let item = hand.action(
                &name(&window.app, &window.title),
                MenuAction::HandOver {
                    display: id,
                    pid: window.target.pid,
                    window: window.target.window_id,
                },
            );
            native::set_app_icon(&item, window.target.pid);
        }
        if hand.is_empty() {
            hand.disabled("你的桌面上没有窗口");
        }
        let title = if display.owner.is_none() {
            "放到这块显示器上".to_string()
        } else {
            format!("交给 {}", display.agent_name())
        };
        menu.submenu(&title, hand).setEnabled(!display.is_ended());

        let back = native::Menu::new("");
        for window in windows::on_display(id) {
            let item = back.action(
                &name(&window.app, &window.title),
                MenuAction::TakeBack {
                    display: id,
                    pid: window.target.pid,
                    window: window.target.window_id,
                },
            );
            native::set_app_icon(&item, window.target.pid);
        }
        if back.is_empty() {
            back.disabled("这块显示器上没有窗口");
        }
        menu.submenu("取回窗口", back);
        menu.separator();
        menu.action("关闭显示器…", MenuAction::Dismiss { display: id });
        let (x, y) = native::mouse_location();
        menu.pop_up_at(x - 12.0, y - 14.0);
    }

    /// A click on the menu bar strip of the display image.
    pub fn menu_bar(&mut self, id: DisplayId, _cx: &mut Context<Self>) {
        match menubar::menu(id) {
            Some(menu) => {
                let (x, y) = native::mouse_location();
                menu.pop_up_at(x, y);
            }
            None => beep(),
        }
    }

    fn menu(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        match action {
            MenuAction::CreateDisplay => self.create_display(cx),
            MenuAction::GrantPermissions => {
                if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
                    objc2_core_graphics::CGRequestScreenCaptureAccess();
                }
                if !ax::is_trusted() {
                    ax::request_trust();
                }
            }
            MenuAction::OpenAeroSpaceConfig => {
                let home = std::env::var("HOME").unwrap_or_default();
                let config = [".config/aerospace/aerospace.toml", ".aerospace.toml"]
                    .iter()
                    .map(|p| std::path::Path::new(&home).join(p))
                    .find(|p| p.exists());
                if let Some(config) = config {
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg(config)
                        .spawn();
                }
            }
            MenuAction::Quit => self.quit(cx),
            MenuAction::HandOver {
                display,
                pid,
                window,
            } => self.hand_over(
                WindowTarget {
                    pid,
                    window_id: window,
                },
                None,
                display,
                cx,
            ),
            MenuAction::TakeBack {
                display,
                pid,
                window,
            } => self.take_back(
                WindowTarget {
                    pid,
                    window_id: window,
                },
                display,
                cx,
            ),
            MenuAction::Dismiss { display } => self.dismiss(display, cx),
            MenuAction::PressProxy { index } => menubar::press(index),
        }
    }

    // MARK: Dismissal

    /// Close a display: ask on the Viewer what happens to its windows,
    /// unless there is nothing to ask.
    pub fn dismiss(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let Some(display) = core.displays.get(id) else {
            return;
        };
        let items = dismissal::plan(display);
        if !dismissal::needs_confirmation(&items, display) {
            return self.dismiss_now(id, items, cx);
        }
        let sheet = Sheet {
            display: id,
            label: display.label(),
            agent: display.agent_name().to_string(),
            items,
            live: display.is_live(),
        };
        self.hide_peek(cx);
        self.open_viewer(id, cx);
        native::forward_keys(None);
        let Some((viewer, window)) = self.viewer.clone() else {
            return;
        };
        native::make_key(&window);
        let _ = viewer.update(cx, |viewer, window, cx| {
            viewer.sheet = Some(sheet);
            window.focus(&viewer.focus, cx);
            cx.notify();
        });
    }

    /// The sheet's answer: the adjusted plan, or None for cancel.
    pub fn answer_dismiss(&mut self, items: Option<Vec<Item>>, cx: &mut Context<Self>) {
        let Some((viewer, _)) = self.viewer.clone() else {
            return;
        };
        let sheet = viewer.update(cx, |viewer, _, cx| {
            cx.notify();
            viewer.sheet.take()
        });
        if let (Ok(Some(sheet)), Some(items)) = (sheet, items) {
            self.dismiss_now(sheet.display, items, cx);
        }
    }

    fn dismiss_now(&mut self, id: DisplayId, items: Vec<Item>, cx: &mut Context<Self>) {
        self.core
            .update(cx, |core, cx| core.end(id, EndReason::Finished, cx));
        let main_name = main_screen_name();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { dismissal::execute(&items, main_name.as_deref()) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.core.update(cx, |core, cx| core.remove(id, cx))
            });
        })
        .detach();
    }

    /// Return one window to the user: its old place for a Handover, the
    /// main display otherwise.
    fn take_back(&mut self, target: WindowTarget, id: DisplayId, cx: &mut Context<Self>) {
        let home = self.core.update(cx, |core, _| {
            let display = core.displays.get_mut(id)?;
            let home = display
                .records
                .get(&target.window_id)
                .and_then(|r| r.home.clone());
            display.forget(target.window_id);
            home
        });
        let main_name = main_screen_name();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { dismissal::give_back(target, home.as_ref(), main_name.as_deref()) })
                .await;
            let _ = this.update(cx, |shell, cx| shell.refresh_counts(cx));
        })
        .detach();
    }

    /// Quit: return or close every window first, as Dismissal would.
    fn quit(&mut self, cx: &mut Context<Self>) {
        let plans: Vec<Item> = {
            let core = self.core.read(cx);
            core.displays
                .list
                .iter()
                .flat_map(dismissal::plan)
                .collect()
        };
        let main_name = main_screen_name();
        cx.spawn(async move |_, cx| {
            if !plans.is_empty() {
                cx.background_executor()
                    .spawn(async move { dismissal::execute(&plans, main_name.as_deref()) })
                    .await;
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    // MARK: Handover

    /// Place one of the user's windows onto a display, remembering where
    /// it was so Dismissal can put it back.
    pub fn hand_over(
        &mut self,
        target: WindowTarget,
        frame: Option<Rect>,
        id: DisplayId,
        cx: &mut Context<Self>,
    ) {
        let mtm = MainThreadMarker::new().expect("main thread");
        let screen = app::screen_name(id, mtm);
        cx.spawn(async move |this, cx| {
            let home = cx
                .background_executor()
                .spawn(async move { dismissal::home_of(target, frame) })
                .await;
            let claimed = this
                .update(cx, |shell, cx| {
                    shell.core.update(cx, |core, cx| {
                        let display = core.displays.get_mut(id)?;
                        display.claim(target, Origin::HandedOver, home);
                        cx.notify();
                        Some(())
                    })
                })
                .ok()
                .flatten();
            if claimed.is_none() {
                return;
            }
            let placed = cx
                .background_executor()
                .spawn(async move { windows::place(target, id, screen.as_deref()) })
                .await;
            let _ = this.update(cx, |shell, cx| match placed {
                Ok(()) => shell.flash(cx),
                Err(error) => {
                    log::warn!("handover of window {} failed: {error}", target.window_id);
                    beep();
                }
            });
        })
        .detach();
    }

    /// A brief accent ring once a Handover landed.
    fn flash(&mut self, cx: &mut Context<Self>) {
        let Some((viewer, _)) = self.viewer.clone() else {
            return;
        };
        let _ = viewer.update(cx, |viewer, _, cx| {
            viewer.flash_until = Some(Instant::now() + Duration::from_millis(450));
            cx.notify();
        });
        self.after(Duration::from_millis(470), cx, move |shell, cx| {
            if let Some((viewer, _)) = &shell.viewer {
                let _ = viewer.update(cx, |_, _, cx| cx.notify());
            }
            shell.refresh_counts(cx);
        });
    }

    /// Where a dragged window can be dropped: the open card of a display
    /// that is not over. Cocoa coordinates.
    fn drop_rect(&self, cx: &App) -> Option<NSRect> {
        let id = self.viewer_display.filter(|_| self.viewer_open)?;
        let display = self.core.read(cx).displays.get(id)?;
        if display.is_ended() {
            return None;
        }
        self.viewer.as_ref().map(|(_, w)| native::frame(w))
    }

    // Title-bar drags belong to WindowServer and the dragged app, so the
    // watcher only observes: the window under the pointer at mouse-down is
    // the candidate, and it counts as dragged once its frame has moved.

    fn drag_began(&mut self, at: (f64, f64)) {
        self.drag = None;
        if !self.viewer_open {
            return;
        }
        let point = (at.0, native::primary_height() - at.1);
        let agent: Vec<Rect> = self
            .known
            .iter()
            .map(|&id| cg::display_bounds(id))
            .collect();
        if agent.iter().any(|r| r.contains(point)) {
            return;
        }
        let own = std::process::id() as i32;
        if let Some(hit) = windows::all()
            .into_iter()
            .find(|w| w.bounds.contains(point) && w.target.pid != own)
        {
            self.drag = Some(Drag {
                target: hit.target,
                frame: hit.bounds,
                last_check: Instant::now(),
            });
        }
    }

    fn dragging(&self) -> bool {
        let Some(drag) = &self.drag else { return false };
        windows::find(drag.target.window_id).is_some_and(|w| {
            (w.bounds.x - drag.frame.x).abs() > 4.0 || (w.bounds.y - drag.frame.y).abs() > 4.0
        })
    }

    fn drag_moved(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        if drag.last_check.elapsed() < Duration::from_millis(50) {
            return;
        }
        drag.last_check = Instant::now();
        let over = self.drop_rect(cx).is_some_and(|r| contains(r, at)) && self.dragging();
        self.set_drop_hover(over, cx);
    }

    fn drag_ended(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        let over = self.drop_rect(cx).is_some_and(|r| contains(r, at));
        let dropped = over && self.dragging();
        let drag = self.drag.take();
        self.set_drop_hover(false, cx);
        if let (true, Some(drag), Some(id)) = (dropped, drag, self.viewer_display) {
            self.hand_over(drag.target, Some(drag.frame), id, cx);
        }
    }

    fn set_drop_hover(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.drop_hover == on {
            return;
        }
        self.drop_hover = on;
        if let Some((viewer, _)) = &self.viewer {
            let _ = viewer.update(cx, |viewer, _, cx| {
                viewer.drop_target = on;
                cx.notify();
            });
        }
    }

    // MARK: AeroSpace guard

    /// Stock AeroSpace manages every display, virtual ones included, and may
    /// show one of the user's workspaces on a new Agent Display. After a
    /// display change, move such a workspace back to the main monitor and
    /// refocus the workspace the user was on. Workspaces force-assigned to a
    /// monitor number cannot move; those become conflicts the menu explains.
    fn schedule_guard(&mut self, cx: &mut Context<Self>) {
        self.guard_generation += 1;
        let generation = self.guard_generation;
        cx.spawn(async move |this, cx| {
            // AeroSpace reacts to display changes on its own schedule.
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let Ok(Some(checks)) = this.update(cx, |shell, cx| {
                (shell.guard_generation == generation).then(|| {
                    let mtm = MainThreadMarker::new().expect("main thread");
                    shell
                        .core
                        .read(cx)
                        .displays
                        .list
                        .iter()
                        .filter_map(|d| {
                            let name = app::screen_name(d.id, mtm)?;
                            Some((name, d.records.keys().copied().collect::<HashSet<u32>>()))
                        })
                        .collect::<Vec<_>>()
                })
            }) else {
                return;
            };
            let conflicts = cx
                .background_executor()
                .spawn(async move {
                    let Some(aerospace) = AeroSpace::current() else {
                        return Vec::new();
                    };
                    checks
                        .iter()
                        .filter_map(|(name, ours)| guard(&aerospace, name, ours))
                        .collect()
                })
                .await;
            let _ = this.update(cx, |shell, _| {
                for (workspace, display) in &conflicts {
                    log::warn!("AeroSpace pins workspace {workspace} to {display}");
                }
                shell.conflicts = conflicts;
            });
        })
        .detach();
    }
}

/// Check one Agent Display; a conflict when its workspace cannot move.
fn guard(aerospace: &AeroSpace, name: &str, ours: &HashSet<u32>) -> Option<(String, String)> {
    let monitor = aerospace.monitor_id(name).ok().flatten()?;
    // Each eviction makes AeroSpace pick another workspace; stop after a few.
    for _ in 0..3 {
        let workspace = aerospace.visible_workspace(&monitor).ok().flatten()?;
        let windows = aerospace.window_ids(&workspace).ok()?;
        if windows.iter().all(|w| ours.contains(w)) {
            return None;
        }
        let focused = aerospace.focused_workspace().ok().flatten();
        if aerospace.move_workspace(&workspace, "main").is_err() {
            return Some((workspace, name.to_string()));
        }
        if let Some(focused) = focused
            && focused != workspace
        {
            let _ = aerospace.focus_workspace(&focused);
        }
    }
    None
}

/// "Safari — Flights", or just the app when the title adds nothing.
fn name(app: &str, title: &str) -> String {
    if title.is_empty() || title == app {
        app.to_string()
    } else {
        format!("{app} — {title}")
    }
}

fn main_screen_name() -> Option<String> {
    app::screen_name(cg::main_display(), MainThreadMarker::new()?)
}

fn beep() {
    objc2_app_kit::NSBeep();
}

/// Keep the user's cursor off Agent Displays. They sit outside anything the
/// user can see, so a cursor there is lost. A window manager can warp it
/// there: AeroSpace's `move-mouse monitor-lazy-center` does when focus
/// moves to a window on an Agent Display.
fn cursor_guard(core: Entity<Core>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut last_good: Option<(f64, f64)> = None;
        loop {
            let ids: Vec<DisplayId> =
                cx.update(|cx| core.read(cx).displays.list.iter().map(|d| d.id).collect());
            let idle = ids.is_empty();
            cx.background_executor()
                .timer(Duration::from_millis(if idle { 500 } else { 33 }))
                .await;
            if idle {
                continue;
            }
            let Some(event) = objc2_core_graphics::CGEvent::new(None) else {
                continue;
            };
            let at = objc2_core_graphics::CGEvent::location(Some(&event));
            let at = (at.x, at.y);
            if !ids.iter().any(|&id| cg::display_bounds(id).contains(at)) {
                last_good = Some(at);
                continue;
            }
            let back = last_good.unwrap_or_else(|| cg::display_bounds(cg::main_display()).mid());
            log::debug!("cursor pulled back from {at:?} to {back:?}");
            objc2_core_graphics::CGWarpMouseCursorPosition(objc2_core_foundation::CGPoint::new(
                back.0, back.1,
            ));
            objc2_core_graphics::CGAssociateMouseAndMouseCursorPosition(true);
        }
    })
    .detach();
}
