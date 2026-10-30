//! Real MCP transport against a mock socket. A queue approval is not an
//! executed drag; errors must not trigger a screenshot or fallback input.
#![cfg(unix)]

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from("/tmp").join(format!("as-queue-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn listener(&self) -> tokio::net::UnixListener {
        #[cfg(target_os = "macos")]
        let base = self.0.join("Library/Application Support/Agent Screen");
        #[cfg(not(target_os = "macos"))]
        let base = self.0.join("agent-screen");
        std::fs::create_dir_all(&base).unwrap();
        tokio::net::UnixListener::bind(base.join("control-queue.sock")).unwrap()
    }
    fn child(&self) -> Client {
        Client(
            Command::new(env!("CARGO_BIN_EXE_agent-screen-mcp"))
                .env("HOME", &self.0)
                .env("XDG_RUNTIME_DIR", &self.0)
                .env("AGENTSCREEN_INSTANCE", "queue")
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
async fn queue_status_and_drag_failure_never_imply_execution() {
    let fixture = Fixture::new();
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut calls = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let method = request["method"].as_str().unwrap();
            if method != "agentReady" {
                calls.push(request.clone());
            }
            let result = match method {
                "agentReady" => json!({}),
                "inputQueue" => {
                    let action = request["params"]["action"].as_str().unwrap();
                    let state = match action {
                        "request" => "queued",
                        "wait" => "approved_waiting_executor",
                        "withdraw" => "stopped",
                        _ => "none",
                    };
                    json!({"executorAvailable":false,"grantActive":action=="wait","requests":[{"requestId":42,"state":state}],"note":"Approval does not execute the action."})
                }
                "drag" if request["params"]["realInput"] == true => {
                    let reply = json!({"id":request["id"],"error":{"code":"input_pending","message":"requestId 42; no drag sent; executor unavailable"}});
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
                "drag" => json!({"route":"accessibility"}),
                "screenshot" => json!({"width":800,"height":600,"data":"","mimeType":"image/png"}),
                _ => {
                    // Keep the isolated listener alive even for a regression,
                    // so no failed mock request can launch the installed app.
                    let reply = json!({"id":request["id"],"error":{"code":"failed","message":"unexpected mock control call"}});
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
            };
            let reply = json!({"id":request["id"],"result":result});
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
        calls
    });
    let mut client = fixture.child();
    tokio::task::spawn_blocking(move || {
        let mut input = client.0.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(client.0.stdout.take().unwrap());
        let mut exchange = |request: Value| -> Value {
            writeln!(input,"{request}").unwrap();
            if request.get("id").is_none() { return Value::Null; }
            let mut line=String::new(); output.read_line(&mut line).unwrap(); serde_json::from_str(&line).unwrap()
        };
        exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"queue-test","version":"1"}}}));
        exchange(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let tools=exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
        assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"]=="input_queue"));
        let tool=|id, name, arguments| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}});
        for (index, action) in ["request","wait","withdraw","done"].into_iter().enumerate() {
            let result=exchange(tool(index+3,"input_queue",json!({"display_id":7,"action":action,"request_id":42,"operation":"drag-1","reason":"Move a card","timeout_seconds":0})));
            assert_ne!(result["result"]["isError"],true,"{result}");
            assert_eq!(result["result"]["structuredContent"]["executorAvailable"],false);
            if action=="wait" { assert_eq!(result["result"]["structuredContent"]["requests"][0]["state"],"approved_waiting_executor"); }
        }
        let invalid=exchange(tool(7,"input_queue",json!({"display_id":7,"action":"request"})));
        assert_eq!(invalid["result"]["isError"],true);
        let drag=|real| json!({"display_id":7,"action":"left_click_drag","start_coordinate":[10,20],"coordinate":[30,40],"real_input":real});
        let pending=exchange(tool(8,"computer",drag(true)));
        assert_eq!(pending["result"]["isError"],true,"{pending}");
        assert!(pending["result"]["content"][0]["text"].as_str().unwrap().contains("no drag sent"));
        let changed=exchange(tool(9,"computer",drag(false)));
        assert_ne!(changed["result"]["isError"],true,"{changed}");
        assert!(changed["result"]["content"][0]["text"].as_str().unwrap().contains("Input route: accessibility"));
        let invalid=exchange(tool(10,"computer",json!({"display_id":7,"action":"left_click","coordinate":[10,20],"real_input":true})));
        assert_eq!(invalid["result"]["isError"],true);
        drop(input); assert!(client.0.wait().unwrap().success());
    }).await.unwrap();
    let calls = server.await.unwrap();
    let methods: Vec<_> = calls
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        [
            "inputQueue",
            "inputQueue",
            "inputQueue",
            "inputQueue",
            "drag",
            "drag",
            "screenshot"
        ]
    );
    assert_eq!(calls[0]["params"]["operation"], "drag-1");
    assert_eq!(calls[1]["params"]["requestId"], 42);
    assert_eq!(calls[4]["params"]["fromX"], 10.0);
    assert_eq!(calls[4]["params"]["realInput"], true);
}
