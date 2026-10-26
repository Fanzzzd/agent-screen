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

/// A request the Agent Screen app refused or could not serve.
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
    /// Sent first on every connection, so an app that starts or restarts
    /// later still hears it.
    greeting: std::sync::Mutex<Option<(String, Value)>>,
    /// Connections opened so far. The app ends its displays when it quits,
    /// so one made on an earlier connection is gone.
    connections: AtomicU64,
    /// Displays this session made, by id: the connection each was made on,
    /// and the apps launch_app started there.
    made: std::sync::Mutex<HashMap<u64, Made>>,
    /// The last element tree each window sent, by display and window, for
    /// act_on_element's changes.
    trees: std::sync::Mutex<HashMap<(i64, i64), String>>,
}

#[derive(Default)]
struct Made {
    connection: u64,
    started: Vec<i64>,
    /// Its apps were handed to a later display.
    adopted: bool,
}

impl Control {
    /// Keep a window's latest tree; the one before it, if any.
    pub fn swap_tree(&self, display: i64, window: i64, tree: String) -> Option<String> {
        self.trees.lock().unwrap().insert((display, window), tree)
    }

    /// Note a display this session made on the current connection.
    pub fn made_display(&self, id: u64) {
        let connection = self.connections.load(Ordering::Relaxed);
        self.made.lock().unwrap().entry(id).or_default().connection = connection;
    }

    /// Note an app launch_app started for a display.
    pub fn started_app(&self, display: u64, pid: i64) {
        if let Some(made) = self.made.lock().unwrap().get_mut(&display) {
            made.started.push(pid);
        }
    }

    /// Whether a display this session made ended when the app quit.
    pub fn ended_with_app(&self, id: u64) -> bool {
        let connection = self.connections.load(Ordering::Relaxed);
        self.made
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|m| m.connection < connection)
    }

    /// The apps started for displays that ended when the app quit, taken
    /// once, for the next display to adopt.
    pub fn orphans(&self) -> Option<Vec<i64>> {
        let connection = self.connections.load(Ordering::Relaxed);
        let mut made = self.made.lock().unwrap();
        let mut any = false;
        let mut pids = Vec::new();
        for m in made.values_mut() {
            if m.connection < connection && !m.adopted {
                m.adopted = true;
                any = true;
                pids.append(&mut m.started);
            }
        }
        any.then_some(pids)
    }

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

    /// Tell the app something now if it is running, without starting it,
    /// and again on each later connection. While the app is down, look for
    /// it every 2 s, so one that starts or restarts later hears it too.
    pub async fn greet(self: &Arc<Self>, method: &str, params: Value) {
        if self.greeting.lock().unwrap().is_none() {
            let control = Arc::downgrade(self);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let Some(control) = control.upgrade() else {
                        return;
                    };
                    let open = control
                        .connection
                        .lock()
                        .await
                        .as_ref()
                        .is_some_and(|c| c.open.load(Ordering::Relaxed));
                    if !open {
                        let _ = control.open_running().await;
                    }
                }
            });
        }
        *self.greeting.lock().unwrap() = Some((method.into(), params));
        let slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref().filter(|c| c.open.load(Ordering::Relaxed)) {
            self.send_greeting(connection).await;
            return;
        }
        drop(slot);
        let _ = self.open_running().await;
    }

    /// Write the greeting without waiting for its reply, which `receive`
    /// drops for want of a pending entry.
    async fn send_greeting(&self, connection: &Connection) {
        let Some((method, params)) = self.greeting.lock().unwrap().clone() else {
            return;
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let Ok(mut line) =
            serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))
        else {
            return;
        };
        line.push(b'\n');
        let _ = connection.writer.lock().await.write_all(&line).await;
    }

    async fn open_running(&self) -> Option<Arc<Connection>> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref().filter(|c| c.open.load(Ordering::Relaxed)) {
            return Some(connection.clone());
        }
        let (reader, writer) = endpoint::connect().await.ok()?;
        self.connections.fetch_add(1, Ordering::Relaxed);
        let connection = attach(reader, writer);
        self.send_greeting(&connection).await;
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
                        "The Agent Screen app is not running ({error}). Ask the user to start it."
                    ),
                ));
            }
        };
        self.connections.fetch_add(1, Ordering::Relaxed);
        let connection = attach(reader, writer);
        self.send_greeting(&connection).await;
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
/// Start the Agent Screen app in the background and connect once it listens.
/// The server is the app's own binary, so it opens the bundle around
/// itself; a bare build opens the installed app by bundle id.
async fn start_app(first: std::io::Error) -> Result<(Reader, Writer), ControlError> {
    let unavailable = |detail: String| {
        ControlError::new(
            "unavailable",
            format!(
                "The Agent Screen app is not running and could not be started ({detail}). Ask the user to open Agent Screen."
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
        None => open.args(["-b", "app.agentscreen.AgentScreen"]),
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
        let _ = settle.send(Err(closed("The Agent Screen app closed the connection.")));
    }
}

fn closed(detail: &str) -> ControlError {
    ControlError::new("unavailable", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a restart does to the counter: the next call opens a new
    /// connection.
    fn reconnect(control: &Control) {
        control.connections.fetch_add(1, Ordering::Relaxed);
    }

    #[test]
    fn a_display_ends_with_the_connection_it_was_made_on() {
        let control = Control::default();
        reconnect(&control);
        control.made_display(7);
        assert!(!control.ended_with_app(7));
        reconnect(&control);
        assert!(control.ended_with_app(7));
        // Not this session's: the app's own answer stands.
        assert!(!control.ended_with_app(8));
    }

    #[test]
    fn the_next_display_adopts_the_apps_started_on_ended_ones_once() {
        let control = Control::default();
        reconnect(&control);
        assert_eq!(control.orphans(), None);
        control.made_display(7);
        control.started_app(7, 100);
        control.started_app(7, 101);
        // An app for a display this session didn't make isn't kept.
        control.started_app(9, 102);
        assert_eq!(control.orphans(), None);
        reconnect(&control);
        assert_eq!(control.orphans(), Some(vec![100, 101]));
        assert_eq!(control.orphans(), None);
        // A display with no apps still says the app quit.
        control.made_display(8);
        reconnect(&control);
        assert_eq!(control.orphans(), Some(vec![]));
    }
}
