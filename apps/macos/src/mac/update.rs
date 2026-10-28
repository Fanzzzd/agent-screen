//! Agent Screen updates itself through Sparkle (ADR 0012).
//!
//! Release builds carry Sparkle.framework. Sparkle checks the feed of the
//! latest GitHub release once a day and downloads a new version in the
//! background, checked against the EdDSA key in Info.plist. Installing
//! replaces the app and restarts it, which ends every Agent Display, so it
//! waits: Sparkle hands Agent Screen the install, and Agent Screen runs it
//! once no Agent Display is open (`Shell::watch_updates`), or when the user
//! picks Restart to Update. Quitting installs it too.
//!
//! The framework loads at run time, so a build without it, such as the
//! development bundle, runs without updates.

use super::native::{self, MenuAction, Native};
use block2::{DynBlock, RcBlock};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSBundle, NSString};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Agent Displays open now, kept by the Shell, for Sparkle's callbacks.
static OPEN_DISPLAYS: AtomicUsize = AtomicUsize::new(0);

/// A downloaded update: its version, and what installs it and restarts
/// the app.
type Pending = (String, RcBlock<dyn Fn()>);

thread_local! {
    static CONTROLLER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    /// A downloaded update waiting to be installed.
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "AgentScreenUpdaterDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    impl Delegate {
        /// A test feed in place of the release's, for scripts only.
        #[unsafe(method_id(feedURLStringForUpdater:))]
        fn feed(&self, _updater: &AnyObject) -> Option<Retained<NSString>> {
            test_feed().map(|feed| NSString::from_str(&feed))
        }

        /// Sparkle downloaded an update and would install it when the app
        /// quits: Agent Screen takes the install, to run it once no Agent
        /// Display is open.
        #[unsafe(method(updater:willInstallUpdateOnQuit:immediateInstallationBlock:))]
        fn will_install_on_quit(
            &self,
            _updater: &AnyObject,
            item: &AnyObject,
            install: &DynBlock<dyn Fn()>,
        ) -> Bool {
            let version = display_version(item);
            log::info!("update: {version} is downloaded and waits to be installed");
            PENDING.with(|p| *p.borrow_mut() = Some((version, install.copy())));
            native::send(Native::UpdateReady);
            Bool::YES
        }

        /// The user chose Install and Relaunch in Sparkle's own window.
        /// With Agent Displays open, they confirm first.
        #[unsafe(method(updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:))]
        fn postpone_relaunch(
            &self,
            _updater: &AnyObject,
            item: &AnyObject,
            install: &DynBlock<dyn Fn()>,
        ) -> Bool {
            if OPEN_DISPLAYS.load(Ordering::Relaxed) == 0 {
                return Bool::NO;
            }
            let version = display_version(item);
            let install = install.copy();
            PENDING.with(|p| *p.borrow_mut() = Some((version, install)));
            native::main_async(|| {
                native::send(Native::Menu(MenuAction::InstallUpdate));
            });
            Bool::YES
        }
    }
);

fn display_version(item: &AnyObject) -> String {
    let version: Option<Retained<NSString>> = unsafe { msg_send![item, displayVersionString] };
    version.map(|v| v.to_string()).unwrap_or_default()
}

/// A feed for tests: AGENTSCREEN_UPDATE_FEED, honored only where the test
/// control methods are.
fn test_feed() -> Option<String> {
    let debug =
        cfg!(debug_assertions) || std::env::var_os("AGENTSCREEN_DEBUG").is_some_and(|v| v == "1");
    debug
        .then(|| std::env::var("AGENTSCREEN_UPDATE_FEED").ok())
        .flatten()
        .filter(|f| !f.is_empty())
}

/// Start Sparkle, when this build carries it. Only the user's own Agent
/// Screen updates itself; a test or development instance doesn't, unless
/// a script gives it a test feed.
pub fn start() {
    if agent_screen_mcp::endpoint::instance().is_some() && test_feed().is_none() {
        return;
    }
    let Some(frameworks) = NSBundle::mainBundle().privateFrameworksPath() else {
        return;
    };
    let path = format!("{frameworks}/Sparkle.framework");
    let Some(bundle) = NSBundle::bundleWithPath(&NSString::from_str(&path)) else {
        return;
    };
    // Sparkle, signed with the app, from its own Frameworks folder.
    if !unsafe { bundle.load() } {
        log::warn!("update: couldn't load {path}");
        return;
    }
    let Some(class) = AnyClass::get(c"SPUStandardUpdaterController") else {
        log::warn!("update: Sparkle has no SPUStandardUpdaterController");
        return;
    };
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
    let controller: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![
            allocated,
            initWithStartingUpdater: true,
            updaterDelegate: &*delegate,
            userDriverDelegate: std::ptr::null::<AnyObject>()
        ]
    };
    let Some(controller) = controller else {
        return;
    };
    log::info!("update: Sparkle started");
    CONTROLLER.with(|c| *c.borrow_mut() = Some(controller));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
}

/// Whether this build updates itself.
pub fn available() -> bool {
    CONTROLLER.with(|c| c.borrow().is_some())
}

/// Check now, with Sparkle's window: Check for Updates… in the menu.
pub fn check() {
    CONTROLLER.with(|c| {
        if let Some(controller) = c.borrow().as_ref() {
            let _: () =
                unsafe { msg_send![&**controller, checkForUpdates: std::ptr::null::<AnyObject>()] };
        }
    });
}

/// Check now without a window, as the daily check does; for scripts.
pub fn check_in_background() {
    CONTROLLER.with(|c| {
        if let Some(controller) = c.borrow().as_ref() {
            let updater: Retained<AnyObject> = unsafe { msg_send![&**controller, updater] };
            let _: () = unsafe { msg_send![&*updater, checkForUpdatesInBackground] };
        }
    });
}

/// The version of the update waiting to be installed.
pub fn pending() -> Option<String> {
    PENDING.with(|p| p.borrow().as_ref().map(|(v, _)| v.clone()))
}

/// Install the waiting update and restart Agent Screen. Ends every Agent
/// Display.
pub fn install() {
    let block = PENDING.with(|p| p.borrow_mut().take()).map(|(_, b)| b);
    if let Some(block) = block {
        log::info!("update: installing and restarting");
        block.call(());
    }
}

pub fn set_open_displays(count: usize) {
    OPEN_DISPLAYS.store(count, Ordering::Relaxed);
}
