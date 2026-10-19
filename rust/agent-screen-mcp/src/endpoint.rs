//! Where the app's control endpoint lives, per platform: a Unix domain
//! socket on macOS and Linux, a named pipe on Windows. Only the user who
//! runs the app can open it.

use tokio::io::{AsyncRead, AsyncWrite};

pub type Reader = Box<dyn AsyncRead + Unpin + Send>;
pub type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// A second app beside the user's, for tests: `SIDESCREEN_INSTANCE=e2e`
/// gives it its own socket and display serials. Letters, digits and dashes.
/// A development build is the `dev` instance unless told otherwise, so it
/// never answers for the user's installed Sidescreen.
pub fn instance() -> Option<String> {
    std::env::var("SIDESCREEN_INSTANCE")
        .ok()
        .filter(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .or_else(|| is_dev_build().then(|| "dev".to_string()))
}

/// The bundle `scripts/bundle.sh` builds for development, beside the
/// release one: its own name and bundle id, so macOS keeps its privacy
/// grants and agents keep their registration apart.
pub const DEV_BUNDLE: &str = "Sidescreen Dev.app";

/// Whether this process runs from the development bundle.
pub fn is_dev_build() -> bool {
    std::env::current_exe().is_ok_and(|exe| is_dev_exe(&exe))
}

pub fn is_dev_exe(exe: &std::path::Path) -> bool {
    exe.components().any(|c| c.as_os_str() == DEV_BUNDLE)
}

/// The socket, in a directory only the user can read.
#[cfg(unix)]
pub fn socket_path() -> std::path::PathBuf {
    use std::path::PathBuf;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    let base = home.join("Library/Application Support/Sidescreen");
    // XDG_RUNTIME_DIR is per user and mode 0700 by the spec.
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("sidescreen"))
        .unwrap_or_else(|| home.join(".local/state/sidescreen"));
    match instance() {
        Some(instance) => base.join(format!("control-{instance}.sock")),
        None => base.join("control.sock"),
    }
}

/// The pipe, named per user. The app creates it with a security
/// descriptor that admits only that user.
#[cfg(windows)]
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match instance() {
        Some(instance) => format!(r"\\.\pipe\sidescreen-control-{user}-{instance}"),
        None => format!(r"\\.\pipe\sidescreen-control-{user}"),
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
