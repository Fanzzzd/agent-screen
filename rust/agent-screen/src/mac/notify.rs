//! System notifications: one while a display's Help Request is open,
//! withdrawn when it ends, and one when an agent leaves windows on its
//! display. Clicking one opens that display in the Viewer; a Help Request's
//! Take Over button also starts a Takeover.
//!
//! Agent Screen asks for permission from the setup window, or once at launch
//! for people who set up before notifications existed; never while posting,
//! when an agent is already waiting.

use super::native::{self, Native};
use crate::i18n::t;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSArray, NSError, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Whether macOS lets Agent Screen notify.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Auth {
    /// Not known yet.
    #[default]
    Unknown,
    /// macOS hasn't asked the user.
    NotAsked,
    /// The user said no, or turned them off in System Settings.
    Denied,
    Allowed,
}

/// What a notification tells the user about a display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The agent asks for help, with its reason.
    Help(String),
    /// The agent stopped using its display and left windows on it.
    Review { windows: usize },
    /// The agent's connection closed and its windows stayed.
    Disconnected { windows: usize },
}

impl Note {
    fn prefix(&self) -> &'static str {
        match self {
            Note::Help(_) => "help",
            Note::Review { .. } | Note::Disconnected { .. } => "review",
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "AgentScreenNotificationDelegate"]
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
            // Show the banner even though Agent Screen is the sender.
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
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
            let display = id.split_once('-').and_then(|(_, d)| d.parse::<u32>().ok());
            let take_over = response.actionIdentifier().to_string() == TAKE_OVER;
            if let Some(display) = display {
                native::main_async(move || {
                    native::send(Native::NotificationClicked { display, take_over })
                });
            }
            handler.call(());
        }
    }
);

thread_local! {
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    /// What each display's notifications last said, by identifier.
    static POSTED: RefCell<HashMap<String, Note>> = RefCell::new(HashMap::new());
}

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

fn identifier(prefix: &str, display: u32) -> String {
    format!("{prefix}-{display}")
}

fn withdraw_id(id: &str) {
    let ids = NSArray::from_retained_slice(&[NSString::from_str(id)]);
    center().removePendingNotificationRequestsWithIdentifiers(&ids);
    center().removeDeliveredNotificationsWithIdentifiers(&ids);
}

/// The Help Request notification's button.
const TAKE_OVER: &str = "take-over";
const HELP_CATEGORY: &str = "help";

pub fn install() {
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
    center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let take_over = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(TAKE_OVER),
        &NSString::from_str(t("Take Over", "接手操作")),
        UNNotificationActionOptions::Foreground,
    );
    let help = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(HELP_CATEGORY),
        &NSArray::from_retained_slice(&[take_over]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    center().setNotificationCategories(&NSSet::from_retained_slice(&[help]));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
}

/// Ask macOS whether Agent Screen may notify; the answer arrives as
/// `Native::NotificationAuth`.
pub fn check() {
    let done = RcBlock::new(|settings: core::ptr::NonNull<UNNotificationSettings>| {
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let auth = if status == UNAuthorizationStatus::NotDetermined {
            Auth::NotAsked
        } else if status == UNAuthorizationStatus::Denied {
            Auth::Denied
        } else {
            Auth::Allowed
        };
        native::main_async(move || native::send(Native::NotificationAuth(auth)));
    });
    center().getNotificationSettingsWithCompletionHandler(&done);
}

/// Show macOS's prompt, when it still shows one; then check again.
pub fn request() {
    let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } {
            log::warn!("notifications: {}", error.localizedDescription());
        }
        log::info!("notifications allowed: {}", granted.as_bool());
        check();
    });
    center().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &done,
    );
}

/// Post `note` about a display, replacing what its notification of the
/// same kind said; or withdraw both of its notifications with `None`.
/// Posting the same note again does nothing. `agent` names the client,
/// `task` is the rest of the Display Label.
pub fn update(display: u32, agent: &str, task: &str, note: Option<Note>) {
    let Some(note) = note else {
        for prefix in ["help", "review"] {
            withdraw(display, prefix);
        }
        return;
    };
    let id = identifier(note.prefix(), display);
    if POSTED.with(|p| p.borrow().get(&id) == Some(&note)) {
        return;
    }
    POSTED.with(|p| p.borrow_mut().insert(id.clone(), note.clone()));

    let (title, body) = match &note {
        Note::Help(reason) => (
            tf!("{agent} needs your help", "{agent} 需要你帮忙"),
            reason.clone(),
        ),
        Note::Review { windows } => (
            tf!("{agent} is done with its display", "{agent} 用完了显示器"),
            windows_waiting(*windows),
        ),
        Note::Disconnected { windows } => (
            tf!("{agent} disconnected", "{agent} 的连接断了"),
            tf!(
                "{windows} windows are still on its display",
                "{windows} 个窗口还在它的显示器上"
            ),
        ),
    };
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&title));
    content.setSubtitle(&NSString::from_str(task));
    content.setBody(&NSString::from_str(&body));
    if matches!(note, Note::Help(_)) {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        content.setCategoryIdentifier(&NSString::from_str(HELP_CATEGORY));
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&id),
        &content,
        None,
    );
    let added = RcBlock::new(move |error: *mut NSError| match unsafe { error.as_ref() } {
        // Not allowed: the Help Card and the menu bar icon still show it.
        Some(error) => log::info!("notification not posted: {}", error.localizedDescription()),
        None => {
            let id = id.clone();
            // Withdrawn while macOS was adding it.
            native::main_async(move || {
                if POSTED.with(|p| !p.borrow().contains_key(&id)) {
                    withdraw_id(&id);
                }
            });
        }
    });
    center().addNotificationRequest_withCompletionHandler(&request, Some(&added));
}

/// Withdraw only the Help Request's notification of a display.
pub fn withdraw_help(display: u32) {
    withdraw(display, "help");
}

fn withdraw(display: u32, prefix: &str) {
    let id = identifier(prefix, display);
    if POSTED.with(|p| p.borrow_mut().remove(&id)).is_some() {
        withdraw_id(&id);
    }
}

fn windows_waiting(windows: usize) -> String {
    if windows == 1 {
        t("1 window waits for your review", "1 个窗口等你查看").to_string()
    } else {
        tf!(
            "{windows} windows wait for your review",
            "{windows} 个窗口等你查看"
        )
    }
}
