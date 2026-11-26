//! AppKit pieces GPUI has no API for: the menu bar icon, native menus and
//! global mouse monitors.
//!
//! Native callbacks never touch GPUI state directly. They send `Native`
//! events down a channel, and a GPUI task handles them. Menus pop up from
//! the main dispatch queue, so their nested run loop never runs inside a
//! GPUI update.

use crate::i18n::t;
use agent_screen_model::glow::{self, Glow};
use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimationContext, NSApplication, NSColor, NSControlStateValueOn, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSRunningApplication,
    NSStatusBar, NSStatusItem, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowCollectionBehavior,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

/// Things that happened on the native side.
#[derive(Debug, Clone)]
pub enum Native {
    /// The button owns both left-button phases. Down captures; only up commits.
    IconPressed {
        gesture: u64,
    },
    IconClicked {
        right: bool,
        mouse: bool,
    },
    /// The pointer entered or left the menu bar icon.
    IconHover(bool),
    /// A mouse button went down anywhere, in Cocoa screen coordinates.
    MouseDown {
        x: f64,
        y: f64,
    },
    LeftDown {
        x: f64,
        y: f64,
    },
    LeftDragged {
        x: f64,
        y: f64,
    },
    LeftUp {
        x: f64,
        y: f64,
    },
    /// A menu item was chosen.
    Menu(MenuAction),
    /// The user touched the mouse or keyboard during an Input Grant.
    UserTookInput,
    /// The Mac locked or slept, or the session was switched out: YOLO
    /// ends (ADR 0025).
    YoloEnds(agent_screen_model::autonomy::YoloEnd),
    /// The Stop chord, ⌃⌥⌘.: Stop, and the mode goes to Ask.
    StopChord,
    /// In YOLO, `HELD_GRACE_MS` passed since the user's first held key or
    /// press: the operation the mode opened ends.
    GraceOver,
    /// The user typed in a launcher such as Raycast.
    LauncherTyped,
    /// A left click of the user's at a global point (top-left origin),
    /// outside Agent Screen: maybe on the Dock.
    UserClicked {
        x: f64,
        y: f64,
    },
    /// Sparkle downloaded an update, which waits to be installed.
    UpdateReady,
    /// App `pid` became the front app, after `previous`.
    Activated {
        pid: i32,
        previous: Option<i32>,
    },
    /// The user clicked, scrolled or typed on the Viewer's display image.
    ScreenTouched {
        display: u32,
    },
    /// A left click on the menu bar strip of the display image, `x` in
    /// Viewer window points from its left edge.
    MenuBarClick {
        display: u32,
        x: f64,
    },
    /// The user clicked a notification about a display, or one of its
    /// actions. `request`: the Help Request it was posted for.
    NotificationClicked {
        display: u32,
        request: Option<u64>,
        action: super::notify::Action,
    },
    /// What macOS says about notifying, after `notify::check`.
    InputQueueOpened,
    NotificationAuth(super::notify::Auth),
    /// Screens were added, removed or rearranged.
    ScreensChanged,
    /// The accent color or another system color changed.
    ColorsChanged,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    CreateDisplay,
    /// Forget where displays landed on the user's screen (#220).
    AllowNewDisplays,
    OpenSettings,
    OpenRecordings,
    ReportProblem,
    /// Open a bug report an agent filed, newest first (ADR 0024).
    OpenReported {
        index: usize,
    },
    /// Turn agents' bug reports on or off.
    ToggleReports,
    /// Stop and Switch to Ask, in Auto and YOLO (ADR 0025).
    StopAutonomy,
    /// Ask or Auto at once; YOLO opens Settings with its confirmation.
    ChooseAutonomy {
        mode: agent_screen_model::autonomy::Mode,
    },
    CheckForUpdates,
    /// Install the downloaded update and restart.
    InstallUpdate,
    OpenAeroSpaceConfig,
    IgnoreInAeroSpace,
    ConnectAgents,
    ChooseWallpaper,
    DefaultWallpaper,
    Quit,
    OpenDisplay {
        display: u32,
    },
    CopyWebLink {
        display: u32,
    },
    DismissDisplay {
        display: u32,
    },
    HandOver {
        display: u32,
        pid: i32,
        window: u32,
    },
    TakeBack {
        display: u32,
        pid: i32,
        window: u32,
    },
    /// Press an accessibility element kept by the menu bar proxy.
    PressProxy {
        index: usize,
    },
}

thread_local! {
    static EVENTS: RefCell<Option<UnboundedSender<Native>>> = const { RefCell::new(None) };
    static KEEP: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) };
}

pub fn set_sender(sender: UnboundedSender<Native>) {
    EVENTS.with(|e| *e.borrow_mut() = Some(sender));
}

/// The level of the Red Glow Agent Screen draws along the user's screens'
/// edges, which lets every click through. The Input Bar sits just above.
pub const BORDER_LEVEL: isize = 1000;

pub fn send(event: Native) {
    EVENTS.with(|e| {
        if let Some(sender) = e.borrow().as_ref() {
            let _ = sender.unbounded_send(event);
        }
    });
}

pub fn keep_monitor(object: Retained<AnyObject>) {
    keep(object);
}

pub(crate) fn keep(object: Retained<AnyObject>) {
    KEEP.with(|k| k.borrow_mut().push(object));
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_time(when: u64, delta: i64) -> u64;
}

/// Run `f` on the main queue, after the current event is handled.
pub fn main_async(f: impl FnOnce() + 'static) {
    extern "C" fn trampoline(context: *mut c_void) {
        let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
        f();
    }
    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe {
        dispatch_async_f(
            &raw const _dispatch_main_q,
            Box::into_raw(boxed).cast(),
            trampoline,
        )
    };
}

/// Run `f` on the main queue once `delay` has passed.
pub fn main_after(delay: std::time::Duration, f: impl FnOnce() + 'static) {
    extern "C" fn trampoline(context: *mut c_void) {
        let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
        f();
    }
    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    let nanos = i64::try_from(delay.as_nanos()).unwrap_or(i64::MAX);
    unsafe {
        // DISPATCH_TIME_NOW is 0.
        dispatch_after_f(
            dispatch_time(0, nanos),
            &raw const _dispatch_main_q,
            Box::into_raw(boxed).cast(),
            trampoline,
        )
    };
}

// MARK: Menu bar icon

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AgentScreenIconTarget"]
    struct IconTarget;

    impl IconTarget {
        #[unsafe(method(clicked:))]
        fn clicked(&self, _sender: &AnyObject) {
            let mtm = MainThreadMarker::from(self);
            let event = NSApplication::sharedApplication(mtm).currentEvent();
            let kind = event.as_ref().map(|e| e.r#type());
            let gesture = event.as_ref().map_or(0, |e| e.eventNumber() as u64);
            if let Some(event) = status_icon_event(kind, gesture) { send(event); }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            send(Native::IconHover(true));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            send(Native::IconHover(false));
        }
    }
);

pub struct StatusIcon {
    item: Retained<NSStatusItem>,
    asking_count: Cell<usize>,
    needs_setup: Cell<bool>,
    working: Cell<bool>,
    /// YOLO is on: the icon is red in every state (ADR 0025).
    yolo: Cell<bool>,
}

/// The icon's pictures, made by assets/status/make.py.
const ICON_IDLE: &str = include_str!("../../assets/status/idle.svg");
const ICON_WORKING: &str = include_str!("../../assets/status/working.svg");
const ICON_ASKING: &str = include_str!("../../assets/status/asking.svg");
const ICON_SETUP: &str = include_str!("../../assets/status/setup.svg");

/// A template image from SVG source, at the size the SVG gives: 20 by 18
/// pt, wider than tall, as the glyph is.
fn template_svg(svg: &str) -> Option<Retained<NSImage>> {
    let data = objc2_foundation::NSData::with_bytes(svg.as_bytes());
    let image = NSImage::initWithData(<NSImage as objc2::AnyThread>::alloc(), &data)?;
    image.setTemplate(true);
    Some(image)
}

/// Where AppKit keeps the icon's place in the menu bar, and Agent Screen's
/// copy of a place where it showed on the user's screen: reinstalling
/// leaves AppKit's value off every screen, among the icons a full menu bar
/// hides.
const ICON_PLACE: &str = "NSStatusItem Preferred Position Item-0";
const ICON_PLACE_SHOWN: &str = "AgentScreen Icon Position Shown";

fn status_icon_mask() -> NSEventMask {
    NSEventMask::LeftMouseDown | NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp
}

