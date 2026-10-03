//! Local control channel for the MCP server: newline-delimited JSON over a
//! Unix socket that only the current user can open (mode 0600).
//!
//!     → {"id": 1, "method": "createDisplay", "params": {...}}
//!     ← {"id": 1, "result": {...}}
//!     ← {"id": 1, "error": {"code": "not_found", "message": "..."}}
//!
//! Requests on one connection may complete out of order; match them by id.
//! The socket runs on its own thread; requests go to the main thread as
//! `Event`s and replies come back through a oneshot channel.

use futures::channel::{mpsc, oneshot};
use serde_json::{Map, Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

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

pub fn socket_path() -> PathBuf {
    let path = sidescreen_mcp::endpoint::socket_path();
    if let Some(base) = path.parent() {
        let _ = std::fs::create_dir_all(base);
        let _ = std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700));
    }
    path
}

/// Start listening. Events arrive on the returned receiver.
pub fn start() -> anyhow::Result<mpsc::UnboundedReceiver<Event>> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let std_listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std_listener.set_nonblocking(true)?;
    let (events, rx) = mpsc::unbounded();
    std::thread::Builder::new()
        .name("sidescreen-control".into())
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
                    tokio::spawn(serve(stream, connection, events.clone()));
                }
            });
        })?;
    Ok(rx)
}

async fn serve(
    stream: tokio::net::UnixStream,
    connection: u64,
    events: mpsc::UnboundedSender<Event>,
) {
    let (reader, mut writer) = stream.into_split();
    // Replies are written in the order they finish, from one task.
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
