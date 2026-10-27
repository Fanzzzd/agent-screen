//! Agent Screen's way into the browser the user runs, signed in as them
//! (ADR 0012): its extension, and the native messaging host between the
//! extension and the app.
//!
//! The browser starts the host, this binary with the extension's origin as
//! its argument, and talks to it on stdin and stdout, each message a
//! 32-bit length and JSON. The host passes each message on to the app's
//! browser socket, and back, NUL-terminated as on a DevTools pipe. The app
//! knows the browser by the host's parent, as the kernel reports it, and
//! hands the connection to `devtools` as that browser's channel.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// The host's name, in the manifests the browsers read.
pub const HOST: &str = "com.agentscreen.browser";
/// The extension's id, fixed by the key in its manifest.
pub const EXTENSION_ID: &str = "onlnbaoaokkbahdlaobgmampgmekddop";

/// Whether this process was started by a browser as the native host: its
/// argument is the extension's origin.
pub fn is_host_invocation(argument: Option<&str>) -> bool {
    argument.is_some_and(|a| a.starts_with("chrome-extension://"))
}

/// The socket the host connects to, beside the control socket.
pub fn socket_path() -> PathBuf {
    let control = agent_screen_mcp::endpoint::socket_path();
    let name = control
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("control.sock")
        .replacen("control", "browser", 1);
    control.with_file_name(name)
}

// MARK: The host

/// Pass messages between the browser and the app until either side closes.
pub fn host() -> ! {
    let Ok(socket) = UnixStream::connect(socket_path()) else {
        // Agent Screen isn't running: the extension tries again later.
        std::process::exit(1);
    };
    let mut to_app = match socket.try_clone() {
        Ok(s) => s,
        Err(_) => std::process::exit(1),
    };
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut length = [0u8; 4];
        while stdin.read_exact(&mut length).is_ok() {
            let mut message = vec![0u8; u32::from_ne_bytes(length) as usize];
            if stdin.read_exact(&mut message).is_err() {
                break;
            }
            message.push(0);
            if to_app.write_all(&message).is_err() {
                break;
            }
        }
        std::process::exit(0);
    });
    let mut from_app = BufReader::new(socket);
    let mut stdout = std::io::stdout().lock();
    let mut message = Vec::new();
    loop {
        message.clear();
        match from_app.read_until(0, &mut message) {
            Ok(0) | Err(_) => std::process::exit(0),
            Ok(_) => {}
        }
        if message.last() == Some(&0) {
            message.pop();
        }
        let length = (message.len() as u32).to_ne_bytes();
        if stdout.write_all(&length).is_err()
            || stdout.write_all(&message).is_err()
            || stdout.flush().is_err()
        {
            std::process::exit(0);
        }
    }
}

// MARK: The app's end

/// The pid of the process at the other end of a Unix socket.
fn peer_pid(stream: &UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERPID: libc::c_int = 2;
    let mut pid: libc::pid_t = 0;
    let mut size = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let got = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut size,
        )
    };
    (got == 0 && pid > 0).then_some(pid)
}

/// Listen for hosts. A connection counts only from this same binary,
/// started by a Chromium browser; the browser's pid names the channel.
pub fn listen() -> std::io::Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    std::thread::Builder::new()
        .name("agent-screen-browser".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let Some(host) = peer_pid(&stream) else {
                    continue;
                };
                let ours = std::env::current_exe().ok();
                let same = super::app::process_path(host)
                    .zip(ours)
                    .is_some_and(|(a, b)| same_file(&a, &b));
                let browser = super::app::parent_pid(host)
                    .filter(|&b| super::devtools::engine(b) == Some(agent_screen_model::devtools::Engine::Chromium));
                match (same, browser) {
                    (true, Some(browser)) => {
                        log::info!("browser: the extension in pid {browser} connected");
                        super::devtools::adopt_extension(browser, stream);
                    }
                    _ => log::warn!(
                        "browser: refused a connection from pid {host}: not this app's host under a Chromium browser"
                    ),
                }
            }
        })?;
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

// MARK: Registering the host

