//! The real MCP binary forwards step actions to an isolated mock socket.
//! No app is launched, no display exists and no Accessibility call runs.
#![cfg(unix)]

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from("/tmp").join(format!("as-steps-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn listener(&self) -> tokio::net::UnixListener {
        #[cfg(target_os = "macos")]
        let base = self.0.join("Library/Application Support/Agent Screen");
        #[cfg(not(target_os = "macos"))]
        let base = self.0.join("agent-screen");
        std::fs::create_dir_all(&base).unwrap();
        tokio::net::UnixListener::bind(base.join("control-steps.sock")).unwrap()
    }

    fn child(&self) -> Client {
        Client(
            Command::new(env!("CARGO_BIN_EXE_agent-screen-mcp"))
                .env("HOME", &self.0)
                .env("XDG_RUNTIME_DIR", &self.0)
                .env("AGENTSCREEN_INSTANCE", "steps")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Client(Child);

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn step_actions_are_advertised_and_forwarded_without_click_fallbacks() {
    let fixture = Fixture::new();
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut methods = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let method = request["method"].as_str().unwrap();
            let result = match method {
                "agentReady" | "settle" => json!({}),
                "elementAction" => {
                    assert_eq!(request["params"]["displayId"], 7);
                    assert_eq!(request["params"]["windowId"], 8);
                    assert_eq!(request["params"]["element"], 9);
                    let action = request["params"]["action"].as_str().unwrap();
                    assert!(matches!(action, "increment" | "decrement"));
                    if action == "decrement" {
                        methods.push(format!("elementAction:{action}"));
                        let reply = json!({"id": request["id"], "error": {"code": "failed", "message": "The control did not confirm decrement; inspect its value."}});
                        write
                            .write_all(format!("{reply}\n").as_bytes())
                            .await
                            .unwrap();
                        continue;
                    }
                    json!({})
                }
                "windowState" => json!({"tree": "[9] slider value=2", "elements": 1}),
                other => panic!("unexpected control method {other}"),
            };
            if method != "agentReady" {
                methods.push(if method == "elementAction" {
                    "elementAction:increment".into()
                } else {
                    method.into()
                });
            }
            let reply = json!({"id": request["id"], "result": result});
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
        methods
    });
    let mut client = fixture.child();
    tokio::task::spawn_blocking(move || {
        let mut input = client.0.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(client.0.stdout.take().unwrap());
        let mut exchange = |request: Value| -> Value {
            writeln!(input, "{request}").unwrap();
            if request.get("id").is_none() { return Value::Null; }
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        };
        exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"steps-test","version":"1"}}}));
        exchange(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let tools = exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
        let definition = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "act_on_element").unwrap();
        let actions = definition["inputSchema"]["properties"]["action"]["enum"].as_array().unwrap();
        assert!(actions.contains(&json!("increment")));
        assert!(actions.contains(&json!("decrement")));
        let call = |id, action| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"act_on_element","arguments":{"display_id":7,"window_id":8,"element":9,"action":action,"result":"none"}}});
        let increment = exchange(call(3, "increment"));
        assert_ne!(increment["result"]["isError"], true, "{increment}");
        assert_eq!(increment["result"]["content"][0]["text"], "Done.");
        let decrement = exchange(call(4, "decrement"));
        assert_eq!(decrement["result"]["isError"], true, "{decrement}");
        assert!(decrement["result"]["content"][0]["text"].as_str().unwrap().contains("inspect its value"));
        let invalid = exchange(call(5, "step_sideways"));
        assert_eq!(invalid["result"]["isError"], true, "{invalid}");
        drop(input);
        assert!(client.0.wait().unwrap().success());
    }).await.unwrap();
    let methods = server.await.unwrap();
    assert_eq!(
        methods,
        [
            "elementAction:increment",
            "settle",
            "windowState",
            "elementAction:decrement"
        ]
    );
}