pub(crate) fn status_icon_event(kind: Option<NSEventType>, gesture: u64) -> Option<Native> {
    // A cancelled right press may not send a valid button-up action. It
    // must never freeze the left-button transaction or become a left click.
    if kind == Some(NSEventType::RightMouseDown) {
        return None;
    }
    Some(match kind {
        Some(NSEventType::LeftMouseDown) => Native::IconPressed { gesture },
        Some(NSEventType::LeftMouseUp) => Native::IconClicked {
            right: false,
            mouse: true,
        },
        Some(NSEventType::RightMouseUp) => Native::IconClicked {
            right: true,
            mouse: true,
        },
        _ => Native::IconClicked {
            right: false,
            mouse: false,
        },
    })
}

impl StatusIcon {
    pub fn install(mtm: MainThreadMarker) -> StatusIcon {
        // Back where the user last saw it, before AppKit places it.
        let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
        if let Some(shown) = defaults.objectForKey(&NSString::from_str(ICON_PLACE_SHOWN)) {
            unsafe { defaults.setObject_forKey(Some(&shown), &NSString::from_str(ICON_PLACE)) };
        }
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-2.0); // NSSquareStatusItemLength
        let target: Retained<IconTarget> = unsafe { msg_send![IconTarget::alloc(mtm), init] };
        if let Some(button) = item.button(mtm) {
            unsafe {
                button.setTarget(Some(&target));
                button.setAction(Some(sel!(clicked:)));
                let _: isize = msg_send![&button, sendActionOn: status_icon_mask().0];
            }
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    <NSTrackingArea as objc2::AnyThread>::alloc(),
                    NSRect::ZERO,
                    options,
                    Some(&target),
                    None,
                )
            };
            button.addTrackingArea(&area);
        }
        keep(Retained::into_super(Retained::into_super(target)));
        let icon = StatusIcon {
            item,
            asking_count: Cell::new(0),
            needs_setup: Cell::new(false),
            working: Cell::new(false),
            yolo: Cell::new(false),
        };
        icon.redraw();
        icon
    }

    /// A raised hand with the pending approval count while any request asks
    /// for the user (#206).
    pub fn set_asking_count(&self, count: usize) {
        self.asking_count.set(count);
        self.redraw();
    }

    pub fn asking_count(&self) -> usize {
        self.asking_count.get()
    }

    /// The icon shows on the user's screen: keep AppKit's place for it, to
    /// restore at the next launch.
    pub fn keep_place(&self) {
        let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
        let (place, shown) = (
            NSString::from_str(ICON_PLACE),
            NSString::from_str(ICON_PLACE_SHOWN),
        );
        if defaults.objectForKey(&place).is_none() {
            return;
        }
        let now = defaults.doubleForKey(&place);
        if defaults.objectForKey(&shown).is_none() || defaults.doubleForKey(&shown) != now {
            defaults.setDouble_forKey(now, &shown);
        }
    }

    /// Which picture shows: idle, working, asking or setup.
    pub fn state(&self) -> &'static str {
        if self.needs_setup.get() {
            "setup"
        } else if self.asking_count.get() > 0 {
            "asking"
        } else if self.working.get() {
            "working"
        } else {
            "idle"
        }
    }

    /// Red while YOLO is on.
    pub fn set_yolo(&self, yolo: bool) {
        self.yolo.set(yolo);
        self.redraw();
    }

    /// The icon's tint for `debugIcon`: "red" while YOLO is on.
    pub fn tint(&self) -> Option<&'static str> {
        self.yolo.get().then_some("red")
    }

    /// The front screen filled while any display is in use.
    pub fn set_working(&self, working: bool) {
        self.working.set(working);
        self.redraw();
    }

    /// A struck-through, orange icon while a permission is missing: nothing
    /// works until the user grants it.
    pub fn set_needs_setup(&self, needs: bool) {
        self.needs_setup.set(needs);
        self.redraw();
    }

    fn redraw(&self) {
        let mtm = MainThreadMarker::new().expect("main thread");
        let Some(button) = self.item.button(mtm) else {
            return;
        };
        let asking_count = self.asking_count.get();
        let badge_count = if self.needs_setup.get() {
            0
        } else {
            asking_count
        };
        let (svg, description): (&str, String) = if self.needs_setup.get() {
            (
                ICON_SETUP,
                t("Agent Screen: needs permissions", "Agent Screen：需要权限").to_string(),
            )
        } else if asking_count > 1 {
            (
                ICON_ASKING,
                tf!(
                    "Agent Screen: {asking_count} requests wait for you",
                    "Agent Screen：{asking_count} 项请求待处理"
                ),
            )
        } else if asking_count == 1 {
            (
                ICON_ASKING,
                t(
                    "Agent Screen: a request waits for you",
                    "Agent Screen：1 项请求待处理",
                )
                .to_string(),
            )
        } else if self.working.get() {
            (
                ICON_WORKING,
                t(
                    "Agent Screen: displays in use",
                    "Agent Screen：显示器使用中",
                )
                .to_string(),
            )
        } else if agent_screen_mcp::endpoint::is_dev_build() {
            (ICON_IDLE, "Agent Screen Dev".to_string())
        } else {
            (ICON_IDLE, "Agent Screen".to_string())
        };
        // -1.0 is NSVariableStatusItemLength; -2.0 is NSSquareStatusItemLength.
        self.item
            .setLength(if badge_count > 0 { -1.0 } else { -2.0 });
        if let Some(image) = template_svg(svg) {
            image.setAccessibilityDescription(Some(&NSString::from_str(&description)));
            button.setImage(Some(&image));
        }
        if badge_count > 0 {
            button.setTitle(&NSString::from_str(&format!(" {badge_count}")));
            // 7 is NSCellImagePosition::NSImageLeading.
            unsafe {
                let _: () = msg_send![&button, setImagePosition: 7usize];
            }
        } else {
            button.setTitle(&NSString::from_str(""));
            // 1 is NSCellImagePosition::NSImageOnly.
            unsafe {
                let _: () = msg_send![&button, setImagePosition: 1usize];
            }
        }
        button.setToolTip(Some(&NSString::from_str(&description)));
        // A development build shows purple, to tell it from the installed app.
        let tint = if self.yolo.get() {
            Some(NSColor::systemRedColor())
        } else if self.needs_setup.get() || asking_count > 0 {
            Some(NSColor::systemOrangeColor())
        } else {
            agent_screen_mcp::endpoint::is_dev_build().then(NSColor::systemPurpleColor)
        };
        button.setContentTintColor(tint.as_deref());
    }

    /// The icon's frame in Cocoa screen coordinates.
    pub fn frame(&self) -> Option<NSRect> {
        let mtm = MainThreadMarker::new()?;
        let button = self.item.button(mtm)?;
        let window = button.window()?;
        let in_window = button.convertRect_toView(button.bounds(), None);
        Some(window.convertRectToScreen(in_window))
    }

    /// Show a menu from the icon, the way a status item menu appears.
    pub fn pop_up(&self, menu: Retained<NSMenu>) {
        let item = self.item.clone();
        main_async(move || {
            let mtm = MainThreadMarker::new().expect("main thread");
            item.setMenu(Some(&menu));
            if let Some(button) = item.button(mtm) {
                let _menu = super::hang::Modal::open();
                unsafe { button.performClick(None) };
            }
            item.setMenu(None);
        });
    }
}

// MARK: Menus

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AgentScreenMenuTarget"]
    #[ivars = MenuAction]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(chosen:))]
        fn chosen(&self, _sender: &AnyObject) {
            send(Native::Menu(self.ivars().clone()));
        }
    }
);

impl MenuTarget {
    fn new(action: MenuAction, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(action);
        unsafe { msg_send![super(this), init] }
    }
}

pub struct Menu {
    menu: Retained<NSMenu>,
    mtm: MainThreadMarker,
}

impl Menu {
    pub fn new(title: &str) -> Menu {
        let mtm = MainThreadMarker::new().expect("main thread");
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        menu.setAutoenablesItems(false);
        Menu { menu, mtm }
    }

    fn item(&self, title: &str) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(title),
                None,
                &NSString::new(),
            )
        };
        self.menu.addItem(&item);
        item
    }

    /// An item that sends `action` when chosen.
    pub fn action(&self, title: &str, action: MenuAction) -> Retained<NSMenuItem> {
        let item = self.item(title);
        let target = MenuTarget::new(action, self.mtm);
        unsafe {
            item.setTarget(Some(&target));
            item.setAction(Some(sel!(chosen:)));
            // NSMenuItem holds its target weakly; its represented object
            // keeps the target alive exactly as long as the item.
            item.setRepresentedObject(Some(&target));
        }
        item
    }

    pub fn disabled(&self, title: &str) -> Retained<NSMenuItem> {
        let item = self.item(title);
        item.setEnabled(false);
        item
    }

    pub fn submenu(&self, title: &str, submenu: Menu) -> Retained<NSMenuItem> {
        let item = self.item(title);
        item.setSubmenu(Some(&submenu.menu));
        item
    }

    pub fn separator(&self) {
        self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
    }

    pub fn is_empty(&self) -> bool {
        self.menu.numberOfItems() == 0
    }

    pub fn into_native(self) -> Retained<NSMenu> {
        self.menu
    }

    /// Pop the menu up with its top-left corner at a Cocoa screen point.
    pub fn pop_up_at(self, x: f64, y: f64) {
        let menu = self.menu;
        main_async(move || {
            let _menu = super::hang::Modal::open();
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), None);
        });
    }
}

