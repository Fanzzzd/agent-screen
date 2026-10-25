//! A minimal MCP client over stdio: JSON-RPC lines to `AgentScreen mcp`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{debug_app, listening, locked, skip, sleep_ms};

/// A JSON-RPC reply.
#[derive(Debug, Clone, Deserialize)]
pub struct Reply {
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

/// One item of a tool result's content.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A tools/call result. A JSON-RPC error reads as an error result whose one
/// text item is the error's JSON.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<Content>,
    /// `Null` when the tool returned none.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub structured_content: Value,
    #[serde(default)]
    pub is_error: bool,
}

/// A window as launch_app and list_windows report it; the control socket's
/// listWindows too.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Window {
    pub window_id: u64,
    pub app: String,
    pub title: String,
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ToolResult {
    fn error(error: &Value) -> ToolResult {
        ToolResult {
            content: vec![Content {
                kind: "text".into(),
                text: Some(error.to_string()),
                ..Content::default()
            }],
            structured_content: Value::Null,
            is_error: true,
        }
    }

    /// The text of the first text item.
    pub fn text(&self) -> &str {
        self.content
            .iter()
            .find(|c| c.kind == "text")
            .and_then(|c| c.text.as_deref())
            .unwrap_or("")
    }

    /// The text of the first item, whatever its type.
    pub fn first_text(&self) -> &str {
        self.content
            .first()
            .and_then(|c| c.text.as_deref())
            .unwrap_or("")
    }

    /// The bytes of the first image item.
    pub fn image(&self) -> Option<Vec<u8>> {
        let image = self.content.iter().find(|c| c.kind == "image")?;
        Some(
            base64::engine::general_purpose::STANDARD
                .decode(image.data.as_deref().unwrap_or(""))
                .unwrap_or_default(),
        )
    }

    /// A field of structuredContent; `Null` when missing.
    pub fn get(&self, key: &str) -> &Value {
        &self.structured_content[key]
    }

    pub fn display_id(&self) -> Option<u64> {
        self.get("display_id").as_u64()
    }

    /// structuredContent's `window`.
    pub fn window(&self) -> Option<Window> {
        serde_json::from_value(self.get("window").clone()).ok()
    }

    /// structuredContent's `windows`.
    pub fn windows(&self) -> Option<Vec<Window>> {
        serde_json::from_value(self.get("windows").clone()).ok()
    }
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Reply>>>>;

/// The server is a child process; replies are matched to requests by id, so
/// several can be out at once.
pub struct McpClient {
    server: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    shots: AtomicUsize,
}

/// A tools/call written to the server whose reply hasn't been read yet.
pub struct Call<'a> {
    client: &'a McpClient,
    name: String,
    reply: Receiver<Reply>,
}

impl Call<'_> {
    fn reply(&self) -> Result<Reply> {
        self.reply
            .recv()
            .map_err(|_| anyhow!("{}: the server closed", self.name))
    }

    /// The result, quietly.
    pub fn wait(self) -> Result<ToolResult> {
        let reply = self.reply()?;
        if let Some(e) = reply.error {
            return Ok(ToolResult::error(&e));
        }
        serde_json::from_value(reply.result.unwrap_or_default())
            .with_context(|| format!("{}'s result", self.name))
    }

    /// The result, its text printed and its images saved to /tmp; a
    /// JSON-RPC error fails.
    pub fn wait_printed(self) -> Result<ToolResult> {
        let name = &self.name;
        let reply = self.reply()?;
        if let Some(e) = reply.error {
            bail!("{name}: {e}");
        }
        let result: ToolResult = serde_json::from_value(reply.result.unwrap_or_default())
            .with_context(|| format!("{name}'s result"))?;
        let error = if result.is_error { " (error)" } else { "" };
        for item in &result.content {
            if item.kind == "image" {
                let n = self.client.shots.fetch_add(1, Ordering::SeqCst) + 1;
                let ext = if item.mime_type.as_deref() == Some("image/png") {
                    "png"
                } else {
                    "jpg"
                };
                let file = format!("/tmp/agent-screen-smoke-{n}.{ext}");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(item.data.as_deref().unwrap_or(""))
                    .unwrap_or_default();
                std::fs::write(&file, bytes)?;
                println!("  {name}: image → {file}");
            } else {
                println!("  {name}{error}: {}", item.text.as_deref().unwrap_or(""));
            }
        }
        Ok(result)
    }
}

