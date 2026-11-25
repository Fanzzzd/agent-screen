//! Local control channel: newline-delimited JSON over a Unix socket that
//! only the current user can open (mode 0600).
//!
//! A connection whose first line is a pipe's hello carries MCP JSON-RPC
//! from then on, served by an MCP host on this thread (ADR 0021). Any
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
use agent_screen_mcp::line::{Line, read_capped};
use agent_screen_mcp::pipe::FirstLine;
use futures::channel::{mpsc, oneshot};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// A request the app refused or could not serve.
#[derive(Debug, Clone)]
pub struct ControlError {
    pub code: &'static str,
    pub message: String,
    /// For `front_blocked`: the app whose front state refused the action,
    /// its pid and name, for the queue (`super::front_queue`).
    pub blocked: Option<(i32, String)>,
}

impl ControlError {
    pub fn invalid(m: impl Into<String>) -> Self {
        Self {
            code: "invalid",
            message: m.into(),
            blocked: None,
        }
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self {
            code: "not_found",
            message: m.into(),
            blocked: None,
        }
    }
    /// The window the call names is the display's and still open, but
    /// shows nowhere: minimized, of a hidden app, or on another Space.
    pub fn off_screen(m: impl Into<String>) -> Self {
        Self {
            code: "off_screen",
            message: m.into(),
            blocked: None,
        }
    }
    pub fn ended(m: impl Into<String>) -> Self {
        Self {
            code: "ended",
            message: m.into(),
            blocked: None,
        }
    }
    /// Refused only because the user's window of the target app has
    /// focus, before anything was sent: `front_queue` queues the action.
    /// Never sent to an agent with this code.
    pub fn front_blocked(m: impl Into<String>) -> Self {
        Self {
            code: "front_blocked",
            message: m.into(),
            blocked: None,
        }
    }
    pub fn takeover(m: impl Into<String>) -> Self {
        Self {
            code: "takeover",
            message: m.into(),
            blocked: None,
        }
    }
    pub fn failed(m: impl Into<String>) -> Self {
        Self {
            code: "failed",
            message: m.into(),
            blocked: None,
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

/// A socket peer that sent a first line: a pipe's hello, or anything else,
/// taken as a v1 request. A peer that sends nothing, as another launch of
/// the app checking the socket, is never one. Being a peer doesn't get a
/// process ended: the sweep's checks of its parent, command line and pid
/// version do the protecting.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Peer {
    pid: i32,
    /// Its pid version when it connected, from its audit token.
    version: Option<u32>,
}

/// What the orphan sweep asks macOS about a process, and how it ends one.
/// Tests give their own.
trait Processes: Send + Sync {
    fn parent(&self, pid: i32) -> Option<i32>;
    fn executable(&self, pid: i32) -> Option<PathBuf>;
    fn arguments(&self, pid: i32) -> Option<Vec<String>>;
    /// The pid version of the process that has `pid` now.
    fn version(&self, pid: i32) -> Option<u32>;
    fn terminate(&self, pid: i32) -> bool;
}

struct Mac;

impl Processes for Mac {
    fn parent(&self, pid: i32) -> Option<i32> {
        crate::mac::app::parent_pid(pid)
    }
    fn executable(&self, pid: i32) -> Option<PathBuf> {
        crate::mac::app::process_path(pid)
    }
    fn arguments(&self, pid: i32) -> Option<Vec<String>> {
        crate::mac::app::arguments(pid)
    }
    fn version(&self, pid: i32) -> Option<u32> {
        crate::mac::app::pid_version(pid)
    }
    fn terminate(&self, pid: i32) -> bool {
        // SAFETY: a signal to one pid, checked to be the peer's process.
        unsafe { libc::kill(pid, libc::SIGTERM) == 0 }
    }
}

/// What every connection's task shares: where requests go, the next
/// connection id, the MCP peers for the orphan sweep, and the display
/// ledger the MCP hosts share.
struct Shared {
    events: mpsc::UnboundedSender<Event>,
    next: AtomicU64,
    peers: Mutex<HashMap<u64, Peer>>,
    ledger: Arc<Ledger>,
    processes: Box<dyn Processes>,
    /// The app's pid, which the sweep never ends.
    app: i32,
    /// The only user whose processes may connect.
    uid: u32,
}

impl Shared {
    fn new(
        events: mpsc::UnboundedSender<Event>,
        ledger: Arc<Ledger>,
        processes: Box<dyn Processes>,
    ) -> Self {
        Self {
            events,
            next: AtomicU64::new(1),
            peers: Mutex::default(),
            ledger,
            processes,
            app: std::process::id() as i32,
            // SAFETY: getuid has no preconditions.
            uid: unsafe { libc::getuid() },
        }
    }

    fn connection(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}

/// The app's display ledger, which the MCP hosts read, with the live
/// displays it was last told of.
static LEDGER: std::sync::OnceLock<(Arc<Ledger>, Mutex<Vec<agent_screen_mcp::ledger::Live>>)> =
    std::sync::OnceLock::new();

/// Keep the ledger in step with the model: its live displays and the apps
/// still theirs (`Displays::live_apps`), so after a restart no display
/// adopts an app the user took back or one of a display that had ended
/// (ADR 0021). Writes only when they changed.
pub fn keep_ledger<H>(displays: &crate::model::Displays<H>) {
    let Some((ledger, seen)) = LEDGER.get() else {
        return;
    };
    let live: Vec<agent_screen_mcp::ledger::Live> = displays
        .live_apps()
        .into_iter()
        .map(|(id, session, apps)| agent_screen_mcp::ledger::Live {
            display: id as u64,
            session,
            apps: apps.into_iter().map(i64::from).collect(),
        })
        .collect();
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if *seen != live {
        ledger.sync(&live);
        *seen = live;
    }
}

/// How long a new connection may take to send its first line, and how long
/// that line may be: a pipe's hello or a v1 request.
const FIRST_LINE_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);
const FIRST_LINE_MAX: usize = 1 << 20;
/// The longest v1 request line.
const LINE_MAX: usize = 16 << 20;
/// The wait after a failed accept.
const ACCEPT_ERROR_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

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
    let ledger = Arc::new(Ledger::persistent(agent_screen_mcp::endpoint::ledger_path()));
    let _ = LEDGER.set((ledger.clone(), Mutex::default()));
    let shared = Arc::new(Shared::new(events, ledger, Box::new(Mac)));
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
                    let stream = match listener.accept().await {
                        Ok((stream, _)) => stream,
                        // Out of descriptors, accept fails at once each
                        // time: wait rather than spin a worker (#288).
                        Err(error) => {
                            log::warn!("control: accept failed: {error}");
                            tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                            continue;
                        }
                    };
                    let connection = shared.connection();
                    tokio::spawn(accept(stream, connection, shared.clone()));
                }
            });
        })?;
    Ok(rx)
}

