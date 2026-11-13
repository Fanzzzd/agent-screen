//! System notifications: one while a display's Help, Window or Control
//! Request is open, withdrawn when it ends, and one when an agent leaves
//! windows on its display. Clicking one opens that display in the Viewer. A
//! Help Request's Take Over also starts a Takeover; its Continue and Reply,
//! offered only to an agent that understands them (ADR 0015), answer it
//! without opening anything. A Help Request's notification carries the
//! request's id, so an answer to a request the agent has replaced since
//! changes nothing.
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
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
    UNTextInputNotificationAction, UNTextInputNotificationResponse, UNUserNotificationCenter,
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

/// What the user did with a notification about a display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Clicked it: open the display.
    Open,
    /// A Help Request's Take Over.
    TakeOver,
    /// A Help Request's Continue: the agent goes on without a Takeover.
    Continue,
    /// A Help Request's Reply, with what the user typed.
    Reply(String),
}

/// What a notification tells the user about a display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The agent asks for help, with its reason. `answers`: its agent
    /// understands Continue and Reply.
    Help {
        reason: String,
        request: u64,
        answers: bool,
    },
    /// An operation is waiting for approval in the shared input queue.
    Input(String),
    /// The agent asks for a window of this app. `holder`: another Agent
    /// Session's Display Label, and whether it is a Review Display, when
    /// one holds the window (ADR 0017).
    Window {
        app: String,
        holder: Option<(String, agent_screen_model::held::Place)>,
    },
    /// The user brought an app to the front whose windows are all on Agent
    /// Displays: its name, and those displays' labels.
    Elsewhere { app: String, places: Vec<String> },
    /// During a Takeover, the agent asks for its display back, with its
    /// reason.
    Control(String),
    /// The agent stopped using its display and left windows on it.
    Review { windows: usize },
    /// The agent's connection closed and its windows stayed.
    Disconnected { windows: usize },
}

