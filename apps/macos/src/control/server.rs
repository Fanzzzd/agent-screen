//! Local control channel: newline-delimited JSON over a Unix socket that
//! only the current user can open (mode 0600).
//!
//! A connection whose first line is a pipe's hello carries MCP JSON-RPC
//! from then on, served by an MCP host on this thread (ADR 0020). Any
//! other connection is v1: app-level requests, from an MCP process older
//! than the pipe or from an MCP host here, in process.
//!
//!     → {"id": 1, "method": "createDisplay", "params": {...}}
//!     ← {"id": 1, "result": {...}}
//!     ← {"id": 1, "error": {"code": "not_found", "message": "..."}}
//!
//! Requests on one connection may complete out of order; match them by id.
//! The socket runs on its own thread; requests go to the main thread as
//! `Event`s and replies come back through a oneshot channel.

use agent_screen_mcp::AppLink;
use agent_screen_mcp::ledger::Ledger;
use agent_screen_mcp::pipe::FirstLine;
use futures::channel::{mpsc, oneshot};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
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
    let path = agent_screen_mcp::endpoint::socket_path();
    if let Some(base) = path.parent() {
        let _ = std::fs::create_dir_all(base);
        let _ = std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700));
    }
    path
}

/// What every connection's task shares: where requests go, the next
/// connection id, each socket peer's pid for the orphan sweep, and the
/// display ledger the MCP hosts share.
struct Shared {
    events: mpsc::UnboundedSender<Event>,
    next: AtomicU64,
    peers: Mutex<HashMap<u64, i32>>,
    ledger: Arc<Ledger>,
}

impl Shared {
    fn connection(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}

/// How often the app looks for orphaned MCP processes.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Start listening. Events arrive on the returned receiver.
pub fn start() -> anyhow::Result<mpsc::UnboundedReceiver<Event>> {
    let path = socket_path();
    // A live socket belongs to another running Agent Screen; replacing it
    // would cut that one off from every agent.
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        anyhow::bail!("another Agent Screen is listening on {}", path.display());
    }
    let _ = std::fs::remove_file(&path);
    let std_listener = std::os::unix::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std_listener.set_nonblocking(true)?;
    let (events, rx) = mpsc::unbounded();
    let shared = Arc::new(Shared {
        events,
        next: AtomicU64::new(1),
        peers: Mutex::default(),
        ledger: Arc::new(Ledger::persistent(agent_screen_mcp::endpoint::ledger_path())),
    });
    std::thread::Builder::new()
        .name("agent-screen-control".into())
        .spawn(move || {
            // The MCP hosts run their tools here too: two workers, so one
            // session's slow call never holds up another's.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("agent-screen-control")
                .enable_all()
                .build()
                .expect("control runtime");
            runtime.block_on(async move {
                let listener = UnixListener::from_std(std_listener).expect("control listener");
                let sweeper = shared.clone();
                tokio::spawn(async move {
                    let mut every = tokio::time::interval(SWEEP_EVERY);
                    loop {
                        every.tick().await;
                        sweep_orphans(&sweeper);
                    }
                });
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        continue;
                    };
                    let connection = shared.connection();
                    if let Some(pid) = stream.peer_cred().ok().and_then(|c| c.pid()) {
                        shared.peers.lock().unwrap().insert(connection, pid);
                        sweep_orphans(&shared);
                    }
                    tokio::spawn(accept(stream, connection, shared.clone()));
                }
            });
        })?;
    Ok(rx)
}

/// End the MCP processes on the socket whose client exited, and log each
/// (ADR 0020). A process whose parent still runs is never touched.
fn sweep_orphans(shared: &Shared) {
    let peers: Vec<(u64, i32)> = shared
        .peers
        .lock()
        .unwrap()
        .iter()
        .map(|(&c, &p)| (c, p))
        .collect();
    for (connection, pid) in peers {
        let parent = crate::mac::app::parent_pid(pid);
        let executable = crate::mac::app::process_path(pid);
        let app = std::process::id() as i32;
        if agent_screen_model::mcp_process::is_orphan(pid, app, parent, executable.as_deref()) {
            // SAFETY: a signal to one pid, which held this connection.
            let sent = unsafe { libc::kill(pid, libc::SIGTERM) } == 0;
            crate::lifecycle::line(&format!(
                "ended orphaned MCP process {pid} on connection {connection}: its client exited (signal sent: {sent})"
            ));
            shared.peers.lock().unwrap().remove(&connection);
        }
    }
}

/// One socket connection: its first line says whether a pipe or a v1
/// process holds it.
async fn accept(stream: tokio::net::UnixStream, connection: u64, shared: Arc<Shared>) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut first = String::new();
    if matches!(reader.read_line(&mut first).await, Ok(n) if n > 0) {
        match agent_screen_mcp::pipe::first_line(&first) {
            FirstLine::Pipe(hello) => {
                log::info!(
                    "control: pipe {} (parent {}) on connection {connection}{}",
                    hello.client.pid,
                    hello.parent_pid,
                    if hello.resumed { ", resumed" } else { "" }
                );
                if writer
                    .write_all(&agent_screen_mcp::pipe::accept_line())
                    .await
                    .is_ok()
                {
                    let link = in_process(shared.clone());
                    let client = agent_screen_mcp::host::Client::piped(hello);
                    let ledger = shared.ledger.clone();
                    if let Err(error) =
                        agent_screen_mcp::host::serve(reader, writer, client, link, ledger).await
                    {
                        log::info!("control: pipe on connection {connection} ended: {error}");
                    }
                }
            }
            FirstLine::Refused(reply) => {
                let _ = writer.write_all(&reply).await;
            }
            FirstLine::V1 => serve(reader, writer, Some(first), connection, &shared.events).await,
        }
    }
    shared.peers.lock().unwrap().remove(&connection);
}

/// The MCP host's way into the app: each call opens a v1 connection to this
/// handler over in-process streams, which closes when the host lets it go.
fn in_process(shared: Arc<Shared>) -> AppLink {
    AppLink::InProcess(Arc::new(move || {
        // One stream each way, so dropping the host's writer is an EOF here
        // and the app's closing is one there.
        let (host_writes, app_reads) = tokio::io::duplex(1 << 20);
        let (app_writes, host_reads) = tokio::io::duplex(1 << 20);
        let connection = shared.connection();
        let shared = shared.clone();
        tokio::spawn(async move {
            serve(
                BufReader::new(app_reads),
                app_writes,
                None,
                connection,
                &shared.events,
            )
            .await;
        });
        (
            Box::new(host_reads) as agent_screen_mcp::endpoint::Reader,
            Box::new(host_writes) as agent_screen_mcp::endpoint::Writer,
        )
    }))
}

/// A v1 connection: app-level NDJSON requests, from a v1 MCP process or an
/// MCP host in this app. `first` is its first line, already read.
async fn serve<R, W>(
    mut reader: BufReader<R>,
    mut writer: W,
    first: Option<String>,
    connection: u64,
    events: &mpsc::UnboundedSender<Event>,
) where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    // Replies are written in the order they finish, from one task.
    let (lines, mut outgoing) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let write_task = tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            if writer.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut pending = first;
    let mut line = String::new();
    loop {
        line.clear();
        if let Some(first) = pending.take() {
            line = first;
        } else {
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
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
        // Diagnostics never registers a session or touches the main/UI thread.
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
