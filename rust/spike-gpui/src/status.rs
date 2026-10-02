//! The menu bar icon and focus probes, through objc2: GPUI has no status
//! item API.

use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSImage, NSStatusBar, NSVariableStatusItemLength, NSWorkspace};
use objc2_foundation::NSString;
use std::cell::RefCell;

thread_local! {
    static TOGGLE: RefCell<Option<UnboundedSender<()>>> = const { RefCell::new(None) };
    static KEEP: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SpikeStatusTarget"]
    struct StatusTarget;

    impl StatusTarget {
        #[unsafe(method(clicked:))]
        fn clicked(&self, _sender: &AnyObject) {
            TOGGLE.with(|t| {
                if let Some(tx) = t.borrow().as_ref() {
                    let _ = tx.unbounded_send(());
                }
            });
        }
    }
);

pub fn install(toggle: UnboundedSender<()>) {
    let mtm = MainThreadMarker::new().expect("main thread");
    TOGGLE.with(|t| *t.borrow_mut() = Some(toggle));
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let target: Retained<StatusTarget> = unsafe { msg_send![StatusTarget::alloc(mtm), init] };
    if let Some(button) = item.button(mtm) {
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("rectangle.on.rectangle.angled"),
            Some(&NSString::from_str("Sidescreen spike")),
        );
        if let Some(image) = image {
            image.setTemplate(true);
            button.setImage(Some(&image));
        } else {
            button.setTitle(&NSString::from_str("S"));
        }
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(clicked:)));
        }
    }
    KEEP.with(|k| {
        k.borrow_mut()
            .push(Retained::into_super(Retained::into_super(item)));
        k.borrow_mut().push(Retained::into_super(target).into());
    });
}

/// The frontmost app's name: it must stay the user's app throughout.
pub fn frontmost() -> String {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| app.localizedName())
        .map(|name| name.to_string())
        .unwrap_or_default()
}
