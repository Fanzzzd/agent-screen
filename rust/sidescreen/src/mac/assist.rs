//! The helper pinned to System Settings while the user grants a permission:
//! Sidescreen's icon, which drags into the permission's list, and one line
//! on what to do.
//!
//! A non-activating panel, so System Settings stays the active app. It
//! finds the Settings window in WindowServer's list, which gives bounds
//! without Screen Recording, and sits just below it, or inside its
//! bottom-right corner when the screen has no room.

use super::cg;
use super::native::primary_height;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSCursor, NSDragOperation, NSDraggingContext, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSEvent, NSFont, NSImageScaling, NSImageView, NSPanel,
    NSRunningApplication, NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSBundle, NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

const SETTINGS: &str = "com.apple.systempreferences";
const WIDTH: f64 = 392.0;
const HEIGHT: f64 = 92.0;
const INSET: f64 = 18.0;
/// Between the Settings window's bottom edge and the helper below it.
const GAP_BELOW: f64 = 10.0;
/// The icon's well, the icon in it, and the text beside them.
const TILE: f64 = 64.0;
const ICON: f64 = 52.0;
const INSET_X: f64 = 14.0;
const TEXT_X: f64 = INSET_X + TILE + 14.0;
const GAP: f64 = 3.0;

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "SidescreenDragTile"]
    #[ivars = ()]
    struct DragTile;

    unsafe impl NSObjectProtocol for DragTile {}

    unsafe impl NSDraggingSource for DragTile {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }

    impl DragTile {
        // The panel never becomes key, so the first click has to count.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::openHandCursor());
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let url = NSBundle::mainBundle().bundleURL();
            let item = NSDraggingItem::initWithPasteboardWriter(
                NSDraggingItem::alloc(),
                ProtocolObject::from_ref(&*url),
            );
            if let Some(path) = url.path() {
                let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
                icon.setSize(NSSize::new(ICON, ICON));
                let inset = (TILE - ICON) / 2.0;
                let frame = NSRect::new(NSPoint::new(inset, inset), NSSize::new(ICON, ICON));
                unsafe { item.setDraggingFrame_contents(frame, Some(&icon)) };
            }
            let items = NSArray::from_retained_slice(&[item]);
            self.beginDraggingSessionWithItems_event_source(
                &items,
                event,
                ProtocolObject::from_ref(self),
            );
        }
    }
);

impl DragTile {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

struct Assist {
    panel: Retained<NSPanel>,
    title: Retained<NSTextField>,
    detail: Retained<NSTextField>,
}

thread_local! {
    static ASSIST: RefCell<Option<Assist>> = const { RefCell::new(None) };
    static SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn label(
    text: &str,
    font: Retained<NSFont>,
    color: Retained<NSColor>,
    frame: NSRect,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let field = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    field.setFont(Some(&font));
    field.setTextColor(Some(&color));
    field.setFrame(frame);
    field
}

fn build(mtm: MainThreadMarker) -> Assist {
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, HEIGHT));
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setLevel(3); // NSFloatingWindowLevel
    panel.setHidesOnDeactivate(false);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(true);
    panel.setMovable(false);
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );

    let card = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
    card.setMaterial(NSVisualEffectMaterial::Popover);
    card.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    card.setState(NSVisualEffectState::Active);
    card.setWantsLayer(true);
    if let Some(layer) = card.layer() {
        layer.setCornerRadius(14.0);
        layer.setMasksToBounds(true);
    }

    // A well the icon sits in, so it reads as something to pick up.
    let tile = DragTile::new(
        NSRect::new(
            NSPoint::new(INSET_X, (HEIGHT - TILE) / 2.0),
            NSSize::new(TILE, TILE),
        ),
        mtm,
    );
    tile.setWantsLayer(true);
    if let Some(layer) = tile.layer() {
        layer.setCornerRadius(14.0);
        layer.setBackgroundColor(Some(&NSColor::quaternaryLabelColor().CGColor()));
        layer.setBorderWidth(1.0);
        layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
    }
    let inset = (TILE - ICON) / 2.0;
    let icon = NSImageView::initWithFrame(
        NSImageView::alloc(mtm),
        NSRect::new(NSPoint::new(inset, inset), NSSize::new(ICON, ICON)),
    );
    // The icon comes at 32 points; an image view only scales down by default.
    icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    if let Some(path) = NSBundle::mainBundle().bundleURL().path() {
        let image = NSWorkspace::sharedWorkspace().iconForFile(&path);
        image.setSize(NSSize::new(ICON, ICON));
        icon.setImage(Some(&image));
    }
    // Clicks go to the tile under it, which starts the drag.
    let _: () = unsafe { msg_send![&icon, unregisterDraggedTypes] };
    tile.addSubview(&icon);
    card.addSubview(&tile);

    let title = label(
        "",
        NSFont::boldSystemFontOfSize(13.0),
        NSColor::labelColor(),
        NSRect::ZERO,
        mtm,
    );
    let detail = label(
        "",
        NSFont::systemFontOfSize(11.5),
        NSColor::secondaryLabelColor(),
        NSRect::ZERO,
        mtm,
    );
    card.addSubview(&title);
    card.addSubview(&detail);
    panel.setContentView(Some(&card));
    Assist {
        panel,
        title,
        detail,
    }
}