/// End the MCP processes on the socket whose client exited, and log each
/// (ADR 0021). A process whose parent still runs is never touched, nor a
/// pid macOS gave to another process since it connected.
fn sweep_orphans(shared: &Shared) {
    let peers: Vec<(u64, Peer)> = shared
        .peers
        .lock()
        .unwrap()
        .iter()
        .map(|(&c, &p)| (c, p))
        .collect();
    let ps = &shared.processes;
    for (connection, peer) in peers {
        let pid = peer.pid;
        if pid == shared.app {
            continue;
        }
        // Only processes adopted by launchd (parent == 1) can be orphans.
        // Check parent before reading executable path and KERN_PROCARGS2 argv.
        let parent = ps.parent(pid);
        if parent != Some(1) {
            continue;
        }
        let executable = ps.executable(pid);
        let arguments = ps.arguments(pid);
        let now = agent_screen_model::mcp_process::Peer {
            pid,
            parent,
            executable: executable.as_deref(),
            arguments: arguments.as_deref(),
        };
        if !agent_screen_model::mcp_process::is_orphan(&now, shared.app) {
            continue;
        }
        // The process read above is the one that connected.
        if peer.version.is_none() || ps.version(pid) != peer.version {
            continue;
        }
        let sent = ps.terminate(pid);
        crate::lifecycle::line(&format!(
            "ended orphaned MCP process {pid} on connection {connection}: its client exited (signal sent: {sent})"
        ));
        shared.peers.lock().unwrap().remove(&connection);
    }
}