/// Set an item's key equivalent, as AX menus describe it.
pub fn set_key_equivalent(
    item: &NSMenuItem,
    key: &str,
    command: bool,
    shift: bool,
    option: bool,
    control: bool,
) {
    item.setKeyEquivalent(&NSString::from_str(&key.to_lowercase()));
    let mut mask = NSEventModifierFlags::empty();
    if command {
        mask |= NSEventModifierFlags::Command;
    }
    if shift {
        mask |= NSEventModifierFlags::Shift;
    }
    if option {
        mask |= NSEventModifierFlags::Option;
    }
    if control {
        mask |= NSEventModifierFlags::Control;
    }
    item.setKeyEquivalentModifierMask(mask);
}

/// A plain alert in front of everything, for a one-off result.
pub fn alert(title: &str, text: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    // A menu bar app is never active on its own.
    #[allow(deprecated)]
    objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let _alert = super::hang::Modal::open();
    alert.runModal();
}

/// An alert asking to go ahead: true when the user picks `go`.
pub fn confirm(title: &str, text: &str, go: &str) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    alert.addButtonWithTitle(&NSString::from_str(go));
    alert.addButtonWithTitle(&NSString::from_str(t("Cancel", "取消")));
    #[allow(deprecated)]
    objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let _alert = super::hang::Modal::open();
    alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
}

const TOOLS_LOADED_BY: &str = "ToolsLoadedBy";
const TOOLS_LOADED_AT: &str = "ToolsLoadedAt";

/// Keep which client last loaded the tools, and when, so Settings and the
/// menu can still say it after Agent Screen restarts with no session open.
pub fn remember_tools_loaded(client: &str) {
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let value = NSString::from_str(client);
    unsafe { defaults.setObject_forKey(Some(&value), &NSString::from_str(TOOLS_LOADED_BY)) };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default();
    defaults.setDouble_forKey(now, &NSString::from_str(TOOLS_LOADED_AT));
}

/// The client that last loaded the tools, and when, in seconds since 1970.
pub fn tools_last_loaded() -> Option<(String, f64)> {
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let client = defaults.stringForKey(&NSString::from_str(TOOLS_LOADED_BY))?;
    let at = defaults.doubleForKey(&NSString::from_str(TOOLS_LOADED_AT));
    (at > 0.0).then(|| (client.to_string(), at))
}

/// Put text on the clipboard.
pub fn copy(text: &str) {
    let board = objc2_app_kit::NSPasteboard::generalPasteboard();
    board.clearContents();
    unsafe {
        board.setString_forType(
            &NSString::from_str(text),
            objc2_app_kit::NSPasteboardTypeString,
        )
    };
}

pub fn set_checked(item: &NSMenuItem) {
    item.setState(NSControlStateValueOn);
}

/// An app's icon at menu size.
pub fn set_app_icon(item: &NSMenuItem, pid: i32) {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return;
    };
    let Some(icon) = app.icon() else { return };
    let icon: Retained<NSImage> = unsafe { msg_send![&icon, copy] };
    icon.setSize(NSSize::new(16.0, 16.0));
    item.setImage(Some(&icon));
}

// MARK: Mouse monitors

/// Report one mouse event of the user's, as the monitors in `watch_mouse`
/// do. The Viewer's private press calls it for the presses it takes before
/// any monitor sees them.
pub fn report_mouse(event: &NSEvent) {
    // The Input Queue's own drag is not the user's.
    if super::real_input::is_ours(event) {
        return;
    }
    let at = NSEvent::mouseLocation();
    let (x, y) = (at.x, at.y);
    match event.r#type() {
        NSEventType::LeftMouseDown => {
            send(Native::MouseDown { x, y });
            send(Native::LeftDown { x, y });
        }
        NSEventType::RightMouseDown => send(Native::MouseDown { x, y }),
        NSEventType::LeftMouseDragged => send(Native::LeftDragged { x, y }),
        NSEventType::LeftMouseUp => send(Native::LeftUp { x, y }),
        _ => {}
    }
}

/// Report every mouse down anywhere, and left-button drags, for Peek's
/// outside clicks and for window Handover by drag. Note the user's presses
/// for the catches.
pub fn watch_mouse() {
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp;
    let global =
        RcBlock::new(move |event: NonNull<NSEvent>| report_mouse(unsafe { event.as_ref() }));
    if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global) {
        keep(monitor);
    }
    // The user's clicks and key presses in other apps, which tell their new
    // windows from an agent's. Events posted to a pid never reach these.
    let user = RcBlock::new(|event: NonNull<NSEvent>| {
        use super::catch::Towards;
        if super::real_input::is_ours(unsafe { event.as_ref() }) {
            return;
        }
        let kind = unsafe { event.as_ref() }.r#type();
        let mtm = MainThreadMarker::new().expect("monitors run on the main thread");
        // A scroll is use of the window under the pointer, for the
        // activity monitor only: it tells the catches nothing.
        if kind == NSEventType::ScrollWheel {
            if !super::activity::running() {
                return;
            }
            let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
                NSEvent::mouseLocation(),
                0,
                mtm,
            );
            if let Ok(window) = u32::try_from(number) {
                super::activity::pressed(window);
            }
            return;
        }
        let towards = if kind == NSEventType::KeyDown {
            super::catch::user_pressed_key();
            return;
        } else {
            if kind == NSEventType::LeftMouseDown {
                let at = NSEvent::mouseLocation();
                let top = super::cg::display_bounds(super::cg::main_display()).height;
                send(Native::UserClicked {
                    x: at.x,
                    y: top - at.y,
                });
            }
            let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
                NSEvent::mouseLocation(),
                0,
                mtm,
            );
            let front = super::input::front();
            if let Ok(window) = u32::try_from(number) {
                super::activity::pressed(window);
            }
            u32::try_from(number).map_or(Towards::Unknown, |n| Towards::Window(n, front))
        };
        super::catch::user_acted(towards);
    });
    let presses = NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::KeyDown
        | NSEventMask::ScrollWheel;
    if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(presses, &user) {
        keep(monitor);
    }
    let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        report_mouse(unsafe { event.as_ref() });
        event.as_ptr()
    });
    if let Some(monitor) =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
    {
        keep(monitor);
    }
}

// MARK: Red Glow

/// One user screen's glow window and the layers its state animates.
struct GlowScreen {
    display: u32,
    frame: NSRect,
    shape: agent_screen_model::screen_edge::Screen,
    window: Retained<NSWindow>,
    /// Holds the layers, hidden until the glow first shows.
    root: Retained<objc2_quartz_core::CALayer>,
    /// Carries the state and the breathing of the bands.
    pulse: Retained<objc2_quartz_core::CALayer>,
    /// The bands, rasterized once.
    bloom: Retained<objc2_quartz_core::CALayer>,
    /// The rim and the hairline.
    rims: Retained<objc2_quartz_core::CALayer>,
    outline: objc2_core_foundation::CFRetained<objc2_core_graphics::CGMutablePath>,
}

#[derive(Default)]
struct GlowWindows {
    screens: Vec<GlowScreen>,
    shown: Option<Glow>,
    /// The screens changed since the windows were built.
    stale: bool,
}

thread_local! {
    static GLOW: RefCell<GlowWindows> = RefCell::new(GlowWindows::default());
}

/// Breathing, on `pulse`; and the fades, on `pulse` and `rims`.
const BREATH: &str = "breath";
const APPEAR: &str = "appear";
const CHANGE: &str = "change";

/// The screens changed while the glow shows: the next `show_glow` builds
/// its windows anew. Off, the next one compares the screens anyway.
fn forget_glow_screens() {
    GLOW.with(|g| {
        let mut g = g.borrow_mut();
        if g.shown.is_some() {
            g.stale = true;
        }
    });
}

/// The user's screens, by display id and frame: not the closet, not an
/// Agent Display or a Spare.
fn glow_screens() -> Vec<(u32, NSRect)> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    objc2_app_kit::NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| Some((screen_number(&screen)?, screen.frame())))
        .filter(|&(id, _)| !super::cg::is_closet(id) && !super::cg::is_agent_screen(id))
        .collect()
}

fn screen_number(screen: &objc2_app_kit::NSScreen) -> Option<u32> {
    let number = screen
        .deviceDescription()
        .objectForKey(&NSString::from_str("NSScreenNumber"))?;
    Some(
        number
            .downcast::<objc2_foundation::NSNumber>()
            .ok()?
            .unsignedIntValue(),
    )
}