impl Note {
    fn prefix(&self) -> &'static str {
        match self {
            Note::Help { .. } => "help",
            Note::Input(_) => "input",
            Note::Window { .. } => "window",
            Note::Elsewhere { .. } => "where",
            Note::Control(_) => "control",
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
            let action = match response.actionIdentifier().to_string().as_str() {
                TAKE_OVER => Action::TakeOver,
                CONTINUE => Action::Continue,
                REPLY => {
                    let text = response
                        .downcast_ref::<UNTextInputNotificationResponse>()
                        .map(|r| r.userText().to_string())
                        .unwrap_or_default();
                    Action::Reply(text)
                }
                _ => Action::Open,
            };
            let request = response
                .notification()
                .request()
                .content()
                .userInfo()
                .objectForKey(&NSString::from_str(REQUEST_KEY))
                .and_then(|n| n.downcast::<NSNumber>().ok())
                .map(|n| n.as_u64());
            if id.starts_with("input-") {
                native::main_async(|| native::send(Native::InputQueueOpened));
            } else if let Some(display) = display {
                native::main_async(move || {
                    let request = answering(&id, request);
                    native::send(Native::NotificationClicked {
                        display,
                        request,
                        action,
                    })
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
    /// The Help Request id each delivered help notification carries. A
    /// request asked again with the same words keeps the banner, and the
    /// banner answers the newest request.
    static SHOWN: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// The Help Request an answer from notification `id` is for: the one it
/// was posted for, or, when the agent asked again with the same words and
/// the banner stayed, the newest.
fn answering(id: &str, carried: Option<u64>) -> Option<u64> {
    let carried = carried?;
    let shown = SHOWN.with(|s| s.borrow().get(id).copied());
    let current = POSTED.with(|p| match p.borrow().get(id) {
        Some(Note::Help { request, .. }) => Some(*request),
        _ => None,
    });
    match current {
        Some(current) if shown == Some(carried) => Some(current),
        _ => Some(carried),
    }
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

/// The Help Request notification's buttons.
const TAKE_OVER: &str = "take-over";
const CONTINUE: &str = "continue";
const REPLY: &str = "reply";
/// Take Over only, for an agent that understands nothing else.
const HELP_CATEGORY: &str = "help";
/// Take Over, Continue and Reply.
const ANSWERS_CATEGORY: &str = "help-answers";
/// The user info key of a Help Request notification's request id.
const REQUEST_KEY: &str = "request";

pub fn install() {
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
    center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let take_over = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(TAKE_OVER),
        &NSString::from_str(t("Take Over", "接手操作")),
        UNNotificationActionOptions::Foreground,
    );
    // Continue and Reply answer without opening anything.
    let go_on = UNNotificationAction::actionWithIdentifier_title_options(
        &NSString::from_str(CONTINUE),
        &NSString::from_str(t("Continue", "继续")),
        UNNotificationActionOptions::empty(),
    );
    let reply = UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
        &NSString::from_str(REPLY),
        &NSString::from_str(t("Reply", "回复")),
        UNNotificationActionOptions::empty(),
        &NSString::from_str(t("Send", "发送")),
        &NSString::from_str(t("Tell the agent what to do", "告诉它怎么做")),
    );
    let help = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(HELP_CATEGORY),
        &NSArray::from_retained_slice(std::slice::from_ref(&take_over)),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    let answers = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(ANSWERS_CATEGORY),
        &NSArray::from_retained_slice(&[take_over, go_on, Retained::into_super(reply)]),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    center().setNotificationCategories(&NSSet::from_retained_slice(&[help, answers]));
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
        for prefix in ["help", "window", "where", "control", "review", "input"] {
            withdraw(display, prefix);
        }
        return;
    };
    let id = identifier(note.prefix(), display);
    let before = POSTED.with(|p| p.borrow().get(&id).cloned());
    if before.as_ref() == Some(&note) {
        return;
    }
    POSTED.with(|p| p.borrow_mut().insert(id.clone(), note.clone()));
    let same_words = matches!(
        (&before, &note),
        (Some(Note::Help { reason: old, .. }), Note::Help { reason, .. }) if old == reason
    );
    // The agent asked again with the same words: the banner stays, and
    // answers the new request (`answering`).
    let same_actions = matches!(
        (&before, &note),
        (Some(Note::Help { answers: old, .. }), Note::Help { answers, .. }) if old == answers
    );
    if same_words && same_actions {
        return;
    }

    let (title, body) = match &note {
        Note::Input(reason) => (
            tf!("{agent} has an input request", "{agent} 有一个输入请求"),
            tf!(
                "{reason} Review the shared queue under the menu bar icon. This build cannot execute real input.",
                "{reason} 在菜单栏图标下查看共享队列。本构建无法执行真实输入。"
            ),
        ),
        Note::Help { reason, .. } => (
            tf!("{agent} needs your help", "{agent} 需要你帮忙"),
            reason.clone(),
        ),
        Note::Window { app, holder: None } => (
            tf!(
                "{agent} asks for your {app} window",
                "{agent} 想要你的 {app} 窗口"
            ),
            t(
                "Choose Hand Over or Keep on the card under the menu bar icon.",
                "在菜单栏图标下的卡片中选择交给它或保留。",
            )
            .to_string(),
        ),
        Note::Window {
            app,
            holder: Some((holder, place)),
        } => {
            use agent_screen_model::held::Place;
            let choose = t(
                "Choose Hand Over or Keep on the card under the menu bar icon.",
                "在菜单栏图标下的卡片中选择交给它或保留。",
            );
            let where_it_is = match place {
                Place::Review => tf!(
                    "It waits for your review on {holder}.",
                    "它在待查看的 {holder} 上。"
                ),
                Place::Display => tf!("{holder} is using it.", "{holder} 正在使用。"),
                Place::UserScreen => tf!(
                    "It is on your screen, and {holder} started it.",
                    "它在你的屏幕上，由 {holder} 打开。"
                ),
            };
            let space = if crate::i18n::lang() == crate::i18n::Lang::En {
                " "
            } else {
                ""
            };
            (
                tf!("{agent} asks for {app}", "{agent} 想要 {app}"),
                format!("{where_it_is}{space}{choose}"),
            )
        }
        Note::Elsewhere { app, places } => {
            let quoted = |open: &str, close: &str, join: &str| {
                places
                    .iter()
                    .map(|p| format!("{open}{p}{close}"))
                    .collect::<Vec<_>>()
                    .join(join)
            };
            let (en, zh) = (quoted("“", "”", ", "), quoted("“", "”", "、"));
            (
                tf!(
                    "{app}'s window is in Agent Screen",
                    "{app} 的窗口在 Agent Screen 里"
                ),
                tf!(
                    "On {en}. Click to open it in the Viewer.",
                    "在 {zh} 上。点按在 Viewer 中打开。"
                ),
            )
        }
        Note::Control(reason) => (
            tf!("{agent} asks to continue", "{agent} 想继续操作"),
            tf!(
                "{reason} Answer from the card under the menu bar icon.",
                "{reason} 在菜单栏图标下的卡片里回答。"
            ),
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
    if matches!(note, Note::Window { .. } | Note::Control(_)) {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    if let Note::Help {
        request, answers, ..
    } = &note
    {
        if !same_words {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        content.setCategoryIdentifier(&NSString::from_str(if *answers {
            ANSWERS_CATEGORY
        } else {
            HELP_CATEGORY
        }));
        SHOWN.with(|s| s.borrow_mut().insert(id.clone(), *request));
        let key = NSString::from_str(REQUEST_KEY);
        let value = NSNumber::new_u64(*request);
        let info = NSDictionary::<NSString, NSNumber>::from_slices(&[&*key], &[&*value]);
        // SAFETY: the dictionary's generic types only narrow its keys and
        // values, and a string to a number is a property list, as user
        // info must be.
        unsafe {
            let info: Retained<NSDictionary> = Retained::cast_unchecked(info);
            content.setUserInfo(&info);
        }
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

/// Withdraw only the Window Request's notification of a display.
pub fn withdraw_window(display: u32) {
    withdraw(display, "window");
}

/// Withdraw the note saying where an app's windows are, so the next one
/// shows again.
pub fn withdraw_where(display: u32) {
    withdraw(display, "where");
}

/// Withdraw only the Control Request's notification of a display.
pub fn withdraw_control(display: u32) {
    withdraw(display, "control");
}

pub fn withdraw_input(display: u32) {
    withdraw(display, "input");
}

fn withdraw(display: u32, prefix: &str) {
    let id = identifier(prefix, display);
    SHOWN.with(|s| s.borrow_mut().remove(&id));
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
