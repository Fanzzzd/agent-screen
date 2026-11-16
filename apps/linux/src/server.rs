//! NDJSON Unix domain socket server (`mode 0600`) for `apps/linux`.

use crate::coordinator::Coordinator;
use serde_json::{Map, Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

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

impl From<crate::ports::PortError> for ControlError {
    fn from(e: crate::ports::PortError) -> Self {
        Self {
            code: e.code,
            message: e.message,
        }
    }
}

pub fn prepare_socket_path() -> anyhow::Result<PathBuf> {
    let path = agent_screen_mcp::endpoint::socket_path();
    if let Some(base) = path.parent() {
        std::fs::create_dir_all(base)?;
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700))?;
    }
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        anyhow::bail!(
            "another Agent Screen daemon is listening on {}",
            path.display()
        );
    }
    let _ = std::fs::remove_file(&path);
    Ok(path)
}

pub async fn serve_socket(
    path: PathBuf,
    coordinator: Arc<Coordinator>,
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
                    let coord = coordinator.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, conn_id, coord).await;
                    });
                }
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

async fn handle_connection(stream: UnixStream, connection: u64, coordinator: Arc<Coordinator>) {
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
        let coord = coordinator.clone();
        tokio::spawn(async move {
            let reply = match coord.dispatch(connection, &method, params).await {
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

    coordinator.connection_closed(connection).await;
    drop(lines_tx);
    let _ = write_task.await;
}
