//! Client for the menu bar app's control socket: newline-delimited JSON.
//!
//! ```text
//! → {"id": 1, "method": "createDisplay", "params": {...}}
//! ← {"id": 1, "result": {...}}
//! ← {"id": 1, "error": {"code": "not_found", "message": "..."}}
//! ```
//!
//! The connection opens on the first call and stays open for the life of this
//! process. The app treats its closing as the end of the Agent Session, so the
//! session's display stops taking input when the MCP server exits.

use crate::endpoint::{self, Reader, Writer};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot};

pub type Object = Map<String, Value>;

/// A request the Sidescreen app refused or could not serve.
#[derive(Debug, Clone)]
pub struct ControlError {
    /// `invalid`, `not_found`, `ended`, `takeover`, `failed`, or
    /// `unavailable` when the app is not running.
    pub code: String,
    pub message: String,
}

impl ControlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid", message)
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

type Reply = Result<Object, ControlError>;

struct Connection {
    writer: Mutex<Writer>,
    pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    open: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Control {
    connection: Mutex<Option<Arc<Connection>>>,
    next_id: AtomicU64,
}

impl Control {
    pub async fn call(&self, method: &str, params: Value) -> Result<Object, ControlError> {
        let connection = self.open().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        connection.pending.lock().unwrap().insert(id, tx);
        let mut line = serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))
            .map_err(|e| ControlError::new("failed", e.to_string()))?;
        line.push(b'\n');
        if let Err(error) = connection.writer.lock().await.write_all(&line).await {
            connection.pending.lock().unwrap().remove(&id);
            return Err(closed(&error.to_string()));
        }
        rx.await
            .unwrap_or_else(|_| Err(closed("the connection dropped")))
    }

    /// Tell the app something if it is running, without starting it.
    pub async fn notify_if_running(&self, method: &str, params: Value) {
        if self.open_running().await.is_some() {
            let _ = self.call(method, params).await;
        }
    }

    async fn open_running(&self) -> Option<Arc<Connection>> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref().filter(|c| c.open.load(Ordering::Relaxed)) {
            return Some(connection.clone());
        }
        let (reader, writer) = endpoint::connect().await.ok()?;
        let connection = attach(reader, writer);
        *slot = Some(connection.clone());
        Some(connection)
    }

    async fn open(&self) -> Result<Arc<Connection>, ControlError> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref().filter(|c| c.open.load(Ordering::Relaxed)) {
            return Ok(connection.clone());
        }
        let (reader, writer) = match endpoint::connect().await {
            Ok(ends) => ends,
            // The app isn't running: start it, as the user would, and wait
            // for its socket.
            #[cfg(target_os = "macos")]
            Err(first) => start_app(first).await?,
            #[cfg(not(target_os = "macos"))]
            Err(error) => {
                return Err(ControlError::new(
                    "unavailable",
                    format!(
                        "The Sidescreen app is not running ({error}). Ask the user to start it."
                    ),
                ));
            }
        };
        let connection = attach(reader, writer);
        *slot = Some(connection.clone());
        Ok(connection)
    }
}

fn attach(reader: Reader, writer: Writer) -> Arc<Connection> {
    let pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Reply>>>> = Default::default();
    let open = Arc::new(AtomicBool::new(true));
    tokio::spawn(receive(
        BufReader::new(reader),
        pending.clone(),
        open.clone(),
    ));
    Arc::new(Connection {
        writer: Mutex::new(writer),
        pending,
        open,
    })
}

#[cfg(target_os = "macos")]
/// Start the Sidescreen app in the background and connect once it listens.
/// The server is the app's own binary, so it opens the bundle around
/// itself; a bare build opens the installed app by bundle id.
async fn start_app(first: std::io::Error) -> Result<(Reader, Writer), ControlError> {
    let unavailable = |detail: String| {
        ControlError::new(
            "unavailable",
            format!(
                "The Sidescreen app is not running and could not be started ({detail}). Ask the user to open Sidescreen."
            ),
        )
    };
    let bundle = std::env::current_exe().ok().and_then(|exe| {
        exe.ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(|p| p.to_path_buf())
    });
    let mut open = std::process::Command::new("/usr/bin/open");
    open.arg("-g");
    match &bundle {
        Some(app) => open.arg(app),
        None => open.args(["-b", "dev.sidescreen.menubar"]),
    };
    let status = open.status().map_err(|e| unavailable(e.to_string()))?;
    if !status.success() {
        return Err(unavailable(format!(
            "open exited with {status}; first error: {first}"
        )));
    }
    for _ in 0..75 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(ends) = endpoint::connect().await {
            return Ok(ends);
        }
    }
    Err(unavailable("it did not answer within 15 s".into()))
}

async fn receive(
    mut reader: BufReader<Reader>,
    pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    open: Arc<AtomicBool>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(reply) = serde_json::from_str::<Value>(line.trim_end()) else {
            continue;
        };
        let Some(id) = reply.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(settle) = pending.lock().unwrap().remove(&id) else {
            continue;
        };
        let outcome = match (reply.get("result"), reply.get("error")) {
            (_, Some(error)) => Err(ControlError::new(
                error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("failed"),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )),
            (Some(Value::Object(result)), None) => Ok(result.clone()),
            _ => Ok(Object::new()),
        };
        let _ = settle.send(outcome);
    }
    open.store(false, Ordering::Relaxed);
    for (_, settle) in pending.lock().unwrap().drain() {
        let _ = settle.send(Err(closed("The Sidescreen app closed the connection.")));
    }
}

fn closed(detail: &str) -> ControlError {
    ControlError::new("unavailable", detail)
}
