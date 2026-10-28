//! The shell around the Agent Displays: the menu bar icon, Peek, the
//! Viewer, Dismissal and Handover, and the guards that keep the user's
//! cursor and AeroSpace workspaces off Agent Displays.

use super::help_card::{self, HelpCard};
use super::peek::{self, Peek};
use super::pointer;
use super::recordings::Recordings;
use super::setup::{self, Setup};
use super::stream::Streams;
use super::viewer::{self, Sheet, Viewer, WindowMenu};
use crate::core::Core;
use crate::dismissal::{self, Item};
use crate::i18n::t;
use crate::mac::aerospace::{self, AeroSpace};
use crate::mac::native::{self, MenuAction, Native, StatusIcon};
use crate::mac::notify::Note;
use crate::mac::permissions::{self, Permission};
use crate::mac::{app, assist, ax, cg, menubar, notify, update, viewer_input, windows};
use crate::model::{DEFAULT_SIZE, DisplayId, EndReason, Origin, Rect, WindowTarget};
use agent_screen_model::peek as peek_intent;
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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
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

// Shell policy seams accept observed geometry/events and return render effects.
// Tests drive these same functions without constructing a panel or app.
fn peek_activation(
    intent: &mut peek_intent::Intent,
    event: &Native,
    pointer: peek_intent::Pointer,
    now: u64,
) -> peek_intent::Effect {
    intent.pointer(pointer, now, peek_intent::Timing::default());
    match event {
        Native::IconPressed { gesture } => {
            intent.press(*gesture);
            peek_intent::Effect::None
        }
        Native::IconClicked {
            right: false,
            mouse: true,
        } => intent.release(true),
        Native::IconClicked {
            right: false,
            mouse: false,
        } => intent.activate(),
        _ => peek_intent::Effect::None,
    }
}

fn peek_outside_press(
    intent: &mut peek_intent::Intent,
    pointer: peek_intent::Pointer,
    hit: peek_intent::Pointer,
    now: u64,
) -> peek_intent::Effect {
    intent.pointer(pointer, now, peek_intent::Timing::default());
    intent.outside_press(hit)
}