fn same_frame(a: NSRect, b: NSRect) -> bool {
    a.origin.x == b.origin.x
        && a.origin.y == b.origin.y
        && a.size.width == b.size.width
        && a.size.height == b.size.height
}

/// Show the Red Glow in `state` on every user screen at once, or none for
/// Off, at once. Clicks pass through; it never shows on an Agent Display,
/// a Spare or the closet. The windows are kept and reused, and built anew
/// when the screens change. Returns whether every user screen has a
/// visible glow window, as real input may run only while it does.
pub fn show_glow(state: Glow) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    GLOW.with(|g| {
        let mut g = g.borrow_mut();
        let Some(look) = glow::look(state) else {
            for screen in &g.screens {
                screen.window.orderOut(None);
                screen.root.removeAllAnimations();
                screen.pulse.removeAllAnimations();
                screen.rims.removeAllAnimations();
            }
            g.shown = None;
            return false;
        };
        let now = glow_screens();
        let built = g.screens.len() == now.len()
            && g.screens
                .iter()
                .zip(&now)
                .all(|(s, &(id, frame))| s.display == id && same_frame(s.frame, frame));
        if g.stale || !built {
            for screen in g.screens.drain(..) {
                screen.window.orderOut(None);
            }
            g.screens = screen_shapes()
                .into_iter()
                .filter(|(id, _, _)| !super::cg::is_agent_screen(*id))
                .filter_map(|(id, frame, shape)| build_glow(mtm, id, frame, shape))
                .collect();
            g.stale = false;
            g.shown = None;
            log::info!("red glow: built for {} screen(s)", g.screens.len());
        }
        let calm =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        let changed = g.shown != Some(state);
        for screen in &g.screens {
            let appearing = !screen.window.isVisible();
            if appearing || changed {
                set_look(screen, state, look, appearing, calm);
            }
            if appearing {
                // The look is committed and flushed first, so the window's
                // first frame already shows the rims. Input waits for the
                // window, never for the fade.
                objc2_quartz_core::CATransaction::flush();
                screen.window.orderFrontRegardless();
            }
        }
        g.shown = Some(state);
        !g.screens.is_empty() && g.screens.iter().all(|s| s.window.isVisible())
    })
}

fn number(value: f64) -> Retained<AnyObject> {
    Retained::into_super(Retained::into_super(Retained::into_super(
        objc2_foundation::NSNumber::new_f64(value),
    )))
}

/// A fade of `layer`'s opacity from what shows now to `to`, `seconds`
/// long. The model value is set already.
fn fade(layer: &objc2_quartz_core::CALayer, from: f64, to: f64, seconds: f64, key: &str) {
    use objc2_quartz_core::{CABasicAnimation, CAMediaTiming};
    let animation = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
    unsafe {
        animation.setFromValue(Some(&number(from)));
        animation.setToValue(Some(&number(to)));
    }
    animation.setDuration(seconds);
    layer.addAnimation_forKey(&animation, Some(&NSString::from_str(key)));
}

/// What `layer` shows now, mid-animation too.
fn shown_opacity(layer: &objc2_quartz_core::CALayer) -> f64 {
    let now = unsafe { layer.presentationLayer() };
    f64::from(now.as_deref().unwrap_or(layer).opacity())
}

fn set_look(screen: &GlowScreen, state: Glow, look: glow::Look, appearing: bool, calm: bool) {
    use objc2_quartz_core::CATransaction;
    let (pulse_was, rims_was) = (shown_opacity(&screen.pulse), shown_opacity(&screen.rims));
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    for layer in [&screen.root, &screen.pulse, &screen.rims] {
        layer.removeAllAnimations();
    }
    screen.root.setOpacity(1.0);
    screen.pulse.setOpacity(look.pulse as f32);
    screen.rims.setOpacity(look.rims as f32);
    let delay = if appearing {
        // The rims are at full strength with the window, before any input
        // goes out; only the bloom fades in.
        fade(&screen.pulse, 0.0, look.pulse, glow::APPEAR_S, APPEAR);
        glow::APPEAR_S
    } else {
        fade(&screen.pulse, pulse_was, look.pulse, glow::CHANGE_S, CHANGE);
        fade(&screen.rims, rims_was, look.rims, glow::CHANGE_S, CHANGE);
        glow::CHANGE_S
    };
    if state == Glow::Live && !calm {
        breathe(&screen.pulse, delay);
    }
    CATransaction::commit();
}

/// Live's breathing on `pulse`, from the top of its breath, starting
/// `delay` seconds from now.
fn breathe(pulse: &objc2_quartz_core::CALayer, delay: f64) {
    use objc2_quartz_core::{
        CABasicAnimation, CAMediaTiming, CAMediaTimingFunction, kCAMediaTimingFunctionEaseInEaseOut,
    };
    let breath = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
    unsafe {
        breath.setFromValue(Some(&number(glow::LIVE.pulse)));
        breath.setToValue(Some(&number(glow::BREATH_LOW)));
    }
    breath.setDuration(glow::BREATH_S / 2.0);
    breath.setAutoreverses(true);
    breath.setRepeatCount(f32::INFINITY);
    breath.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
        kCAMediaTimingFunctionEaseInEaseOut
    })));
    if delay > 0.0 {
        breath.setBeginTime(objc2_quartz_core::CACurrentMediaTime() + delay);
    }
    pulse.addAnimation_forKey(&breath, Some(&NSString::from_str(BREATH)));
}

/// The Red Glow's window for screen `id`, ordered out, its layers at
/// opacity 0 until `set_look`.
fn build_glow(
    mtm: MainThreadMarker,
    id: u32,
    frame: NSRect,
    shape: agent_screen_model::screen_edge::Screen,
) -> Option<GlowScreen> {
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindowStyleMask};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGLineCap, CGLineJoin, CGPath};
    use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction};
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setIgnoresMouseEvents(true);
    window.setHasShadow(false);
    window.setLevel(BORDER_LEVEL);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), frame.size),
    );
    view.setWantsLayer(true);
    let host = view.layer()?;
    let bounds = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(frame.size.width, frame.size.height),
    );
    let (r, g, b) = glow::RED_P3;
    let red = NSColor::colorWithDisplayP3Red_green_blue_alpha(r, g, b, 1.0).CGColor();
    let outline = edge_path(shape);
    // The glass: a fresh layer for each mask, with no superlayer, as a
    // mask must be.
    let glass = || {
        let glass = CAShapeLayer::new();
        glass.setFrame(bounds);
        glass.setPath(Some(&outline));
        glass
    };
    let layer = || {
        let layer = CALayer::new();
        layer.setFrame(bounds);
        layer
    };
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let (root, pulse, bloom, rims) = (layer(), layer(), layer(), layer());
    // The bands: each casts the shadow of the outline stroked at its
    // width, centred on the glass's edge; the glass keeps the inner half,
    // which has no hard inner edge, as a blurred visible stroke would.
    let u = glow::scale(frame.size.width, frame.size.height);
    for band in glow::BANDS.iter().rev() {
        let (width, radius) = band.at(u);
        let path = unsafe {
            CGPath::new_copy_by_stroking_path(
                Some(&outline),
                std::ptr::null(),
                width,
                CGLineCap::Round,
                CGLineJoin::Round,
                10.0,
            )
        }?;
        let cast = layer();
        cast.setShadowPath(Some(&path));
        cast.setShadowColor(Some(&red));
        cast.setShadowRadius(radius);
        cast.setShadowOpacity(band.opacity as f32);
        cast.setShadowOffset(CGSize::new(0.0, 0.0));
        bloom.addSublayer(&cast);
    }
    unsafe { bloom.setMask(Some(&glass())) };
    // The bands hold only low frequencies: drawn once, at a pixel a point,
    // so the breathing animates one opacity and never draws them again.
    bloom.setShouldRasterize(true);
    bloom.setRasterizationScale(1.0);
    pulse.addSublayer(&bloom);
    // The rim and the hairline: the outline stroked twice as wide as what
    // shows and clipped to the glass, so the width holds along every curve
    // and around the notch. The rim lies over the hairline's outer part.
    let stroke = |color: &objc2_core_graphics::CGColor, width: f64| {
        let line = CAShapeLayer::new();
        line.setFrame(bounds);
        line.setPath(Some(&outline));
        line.setFillColor(None);
        line.setStrokeColor(Some(color));
        line.setLineWidth(width);
        line
    };
    let white = NSColor::colorWithWhite_alpha(1.0, glow::HAIRLINE_ALPHA).CGColor();
    rims.addSublayer(&stroke(&white, glow::hairline_stroke()));
    rims.addSublayer(&stroke(&red, glow::rim_stroke()));
    unsafe { rims.setMask(Some(&glass())) };
    root.addSublayer(&pulse);
    root.addSublayer(&rims);
    root.setOpacity(0.0);
    host.addSublayer(&root);
    CATransaction::commit();
    window.setContentView(Some(&view));
    Some(GlowScreen {
        display: id,
        frame,
        shape,
        window,
        root,
        pulse,
        bloom,
        rims,
        outline,
    })
}

