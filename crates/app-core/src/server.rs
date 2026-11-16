//! Local control channel for the MCP server: newline-delimited JSON over a
//! Unix socket that only the current user can open (mode 0600).
//!
//! ```text
//! → {"id": 1, "method": "createDisplay", "params": {...}}
//! ← {"id": 1, "result": {...}}
//! ← {"id": 1, "error": {"code": "not_found", "message": "..."}}
//! ```
//!
//! Requests on one connection may complete out of order; match them by id.
//! Supports both channel-based main-thread dispatch (`start`, used by
//! `apps/macos`) and async handler dispatch (`serve_socket`, used by
//! `apps/linux`).

use crate::ports::PortError;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

/// A request the app refused or could not serve.
#[derive(Debug, Clone)]
pub struct ControlError {
    pub code: &'static str,
    pub message: String,
}

impl ControlError {
    pub fn invalid(m: impl Into<String>) -> Self {
        Self {
            code: "invalid",
            message: m.into(),
        }
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: m.into(),
        }
    }
    pub fn ended(m: impl Into<String>) -> Self {
        Self {
            code: "ended",
            message: m.into(),
        }
    }
    pub fn takeover(m: impl Into<String>) -> Self {
        Self {
            code: "takeover",
            message: m.into(),
        }
    }
    pub fn failed(m: impl Into<String>) -> Self {
        Self {
            code: "failed",
            message: m.into(),
        }
    }
}

impl From<PortError> for ControlError {
    fn from(e: PortError) -> Self {
        Self {
            code: e.code,
            message: e.message,
        }
    }
}

pub type Reply = Result<Map<String, Value>, ControlError>;

pub enum Event {
    Request {
        connection: u64,
        method: String,
        params: Map<String, Value>,
        reply: oneshot::Sender<Reply>,
    },
    Closed {
        connection: u64,
    },
}

/// Async handler trait for `serve_socket` (`apps/linux`).
pub trait RequestHandler: Send + Sync + 'static {
    fn dispatch<'a>(
        &'a self,
        connection: u64,
        method: &'a str,
        params: Map<String, Value>,
    ) -> BoxFuture<'a, Reply>;

    fn connection_closed(&self, connection: u64) -> BoxFuture<'_, ()>;
}

pub fn socket_path() -> PathBuf {
    let path = agent_screen_mcp::endpoint::socket_path();
    if let Some(base) = path.parent() {
        let _ = std::fs::create_dir_all(base);
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700));
        }
    }
    path
}

#[cfg(unix)]
pub fn prepare_socket_path() -> anyhow::Result<PathBuf> {
    let path = socket_path();
    // A live socket belongs to another running Agent Screen; replacing it
    // would cut that one off from every agent.
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        anyhow::bail!("another Agent Screen is listening on {}", path.display());
    }
    let _ = std::fs::remove_file(&path);
    Ok(path)
}

/// Start listening on a background thread and forward events through an
/// unbounded channel (`apps/macos`).
#[cfg(unix)]
pub fn start() -> anyhow::Result<mpsc::UnboundedReceiver<Event>> {
    let path = prepare_socket_path()?;
    let std_listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std_listener.set_nonblocking(true)?;
    let (events, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name("agent-screen-control".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("control runtime");
            runtime.block_on(async move {
                let listener = UnixListener::from_std(std_listener).expect("control listener");
                let next = AtomicU64::new(1);
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let connection = next.fetch_add(1, Ordering::Relaxed);
                    tokio::spawn(serve_channel(stream, connection, events.clone()));
                }
            });
        })?;
    Ok(rx)
}

#[cfg(unix)]
async fn serve_channel(stream: UnixStream, connection: u64, events: mpsc::UnboundedSender<Event>) {
    let (reader, mut writer) = stream.into_split();
    let (lines, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let write_task = tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            if writer.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(Value::Object(request)) = serde_json::from_str::<Value>(line.trim_end()) else {
            continue;
        };
        let Some(method) = request
            .get("method")
            .and_then(Value::as_str)
            .map(String::from)
        else {
            continue;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if let Some(reply) = agent_screen_mcp::diagnostics::control_reply(
            id.clone(),
            &method,
            env!("CARGO_PKG_VERSION"),
        ) {
            let mut bytes = serde_json::to_vec(&reply).unwrap_or_default();
            bytes.push(b'\n');
            let _ = lines.send(bytes);
            continue;
        }
        let params = match request.get("params") {
            Some(Value::Object(params)) => params.clone(),
            _ => Map::new(),
        };
        let (reply, answer) = oneshot::channel();
        if events
            .unbounded_send(Event::Request {
                connection,
                method,
                params,
                reply,
            })
            .is_err()
        {
            break;
        }
        let lines = lines.clone();
        tokio::spawn(async move {
            let message = match answer.await {
                Ok(Ok(result)) => json!({ "id": id, "result": result }),
                Ok(Err(error)) => {
                    json!({ "id": id, "error": { "code": error.code, "message": error.message } })
                }
                Err(_) => {
                    json!({ "id": id, "error": { "code": "failed", "message": "the app dropped the request" } })
                }
            };
            let mut bytes = serde_json::to_vec(&message).unwrap_or_default();
            bytes.push(b'\n');
            let _ = lines.send(bytes);
        });
    }
    let _ = events.unbounded_send(Event::Closed { connection });
    drop(lines);
    let _ = write_task.await;
}

/// Serve NDJSON requests on an existing Tokio runtime until `quit` fires (`apps/linux`).
#[cfg(unix)]
pub async fn serve_socket<H: RequestHandler>(
    path: PathBuf,
    handler: Arc<H>,
    quit: Arc<Notify>,
) -> anyhow::Result<()> {
    let std_listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std_listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(std_listener)?;
    let next_conn = AtomicU64::new(1);

    loop {
        let quit_fut = quit.notified();
        tokio::select! {
            _ = quit_fut => break,
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    let conn_id = next_conn.fetch_add(1, Ordering::Relaxed);
                    let h = handler.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, conn_id, h).await;
                    });
                }
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

#[cfg(unix)]
async fn handle_connection<H: RequestHandler>(
    stream: UnixStream,
    connection: u64,
    handler: Arc<H>,
) {
    let (reader, mut writer) = stream.into_split();
    let (lines_tx, mut lines_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let write_task = tokio::spawn(async move {
        while let Some(bytes) = lines_rx.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(Value::Object(req)) = serde_json::from_str::<Value>(line.trim_end()) else {
            continue;
        };
        let Some(method) = req.get("method").and_then(Value::as_str).map(String::from) else {
            continue;
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        if let Some(reply) = agent_screen_mcp::diagnostics::control_reply(
            id.clone(),
            &method,
            env!("CARGO_PKG_VERSION"),
        ) {
            let mut bytes = serde_json::to_vec(&reply).unwrap_or_default();
            bytes.push(b'\n');
            let _ = lines_tx.send(bytes);
            continue;
        }
        let params = match req.get("params") {
            Some(Value::Object(p)) => p.clone(),
            _ => Map::new(),
        };
        let tx = lines_tx.clone();
        let h = handler.clone();
        tokio::spawn(async move {
            let reply = match h.dispatch(connection, &method, params).await {
                Ok(result) => json!({ "id": id, "result": result }),
                Err(err) => {
                    json!({ "id": id, "error": { "code": err.code, "message": err.message } })
                }
            };
            let mut bytes = serde_json::to_vec(&reply).unwrap_or_default();
            bytes.push(b'\n');
            let _ = tx.send(bytes);
        });
    }

    handler.connection_closed(connection).await;
    drop(lines_tx);
    let _ = write_task.await;
}