fn peek_timer_effect(
    intent: &mut peek_intent::Intent,
    timer: peek_intent::Timer,
    pointer: peek_intent::Pointer,
    now: u64,
    allowed: bool,
) -> peek_intent::Effect {
    if !allowed {
        intent.cancel_timer();
        return peek_intent::Effect::None;
    }
    intent.pointer(pointer, now, peek_intent::Timing::default());
    intent.fire(timer, now)
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

/// The repository's bug report form, which asks for the diagnostics.
const REPORT_FORM: &str =
    "https://github.com/Fanzzzd/agent-screen/issues/new?template=bug_report.yml";

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
    card: Option<(WindowHandle<HelpCard>, Retained<NSWindow>)>,
    /// The display the Help Card shows, and whether for a Window or
    /// Control Request, which have no picture.
    card_shown: Option<(DisplayId, bool)>,
    /// Help Requests the user put off with Later, by display.
    card_later: HashMap<DisplayId, String>,
    /// When the user was last told where an app's windows are, by pid.
    reminded: HashMap<i32, Instant>,
    /// A downloaded update waits for no Agent Display to be open.
    watching_updates: bool,

    peek_shown: bool,
    /// Some display is in use: the icon's front screen is filled.
    working: bool,
    /// Displays being made for "+", each a waiting tile in Peek.
    creating: usize,
    /// Peek's card while it slides: where it ends, and which slide.
    slide_card: Option<NSRect>,
    peek_slides: u64,
    peek_intent: peek_intent::Intent,
    peek_clock: Instant,
    peek_timer: Option<peek_intent::Timer>,
    /// Cancels a queued native show without cancelling a visible resize.
    peek_native_generation: Rc<Cell<u64>>,
    /// Hover as AppKit and GPUI report it. An exit can go unreported (Peek
    /// ordered out under the pointer, a menu tracking), so Peek decides to
    /// stay by where the pointer is; see `pointer_over`.
    over_icon: bool,
    over_card: bool,
    peek_generation: u64,
    /// Counts Peek's reminders of where an app's windows are.
    remind_seq: u64,
    watch_generation: u64,
    /// No hover Peek until then: the pointer is still on its way.
    settle_until: Option<Instant>,

    viewer_display: Option<DisplayId>,
    /// The display new windows go to after the user typed in a launcher
    /// while working in the Viewer, and until when.
    launch_here: Option<(DisplayId, std::time::Instant)>,
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
    /// Whether macOS lets Agent Screen notify.
    notifications: notify::Auth,
    /// The ColorSync helper (ADR 0009), as the last run found it; None
    /// before the first, 10 s after launch.
    colorsync: Option<crate::mac::colorsync::Helper>,
    /// Setting it up is running.
    colorsync_busy: bool,
    colorsync_error: Option<String>,
    /// Test hook: the setup window shows both permissions missing.
    setup_preview: bool,
    setup: Option<WindowHandle<Setup>>,
    recordings: Option<WindowHandle<Recordings>>,
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
    /// Clients whose config runs this copy of Agent Screen, read when the
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

/// Back to a menu bar app, out of the Dock. A regular app also takes part
/// in each display's menu bar: with six displays, display changes kept its
/// main thread busy in AppKit's menu bar code too, up to 880 ms at a time.
fn leave_dock(cx: &mut App) {
    cx.set_activation_policy(gpui::ActivationPolicy::Accessory);
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

/// Peek's card, in Cocoa coordinates: its window less the margin where the
/// card's shadow is drawn.
fn peek_card(window: &NSWindow) -> NSRect {
    native::inset(native::frame(window), peek::MARGIN)
}

/// Peek's window frame for a card frame.
fn outset_card(card: NSRect) -> NSRect {
    native::outset(card, peek::MARGIN)
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
        // Panels are never the key window, and GPUI holds an inactive
        // window's animations to 30 fps.
        inactive_frame_interval: None,
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
    native::watch_activations();
    crate::mac::cg::watch_display_changes();
    crate::mac::hang::watch();
    native::watch_colors();
    let accent = native::accent();
    super::theme::set_accent(accent);
    viewer_input::install();
    notify::install();
    notify::check();
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
            card: None,
            card_shown: None,
            reminded: HashMap::new(),
            watching_updates: false,
            card_later: HashMap::new(),
            peek_shown: false,
            working: false,
            creating: 0,
            slide_card: None,
            peek_slides: 0,
            peek_intent: peek_intent::Intent::default(),
            peek_clock: Instant::now(),
            peek_timer: None,
            peek_native_generation: Rc::new(Cell::new(0)),
            over_icon: false,
            over_card: false,
            peek_generation: 0,
            remind_seq: 0,
            watch_generation: 0,
            settle_until: None,
            viewer_display: None,
            launch_here: None,
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
            notifications: notify::Auth::Unknown,
            colorsync: None,
            colorsync_busy: false,
            colorsync_error: None,
            setup_preview: false,
            setup: None,
            recordings: None,
            agent_ready: Vec::new(),
            waiting: None,
            restart: false,
            setup_shown_for: None,
            connecting: false,
            rechecking: false,
            registered: Vec::new(),
        }
    });
    // The Dock icon goes with the setup window.
    cx.on_window_closed({
        let shell = shell.downgrade();
        move |cx, id| {
            let _ = shell.update(cx, |shell, cx| shell.window_closed(id, cx));
        }
    })
    .detach();
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
    let card = {
        let (weak, core, streams) = (weak.clone(), core.clone(), streams.clone());
        cx.open_window(panel_options(), move |window, cx| {
            cx.new(|cx| HelpCard::new(weak, core, streams, window, cx))
        })?
    };
    let card_ns = card
        .update(cx, |_, window, _| native::ns_window(window))?
        .expect("Help Card window");
    native::style_panel(&card_ns, native::LEVEL_POP_UP_MENU);
    native::set_keyable(&card_ns, false);
    let peek_ns = peek
        .update(cx, |_, window, _| native::ns_window(window))?
        .expect("Peek window");
    let viewer_ns = viewer
        .update(cx, |_, window, _| native::ns_window(window))?
        .expect("Viewer window");
    native::style_panel(&peek_ns, native::LEVEL_POP_UP_MENU);
    native::own_shadow(&peek_ns, peek::MARGIN);
    native::style_panel(&viewer_ns, native::LEVEL_FLOATING);
    // Peek never takes the keyboard from the user's app; neither panel
    // becomes the main window.
    native::set_keyable(&peek_ns, false);
    native::set_keyable(&viewer_ns, true);
    viewer_input::attach(&viewer_ns);
    shell.update(cx, |shell, cx| {
        shell.peek = Some((peek, peek_ns));
        shell.viewer = Some((viewer, viewer_ns));
        shell.card = Some((card, card_ns));
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
            crate::mac::claims::delete();
            // Every display goes with the process: the log says so, as
            // each is a display change all apps handle.
            for id in core.read(cx).virtual_ids() {
                crate::lifecycle::note("released", id, "quit");
            }
            crate::lifecycle::line(&format!("app quit pid={}", std::process::id()));
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
            // Both ask WindowServer, which can keep them waiting a second.
            let (now, settings) = cx
                .background_spawn(async move { (permissions::status(), fast.then(assist::look)) })
                .await;
            if checked
                .update(cx, |shell, cx| shell.check_permissions(now, settings, cx))
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
                    if shell.icon_rect().is_some() {
                        shell.icon.keep_place();
                    }
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
        if TIDIED.with(Cell::get) {
            self.peek_intent.stop();
        }
        if self.peek_intent.stopped()
            && matches!(
                &event,
                Native::IconHover(_) | Native::IconPressed { .. } | Native::IconClicked { .. }
            )
        {
            return;
        }
        match event {
            event @ Native::IconPressed { .. } => {
                let (pointer, now) = (self.peek_pointer(), self.peek_now());
                peek_activation(&mut self.peek_intent, &event, pointer, now);
                self.schedule_peek_timer(cx);
            }
            Native::IconHover(over) => {
                // Before the menu opens; AeroSpace may have changed since.
                if over && !self.over_icon {
                    self.check_aerospace(cx);
                }
                self.over_icon = over;
                self.hover_changed(cx);
            }
            Native::IconClicked { right: true, .. } => {
                self.peek_intent.release(false);
                self.hide_peek(cx);
                self.status_menu();
            }
            Native::IconClicked { right: false, .. } if !self.permissions.complete() => {
                self.hide_peek(cx);
                self.open_setup(cx);
            }
            event @ Native::IconClicked { right: false, .. } => {
                let (pointer, now) = (self.peek_pointer(), self.peek_now());
                let effect = peek_activation(&mut self.peek_intent, &event, pointer, now);
                self.apply_peek_effect(effect, cx);
                // A deliberate open/pin can take keys; hover never does.
                if effect == peek_intent::Effect::Show
                    && self.peek_shown
                    && let Some((peek, window)) = self.peek.clone()
                {
                    native::make_key(&window);
                    let _ = peek.update(cx, |peek, window, cx| {
                        peek.focused = None;
                        peek.keys = true;
                        window.focus(&peek.focus, cx);
                        cx.notify();
                    });
                }
            }
            Native::MouseDown { x, y } => {
                // The status button captures its own down. A monitor must
                // never dismiss it before that same gesture's release.
                let on_icon = self.icon.frame().is_some_and(|f| contains(f, (x, y)));
                let on_card = self
                    .peek
                    .as_ref()
                    .filter(|_| self.peek_shown)
                    .is_some_and(|(_, w)| contains(peek_card(w), (x, y)));
                let (pointer, now) = (self.peek_pointer(), self.peek_now());
                let effect = peek_outside_press(
                    &mut self.peek_intent,
                    pointer,
                    peek_intent::Pointer {
                        icon: on_icon,
                        peek: on_card,
                    },
                    now,
                );
                self.apply_peek_effect(effect, cx);
            }
            Native::LeftDown { x, y } => self.drag_began((x, y)),
            Native::LeftDragged { x, y } => self.drag_moved((x, y), cx),
            Native::LeftUp { x, y } => {
                if !self.icon.frame().is_some_and(|f| contains(f, (x, y))) {
                    self.peek_intent.release(false);
                    self.hover_changed(cx);
                }
                self.drag_ended((x, y), cx);
            }
            Native::Menu(action) => self.menu(action, cx),
            // A notification only opens the display; the user's first
            // input there, or its Take Over button, starts a Takeover.
            Native::NotificationClicked { display, take_over } => {
                let asking = self
                    .core
                    .read(cx)
                    .displays
                    .get(display)
                    .map(|d| d.help_request.is_some() && d.is_live());
                match asking {
                    Some(true) if take_over => self.open_for_help(display, cx),
                    Some(_) => self.card_open(display, cx),
                    None => {}
                }
            }
            Native::NotificationAuth(auth) => self.notification_auth(auth, cx),
            Native::ScreenTouched { display } => self.user_input(display, cx),
            Native::LauncherTyped => self.launcher_typed(cx),
            Native::Activated { pid, previous } => self.activated(pid, previous, cx),
            Native::UserClicked { x, y } => self.user_clicked((x, y), cx).detach(),
            Native::UpdateReady => self.watch_updates(cx),
            Native::MenuBarClick { display, x } => self.menu_bar(display, x, cx),
            Native::ScreensChanged => {
                self.core.update(cx, |core, cx| {
                    core.repark(cx);
                    core.hold_modes(cx);
                });
                self.streams.update(cx, |s, cx| s.restart_misrouted(cx));
                self.schedule_guard(cx);
                self.schedule_regather(cx);
            }
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
        let helper = self.core.read(cx).colorsync_helper();
        if helper.is_some() && helper != self.colorsync {
            self.colorsync = helper;
            self.push_setup(cx);
        }
        let core = self.core.read(cx);
        let ids: HashSet<DisplayId> = core.displays.list.iter().map(|d| d.id).collect();
        let asking = core.displays.list.iter().any(|d| {
            (d.help_request.is_some() || d.window_request.is_some() || d.control_request.is_some())
                && !d.is_ended()
        });
        if asking != self.asking {
            self.asking = asking;
            self.icon.set_asking(asking);
        }
        let working = core.displays.list.iter().any(|d| !d.is_ended());
        if working != self.working {
            self.working = working;
            self.icon.set_working(working);
        }
        for display in &core.displays.list {
            let task = display.owner.as_ref().map_or("", |o| o.title.as_str());
            match display
                .help_request
                .as_deref()
                .filter(|_| display.is_live())
            {
                Some(reason) => notify::update(
                    display.id,
                    display.agent_name(),
                    task,
                    Some(Note::Help(reason.to_string())),
                ),
                None => notify::withdraw_help(display.id),
            }
            match display
                .window_request
                .as_ref()
                .filter(|_| display.is_live())
            {
                Some(request) => notify::update(
                    display.id,
                    display.agent_name(),
                    task,
                    Some(Note::Window(request.app.clone())),
                ),
                None => notify::withdraw_window(display.id),
            }
            match display
                .control_request
                .as_deref()
                .filter(|_| display.is_live())
            {
                Some(reason) => notify::update(
                    display.id,
                    display.agent_name(),
                    task,
                    Some(Note::Control(reason.to_string())),
                ),
                None => notify::withdraw_control(display.id),
            }
            // An agent left windows on its display: tell the user once.
            if display.owner.is_some() && display.is_ended() && !self.ended.contains(&display.id) {
                let windows = windows::on_display(display.id).len();
                let note = match display.ended {
                    Some(EndReason::Disconnected) => Note::Disconnected { windows },
                    _ => Note::Review { windows },
                };
                if windows > 0 && display.ended != Some(EndReason::ClosedByUser) {
                    notify::update(display.id, display.agent_name(), task, Some(note));
                }
            }
        }
        let gone: Vec<DisplayId> = self.known.difference(&ids).copied().collect();
        for id in &gone {
            notify::update(*id, "", "", None);
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
        self.sync_card(cx);
    }

    // MARK: Help Card

    /// Show the Help Card for the first open Help Request the user hasn't
    /// put off, or hide it. It steps aside for Peek, and for the Viewer
    /// showing that display.
    fn sync_card(&mut self, cx: &mut Context<Self>) {
        let Some((card, window)) = self.card.clone() else {
            return;
        };
        let core = self.core.clone();
        let core = core.read(cx);
        self.card_later.retain(|id, reason| {
            core.displays.get(*id).and_then(|d| d.help_request.as_ref()) == Some(reason)
        });
        let viewing = self.viewer_open.then_some(self.viewer_display).flatten();
        // Window and Control Requests wait for an answer even beside the
        // Viewer, where the user operates the display during a Takeover.
        let requests = core
            .displays
            .list
            .iter()
            .filter(|d| d.is_live() && (d.window_request.is_some() || d.control_request.is_some()))
            .map(|d| (d.id, true, (d.width, d.height)));
        let helps = core
            .displays
            .list
            .iter()
            .filter(|d| {
                d.is_live()
                    && !d.takeover
                    && viewing != Some(d.id)
                    && d.help_request
                        .as_ref()
                        .is_some_and(|r| self.card_later.get(&d.id) != Some(r))
            })
            .map(|d| (d.id, false, (d.width, d.height)));
        let want = requests.chain(helps).find(|_| !self.peek_shown);
        if want.map(|w| (w.0, w.1)) == self.card_shown {
            return;
        }
        if let Some((old, _)) = self.card_shown.take() {
            self.streams
                .update(cx, |s, cx| s.want(old, "card", 0, (0, 0), cx));
        }
        let Some((id, request, size)) = want else {
            native::order_out(&window);
            return;
        };
        let _ = card.update(cx, |card, _, cx| {
            card.display = Some(id);
            cx.notify();
        });
        self.streams.update(cx, |s, cx| {
            s.want(id, "card", 4, thumbnail_pixels(size), cx)
        });
        let Some((screen, _)) = self.host_screen() else {
            return;
        };
        let (w, h) = (
            f64::from(help_card::WIDTH),
            f64::from(if request {
                help_card::REQUEST_HEIGHT
            } else {
                help_card::height(size)
            }),
        );
        let anchor = self.anchor();
        let (mid, top) = (
            anchor.origin.x + anchor.size.width / 2.0,
            anchor.origin.y - 6.0,
        );
        let x = (mid - w / 2.0)
            .min(screen.origin.x + screen.size.width - w - 8.0)
            .max(screen.origin.x + 8.0);
        let frame = rect(x.round(), (top - h).round(), w, h);
        let shown = window.clone();
        native::later(move || {
            native::set_alpha(&shown, 0.0);
            native::set_frame(&shown, offset(frame, 10.0));
            native::order_front(&shown);
            native::animate(&shown, frame, 1.0, 0.24);
        });
        self.card_shown = Some((id, request));
    }

    /// The user answers a Window Request from the card: a window given
    /// goes over as a Handover. False when there was no request.
    pub fn answer_window(&mut self, id: DisplayId, give: bool, cx: &mut Context<Self>) -> bool {
        let answered = self.core.update(cx, |core, cx| {
            let open = core
                .displays
                .get(id)
                .is_some_and(|d| d.window_request.is_some());
            let target = core.displays.answer_window(id, give);
            cx.notify();
            open.then_some(target)
        });
        let Some(target) = answered else {
            return false;
        };
        if let Some(target) = target {
            self.hand_over(target, None, id, cx);
        }
        true
    }

    /// The user answers a Control Request from the card: Continue hands
    /// the display back as the Viewer's button does, Keep Control leaves
    /// the Takeover. False when there was no request.
    pub fn answer_control(&mut self, id: DisplayId, give: bool, cx: &mut Context<Self>) -> bool {
        self.core.update(cx, |core, cx| {
            let answered = core.displays.answer_control(id, give);
            if answered && give {
                core.stop_agent_input(id, false);
            }
            if answered {
                cx.notify();
            }
            answered
        })
    }

    /// A click on the Help Card's picture opens the display, as a
    /// notification does.
    pub fn card_open(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        self.hide_peek(cx);
        self.open_viewer(id, cx);
    }

    /// Later: put this Help Request off. The orange icon and Peek still
    /// show it; a new request from the display brings the card back.
    pub fn card_later(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        if let Some(reason) = self
            .core
            .read(cx)
            .displays
            .get(id)
            .and_then(|d| d.help_request.clone())
        {
            self.card_later.insert(id, reason);
        }
        self.sync_card(cx);
    }

    /// Window counts for the status lines.
    fn refresh_counts(&mut self, cx: &mut Context<Self>) {
        if !self.peek_shown && !self.viewer_open {
            return;
        }
        let ids: Vec<DisplayId> = self.known.iter().copied().collect();
        // The window list can wait on WindowServer; count off the UI thread.
        cx.spawn(async move |shell, cx| {
            let counts = cx
                .background_spawn(async move { windows::counts(&ids) })
                .await;
            let _ = shell.update(cx, |shell, cx| shell.show_counts(counts, cx));
        })
        .detach();
    }

    fn show_counts(
        &mut self,
        counts: std::collections::HashMap<DisplayId, usize>,
        cx: &mut Context<Self>,
    ) {
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

    /// The display the Viewer shows, open or not.
    pub fn viewer_display(&self) -> Option<DisplayId> {
        self.viewer_display
    }

    // MARK: Peek

    fn peek_now(&self) -> u64 {
        self.peek_clock.elapsed().as_millis() as u64
    }

    fn hover_changed(&mut self, cx: &mut Context<Self>) {
        if TIDIED.with(Cell::get) {
            self.peek_intent.stop();
        }
        if (!self.permissions.complete() && !self.peek_shown)
            || self.settle_until.is_some_and(|t| t > Instant::now())
        {
            self.peek_intent.cancel_timer();
        } else {
            self.peek_intent.pointer(
                self.peek_pointer(),
                self.peek_now(),
                peek_intent::Timing::default(),
            );
        }
        self.schedule_peek_timer(cx);
    }

    fn schedule_peek_timer(&mut self, cx: &mut Context<Self>) {
        let pending = self.peek_intent.pending();
        if pending == self.peek_timer {
            return;
        }
        self.peek_timer = pending;
        let Some(timer) = pending else {
            return;
        };
        self.after(
            Duration::from_millis(timer.due_ms.saturating_sub(self.peek_now())),
            cx,
            move |shell, cx| {
                if shell.peek_timer == Some(timer) {
                    shell.peek_timer = None;
                }
                if TIDIED.with(Cell::get) {
                    shell.peek_intent.stop();
                    return;
                }
                let allowed = (shell.permissions.complete() || shell.peek_shown)
                    && !shell.settle_until.is_some_and(|t| t > Instant::now())
                    && (shell.peek_shown || shell.icon_rect().is_some() || shell.stage.is_some());
                let (pointer, now) = (shell.peek_pointer(), shell.peek_now());
                let effect =
                    peek_timer_effect(&mut shell.peek_intent, timer, pointer, now, allowed);
                shell.apply_peek_effect(effect, cx);
                shell.schedule_peek_timer(cx);
            },
        );
    }

    fn apply_peek_effect(&mut self, effect: peek_intent::Effect, cx: &mut Context<Self>) {
        match effect {
            peek_intent::Effect::Show => {
                if !self.show_peek(cx) {
                    self.peek_intent.dismiss();
                    self.peek_timer = None;
                }
            }
            peek_intent::Effect::Hide => self.hide_peek(cx),
            peek_intent::Effect::None => {}
        }
    }

    /// Whether the pointer is on the icon, on Peek, or in the gap between.
    /// Tests on a stage report hover by hand, away from the real pointer.
    fn peek_pointer(&self) -> peek_intent::Pointer {
        if self.stage.is_some() {
            return peek_intent::Pointer {
                icon: self.over_icon,
                peek: self.over_card,
            };
        }
        let at = native::mouse_location();
        let on_icon = self.icon.frame().is_some_and(|f| contains(f, at));
        let on_card = self
            .peek
            .as_ref()
            .filter(|_| self.peek_shown)
            .is_some_and(|(_, w)| {
                // Up to the menu bar: Peek hangs 6 pt below the icon.
                let mut f = peek_card(w);
                f.size.height += 8.0;
                contains(f, at)
            });
        peek_intent::Pointer {
            icon: on_icon,
            peek: on_card,
        }
    }

    /// Position sampling repairs missed AppKit/GPUI exit reports. The model
    /// owns the close deadline and keeps icon/Peek/their gap as one region.
    fn watch_pointer(&mut self, cx: &mut Context<Self>) {
        self.watch_generation += 1;
        let generation = self.watch_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let go_on = this.update(cx, |shell, cx| {
                    if shell.watch_generation != generation || !shell.peek_shown {
                        return false;
                    }
                    shell.hover_changed(cx);
                    true
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
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
    /// Which picture the menu bar icon shows.
    pub fn icon_state(&self) -> &'static str {
        self.icon.state()
    }

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

    fn show_peek(&mut self, cx: &mut Context<Self>) -> bool {
        if self.peek_intent.mode() == peek_intent::Mode::Closed {
            return false;
        }
        let Some((peek, window)) = self.peek.clone() else {
            return false;
        };
        if !self.peek_shown {
            // Ended displays whose windows are all gone close before Peek
            // would show them.
            self.core.update(cx, |core, cx| core.sweep_ended(cx));
        }
        let ids = peek::ordered(self.core.read(cx));
        let Some((screen, _)) = self.host_screen() else {
            return false;
        };
        let (w, h, tile) = Peek::size(ids.len() + self.creating, screen.size.width - 16.0);
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
        let creating = self.creating;
        let _ = peek.update(cx, |peek, _, cx| {
            peek.shown = true;
            peek.active = active;
            peek.tile = tile;
            peek.pending = creating;
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
            let now = peek_card(&window);
            if now.size.width != frame.size.width && now.size.height == frame.size.height {
                self.slide_peek(now, frame, cx);
            } else {
                // A new height jumps: the card fills the window again.
                if now.size.width != frame.size.width {
                    self.peek_slides += 1;
                    self.slide_card = None;
                    let _ = peek.update(cx, |peek, _, _| peek.slide = None);
                }
                native::set_frame_later(&window, outset_card(frame));
            }
        } else {
            self.watch_pointer(cx);
            // It slides 10 pt down out of the menu bar and fades in.
            let guard = self.peek_native_generation.clone();
            guard.set(guard.get().wrapping_add(1));
            let generation = guard.get();
            native::later(move || {
                if guard.get() != generation || TIDIED.with(Cell::get) {
                    return;
                }
                native::set_alpha(&window, 0.0);
                native::set_frame(&window, offset(outset_card(frame), 10.0));
                native::order_front(&window);
                native::animate(&window, outset_card(frame), 1.0, 0.24);
            });
        }
        self.peek_shown = true;
        self.refresh_counts(cx);
        self.sync_card(cx);
        true
    }

    /// A tile grows in or shrinks out, and the card follows it from `from`
    /// to `to`: the window spans both while the card slides inside it,
    /// then takes `to`, where the card then stands.
    fn slide_peek(&mut self, from: NSRect, to: NSRect, cx: &mut Context<Self>) {
        let Some((peek, window)) = self.peek.clone() else {
            return;
        };
        // A slide still going: it starts where the card stands now.
        let from = self.slide_card.unwrap_or(from);
        let left = from.origin.x.min(to.origin.x);
        let right = (from.origin.x + from.size.width).max(to.origin.x + to.size.width);
        let span = rect(left, to.origin.y, right - left, to.size.height);
        self.peek_slides += 1;
        let generation = self.peek_slides;
        self.slide_card = Some(to);
        let slide = peek::Slide {
            from: ((from.origin.x - left) as f32, from.size.width as f32),
            to: ((to.origin.x - left) as f32, to.size.width as f32),
            started: std::time::Instant::now(),
        };
        let _ = peek.update(cx, |peek, _, cx| {
            peek.slide = Some(slide);
            cx.notify();
        });
        native::set_frame_later(&window, outset_card(span));
        self.after(
            peek::RESIZE + Duration::from_millis(30),
            cx,
            move |shell, _| {
                if shell.peek_slides != generation || !shell.peek_shown {
                    return;
                }
                shell.slide_card = None;
                // Peek draws the card where the slide left it until the
                // window has this frame, then fills the window.
                if let Some((_, window)) = shell.peek.clone() {
                    native::set_frame_later(&window, outset_card(to));
                }
            },
        );
    }

    pub fn hide_peek(&mut self, cx: &mut Context<Self>) {
        self.peek_intent.dismiss();
        self.peek_timer = None;
        self.peek_native_generation
            .set(self.peek_native_generation.get().wrapping_add(1));
        // A slide going stops where it is; the next show sizes afresh.
        self.peek_slides += 1;
        self.slide_card = None;
        if let Some((peek, _)) = &self.peek {
            let _ = peek.update(cx, |peek, _, _| {
                peek.keys = false;
                peek.focused = None;
                peek.slide = None;
                peek.remind = None;
            });
        }
        // Gone from under the pointer, which GPUI won't report.
        self.over_card = false;
        if !self.peek_shown {
            return;
        }
        self.peek_shown = false;
        self.sync_card(cx);
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
        let card = peek_card(window);
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
        // A tile says so at once, whatever macOS takes.
        self.creating += 1;
        if self.peek_shown {
            self.show_peek(cx);
        }
        // A spare of the size, or a new display: macOS takes about half a
        // second for that, off the main thread, so Peek keeps drawing.
        let spare = self
            .core
            .update(cx, |core, _| core.take_spare(DEFAULT_SIZE));
        // A new display stops every window on the Mac from drawing for a
        // moment: start it once the waiting tile has grown in.
        let grown = peek::RESIZE + Duration::from_millis(40);
        let (made, name) = match spare {
            Some((handle, name, false)) => (gpui::Task::ready(Some(handle)), name),
            // A spare of another size, resized on the display thread.
            Some((mut handle, name, true)) => (
                cx.background_spawn({
                    let executor = cx.background_executor().clone();
                    async move {
                        executor.timer(grown).await;
                        let (w, h) = (DEFAULT_SIZE.0 as u32, DEFAULT_SIZE.1 as u32);
                        crate::mac::display::configure(move || {
                            handle.resize(w, h).then_some(handle)
                        })
                        .await
                    }
                }),
                name,
            ),
            None => {
                let (serial, name) = self.core.update(cx, |core, _| core.next_name());
                let others = self.core.read(cx).virtual_ids();
                let executor = cx.background_executor().clone();
                let made = cx.background_spawn({
                    let name = name.clone();
                    async move {
                        executor.timer(grown).await;
                        crate::mac::display::create_parked(
                            name,
                            DEFAULT_SIZE.0 as u32,
                            DEFAULT_SIZE.1 as u32,
                            serial,
                            others,
                        )
                        .await
                    }
                });
                (made, name)
            }
        };
        cx.spawn(async move |shell, cx| {
            let made = made.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.creating = shell.creating.saturating_sub(1);
                let Some(handle) = made else {
                    beep();
                    if shell.peek_shown {
                        shell.show_peek(cx);
                    }
                    return;
                };
                let id = shell.core.update(cx, |core, cx| {
                    core.adopt(handle, name, DEFAULT_SIZE, None, cx)
                });
                // Its tile takes the waiting one's place.
                if let Some((peek, _)) = &shell.peek {
                    let creating = shell.creating;
                    let _ = peek.update(cx, |peek, _, _| {
                        peek.arrived.insert(id);
                        peek.pending = creating;
                    });
                }
                // Peek stays up and grows a tile; the Viewer opens on the
                // new display, or switches to it when already open.
                shell.after(Duration::from_millis(500), cx, move |shell, cx| {
                    shell.open_viewer(id, cx)
                });
            });
        })
        .detach();
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
            native::set_frame_later(&window, frame);
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
            let shown = window.clone();
            native::later(move || {
                native::set_alpha(&shown, 0.0);
                native::set_frame(&shown, fit_into(frame, from));
                native::order_front(&shown);
                native::animate_frame(&shown, frame, 0.45, native::Curve::Spring);
                native::animate_alpha(&shown, 1.0, 0.16, native::Curve::Settle);
            });
            self.set_morphing(true, cx);
            let generation = self.viewer_generation;
            self.after(Duration::from_millis(450), cx, move |shell, cx| {
                if shell.viewer_generation == generation {
                    shell.set_morphing(false, cx);
                }
            });
        }
        self.viewer_open = true;
        self.sync_card(cx);
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
        self.sync_card(cx);
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
            // Held open, as a click holds it, so scripts can look at it.
            "peek" => {
                let effect = self.peek_intent.pin();
                self.apply_peek_effect(effect, cx);
            }
            "recordings" => self.open_recordings(cx),
            "recordings-close" => {
                if let Some(handle) = self.recordings.take() {
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    });
                }
                if self.setup.is_none() {
                    leave_dock(cx);
                }
            }
            "setup" | "settings" => self.open_setup(cx),
            "setup-close" => self.close_setup(cx),
            // The open setup window as with both permissions missing, the
            // first row explaining what to do in System Settings: the
            // tallest it gets.
            "setup-waiting" => {
                self.setup_preview = true;
                self.push_setup(cx);
            }
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
            // The Help Card's buttons, as a click would press them.
            "card-later" => self.card_later(id, cx),
            "card-take-over" => self.open_for_help(id, cx),
            "card-open" => self.card_open(id, cx),
            "card-give" => {
                self.answer_window(id, true, cx);
            }
            "card-keep" => {
                self.answer_window(id, false, cx);
            }
            "card-continue" => {
                self.answer_control(id, true, cx);
            }
            "card-keep-control" => {
                self.answer_control(id, false, cx);
            }
            "connect" => self.menu(MenuAction::ConnectAgents, cx),
            "wallpaper" => self.choose_wallpaper(cx),
            "click" => self.native(
                Native::IconClicked {
                    right: false,
                    mouse: false,
                },
                cx,
            ),
            // The pointer entering (1) or leaving (0) the icon or Peek, as
            // AppKit and GPUI report it.
            "icon-hover" => self.native(Native::IconHover(id != 0), cx),
            "card-hover" => self.card_hover(id != 0, cx),
            "menu" => self.status_menu(),
            "hide" => self.hide_peek(cx),
            "collapse" => self.close_viewer(cx),
            "create" => self.create_display(cx),
            "dismiss" => self.dismiss(id, cx),
            // The Dismiss sheet's confirm, with the plan as it stands.
            "dismiss-confirm" => {
                let items = self
                    .core
                    .read(cx)
                    .displays
                    .get(id)
                    .map(dismissal::plan)
                    .ok_or("no such display")?;
                self.dismiss_now(id, items, cx);
            }
            "primary" => self.primary(id, cx),
            "give" => self.window_menu(id, WindowMenu::Give, None, cx),
            "take-back" => self.window_menu(id, WindowMenu::TakeBack, None, cx),
            "help" => self.open_for_help(id, cx),
            "answer" => {
                let Some((viewer, _)) = self.viewer.clone() else {
                    return Err("no Viewer".into());
                };
                let _ = viewer.update(cx, |viewer, _, cx| viewer.answer(id != 0, cx));
            }
            "focus" => self.take_keyboard(),
            "viewer" => self.open_viewer(id, cx),
            "stage" => {
                #[cfg(debug_assertions)]
                if let Some(old) = self.stage {
                    cg::clear_user_stage(old);
                }
                self.stage = (id != 0).then_some(id);
            }
            "select" => self.select(id, cx),
            "quit" => self.quit(cx),
            other => return Err(format!("unknown surface {other}")),
        }
        Ok(())
    }

    // MARK: Menus

    fn status_menu(&self) {
        let menu = native::Menu::new("Agent Screen");
        let missing = self.permissions.missing();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|p| p.name()).collect();
            menu.action(
                &tf!(
                    "Finish Setup: Allow {}…",
                    "完成设置：还差{}…",
                    names.join(t(" and ", "、"))
                ),
                MenuAction::OpenSettings,
            );
            menu.separator();
        } else if self.restart {
            menu.action(
                t(
                    "Finish Setup: Reopen Agent Screen…",
                    "完成设置：重新打开 Agent Screen…",
                ),
                MenuAction::OpenSettings,
            );
            menu.separator();
        }
        // A workspace on an agent's display is gone from the user's screen:
        // that can't wait for Settings.
        if !self.conflicts.is_empty() {
            let workspaces: Vec<&str> = self.conflicts.iter().map(|(w, _)| w.as_str()).collect();
            let fix = menu.action(
                &tf!(
                    "AeroSpace Put Workspace {} on an Agent Display…",
                    "AeroSpace 把工作区 {} 放到了 Agent 显示器上…",
                    workspaces.join(", ")
                ),
                MenuAction::OpenAeroSpaceConfig,
            );
            fix.setToolTip(Some(&objc2_foundation::NSString::from_str(t(
                "Agent displays take monitor numbers in workspace-to-monitor-force-assignment. Use monitor names instead, like 2 = 'DELL'.",
                "Agent 显示器会占用 workspace-to-monitor-force-assignment 里的显示器编号。改用显示器名称，例如 2 = 'DELL'。",
            ))));
            menu.separator();
        }
        // Setup worked on the agent's side too: its session has the tools.
        match self.agents_ready() {
            Some(agent) => menu.disabled(&tf!(
                "{agent} loaded Agent Screen's tools",
                "{agent} 已加载 Agent Screen 的工具"
            )),
            None => menu.disabled(t(
                "No agent has loaded the tools yet",
                "还没有 agent 加载工具",
            )),
        };
        menu.separator();
        // What to do with displays and their videos, then the app itself.
        let create = menu.action(t("New Display", "新建显示器"), MenuAction::CreateDisplay);
        native::set_key_equivalent(&create, "n", true, false, false, false);
        menu.action(t("Recordings…", "录制回看…"), MenuAction::OpenRecordings);
        menu.separator();
        // Agents, wallpaper, AeroSpace and the optional helpers.
        let settings = menu.action(t("Settings…", "设置…"), MenuAction::OpenSettings);
        native::set_key_equivalent(&settings, ",", true, false, false, false);
        menu.action(
            t("Report a Problem…", "报告问题…"),
            MenuAction::ReportProblem,
        );
        if let Some(version) = update::pending() {
            menu.action(
                &tf!("Restart to Update to {version}…", "重启以更新到 {version}…"),
                MenuAction::InstallUpdate,
            );
        } else if update::available() {
            menu.action(
                t("Check for Updates…", "检查更新…"),
                MenuAction::CheckForUpdates,
            );
        }
        menu.separator();
        let quit = menu.action(
            t("Quit Agent Screen", "退出 Agent Screen"),
            MenuAction::Quit,
        );
        native::set_key_equivalent(&quit, "q", true, false, false, false);
        self.icon.pop_up(menu.into_native());
    }

    /// One of the Viewer header's menus: the user's windows to give the
    /// agent, the display's windows to take back, or closing the display.
    /// `button` is where its button is in the Viewer; the menu drops from
    /// its bottom-left corner, or from the pointer without one.
    pub fn window_menu(
        &mut self,
        id: DisplayId,
        which: WindowMenu,
        button: Option<gpui::Bounds<gpui::Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.read(cx);
        if core.displays.get(id).is_none() {
            return;
        }
        let menu = native::Menu::new("");
        match which {
            WindowMenu::Give => {
                let agent_screens: Vec<Rect> = core
                    .displays
                    .list
                    .iter()
                    .map(|d| cg::display_bounds(d.id))
                    .collect();
                for window in windows::all()
                    .into_iter()
                    .filter(|w| !agent_screens.iter().any(|s| s.intersects(&w.bounds)))
                {
                    let item = menu.action(
                        &name(&window.app, &window.title),
                        MenuAction::HandOver {
                            display: id,
                            pid: window.target.pid,
                            window: window.target.window_id,
                        },
                    );
                    native::set_app_icon(&item, window.target.pid);
                }
                if menu.is_empty() {
                    menu.disabled(t("No Windows on Your Desktop", "你的桌面上没有窗口"));
                }
            }
            WindowMenu::TakeBack => {
                for window in windows::on_display(id) {
                    let item = menu.action(
                        &name(&window.app, &window.title),
                        MenuAction::TakeBack {
                            display: id,
                            pid: window.target.pid,
                            window: window.target.window_id,
                        },
                    );
                    native::set_app_icon(&item, window.target.pid);
                }
                if menu.is_empty() {
                    menu.disabled(t("No Windows on This Display", "这块显示器上没有窗口"));
                }
            }
        }
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
            MenuAction::OpenSettings => self.open_setup(cx),
            MenuAction::OpenRecordings => self.open_recordings(cx),
            MenuAction::ReportProblem => self.report_problem(cx),
            MenuAction::CheckForUpdates => update::check(),
            MenuAction::InstallUpdate => self.install_update(cx),
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
                        "Your AeroSpace config assigns workspace {} to a monitor by number, in workspace-to-monitor-force-assignment. Agent displays count as monitors and take those numbers, so the workspace lands on an agent's display and Agent Screen can't move it. Use the monitor's name instead, like 2 = 'DELL', or install Agent Screen's AeroSpace and add ignored-monitors.",
                        "你的 AeroSpace 配置在 workspace-to-monitor-force-assignment 里按编号把工作区 {} 分给了某块显示器。Agent 显示器也算显示器，会占用这些编号，所以这个工作区落到了 Agent 的显示器上，Agent Screen 挪不动它。改用显示器名称，例如 2 = 'DELL'；或者安装 Agent Screen 版 AeroSpace 并加上 ignored-monitors。",
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
            MenuAction::ChooseWallpaper => self.choose_wallpaper(cx),
            MenuAction::DefaultWallpaper => self.set_wallpaper(None, cx),
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
            MenuAction::PressProxy { index } => {
                let Some(element) = menubar::chosen(index) else {
                    return;
                };
                // A menu command acts on the app's key window: make that the
                // display's window of the app first, not the user's own in a
                // shared app. Leaving the Viewer hands it back.
                let pid = self.viewer.as_ref().and_then(|(viewer, _)| {
                    viewer.read(cx).ok()?.menu_bar.as_ref().map(|bar| bar.pid)
                });
                let forwarder = self
                    .viewer_display
                    .map(|id| (id, self.core.update(cx, |core, _| core.forwarder(id))));
                cx.background_spawn(async move {
                    let (Some(pid), Some((id, forwarder))) = (pid, forwarder) else {
                        return;
                    };
                    if forwarder
                        .focus_app(pid, crate::mac::cg::display_bounds(id))
                        .await
                    {
                        element.perform("AXPress");
                    } else {
                        log::info!("menu strip: no window of the app on the display, not pressed");
                    }
                })
                .detach();
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
            let asking = cx
                .background_executor()
                .spawn(async move { dismissal::execute(&items, main_name.as_deref()) })
                .await;
            // A window asking whether to save keeps the display: the
            // question stays on it, in the Viewer, instead of landing on the
            // user's screen once the display goes. The display goes once
            // those windows have closed. One left open without a sheet, as
            // after Cancel, stays on the display, which waits for review.
            if !asking.is_empty() {
                let _ = this.update(cx, |shell, cx| shell.open_viewer(id, cx));
                let mut calm = 0;
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let left = asking.clone();
                    let (open, sheets) = cx
                        .background_executor()
                        .spawn(async move {
                            let open: Vec<WindowTarget> = left
                                .into_iter()
                                .filter(|t| windows::find(t.window_id).is_some())
                                .collect();
                            let sheets = open.iter().any(|&t| crate::mac::ax::has_sheet(t));
                            (open, sheets)
                        })
                        .await;
                    if open.is_empty() {
                        break;
                    }
                    calm = if sheets { 0 } else { calm + 1 };
                    if calm >= 6 {
                        log::info!(
                            "display {id}: a window stayed open without asking; keeping the display"
                        );
                        return;
                    }
                }
            }
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
            // later windows stay on the user's screen, even one it opens
            // beside a window of its own still on the display.
            core.kept_catches.retain(|(_, k)| k.pid() != target.pid);
            core.pending_returns.insert((id, target.pid));
            home
        });
        // Keys typed in the Viewer must not follow the window home.
        let forwarder = self.core.update(cx, |core, _| core.forwarder(id));
        cx.background_spawn(async move { forwarder.forget_target(target).await })
            .detach();
        let main_name = main_screen_name();
        // The app's new windows go to the screen this window came from.
        let screen = home
            .as_ref()
            .and_then(|h| {
                crate::mac::cg::active_displays()
                    .into_iter()
                    .map(crate::mac::cg::display_bounds)
                    .find(|b| b.contains(h.frame.mid()))
            })
            .unwrap_or_else(|| crate::mac::cg::display_bounds(crate::mac::cg::main_display()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { dismissal::give_back(target, home.as_ref(), main_name.as_deref()) })
                .await;
            // Armed once the window is back, on the catch thread:
            // registering asks the app, which may be slow.
            let Some(catch) = crate::mac::catch::Catch::returning(target.pid, id, screen).await
            else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                shell.core.update(cx, |core, _| {
                    // Still wanted: no Handover of the app and the display
                    // still there. Otherwise the catch drops here.
                    let wanted = core.pending_returns.remove(&(id, target.pid))
                        && !core
                            .returning
                            .iter()
                            .any(|(d, k)| *d == id && k.pid() == target.pid);
                    if wanted {
                        core.returning.push((id, catch));
                    }
                });
                shell.refresh_counts(cx)
            });
        })
        .detach();
    }

    /// Quit. The tidy-up runs in the quit hook, so logout gets it too.
    fn quit(&mut self, cx: &mut Context<Self>) {
        self.hide_peek(cx);
        self.peek_intent.stop();
        cx.quit();
    }

    // MARK: Permissions

    /// The official bug report: with the user's consent, put this app's
    /// read-only diagnostics on the clipboard and open the issue form. It
    /// reads only build and connection metadata, starts nothing and uploads
    /// nothing; the user pastes and submits it.
    /// Restart to Update: with Agent Displays open, the user confirms
    /// first, as the restart ends them.
    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(version) = update::pending() else {
            return;
        };
        let open = self.core.read(cx).displays.list.len();
        if open > 0
            && !native::confirm(
                &tf!(
                    "Update to Agent Screen {version}?",
                    "更新到 Agent Screen {version}？"
                ),
                &tf!(
                    "Agent Screen restarts to update, which ends the {open} Agent Displays open now and the agents' work on them. Otherwise the update installs by itself once no Agent Display is open, or when you quit.",
                    "Agent Screen 更新时要重启，会结束现在打开的 {open} 块 Agent 显示器，agent 在上面的工作也会中断。不更新的话，等没有 Agent 显示器打开时，或者你退出时，更新会自动安装。"
                ),
                t("Restart and Update", "重启并更新"),
            )
        {
            self.watch_updates(cx);
            return;
        }
        update::install();
    }

    /// A downloaded update installs once no Agent Display has been open for
    /// a while: the restart ends none, and agents' MCP servers reconnect.
    fn watch_updates(&mut self, cx: &mut Context<Self>) {
        if self.watching_updates {
            return;
        }
        self.watching_updates = true;
        let every = Duration::from_secs(
            std::env::var("AGENTSCREEN_UPDATE_IDLE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|_| {
                    cfg!(debug_assertions) || std::env::var_os("AGENTSCREEN_DEBUG").is_some()
                })
                .unwrap_or(30),
        );
        cx.spawn(async move |this, cx| {
            let mut idle = 0;
            loop {
                cx.background_executor().timer(every).await;
                let Ok(open) = this.update(cx, |shell, cx| shell.core.read(cx).displays.list.len())
                else {
                    return;
                };
                update::set_open_displays(open);
                if update::pending().is_none() {
                    let _ = this.update(cx, |shell, _| shell.watching_updates = false);
                    return;
                }
                idle = if open == 0 { idle + 1 } else { 0 };
                // Twice in a row, so a display that is being made isn't cut off.
                if idle >= 2 {
                    update::install();
                    return;
                }
            }
        })
        .detach();
    }

    fn report_problem(&mut self, cx: &mut Context<Self>) {
        let go = native::confirm(
            t("Report a Problem", "报告问题"),
            t(
                "Agent Screen copies its diagnostics to the clipboard and opens the bug report form on GitHub. They name this build and whether the app answered, with no screens, recordings, window titles or account details. Paste them into the form. For a problem in an agent session, also ask that agent to run get_diagnostics.",
                "Agent Screen 会把诊断信息复制到剪贴板，并在 GitHub 打开问题报告表单。诊断只包含当前构建和 app 是否响应，不含画面、录制、窗口标题或账号信息。请粘贴到表单里。如果问题出在某个 agent 会话里，也请让那个 agent 运行 get_diagnostics。",
            ),
            t("Copy Diagnostics and Open Form", "复制诊断并打开表单"),
        );
        if !go {
            return;
        }
        cx.spawn(async move |_, cx| {
            // A separate read-only connection to this app's own socket,
            // off the main thread that answers it.
            let report = cx
                .background_executor()
                .spawn(async move {
                    agent_screen_mcp::diagnostics::cli(
                        &["--json".into()],
                        env!("CARGO_PKG_VERSION"),
                    )
                })
                .await;
            let text = match report {
                Ok(report) => report,
                Err(error) => format!("diagnostics unavailable: {error}"),
            };
            native::copy(&text);
            let _ = std::process::Command::new("/usr/bin/open")
                .arg(REPORT_FORM)
                .spawn();
        })
        .detach();
    }

    fn open_recordings(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.recordings
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            cx.activate(true);
            return;
        }
        cx.set_activation_policy(gpui::ActivationPolicy::Regular);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1060.), px(680.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(t("Recordings", "录制回看").into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(720.), px(440.))),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            ..Default::default()
        };
        match cx.open_window(options, |_, cx| cx.new(Recordings::new)) {
            Ok(handle) => {
                self.recordings = Some(handle);
                cx.activate(true);
            }
            Err(error) => log::warn!("recordings window: {error}"),
        }
    }

    pub fn open_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.setup
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            cx.activate(true);
            return;
        }
        // In the Dock and the app switcher while the window is open, so it
        // can be found behind other windows.
        cx.set_activation_policy(gpui::ActivationPolicy::Regular);
        let weak = cx.entity().downgrade();
        let status = self.permissions;
        let bounds = Bounds::centered(None, size(px(setup::WIDTH), px(setup::HEIGHT)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(t("Agent Screen Settings", "Agent Screen 设置").into()),
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
                self.check_aerospace(cx);
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
                    agent_screen_mcp::install::registered(&exe)
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
            // After this update: closing it tells `window_closed`, which
            // needs the shell.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        self.waiting = None;
        self.setup_preview = false;
        assist::hide();
        if self.recordings.is_none() {
            leave_dock(cx);
        }
    }

    /// The setup window closed with its close button.
    fn window_closed(&mut self, id: gpui::WindowId, cx: &mut Context<Self>) {
        if self.peek.as_ref().is_some_and(|(h, _)| h.window_id() == id) {
            self.peek_intent.dismiss();
            self.peek_timer = None;
            self.peek_native_generation
                .set(self.peek_native_generation.get().wrapping_add(1));
            self.peek_generation += 1;
            self.watch_generation += 1;
            self.peek_shown = false;
            self.over_card = false;
            self.peek = None;
            for id in self.known.iter().copied() {
                self.streams
                    .update(cx, |s, cx| s.want(id, "peek", 0, (0, 0), cx));
            }
            self.sync_card(cx);
        }
        if self.setup.is_some_and(|h| h.window_id() == id) {
            self.setup = None;
            self.waiting = None;
            assist::hide();
            if self.recordings.is_none() {
                leave_dock(cx);
            }
        }
        if self.recordings.is_some_and(|h| h.window_id() == id) {
            self.recordings = None;
            if self.setup.is_none() {
                leave_dock(cx);
            }
        }
    }

    /// macOS said whether Agent Screen may notify. People who finished setup
    /// before notifications existed get macOS's prompt once, at launch.
    fn notification_auth(&mut self, auth: notify::Auth, cx: &mut Context<Self>) {
        let first = self.notifications == notify::Auth::Unknown;
        if auth == self.notifications {
            return;
        }
        self.notifications = auth;
        if first && auth == notify::Auth::NotAsked && self.permissions.complete() {
            notify::request();
        }
        self.push_setup(cx);
    }

    /// Ask for a picture for Agent Displays with an Open panel.
    pub fn choose_wallpaper(&mut self, cx: &mut Context<Self>) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let panel = objc2_app_kit::NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setMessage(Some(&objc2_foundation::NSString::from_str(t(
            "Choose a picture for agents' displays. You see it in the Viewer and in recordings; agents' screenshots leave it out.",
            "为 agent 的显示器选一张壁纸。它出现在 Viewer 和录屏里，agent 截图时不包含它。",
        ))));
        let types: Vec<objc2::rc::Retained<objc2_foundation::NSString>> =
            ["png", "jpg", "jpeg", "heic", "tiff", "gif", "webp"]
                .iter()
                .map(|e| objc2_foundation::NSString::from_str(e))
                .collect();
        let types = objc2_foundation::NSArray::from_retained_slice(&types);
        // allowedContentTypes needs UTType; the file-type list still works.
        let _: () = unsafe { objc2::msg_send![&panel, setAllowedFileTypes: &*types] };
        cx.activate(true);
        let modal = crate::mac::hang::Modal::open();
        let chosen = panel.runModal();
        drop(modal);
        if chosen != objc2_app_kit::NSModalResponseOK {
            return;
        }
        let Some(path) = panel.URL().and_then(|url| url.path()) else {
            return;
        };
        self.set_wallpaper(Some(std::path::PathBuf::from(path.to_string())), cx);
    }

    /// Keep `picture` for Agent Displays, or Agent Screen's own with None,
    /// and show it on the displays there are.
    pub fn set_wallpaper(&mut self, picture: Option<std::path::PathBuf>, cx: &mut Context<Self>) {
        if let Err(error) = crate::mac::app::set_wallpaper(picture.as_deref()) {
            log::warn!("wallpaper: {error}");
            return;
        }
        let ids = self.core.read(cx).virtual_ids();
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            for id in ids {
                crate::mac::app::label_display(id, mtm);
            }
        }
        log::info!(
            "Agent Display wallpaper: {}",
            picture.map_or("Agent Screen's".into(), |p| p.display().to_string())
        );
        self.push_setup(cx);
    }

    /// The setup window's button for the ColorSync helper: macOS's
    /// administrator prompt, then a run (ADR 0009).
    pub fn set_up_colorsync(&mut self, cx: &mut Context<Self>) {
        if self.colorsync_busy {
            return;
        }
        self.colorsync_busy = true;
        self.colorsync_error = None;
        self.push_setup(cx);
        let prompt = t(
            "Agent Screen wants to keep macOS's list of displays short, so your Mac doesn't stutter when displays change.",
            "Agent Screen 要精简 macOS 的显示器列表，让 Mac 在显示器变化时不卡顿。",
        );
        cx.spawn(async move |shell, cx| {
            let (result, helper) = cx
                .background_executor()
                .spawn(async move {
                    let result = crate::mac::colorsync::set_up(prompt);
                    (result, crate::mac::colorsync::helper())
                })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.colorsync_busy = false;
                shell.colorsync = Some(helper);
                match result {
                    Ok(()) => {
                        crate::lifecycle::line("colorsync helper set up");
                        shell.core.update(cx, |core, cx| core.reclaim_soon(cx));
                    }
                    Err(error) if error == "cancelled" => {}
                    Err(error) => {
                        log::warn!("setting up the ColorSync helper: {error}");
                        shell.colorsync_error = Some(error);
                    }
                }
                shell.push_setup(cx);
            });
        })
        .detach();
    }

    /// The setup window's button for notifications: macOS's prompt, or its
    /// Notifications pane once the user has answered.
    pub fn allow_notifications(&mut self) {
        match self.notifications {
            notify::Auth::Denied => {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("x-apple.systempreferences:com.apple.Notifications-Settings.extension")
                    .spawn();
            }
            _ => notify::request(),
        }
    }

    /// The user clicked 开启… for `p`: the system prompt when macOS still
    /// shows one, else the pane, with the helper pinned to it.
    pub fn grant(&mut self, p: Permission, cx: &mut Context<Self>) {
        permissions::request(p);
        // Asking puts Agent Screen in the list, switched off.
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
            agent_screen_mcp::install::install(&exe)
        });
        cx.spawn(async move |shell, cx| {
            use agent_screen_mcp::install::Done;
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
                    "For another MCP client, add a stdio server with command\n{}\nand arguments [\"mcp\"]. If Claude Code is installed, run in Terminal:\n\"{}\" mcp install",
                    "其他 MCP 客户端：添加一个 stdio 服务器，命令是\n{}\n参数是 [\"mcp\"]。装了 Claude Code 的话，在终端里运行：\n\"{}\" mcp install",
                    exe.display(),
                    exe.display()
                )
            } else if failed {
                tf!(
                    "To try in Terminal, where your agents' PATH is set:\n\"{}\" mcp install",
                    "可以在终端里重试，那里有 agent 需要的 PATH：\n\"{}\" mcp install",
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
                .background_spawn(async move { agent_screen_mcp::install::registered(&exe) })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.connecting = false;
                shell.registered = registered;
                shell.push_setup(cx);
            });
            native::alert(
                t("Agent Screen and your agents", "Agent Screen 与 agent"),
                &format!("{}\n\n{next}", lines.join("\n")),
            );
        })
        .detach();
    }

    pub fn last_agent_ready(&self) -> Option<String> {
        self.agents_ready()
    }

    /// The agents that loaded the tools since Agent Screen started, by name.
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
        let (status, waiting, restart) = if self.setup_preview {
            let none = permissions::Status {
                screen: false,
                accessibility: false,
            };
            (none, Some(Permission::ScreenRecording), false)
        } else {
            (self.permissions, self.waiting, self.restart)
        };
        let agent = self.agents_ready();
        let connecting = self.connecting;
        let Some(handle) = self.setup else { return };
        let registered = self.registered.clone();
        let notifications = self.notifications;
        let colorsync = (
            self.colorsync,
            self.colorsync_busy,
            self.colorsync_error.clone(),
        );
        let aerospace = self.aerospace;
        let pinned: Vec<String> = self.conflicts.iter().map(|(w, _)| w.clone()).collect();
        let wallpaper = crate::mac::app::custom_wallpaper();
        if handle
            .update(cx, |setup, _, cx| {
                setup.aerospace = aerospace;
                setup.pinned = pinned;
                setup.wallpaper = wallpaper;
                setup.status = status;
                setup.notifications = notifications;
                (setup.colorsync, setup.colorsync_busy, setup.colorsync_error) = colorsync;
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

    fn check_permissions(
        &mut self,
        now: permissions::Status,
        settings: Option<assist::Settings>,
        cx: &mut Context<Self>,
    ) {
        if let Some(settings) = settings
            && assist::is_shown()
            && !assist::tick(settings)
        {
            // System Settings quit.
            assist::hide();
        }
        let before = self.permissions;
        if self.setup.is_some() {
            // The user may answer in System Settings.
            notify::check();
        }
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

    // MARK: Launcher

    /// The user typed in a launcher such as Raycast while working in the
    /// Viewer: its panel takes keys without coming to the front, so Agent
    /// Screen still is. The first new window in the next 10 s, and the
    /// windows its app makes in the 2 s after it, go to the Viewer's
    /// display as a Handover (ADR 0007). Typed in after a click on their
    /// own screen, the launcher's apps open there.
    /// An app the agent started made itself the front app, as Electron does
    /// to show a new window: unless the user just acted on it, their app
    /// goes back to the front (issue #67, ADR 0001). A press in the Viewer
    /// makes Agent Screen the front app first, so the Viewer is left alone.
    /// The user brought `pid` to the front, and every window it has is on
    /// Agent Displays, where they can't see it: a notification names those
    /// displays, and a click on it opens the first in the Viewer (#51). At
    /// most once a minute for an app. The displays named.
    ///
    /// `docked`: the user clicked the app's Dock icon, which says they want
    /// it even when it was already the front app and nothing activated
    /// (#51): told again after 5 s.
    pub fn remind_where(
        &mut self,
        pid: i32,
        docked: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Vec<DisplayId>> {
        let again = Duration::from_secs(if docked { 5 } else { 60 });
        let core = self.core.read(cx);
        let places: Vec<(DisplayId, Rect, String)> = core
            .displays
            .list
            .iter()
            .map(|d| (d.id, cg::display_bounds(d.id), d.label()))
            .collect();
        // An app the agent started brings itself to the front, as Electron
        // does: no one to tell, unless the user just used it.
        let agents = core.displays.list.iter().any(|d| d.started.contains(&pid));
        let recent = self
            .reminded
            .get(&pid)
            .is_some_and(|at| at.elapsed() < again);
        if places.is_empty() || recent || pid == std::process::id() as i32 {
            return gpui::Task::ready(Vec::new());
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    if agents && !docked && !crate::mac::catch::user_acted_recently(pid) {
                        return None;
                    }
                    // Windows on screen: an app keeps windows it never
                    // shows, which count for nothing here.
                    let windows: Vec<_> = cg::on_screen_windows()
                        .into_iter()
                        .filter(|w| {
                            w.target.pid == pid
                                && w.layer == 0
                                && w.bounds.width > 40.0
                                && w.bounds.height > 40.0
                        })
                        .collect();
                    let app = windows.first()?.owner_name.clone();
                    let on: Vec<Option<DisplayId>> = windows
                        .iter()
                        .map(|w| {
                            places
                                .iter()
                                .find(|(_, area, _)| area.contains(w.bounds.mid()))
                                .map(|(id, _, _)| *id)
                        })
                        .collect();
                    let ids = agent_screen_model::model::only_on_agent_displays(&on);
                    let labels: Vec<String> = ids
                        .iter()
                        .filter_map(|id| places.iter().find(|p| p.0 == *id))
                        .map(|p| p.2.clone())
                        .collect();
                    // The windows on the first of them, in its points.
                    let first = ids.first().and_then(|id| places.iter().find(|p| p.0 == *id));
                    let frames: Vec<(WindowTarget, Rect)> = first
                        .map(|(_, area, _)| {
                            windows
                                .iter()
                                .filter(|w| area.contains(w.bounds.mid()))
                                .map(|w| {
                                    let b = w.bounds;
                                    (
                                        w.target,
                                        Rect::new(b.x - area.x, b.y - area.y, b.width, b.height),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some((app, ids, labels, frames))
                })
                .await;
            let Some((app, ids, places, frames)) =
                found.filter(|(_, ids, _, _)| !ids.is_empty())
            else {
                return Vec::new();
            };
            let shown = this
                .update(cx, |shell, cx| {
                    shell.reminded.insert(pid, Instant::now());
                    shell.remind_in_peek(ids[0], app.clone(), frames, cx)
                })
                .unwrap_or(false);
            log::info!("{app} came to the front with its windows only on displays {ids:?}; telling the user");
            if shown {
                return ids;
            }
            // Peek can't come down, as before setup is done: a notification.
            let first = ids[0];
            notify::withdraw_where(first);
            notify::update(first, "", "", Some(Note::Elsewhere { app, places }));
            ids
        })
    }

    /// Peek comes down to show where `app`'s windows are: display `id`'s
    /// tile glows, its windows outlined, and goes after a few seconds
    /// unless the pointer comes onto it. False when Peek can't show.
    fn remind_in_peek(
        &mut self,
        id: DisplayId,
        app: String,
        windows: Vec<(WindowTarget, Rect)>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.permissions.complete() || self.peek_intent.stopped() {
            return false;
        }
        let Some((peek, _)) = self.peek.clone() else {
            return false;
        };
        self.remind_seq += 1;
        let remind = peek::Remind {
            display: id,
            app,
            windows,
            seq: self.remind_seq,
        };
        let _ = peek.update(cx, |peek, _, cx| {
            peek.remind = Some(remind);
            cx.notify();
        });
        let effect = self
            .peek_intent
            .remind(self.peek_now(), peek_intent::REMIND_MS);
        self.apply_peek_effect(effect, cx);
        self.schedule_peek_timer(cx);
        if !self.peek_shown {
            let _ = peek.update(cx, |peek, _, _| peek.remind = None);
        }
        self.peek_shown
    }

    /// What Peek shows of a reminder now, for scripts: the display, the
    /// app and how many of its windows are outlined.
    pub fn reminder(&self, cx: &App) -> Option<serde_json::Value> {
        if !self.peek_shown {
            return None;
        }
        let (peek, _) = self.peek.as_ref()?;
        let peek = peek.read(cx).ok()?;
        let remind = peek.remind.as_ref()?;
        Some(serde_json::json!({
            "displayId": remind.display,
            "app": remind.app,
            "windows": remind.windows.len(),
        }))
    }

    /// Take Back on a reminder: the app's windows come to the user's
    /// screen, and Peek goes.
    pub fn take_back_reminded(
        &mut self,
        id: DisplayId,
        targets: Vec<WindowTarget>,
        cx: &mut Context<Self>,
    ) {
        for target in targets {
            self.take_back(target, id, cx);
        }
        self.hide_peek(cx);
    }

    /// A left click of the user's outside Agent Screen: on an app's Dock
    /// icon, where its windows are. The app may already be the front app,
    /// so no activation says so.
    pub fn user_clicked(
        &mut self,
        at: (f64, f64),
        cx: &mut Context<Self>,
    ) -> gpui::Task<Vec<DisplayId>> {
        if self.core.read(cx).displays.list.is_empty() {
            return gpui::Task::ready(Vec::new());
        }
        cx.spawn(async move |this, cx| {
            let pids = cx
                .background_executor()
                .spawn(async move {
                    let dock = crate::mac::app::dock_pid()?;
                    let name = crate::mac::ax::dock_app_at(dock, at)?;
                    Some(crate::mac::app::pids_named(&name))
                })
                .await
                .unwrap_or_default();
            let mut told = Vec::new();
            for pid in pids {
                let Ok(task) = this.update(cx, |shell, cx| shell.remind_where(pid, true, cx))
                else {
                    break;
                };
                told.extend(task.await);
            }
            told
        })
    }

    fn activated(&mut self, pid: i32, previous: Option<i32>, cx: &mut Context<Self>) {
        self.remind_where(pid, false, cx).detach();
        let own = std::process::id() as i32;
        let Some(previous) = previous.filter(|&p| p != own && p != pid) else {
            return;
        };
        let agents = self
            .core
            .read(cx)
            .displays
            .list
            .iter()
            .any(|d| d.started.contains(&pid));
        if !agents || crate::mac::ime::holding(pid) {
            return;
        }
        cx.background_executor()
            .spawn(async move {
                if crate::mac::catch::user_acted_recently(pid) {
                    return;
                }
                log::info!(
                    "pid {pid}, an app on an Agent Display, made itself the front app; giving the front back to pid {previous}"
                );
                crate::mac::ime::return_front(previous);
            })
            .detach();
    }

    fn launcher_typed(&mut self, cx: &mut Context<Self>) {
        const SPAN: Duration = Duration::from_secs(10);
        let Some(display) = self.viewer.as_ref().and(self.viewer_display) else {
            return;
        };
        if crate::mac::input::front() != Some(std::process::id() as i32) {
            return;
        }
        let now = std::time::Instant::now();
        let watching = self
            .launch_here
            .is_some_and(|(d, until)| d == display && until > now);
        self.launch_here = Some((display, now + SPAN));
        if watching {
            return;
        }
        log::info!(
            "launcher typed in from the Viewer of display {display}: its new windows go there"
        );
        let ids = || -> std::collections::HashSet<u32> {
            windows::all()
                .into_iter()
                .map(|w| w.target.window_id)
                .collect()
        };
        cx.spawn(async move |this, cx| {
            let mut known = cx.background_executor().spawn(async move { ids() }).await;
            // The app the launcher opened, once its first window came.
            let mut opened: Option<i32> = None;
            loop {
                // Every frame: the new window shows on the user's screen
                // until it moves, and AeroSpace tiles their windows around
                // it meanwhile. A list of windows takes a few ms, for at
                // most 10 s after a key typed into a launcher.
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Ok(Some((d, until))) = this.read_with(cx, |shell, _| shell.launch_here) else {
                    return;
                };
                if d != display || std::time::Instant::now() > until {
                    return;
                }
                let seen = known.clone();
                let fresh: Vec<windows::Window> = cx
                    .background_executor()
                    .spawn(async move {
                        windows::all()
                            .into_iter()
                            .filter(|w| {
                                w.layer == 0
                                    && !seen.contains(&w.target.window_id)
                                    && !windows::on_virtual_display(w.target.window_id)
                                    && !app::is_launcher(w.target.pid)
                            })
                            .collect()
                    })
                    .await;
                for window in fresh {
                    known.insert(window.target.window_id);
                    if opened.is_some_and(|pid| pid != window.target.pid) {
                        continue;
                    }
                    if opened.is_none() {
                        opened = Some(window.target.pid);
                        let _ = this.update(cx, |shell, _| {
                            shell.launch_here =
                                Some((display, std::time::Instant::now() + Duration::from_secs(2)));
                        });
                    }
                    log::info!(
                        "window {} of {} opened from the launcher: to display {display}",
                        window.target.window_id,
                        window.app
                    );
                    // Off the user's screen first: the Handover below asks
                    // AeroSpace where the window was before it moves it,
                    // and each question to AeroSpace takes a while.
                    let target = window.target;
                    cx.background_executor()
                        .spawn(async move {
                            let _ = ax::move_into(target, cg::display_bounds(display));
                        })
                        .await;
                    let _ = this.update(cx, |shell, cx| {
                        shell.hand_over(window.target, Some(window.bounds), display, cx)
                    });
                }
            }
        })
        .detach();
    }

    // MARK: Handover

    #[cfg(debug_assertions)]
    pub fn stage_display(&self) -> Option<DisplayId> {
        self.stage
    }

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
                        // Handed over again: the app's late windows are the
                        // agent's to have, not to send back.
                        core.returning
                            .retain(|(d, k)| !(*d == id && k.pid() == target.pid));
                        core.pending_returns.remove(&(id, target.pid));
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
    /// The user's window under a drag that began at `at`, found off the UI
    /// thread: the window list can wait on WindowServer.
    fn drag_candidate(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
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
        cx.spawn(async move |shell, cx| {
            let hit = cx
                .background_spawn(async move {
                    windows::all()
                        .into_iter()
                        .find(|w| w.bounds.contains(point) && w.target.pid != own)
                })
                .await;
            let _ = shell.update(cx, |shell, _| {
                shell.drag = hit.map(|hit| Drag {
                    target: hit.target,
                    frame: hit.bounds,
                    last_check: Instant::now(),
                });
            });
        })
        .detach();
    }

    /// Whether the dragged window has moved from where the drag began.
    /// Moved, not resized: a resize from the left or top edge keeps the
    /// opposite edge still. A tiled window that grows back to its old size
    /// as it's dragged away still moves both. Blocking.
    fn dragged(target: WindowTarget, frame: Rect) -> bool {
        windows::find(target.window_id).is_some_and(|w| {
            let (b, f) = (&w.bounds, &frame);
            let moved_x = (b.x - f.x).abs() > 4.0 && (b.max_x() - f.max_x()).abs() > 4.0;
            let moved_y = (b.y - f.y).abs() > 4.0 && (b.max_y() - f.max_y()).abs() > 4.0;
            moved_x || moved_y
        })
    }

    fn drag_moved(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        if let Some(from) = self.drag_from.take() {
            self.drag_candidate(from, cx);
        }
        let Some(drag) = &mut self.drag else { return };
        if drag.last_check.elapsed() < Duration::from_millis(50) {
            return;
        }
        drag.last_check = Instant::now();
        let (target, frame) = (drag.target, drag.frame);
        if !self.drop_rect(cx).is_some_and(|r| contains(r, at)) {
            self.set_drop_hover(false, cx);
            return;
        }
        cx.spawn(async move |shell, cx| {
            let moved = cx
                .background_spawn(async move { Self::dragged(target, frame) })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                // Unless the drag ended meanwhile.
                if shell.drag.as_ref().is_some_and(|d| d.target == target) {
                    shell.set_drop_hover(moved, cx);
                }
            });
        })
        .detach();
    }

    fn drag_ended(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        self.drag_from = None;
        let over = self.drop_rect(cx).is_some_and(|r| contains(r, at));
        let drag = self.drag.take();
        self.set_drop_hover(false, cx);
        let (Some(drag), Some(id), true) = (drag, self.viewer_display, over) else {
            return;
        };
        cx.spawn(async move |shell, cx| {
            let (target, frame) = (drag.target, drag.frame);
            let dropped = cx
                .background_spawn(async move { Self::dragged(target, frame) })
                .await;
            if !dropped {
                return;
            }
            // Let the window manager finish its own drop handling first, or
            // it re-tiles the window after we move it.
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.hand_over(drag.target, Some(drag.frame), id, cx)
            });
        })
        .detach();
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
            let _ = this.update(cx, |shell, cx| {
                shell.aerospace = ignoring;
                shell.push_setup(cx);
            });
        })
        .detach();
    }

    /// Set AeroSpace to leave Agent Displays alone, or say how to get an
    /// AeroSpace that can (docs/adr/0006).
    fn ignore_in_aerospace(&mut self, cx: &mut Context<Self>) {
        let why = t(
            "AeroSpace manages every display, Agent Screen's too. It shows one of your workspaces on each new display, which takes those windows off your screen and into the agent's screenshots, and it files the agents' new windows in your workspace, so they pass over your screen first.",
            "AeroSpace 会管理所有显示器，包括 Agent Screen 的。每建一块显示器，它就把你的一个工作区放上去，那些窗口会从你屏幕上消失，出现在 Agent 的截图里。Agent 打开的新窗口也会被它登记到你的工作区，先在你屏幕上闪一下。",
        );
        if self.aerospace == Some(aerospace::Ignoring::Unsupported) {
            let commands = "brew tap fanzzzd/agent-screen
brew trust fanzzzd/agent-screen
brew uninstall --cask aerospace
brew install --cask aerospace-agent-screen
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
                        "Agent Screen's build of AeroSpace adds the option it needs and keeps your config. The guide shows the five Terminal commands; they're on your clipboard.",
                        "Agent Screen 提供的 AeroSpace 加上了需要的选项，你的配置不变。说明页里有 5 条终端命令，已经复制到剪贴板。",
                    ),
                ),
                t("Copy Commands and Open Guide", "复制命令并打开说明"),
            );
            if go {
                native::copy(commands);
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("https://github.com/Fanzzzd/agent-screen/blob/main/docs/aerospace.md")
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
                "Let AeroSpace leave Agent Screen's displays alone?",
                "让 AeroSpace 不管 Agent Screen 的显示器？",
            ),
            &format!(
                "{why}

{}",
                tf!(
                    "Agent Screen adds ignored-monitors = ['^Agent Screen'] to {}, keeps the old file beside it as .before-agent-screen, and reloads AeroSpace's config.",
                    "Agent Screen 会在 {} 里加上 ignored-monitors = ['^Agent Screen']，旧文件另存为 .before-agent-screen，然后让 AeroSpace 重新加载配置。",
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
                            "AeroSpace now leaves Agent Screen's displays alone",
                            "AeroSpace 不再管 Agent Screen 的显示器了",
                        ),
                        &format!(
                            "{}\n\n{old}",
                            tf!(
                                "Added ignored-monitors = ['^Agent Screen'] to {}.",
                                "已在 {} 里加上 ignored-monitors = ['^Agent Screen']。",
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
                            "Agent Screen couldn't put your old config back; it's in the .before-agent-screen file beside it.",
                            "Agent Screen 没能把旧配置放回去；它在旁边的 .before-agent-screen 文件里。",
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
    /// The displays changed. When one comes or goes, macOS moves the others
    /// and may drop their windows on the user's screen, where AeroSpace
    /// tiles them in the user's workspace. Put each window of a live Agent
    /// Display back once the displays have settled.
    fn schedule_regather(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            for wait in [500, 1500] {
                cx.background_executor()
                    .timer(Duration::from_millis(wait))
                    .await;
                let Ok(windows) = this.update(cx, |shell, cx| {
                    shell
                        .core
                        .read(cx)
                        .displays
                        .list
                        .iter()
                        .filter(|d| !d.is_ended())
                        .flat_map(|d| d.records.values().map(move |r| (d.id, r.target)))
                        .collect::<Vec<(DisplayId, WindowTarget)>>()
                }) else {
                    return;
                };
                cx.background_executor()
                    .spawn(async move {
                        for (id, target) in windows {
                            if windows::find(target.window_id).is_some()
                                && !windows::on_virtual_display(target.window_id)
                            {
                                let moved = crate::mac::ax::move_into(target, cg::display_bounds(id));
                                log::info!(
                                    "window {} fell off display {id} as the displays changed; put back: {}",
                                    target.window_id,
                                    moved.is_ok()
                                );
                            }
                        }
                    })
                    .await;
            }
        })
        .detach();
    }

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
                let _ = this.update(cx, |shell, cx| {
                    for (workspace, display) in &conflicts {
                        log::warn!("AeroSpace pins workspace {workspace} to {display}");
                    }
                    if shell.conflicts != conflicts {
                        shell.conflicts = conflicts;
                        shell.push_setup(cx);
                    }
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
            // Reading the cursor asks WindowServer: off the UI thread.
            last_good = cx
                .background_spawn(async move { pull_cursor_back(&ids, last_good) })
                .await;
        }
    })
    .detach();
}

