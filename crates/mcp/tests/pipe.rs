//! The pipe binary against a socket that plays the app: the hello carries
//! the client's environment, an MCP host serves it as the app does, and the
//! pipe ends with its parent. No app, display or input.
#![cfg(unix)]

use agent_screen_mcp::AppLink;
use agent_screen_mcp::endpoint::{Reader, Writer};
use agent_screen_mcp::host::{self, Client};
use agent_screen_mcp::pipe::{self, FirstLine};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let path = PathBuf::from("/tmp").join(format!("as-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn listener(&self, instance: &str) -> tokio::net::UnixListener {
        #[cfg(target_os = "macos")]
        let base = self.0.join("Library/Application Support/Agent Screen");
        #[cfg(not(target_os = "macos"))]
        let base = self.0.join("agent-screen");
        std::fs::create_dir_all(&base).unwrap();
        tokio::net::UnixListener::bind(base.join(format!("control-{instance}.sock"))).unwrap()
    }

    fn command(&self, program: &str, instance: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env("HOME", &self.0)
            .env("XDG_RUNTIME_DIR", &self.0)
            .env("AGENTSCREEN_INSTANCE", instance)
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CLAUDE_PROJECT_DIR");
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app's v1 handler, in process: answers createDisplay and keeps every
/// request it gets.
fn fake_app(requests: Arc<Mutex<Vec<Value>>>) -> AppLink {
    AppLink::InProcess(Arc::new(move || {
        let (host_writes, app_reads) = tokio::io::duplex(1 << 16);
        let (mut app_writes, host_reads) = tokio::io::duplex(1 << 16);
        let requests = requests.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(app_reads).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "createDisplay" => {
                        json!({ "displayId": 7, "width": 1280, "height": 800, "scale": 1 })
                    }
                    _ => json!({}),
                };
                requests.lock().unwrap().push(request.clone());
                let reply = json!({ "id": request["id"], "result": result });
                app_writes
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        (
            Box::new(host_reads) as Reader,
            Box::new(host_writes) as Writer,
        )
    }))
}

#[tokio::test]
async fn the_app_identifies_the_session_from_the_pipes_environment() {
    let fixture = Fixture::new("pipe-session");
    let project = fixture.0.join("where-the-client-works");
    std::fs::create_dir(&project).unwrap();
    let listener = fixture.listener("pipe-session");
    let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
    let app_requests = requests.clone();
    let test_pid = std::process::id();
    let app = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut first = String::new();
        reader.read_line(&mut first).await.unwrap();
        let FirstLine::Pipe(hello) = pipe::first_line(&first) else {
            panic!("not a hello: {first}");
        };
        writer.write_all(&pipe::accept_line()).await.unwrap();
        let seen = hello.clone();
        host::serve(
            reader,
            writer,
            Client::piped(hello),
            fake_app(app_requests),
            Default::default(),
        )
        .await
        .unwrap();
        seen
    });

    let program = env!("CARGO_BIN_EXE_agent-screen-mcp");
    let mut command = fixture.command(program, "pipe-session");
    command
        .current_dir(&project)
        .env("CLAUDE_CODE_SESSION_ID", "from-the-client")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let (pid, listed, created) = tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        let mut input = child.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let call = |input: &mut std::process::ChildStdin, request: Value| -> Value {
            writeln!(input, "{request}").unwrap();
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        };
        let mut call = call;
        let init = call(&mut input, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2"}}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "agent-screen");
        writeln!(input, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .unwrap();
        let listed = call(&mut input, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let created = call(&mut input, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"create_display","arguments":{}}}));
        drop(input);
        assert!(child.wait().unwrap().success());
        (pid, listed, created)
    })
    .await
    .unwrap();

    let hello = app.await.unwrap();
    assert_eq!(hello.version, pipe::PIPE_PROTOCOL_VERSION);
    assert_eq!(hello.client.pid, pid);
    assert_eq!(hello.parent_pid, test_pid);
    assert_eq!(
        hello.client.env["CLAUDE_CODE_SESSION_ID"],
        "from-the-client"
    );
    assert!(hello.client.cwd.ends_with("where-the-client-works"));
    assert!(hello.build.is_some());

    // The tool list and the tools come from the host, through the pipe.
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"create_display"), "{names:?}");
    assert_eq!(created["result"]["structuredContent"]["display_id"], 7);

    // The session is the client's, not the process the host runs in.
    let requests = requests.lock().unwrap();
    let create = requests
        .iter()
        .find(|r| r["method"] == "createDisplay")
        .expect("createDisplay reached the app");
    assert_eq!(
        create["params"]["sessionKey"],
        "claude-code:from-the-client"
    );
    assert_eq!(create["params"]["client"], "Claude Code");
    assert_eq!(create["params"]["title"], "where-the-client-works");
    assert!(requests.iter().any(|r| r["method"] == "agentReady"));
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn killing_the_parent_ends_the_pipe_while_stdin_stays_open() {
    let fixture = Fixture::new("pipe-parent");
    // The shell starts the pipe on its own stdin, which this test holds
    // open, prints its pid and waits: the pipe's parent is the shell.
    let mut shell = fixture
        .command("/bin/sh", "pipe-parent")
        .args([
            "-c",
            r#"exec 3<&0; "$0" <&3 & echo $!; wait"#,
            env!("CARGO_BIN_EXE_agent-screen-mcp"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let _stdin = shell.stdin.take().unwrap();
    let mut first = String::new();
    std::io::BufReader::new(shell.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let pipe: i32 = first.trim().parse().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(alive(pipe), "the pipe ended while its parent ran");

    shell.kill().unwrap();
    shell.wait().unwrap();
    let start = Instant::now();
    while alive(pipe) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let ended = !alive(pipe);
    if !ended {
        // SAFETY: the pipe this test started.
        unsafe { libc::kill(pipe, libc::SIGKILL) };
    }
    assert!(ended, "the pipe outlived its parent by 5 s");
    println!("the pipe ended {:?} after its parent", start.elapsed());
}
