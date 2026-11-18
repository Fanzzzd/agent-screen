//! `AgentScreen mcp`: the pipe between one MCP client and the app (ADR 0021).
//!
//! It tells the app once per connection what only it can see, the client's
//! environment and its own pid, then forwards JSON-RPC lines both ways. The
//! tools and session detection live in the app, so an app update reaches
//! every session at once.
//!
//! ```text
//! → {"id": 0, "method": "pipe", "params": {"version": 2, "pid": …, "env": {…}, …}}
//! ← {"id": 0, "result": {"version": 2}}
//! ⇄ MCP JSON-RPC lines, unchanged
//! ```

use crate::diagnostics::BuildIdentity;
use crate::endpoint::{Reader, Writer};
use crate::session::ClientEnv;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// The control protocol version of a pipe's connection. Version 1 is the
/// app-level NDJSON protocol a v1 MCP process speaks (ADR 0005).
pub const PIPE_PROTOCOL_VERSION: u32 = 2;

/// The method of a pipe's first line.
pub const HELLO_METHOD: &str = "pipe";

/// The id the pipe gives the client's `initialize` when it replays it to a
/// restarted app; the app's reply to it never reaches the client.
const REPLAY_ID: &str = "agent-screen-pipe:replay";

/// How long the pipe waits for the app's answer to its hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// What the pipe tells the app on each connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub version: u32,
    /// The pipe's pid, working directory and session variables.
    #[serde(flatten)]
    pub client: ClientEnv,
    /// The process that started the pipe, normally the client.
    #[serde(default)]
    pub parent_pid: u32,
    #[serde(default)]
    pub build: Option<BuildIdentity>,
    /// The pipe connected before and replays the client's handshake.
    #[serde(default)]
    pub resumed: bool,
}

impl Hello {
    /// This process's.
    pub fn current(package_version: &str) -> Self {
        Self {
            version: PIPE_PROTOCOL_VERSION,
            client: ClientEnv {
                connection: Some(crate::session::new_connection_id()),
                ..ClientEnv::current()
            },
            parent_pid: parent_pid(),
            build: Some(crate::diagnostics::build_identity(package_version)),
            resumed: false,
        }
    }

    fn line(&self) -> Vec<u8> {
        let mut line =
            serde_json::to_vec(&json!({ "id": 0, "method": HELLO_METHOD, "params": self }))
                .expect("hello");
        line.push(b'\n');
        line
    }
}

/// What a connection's first line says, on the app's side.
#[derive(Debug, PartialEq)]
pub enum FirstLine {
    /// A pipe's hello the app can serve.
    Pipe(Hello),
    /// A pipe's hello of a version the app doesn't speak, with the reply
    /// that says so.
    Refused(Vec<u8>),
    /// Anything else: a v1 connection, whose first request this is.
    V1,
}

pub fn first_line(line: &str) -> FirstLine {
    let Ok(Value::Object(request)) = serde_json::from_str::<Value>(line.trim_end()) else {
        return FirstLine::V1;
    };
    if request.get("method").and_then(Value::as_str) != Some(HELLO_METHOD) {
        return FirstLine::V1;
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    match serde_json::from_value::<Hello>(params) {
        Ok(hello) if hello.version == PIPE_PROTOCOL_VERSION => FirstLine::Pipe(hello),
        Ok(hello) => FirstLine::Refused(reply_line(json!({
            "id": id,
            "error": {
                "code": "unsupported",
                "message": format!("control protocol {} is not supported; this app speaks {PIPE_PROTOCOL_VERSION}", hello.version),
                "version": PIPE_PROTOCOL_VERSION,
            }
        }))),
        Err(error) => FirstLine::Refused(reply_line(json!({
            "id": id,
            "error": { "code": "invalid", "message": format!("bad hello: {error}") }
        }))),
    }
}

/// The app's answer to a hello it serves.
pub fn accept_line() -> Vec<u8> {
    reply_line(json!({ "id": 0, "result": { "version": PIPE_PROTOCOL_VERSION } }))
}

fn reply_line(value: Value) -> Vec<u8> {
    let mut line = serde_json::to_vec(&value).expect("reply");
    line.push(b'\n');
    line
}

/// Only the parts of a JSON-RPC line the pipe looks at.
#[derive(Deserialize)]
struct Head {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
}

fn head(line: &[u8]) -> Head {
    serde_json::from_slice(line).unwrap_or(Head {
        id: None,
        method: None,
    })
}

/// The client's requests the app hasn't answered, by id, with their
/// method; `None` once the app's connection closed.
type Inflight = Arc<Mutex<Option<HashMap<String, (Value, String)>>>>;

/// An answer to a request the app can't serve: an error result the model
/// can read for a tool call, a JSON-RPC error for anything else.
fn failure(id: &Value, method: &str, message: &str) -> Vec<u8> {
    let message = format!("unavailable: {message}");
    reply_line(if method == "tools/call" {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": message }], "isError": true }
        })
    } else {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
    })
}