/// Each Chromium browser's own folder under Application Support; the host
/// manifest goes in its `NativeMessagingHosts`.
const BROWSERS: &[&str] = &[
    "Google/Chrome",
    "Google/Chrome Beta",
    "Google/Chrome Dev",
    "Google/Chrome Canary",
    "Chromium",
    "Microsoft Edge",
    "BraveSoftware/Brave-Browser",
    "Vivaldi",
    "Arc/User Data",
    "Dia/User Data",
];

/// The host manifest, for this binary.
pub fn manifest(exe: &Path) -> serde_json::Value {
    serde_json::json!({
        "name": HOST,
        "description": "Agent Screen",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    })
}

/// Where the user loads the extension from: a copy outside the app, which
/// a browser's file dialog can reach and an update of the app refreshes.
pub fn extension_dir() -> PathBuf {
    agent_screen_mcp::endpoint::socket_path().with_file_name("Browser Extension")
}

/// Put the app's copy of the extension where the user loads it from.
pub fn copy_extension() -> std::io::Result<PathBuf> {
    let bundled = std::env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources/BrowserExtension"))
        .filter(|d| d.is_dir())
        .ok_or_else(|| std::io::Error::other("the app has no BrowserExtension folder"))?;
    let dir = extension_dir();
    std::fs::create_dir_all(&dir)?;
    for file in std::fs::read_dir(&bundled)?.flatten() {
        std::fs::copy(file.path(), dir.join(file.file_name()))?;
    }
    Ok(dir)
}

/// The names of the browsers whose extension is connected.
pub fn connected() -> Vec<String> {
    let mut names: Vec<String> = super::devtools::extension_pids()
        .into_iter()
        .filter_map(super::app::name)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Tell every browser the user has where the host is. Only the user's own
/// Agent Screen does: a test or development instance would point their
/// browsers at itself.
pub fn register_host() {
    if agent_screen_mcp::endpoint::instance().is_some() {
        return;
    }
    let (Some(home), Ok(exe)) = (std::env::var_os("HOME"), std::env::current_exe()) else {
        return;
    };
    // A copy the user loaded follows the app's version.
    if extension_dir().is_dir()
        && let Err(e) = copy_extension()
    {
        log::warn!("browser: couldn't refresh the extension: {e}");
    }
    let support = PathBuf::from(home).join("Library/Application Support");
    let text = serde_json::to_vec_pretty(&manifest(&exe)).unwrap_or_default();
    for browser in BROWSERS {
        let dir = support.join(browser);
        if !dir.is_dir() {
            continue;
        }
        let hosts = dir.join("NativeMessagingHosts");
        let file = hosts.join(format!("{HOST}.json"));
        if std::fs::read(&file).is_ok_and(|old| old == text) {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&hosts).and_then(|()| std::fs::write(&file, &text))
        {
            log::warn!("browser: couldn't register the host for {browser}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_is_started_with_the_extensions_origin() {
        assert!(is_host_invocation(Some(
            "chrome-extension://onlnbaoaokkbahdlaobgmampgmekddop/"
        )));
        assert!(!is_host_invocation(Some("mcp")));
        assert!(!is_host_invocation(None));
    }

    #[test]
    fn the_manifest_admits_only_agent_screens_extension() {
        let m = manifest(Path::new(
            "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen",
        ));
        assert_eq!(m["name"], HOST);
        assert_eq!(m["type"], "stdio");
        assert_eq!(
            m["allowed_origins"],
            serde_json::json!([format!("chrome-extension://{EXTENSION_ID}/")])
        );
    }

    #[test]
    fn the_extension_id_comes_from_its_key() {
        use base64::Engine;
        unsafe extern "C" {
            fn CC_SHA256(data: *const u8, len: u32, md: *mut u8) -> *mut u8;
        }
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../resources/browser-extension/manifest.json"
        ))
        .unwrap();
        let key = base64::engine::general_purpose::STANDARD
            .decode(manifest["key"].as_str().unwrap())
            .unwrap();
        let mut hash = [0u8; 32];
        unsafe { CC_SHA256(key.as_ptr(), key.len() as u32, hash.as_mut_ptr()) };
        let id: String = hash[..16]
            .iter()
            .flat_map(|b| [b >> 4, b & 15])
            .map(|n| (b'a' + n) as char)
            .collect();
        assert_eq!(id, EXTENSION_ID);
    }
}