/// Note `peer` as an MCP process on `connection`, then sweep.
fn identified(shared: &Shared, connection: u64, peer: Option<Peer>) {
    if let Some(peer) = peer {
        shared.peers.lock().unwrap().insert(connection, peer);
        sweep_orphans(shared);
    }
}

/// Wraps a pipe's reader: when it reaches the end, `close` says so at once,
/// before the MCP host has finished with its calls.
struct UntilEnd<R> {
    inner: R,
    close: tokio::sync::watch::Sender<bool>,
}

impl<R: AsyncRead + Unpin> AsyncRead for UntilEnd<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(result) = &polled
            && (result.is_err() || (buf.filled().len() == before && buf.remaining() > 0))
        {
            let _ = self.close.send(true);
        }
        polled
    }
}

/// One socket connection: its first line says whether a pipe or a v1
/// process holds it.
async fn accept(stream: tokio::net::UnixStream, connection: u64, shared: Arc<Shared>) {
    // The socket's mode already keeps other users out; check the peer too.
    let credentials = stream.peer_cred().ok();
    if credentials.as_ref().map(|c| c.uid()) != Some(shared.uid) {
        log::warn!("control: refused connection {connection} from another user");
        return;
    }
    use std::os::fd::AsRawFd;
    let peer = credentials.and_then(|c| c.pid()).map(|pid| Peer {
        pid,
        version: crate::mac::app::peer_pid_version(stream.as_raw_fd()),
    });
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    // A peer that says nothing, or sends a line with no end, must not
    // hold its task and descriptor for good (#288).
    let mut bytes = Vec::new();
    let read = tokio::time::timeout(
        FIRST_LINE_WITHIN,
        read_capped(&mut reader, &mut bytes, FIRST_LINE_MAX),
    )
    .await;
    if !matches!(read, Ok(Ok(Line::Read))) {
        log::debug!("control: connection {connection} sent no first line: {read:?}");
    }
    let first = String::from_utf8_lossy(&bytes).into_owned();
    if matches!(read, Ok(Ok(Line::Read))) {
        match agent_screen_mcp::pipe::first_line(&first) {
            FirstLine::Pipe(hello) => {
                log::info!(
                    "control: pipe {} (parent {}) on connection {connection}{}",
                    hello.client.pid,
                    hello.parent_pid,
                    if hello.resumed { ", resumed" } else { "" }
                );
                identified(&shared, connection, peer);
                if writer
                    .write_all(&agent_screen_mcp::pipe::accept_line())
                    .await
                    .is_ok()
                {
                    // The host's own connections into the app close when
                    // the pipe does, so the session's Disconnect Grace
                    // starts then, whatever calls the host still holds.
                    let (close, closed) = tokio::sync::watch::channel(false);
                    let link = in_process(shared.clone(), closed);
                    let reader = UntilEnd {
                        inner: reader,
                        close: close.clone(),
                    };
                    let client = agent_screen_mcp::host::Client::piped(hello);
                    let ledger = shared.ledger.clone();
                    if let Err(error) =
                        agent_screen_mcp::host::serve(reader, writer, client, link, ledger).await
                    {
                        log::info!("control: pipe on connection {connection} ended: {error}");
                    }
                    let _ = close.send(true);
                }
            }
            FirstLine::Refused(reply) => {
                let _ = writer.write_all(&reply).await;
            }
            FirstLine::V1 => {
                identified(&shared, connection, peer);
                let ended = || {
                    shared.peers.lock().unwrap().remove(&connection);
                };
                serve(
                    reader,
                    writer,
                    Some(first),
                    connection,
                    &shared.events,
                    None,
                    ended,
                )
                .await;
            }
        }
    }
    shared.peers.lock().unwrap().remove(&connection);
}

