//! The real pipe binary driven as different MCP clients, against a socket
//! that plays the app and serves each pipe with an MCP host, as the app
//! does. Each client lists the tools and calls one; the sessions the app
//! hears must follow the rules in `session.rs`, and each protocol version
//! must get its own wire shape. No app, display or input.
#![cfg(unix)]

mod common;

use agent_screen_mcp::host::{self, Client};
use agent_screen_mcp::pipe::{self, FirstLine};
use common::{Fixture, fake_app};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const INSTANCE: &str = "clients";

/// One pipe, spoken to as a client would.
struct Pipe {
    child: Child,
    input: ChildStdin,
    lines: Receiver<String>,
    next: u64,
    /// The request `_meta` a 2026-07-28 client sends on every request;
    /// `None` for a client that sent initialize.
    stateless: Option<Value>,
}

impl Pipe {
    fn start(fixture: &Fixture, env: &[(&str, &str)]) -> Pipe {
        let mut child = fixture
            .command(env!("CARGO_BIN_EXE_agent-screen-mcp"), INSTANCE)
            .envs(env.iter().copied())
            .current_dir(&fixture.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Pipe {
            child,
            input,
            lines,
            next: 1,
            stateless: None,
        }
    }

    /// A client on a protocol with initialize.
    fn initialize(
        fixture: &Fixture,
        env: &[(&str, &str)],
        name: &str,
        version: &str,
    ) -> (Pipe, Value) {
        let mut pipe = Pipe::start(fixture, env);
        let init = pipe.request(
            "initialize",
            json!({ "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": name, "version": "1" } }),
        );
        writeln!(
            pipe.input,
            "{}",
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .unwrap();
        (pipe, init)
    }

    /// A client on protocol 2026-07-28: no initialize, its version and
    /// identity in every request's `_meta`.
    fn stateless(fixture: &Fixture, env: &[(&str, &str)], name: &str) -> Pipe {
        let mut pipe = Pipe::start(fixture, env);
        pipe.stateless = Some(json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": { "name": name, "version": "1" },
        }));
        pipe
    }

    fn request(&mut self, method: &str, mut params: Value) -> Value {
        if let Some(meta) = &self.stateless {
            let own = params["_meta"].as_object().cloned().unwrap_or_default();
            let mut merged = meta.as_object().unwrap().clone();
            merged.extend(own);
            params["_meta"] = Value::Object(merged);
        }
        let id = self.next;
        self.next += 1;
        writeln!(
            self.input,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        )
        .unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("no reply to {method}"));
            let reply: Value = serde_json::from_str(&line).unwrap();
            if reply["id"] == id {
                return reply;
            }
        }
    }

    fn tool_names(&mut self) -> (Vec<String>, Value) {
        let listed = self.request("tools/list", json!({}));
        let names = listed["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tools: {listed}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        (names, listed)
    }

    fn create_display(&mut self, meta: Value) -> Value {
        let mut params = json!({ "name": "create_display", "arguments": {} });
        if !meta.is_null() {
            params["_meta"] = meta;
        }
        self.request("tools/call", params)
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The app: every pipe that connects gets an MCP host, and every
/// createDisplay the hosts send lands in `requests`.
fn start_app(fixture: &Fixture, runtime: &tokio::runtime::Runtime) -> Arc<Mutex<Vec<Value>>> {
    let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
    let listener = {
        let _guard = runtime.enter();
        fixture.listener(INSTANCE)
    };
    let app_requests = requests.clone();
    runtime.spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let app_requests = app_requests.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut reader = tokio::io::BufReader::new(reader);
                let mut first = String::new();
                reader.read_line(&mut first).await.unwrap();
                let FirstLine::Pipe(hello) = pipe::first_line(&first) else {
                    panic!("not a hello: {first}");
                };
                writer.write_all(&pipe::accept_line()).await.unwrap();
                let _ = host::serve(
                    reader,
                    writer,
                    Client::piped(hello),
                    fake_app(app_requests),
                    Default::default(),
                )
                .await;
            });
        }
    });
    requests
}

/// The session key of the latest createDisplay the app heard.
fn last_session(requests: &Mutex<Vec<Value>>) -> String {
    requests
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|r| r["method"] == "createDisplay")
        .map(|r| r["params"]["sessionKey"].as_str().unwrap().to_string())
        .expect("a createDisplay")
}

fn created(reply: &Value) -> bool {
    reply["result"]["structuredContent"]["display_id"] == 7
}