/// What `debugGlow` reads of one glow window.
pub struct GlowProbe {
    pub display: u32,
    pub window: u32,
    pub frame: crate::model::Rect,
    pub visible: bool,
    pub ignores_mouse: bool,
    pub level: isize,
    pub scale: f64,
    pub backing: f64,
    pub rasterized: bool,
    pub animations: Vec<String>,
    /// Where to read each sample, in the window's points from its top-left
    /// corner: (place, d, point). `d` is the contract row's, at scale 1.
    pub samples: Vec<(&'static str, f64, (f64, f64))>,
}

/// A line `debugGlow` reads along: its place, where it starts outside the
/// glass, and its direction.
type Ray = (&'static str, (f64, f64), (f64, f64));

/// The glow windows as they are, with where `debugGlow` reads the
/// contract (`glow::CONTRACT`): on the perpendicular at the middle of each
/// edge, under the notch, on the 45° line into each top corner, and the
/// centre. Each ray starts outside the glass and the reading starts where
/// it enters it.
pub fn glow_probe() -> Vec<GlowProbe> {
    GLOW.with(|g| {
        let g = g.borrow();
        g.screens
            .iter()
            .map(|s| {
                let (w, h) = (s.frame.size.width, s.frame.size.height);
                let diagonal = std::f64::consts::FRAC_1_SQRT_2;
                let top_x = s.shape.notch.map_or(w / 2.0, |n| n.left / 2.0);
                let mut rays: Vec<Ray> = vec![
                    ("left", (0.0, h / 2.0), (1.0, 0.0)),
                    ("right", (w, h / 2.0), (-1.0, 0.0)),
                    ("top", (top_x, 0.0), (0.0, 1.0)),
                    ("bottom", (w / 2.0, h), (0.0, -1.0)),
                    ("corner", (0.0, 0.0), (diagonal, diagonal)),
                    ("cornerRight", (w, 0.0), (-diagonal, diagonal)),
                ];
                if s.shape.notch.is_some() {
                    rays.push(("notch", (w / 2.0, 0.0), (0.0, 1.0)));
                }
                let inside = |(x, y): (f64, f64)| unsafe {
                    objc2_core_graphics::CGPath::contains_point(
                        Some(&s.outline),
                        std::ptr::null(),
                        objc2_core_foundation::CGPoint::new(x, h - y),
                        false,
                    )
                };
                let mut samples = Vec::new();
                for (place, from, (dx, dy)) in rays {
                    let at = |t: f64| (from.0 + dx * t, from.1 + dy * t);
                    let Some(edge) = (0..2000)
                        .map(|i| f64::from(i) * 0.25)
                        .find(|&t| inside(at(t)))
                    else {
                        continue;
                    };
                    for (d, _, _) in glow::CONTRACT {
                        samples.push((place, d, at(edge + glow::distance(d, w, h))));
                    }
                }
                samples.push(("centre", 0.0, (w / 2.0, h / 2.0)));
                // Running ones: Core Animation takes a finished fade off
                // only at a later commit.
                let animations = s
                    .pulse
                    .animationKeys()
                    .map(|keys| {
                        keys.iter()
                            .filter(|key| running(&s.pulse, key))
                            .map(|k| k.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                GlowProbe {
                    display: s.display,
                    window: s.window.windowNumber() as u32,
                    frame: crate::model::Rect::new(s.frame.origin.x, s.frame.origin.y, w, h),
                    visible: s.window.isVisible(),
                    ignores_mouse: s.window.ignoresMouseEvents(),
                    level: s.window.level(),
                    scale: glow::scale(w, h),
                    backing: s.window.backingScaleFactor(),
                    rasterized: s.bloom.shouldRasterize(),
                    animations,
                    samples,
                }
            })
            .collect()
    })
}

/// Whether `layer`'s animation `key` still runs.
fn running(layer: &objc2_quartz_core::CALayer, key: &NSString) -> bool {
    use objc2_quartz_core::CAMediaTiming;
    let Some(animation) = (unsafe { layer.animationForKey(key) }) else {
        return false;
    };
    if animation.repeatCount().is_infinite() {
        return true;
    }
    let turns = if animation.autoreverses() { 2.0 } else { 1.0 };
    let now = layer.convertTime_fromLayer(objc2_quartz_core::CACurrentMediaTime(), None);
    animation.beginTime() + animation.duration() * turns > now
}

/// Hold Live's breath at its top while `debugGlow` reads the windows, or
/// let it breathe again.
pub fn glow_hold_breath(hold: bool) {
    use objc2_quartz_core::CATransaction;
    GLOW.with(|g| {
        let g = g.borrow();
        if g.shown != Some(Glow::Live) {
            return;
        }
        let calm =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for s in &g.screens {
            if hold {
                s.pulse.removeAnimationForKey(&NSString::from_str(BREATH));
            } else if !calm {
                breathe(&s.pulse, 0.0);
            }
        }
        CATransaction::commit();
    })
}

/// Each screen with its frame and shape: its camera notch where NSScreen
/// reports one, and a built-in panel's measured rounding.
fn screen_shapes() -> Vec<(u32, NSRect, agent_screen_model::screen_edge::Screen)> {
    use agent_screen_model::screen_edge::{Bezel, Notch, Screen};
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    objc2_app_kit::NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            let id = screen_number(&screen)?;
            if crate::mac::cg::is_closet(id) {
                return None;
            }
            let frame = screen.frame();
            let builtin = objc2_core_graphics::CGDisplayIsBuiltin(id);
            // The areas either side of the notch, on a screen that has one.
            let (left, right) = (
                screen.auxiliaryTopLeftArea(),
                screen.auxiliaryTopRightArea(),
            );
            let depth = screen.safeAreaInsets().top;
            let notch = (builtin && depth > 0.0 && left.size.width > 0.0 && right.size.width > 0.0)
                .then(|| {
                    Notch::from_areas(
                        frame.size.width,
                        left.origin.x + left.size.width - frame.origin.x,
                        right.origin.x - frame.origin.x,
                        depth,
                    )
                });
            let panel = builtin.then(|| super::display::native_pixels(id)).flatten();
            let bezel = panel.map_or(Bezel::SQUARE, |p| Bezel::of_builtin(p, notch.is_some()));
            let shape = Screen {
                width: frame.size.width,
                height: frame.size.height,
                scale: panel.map_or(screen.backingScaleFactor(), |(w, _)| {
                    f64::from(w) / frame.size.width
                }),
                bezel,
                notch,
            };
            if builtin {
                log::info!(
                    "red glow: display {id}, panel {panel:?}, {}x{} pt, {bezel:?}, {notch:?}",
                    frame.size.width,
                    frame.size.height
                );
            }
            Some((id, frame, shape))
        })
        .collect()
}

/// The screen's glass outline in the view's coordinates, y up.
fn edge_path(
    shape: agent_screen_model::screen_edge::Screen,
) -> objc2_core_foundation::CFRetained<objc2_core_graphics::CGMutablePath> {
    use agent_screen_model::screen_edge::{Segment, outline};
    use objc2_core_graphics::CGMutablePath;
    let path = CGMutablePath::new();
    let up = |(x, y): (f64, f64)| (x, shape.height - y);
    let o = outline(shape);
    let none = std::ptr::null();
    let (x, y) = up(o.start);
    unsafe { CGMutablePath::move_to_point(Some(&path), none, x, y) };
    for segment in o.segments {
        match segment {
            Segment::Line(p) => {
                let (x, y) = up(p);
                unsafe { CGMutablePath::add_line_to_point(Some(&path), none, x, y) };
            }
            Segment::Curve(a, b, p) => {
                let ((ax, ay), (bx, by), (x, y)) = (up(a), up(b), up(p));
                unsafe {
                    CGMutablePath::add_curve_to_point(Some(&path), none, ax, ay, bx, by, x, y)
                };
            }
        }
    }
    CGMutablePath::close_subpath(Some(&path));
    path
}

// MARK: Windows that draw their own shadow

thread_local! {
    /// Windows with a transparent margin around their content, where the
    /// app draws the content's shadow, by margin: top, left, bottom, right.
    /// WindowServer's own shadow is drawn from a window's last picture, a
    /// frame behind, so it trails content that changes size in the window.
    static MARGINS: RefCell<Vec<(Retained<NSWindow>, [f64; 4])>> = const { RefCell::new(Vec::new()) };
}

/// Draw `window`'s shadow in the app: no system shadow, and over the
/// margin, mouse events go to what is below, as they would around a
/// system shadow. The window takes them only over its content.
pub fn own_shadow(window: &NSWindow, margin: [f64; 4]) {
    window.setHasShadow(false);
    let first = MARGINS.with(|m| {
        let mut m = m.borrow_mut();
        m.push((window.retain(), margin));
        m.len() == 1
    });
    if first {
        let follow = |_: &NSEvent| follow_pointer(NSEvent::mouseLocation());
        let mask = NSEventMask::MouseMoved | NSEventMask::LeftMouseDragged;
        let global = RcBlock::new(move |event: NonNull<NSEvent>| follow(unsafe { event.as_ref() }));
        if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global)
        {
            keep(monitor);
        }
        let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            follow(unsafe { event.as_ref() });
            event.as_ptr()
        });
        if let Some(monitor) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
        {
            keep(monitor);
        }
    }
    follow_pointer(NSEvent::mouseLocation());
}

