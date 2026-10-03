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

    async fn open(&self) -> Result<Arc<Connection>, ControlError> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref().filter(|c| c.open.load(Ordering::Relaxed)) {
            return Ok(connection.clone());
        }
        let (reader, writer) = endpoint::connect().await.map_err(|error| {
            ControlError::new(
                "unavailable",
                format!("The Sidescreen app is not running ({error}). Ask the user to start it."),
            )
        })?;
        let pending: Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Reply>>>> =
            Default::default();
        let open = Arc::new(AtomicBool::new(true));
        tokio::spawn(receive(
            BufReader::new(reader),
            pending.clone(),
            open.clone(),
        ));
        let connection = Arc::new(Connection {
            writer: Mutex::new(writer),
            pending,
            open,
        });
        *slot = Some(connection.clone());
        Ok(connection)
    }
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
