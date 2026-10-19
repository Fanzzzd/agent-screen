//! Running apps and screens, through AppKit.

use crate::model::DisplayId;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSScreen, NSWorkspace};
use objc2_foundation::{NSNumber, NSObjectProtocol, NSString};
use std::path::{Path, PathBuf};

pub fn is_running(bundle_id: &str) -> bool {
    !NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
        .is_empty()
}

/// Every running instance of the app at `bundle`. Reads the process table
/// rather than asking AppKit for every running app and its bundle, which
/// takes tens of milliseconds on a busy Mac; launch_app asks every 20 ms
/// while an app starts, so this is safe to call off the main thread too.
pub fn running_pids(bundle: &Path) -> Vec<i32> {
    // Safari's /Applications entry is a link into a cryptex; the running
    // app reports where it really is.
    let bundle = std::fs::canonicalize(bundle).unwrap_or_else(|_| bundle.to_path_buf());
    let Some(executable) = main_executable(&bundle) else {
        return Vec::new();
    };
    process_ids()
        .into_iter()
        .filter(|&pid| process_path(pid).as_deref() == Some(executable.as_path()))
        // A command-line run of the same binary isn't the app.
        .filter(|&pid| NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some())
        .collect()
}

pub fn running_pid(bundle: &Path) -> Option<i32> {
    running_pids(bundle).into_iter().next()
}

/// The bundle's main executable, as its Info.plist names it.
fn main_executable(bundle: &Path) -> Option<PathBuf> {
    let plist = std::fs::read(bundle.join("Contents/Info.plist")).ok()?;
    let name = info_value(&plist, "CFBundleExecutable")?;
    let path = bundle.join("Contents/MacOS").join(name);
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

/// A string value from an Info.plist, XML or binary.
fn info_value(plist: &[u8], key: &str) -> Option<String> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::data::CFData;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::propertylist::{CFPropertyList, create_with_data};
    use core_foundation::string::CFString;
    let (list, _) = create_with_data(CFData::from_buffer(plist), 0).ok()?;
    let list =
        unsafe { CFPropertyList::wrap_under_create_rule(list) }.downcast_into::<CFDictionary>()?;
    let list: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(list.as_concrete_TypeRef()) };
    list.find(CFString::new(key))?
        .downcast::<CFString>()
        .map(|s| s.to_string())
}

/// Every process id on the Mac.
fn process_ids() -> Vec<i32> {
    let mut pids = vec![0i32; 4096];
    let bytes = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if bytes <= 0 {
        return Vec::new();
    }
    pids.truncate(bytes as usize);
    pids.retain(|&pid| pid > 0);
    pids
}

/// The executable a process runs.
fn process_path(pid: i32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    Some(PathBuf::from(String::from_utf8(buffer).ok()?))
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

/// An app's name as the user sees it.
pub fn name(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.localizedName())
        .map(|name| name.to_string())
}

/// Bring an app to the front through NSRunningApplication; false when
/// macOS refuses.
pub fn activate(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|app| {
        app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty())
    })
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
pub fn is_hidden(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .is_some_and(|app| app.isHidden())
}

/// Hide an app, as Command-H would, without touching the front app.
pub fn hide(pid: i32) {
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        app.hide();
    }
}

/// Show a hidden app without activating it. An app that made itself an
/// accessory after it started hidden, as Electron development builds that
/// open windows in the background do, ignores `unhide`; it takes the
/// accessibility attribute.
pub fn unhide(pid: i32) {
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        app.unhide();
    }
    super::ax::show_app(pid);
}

/// An app by bundle id or name, as an .app path.
pub fn resolve(app: &str) -> Option<PathBuf> {
    // A bundle by path, as a build under /tmp that Launch Services skips.
    let home = std::env::var("HOME").unwrap_or_default();
    let by_path = match app.strip_prefix("~/") {
        Some(rest) => Some(Path::new(&home).join(rest)),
        None => app.starts_with('/').then(|| PathBuf::from(app)),
    };
    if let Some(path) = by_path {
        return path.join("Contents/Info.plist").exists().then_some(path);
    }
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

/// Where Agent Screen keeps the picture the user chose for Agent Displays.
fn support_folder() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/Agent Screen"))
}

/// The picture the user chose for Agent Displays, if any: a copy, so moving
/// or deleting the original changes nothing.
pub fn custom_wallpaper() -> Option<std::path::PathBuf> {
    std::fs::read_dir(support_folder()?)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("wallpaper-"))
        })
}

/// Keep a copy of `picture` as Agent Displays' desktop picture, or go back
/// to Agent Screen's own with None. Each copy gets a new name: macOS skips
/// setting a picture whose URL it already shows.
pub fn set_wallpaper(picture: Option<&Path>) -> Result<(), String> {
    let folder = support_folder().ok_or("no home folder")?;
    if let Some(old) = custom_wallpaper() {
        std::fs::remove_file(&old).map_err(|e| format!("{}: {e}", old.display()))?;
    }
    let Some(picture) = picture else {
        return Ok(());
    };
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let extension = picture
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let copy = folder.join(format!("wallpaper-{stamp}.{extension}"));
    std::fs::copy(picture, &copy).map_err(|e| format!("{}: {e}", picture.display()))?;
    Ok(())
}

/// The picture for Agent Displays: the user's, else Agent Screen's, which
/// names the display. None in a bare debug binary without either.
fn wallpaper() -> Option<std::path::PathBuf> {
    custom_wallpaper().or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.parent()?.join("Resources/AgentDisplay.png")))
            .filter(|path| path.exists())
    })
}

/// Give an Agent Display its desktop picture: the user's choice, or the
/// one that names it, so System Settings, which shows each display with its
/// desktop, tells it apart from the user's monitors. False when its screen
/// isn't listed yet or there is no picture, as in a bare debug binary. Main
/// thread only.
pub fn label_display(display: DisplayId, mtm: MainThreadMarker) -> bool {
    let Some(picture) = wallpaper() else {
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
    // macOS remembers a display's picture by its serial, and serials come
    // back: setting it again costs up to seconds for nothing.
    let workspace = NSWorkspace::sharedWorkspace();
    if workspace
        .desktopImageURLForScreen(&screen)
        .is_some_and(|now| now.isEqual(Some(&url)))
    {
        log::debug!("display {display}: desktop picture already set");
        return true;
    }
    let options = objc2_foundation::NSDictionary::new();
    unsafe {
        workspace
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