/// Show the helper for `p`. `listed`: Sidescreen already has an entry in
/// the list, so the user only flips its switch.
pub fn show(p: super::permissions::Permission, listed: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    ASSIST.with(|a| {
        let mut a = a.borrow_mut();
        let assist = a.get_or_insert_with(|| build(mtm));
        let (title, detail) = if listed {
            (
                "打开列表里 Sidescreen 的开关".to_string(),
                format!(
                    "在“{}”列表里找到 Sidescreen。\n没有的话，把左边的图标拖进列表。",
                    p.name()
                ),
            )
        } else {
            (
                "把左边的图标拖进上面的列表".to_string(),
                format!("拖进“{}”列表后，打开 Sidescreen 的开关。", p.name()),
            )
        };
        assist.title.setStringValue(&NSString::from_str(&title));
        assist.detail.setStringValue(&NSString::from_str(&detail));
        // The two lines as one block, centered on the icon's well.
        let width = WIDTH - TEXT_X - INSET_X;
        let fit = |field: &NSTextField| {
            field.setPreferredMaxLayoutWidth(width);
            field.sizeThatFits(NSSize::new(width, 200.0)).height.ceil()
        };
        let (title_h, detail_h) = (fit(&assist.title), fit(&assist.detail));
        let bottom = ((HEIGHT - title_h - GAP - detail_h) / 2.0).round();
        assist.detail.setFrame(NSRect::new(
            NSPoint::new(TEXT_X, bottom),
            NSSize::new(width, detail_h),
        ));
        assist.title.setFrame(NSRect::new(
            NSPoint::new(TEXT_X, bottom + detail_h + GAP),
            NSSize::new(width, title_h),
        ));
    });
    SHOWN.set(true);
    tick();
}

/// Show the helper at a point, global top-left, without following System
/// Settings; for checking its look.
pub fn preview(p: super::permissions::Permission, listed: bool, x: f64, y: f64) {
    show(p, listed);
    SHOWN.set(false);
    ASSIST.with(|a| {
        if let Some(a) = a.borrow().as_ref() {
            a.panel
                .setFrameOrigin(NSPoint::new(x, primary_height() - y - HEIGHT));
            a.panel.orderFrontRegardless();
        }
    });
}

pub fn hide() {
    SHOWN.set(false);
    ASSIST.with(|a| {
        if let Some(a) = a.borrow().as_ref() {
            a.panel.orderOut(None);
        }
    });
}

pub fn is_shown() -> bool {
    SHOWN.get()
}

/// The System Settings window, in global top-left points.
fn settings_window() -> Option<crate::model::Rect> {
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        SETTINGS,
    ));
    let pids: Vec<i32> = apps.iter().map(|a| a.processIdentifier()).collect();
    if pids.is_empty() {
        return None;
    }
    cg::on_screen_windows()
        .into_iter()
        .find(|w| {
            pids.contains(&w.target.pid)
                && w.layer == 0
                && w.bounds.width > 520.0
                && w.bounds.height > 360.0
        })
        .map(|w| w.bounds)
}

/// Where the helper goes, Cocoa coordinates: just below the Settings
/// window, right edges aligned, so it covers none of the list; inside its
/// bottom-right corner when the screen has no room below.
fn place(w: crate::model::Rect) -> NSPoint {
    let x = w.x + w.width - WIDTH - INSET;
    let below = NSPoint::new(x, primary_height() - (w.y + w.height + GAP_BELOW) - HEIGHT);
    let fits = super::native::screens().iter().any(|(_, _, visible)| {
        below.x >= visible.origin.x
            && below.y >= visible.origin.y
            && below.x + WIDTH <= visible.origin.x + visible.size.width
            && below.y + HEIGHT <= visible.origin.y + visible.size.height
    });
    if fits {
        return below;
    }
    let top = w.y + w.height - HEIGHT - INSET;
    NSPoint::new(x, primary_height() - top - HEIGHT)
}

fn settings_frontmost() -> bool {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|a| a.bundleIdentifier())
        .is_some_and(|id| id.to_string() == SETTINGS)
}

/// Follow the Settings window. False once System Settings is gone, so the
/// caller can stop.
pub fn tick() -> bool {
    if !SHOWN.get() {
        return false;
    }
    let running = !NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(SETTINGS),
    )
    .is_empty();
    let window = settings_window();
    ASSIST.with(|a| {
        let a = a.borrow();
        let Some(a) = a.as_ref() else { return };
        match window.filter(|_| settings_frontmost()) {
            Some(w) => {
                a.panel.setFrameOrigin(place(w));
                a.panel.orderFrontRegardless();
            }
            None => a.panel.orderOut(None),
        }
    });
    running
}