/// `frame` less a margin of top, left, bottom, right, in Cocoa coordinates.
pub fn inset(frame: NSRect, [top, left, bottom, right]: [f64; 4]) -> NSRect {
    NSRect::new(
        NSPoint::new(frame.origin.x + left, frame.origin.y + bottom),
        NSSize::new(
            frame.size.width - left - right,
            frame.size.height - top - bottom,
        ),
    )
}

/// `content` with a margin of top, left, bottom, right around it.
pub fn outset(content: NSRect, [top, left, bottom, right]: [f64; 4]) -> NSRect {
    inset(content, [-top, -left, -bottom, -right])
}

/// Each visible window with a margin takes mouse events only with the
/// pointer over its content. Called on every move, and when a window moves
/// or resizes. Skips hidden windows to avoid WindowServer fence IPCs while
/// Peek, the Viewer and cards are closed.
pub fn follow_pointer(at: NSPoint) {
    MARGINS.with(|m| {
        for (window, margin) in m.borrow().iter() {
            if !window.isVisible() {
                continue;
            }
            let content = inset(window.frame(), *margin);
            let over = at.x >= content.origin.x
                && at.x <= content.origin.x + content.size.width
                && at.y >= content.origin.y
                && at.y <= content.origin.y + content.size.height;
            if window.ignoresMouseEvents() == over {
                window.setIgnoresMouseEvents(!over);
            }
        }
    });
}

/// The pointer, in Cocoa screen coordinates.
pub fn mouse_location() -> (f64, f64) {
    let at = NSEvent::mouseLocation();
    (at.x, at.y)
}

/// Height of the primary screen: the pivot between Cocoa (bottom-left) and
/// CoreGraphics (top-left) global coordinates.
pub fn primary_height() -> f64 {
    let mtm = MainThreadMarker::new().expect("main thread");
    objc2_app_kit::NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |s| s.frame().size.height)
}

// MARK: Windows

/// The AppKit window behind a GPUI window.
pub fn ns_window(window: &gpui::Window) -> Option<Retained<NSWindow>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    view.window()
}

/// A panel above other apps' windows, on every Space, with a system shadow
/// that follows the rounded card.
pub fn style_panel(window: &NSWindow, level: isize) {
    window.setLevel(level);
    window.setHasShadow(true);
    window.setHidesOnDeactivate(false);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
}

/// Whether a GPUI window may become key. GPUI's panel class says yes to
/// key and main for every window, so each window gets a subclass of its
/// own class that answers for it. Never main, either way.
pub fn set_keyable(window: &NSWindow, keyable: bool) {
    use objc2::ffi::object_setClass;
    use objc2::runtime::{AnyClass, Bool, ClassBuilder, Sel};
    extern "C-unwind" fn yes(_: &AnyObject, _: Sel) -> Bool {
        // Not while the Help Card gives the keyboard back: AppKit must not
        // hand it to the Viewer instead of the user's app.
        Bool::new(!GIVING_BACK.load(std::sync::atomic::Ordering::Relaxed))
    }
    extern "C-unwind" fn no(_: &AnyObject, _: Sel) -> Bool {
        Bool::NO
    }
    let base = window.class();
    let name = format!(
        "AgentScreen{}{}",
        if keyable { "Keyable" } else { "Unkeyable" },
        base.name().to_string_lossy()
    );
    let name = std::ffi::CString::new(name).expect("class name");
    let class = AnyClass::get(&name).or_else(|| {
        let mut builder = ClassBuilder::new(&name, base)?;
        unsafe {
            builder.add_method(
                sel!(canBecomeKeyWindow),
                if keyable { yes } else { no } as extern "C-unwind" fn(_, _) -> _,
            );
            builder.add_method(
                sel!(canBecomeMainWindow),
                no as extern "C-unwind" fn(_, _) -> _,
            );
        }
        Some(builder.register())
    });
    if let Some(class) = class {
        unsafe {
            object_setClass(
                (window as *const NSWindow).cast_mut().cast(),
                (class as *const AnyClass).cast(),
            )
        };
    }
}

/// Whether the Help Card may take the keyboard: only while its reply field
/// is open.
static CARD_KEYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Make `window` keyable only while `card_keys(true)`, as `set_keyable`
/// does for good: the Help Card takes the keyboard for its reply field and
/// at no other time, so a click on its buttons leaves the user's app the
/// keyboard. Never main.
pub fn set_keyable_while_typing(window: &NSWindow) {
    use objc2::ffi::object_setClass;
    use objc2::runtime::{AnyClass, Bool, ClassBuilder, Sel};
    extern "C-unwind" fn typing(_: &AnyObject, _: Sel) -> Bool {
        Bool::new(CARD_KEYS.load(std::sync::atomic::Ordering::Relaxed))
    }
    extern "C-unwind" fn no(_: &AnyObject, _: Sel) -> Bool {
        Bool::NO
    }
    let base = window.class();
    let name = format!("AgentScreenTyping{}", base.name().to_string_lossy());
    let name = std::ffi::CString::new(name).expect("class name");
    let class = AnyClass::get(&name).or_else(|| {
        let mut builder = ClassBuilder::new(&name, base)?;
        unsafe {
            builder.add_method(
                sel!(canBecomeKeyWindow),
                typing as extern "C-unwind" fn(_, _) -> _,
            );
            builder.add_method(
                sel!(canBecomeMainWindow),
                no as extern "C-unwind" fn(_, _) -> _,
            );
        }
        Some(builder.register())
    });
    if let Some(class) = class {
        unsafe {
            object_setClass(
                (window as *const NSWindow).cast_mut().cast(),
                (class as *const AnyClass).cast(),
            )
        };
    }
}

/// The Help Card's reply field opened (true) or closed.
pub fn card_keys(on: bool) {
    CARD_KEYS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the Help Card may take the keyboard now.
pub fn card_keys_on() -> bool {
    CARD_KEYS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set while the Help Card gives the keyboard back, until the next turn of
/// the run loop: no Agent Screen window may become key meanwhile.
static GIVING_BACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Give the keyboard back to the user's app if `window` has it. The card is
/// a non-activating panel: the user's app stayed active while it typed, and
/// a panel like that gives the keyboard back only by leaving the screen. So
/// it leaves and comes straight back, in one turn of the run loop, without
/// taking the keyboard again, and while it is away no other Agent Screen
/// window, such as the Viewer, may take it.
pub fn give_up_keys(window: &NSWindow) {
    if !window.isKeyWindow() {
        return;
    }
    use std::sync::atomic::Ordering;
    GIVING_BACK.store(true, Ordering::Relaxed);
    window.orderOut(None);
    window.orderFrontRegardless();
    follow_pointer(NSEvent::mouseLocation());
    later(|| GIVING_BACK.store(false, Ordering::Relaxed));
}

pub fn is_key(window: &NSWindow) -> bool {
    window.isKeyWindow()
}

thread_local! {
    /// Asked when the app should terminate: true holds the quit until
    /// `reply_terminate`.
    static ON_TERMINATE: std::cell::RefCell<Option<Box<dyn Fn() -> bool>>> =
        const { std::cell::RefCell::new(None) };
}

/// Answer the app delegate's `applicationShouldTerminate:` with `hold`:
/// true makes the quit wait for `reply_terminate`.
///
/// GPUI sends `terminate:` from a main-queue block, and the quit waits in a
/// nested run loop inside it, so no GPUI task runs until it is answered.
/// `hold` therefore does its work right away or on its own thread.
pub fn intercept_terminate(mtm: objc2::MainThreadMarker, hold: impl Fn() -> bool + 'static) {
    use objc2::ffi::object_setClass;
    use objc2::runtime::{AnyClass, ClassBuilder, Sel};
    extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
        let later = ON_TERMINATE.with(|f| f.borrow().as_ref().is_some_and(|hold| hold()));
        // NSTerminateLater or NSTerminateNow
        if later { 2 } else { 1 }
    }
    ON_TERMINATE.with(|f| *f.borrow_mut() = Some(Box::new(hold)));
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else {
        return;
    };
    let delegate: &AnyObject = delegate.as_ref();
    let base = delegate.class();
    let name = format!("AgentScreenQuitting{}", base.name().to_string_lossy());
    let name = std::ffi::CString::new(name).expect("class name");
    let class = AnyClass::get(&name).or_else(|| {
        let mut builder = ClassBuilder::new(&name, base)?;
        unsafe {
            builder.add_method(
                sel!(applicationShouldTerminate:),
                should_terminate as extern "C-unwind" fn(_, _, _) -> _,
            );
        }
        Some(builder.register())
    });
    if let Some(class) = class {
        unsafe {
            object_setClass(
                (delegate as *const AnyObject).cast_mut().cast(),
                (class as *const AnyClass).cast(),
            )
        };
    }
}

/// Let a held quit go ahead. Callable from any thread: the reply goes
/// through the main run loop, which the waiting quit keeps running.
pub fn reply_terminate() {
    use objc2_core_foundation::{CFRunLoop, kCFRunLoopCommonModes};
    let Some(main) = CFRunLoop::main() else {
        return;
    };
    let block = RcBlock::new(|| {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            objc2_app_kit::NSApplication::sharedApplication(mtm)
                .replyToApplicationShouldTerminate(true);
        }
    });
    unsafe {
        main.perform_block(kCFRunLoopCommonModes.map(|m| m.as_ref()), Some(&block));
    }
    main.wake_up();
}

