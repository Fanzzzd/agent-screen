//! A system notification for each Help Request, withdrawn when the request
//! ends. Clicking one opens the display.

use super::native::{self, Native};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSArray, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::cell::RefCell;
use std::collections::HashMap;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "SidescreenNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // Show the banner even though Sidescreen is the sender.
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let id = response.notification().request().identifier().to_string();
            if let Some(display) = id.strip_prefix("help-").and_then(|d| d.parse::<u32>().ok()) {
                native::main_async(move || native::send(Native::HelpClicked { display }));
            }
            handler.call(());
        }
    }
);

thread_local! {
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    /// The reason last posted for each display.
    static POSTED: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
}

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

pub fn install() {
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
    center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
}

/// Post, replace or withdraw the notification of one display.
pub fn update(display: u32, label: &str, reason: Option<&str>) {
    let identifier = NSString::from_str(&format!("help-{display}"));
    let Some(reason) = reason else {
        if POSTED.with(|p| p.borrow_mut().remove(&display)).is_some() {
            center().removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&[
                identifier,
            ]));
        }
        return;
    };
    if POSTED.with(|p| p.borrow().get(&display).map(String::as_str) == Some(reason)) {
        return;
    }
    POSTED.with(|p| p.borrow_mut().insert(display, reason.to_string()));

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&format!("{label} 需要你帮忙")));
    content.setBody(&NSString::from_str(reason));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let added = RcBlock::new(|error: *mut NSError| match unsafe { error.as_ref() } {
        Some(error) => log::warn!("notification failed: {}", error.localizedDescription()),
        None => log::debug!("notification posted"),
    });
    let authorized = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } {
            log::warn!(
                "notifications unavailable: {}",
                error.localizedDescription()
            );
        }
        if granted.as_bool() {
            center().addNotificationRequest_withCompletionHandler(&request, Some(&added));
        }
    });
    center().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &authorized,
    );
}
