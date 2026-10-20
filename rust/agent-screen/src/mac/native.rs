//! AppKit pieces GPUI has no API for: the menu bar icon, native menus and
//! global mouse monitors.
//!
//! Native callbacks never touch GPUI state directly. They send `Native`
//! events down a channel, and a GPUI task handles them. Menus pop up from
//! the main dispatch queue, so their nested run loop never runs inside a
//! GPUI update.

use crate::i18n::t;
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
    /// A click on the menu bar icon; `right` for a right click.
    IconClicked {
        right: bool,
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
    /// The user clicked a notification about a display, or its Take Over
    /// button.
    NotificationClicked {
        display: u32,
        take_over: bool,
    },
    /// What macOS says about notifying, after `notify::check`.
    NotificationAuth(super::notify::Auth),
    /// Screens were added, removed or rearranged.
    ScreensChanged,
    /// The accent color or another system color changed.
    ColorsChanged,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    CreateDisplay,
    GrantPermissions,
    OpenAeroSpaceConfig,
    IgnoreInAeroSpace,
    ConnectAgents,
    ChooseWallpaper,
    DefaultWallpaper,
    Quit,
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

fn keep(object: Retained<AnyObject>) {
    KEEP.with(|k| k.borrow_mut().push(object));
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
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
            let right = NSApplication::sharedApplication(mtm)
                .currentEvent()
                .is_some_and(|e| e.r#type() == NSEventType::RightMouseUp);
            send(Native::IconClicked { right });
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
    asking: Cell<bool>,
    needs_setup: Cell<bool>,
}

impl StatusIcon {
    pub fn install(mtm: MainThreadMarker) -> StatusIcon {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-2.0); // NSSquareStatusItemLength
        let target: Retained<IconTarget> = unsafe { msg_send![IconTarget::alloc(mtm), init] };
        if let Some(button) = item.button(mtm) {
            unsafe {
                button.setTarget(Some(&target));
                button.setAction(Some(sel!(clicked:)));
                let _: isize = msg_send![&button, sendActionOn: (NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp).0];
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
            asking: Cell::new(false),
            needs_setup: Cell::new(false),
        };
        icon.redraw();
        icon
    }

    /// A raised hand while any Help Request is open.
    pub fn set_asking(&self, asking: bool) {
        self.asking.set(asking);
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
        let (symbol, description) = if self.needs_setup.get() {
            (
                "rectangle.on.rectangle.slash",
                t("Agent Screen: needs permissions", "Agent Screen：需要权限"),
            )
        } else if self.asking.get() {
            (
                "hand.raised.fill",
                t(
                    "Agent Screen: an agent is asking for help",
                    "Agent Screen：agent 在请求帮助",
                ),
            )
        } else if agent_screen_mcp::endpoint::is_dev_build() {
            ("rectangle.on.rectangle", "Agent Screen Dev")
        } else {
            ("rectangle.on.rectangle", "Agent Screen")
        };
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(description)),
        );
        if let Some(image) = image {
            image.setTemplate(true);
            button.setImage(Some(&image));
        }
        button.setToolTip(Some(&NSString::from_str(description)));
        // A development build shows purple, to tell it from the installed app.
        let tint = if self.needs_setup.get() || self.asking.get() {
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
    alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
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

/// Report every mouse down anywhere, and left-button drags, for Peek's
/// outside clicks and for window Handover by drag. Note the user's presses
/// for the catches.
pub fn watch_mouse() {
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp;
    let report = |event: &NSEvent| {
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
    };
    let global = RcBlock::new(move |event: NonNull<NSEvent>| report(unsafe { event.as_ref() }));
    if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global) {
        keep(monitor);
    }
    // The user's clicks and key presses in other apps, which tell their new
    // windows from an agent's. Events posted to a pid never reach these.
    let user = RcBlock::new(|event: NonNull<NSEvent>| {
        use super::catch::Towards;
        let towards = if unsafe { event.as_ref() }.r#type() == NSEventType::KeyDown {
            super::catch::user_pressed_key();
            return;
        } else {
            let mtm = MainThreadMarker::new().expect("monitors run on the main thread");
            let number = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
                NSEvent::mouseLocation(),
                0,
                mtm,
            );
            let front = super::input::front();
            u32::try_from(number).map_or(Towards::Unknown, |n| Towards::Window(n, front))
        };
        super::catch::user_acted(towards);
    });
    let presses = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::KeyDown;
    if let Some(monitor) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(presses, &user) {
        keep(monitor);
    }
    let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        report(unsafe { event.as_ref() });
        event.as_ptr()
    });
    if let Some(monitor) =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
    {
        keep(monitor);
    }
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
        Bool::YES
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
}

/// Show without taking the keyboard from the user's app.
pub fn order_front(window: &NSWindow) {
    window.orderFrontRegardless();
}

pub fn order_out(window: &NSWindow) {
    window.orderOut(None);
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

/// Every screen: display id, frame and visible frame, Cocoa coordinates.
pub fn screens() -> Vec<(u32, NSRect, NSRect)> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    objc2_app_kit::NSScreen::screens(mtm)
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
        .collect()
}

/// Height of the menu bar on a display, in points.
pub fn menu_bar_height(display: u32) -> f64 {
    let Some(mtm) = MainThreadMarker::new() else {
        return 24.0;
    };
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
            return bar.max(24.0);
        }
    }
    24.0
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
    let block =
        RcBlock::new(|_: NonNull<objc2_foundation::NSNotification>| send(Native::ScreensChanged));
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
            .map(|app| app.processIdentifier());
        super::input::note_activation(pid.or_else(super::app::frontmost_pid))
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

/// The app's own icon as PNG, for drawing it in GPUI.
pub fn app_icon_png() -> Option<Vec<u8>> {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    let path = objc2_foundation::NSBundle::mainBundle()
        .bundleURL()
        .path()?;
    let icon = objc2_app_kit::NSWorkspace::sharedWorkspace().iconForFile(&path);
    icon.setSize(objc2_foundation::NSSize::new(256.0, 256.0));
    let tiff = icon.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let png = unsafe {
        rep.representationUsingType_properties(
            NSBitmapImageFileType::PNG,
            &objc2_foundation::NSDictionary::new(),
        )
    }?;
    Some(png.to_vec())
}