pub const LEVEL_FLOATING: isize = 3;
pub const LEVEL_POP_UP_MENU: isize = 101;

pub fn set_frame(window: &NSWindow, frame: NSRect) {
    window.setFrame_display(frame, true);
    window.invalidateShadow();
    follow_pointer(NSEvent::mouseLocation());
}

/// Resize a GPUI window that is showing, once the GPUI update in progress
/// has returned. Resizing redraws the window at its new size on the spot,
/// and inside an update GPUI can't draw, so the window would show its old
/// picture in its new frame, with an empty strip, until its next frame:
/// hundreds of ms while displays change.
pub fn set_frame_later(window: &NSWindow, frame: NSRect) {
    let window: Retained<NSWindow> = window.retain();
    later(move || set_frame(&window, frame));
}

/// Run `f` on the main thread once the GPUI update in progress has
/// returned: for showing or resizing a GPUI window, which draws on the spot.
pub fn later(f: impl FnOnce() + 'static) {
    use objc2_core_foundation::{CFRunLoop, kCFRunLoopCommonModes};
    let Some(main) = CFRunLoop::main() else {
        f();
        return;
    };
    let f = Cell::new(Some(f));
    let block = RcBlock::new(move || {
        if let Some(f) = f.take() {
            f();
        }
    });
    unsafe {
        main.perform_block(kCFRunLoopCommonModes.map(|m| m.as_ref()), Some(&block));
    }
    main.wake_up();
}

/// Show without taking the keyboard from the user's app. Refresh mouse-ignore
/// state for an unmoved pointer since `follow_pointer` skips hidden windows.
pub fn order_front(window: &NSWindow) {
    window.orderFrontRegardless();
    follow_pointer(NSEvent::mouseLocation());
}

pub fn order_out(window: &NSWindow) {
    window.orderOut(None);
}

thread_local! {
    static ANCHOR: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

/// The window a front hold the Autonomy Mode started takes the front with,
/// as no press of the user's names one (ADR 0025): 1 by 1 point, clear, at
/// the main screen's bottom-left corner, ignoring the mouse. Ordered in;
/// its window number. Main thread.
pub fn front_anchor() -> Option<u32> {
    use objc2_app_kit::{NSBackingStoreType, NSWindowStyleMask};
    let mtm = MainThreadMarker::new()?;
    ANCHOR.with(|anchor| {
        let mut anchor = anchor.borrow_mut();
        let window = anchor.get_or_insert_with(|| {
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setIgnoresMouseEvents(true);
            window.setHasShadow(false);
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Transient
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            window
        });
        window.orderFrontRegardless();
        Some(window.windowNumber() as u32)
    })
}

/// The hold ended: the anchor goes.
pub fn front_anchor_away() {
    ANCHOR.with(|anchor| {
        if let Some(window) = anchor.borrow().as_ref() {
            window.orderOut(None);
        }
    });
}

pub fn make_key(window: &NSWindow) {
    window.makeKeyWindow();
}

/// How an animation moves.
#[derive(Clone, Copy)]
pub enum Curve {
    /// Fast start, long soft landing: things arriving.
    Settle,
    /// Slow start: things leaving.
    EaseIn,
    /// Overshoots a little before landing: the Viewer opening.
    Spring,
}

/// Move `window` to `frame` and fade it to `alpha` over `seconds`.
pub fn animate(window: &NSWindow, frame: NSRect, alpha: f64, seconds: f64) {
    animate_frame(window, frame, seconds, Curve::Settle);
    animate_alpha(window, alpha, seconds, Curve::Settle);
}

fn run_animation(seconds: f64, curve: Curve, change: impl Fn() + 'static) {
    use objc2_quartz_core::{CAMediaTimingFunction, kCAMediaTimingFunctionEaseIn};
    let changes = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
        let context = unsafe { context.as_ref() };
        context.setDuration(seconds);
        let timing = match curve {
            Curve::Settle => CAMediaTimingFunction::functionWithControlPoints(0.16, 1.0, 0.3, 1.0),
            Curve::Spring => CAMediaTimingFunction::functionWithControlPoints(0.34, 1.4, 0.64, 1.0),
            Curve::EaseIn => {
                CAMediaTimingFunction::functionWithName(unsafe { kCAMediaTimingFunctionEaseIn })
            }
        };
        context.setTimingFunction(Some(&timing));
        change();
    });
    NSAnimationContext::runAnimationGroup(&changes);
}

pub fn animate_frame(window: &NSWindow, frame: NSRect, seconds: f64, curve: Curve) {
    let window: Retained<NSWindow> = window.retain();
    run_animation(seconds, curve, move || {
        let animator: Retained<NSWindow> = unsafe { msg_send![&window, animator] };
        animator.setFrame_display(frame, true);
    });
}

pub fn animate_alpha(window: &NSWindow, alpha: f64, seconds: f64, curve: Curve) {
    let window: Retained<NSWindow> = window.retain();
    run_animation(seconds, curve, move || {
        let animator: Retained<NSWindow> = unsafe { msg_send![&window, animator] };
        animator.setAlphaValue(alpha);
    });
}

pub fn set_alpha(window: &NSWindow, alpha: f64) {
    window.setAlphaValue(alpha);
}

pub fn frame(window: &NSWindow) -> NSRect {
    window.frame()
}

type ScreenEntry = (u32, NSRect, NSRect);
type ScreensCache = Option<(u64, Vec<ScreenEntry>)>;

thread_local! {
    /// Cached `NSScreen::screens` result keyed by `cg::display_change_stamp()`,
    /// cleared on `NSApplicationDidChangeScreenParametersNotification`.
    static SCREENS_CACHE: RefCell<ScreensCache> = const { RefCell::new(None) };
}

pub fn forget_screens() {
    SCREENS_CACHE.with(|c| *c.borrow_mut() = None);
}

/// Every screen: display id, frame and visible frame, Cocoa coordinates.
pub fn screens() -> Vec<(u32, NSRect, NSRect)> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let stamp = crate::mac::cg::display_change_stamp();
    if let Some(cached) = SCREENS_CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(s, _)| *s == stamp)
            .map(|(_, list)| list.clone())
    }) {
        return cached;
    }
    let list: Vec<(u32, NSRect, NSRect)> = objc2_app_kit::NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            let number = screen
                .deviceDescription()
                .objectForKey(&NSString::from_str("NSScreenNumber"))?;
            let number = number.downcast::<objc2_foundation::NSNumber>().ok()?;
            if crate::mac::cg::is_closet(number.unsignedIntValue()) {
                return None;
            }
            Some((
                number.unsignedIntValue(),
                screen.frame(),
                screen.visibleFrame(),
            ))
        })
        .collect();
    SCREENS_CACHE.with(|c| *c.borrow_mut() = Some((stamp, list.clone())));
    list
}

/// Menu bar heights measured since the screens last changed. Measuring asks
/// WindowServer for the display's spaces, which blocks while displays are
/// being added or removed; the Viewer reads it every frame.
static MENU_BARS: std::sync::Mutex<Vec<(u32, f64)>> = std::sync::Mutex::new(Vec::new());

