//! The shell around the Agent Displays: the menu bar icon, Peek, the
//! Viewer, Dismissal and Handover, and the guards that keep the user's
//! cursor and AeroSpace workspaces off Agent Displays.

use super::peek::{self, Peek};
use super::pointer;
use super::setup::{self, Setup};
use super::stream::Streams;
use super::viewer::{self, Sheet, Viewer};
use crate::core::Core;
use crate::dismissal::{self, Item};
use crate::i18n::t;
use crate::mac::aerospace::{self, AeroSpace};
use crate::mac::native::{self, MenuAction, Native, StatusIcon};
use crate::mac::permissions::{self, Permission};
use crate::mac::{app, assist, cg, menubar, notify, viewer_input, windows};
use crate::model::{DEFAULT_SIZE, DisplayId, EndReason, Origin, Rect, WindowTarget};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{
    AnyView, App, AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, ParentElement,
    Render, SharedString, Styled, TitlebarOptions, WeakEntity, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point, px, size,
};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::cell::Cell;
use std::collections::HashSet;
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
    /// Windows already went back before quitting.
    static TIDIED: Cell<bool> = const { Cell::new(false) };
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
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The same dark bubble in light and dark mode, like a macOS tooltip
        // over media.
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .max_w(px(360.))
            .bg(gpui::black().opacity(0.85))
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(gpui::white())
            .child(self.0.clone())
    }
}

/// How long the pointer rests on a control before its hint shows.
pub const HINT_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// Hints over thumbnails wait longer, so scanning the tiles shows none.
pub const TILE_HINT_DELAY: std::time::Duration = std::time::Duration::from_millis(800);

pub fn hint(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
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
    /// When the Viewer's open animation lands.
    viewer_settled_at: Instant,
    /// Displays known to have ended, to notice one ending.
    ended: HashSet<DisplayId>,
    viewer_open: bool,
    viewer_generation: u64,
    /// Where the card rests, so it comes back where the user dragged it.
    resting: Option<NSRect>,

    known: HashSet<DisplayId>,
    /// Tests: an Agent Display that stands in for the user's screen, so
    /// panels open where the user can't see them.
    stage: Option<DisplayId>,
    drag: Option<Drag>,
    /// Where the left button went down, until the first drag event.
    drag_from: Option<(f64, f64)>,
    drop_hover: bool,
    /// AeroSpace workspaces pinned to a monitor number an Agent Display took.
    conflicts: Vec<(String, String)>,
    /// Whether AeroSpace leaves Agent Displays alone; None without AeroSpace.
    aerospace: Option<aerospace::Ignoring>,
    guard_generation: u64,
    asking: bool,
    /// Screen Recording and Accessibility, as last checked.
    permissions: permissions::Status,
    setup: Option<WindowHandle<Setup>>,
    /// The agents whose MCP servers loaded the tools, first to last.
    agent_ready: Vec<String>,
    /// The permission whose pane the user was sent to.
    waiting: Option<Permission>,
    /// Screen Recording came on while running and capture still fails.
    restart: bool,
    /// A Connect is running.
    connecting: bool,
    /// A late check of Screen Recording is pending.
    rechecking: bool,
    /// The missing permissions and reopen state an agent's call last
    /// opened the setup window for.
    setup_shown_for: Option<(Vec<Permission>, bool)>,
    /// Clients whose config runs this copy of Sidescreen, read when the
    /// setup window opens and after a Connect: not on every tick.
    registered: Vec<&'static str>,
}

