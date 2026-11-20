//! Where the app's control endpoint lives, per platform: a Unix domain
//! socket on macOS and Linux, a named pipe on Windows. Only the user who
//! runs the app can open it.

use tokio::io::{AsyncRead, AsyncWrite};

pub type Reader = Box<dyn AsyncRead + Unpin + Send>;
pub type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// A second app beside the user's, for tests: `AGENTSCREEN_INSTANCE=e2e`
/// gives it its own socket and display serials. Letters, digits and dashes.
/// A development build is the `dev` instance unless told otherwise, so it
/// never answers for the user's installed Agent Screen.
pub fn instance() -> Option<String> {
    std::env::var("AGENTSCREEN_INSTANCE")
        .ok()
        .filter(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .or_else(|| is_dev_build().then(|| "dev".to_string()))
}

/// The bundle `scripts/bundle.sh` builds for development, beside the
/// release one: its own name and bundle id, so macOS keeps its privacy
/// grants and agents keep their registration apart.
pub const DEV_BUNDLE: &str = "Agent Screen Dev.app";

/// Whether this process runs from the development bundle.
pub fn is_dev_build() -> bool {
    std::env::current_exe().is_ok_and(|exe| is_dev_exe(&exe))
}

pub fn is_dev_exe(exe: &std::path::Path) -> bool {
    exe.components().any(|c| c.as_os_str() == DEV_BUNDLE)
}

/// The per-user directory where the app keeps its control socket and web
/// preview state file.
pub fn state_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Agent Screen")
    }
    // XDG_RUNTIME_DIR is per user and mode 0700 by the spec.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("agent-screen"))
            .unwrap_or_else(|| home.join(".local/state/agent-screen"))
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("Agent Screen"))
            .unwrap_or_else(|| home.join("AppData/Local/Agent Screen"))
    }
}

/// The socket, in a directory only the user can read.
#[cfg(unix)]
pub fn socket_path() -> std::path::PathBuf {
    let base = state_dir();
    match instance() {
        Some(instance) => base.join(format!("control-{instance}.sock")),
        None => base.join("control.sock"),
    }
}

/// Where the app records the port of its loopback web preview server, named
/// per instance like `socket_path`.
pub fn web_state_path() -> std::path::PathBuf {
    web_state_path_for(&state_dir(), instance().as_deref())
}

pub fn web_state_path_for(base: &std::path::Path, instance: Option<&str>) -> std::path::PathBuf {
    match instance {
        Some(instance) => base.join(format!("web-{instance}.json")),
        None => base.join("web.json"),
    }
}

/// The app's display ledger (`crate::ledger`), named per instance like
/// `socket_path`.
pub fn ledger_path() -> std::path::PathBuf {
    match instance() {
        Some(instance) => state_dir().join(format!("ledger-{instance}.json")),
        None => state_dir().join("ledger.json"),
    }
}

pub fn parse_web_port(contents: &str) -> Option<u16> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    let port = value.get("port")?.as_u64()?;
    u16::try_from(port).ok().filter(|&p| p > 0)
}

/// The pipe, named per user. The app creates it with a security
/// descriptor that admits only that user.
#[cfg(windows)]
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match instance() {
        Some(instance) => format!(r"\\.\pipe\agent-screen-control-{user}-{instance}"),
        None => format!(r"\\.\pipe\agent-screen-control-{user}"),
    }
}

pub async fn connect() -> std::io::Result<(Reader, Writer)> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(socket_path()).await?;
        let (reader, writer) = stream.into_split();
        Ok((Box::new(reader), Box::new(writer)))
    }
    #[cfg(windows)]
    {
        let pipe = tokio::net::windows::named_pipe::ClientOptions::new().open(pipe_name())?;
        let (reader, writer) = tokio::io::split(pipe);
        Ok((Box::new(reader), Box::new(writer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn web_state_path_names_instance_like_control_socket() {
        let base = Path::new("/tmp/agent-screen-state");
        assert_eq!(
            web_state_path_for(base, None),
            Path::new("/tmp/agent-screen-state/web.json")
        );
        assert_eq!(
            web_state_path_for(base, Some("e2e")),
            Path::new("/tmp/agent-screen-state/web-e2e.json")
        );
    }

    #[test]
    fn parse_web_port_accepts_valid_ports_and_rejects_bad_json() {
        assert_eq!(parse_web_port(r#"{"port":51234,"pid":99}"#), Some(51234));
        assert_eq!(parse_web_port(r#"{"port":0}"#), None);
        assert_eq!(parse_web_port(r#"{"port":70000}"#), None);
        assert_eq!(parse_web_port(r#"{}"#), None);
        assert_eq!(parse_web_port("not json"), None);
    }
}