/// The cached height for `display`, or `measure`'s, kept for next time.
/// Without a measurement (off the main thread, or no such screen) it's the
/// standard 24 pt and nothing is kept.
fn cached_menu_bar(display: u32, measure: impl FnOnce() -> Option<f64>) -> f64 {
    let mut bars = MENU_BARS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, height)) = bars.iter().find(|(id, _)| *id == display) {
        return height;
    }
    match measure() {
        Some(height) => {
            bars.push((display, height));
            height
        }
        None => 24.0,
    }
}

/// Measure again after the screens change.
pub fn forget_menu_bars() {
    MENU_BARS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Height of the menu bar on a display, in points.
pub fn menu_bar_height(display: u32) -> f64 {
    cached_menu_bar(display, || {
        MainThreadMarker::new().and_then(|mtm| measure_menu_bar(mtm, display))
    })
}

fn measure_menu_bar(mtm: MainThreadMarker, display: u32) -> Option<f64> {
    for screen in objc2_app_kit::NSScreen::screens(mtm).iter() {
        let number = screen
            .deviceDescription()
            .objectForKey(&NSString::from_str("NSScreenNumber"));
        let Some(number) = number.and_then(|n| n.downcast::<objc2_foundation::NSNumber>().ok())
        else {
            continue;
        };
        if number.unsignedIntValue() == display {
            let frame = screen.frame();
            let visible = screen.visibleFrame();
            let bar =
                (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);
            return Some(bar.max(24.0));
        }
    }
    None
}

/// Force light or dark for screenshots in debug builds:
/// AGENTSCREEN_APPEARANCE=light|dark. The system setting stays untouched.
pub fn debug_appearance() {
    if !cfg!(debug_assertions) {
        return;
    }
    let name = match std::env::var("AGENTSCREEN_APPEARANCE").as_deref() {
        Ok("light") => "NSAppearanceNameAqua",
        Ok("dark") => "NSAppearanceNameDarkAqua",
        _ => return,
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let appearance = objc2_app_kit::NSAppearance::appearanceNamed(&NSString::from_str(name));
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

/// Report screens being added, removed or rearranged.
pub fn watch_colors() {
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    let block =
        RcBlock::new(|_: NonNull<objc2_foundation::NSNotification>| send(Native::ColorsChanged));
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSSystemColorsDidChangeNotification),
            None,
            Some(&objc2_foundation::NSOperationQueue::mainQueue()),
            &block,
        )
    };
    let observer: Retained<AnyObject> = observer.into();
    keep(observer);
}

/// The user's accent color as sRGB, for light and for dark.
pub fn accent() -> Option<([f32; 3], [f32; 3])> {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSColorSpace,
    };
    let resolve = |name| -> Option<[f32; 3]> {
        let appearance = NSAppearance::appearanceNamed(name)?;
        let rgb = std::cell::Cell::new(None);
        let block = RcBlock::new(|| {
            let color =
                NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace());
            rgb.set(color.map(|c| {
                [
                    c.redComponent() as f32,
                    c.greenComponent() as f32,
                    c.blueComponent() as f32,
                ]
            }));
        });
        appearance.performAsCurrentDrawingAppearance(&block);
        rgb.get()
    };
    Some((
        resolve(unsafe { NSAppearanceNameAqua })?,
        resolve(unsafe { NSAppearanceNameDarkAqua })?,
    ))
}

pub fn watch_screens() {
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    let block = RcBlock::new(|_: NonNull<objc2_foundation::NSNotification>| {
        forget_menu_bars();
        forget_screens();
        forget_glow_screens();
        crate::mac::app::forget_screen_names();
        send(Native::ScreensChanged)
    });
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSApplicationDidChangeScreenParametersNotification),
            None,
            Some(&objc2_foundation::NSOperationQueue::mainQueue()),
            &block,
        )
    };
    let observer: Retained<AnyObject> = observer.into();
    keep(observer);
    // Unlocking and waking the screens can leave streams showing another
    // display with no screen parameter change after it: look again then.
    let again = RcBlock::new(|_: NonNull<objc2_foundation::NSNotification>| {
        log::info!("the screens woke or unlocked; streams are checked");
        forget_screens();
        crate::mac::app::forget_screen_names();
        send(Native::ScreensChanged)
    });
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace().notificationCenter();
    let observer = unsafe {
        workspace.addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSWorkspaceScreensDidWakeNotification),
            None,
            Some(&objc2_foundation::NSOperationQueue::mainQueue()),
            &again,
        )
    };
    let observer: Retained<AnyObject> = observer.into();
    keep(observer);
    let distributed = objc2_foundation::NSDistributedNotificationCenter::defaultCenter();
    let unlocked = NSString::from_str("com.apple.screenIsUnlocked");
    let observer = unsafe {
        distributed.addObserverForName_object_queue_usingBlock(
            Some(&unlocked),
            None,
            Some(&objc2_foundation::NSOperationQueue::mainQueue()),
            &again,
        )
    };
    let observer: Retained<AnyObject> = observer.into();
    keep(observer);
}

/// Count the system's front app changing, even for a moment, as when an
/// app starts: each change takes key status from the windows Agent Screen
/// focused in background apps, so input forwarders focus them again.
pub fn watch_activations() {
    let center = objc2_app_kit::NSWorkspace::sharedWorkspace().notificationCenter();
    super::input::note_activation(super::app::frontmost_pid());
    // The notification names the app, so no question goes to WindowServer;
    // frontmostApplication asks it synchronously and can wait seconds.
    let block = RcBlock::new(|note: NonNull<objc2_foundation::NSNotification>| {
        let app = unsafe { note.as_ref() }.userInfo().and_then(|info| {
            info.objectForKey(unsafe { objc2_app_kit::NSWorkspaceApplicationKey })
        });
        let pid = app
            .and_then(|app| app.downcast::<NSRunningApplication>().ok())
            .map(|app| app.processIdentifier())
            .or_else(super::app::frontmost_pid);
        let previous = super::input::front();
        super::input::note_activation(pid);
        if let Some(pid) = pid {
            send(Native::Activated { pid, previous });
        }
    });
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSWorkspaceDidActivateApplicationNotification),
            None,
            None,
            &block,
        )
    };
    let observer: Retained<AnyObject> = observer.into();
    keep(observer);
}

#[cfg(test)]
mod peek_activation_tests {
    use super::*;
    use agent_screen_model::peek::{Effect, Intent, Mode};

    #[test]
    fn status_button_down_and_up_share_one_action_owner_and_commit_once() {
        assert!(status_icon_mask().contains(NSEventMask::LeftMouseDown | NSEventMask::LeftMouseUp));
        assert!(!status_icon_mask().contains(NSEventMask::RightMouseDown));
        assert!(status_icon_mask().contains(NSEventMask::RightMouseUp));
        assert!(status_icon_event(Some(NSEventType::RightMouseDown), 9).is_none());
        assert!(matches!(
            status_icon_event(Some(NSEventType::RightMouseUp), 10),
            Some(Native::IconClicked {
                right: true,
                mouse: true
            })
        ));
        let mut intent = Intent::default();
        if let Native::IconPressed { gesture } =
            status_icon_event(Some(NSEventType::LeftMouseDown), 7).unwrap()
        {
            intent.press(gesture);
        }
        assert_eq!(intent.mode(), Mode::Closed);
        assert!(matches!(
            status_icon_event(Some(NSEventType::LeftMouseUp), 8).unwrap(),
            Native::IconClicked {
                mouse: true,
                right: false
            }
        ));
        assert_eq!(intent.release(true), Effect::Show);
        assert_eq!(intent.release(true), Effect::None);
        assert!(matches!(
            status_icon_event(None, 0).unwrap(),
            Native::IconClicked {
                mouse: false,
                right: false
            }
        ));
    }
}

#[cfg(test)]
mod menu_bar_cache_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_frame_after_the_first_does_not_ask_window_server_again() {
        // Display ids no real screen uses, so the shared cache can't clash.
        let (display, other) = (0xA5A5_0001, 0xA5A5_0002);
        let asked = Cell::new(0);
        let measure = || {
            asked.set(asked.get() + 1);
            Some(33.0)
        };
        assert_eq!(cached_menu_bar(display, measure), 33.0);
        for _ in 0..60 {
            assert_eq!(cached_menu_bar(display, measure), 33.0);
        }
        assert_eq!(asked.get(), 1, "a Viewer frame measured again");
        // No measurement, as off the main thread: the default, not kept.
        assert_eq!(cached_menu_bar(other, || None), 24.0);
        assert_eq!(cached_menu_bar(other, || Some(40.0)), 40.0);
        // The screens changed: measure once more.
        forget_menu_bars();
        assert_eq!(cached_menu_bar(display, || Some(37.0)), 37.0);
        assert_eq!(cached_menu_bar(display, measure), 37.0);
        assert_eq!(asked.get(), 1);
    }
}