/// Pixel size a thumbnail needs: twice its width in points, for Retina.
fn thumbnail_pixels(display: (f64, f64)) -> (u32, u32) {
    let width = peek::TILE_WIDTH as f64 * 2.0;
    (width as u32, (width * display.1 / display.0) as u32)
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

/// `card` scaled down to fit inside `target`, centered on it: where a
/// morph starts or ends.
fn fit_into(card: NSRect, target: NSRect) -> NSRect {
    let scale = (target.size.width / card.size.width).min(target.size.height / card.size.height);
    let (w, h) = (card.size.width * scale, card.size.height * scale);
    rect(
        target.origin.x + (target.size.width - w) / 2.0,
        target.origin.y + (target.size.height - h) / 2.0,
        w,
        h,
    )
}

/// `card` moved into `screen` as far as it fits. A card taller than the
/// screen keeps its header, at the top, in view.
fn keep_inside(card: NSRect, screen: NSRect) -> NSRect {
    let (w, h) = (card.size.width, card.size.height);
    let right = screen.origin.x + screen.size.width;
    let top = screen.origin.y + screen.size.height;
    let x = card.origin.x.min(right - w).max(screen.origin.x);
    let y = card.origin.y.max(screen.origin.y).min(top - h);
    rect(x, y, w, h)
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
    native::watch_screens();
    native::watch_colors();
    let accent = native::accent();
    super::theme::set_accent(accent);
    viewer_input::install();
    notify::install();
    native::debug_appearance();

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
            viewer_settled_at: Instant::now(),
            ended: HashSet::new(),
            viewer_open: false,
            viewer_generation: 0,
            resting: None,
            known: HashSet::new(),
            stage: None,
            drag: None,
            drag_from: None,
            drop_hover: false,
            conflicts: Vec::new(),
            aerospace: None,
            guard_generation: 0,
            asking: false,
            permissions: permissions::status(),
            setup: None,
            agent_ready: Vec::new(),
            waiting: None,
            restart: false,
            setup_shown_for: None,
            connecting: false,
            rechecking: false,
            registered: Vec::new(),
        }
    });
    let weak = shell.downgrade();
    CURRENT.with(|c| *c.borrow_mut() = Some(shell.clone()));

    let peek = {
        let (weak, core, streams) = (weak.clone(), core.clone(), streams.clone());
        cx.open_window(panel_options(), move |window, cx| {
            cx.new(|cx| Peek::new(weak, core, streams, window, cx))
        })?
    };
    let viewer = {
        let (weak, core, streams) = (weak.clone(), core.clone(), streams.clone());
        cx.open_window(panel_options(), move |window, cx| {
            cx.new(|cx| Viewer::new(weak, core, streams, window, cx))
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
    // Peek never takes the keyboard from the user's app; neither panel
    // becomes the main window.
    native::set_keyable(&peek_ns, false);
    native::set_keyable(&viewer_ns, true);
    viewer_input::attach(&viewer_ns);
    shell.update(cx, |shell, cx| {
        shell.peek = Some((peek, peek_ns));
        shell.viewer = Some((viewer, viewer_ns));
        shell.check_aerospace(cx);
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
    // A normal quit, the menu's or a logout's, tidies up as Dismissal
    // would, without asking: each window follows its Disposition. A crash
    // leaves the windows to macOS.
    {
        // Before the app exits, every display's windows go where Dismissal
        // sends them, as if each display were closed. It runs on its own
        // thread: the AeroSpace CLI it calls queries this app over AX.
        let (core, app) = (core.clone(), cx.to_async());
        native::intercept_terminate(mtm, move || {
            let plans: Vec<Item> = app.update(|cx| {
                core.read(cx)
                    .displays
                    .list
                    .iter()
                    .flat_map(dismissal::plan)
                    .collect()
            });
            TIDIED.with(|t| t.set(true));
            if plans.is_empty() {
                return false;
            }
            let main = main_screen_name();
            std::thread::spawn(move || {
                dismissal::execute(&plans, main.as_deref());
                native::reply_terminate();
            });
            true
        });
    }
    {
        let core = core.clone();
        // A fallback for a quit that skipped applicationShouldTerminate.
        cx.on_app_quit(move |cx| {
            let plans: Vec<Item> = core
                .read(cx)
                .displays
                .list
                .iter()
                .flat_map(dismissal::plan)
                .collect();
            if !plans.is_empty() && !TIDIED.with(Cell::get) {
                dismissal::execute(&plans, main_screen_name().as_deref());
            }
            async {}
        })
        .detach();
    }
    cursor_guard(core, cx);
    // Permissions: the icon shows what's missing, and the setup window
    // opens by itself at launch. Checked often while the user is granting,
    // rarely after, to notice one being turned off.
    shell.update(cx, |shell, cx| {
        let status = shell.permissions;
        shell.icon.set_needs_setup(!status.complete());
        // Reopened from the setup window: finish setup with the agents.
        let reopened = std::env::args().any(|a| a == "--setup");
        if !status.complete() || reopened {
            shell.open_setup(cx);
        }
    });
    let checked = shell.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            let fast = checked
                .update(cx, |shell, _| shell.setup.is_some() || assist::is_shown())
                .unwrap_or(false);
            let wait = if fast { 400 } else { 2000 };
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            if checked
                .update(cx, |shell, cx| shell.check_permissions(cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    // Windows come and go without telling us; recount once a second while
    // Peek or the Viewer shows the counts.
    let counted = shell.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if counted
                .update(cx, |shell, cx| {
                    shell.refresh_counts(cx);
                    // A Review Display closes once its last window goes,
                    // whenever that happens.
                    if shell
                        .core
                        .read(cx)
                        .displays
                        .list
                        .iter()
                        .any(|d| d.is_ended())
                    {
                        shell.core.update(cx, |core, cx| core.sweep_ended(cx));
                    }
                    // The front app on the display can change any time.
                    shell.refresh_menu_bar(cx);
                    if shell.peek_shown || shell.viewer_open {
                        shell.streams.update(cx, |s, cx| s.refresh_filters(cx));
                    }
                })
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
                // Before the menu opens; AeroSpace may have changed since.
                if over && !self.over_icon {
                    self.check_aerospace(cx);
                }
                self.over_icon = over;
                self.hover_changed(cx);
            }
            Native::IconClicked { right: true } => {
                self.hide_peek(cx);
                self.status_menu();
            }
            Native::IconClicked { right: false } if !self.permissions.complete() => {
                self.open_setup(cx);
            }
            Native::IconClicked { right: false } => {
                if self.peek_shown && self.latched {
                    self.hide_peek(cx);
                } else {
                    self.latched = true;
                    self.show_peek(cx);
                    // Opened on purpose: the keyboard can drive it. Peek is a
                    // non-activating panel, so the user's app stays active.
                    if let Some((peek, window)) = self.peek.clone() {
                        native::make_key(&window);
                        let _ = peek.update(cx, |peek, window, cx| {
                            peek.focused = None;
                            peek.keys = true;
                            window.focus(&peek.focus, cx);
                            cx.notify();
                        });
                    }
                }
            }
            Native::MouseDown { x, y } => {
                // A click anywhere else unlatches Peek; hover decides the rest.
                let on_icon = self.icon.frame().is_some_and(|f| contains(f, (x, y)));
                let on_card = self
                    .peek
                    .as_ref()
                    .is_some_and(|(_, w)| contains(native::frame(w), (x, y)));
                if self.latched && !on_icon && !on_card {
                    self.latched = false;
                    if self.over_icon || self.over_card {
                        self.hover_changed(cx);
                    } else {
                        self.hide_peek(cx);
                    }
                }
            }
            Native::LeftDown { x, y } => self.drag_began((x, y)),
            Native::LeftDragged { x, y } => self.drag_moved((x, y), cx),
            Native::LeftUp { x, y } => self.drag_ended((x, y), cx),
            Native::Menu(action) => self.menu(action, cx),
            Native::HelpClicked { display } => self.open_for_help(display, cx),
            Native::ScreenTouched { display } => self.user_input(display, cx),
            Native::MenuBarClick { display, x } => self.menu_bar(display, x, cx),
            Native::ScreensChanged => self.schedule_guard(cx),
            Native::ColorsChanged => {
                super::theme::set_accent(native::accent());
                self.repaint(cx);
            }
        }
    }

    // MARK: Following the displays

    /// The displays changed: refresh what shows them.
    /// Redraw both panels, as after a system color change.
    fn repaint(&self, cx: &mut Context<Self>) {
        if let Some((peek, _)) = &self.peek {
            let _ = peek.update(cx, |_, _, cx| cx.notify());
        }
        if let Some((viewer, _)) = &self.viewer {
            let _ = viewer.update(cx, |_, _, cx| cx.notify());
        }
    }

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
        for display in &core.displays.list {
            let reason = display
                .help_request
                .as_deref()
                .filter(|_| display.is_live());
            notify::update(display.id, &display.label(), reason);
        }
        let gone: Vec<DisplayId> = self.known.difference(&ids).copied().collect();
        for id in &gone {
            notify::update(*id, "", None);
        }
        let new: Vec<(DisplayId, (f64, f64))> = core
            .displays
            .list
            .iter()
            .filter(|d| !self.known.contains(&d.id))
            .map(|d| (d.id, (d.width, d.height)))
            .collect();
        let added = !new.is_empty();
        let ended: HashSet<DisplayId> = core
            .displays
            .list
            .iter()
            .filter(|d| d.is_ended())
            .map(|d| d.id)
            .collect();
        // A display that ends is a display change too for AeroSpace.
        let newly_ended = ended.difference(&self.ended).count() > 0;
        self.ended = ended;
        self.known = ids.clone();
        // Every display keeps a 1 fps thumbnail stream, so Peek and the
        // Viewer open on a frame at most a second old.
        for (id, size) in new {
            self.streams.update(cx, |s, cx| {
                s.want(id, "warm", 1, thumbnail_pixels(size), cx)
            });
        }
        for id in &gone {
            self.streams.update(cx, |s, cx| s.drop_display(*id, cx));
            pointer::forget(*id);
        }
        if let Some(id) = self.viewer_display
            && !ids.contains(&id)
        {
            // The Viewer moves on to the next display, or collapses.
            self.viewer_display = None;
            let next = peek::ordered(self.core.read(cx)).first().copied();
            match next {
                Some(next) if self.viewer_open => self.open_viewer(next, cx),
                _ => self.close_viewer(cx),
            }
        }
        if newly_ended {
            self.schedule_guard(cx);
        }
        if !gone.is_empty() || added {
            self.schedule_guard(cx);
            // Screens just changed under the pointer; let them settle.
            self.settle_hover(cx);
            if self.peek_shown {
                self.show_peek(cx);
            }
            // The once-a-second recount covers windows moving; agent
            // clicks and moves notify far more often.
            self.refresh_counts(cx);
        }
    }

    /// Window counts for the status lines.
    fn refresh_counts(&mut self, cx: &mut Context<Self>) {
        if !self.peek_shown && !self.viewer_open {
            return;
        }
        let ids: Vec<DisplayId> = self.known.iter().copied().collect();
        let counts = windows::counts(&ids);
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
        // Without permissions Peek has nothing to show; a click opens setup.
        if !self.permissions.complete() && !self.peek_shown {
            return;
        }
        if self.latched || self.settle_until.is_some_and(|t| t > Instant::now()) {
            return;
        }
        let over = self.over_icon || self.over_card;
        if over && !self.peek_shown {
            self.after(Duration::from_millis(120), cx, move |shell, cx| {
                let still =
                    shell.hover_generation == generation && (shell.over_icon || shell.over_card);
                // Hover only means something on a visible icon.
                if still && shell.icon_rect().is_some() {
                    shell.show_peek(cx);
                }
            });
        } else if !over && self.peek_shown {
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

    /// Displays appearing or going move screens under the pointer. Ignore
    /// hover for a moment, then follow the pointer again.
    fn settle_hover(&mut self, cx: &mut Context<Self>) {
        self.settle_until = Some(Instant::now() + Duration::from_millis(1200));
        self.after(Duration::from_millis(1250), cx, |shell, cx| {
            shell.hover_changed(cx)
        });
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
            .filter(|(id, _, _)| match self.stage {
                Some(stage) => *id == stage,
                None => !self.known.contains(id),
            })
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
        let Some((screen, _)) = self.host_screen() else {
            return;
        };
        let (w, h, tile) = Peek::size(ids.len(), screen.size.width - 16.0);
        let anchor = self.anchor();
        let (mid, top) = (
            anchor.origin.x + anchor.size.width / 2.0,
            anchor.origin.y - 6.0,
        );
        // Centered under the anchor, inside the screen; a card wider than
        // the screen starts at its left edge.
        let x = (mid - w / 2.0)
            .min(screen.origin.x + screen.size.width - w - 8.0)
            .max(screen.origin.x + 8.0);
        let frame = rect(x.round(), (top - h).round(), w, h);

        let active = self.viewer_open.then_some(self.viewer_display).flatten();
        let _ = peek.update(cx, |peek, _, cx| {
            peek.shown = true;
            peek.active = active;
            peek.tile = tile;
            cx.notify();
        });
        for id in &ids {
            let size = self
                .core
                .read(cx)
                .displays
                .get(*id)
                .map_or(DEFAULT_SIZE, |d| (d.width, d.height));
            self.streams.update(cx, |s, cx| {
                s.want(*id, "peek", 12, thumbnail_pixels(size), cx)
            });
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

    pub fn hide_peek(&mut self, cx: &mut Context<Self>) {
        if let Some((peek, _)) = &self.peek {
            let _ = peek.update(cx, |peek, _, _| {
                peek.keys = false;
                peek.focused = None;
            });
        }
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
        // Leaving: a slow start, unlike the arrival's soft landing.
        native::animate_frame(
            &window,
            offset(native::frame(&window), 6.0),
            0.14,
            native::Curve::EaseIn,
        );
        native::animate_alpha(&window, 0.0, 0.14, native::Curve::EaseIn);
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
        let from = self.tile_rect(id, cx);
        self.hide_peek(cx);
        self.open_viewer_from(id, from, cx);
    }

    /// The thumbnail of a display in Peek, in Cocoa coordinates.
    fn tile_rect(&self, id: DisplayId, cx: &App) -> Option<NSRect> {
        let (_, window) = self.peek.as_ref().filter(|_| self.peek_shown)?;
        let index = peek::ordered(self.core.read(cx))
            .iter()
            .position(|&d| d == id)?;
        let card = native::frame(window);
        let tile = self.peek.as_ref()?.0.read(cx).ok()?.tile;
        let thumb = peek::thumb_height(tile) as f64;
        let step = (tile + peek::SPACING) as f64;
        let top = card.origin.y + card.size.height - peek::PADDING as f64;
        Some(rect(
            card.origin.x + peek::PADDING as f64 + index as f64 * step,
            top - thumb,
            tile as f64,
            thumb,
        ))
    }

    pub fn create_display(&mut self, cx: &mut Context<Self>) {
        let id = self
            .core
            .update(cx, |core, cx| core.create(DEFAULT_SIZE, None, cx));
        let Some(id) = id else {
            beep();
            return;
        };
        // Peek stays up and grows a tile; the Viewer opens on the new
        // display, or switches to it when already open.
        self.after(Duration::from_millis(500), cx, move |shell, cx| {
            shell.open_viewer(id, cx)
        });
    }

    // MARK: Viewer

    pub fn open_viewer(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        self.open_viewer_from(id, None, cx);
    }

    /// Open the Viewer on a display. It grows out of `from`, or out of the
    /// menu bar icon.
    pub fn open_viewer_from(
        &mut self,
        id: DisplayId,
        from: Option<NSRect>,
        cx: &mut Context<Self>,
    ) {
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
        let count = windows::on_display(id).len();
        let _ = viewer.update(cx, |viewer, _, cx| {
            if viewer.sheet.as_ref().is_some_and(|s| s.display != id) {
                viewer.sheet = None;
            }
            if viewer.display != Some(id) {
                viewer.switches += 1;
                viewer.menu_bar = None;
            }
            viewer.display = Some(id);
            viewer.count = count;
            viewer.drop_target = false;
            cx.notify();
        });
        viewer_input::show(Some((id, forwarder)));

        let Some((_, visible)) = self.host_screen() else {
            return;
        };
        let (w, h) = Viewer::card_size(size, (visible.size.width, visible.size.height));
        if self.viewer_open {
            // Switching displays: the card stays where the user dragged it.
            self.remember_resting(&window);
        }
        // Keep the top edge and center where the user left the card, as
        // long as a screen of theirs is still there.
        let resting = self.resting.and_then(|r| {
            let card = rect(
                r.origin.x + r.size.width / 2.0 - w / 2.0,
                r.origin.y + r.size.height - h,
                w,
                h,
            );
            let center = (
                r.origin.x + r.size.width / 2.0,
                r.origin.y + r.size.height / 2.0,
            );
            self.user_screens()
                .into_iter()
                .find(|(frame, _)| contains(*frame, center))
                .map(|(_, visible)| keep_inside(card, visible))
        });
        let frame = match resting {
            Some(frame) => frame,
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
            // A switch can cut an open morph short, whose timer then bails.
            self.set_morphing(false, cx);
        } else {
            // Watching only: the keyboard stays with the user's app until
            // they click into the display.
            let from = from.unwrap_or_else(|| self.anchor());
            log::debug!(
                "viewer opens at {frame:?} from {from:?}, resting {:?}",
                self.resting
            );
            native::set_alpha(&window, 0.0);
            native::set_frame(&window, fit_into(frame, from));
            native::order_front(&window);
            native::animate_frame(&window, frame, 0.45, native::Curve::Spring);
            native::animate_alpha(&window, 1.0, 0.16, native::Curve::Settle);
            self.set_morphing(true, cx);
            let generation = self.viewer_generation;
            self.after(Duration::from_millis(450), cx, move |shell, cx| {
                if shell.viewer_generation == generation {
                    shell.set_morphing(false, cx);
                }
            });
        }
        self.viewer_open = true;
        // Until the open animation lands, the window's frame is in flight.
        self.resting = Some(frame);
        self.viewer_settled_at = Instant::now() + Duration::from_millis(460);
        self.streams
            .update(cx, |s, cx| s.want(id, "viewer", 60, pixels(size), cx));
        self.refresh_menu_bar(cx);
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
        self.remember_resting(&window);
        self.viewer_open = false;
        viewer_input::show(None);
        if let Some(id) = self.viewer_display {
            self.streams
                .update(cx, |s, cx| s.want(id, "viewer", 0, (0, 0), cx));
        }
        self.viewer_generation += 1;
        let generation = self.viewer_generation;
        // It shrinks back into the menu bar icon, fading on the way.
        // The fade waits 140 ms, so the card is seen shrinking first.
        let into = fit_into(native::frame(&window), self.anchor());
        self.set_morphing(true, cx);
        native::animate_frame(&window, into, 0.34, native::Curve::Settle);
        let fading = window.clone();
        self.after(Duration::from_millis(140), cx, move |shell, _| {
            if shell.viewer_generation == generation {
                native::animate_alpha(&fading, 0.0, 0.16, native::Curve::EaseIn);
            }
        });
        self.after(Duration::from_millis(320), cx, move |shell, _| {
            if shell.viewer_generation == generation {
                native::order_out(&window);
            }
        });
    }

    fn set_morphing(&self, morphing: bool, cx: &mut Context<Self>) {
        if let Some((viewer, _)) = &self.viewer {
            let _ = viewer.update(cx, |viewer, _, cx| {
                if viewer.morphing != morphing {
                    viewer.morphing = morphing;
                    if !morphing {
                        viewer.landed += 1;
                    }
                    cx.notify();
                }
            });
        }
    }

    /// Where the card rests: the live frame once the open animation has
    /// landed, which follows the user's drags. Mid-animation the frame is
    /// in flight, so the target stays.
    fn remember_resting(&mut self, window: &NSWindow) {
        if self.viewer_open && Instant::now() >= self.viewer_settled_at {
            self.resting = Some(native::frame(window));
        }
    }

    /// Give the Viewer the keyboard, as a click on the display would.
    fn take_keyboard(&self) {
        if let Some((_, window)) = &self.viewer {
            native::make_key(window);
        }
    }

    /// The user's first input takes the display over from its agent.
    pub fn user_input(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| {
            if core.displays.begin_takeover(id) {
                core.stop_agent_input(id, true);
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
                    core.stop_agent_input(id, false);
                    cx.notify();
                }
            });
        } else {
            self.user_input(id, cx);
            self.take_keyboard();
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
            "setup" => self.open_setup(cx),
            "setup-close" => self.close_setup(cx),
            // The pane and its helper, without the system prompt.
            "assist-screen" | "assist-ax" => {
                let p = if surface == "assist-screen" {
                    Permission::ScreenRecording
                } else {
                    Permission::Accessibility
                };
                permissions::open_pane(p);
                assist::show(p, permissions::listed(p).unwrap_or(true));
                self.waiting = Some(p);
                self.push_setup(cx);
            }
            // The helper alone at the top-left corner of the main screen.
            "assist-preview" | "assist-preview-unlisted" => assist::preview(
                Permission::Accessibility,
                surface == "assist-preview",
                40.0,
                60.0,
            ),
            "assist-hide" => assist::hide(),
            "connect" => self.menu(MenuAction::ConnectAgents, cx),
            "click" => self.native(Native::IconClicked { right: false }, cx),
            "menu" => self.status_menu(),
            "hide" => self.hide_peek(cx),
            "collapse" => self.close_viewer(cx),
            "create" => self.create_display(cx),
            "dismiss" => self.dismiss(id, cx),
            "primary" => self.primary(id, cx),
            "windows" => self.windows_menu(id, None, cx),
            "help" => self.open_for_help(id, cx),
            "answer" => {
                let Some((viewer, _)) = self.viewer.clone() else {
                    return Err("no Viewer".into());
                };
                let _ = viewer.update(cx, |viewer, _, cx| viewer.answer(id != 0, cx));
            }
            "focus" => self.take_keyboard(),
            "viewer" => self.open_viewer(id, cx),
            "stage" => self.stage = (id != 0).then_some(id),
            "select" => self.select(id, cx),
            "quit" => self.quit(cx),
            other => return Err(format!("unknown surface {other}")),
        }
        Ok(())
    }

    // MARK: Menus

    fn status_menu(&self) {
        let menu = native::Menu::new("Sidescreen");
        let missing = self.permissions.missing();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|p| p.name()).collect();
            menu.action(
                &tf!(
                    "Finish Setup: Allow {}…",
                    "完成设置：还差{}…",
                    names.join(t(" and ", "、"))
                ),
                MenuAction::GrantPermissions,
            );
            menu.separator();
        } else if self.restart {
            menu.action(
                t(
                    "Finish Setup: Reopen Sidescreen…",
                    "完成设置：重新打开 Sidescreen…",
                ),
                MenuAction::GrantPermissions,
            );
            menu.separator();
        }
        // Setup worked on the agent's side too: its session has the tools.
        match self.agents_ready() {
            Some(agent) => menu.disabled(&tf!(
                "{agent} loaded Sidescreen's tools",
                "{agent} 已加载 Sidescreen 的工具"
            )),
            None => menu.disabled(t(
                "No agent has loaded the tools yet",
                "还没有 agent 加载工具",
            )),
        };
        menu.separator();
        let create = menu.action(t("New Display", "新建显示器"), MenuAction::CreateDisplay);
        native::set_key_equivalent(&create, "n", true, false, false, false);
        menu.separator();
        if !self.conflicts.is_empty() {
            for (workspace, display) in &self.conflicts {
                menu.disabled(&tf!(
                    "AeroSpace pins workspace {workspace} to {display}",
                    "AeroSpace 把工作区 {workspace} 固定到了 {display}"
                ));
            }
            let fix = menu.action(
                t("Open AeroSpace Config…", "打开 AeroSpace 配置…"),
                MenuAction::OpenAeroSpaceConfig,
            );
            fix.setToolTip(Some(&objc2_foundation::NSString::from_str(t(
                "Agent displays take monitor numbers in workspace-to-monitor-force-assignment. Use monitor names instead, like 2 = 'DELL'.",
                "Agent 显示器会占用 workspace-to-monitor-force-assignment 里的显示器编号。改用显示器名称，例如 2 = 'DELL'。",
            ))));
            menu.separator();
        }
        if matches!(
            self.aerospace,
            Some(aerospace::Ignoring::Available | aerospace::Ignoring::Unsupported)
        ) {
            let item = menu.action(
                t(
                    "Keep AeroSpace Off Sidescreen's Displays…",
                    "让 AeroSpace 不管 Sidescreen 的显示器…",
                ),
                MenuAction::IgnoreInAeroSpace,
            );
            item.setToolTip(Some(&objc2_foundation::NSString::from_str(t(
                "AeroSpace shows your workspaces on these displays, and agents' new windows pass over your screen.",
                "AeroSpace 会把你的工作区放到这些显示器上，agent 打开的新窗口也会先在你屏幕上闪一下。",
            ))));
            menu.separator();
        }
        menu.action(
            t("Connect Claude Code and Codex", "接入 Claude Code 和 Codex"),
            MenuAction::ConnectAgents,
        );
        menu.separator();
        let quit = menu.action(t("Quit Sidescreen", "退出 Sidescreen"), MenuAction::Quit);
        native::set_key_equivalent(&quit, "q", true, false, false, false);
        self.icon.pop_up(menu.into_native());
    }

    /// 窗口 menu: hand one of the user's windows over, take one back, or
    /// close the display.
    /// `button` is where the 窗口 button is in the Viewer; the menu drops
    /// from its bottom-left corner, or from the pointer without one.
    pub fn windows_menu(
        &mut self,
        id: DisplayId,
        button: Option<gpui::Bounds<gpui::Pixels>>,
        cx: &mut Context<Self>,
    ) {
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
        let menu = native::Menu::new(t("Windows", "窗口"));

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
            hand.disabled(t("No Windows on Your Desktop", "你的桌面上没有窗口"));
        }
        let title = if display.owner.is_none() {
            t("Move to This Display", "放到这块显示器上").to_string()
        } else {
            tf!("Hand Over to {}", "交给 {}", display.agent_name())
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
            back.disabled(t("No Windows on This Display", "这块显示器上没有窗口"));
        }
        menu.submenu(t("Take Back Window", "取回窗口"), back);
        menu.separator();
        menu.action(
            t("Close Display…", "关闭显示器…"),
            MenuAction::Dismiss { display: id },
        );
        let at = button.zip(self.viewer.as_ref()).map(|(b, (_, window))| {
            let frame = native::frame(window);
            (
                frame.origin.x + f64::from(f32::from(b.origin.x)),
                frame.origin.y + frame.size.height
                    - f64::from(f32::from(b.origin.y + b.size.height))
                    - 4.0,
            )
        });
        let (x, y) = at.unwrap_or_else(|| {
            let (x, y) = native::mouse_location();
            (x - 12.0, y - 14.0)
        });
        menu.pop_up_at(x, y);
    }

    /// A click on the Viewer's menu bar: open the menu under that title,
    /// right below it, as the real menu bar would.
    pub fn menu_bar(&mut self, id: DisplayId, x: f64, cx: &mut Context<Self>) {
        let Some((viewer, window)) = self.viewer.clone() else {
            return;
        };
        let hit = viewer
            .read(cx)
            .ok()
            .and_then(|v| Some((v.menu_bar.clone()?, viewer::title_at(x)?)))
            .filter(|_| self.viewer_display == Some(id));
        // A click beside the titles opens nothing; the strip flashes red so
        // it doesn't look broken.
        let Some((bar, (index, bounds))) = hit else {
            if self.viewer_display == Some(id) {
                let _ = viewer.update(cx, |viewer, _, cx| {
                    viewer.refused += 1;
                    cx.notify();
                });
            }
            return;
        };
        let frame = native::frame(&window);
        let at = (
            frame.origin.x + bounds[0],
            frame.origin.y + frame.size.height - (bounds[1] + bounds[3]),
        );
        // Choosing from the agent app's menu is the user operating it.
        self.user_input(id, cx);
        let title = bar.titles.get(index).cloned().unwrap_or_default();
        let pid = bar.pid;
        cx.spawn(async move |_, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { menubar::items(pid, index) })
                .await;
            if let Some(items) = items {
                menubar::build_items(&title, items).pop_up_at(at.0, at.1);
            }
        })
        .detach();
    }

    /// Read the menu bar of the front app on the Viewer's display, so the
    /// Viewer can show it in place of the user's own.
    fn refresh_menu_bar(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.viewer_display.filter(|_| self.viewer_open) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let bar = cx
                .background_executor()
                .spawn(async move { menubar::bar(id) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.viewer_display != Some(id) {
                    return;
                }
                if let Some((viewer, _)) = &shell.viewer {
                    let _ = viewer.update(cx, |viewer, _, cx| {
                        if viewer.menu_bar != bar {
                            viewer.menu_bar = bar;
                            cx.notify();
                        }
                    });
                }
            });
        })
        .detach();
    }

    fn menu(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        match action {
            MenuAction::CreateDisplay => self.create_display(cx),
            MenuAction::GrantPermissions => self.open_setup(cx),
            MenuAction::OpenAeroSpaceConfig => {
                let pinned: Vec<String> = self
                    .conflicts
                    .iter()
                    .map(|(workspace, _)| workspace.clone())
                    .collect();
                let go = native::confirm(
                    t(
                        "A workspace is pinned to a monitor number",
                        "有工作区被固定到了显示器编号",
                    ),
                    &tf!(
                        "Your AeroSpace config assigns workspace {} to a monitor by number, in workspace-to-monitor-force-assignment. Agent displays count as monitors and take those numbers, so the workspace lands on an agent's display and Sidescreen can't move it. Use the monitor's name instead, like 2 = 'DELL', or install Sidescreen's AeroSpace and add ignored-monitors.",
                        "你的 AeroSpace 配置在 workspace-to-monitor-force-assignment 里按编号把工作区 {} 分给了某块显示器。Agent 显示器也算显示器，会占用这些编号，所以这个工作区落到了 Agent 的显示器上，Sidescreen 挪不动它。改用显示器名称，例如 2 = 'DELL'；或者安装 Sidescreen 版 AeroSpace 并加上 ignored-monitors。",
                        pinned.join(", ")
                    ),
                    t("Open Config", "打开配置"),
                );
                if go && let Some(config) = aerospace::config_path() {
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg(config)
                        .spawn();
                }
            }
            MenuAction::IgnoreInAeroSpace => self.ignore_in_aerospace(cx),
            MenuAction::ConnectAgents => self.connect_agents(cx),
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
            MenuAction::PressProxy { index } => {
                if let Some(element) = menubar::chosen(index) {
                    cx.background_spawn(async move { element.perform("AXPress") })
                        .detach();
                }
            }
        }
    }

    // MARK: Dismissal

    /// Close a display: ask on the Viewer what happens to its windows,
    /// unless there is nothing to ask.
    pub fn dismiss(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        // Asked again while the question is up: keep the user's choices.
        let asking = self.viewer.as_ref().is_some_and(|(viewer, _)| {
            viewer
                .read(cx)
                .is_ok_and(|v| v.sheet.as_ref().is_some_and(|s| s.display == id))
        });
        if asking {
            self.open_viewer(id, cx);
            self.take_keyboard();
            return;
        }
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
        // The question appears on the Viewer; Peek stays up.
        self.open_viewer(id, cx);
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
            let sheet = viewer.sheet.take();
            viewer.leaving = sheet.clone();
            sheet
        });
        self.after(super::viewer::SHEET_OUT, cx, move |_, cx| {
            let _ = viewer.update(cx, |viewer, _, cx| {
                viewer.leaving = None;
                cx.notify();
            });
        });
        if let (Ok(Some(sheet)), Some(items)) = (sheet, items) {
            self.dismiss_now(sheet.display, items, cx);
        }
    }

    fn dismiss_now(&mut self, id: DisplayId, items: Vec<Item>, cx: &mut Context<Self>) {
        self.core
            .update(cx, |core, cx| core.end(id, EndReason::ClosedByUser, cx));
        // The agent may have opened windows while the sheet was up: plan
        // again now it has stopped, keeping the user's choice for each
        // window they saw.
        let items = match self.core.read(cx).displays.get(id) {
            Some(display) => dismissal::plan(display)
                .into_iter()
                .map(|mut fresh| {
                    if let Some(seen) = items.iter().find(|i| i.target == fresh.target) {
                        fresh.fate = seen.fate;
                    }
                    fresh
                })
                .collect(),
            None => items,
        };
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
    pub fn take_back(&mut self, target: WindowTarget, id: DisplayId, cx: &mut Context<Self>) {
        let home = self.core.update(cx, |core, _| {
            let display = core.displays.get_mut(id)?;
            let home = display
                .records
                .get(&target.window_id)
                .and_then(|r| r.home.clone());
            display.give_back(target);
            // Taking a window back says the app is the user's now: its
            // later windows stay on the user's screen.
            core.kept_catches.retain(|(_, k)| k.pid() != target.pid);
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

    /// Quit. The tidy-up runs in the quit hook, so logout gets it too.
    fn quit(&mut self, cx: &mut Context<Self>) {
        cx.quit();
    }

    // MARK: Permissions

    pub fn open_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.setup
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            cx.activate(true);
            return;
        }
        let weak = cx.entity().downgrade();
        let status = self.permissions;
        let bounds = Bounds::centered(None, size(px(setup::WIDTH), px(setup::HEIGHT)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(t("Set Up Sidescreen", "设置 Sidescreen").into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        };
        match cx.open_window(options, |_, cx| cx.new(|_| Setup::new(weak, status))) {
            Ok(handle) => {
                self.setup = Some(handle);
                self.push_setup(cx);
                self.read_registered(cx);
                cx.activate(true);
            }
            Err(error) => log::info!("setup window: {error}"),
        }
    }

    /// An agent's call failed for want of `state`'s permissions or a
    /// reopen: show the setup window, once per state, so closing it sticks.
    pub fn setup_for_agent(&mut self, state: (Vec<Permission>, bool), cx: &mut Context<Self>) {
        if self.setup_shown_for.as_ref() == Some(&state) {
            return;
        }
        self.setup_shown_for = Some(state);
        let open = self
            .setup
            .is_some_and(|h| h.update(cx, |_, _, _| ()).is_ok());
        if !open {
            self.open_setup(cx);
        }
    }

    /// Which clients run this copy, off the main thread: Claude Code's
    /// config holds every project's history and can be large.
    fn read_registered(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |shell, cx| {
            let registered = cx
                .background_spawn(async {
                    let exe = std::env::current_exe().unwrap_or_default();
                    sidescreen_mcp::install::registered(&exe)
                })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.registered = registered;
                shell.push_setup(cx);
            });
        })
        .detach();
    }

    /// While the setup window asks for a reopen, look every 2 s whether
    /// macOS applied the grant late after all.
    fn recheck_capture(&mut self, cx: &mut Context<Self>) {
        if !self.restart || self.setup.is_none() || self.rechecking {
            return;
        }
        self.rechecking = true;
        cx.spawn(async move |shell, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let works = cx
                .background_spawn(async { crate::mac::screenshot::can_capture() })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.rechecking = false;
                if works && shell.restart {
                    permissions::NEEDS_REOPEN.store(false, std::sync::atomic::Ordering::Relaxed);
                    shell.capture_works(cx);
                }
            });
        })
        .detach();
    }

    /// Capture works after all: no reopen needed.
    pub fn capture_works(&mut self, cx: &mut Context<Self>) {
        self.restart = false;
        self.push_setup(cx);
    }

    pub fn close_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.setup.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.waiting = None;
        assist::hide();
    }

    /// The user clicked 开启… for `p`: the system prompt when macOS still
    /// shows one, else the pane, with the helper pinned to it.
    pub fn grant(&mut self, p: Permission, cx: &mut Context<Self>) {
        permissions::request(p);
        // Asking puts Sidescreen in the list, switched off.
        let listed = permissions::listed(p).unwrap_or(true);
        assist::show(p, listed);
        self.waiting = Some(p);
        self.push_setup(cx);
    }

    pub fn relaunch(&mut self, cx: &mut Context<Self>) {
        permissions::relaunch();
        self.quit(cx);
    }

    /// Register the MCP server with Claude Code and Codex, and say how it went.
    pub fn connect_agents(&mut self, cx: &mut Context<Self>) {
        // Finding the user's PATH and running the CLI takes seconds; a second
        // press would race the first.
        if self.connecting {
            return;
        }
        self.connecting = true;
        self.push_setup(cx);
        let install = cx.background_spawn(async {
            let exe = std::env::current_exe().unwrap_or_default();
            sidescreen_mcp::install::install(&exe)
        });
        cx.spawn(async move |shell, cx| {
            use sidescreen_mcp::install::Done;
            let outcomes = install.await;
            let lines: Vec<String> = outcomes
                .iter()
                .map(|o| match &o.result {
                    None => tf!("{}: not installed", "{}：没有安装", o.client),
                    Some(Ok(Done::Already)) => {
                        tf!("{}: already connected", "{}：已经接入", o.client)
                    }
                    Some(Ok(Done::Connected)) => tf!(
                        "{}: connected for all projects",
                        "{}：已接入，所有项目都能用",
                        o.client
                    ),
                    Some(Ok(Done::AddedTo(path))) => tf!(
                        "{}: added to {}",
                        "{}：已写入 {}",
                        o.client,
                        path.display()
                    ),
                    Some(Err(error)) => {
                        tf!("{}: failed. {}", "{}：失败。{}", o.client, error)
                    }
                })
                .collect();
            let exe = std::env::current_exe().unwrap_or_default();
            let failed = outcomes.iter().any(|o| matches!(o.result, Some(Err(_))));
            let none = outcomes.iter().all(|o| o.result.is_none());
            let next = if none {
                tf!(
                    "For another MCP client, add a stdio server with command\n{}\nand arguments [\"mcp\"]. If Claude Code is installed, run in Terminal:\n{} mcp install",
                    "其他 MCP 客户端：添加一个 stdio 服务器，命令是\n{}\n参数是 [\"mcp\"]。装了 Claude Code 的话，在终端里运行：\n{} mcp install",
                    exe.display(),
                    exe.display()
                )
            } else if failed {
                tf!(
                    "To try in Terminal, where your agents' PATH is set:\n{} mcp install",
                    "可以在终端里重试，那里有 agent 需要的 PATH：\n{} mcp install",
                    exe.display()
                )
            } else {
                t(
                    "Start a new agent session to load the tools.",
                    "新开一个 agent 会话就能用上这些工具。",
                )
                .to_string()
            };
            let registered = cx
                .background_spawn(async move { sidescreen_mcp::install::registered(&exe) })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.connecting = false;
                shell.registered = registered;
                shell.push_setup(cx);
            });
            native::alert(
                t("Sidescreen and your agents", "Sidescreen 与 agent"),
                &format!("{}\n\n{next}", lines.join("\n")),
            );
        })
        .detach();
    }

    pub fn last_agent_ready(&self) -> Option<String> {
        self.agents_ready()
    }

    /// The agents that loaded the tools since Sidescreen started, by name.
    fn agents_ready(&self) -> Option<String> {
        match self.agent_ready.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [rest @ .., last] => Some(format!("{} {} {last}", rest.join(", "), t("and", "和"))),
        }
    }

    /// An agent's MCP server listed the tools: setup worked on its side.
    pub fn agent_ready(&mut self, client: String, cx: &mut Context<Self>) {
        log::info!("{client} loaded the tools");
        if !self.agent_ready.contains(&client) {
            self.agent_ready.push(client);
        }
        self.push_setup(cx);
    }

    fn push_setup(&mut self, cx: &mut Context<Self>) {
        let (status, waiting, restart) = (self.permissions, self.waiting, self.restart);
        let agent = self.agents_ready();
        let connecting = self.connecting;
        let Some(handle) = self.setup else { return };
        let registered = self.registered.clone();
        if handle
            .update(cx, |setup, _, cx| {
                setup.status = status;
                setup.waiting = waiting;
                setup.restart = restart;
                setup.agent = agent;
                setup.connecting = connecting;
                setup.registered = registered;
                cx.notify();
            })
            .is_err()
        {
            // The user closed it with its close button.
            self.setup = None;
            self.waiting = None;
            assist::hide();
        }
    }

    fn check_permissions(&mut self, cx: &mut Context<Self>) {
        if assist::is_shown() && !assist::tick() {
            // System Settings quit.
            assist::hide();
        }
        let now = permissions::status();
        let before = self.permissions;
        if now == before {
            self.recheck_capture(cx);
            self.push_setup(cx);
            return;
        }
        self.permissions = now;
        self.icon.set_needs_setup(!now.complete());
        // The Viewer covers its picture while Screen Recording is off.
        self.repaint(cx);
        if self.waiting.is_some_and(|p| now.has(p)) {
            // Granted: back to the setup window, which shows the next step.
            self.waiting = None;
            assist::hide();
            if let Some(handle) = self.setup {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                cx.activate(true);
            }
        }
        if now.screen && !before.screen && !permissions::screen_recording_at_launch() {
            // macOS may apply Screen Recording only to a new process.
            cx.spawn(async move |shell, cx| {
                let works = cx
                    .background_executor()
                    .spawn(async { crate::mac::screenshot::can_capture() })
                    .await;
                let _ = shell.update(cx, |shell, cx| {
                    shell.restart = !works;
                    permissions::NEEDS_REOPEN.store(!works, std::sync::atomic::Ordering::Relaxed);
                    shell.push_setup(cx);
                });
            })
            .detach();
        }
        if !now.complete() && before.complete() {
            log::info!("permission turned off: {:?}", now.missing());
        }
        self.push_setup(cx);
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
    // watcher only observes: the window under the mouse-down point is the
    // candidate, and it counts as dragged once its frame has moved. The
    // window list is read at the first drag event, not on every click.

    fn drag_began(&mut self, at: (f64, f64)) {
        self.drag = None;
        self.drag_from = self.viewer_open.then_some(at);
    }

    /// Find the window the drag started on.
    fn drag_candidate(&mut self, at: (f64, f64)) {
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
        if let Some(from) = self.drag_from.take() {
            self.drag_candidate(from);
        }
        let Some(drag) = &mut self.drag else { return };
        if drag.last_check.elapsed() < Duration::from_millis(50) {
            return;
        }
        drag.last_check = Instant::now();
        let over = self.drop_rect(cx).is_some_and(|r| contains(r, at)) && self.dragging();
        self.set_drop_hover(over, cx);
    }

    fn drag_ended(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        self.drag_from = None;
        let over = self.drop_rect(cx).is_some_and(|r| contains(r, at));
        let dropped = over && self.dragging();
        let drag = self.drag.take();
        self.set_drop_hover(false, cx);
        if let (true, Some(drag), Some(id)) = (dropped, drag, self.viewer_display) {
            // Let the window manager finish its own drop handling first, or
            // it re-tiles the window after we move it.
            self.after(Duration::from_millis(150), cx, move |shell, cx| {
                shell.hand_over(drag.target, Some(drag.frame), id, cx)
            });
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

    // MARK: AeroSpace

    fn check_aerospace(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let ignoring = cx
                .background_executor()
                .spawn(async { aerospace::ignoring() })
                .await;
            let _ = this.update(cx, |shell, _| shell.aerospace = ignoring);
        })
        .detach();
    }

    /// Set AeroSpace to leave Agent Displays alone, or say how to get an
    /// AeroSpace that can (docs/adr/0006).
    fn ignore_in_aerospace(&mut self, cx: &mut Context<Self>) {
        let why = t(
            "AeroSpace manages every display, Sidescreen's too. It shows one of your workspaces on each new display, which takes those windows off your screen and into the agent's screenshots, and it files the agents' new windows in your workspace, so they pass over your screen first.",
            "AeroSpace 会管理所有显示器，包括 Sidescreen 的。每建一块显示器，它就把你的一个工作区放上去，那些窗口会从你屏幕上消失，出现在 Agent 的截图里。Agent 打开的新窗口也会被它登记到你的工作区，先在你屏幕上闪一下。",
        );
        if self.aerospace == Some(aerospace::Ignoring::Unsupported) {
            let commands = "brew tap fanzzzd/sidescreen
brew uninstall --cask aerospace
brew install --cask aerospace-sidescreen
tccutil reset Accessibility bobko.aerospace
open -a AeroSpace";
            let go = native::confirm(
                t(
                    "This AeroSpace can't leave displays alone",
                    "这个版本的 AeroSpace 还不能不管显示器",
                ),
                &format!(
                    "{why}\n\n{}",
                    t(
                        "Sidescreen's build of AeroSpace adds the option it needs and keeps your config. The guide shows the five Terminal commands; they're on your clipboard.",
                        "Sidescreen 提供的 AeroSpace 加上了需要的选项，你的配置不变。说明页里有 5 条终端命令，已经复制到剪贴板。",
                    ),
                ),
                t("Copy Commands and Open Guide", "复制命令并打开说明"),
            );
            if go {
                native::copy(commands);
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("https://github.com/Fanzzzd/sidescreen/blob/main/docs/aerospace.md")
                    .spawn();
            }
            return;
        }
        let path = aerospace::config_path()
            .map(|p| {
                p.display()
                    .to_string()
                    .replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1)
            })
            .unwrap_or_else(|| "~/.config/aerospace/aerospace.toml".into());
        let go = native::confirm(
            t(
                "Let AeroSpace leave Sidescreen's displays alone?",
                "让 AeroSpace 不管 Sidescreen 的显示器？",
            ),
            &format!(
                "{why}

{}",
                tf!(
                    "Sidescreen adds ignored-monitors = ['^Sidescreen'] to {}, keeps the old file beside it as .before-sidescreen, and reloads AeroSpace's config.",
                    "Sidescreen 会在 {} 里加上 ignored-monitors = ['^Sidescreen']，旧文件另存为 .before-sidescreen，然后让 AeroSpace 重新加载配置。",
                    path
                )
            ),
            t("Add and Reload", "添加并重新加载"),
        );
        if !go {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { aerospace::ignore_agent_displays() })
                .await;
            let short = |p: &std::path::Path| {
                p.display()
                    .to_string()
                    .replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1)
            };
            match result {
                Ok(done) => {
                    log::info!("AeroSpace now ignores Agent Displays");
                    let old = match &done.backup {
                        Some(backup) => tf!(
                            "Your old config is at {}.",
                            "旧配置在 {}。",
                            short(backup)
                        ),
                        None => t(
                            "You had no config; this one is AeroSpace's default plus that line.",
                            "你原来没有配置文件；新文件是 AeroSpace 的默认配置加上这一行。",
                        )
                        .to_string(),
                    };
                    native::alert(
                        t(
                            "AeroSpace now leaves Sidescreen's displays alone",
                            "AeroSpace 不再管 Sidescreen 的显示器了",
                        ),
                        &format!(
                            "{}\n\n{old}",
                            tf!(
                                "Added ignored-monitors = ['^Sidescreen'] to {}.",
                                "已在 {} 里加上 ignored-monitors = ['^Sidescreen']。",
                                short(&done.config)
                            )
                        ),
                    );
                }
                Err(failed) => {
                    log::warn!("AeroSpace refused ignored-monitors: {}", failed.reason);
                    let state = if failed.unchanged {
                        t("Your config is as it was.", "你的配置没有变。")
                    } else {
                        t(
                            "Sidescreen couldn't put your old config back; it's in the .before-sidescreen file beside it.",
                            "Sidescreen 没能把旧配置放回去；它在旁边的 .before-sidescreen 文件里。",
                        )
                    };
                    native::alert(
                        t("AeroSpace didn't take the change", "AeroSpace 没有接受这个改动"),
                        &format!("{state}\n\n{}", failed.reason),
                    );
                }
            }
            let _ = this.update(cx, |shell, cx| {
                shell.check_aerospace(cx);
                shell.schedule_guard(cx);
            });
        })
        .detach();
    }

    // MARK: AeroSpace guard

    /// Stock AeroSpace manages every display, virtual ones included, and may
    /// show one of the user's workspaces on a new Agent Display. After a
    /// display change, move such a workspace back to the monitor it was on,
    /// then show and focus what the user had before. Workspaces
    /// force-assigned to a monitor number cannot move; those become
    /// conflicts the menu explains. While nothing needs fixing, note where
    /// the user's workspaces are, for the next time.
    fn schedule_guard(&mut self, cx: &mut Context<Self>) {
        self.guard_generation += 1;
        let generation = self.guard_generation;
        cx.spawn(async move |this, cx| {
            // AeroSpace reacts to display changes on its own schedule. Look
            // soon, since a user's workspace on an Agent Display vanishes
            // from their screen and shows to the agent, and look again.
            // Most users run no AeroSpace, and with ignored-monitors it
            // takes nothing: then there is nothing to guard.
            let needed = cx
                .background_executor()
                .spawn(async {
                    aerospace::ignoring().is_some_and(|i| i != aerospace::Ignoring::Yes)
                })
                .await;
            if !needed {
                return;
            }
            let waits = [100, 150, 250, 650, 1100];
            for (pass, wait) in waits.into_iter().enumerate() {
                let last = pass + 1 == waits.len();
                cx.background_executor()
                    .timer(Duration::from_millis(wait))
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
                        // With ignored-monitors, AeroSpace takes nothing.
                        if aerospace::ignores_now() {
                            return Vec::new();
                        }
                        let Some(aerospace) = AeroSpace::current() else {
                            return Vec::new();
                        };
                        let results: Vec<Guarded> = checks
                            .iter()
                            .map(|(name, ours)| guard(&aerospace, name, ours))
                            .collect();
                        if results.contains(&Guarded::Moved) {
                            aerospace.restore();
                        } else if last && results.iter().all(|r| *r == Guarded::Clean) {
                            aerospace.remember();
                        }
                        results
                            .into_iter()
                            .filter_map(|r| match r {
                                Guarded::Pinned(workspace, display) => Some((workspace, display)),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                let _ = this.update(cx, |shell, _| {
                    for (workspace, display) in &conflicts {
                        log::warn!("AeroSpace pins workspace {workspace} to {display}");
                    }
                    shell.conflicts = conflicts;
                });
            }
        })
        .detach();
    }
}

#[derive(Debug, PartialEq)]
enum Guarded {
    /// The display shows only the agent's windows.
    Clean,
    /// A workspace of the user's moved back where it was.
    Moved,
    /// The workspace, pinned to the display, cannot move.
    Pinned(String, String),
}

/// Check one Agent Display, and move a workspace of the user's on it back.
fn guard(aerospace: &AeroSpace, name: &str, ours: &HashSet<u32>) -> Guarded {
    // One call when nothing is wrong, the usual case: AeroSpace answers
    // slowly right after a display change, and every call waits in line.
    if aerospace
        .visible_windows_on(name)
        .is_ok_and(|windows| windows.iter().all(|w| ours.contains(w)))
    {
        return Guarded::Clean;
    }
    let Some(monitor) = aerospace.monitor_id(name).ok().flatten() else {
        return Guarded::Clean;
    };
    let mut result = Guarded::Clean;
    // Each eviction makes AeroSpace pick another workspace; stop after a few.
    for _ in 0..3 {
        let Some(workspace) = aerospace.visible_workspace(&monitor).ok().flatten() else {
            break;
        };
        let Ok(windows) = aerospace.window_ids(&workspace) else {
            break;
        };
        if windows.iter().all(|w| ours.contains(w)) {
            break;
        }
        let home = aerospace.home_of(&workspace);
        log::info!("aerospace: workspace {workspace} took {name}; back to monitor {home}");
        if let Err(error) = aerospace.move_workspace(&workspace, &home) {
            // A slow AeroSpace isn't a pin; the next pass tries again.
            if error.contains("timed out") {
                break;
            }
            return Guarded::Pinned(workspace, name.to_string());
        }
        result = Guarded::Moved;
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: NSRect, b: NSRect) -> bool {
        (a.origin.x, a.origin.y, a.size.width, a.size.height)
            == (b.origin.x, b.origin.y, b.size.width, b.size.height)
    }

    #[test]
    fn a_card_inside_the_screen_stays_put() {
        let screen = rect(0.0, 0.0, 1440.0, 875.0);
        let card = rect(100.0, 100.0, 800.0, 500.0);
        assert!(same(keep_inside(card, screen), card));
    }

    #[test]
    fn a_card_past_an_edge_moves_back_in() {
        let screen = rect(0.0, 0.0, 1440.0, 875.0);
        let card = rect(1000.0, 600.0, 800.0, 500.0);
        assert!(same(
            keep_inside(card, screen),
            rect(640.0, 375.0, 800.0, 500.0)
        ));
    }

    #[test]
    fn a_card_taller_than_the_screen_keeps_its_header_in_view() {
        let screen = rect(0.0, 0.0, 1440.0, 875.0);
        let card = rect(100.0, -300.0, 800.0, 1000.0);
        assert!(same(
            keep_inside(card, screen),
            rect(100.0, -125.0, 800.0, 1000.0)
        ));
    }
}