const CLOSED: &str = "The Agent Screen app closed the connection. Call the tool again.";
const TOO_OLD: &str = "The running Agent Screen app is older than this MCP server and can't serve it. Ask the user to quit and reopen Agent Screen.";

struct App {
    writer: Writer,
    inflight: Inflight,
}

/// Forward lines between the client and the app until the client closes its
/// end. `connect` reaches the app, starting it when it isn't running; its
/// error is the message the client's request gets.
pub async fn run<CR, CW, F, Fut>(
    client_in: CR,
    client_out: CW,
    mut hello: Hello,
    connect: F,
) -> std::io::Result<()>
where
    CR: AsyncRead + Unpin,
    CW: AsyncWrite + Unpin + Send + 'static,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<(Reader, Writer), String>>,
{
    // Lines to the client come from here and from the app's reader, in one
    // task so they never interleave.
    let (to_client, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        let mut client_out = client_out;
        while let Some(line) = outgoing.recv().await {
            if client_out.write_all(&line).await.is_err() || client_out.flush().await.is_err() {
                break;
            }
        }
    });

    let mut client_in = BufReader::new(client_in);
    let mut initialize: Option<Value> = None;
    let mut initialized = false;
    let mut connected_before = false;
    let mut app: Option<App> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        match client_in.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if line.last() != Some(&b'\n') {
            line.push(b'\n');
        }
        let Head { id, method } = head(&line);
        let request = id.clone().zip(method.clone());
        match method.as_deref() {
            Some("initialize") => {
                initialize = serde_json::from_slice(&line).ok();
                initialized = false;
            }
            Some("notifications/initialized") => initialized = true,
            _ => {}
        }

        // Register the request before writing it, under the same lock the
        // app's reader takes when the connection closes, so a request is
        // either answered by the app or failed here, never lost.
        let mut registered = false;
        for _ in 0..2 {
            if app.is_none() {
                // A client that starts over with initialize needs no replay.
                let replaying = method.as_deref() != Some("initialize");
                hello.resumed = connected_before && replaying && initialize.is_some();
                match open(&connect, &hello, &to_client).await {
                    Ok(mut opened) => {
                        connected_before = true;
                        if hello.resumed {
                            replay(&mut opened.writer, initialize.as_ref(), initialized).await;
                        }
                        app = Some(opened);
                    }
                    Err(message) => {
                        if let Some((id, method)) = &request {
                            let _ = to_client.send(failure(id, method, &message));
                        }
                        break;
                    }
                }
            }
            let current = app.as_mut().expect("connected");
            {
                let mut inflight = current.inflight.lock().unwrap();
                if let Some(map) = inflight.as_mut() {
                    if let Some((id, method)) = &request {
                        map.insert(id.to_string(), (id.clone(), method.clone()));
                    }
                    registered = true;
                }
            }
            // Closed since the last line: connect again.
            if !registered {
                app = None;
                continue;
            }
            if current.writer.write_all(&line).await.is_err() {
                // The reader sees the close too and fails what's in flight.
                app = None;
            }
            break;
        }
    }

    // The client is done. Let the app finish what it was asked, briefly,
    // as a client that closes stdin after its last request expects.
    if let Some(inflight) = app.as_ref().map(|app| app.inflight.clone()) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let waiting = || {
            inflight
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|m| !m.is_empty())
        };
        while tokio::time::Instant::now() < deadline && waiting() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    // The app sees the connection close and starts the session's
    // Disconnect Grace.
    if let Some(mut app) = app {
        let _ = app.writer.shutdown().await;
    }
    drop(to_client);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    Ok(())
}