#[test]
fn each_client_gets_its_own_session_by_the_documented_rules() {
    let fixture = Fixture::new("clients-sessions");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let requests = start_app(&fixture, &runtime);
    let claude_env = [
        ("CLAUDE_CODE_SESSION_ID", "cc-1"),
        ("CLAUDE_PROJECT_DIR", "/w/proj"),
    ];

    // Claude Code: its variable names the session, on every call.
    let (mut claude, _) = Pipe::initialize(&fixture, &claude_env, "claude-code", "2025-06-18");
    let (names, _) = claude.tool_names();
    assert!(names.iter().any(|n| n == "create_display"), "{names:?}");
    assert!(created(&claude.create_display(Value::Null)));
    assert_eq!(last_session(&requests), "claude-code:cc-1");
    claude.create_display(Value::Null);
    assert_eq!(last_session(&requests), "claude-code:cc-1");

    // Codex: each call names its thread, over one pipe, even with Claude
    // Code's variable inherited.
    let (mut codex, _) = Pipe::initialize(&fixture, &claude_env, "codex-mcp-client", "2025-06-18");
    assert!(codex.tool_names().0.iter().any(|n| n == "create_display"));
    let turn = |id: &str| json!({ "x-codex-turn-metadata": { "session_id": id, "workspaces": ["/w/app"] } });
    assert!(created(&codex.create_display(turn("t1"))));
    assert_eq!(last_session(&requests), "codex:t1");
    codex.create_display(turn("t2"));
    assert_eq!(last_session(&requests), "codex:t2");

    // Another client started from Claude Code's shell inherits the variable
    // but keeps a session of its own.
    let (mut gemini, _) =
        Pipe::initialize(&fixture, &claude_env, "gemini-cli-mcp-client", "2025-06-18");
    assert!(created(&gemini.create_display(Value::Null)));
    let gemini_key = last_session(&requests);
    assert!(gemini_key.starts_with("pipe:"), "{gemini_key}");

    // Two bare clients on 2026-07-28 with no hint: one session each, the
    // same over every call.
    let mut first = Pipe::stateless(&fixture, &[], "bare-client");
    let (names, _) = first.tool_names();
    assert!(names.iter().any(|n| n == "create_display"), "{names:?}");
    assert!(created(&first.create_display(Value::Null)));
    let first_key = last_session(&requests);
    first.create_display(Value::Null);
    assert_eq!(last_session(&requests), first_key);
    let mut second = Pipe::stateless(&fixture, &[], "bare-client");
    assert!(created(&second.create_display(Value::Null)));
    let second_key = last_session(&requests);
    assert!(first_key.starts_with("pipe:") && second_key.starts_with("pipe:"));

    let keys = [
        "claude-code:cc-1",
        "codex:t1",
        "codex:t2",
        &gemini_key,
        &first_key,
        &second_key,
    ];
    let distinct: std::collections::HashSet<_> = keys.iter().collect();
    assert_eq!(distinct.len(), keys.len(), "{keys:?}");
}

#[test]
fn every_protocol_version_gets_its_own_wire_shape() {
    let fixture = Fixture::new("clients-versions");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _requests = start_app(&fixture, &runtime);

    // Clients with initialize: the server answers in the version asked for,
    // without the 2026-07-28 fields.
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let (mut client, init) = Pipe::initialize(&fixture, &[], "versions", version);
        assert_eq!(init["result"]["protocolVersion"], version, "{init}");
        assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("create_display")
        );
        let (names, listed) = client.tool_names();
        assert_eq!(names.len(), 18, "{version}");
        let list = &listed["result"];
        for field in ["resultType", "ttlMs", "cacheScope"] {
            assert!(
                list.get(field).is_none(),
                "{version} tools/list has {field}: {list}"
            );
        }
        let call = client.create_display(Value::Null);
        assert!(created(&call), "{version}: {call}");
        assert!(
            call["result"].get("resultType").is_none(),
            "{version}: {call}"
        );
    }

    // A client that asks for 2026-07-28 but still sends initialize gets the
    // newest version that has one.
    let (_client, init) = Pipe::initialize(&fixture, &[], "versions", "2026-07-28");
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25", "{init}");

    // A 2026-07-28 client: discover, then lists with a cache lifetime and
    // results marked complete.
    let mut client = Pipe::stateless(&fixture, &[], "versions");
    let discovered = client.request("server/discover", json!({}));
    assert!(discovered.get("error").is_none(), "{discovered}");
    let found = &discovered["result"];
    // 2026-07-28 names the server in the result's _meta.
    assert_eq!(
        found["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "agent-screen",
        "{found}"
    );
    assert!(
        found["instructions"]
            .as_str()
            .unwrap()
            .contains("create_display")
    );
    let (names, listed) = client.tool_names();
    assert_eq!(names.len(), 18);
    let list = &listed["result"];
    assert_eq!(list["resultType"], "complete", "{list}");
    assert_eq!(list["ttlMs"], 3_600_000, "{list}");
    assert_eq!(list["cacheScope"], "public", "{list}");
    let call = client.create_display(Value::Null);
    assert!(created(&call), "{call}");
    assert_eq!(call["result"]["resultType"], "complete", "{call}");

    // A 2026-07-28 request without its required _meta is refused, not
    // served as some other version.
    let mut bare = Pipe::start(&fixture, &[]);
    let refused = bare.request("tools/list", json!({}));
    assert!(refused.get("error").is_some(), "{refused}");
}
