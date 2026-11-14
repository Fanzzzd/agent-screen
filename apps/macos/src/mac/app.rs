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
pub fn main_executable(bundle: &Path) -> Option<PathBuf> {
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

/// The pid of a process's parent.
pub fn parent_pid(pid: i32) -> Option<i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (read == size).then_some(info.pbi_ppid as i32)
}

/// A process's arguments, its own path first.
pub fn arguments(pid: i32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    let probe = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if probe != 0 || size < 4 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    // argc, the executable's path, padding, then argc strings.
    let argc = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?) as usize;
    let rest = &buffer[4..];
    let path_end = rest.iter().position(|&b| b == 0)?;
    let rest = &rest[path_end..];
    let start = rest.iter().position(|&b| b != 0)?;
    Some(
        rest[start..]
            .split(|&b| b == 0)
            .take(argc)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
}

fn bundle_from_executable_path(path: &str) -> Option<PathBuf> {
    let (bundle, _) = path.rsplit_once("/Contents/MacOS/")?;
    (bundle.ends_with(".app") || bundle.ends_with(".appex") || bundle.ends_with(".xpc"))
        .then(|| PathBuf::from(bundle))
}

/// Where the app or view service with this pid is installed. ExtensionKit
/// (`.appex`) and XPC (`.xpc`) processes are not registered as running GUI
/// apps in `NSRunningApplication`, so fall back to `proc_pidpath`.
pub fn bundle_of(pid: i32) -> Option<PathBuf> {
    if let Some(url) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.bundleURL())
        .and_then(|url| url.path())
    {
        return Some(PathBuf::from(url.to_string()));
    }
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    (n > 0)
        .then(|| String::from_utf8_lossy(&path[..n as usize]).into_owned())
        .and_then(|p| bundle_from_executable_path(&p))
}

/// Whether the app draws its controls with AppKit, so its pop-up buttons
/// are NSPopUpButtons: no Qt, Java, Flutter or Chromium runtime in its
/// bundle. None when its bundle can't be found. An app without any of
/// those markers counts as AppKit: every other Mac toolkit we know of
/// ships one of them.
pub fn is_cocoa(pid: i32) -> Option<bool> {
    let bundle = bundle_of(pid)?;
    Some(!foreign_toolkit(&bundle))
}

