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
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = PathBuf::from("/tmp").join(format!(
            "as-steps-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
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

#[tokio::test]
async fn subtree_reads_forward_scope_and_keep_change_baselines_separate() {
    let fixture = Fixture::new();
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut reads = 0;
        let mut pressed = false;
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let p = &request["params"];
            if p["root"] == 999 {
                let reply = json!({"id":request["id"],"error":{"code":"invalid","message":"root element 999 is unavailable in this window"}});
                write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
                continue;
            }
            let result = match request["method"].as_str().unwrap() {
                "agentReady" | "settle" => json!({}),
                "elementAction" => {
                    assert_eq!(p["element"], 10);
                    assert_eq!(p["root"], 2);
                    pressed = true;
                    json!({})
                }
                "windowState" => {
                    reads += 1;
                    let tree = match (p["root"].as_u64(), p["maxDepth"].as_u64()) {
                        (None, None | Some(1)) => "[1] row Sidebar\n[2] table Messages",
                        (Some(2), Some(1)) if pressed => {
                            "[2] table Messages\n  [10] row Card opened"
                        }
                        (Some(2), Some(1)) => "[2] table Messages\n  [10] row Card",
                        (Some(0), Some(0)) => "[0] window",
                        _ => panic!("unexpected snapshot parameters {p}"),
                    };
                    json!({"tree":tree,"elements":2,"root":p["root"],"depthLimited":p["root"] == 0,"data":"","mimeType":"image/png"})
                }
                other => panic!("unexpected method {other}"),
            };
            let reply = json!({"id":request["id"],"result":result});
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
        // result "none" reads the scoped state too, for its strayed-window
        // note and its change baseline.
        assert_eq!(reads, 8);
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
        exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"subtree-test","version":"1"}}}));
        exchange(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let tools = exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
        for name in ["get_window_state", "act_on_element"] {
            let tool = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap();
            assert_eq!(tool["inputSchema"]["properties"]["root"]["type"], "integer");
        }
        let mut call = |id, name, args| exchange(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}));
        for (id, args) in [
            (3, json!({"display_id":7,"window_id":8})),
            (4, json!({"display_id":7,"window_id":8,"max_depth":1})),
            (5, json!({"display_id":7,"window_id":8,"root":2,"max_depth":1})),
        ] {
            let reply = call(id, "get_window_state", args);
            assert_ne!(reply["result"]["isError"], true, "{reply}");
        }
        let changed = call(6, "act_on_element", json!({"display_id":7,"window_id":8,"element":10,"action":"press","root":2,"max_depth":1,"result":"changes"}));
        assert_ne!(changed["result"]["isError"], true, "{changed}");
        let text = changed["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("~ [10] row Card opened"), "{text}");
        assert!(!text.contains("Sidebar"), "{text}");
        assert!(!text.contains("+ [2]"), "{text}");
        let none = call(7, "act_on_element", json!({"display_id":7,"window_id":8,"element":10,"action":"press","root":2,"max_depth":1,"result":"none"}));
        assert_ne!(none["result"]["isError"], true, "{none}");
        assert_eq!(none["result"]["content"].as_array().unwrap().len(), 1);
        assert_eq!(none["result"]["content"][0]["text"], "Done.");
        let zero = call(8, "get_window_state", json!({"display_id":7,"window_id":8,"root":0,"max_depth":0}));
        assert_ne!(zero["result"]["isError"], true, "{zero}");
        assert!(zero["result"]["content"][0]["text"].as_str().unwrap().contains("Deeper elements were not read"));
        for name in ["get_window_state", "act_on_element"] {
            let stale = call(9, name, json!({"display_id":7,"window_id":8,"element":10,"action":"press","root":999}));
            assert_eq!(stale["result"]["isError"], true, "{stale}");
            let text = stale["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("root element 999"), "{text}");
            assert!(!text.contains("closed window"), "{text}");
        }
        for root in [json!(-1), json!("2")] {
            let invalid = call(7, "get_window_state", json!({"display_id":7,"window_id":8,"root":root}));
            assert_eq!(invalid["result"]["isError"], true, "{invalid}");
        }
        drop(input);
        assert!(client.0.wait().unwrap().success());
    }).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn scoped_actions_preflight_legacy_peers_and_keep_success_when_the_root_is_removed() {
    let fixture = Fixture::new();
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut removed = std::collections::HashSet::new();
        let mut legacy_actions = 0;
        let mut actions = 0;
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let p = &request["params"];
            let root = p["root"].as_u64();
            let result = match request["method"].as_str().unwrap() {
                "agentReady" | "settle" => json!({}),
                "windowState" if removed.contains(&root.unwrap()) => {
                    let reply = json!({"id":request["id"],"error":{"code":"invalid","message":"the root element no longer belongs to this window"}});
                    write
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
                // A legacy app ignores root and returns a whole-window tree.
                "windowState" if root.unwrap() >= 99 => {
                    json!({"tree":"[0] window","elements":1,"data":"","mimeType":"image/png"})
                }
                "windowState" => {
                    json!({"root":root,"tree":format!("[{}] row", root.unwrap()),"elements":1,"data":"","mimeType":"image/png"})
                }
                "elementAction" if root.unwrap() >= 99 => {
                    legacy_actions += 1;
                    if p["action"] == "read_text" {
                        json!({"text":"legacy text"})
                    } else {
                        json!({"menu":"[5] menuitem"})
                    }
                }
                "elementAction" => {
                    actions += 1;
                    removed.insert(root.unwrap());
                    json!({})
                }
                other => panic!("unexpected control call {other}"),
            };
            let reply = json!({"id":request["id"],"result":result});
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
        assert_eq!(actions, 3);
        assert_eq!(
            legacy_actions, 0,
            "an unsupported scoped action reached the old app"
        );
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
        exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"subtree-actions-test","version":"1"}}}));
        exchange(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let mut call = |id, arguments| exchange(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"act_on_element","arguments":arguments}}));
        for (root, result) in [(11, "none"), (12, "state"), (13, "changes")] {
            let reply = call(root, json!({"display_id":7,"window_id":8,"root":root,"element":10,"action":"press","result":result}));
            assert_ne!(reply["result"]["isError"], true, "an action that already ran must not report failure: {reply}");
            let text = reply["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.starts_with("Done"), "{text}");
        }
        for (root, action) in [(99, "press"), (99, "read_text"), (100, "press")] {
            let reply = call(root, json!({"display_id":7,"window_id":8,"root":root,"element":10,"action":action,"result":"none"}));
            assert_eq!(reply["result"]["isError"], true, "{reply}");
            assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("did not confirm"));
        }
        drop(input);
        assert!(client.0.wait().unwrap().success());
    }).await.unwrap();
    server.await.unwrap();
}