/// Connect, send the hello and read the app's answer; then forward the
/// app's lines to the client from a task of their own.
async fn open<F, Fut>(
    connect: &F,
    hello: &Hello,
    to_client: &mpsc::UnboundedSender<Vec<u8>>,
) -> Result<App, String>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<(Reader, Writer), String>>,
{
    let (reader, mut writer) = connect().await?;
    writer
        .write_all(&hello.line())
        .await
        .map_err(|_| CLOSED.to_string())?;
    let mut reader = BufReader::new(reader);
    let mut answer = String::new();
    match tokio::time::timeout(HELLO_TIMEOUT, reader.read_line(&mut answer)).await {
        Ok(Ok(n)) if n > 0 => {}
        Ok(_) => return Err(CLOSED.into()),
        Err(_) => return Err(TOO_OLD.into()),
    }
    let answer: Value = serde_json::from_str(answer.trim_end()).unwrap_or_default();
    if answer["result"]["version"].as_u64() != Some(PIPE_PROTOCOL_VERSION as u64) {
        return Err(TOO_OLD.into());
    }
    let inflight: Inflight = Arc::new(Mutex::new(Some(HashMap::new())));
    tokio::spawn(forward(reader, inflight.clone(), to_client.clone()));
    Ok(App { writer, inflight })
}

/// The client's handshake again, for a restarted app, under the pipe's own
/// id.
async fn replay(writer: &mut Writer, initialize: Option<&Value>, initialized: bool) {
    let Some(mut initialize) = initialize.cloned() else {
        return;
    };
    initialize["id"] = json!(REPLAY_ID);
    let mut lines = reply_line(initialize);
    if initialized {
        lines.extend(reply_line(
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        ));
    }
    let _ = writer.write_all(&lines).await;
}

/// The app's lines to the client, until the app closes the connection;
/// then the client's requests still out fail.
async fn forward(
    mut reader: BufReader<Reader>,
    inflight: Inflight,
    to_client: mpsc::UnboundedSender<Vec<u8>>,
) {
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Head { id, method } = head(&line);
        if method.is_none()
            && let Some(id) = id
        {
            if id == json!(REPLAY_ID) {
                continue;
            }
            if let Some(map) = inflight.lock().unwrap().as_mut() {
                map.remove(&id.to_string());
            }
        }
        if line.last() != Some(&b'\n') {
            line.push(b'\n');
        }
        if to_client.send(line.clone()).is_err() {
            break;
        }
    }
    let left = inflight.lock().unwrap().take().unwrap_or_default();
    let mut left: Vec<_> = left.into_values().collect();
    left.sort_by_key(|(id, _)| id.to_string());
    for (id, method) in left {
        let _ = to_client.send(failure(&id, &method, CLOSED));
    }
}

/// The pid of this process's parent.
pub fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// End this process when the parent it started under exits, even if stdin
/// stays open in another process (ADR 0021).
pub fn exit_with_parent() {
    let parent = parent_pid();
    // Already adopted by launchd or init: no client to wait for.
    if parent <= 1 {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::thread::Builder::new()
            .name("parent-watch".into())
            .spawn(move || {
                if watch_exit(parent as i32).is_err() {
                    poll_parent(parent);
                }
                std::process::exit(0);
            });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::thread::Builder::new()
            .name("parent-watch".into())
            .spawn(move || {
                poll_parent(parent);
                std::process::exit(0);
            });
    }
}

/// Block until `pid` exits, through kqueue.
#[cfg(target_os = "macos")]
fn watch_exit(pid: i32) -> std::io::Result<()> {
    // SAFETY: plain kqueue calls on a descriptor this function owns.
    unsafe {
        let queue = libc::kqueue();
        if queue < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let change = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if libc::kevent(queue, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
            let error = std::io::Error::last_os_error();
            libc::close(queue);
            // ESRCH: it exited before the watch began.
            return if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            };
        }
        let mut event: libc::kevent = std::mem::zeroed();
        loop {
            let n = libc::kevent(queue, std::ptr::null(), 0, &mut event, 1, std::ptr::null());
            if n > 0 {
                break;
            }
            if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break;
            }
        }
        libc::close(queue);
    }
    Ok(())
}