/// Wait up to 10 s for a freshly started app to listen on its socket.
fn wait_for_app() -> Result<()> {
    for _ in 0..50 {
        if listening() {
            return Ok(());
        }
        sleep_ms(200);
    }
    bail!("the Agent Screen app is not listening on its control socket")
}

impl McpClient {
    /// Spawn the server and run the 2025-06-18 handshake. Skips the script
    /// when the screen is locked: it blocks input and lists no windows, so
    /// every check would fail for that reason alone.
    pub fn start(env: &[(&str, &str)]) -> Result<McpClient> {
        if locked() {
            skip("the screen is locked");
        }
        wait_for_app()?;
        // AGENTSCREEN_MCP="<binary> [args]" runs another server build.
        let (command, args) = match std::env::var("AGENTSCREEN_MCP") {
            Ok(custom) => {
                let mut parts = custom.split(' ').map(String::from);
                let command = parts.next().unwrap_or_default();
                (command, parts.collect::<Vec<_>>())
            }
            Err(_) => (
                format!("{}/Contents/MacOS/AgentScreen", debug_app().display()),
                vec!["mcp".to_string()],
            ),
        };
        let mut server = Command::new(&command)
            .args(&args)
            .env("CLAUDE_PROJECT_DIR", "/tmp/smoke")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("starting {command}"))?;
        let stdout = server.stdout.take().unwrap();
        let pending: Pending = Arc::default();
        let replies = pending.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(reply) = serde_json::from_str::<Reply>(&line) else {
                    continue;
                };
                let Some(id) = reply.id else { continue };
                if let Some(to) = replies.lock().unwrap().remove(&id) {
                    let _ = to.send(reply);
                }
            }
            replies.lock().unwrap().clear();
        });
        let client = McpClient {
            stdin: Mutex::new(server.stdin.take()),
            server: Mutex::new(server),
            pending,
            next_id: AtomicU64::new(1),
            shots: AtomicUsize::new(0),
        };
        let init = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "smoke", "version": "0" },
            }),
        )?;
        let version = init
            .result
            .as_ref()
            .and_then(|r| r["protocolVersion"].as_str())
            .unwrap_or("none");
        println!("protocol: {version}");
        client.write(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok(client)
    }

    fn write(&self, message: &Value) -> Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| anyhow!("the server is closed"))?;
        writeln!(stdin, "{message}")?;
        Ok(())
    }

    /// Write a JSON-RPC request; its reply comes on the receiver.
    fn send(&self, method: &str, params: Value) -> Result<Receiver<Reply>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (to, from) = channel();
        self.pending.lock().unwrap().insert(id, to);
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        Ok(from)
    }

    /// A JSON-RPC request; the reply, error included.
    pub fn request(&self, method: &str, params: Value) -> Result<Reply> {
        self.send(method, params)?
            .recv()
            .map_err(|_| anyhow!("{method}: the server closed"))
    }

    /// Write a tools/call and go on; its result comes with [`Call::wait`].
    pub fn begin(&self, name: &str, args: Value) -> Result<Call<'_>> {
        if name == "create_display" {
            crate::spend_display()?;
        }
        Ok(Call {
            client: self,
            name: name.into(),
            reply: self.send("tools/call", json!({ "name": name, "arguments": args }))?,
        })
    }

    /// A tools/call, quiet.
    pub fn tool(&self, name: &str, args: Value) -> Result<ToolResult> {
        self.begin(name, args)?.wait()
    }

    /// Call a tool, print its text and save its images to /tmp. A JSON-RPC
    /// error fails.
    pub fn call(&self, name: &str, args: Value) -> Result<ToolResult> {
        self.begin(name, args)?.wait_printed()
    }

    pub fn close(&self) {
        self.stdin.lock().unwrap().take();
        let mut server = self.server.lock().unwrap();
        let _ = server.kill();
        let _ = server.wait();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}