/// Whether a bundle carries a toolkit of its own, whose pop-ups may take
/// keys differently from AppKit's.
fn foreign_toolkit(bundle: &std::path::Path) -> bool {
    let contents = bundle.join("Contents");
    let names = |dir: &str| -> Vec<String> {
        std::fs::read_dir(contents.join(dir))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let frameworks = names("Frameworks");
    let plugins = names("PlugIns");
    let has = |list: &[String], f: &dyn Fn(&str) -> bool| list.iter().any(|n| f(n));
    // Qt
    has(&frameworks, &|n| n.starts_with("Qt") && n.ends_with(".framework"))
        || contents.join("PlugIns/platforms/libqcocoa.dylib").exists()
        // Java: a bundled runtime or the launcher's library
        || has(&plugins, &|n| n.ends_with(".jre") || n.ends_with(".jdk"))
        || contents.join("runtime").exists()
        || contents.join("Home/lib/libjli.dylib").exists()
        || has(&frameworks, &|n| n == "JavaVM.framework" || n.ends_with("libjli.dylib"))
        // Flutter
        || has(&frameworks, &|n| n == "FlutterMacOS.framework")
        // Chromium and Electron, whose own controls aren't AppKit's
        || super::devtools::is_chromium(bundle)
}

/// Whether the app is built on Electron, which ships its framework inside.
pub fn is_electron(pid: i32) -> bool {
    bundle_of(pid).is_some_and(|b| {
        b.join("Contents/Frameworks/Electron Framework.framework")
            .exists()
    })
}

/// Whether the app is built on VS Code, as Cursor and Windsurf are: its
/// workbench ships inside. VS Code runs a menu item Electron reports as a
/// shortcut as its key binding in whatever has focus (#39).
pub fn is_vs_code_based(pid: i32) -> bool {
    bundle_of(pid).is_some_and(|b| b.join("Contents/Resources/app/out/vs/workbench").exists())
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

/// A running app's bundle id, which Window Access goes by.
pub fn bundle_id(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.bundleIdentifier())
        .map(|id| id.to_string())
}

/// An app the user named, by name or bundle id, as Window Access keeps it:
/// a running app first, then one installed. None when no app has it.
pub fn identify(app: &str) -> Option<agent_screen_model::access::AppId> {
    use agent_screen_model::access::AppId;
    let wanted = app.trim();
    let running = NSWorkspace::sharedWorkspace().runningApplications();
    for candidate in running.iter() {
        let Some(id) = candidate.bundleIdentifier().map(|i| i.to_string()) else {
            continue;
        };
        let name = candidate.localizedName().map(|n| n.to_string());
        if id.eq_ignore_ascii_case(wanted)
            || name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
        {
            return Some(AppId {
                name: name.unwrap_or_else(|| id.clone()),
                bundle_id: id,
            });
        }
    }
    let bundle = resolve(wanted)?;
    let plist = std::fs::read(bundle.join("Contents/Info.plist")).ok()?;
    let bundle_id = info_value(&plist, "CFBundleIdentifier")?;
    let name = info_value(&plist, "CFBundleDisplayName")
        .or_else(|| info_value(&plist, "CFBundleName"))
        .or_else(|| bundle.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| bundle_id.clone());
    Some(AppId { bundle_id, name })
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

/// Whether `pid` is a launcher, which opens apps from keys typed in a panel
/// that doesn't bring it to the front.
pub fn is_launcher(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.bundleIdentifier())
        .is_some_and(|id| {
            matches!(
                id.to_string().as_str(),
                "com.apple.Spotlight" | "com.raycast.macos" | "com.runningwithcrayons.Alfred"
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
    let home = std::env::var("HOME").unwrap_or_default();
    agent_screen_model::launch::find_app(app, &home, &Mac)
}

/// The app lookups launch_app makes, answered by this Mac.
struct Mac;

impl agent_screen_model::launch::AppLookup for Mac {
    fn by_bundle_id(&self, id: &str) -> Option<PathBuf> {
        let url = NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(id))?;
        Some(PathBuf::from(url.path()?.to_string()))
    }

    /// A bundle by path, as a build under /tmp that LaunchServices skips.
    /// The folder's own entry gives the name's case; a link such as
    /// /Applications/Safari.app stays a link.
    fn bundle_at(&self, path: &Path) -> Option<PathBuf> {
        if !path.join("Contents/Info.plist").exists() {
            return None;
        }
        let (folder, name) = (path.parent()?, path.file_name()?.to_str()?);
        let on_disk = std::fs::read_dir(folder)
            .ok()
            .and_then(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name())
                    .find(|entry| entry.to_str().is_some_and(|e| e.eq_ignore_ascii_case(name)))
            })
            .map(|entry| folder.join(entry));
        Some(on_disk.unwrap_or_else(|| path.to_path_buf()))
    }

    fn by_name(&self, name: &str) -> Option<PathBuf> {
        // Deprecated, but the only LaunchServices lookup by an app's name.
        #[allow(deprecated)]
        let path =
            NSWorkspace::sharedWorkspace().fullPathForApplication(&NSString::from_str(name))?;
        Some(PathBuf::from(path.to_string()))
    }
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

/// The Dock's pid.
pub fn dock_pid() -> Option<i32> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        "com.apple.dock",
    ))
    .firstObject()
    .map(|app| app.processIdentifier())
}

/// The running apps named `name`, as the Dock and the app switcher name
/// them.
pub fn pids_named(name: &str) -> Vec<i32> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.localizedName().is_some_and(|n| n.to_string() == name))
        .map(|app| app.processIdentifier())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finder lives in /System/Library/CoreServices (#168).
    #[test]
    fn finder_resolves_by_name_and_bundle_id() {
        let finder = Some(PathBuf::from("/System/Library/CoreServices/Finder.app"));
        assert_eq!(resolve("Finder"), finder);
        assert_eq!(resolve("finder"), finder);
        assert_eq!(resolve("com.apple.finder"), finder);
        assert_eq!(
            resolve("Calculator"),
            Some(PathBuf::from("/System/Applications/Calculator.app"))
        );
        assert_eq!(resolve("No Such App Anywhere"), None);
    }

    #[test]
    fn bundle_path_extracts_app_appex_and_xpc_bundles() {
        assert_eq!(
            bundle_from_executable_path(
                "/System/Library/ExtensionKit/Extensions/TimeMachineSettings.appex/Contents/MacOS/TimeMachineSettings"
            ),
            Some(PathBuf::from(
                "/System/Library/ExtensionKit/Extensions/TimeMachineSettings.appex"
            ))
        );
        assert_eq!(bundle_from_executable_path("/usr/bin/true"), None);
    }
}
