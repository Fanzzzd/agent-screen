//! Running apps and screens, through AppKit.

use crate::model::DisplayId;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSScreen, NSWorkspace};
use objc2_foundation::{NSNumber, NSString};
use std::path::{Path, PathBuf};

pub fn is_running(bundle_id: &str) -> bool {
    !NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .is_empty()
}

/// The pid of a running app at `bundle`, if any.
pub fn running_pid(bundle: &Path) -> Option<i32> {
    // Safari's /Applications entry is a link into a cryptex; the running
    // app reports where it really is.
    let bundle = std::fs::canonicalize(bundle).unwrap_or_else(|_| bundle.to_path_buf());
    let apps = NSWorkspace::sharedWorkspace().runningApplications();
    apps.iter().find_map(|app| {
        let url = PathBuf::from(app.bundleURL()?.path()?.to_string());
        let url = std::fs::canonicalize(&url).unwrap_or(url);
        (url == bundle).then(|| app.processIdentifier())
    })
}

/// Where the app with this pid is installed.
pub fn bundle_of(pid: i32) -> Option<PathBuf> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    Some(PathBuf::from(app.bundleURL()?.path()?.to_string()))
}

/// Whether the app started less than `span` ago.
pub fn launched_within(pid: i32, span: std::time::Duration) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.launchDate())
        .is_some_and(|date| -date.timeIntervalSinceNow() < span.as_secs_f64())
}

/// The frontmost app's pid.
pub fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
}

/// Whether the user opens files and apps through this process: a click in
/// it can open a window in any app.
pub fn opens_things(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.bundleIdentifier())
        .is_some_and(|id| {
            matches!(
                id.to_string().as_str(),
                "com.apple.finder"
                    | "com.apple.dock"
                    | "com.apple.Spotlight"
                    | "com.raycast.macos"
                    | "com.runningwithcrayons.Alfred"
            )
        })
}

/// The bundle of a running app, by bundle id.
pub fn running_bundle(bundle_id: &str) -> Option<PathBuf> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .iter()
        .find_map(|app| Some(PathBuf::from(app.bundleURL()?.path()?.to_string())))
}

/// Whether an app opens web pages, by the URL schemes its Info.plist
/// declares.
pub fn handles_web(bundle: &Path) -> bool {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::new());
    if let Some((_, web)) = SEEN.lock().unwrap().iter().find(|(p, _)| p == bundle) {
        return *web;
    }
    let web = read_handles_web(bundle);
    SEEN.lock().unwrap().push((bundle.to_path_buf(), web));
    web
}

fn read_handles_web(bundle: &Path) -> bool {
    std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleURLTypes", "json", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            text.contains("\"http\"") || text.contains("\"https\"")
        })
}

/// Show a hidden app's windows, where they are now.
pub fn unhide(pid: i32) {
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        app.unhide();
    }
}

/// An app by bundle id or name, as an .app path.
pub fn resolve(app: &str) -> Option<PathBuf> {
    if let Some(url) = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(app))
        && let Some(path) = url.path()
    {
        return Some(PathBuf::from(path.to_string()));
    }
    let name = if app.ends_with(".app") {
        app.to_string()
    } else {
        format!("{app}.app")
    };
    let home = std::env::var("HOME").unwrap_or_default();
    [
        "/Applications",
        "/System/Applications",
        "/System/Applications/Utilities",
        &format!("{home}/Applications"),
    ]
    .iter()
    .map(|folder| Path::new(folder).join(&name))
    .find(|path| path.exists())
}

/// Give an Agent Display the desktop picture that names it, so System
/// Settings, which shows each display with its desktop, tells it apart from
/// the user's monitors. False when its screen isn't listed yet or the
/// picture is missing, as in a bare debug binary. Main thread only.
pub fn label_display(display: DisplayId, mtm: MainThreadMarker) -> bool {
    let Some(picture) = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("Resources/AgentDisplay.png")))
        .filter(|path| path.exists())
    else {
        return false;
    };
    let Some(screen) = NSScreen::screens(mtm).iter().find(|screen| {
        screen
            .deviceDescription()
            .objectForKey(&NSString::from_str("NSScreenNumber"))
            .and_then(|n| n.downcast::<NSNumber>().ok())
            .is_some_and(|n| n.unsignedIntValue() == display)
    }) else {
        return false;
    };
    let url =
        objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(&picture.to_string_lossy()));
    let options = objc2_foundation::NSDictionary::new();
    unsafe {
        NSWorkspace::sharedWorkspace()
            .setDesktopImageURL_forScreen_options_error(&url, &screen, &options)
            .is_ok()
    }
}

/// The localized name of the screen showing `display`. Main thread only.
pub fn screen_name(display: DisplayId, mtm: MainThreadMarker) -> Option<String> {
    NSScreen::screens(mtm).iter().find_map(|screen| {
        let number = screen
            .deviceDescription()
            .objectForKey(&NSString::from_str("NSScreenNumber"))?;
        let number = number.downcast::<NSNumber>().ok()?;
        (number.unsignedIntValue() == display).then(|| screen.localizedName().to_string())
    })
}