/// Put the cursor back where it last was off `ids`, if it is on one of
/// them. Where it is now when that is a good place.
fn pull_cursor_back(ids: &[DisplayId], last_good: Option<(f64, f64)>) -> Option<(f64, f64)> {
    let Some(event) = objc2_core_graphics::CGEvent::new(None) else {
        return last_good;
    };
    let at = objc2_core_graphics::CGEvent::location(Some(&event));
    let at = (at.x, at.y);
    if !ids.iter().any(|&id| cg::display_bounds(id).contains(at)) {
        return Some(at);
    }
    let back = last_good.unwrap_or_else(|| cg::display_bounds(cg::main_display()).mid());
    log::debug!("cursor pulled back from {at:?} to {back:?}");
    objc2_core_graphics::CGWarpMouseCursorPosition(objc2_core_foundation::CGPoint::new(
        back.0, back.1,
    ));
    objc2_core_graphics::CGAssociateMouseAndMouseCursorPosition(true);
    last_good
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

#[cfg(test)]
mod peek_input_tests {
    use super::{peek_activation, peek_outside_press, peek_timer_effect};
    use crate::mac::native::status_icon_event;
    use agent_screen_model::peek::{Effect, Intent, Mode, OPEN_DELAY_MS, Pointer, Timing};
    use objc2_app_kit::NSEventType;

    const ICON: Pointer = Pointer {
        icon: true,
        peek: false,
    };
    const OUT: Pointer = Pointer {
        icon: false,
        peek: false,
    };
    fn hover() -> Intent {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let timer = intent.pending().unwrap();
        assert_eq!(
            peek_timer_effect(&mut intent, timer, ICON, OPEN_DELAY_MS, true),
            Effect::Show
        );
        intent
    }

    #[test]
    fn peek_shell_right_down_without_valid_up_does_not_freeze_hover() {
        let mut intent = hover();
        // A right press dragged out may never yield the button's valid-up
        // action. It must not arm the left-click transaction at all.
        if let Some(down) = status_icon_event(Some(NSEventType::RightMouseDown), 9) {
            peek_activation(&mut intent, &down, ICON, 301);
        }
        intent.pointer(OUT, 302, Timing::default());
        let close = intent
            .pending()
            .expect("right down must not freeze hover departure");
        assert_eq!(
            peek_timer_effect(&mut intent, close, OUT, close.due_ms, true),
            Effect::Hide
        );
        assert_eq!(intent.mode(), Mode::Closed);
        assert!(matches!(
            status_icon_event(Some(NSEventType::RightMouseUp), 10),
            Some(crate::mac::native::Native::IconClicked {
                right: true,
                mouse: true
            })
        ));
    }

    #[test]
    fn peek_shell_native_hover_click_pins_without_a_hide_effect() {
        let mut intent = hover();
        let down = status_icon_event(Some(NSEventType::LeftMouseDown), 1).unwrap();
        let up = status_icon_event(Some(NSEventType::LeftMouseUp), 2).unwrap();
        assert_eq!(peek_activation(&mut intent, &down, ICON, 301), Effect::None);
        assert_eq!(
            peek_outside_press(&mut intent, ICON, ICON, 301),
            Effect::None
        );
        assert_eq!(peek_activation(&mut intent, &up, ICON, 302), Effect::Show);
        assert_eq!(intent.mode(), Mode::Pinned);
        assert_eq!(peek_activation(&mut intent, &up, ICON, 303), Effect::None);
        let next_down = status_icon_event(Some(NSEventType::LeftMouseDown), 3).unwrap();
        peek_activation(&mut intent, &next_down, ICON, 1000);
        assert_eq!(peek_activation(&mut intent, &up, ICON, 1001), Effect::Hide);
        assert_eq!(intent.mode(), Mode::Closed);
    }

    #[test]
    fn peek_shell_press_crosses_hover_deadline_without_timer_activation() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let timer = intent.pending().unwrap();
        let down = status_icon_event(Some(NSEventType::LeftMouseDown), 1).unwrap();
        let up = status_icon_event(Some(NSEventType::LeftMouseUp), 2).unwrap();
        assert_eq!(
            peek_activation(&mut intent, &down, ICON, OPEN_DELAY_MS - 1),
            Effect::None
        );
        assert_eq!(
            peek_timer_effect(&mut intent, timer, ICON, OPEN_DELAY_MS + 1, true),
            Effect::None
        );
        assert_eq!(intent.mode(), Mode::Closed);
        assert_eq!(
            peek_activation(&mut intent, &up, ICON, OPEN_DELAY_MS + 20),
            Effect::Show
        );
        assert_eq!(intent.mode(), Mode::Pinned);
    }

    #[test]
    fn peek_shell_dismiss_between_down_and_up_cannot_reopen() {
        let mut intent = hover();
        let keyboard = status_icon_event(None, 0).unwrap();
        peek_activation(&mut intent, &keyboard, ICON, 301);
        let down = status_icon_event(Some(NSEventType::LeftMouseDown), 1).unwrap();
        let up = status_icon_event(Some(NSEventType::LeftMouseUp), 2).unwrap();
        peek_activation(&mut intent, &down, ICON, 400);
        assert_eq!(peek_outside_press(&mut intent, OUT, OUT, 401), Effect::Hide);
        assert_eq!(peek_activation(&mut intent, &up, ICON, 402), Effect::None);
        assert_eq!(intent.mode(), Mode::Closed);
    }

    #[test]
    fn peek_shell_timer_rechecks_permission_and_stale_close_after_pin() {
        let mut intent = Intent::default();
        intent.pointer(ICON, 0, Timing::default());
        let open = intent.pending().unwrap();
        assert_eq!(
            peek_timer_effect(&mut intent, open, ICON, 300, false),
            Effect::None
        );
        assert_eq!(
            peek_timer_effect(&mut intent, open, ICON, 301, true),
            Effect::None
        );
        let mut intent = hover();
        intent.pointer(OUT, 400, Timing::default());
        let close = intent.pending().unwrap();
        let activate = status_icon_event(None, 0).unwrap();
        assert_eq!(
            peek_activation(&mut intent, &activate, ICON, 450),
            Effect::Show
        );
        assert_eq!(
            peek_timer_effect(&mut intent, close, OUT, 1000, true),
            Effect::None
        );
        assert_eq!(intent.mode(), Mode::Pinned);
    }
}