/// The MCP host's way into the app: each call opens a v1 connection to this
/// handler over in-process streams, which closes when the host lets it go
/// or when `closed` turns true.
fn in_process(shared: Arc<Shared>, closed: tokio::sync::watch::Receiver<bool>) -> AppLink {
    AppLink::InProcess(Arc::new(move || {
        // One stream each way, so dropping the host's writer is an EOF here
        // and the app's closing is one there.
        let (host_writes, app_reads) = tokio::io::duplex(1 << 20);
        let (app_writes, host_reads) = tokio::io::duplex(1 << 20);
        let connection = shared.connection();
        let shared = shared.clone();
        let closed = closed.clone();
        tokio::spawn(async move {
            serve(
                BufReader::new(app_reads),
                app_writes,
                None,
                connection,
                &shared.events,
                Some(closed),
                || {},
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
/// MCP host in this app. `first` is its first line, already read. It ends
/// at the reader's end, or when `closed` turns true; then `ended` runs and
/// the app hears the connection closed, before replies still pending.
async fn serve<R, W>(
    mut reader: BufReader<R>,
    mut writer: W,
    first: Option<String>,
    connection: u64,
    events: &mpsc::UnboundedSender<Event>,
    mut closed: Option<tokio::sync::watch::Receiver<bool>>,
    ended: impl FnOnce(),
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
    let mut bytes = Vec::new();
    loop {
        line.clear();
        if let Some(first) = pending.take() {
            line = first;
        } else {
            let read = match closed.as_mut() {
                // The close first: a call still running after the pipe
                // ended may open a connection and send at once, and the
                // app must not serve it.
                Some(closed) => tokio::select! {
                    biased;
                    _ = closed.wait_for(|c| *c) => break,
                    read = read_capped(&mut reader, &mut bytes, LINE_MAX) => read,
                },
                None => read_capped(&mut reader, &mut bytes, LINE_MAX).await,
            };
            match read {
                Ok(Line::Read) => {}
                Ok(Line::TooLong) => {
                    log::warn!(
                        "control: connection {connection} sent a line over {LINE_MAX} bytes; closing it"
                    );
                    break;
                }
                Ok(Line::End) | Err(_) => break,
            }
            // A line that isn't UTF-8 is no request, as bad JSON isn't; it
            // no longer ends the connection.
            line = String::from_utf8_lossy(&bytes).into_owned();
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
        if let Some(mut reply) = agent_screen_mcp::diagnostics::control_reply(
            id.clone(),
            &method,
            env!("CARGO_PKG_VERSION"),
        ) {
            // Why the last displays ended, from memory the main thread
            // doesn't hold (#326).
            reply["result"]["endedDisplays"] = crate::lifecycle::recent_ends();
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
                    // front_blocked is internal: a click, type or key call
                    // queues it. Any other call that meets it just failed.
                    let code = match error.code {
                        "front_blocked" => "failed",
                        code => code,
                    };
                    json!({ "id": id, "error": { "code": code, "message": error.message } })
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
    ended();
    let _ = events.unbounded_send(Event::Closed { connection });
    drop(lines);
    if closed.is_some_and(|c| *c.borrow()) {
        // Its MCP host is gone: nobody reads the replies still pending.
        write_task.abort();
    }
    let _ = write_task.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_screen_mcp::ClientEnv;
    use agent_screen_mcp::pipe::{Hello, PIPE_PROTOCOL_VERSION};
    use agent_screen_mcp::process::Identity;
    use futures::StreamExt;
    use std::time::{Duration, Instant};
    use tokio::io::AsyncBufReadExt;
    use tokio::io::{AsyncWriteExt, Lines};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    /// macOS as a test describes it: every pid has `parent`, runs
    /// AgentScreen with `arguments`, and has pid version `version`.
    #[derive(Default)]
    struct Fake {
        parent: Mutex<Option<i32>>,
        arguments: Mutex<Vec<String>>,
        version: Mutex<Option<u32>>,
        ended: Mutex<Vec<i32>>,
    }

    struct FakeProcesses(Arc<Fake>);

    impl Processes for FakeProcesses {
        fn parent(&self, _: i32) -> Option<i32> {
            *self.0.parent.lock().unwrap()
        }
        fn executable(&self, _: i32) -> Option<PathBuf> {
            Some("/Applications/Agent Screen.app/Contents/MacOS/AgentScreen".into())
        }
        fn arguments(&self, _: i32) -> Option<Vec<String>> {
            Some(self.0.arguments.lock().unwrap().clone())
        }
        fn version(&self, _: i32) -> Option<u32> {
            *self.0.version.lock().unwrap()
        }
        fn terminate(&self, pid: i32) -> bool {
            self.0.ended.lock().unwrap().push(pid);
            true
        }
    }

    fn orphan(arguments: &[&str]) -> Arc<Fake> {
        let fake = Arc::new(Fake::default());
        *fake.parent.lock().unwrap() = Some(1);
        *fake.arguments.lock().unwrap() = arguments.iter().map(|s| s.to_string()).collect();
        fake
    }

    /// The app's control side, with `fake` as macOS; the test process is
    /// not the app, so the sweep may pick it.
    fn shared(fake: &Arc<Fake>, ledger: Ledger) -> (Arc<Shared>, mpsc::UnboundedReceiver<Event>) {
        let (events, rx) = mpsc::unbounded();
        let shared = Shared {
            app: 1,
            ..Shared::new(
                events,
                Arc::new(ledger),
                Box::new(FakeProcesses(fake.clone())),
            )
        };
        (Arc::new(shared), rx)
    }

    /// A client of the socket, served by `accept` as the app serves one.
    fn connect(shared: &Arc<Shared>) -> (Lines<BufReader<OwnedReadHalf>>, OwnedWriteHalf) {
        let (client, app_end) = UnixStream::pair().unwrap();
        tokio::spawn(accept(app_end, shared.connection(), shared.clone()));
        let (reader, writer) = client.into_split();
        (BufReader::new(reader).lines(), writer)
    }

    fn hello(connection: &str) -> Vec<u8> {
        Hello {
            version: PIPE_PROTOCOL_VERSION,
            client: ClientEnv {
                pid: 4321,
                connection: Some(connection.into()),
                cwd: "/w/proj".into(),
                env: Default::default(),
            },
            parent_pid: 4320,
            build: None,
            resumed: false,
        }
        .line()
    }

    async fn send(writer: &mut OwnedWriteHalf, value: Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn next(lines: &mut Lines<BufReader<OwnedReadHalf>>) -> Option<Value> {
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("a line from the app")
            .unwrap()?;
        Some(serde_json::from_str(&line).unwrap())
    }

    /// The next request the app hears, skipping connections closing.
    async fn request(
        events: &mut mpsc::UnboundedReceiver<Event>,
    ) -> (String, Map<String, Value>, oneshot::Sender<Reply>) {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.next())
                .await
                .expect("an event")
                .expect("open");
            if let Event::Request {
                method,
                params,
                reply,
                ..
            } = event
            {
                return (method, params, reply);
            }
        }
    }

    /// A pipe's hello and an MCP handshake, as a client on 2025-06-18.
    async fn handshake(
        connection: &str,
        shared: &Arc<Shared>,
    ) -> (Lines<BufReader<OwnedReadHalf>>, OwnedWriteHalf) {
        let (mut lines, mut writer) = connect(shared);
        writer.write_all(&hello(connection)).await.unwrap();
        assert_eq!(
            next(&mut lines).await.unwrap()["result"]["version"],
            PIPE_PROTOCOL_VERSION
        );
        send(&mut writer, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } })).await;
        assert_eq!(
            next(&mut lines).await.unwrap()["result"]["protocolVersion"],
            "2025-06-18"
        );
        send(
            &mut writer,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        (lines, writer)
    }

    fn create_display() -> Value {
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "create_display", "arguments": {} } })
    }

    fn made() -> Map<String, Value> {
        json!({ "displayId": 7, "width": 1280, "height": 800, "scale": 1 })
            .as_object()
            .unwrap()
            .clone()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_pipe_gets_a_host_whose_connections_close_with_it() {
        runtime().block_on(async {
            let fake = Arc::new(Fake::default());
            let (shared, mut events) = shared(&fake, Ledger::default());
            let (mut lines, mut writer) = handshake("conn-a", &shared).await;

            send(&mut writer, create_display()).await;
            let (method, params, reply) = request(&mut events).await;
            assert_eq!(method, "createDisplay");
            assert_eq!(params["sessionKey"], "pipe:conn-a");
            reply.send(Ok(made())).unwrap();
            let created = next(&mut lines).await.unwrap();
            assert_eq!(created["result"]["structuredContent"]["display_id"], 7, "{created}");

            // A call the app holds, as wait_for_control does, then the
            // client goes.
            send(&mut writer, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "wait_for_control", "arguments": { "display_id": 7 } } })).await;
            let (method, _, _held) = request(&mut events).await;
            assert_eq!(method, "waitForControl");
            drop(writer);
            let gone = Instant::now();
            let closed = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(Event::Closed { .. }) = events.next().await {
                        return;
                    }
                }
            })
            .await;
            assert!(closed.is_ok(), "the app never heard the session's connection close");
            assert!(gone.elapsed() < Duration::from_secs(1), "closed after {:?}", gone.elapsed());
        });
    }

    #[test]
    fn a_connection_the_host_opens_after_its_pipe_ended_serves_nothing() {
        runtime().block_on(async {
            let fake = Arc::new(Fake::default());
            let (shared, mut events) = shared(&fake, Ledger::default());
            let (close, closed) = tokio::sync::watch::channel(false);
            let AppLink::InProcess(open) = in_process(shared.clone(), closed) else {
                panic!("not in process");
            };
            let _ = close.send(true);
            // A call still running opens a connection and sends at once:
            // the line and the close are both there when it is read. The
            // host may also end the connection before the line goes, and
            // then the write finds the pipe broken; either way nothing may
            // be served.
            for _ in 0..20 {
                let (reader, mut writer) = open();
                match writer
                    .write_all(b"{\"id\":1,\"method\":\"listDisplays\"}\n")
                    .await
                {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Err(error) => panic!("the write failed: {error}"),
                }
                let mut lines = BufReader::new(reader).lines();
                let end = tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await;
                assert_eq!(end.expect("the connection ends").unwrap(), None);
                let event = tokio::time::timeout(Duration::from_secs(5), events.next())
                    .await
                    .expect("an event")
                    .expect("open");
                if let Event::Request { method, .. } = event {
                    panic!("served {method} after the pipe ended");
                }
            }
        });
    }

    #[test]
    fn a_session_adopts_only_apps_whose_pid_still_has_the_same_process() {
        let dir = std::env::temp_dir().join(format!("as-server-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ledger-test.json");
        let then =
            json!({ "start_us": 1, "executable": "/Applications/App.app/Contents/MacOS/App" });
        // The previous run: pid 100 still runs that app; macOS gave 666 to
        // another process since.
        std::fs::write(
            &file,
            json!([{ "display": 3, "session": "pipe:conn-r", "apps": [
                { "pid": 100, "process": then },
                { "pid": 666, "process": then },
            ] }])
            .to_string(),
        )
        .unwrap();
        fn identify(pid: i64) -> Option<Identity> {
            Some(Identity {
                start_us: if pid == 100 { 1 } else { 2 },
                executable: "/Applications/App.app/Contents/MacOS/App".into(),
            })
        }
        let ledger = Ledger::persistent_with(file, identify, None);
        runtime().block_on(async {
            let fake = Arc::new(Fake::default());
            let (shared, mut events) = shared(&fake, ledger);
            let (_lines, mut writer) = handshake("conn-r", &shared).await;
            send(&mut writer, create_display()).await;
            let (method, params, _reply) = request(&mut events).await;
            assert_eq!(method, "createDisplay");
            assert_eq!(
                params["adoptGroups"],
                json!([{ "display": 3, "pids": [100] }])
            );
            assert_eq!(params["adopt"], json!([100]));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn until(what: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if what() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        what()
    }

    #[test]
    fn the_sweep_ends_only_an_mcp_peer_launchd_adopted_and_still_the_same_process() {
        runtime().block_on(async {
            let me = std::process::id() as i32;
            let fake = orphan(&["AgentScreen", "mcp", "--v1"]);
            // The test process's own pid version, as its socket reports it.
            *fake.version.lock().unwrap() = crate::mac::app::pid_version(me);
            assert!(fake.version.lock().unwrap().is_some());
            let (shared, _events) = shared(&fake, Ledger::default());

            // Another launch of the app looks for a running one and says
            // nothing: never a peer, never swept.
            let (_probe_lines, _probe) = connect(&shared);
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(shared.peers.lock().unwrap().is_empty());
            sweep_orphans(&shared);
            assert!(fake.ended.lock().unwrap().is_empty());

            // A v1 MCP process says what it wants: a peer, and an orphan.
            let (_lines, mut writer) = connect(&shared);
            send(&mut writer, json!({ "id": 1, "method": "listDisplays" })).await;
            assert!(until(|| fake.ended.lock().unwrap().contains(&me)).await);
            // The sweep ends the process before it drops the peer.
            assert!(until(|| shared.peers.lock().unwrap().is_empty()).await);

            // The same peer, read again with another command line or with
            // its pid given to another process, stays.
            let peer = Peer {
                pid: 500,
                version: Some(9),
            };
            shared.peers.lock().unwrap().insert(90, peer);
            *fake.version.lock().unwrap() = Some(9);
            *fake.arguments.lock().unwrap() = vec!["AgentScreen".into()];
            sweep_orphans(&shared);
            *fake.arguments.lock().unwrap() = vec!["AgentScreen".into(), "mcp".into()];
            *fake.version.lock().unwrap() = Some(10);
            sweep_orphans(&shared);
            assert!(!fake.ended.lock().unwrap().contains(&500));
            *fake.version.lock().unwrap() = Some(9);
            sweep_orphans(&shared);
            assert!(fake.ended.lock().unwrap().contains(&500));
        });
    }

    #[test]
    fn a_v1_peer_leaves_at_its_end_with_a_reply_pending() {
        runtime().block_on(async {
            let fake = orphan(&["AgentScreen", "mcp", "--v1"]);
            // Its client runs.
            *fake.parent.lock().unwrap() = Some(4242);
            let (shared, mut events) = shared(&fake, Ledger::default());
            let (_lines, mut writer) = connect(&shared);
            send(
                &mut writer,
                json!({ "id": 1, "method": "waitForControl", "params": {} }),
            )
            .await;
            let (_, _, _held) = request(&mut events).await;
            assert_eq!(shared.peers.lock().unwrap().len(), 1);
            drop(writer);
            assert!(until(|| shared.peers.lock().unwrap().is_empty()).await);
            assert!(fake.ended.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn another_users_connection_is_refused() {
        runtime().block_on(async {
            let fake = Arc::new(Fake::default());
            let (events, _rx) = mpsc::unbounded();
            let shared = Arc::new(Shared {
                uid: unsafe { libc::getuid() } + 1,
                ..Shared::new(events, Arc::default(), Box::new(FakeProcesses(fake)))
            });
            let (mut lines, mut writer) = connect(&shared);
            let _ = writer.write_all(&hello("conn-u")).await;
            assert_eq!(next(&mut lines).await, None);
            assert!(shared.peers.lock().unwrap().is_empty());
        });
    }
}
