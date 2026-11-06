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
    fn new(name: &str) -> Self {
        let path = PathBuf::from("/tmp").join(format!("as-{name}-{}", std::process::id()));
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
    let fixture = Fixture::new("queue");
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
                        "wait" => "approved",
                        "withdraw" => "stopped",
                        _ => "none",
                    };
                    json!({"executorAvailable":true,"grantActive":action=="wait","requests":[{"requestId":42,"state":state}],"note":"Approval does not execute the action."})
                }
                "drag" if request["params"]["realInput"] == true => {
                    let reply = json!({"id":request["id"],"error":{"code":"input_pending","message":"requestId 42; no drag sent; waiting for user approval"}});
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
                "drag" => json!({"route":"accessibility"}),
                "click" if request["params"]["realInput"] == true => {
                    let reply = json!({"id":request["id"],"error":{"code":"input_pending","message":"The click at (10, 20) waits for the user to allow their pointer. No click was sent, not on the background route either."}});
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
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
            let mut arguments=json!({"display_id":7,"action":action,"request_id":42,"operation":"drag-1","reason":"Move a card","timeout_seconds":0});
            if action=="done" { arguments.as_object_mut().unwrap().remove("request_id"); }
            let result=exchange(tool(index+3,"input_queue",arguments));
            assert_ne!(result["result"]["isError"],true,"{result}");
            assert_eq!(result["result"]["structuredContent"]["executorAvailable"],true);
            if action=="wait" { assert_eq!(result["result"]["structuredContent"]["requests"][0]["state"],"approved"); }
        }
        let invalid=exchange(tool(7,"input_queue",json!({"display_id":7,"action":"request"})));
        assert_eq!(invalid["result"]["isError"],true);
        // done forgets the session's requests; it is refused with a request
        // id before reaching the app, so it cannot succeed and then fail.
        let done=exchange(tool(11,"input_queue",json!({"display_id":7,"action":"done","request_id":42})));
        assert_eq!(done["result"]["isError"],true,"done with request_id must be refused: {done}");
        let drag=|real| json!({"display_id":7,"action":"left_click_drag","start_coordinate":[10,20],"coordinate":[30,40],"real_input":real});
        let pending=exchange(tool(8,"computer",drag(true)));
        assert_eq!(pending["result"]["isError"],true,"{pending}");
        assert!(pending["result"]["content"][0]["text"].as_str().unwrap().contains("no drag sent"));
        let changed=exchange(tool(9,"computer",drag(false)));
        assert_ne!(changed["result"]["isError"],true,"{changed}");
        assert!(changed["result"]["content"][0]["text"].as_str().unwrap().contains("Input route: accessibility"));
        // A click asks for the user's pointer and, while it waits, sends
        // nothing: no background click, no screenshot.
        let click=exchange(tool(10,"computer",json!({"display_id":7,"action":"double_click","coordinate":[10,20],"real_input":true})));
        assert_eq!(click["result"]["isError"],true,"{click}");
        assert!(click["result"]["content"][0]["text"].as_str().unwrap().contains("No click was sent"));
        let invalid=exchange(tool(12,"computer",json!({"display_id":7,"action":"scroll","coordinate":[10,20],"real_input":true})));
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
            "screenshot",
            "click"
        ]
    );
    assert_eq!(calls[0]["params"]["operation"], "drag-1");
    assert_eq!(calls[1]["params"]["requestId"], 42);
    assert_eq!(calls[4]["params"]["fromX"], 10.0);
    assert_eq!(calls[4]["params"]["realInput"], true);
    assert_eq!(calls[7]["params"]["realInput"], true);
    assert_eq!(calls[7]["params"]["button"], "left");
    assert_eq!(calls[7]["params"]["count"], 2);
}

/// A menu press that waits for a real click and comes back input_pending
/// reports it and stops: no window state, no second press. real_input on
/// any other action is refused before it reaches the app.
#[tokio::test]
async fn a_pending_menu_press_is_reported_without_a_fallback() {
    let fixture = Fixture::new("menu");
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut calls = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let method = request["method"].as_str().unwrap().to_string();
            if method != "agentReady" {
                calls.push(request.clone());
            }
            let reply = if method == "elementAction" {
                json!({"id":request["id"],"error":{"code":"input_pending","message":"Go > Previous view needs a real click. No click was sent, and the item was not pressed through accessibility either."}})
            } else if method == "agentReady" {
                json!({"id":request["id"],"result":{}})
            } else {
                json!({"id":request["id"],"error":{"code":"failed","message":"unexpected mock control call"}})
            };
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
            writeln!(input, "{request}").unwrap();
            if request.get("id").is_none() {
                return Value::Null;
            }
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        };
        exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"menu-test","version":"1"}}}));
        exchange(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let tools = exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
        let act = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "act_on_element")
            .unwrap();
        assert_eq!(act["inputSchema"]["properties"]["real_input"]["type"], "boolean");
        let call = |id, action, real| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"act_on_element","arguments":{"display_id":7,"window_id":8,"element":9,"action":action,"real_input":real}}});
        let pending = exchange(call(3, "press", json!(true)));
        assert_eq!(pending["result"]["isError"], true, "{pending}");
        assert!(pending["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No click was sent"));
        let refused = exchange(call(4, "focus", json!(true)));
        assert_eq!(refused["result"]["isError"], true, "{refused}");
        let wrong = exchange(call(5, "press", json!("yes")));
        assert_eq!(wrong["result"]["isError"], true, "{wrong}");
        drop(input);
        assert!(client.0.wait().unwrap().success());
    })
    .await
    .unwrap();
    let calls = server.await.unwrap();
    let methods: Vec<_> = calls
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["elementAction"]);
    assert_eq!(calls[0]["params"]["action"], "press");
    assert_eq!(calls[0]["params"]["realInput"], true);
}
