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
    let apps = NSWorkspace::sharedWorkspace().runningApplications();
    apps.iter().find_map(|app| {
        let url = app.bundleURL()?.path()?.to_string();
        (Path::new(&url) == bundle).then(|| app.processIdentifier())
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

/// The app in front on the user's screen.
pub fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
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