/// Wait until this process is no longer `parent`'s child.
#[cfg(unix)]
fn poll_parent(parent: u32) {
    while parent_pid() == parent {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    fn hello() -> Hello {
        Hello {
            version: PIPE_PROTOCOL_VERSION,
            client: ClientEnv {
                pid: 77,
                connection: Some("77-test".into()),
                cwd: "/w/proj".into(),
                env: [("CLAUDE_CODE_SESSION_ID".to_string(), "s1".to_string())]
                    .into_iter()
                    .collect(),
            },
            parent_pid: 76,
            build: None,
            resumed: false,
        }
    }

    /// The app's end of each connection the pipe opens.
    type Accepted = UnboundedReceiver<DuplexStream>;

    type Connecting =
        std::pin::Pin<Box<dyn Future<Output = Result<(Reader, Writer), String>> + Send>>;

    /// A connector whose connections the test answers as the app, failing
    /// while `down` holds a message.
    fn connector(down: Arc<Mutex<Option<String>>>) -> (impl Fn() -> Connecting, Accepted) {
        let (tx, rx): (UnboundedSender<DuplexStream>, _) = unbounded_channel();
        let connect = move || {
            let tx = tx.clone();
            let down = down.lock().unwrap().clone();
            Box::pin(async move {
                if let Some(message) = down {
                    return Err(message);
                }
                let (pipe_end, app_end) = duplex(1 << 16);
                tx.send(app_end).unwrap();
                let (r, w) = tokio::io::split(pipe_end);
                Ok((Box::new(r) as Reader, Box::new(w) as Writer))
            }) as Connecting
        };
        (connect, rx)
    }

    struct Client {
        to_pipe: tokio::io::WriteHalf<DuplexStream>,
        from_pipe: tokio::io::Lines<BufReader<tokio::io::ReadHalf<DuplexStream>>>,
    }

    impl Client {
        async fn send(&mut self, value: Value) {
            let mut line = serde_json::to_vec(&value).unwrap();
            line.push(b'\n');
            self.to_pipe.write_all(&line).await.unwrap();
        }
        async fn next(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.from_pipe.next_line())
                .await
                .expect("a line from the pipe")
                .unwrap()
                .expect("the pipe is open");
            serde_json::from_str(&line).unwrap()
        }
    }

    struct FakeApp {
        lines: tokio::io::Lines<BufReader<tokio::io::ReadHalf<DuplexStream>>>,
        out: tokio::io::WriteHalf<DuplexStream>,
    }

    impl FakeApp {
        async fn accept(accepted: &mut Accepted) -> FakeApp {
            let stream = tokio::time::timeout(Duration::from_secs(5), accepted.recv())
                .await
                .expect("a connection")
                .unwrap();
            let (r, w) = tokio::io::split(stream);
            FakeApp {
                lines: BufReader::new(r).lines(),
                out: w,
            }
        }
        async fn next(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("a line from the pipe")
                .unwrap()
                .expect("open");
            serde_json::from_str(&line).unwrap()
        }
        async fn send(&mut self, value: Value) {
            let mut line = serde_json::to_vec(&value).unwrap();
            line.push(b'\n');
            self.out.write_all(&line).await.unwrap();
        }
        /// Read the hello and accept it, as a version 2 app does.
        async fn hello(&mut self) -> Hello {
            let line = self.next().await;
            let FirstLine::Pipe(hello) = first_line(&line.to_string()) else {
                panic!("not a hello: {line}");
            };
            self.out.write_all(&accept_line()).await.unwrap();
            hello
        }
    }

    fn start(
        down: Arc<Mutex<Option<String>>>,
    ) -> (
        Client,
        Accepted,
        tokio::task::JoinHandle<std::io::Result<()>>,
    ) {
        let (client_end, pipe_end) = duplex(1 << 16);
        let (pipe_in, pipe_out) = tokio::io::split(pipe_end);
        let (connect, accepted) = connector(down);
        let task = tokio::spawn(async move { run(pipe_in, pipe_out, hello(), connect).await });
        let (r, w) = tokio::io::split(client_end);
        (
            Client {
                to_pipe: w,
                from_pipe: BufReader::new(r).lines(),
            },
            accepted,
            task,
        )
    }

    fn initialize() -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "claude-code", "version": "2" } } })
    }

    #[tokio::test]
    async fn the_hello_comes_first_then_lines_pass_both_ways_unchanged() {
        let (mut client, mut accepted, _task) = start(Arc::default());
        client.send(initialize()).await;
        let mut app = FakeApp::accept(&mut accepted).await;
        let hello = app.hello().await;
        assert_eq!(hello.client.env["CLAUDE_CODE_SESSION_ID"], "s1");
        assert_eq!((hello.client.pid, hello.parent_pid), (77, 76));
        assert!(!hello.resumed);
        assert_eq!(app.next().await, initialize());
        let reply = json!({ "jsonrpc": "2.0", "id": 1, "result": { "serverInfo": { "name": "agent-screen" } } });
        app.send(reply.clone()).await;
        assert_eq!(client.next().await, reply);
        // A notification from the app reaches the client too.
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" });
        app.send(note.clone()).await;
        assert_eq!(client.next().await, note);
    }

    #[tokio::test]
    async fn a_restarted_app_gets_the_handshake_again_and_requests_in_flight_fail() {
        let (mut client, mut accepted, _task) = start(Arc::default());
        client.send(initialize()).await;
        let mut app = FakeApp::accept(&mut accepted).await;
        app.hello().await;
        app.next().await;
        app.send(json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
            .await;
        client.next().await;
        client
            .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        client.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "computer" } })).await;
        app.next().await;
        app.next().await;
        // The app quits with the call out.
        drop(app);
        let failed = client.next().await;
        assert_eq!(failed["id"], 2);
        assert_eq!(failed["result"]["isError"], true);
        assert!(
            failed["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("closed the connection")
        );

        // The next call reaches the new app after the replayed handshake.
        client
            .send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }))
            .await;
        let mut app = FakeApp::accept(&mut accepted).await;
        assert!(app.hello().await.resumed);
        let replayed = app.next().await;
        assert_eq!(replayed["method"], "initialize");
        assert_eq!(replayed["id"], REPLAY_ID);
        assert_eq!(replayed["params"], initialize()["params"]);
        assert_eq!(app.next().await["method"], "notifications/initialized");
        assert_eq!(app.next().await["id"], 3);
        // The reply to the replay stays in the pipe.
        app.send(json!({ "jsonrpc": "2.0", "id": REPLAY_ID, "result": {} }))
            .await;
        app.send(json!({ "jsonrpc": "2.0", "id": 3, "result": { "tools": [] } }))
            .await;
        assert_eq!(client.next().await["id"], 3);
    }

    #[tokio::test]
    async fn an_app_that_wont_start_or_predates_the_pipe_fails_the_request() {
        let down = Arc::new(Mutex::new(Some("could not start".to_string())));
        let (mut client, mut accepted, _task) = start(down.clone());
        client.send(initialize()).await;
        let failed = client.next().await;
        assert_eq!(failed["id"], 1);
        assert!(
            failed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("could not start")
        );

        // The app is up now, but an old one: it answers the hello as an
        // unknown method.
        *down.lock().unwrap() = None;
        client.send(initialize()).await;
        let mut app = FakeApp::accept(&mut accepted).await;
        let hello = app.next().await;
        assert_eq!(hello["method"], HELLO_METHOD);
        app.send(
            json!({ "id": 0, "error": { "code": "invalid", "message": "unknown method pipe" } }),
        )
        .await;
        let failed = client.next().await;
        assert!(
            failed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("older than this MCP server")
        );
    }

    #[tokio::test]
    async fn the_pipe_ends_when_the_client_closes_stdin() {
        let (client, mut accepted, task) = start(Arc::default());
        let Client {
            mut to_pipe,
            mut from_pipe,
        } = client;
        to_pipe
            .write_all(format!("{}\n", initialize()).as_bytes())
            .await
            .unwrap();
        let mut app = FakeApp::accept(&mut accepted).await;
        app.hello().await;
        app.next().await;
        // The client closes stdin with its request out: the answer still
        // comes, then the pipe ends.
        to_pipe.shutdown().await.unwrap();
        drop(to_pipe);
        app.send(json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
            .await;
        let line = from_pipe.next_line().await.unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], 1);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("the pipe ends")
            .unwrap()
            .unwrap();
        // And the app sees its connection close.
        assert!(app.lines.next_line().await.unwrap().is_none());
    }

    #[test]
    fn the_app_tells_a_hello_from_a_v1_request_and_an_unknown_version() {
        let line = String::from_utf8(hello().line()).unwrap();
        assert_eq!(first_line(&line), FirstLine::Pipe(hello()));
        assert_eq!(
            first_line(r#"{"id":1,"method":"createDisplay","params":{}}"#),
            FirstLine::V1
        );
        assert_eq!(first_line("not json"), FirstLine::V1);
        let mut future = hello();
        future.version = 3;
        let FirstLine::Refused(reply) = first_line(&String::from_utf8(future.line()).unwrap())
        else {
            panic!("version 3 served");
        };
        let reply: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(reply["error"]["code"], "unsupported");
        assert_eq!(reply["error"]["version"], PIPE_PROTOCOL_VERSION);
    }
}
