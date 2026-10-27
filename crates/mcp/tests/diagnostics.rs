//! Subprocesses isolate endpoint environment from both the test runner and the
//! user's app. No production socket, display, GUI, or launch path is used.
#![cfg(unix)]

use agent_screen_mcp::diagnostics::control_reply;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = PathBuf::from("/tmp").join(format!(
            "as-diag-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-screen-mcp"));
        command
            .env("HOME", &self.0)
            .env("XDG_RUNTIME_DIR", &self.0)
            .env("AGENTSCREEN_INSTANCE", "diagnostics-test");
        command
    }

    fn listener(&self) -> tokio::net::UnixListener {
        #[cfg(target_os = "macos")]
        let base = self.0.join("Library/Application Support/Agent Screen");
        #[cfg(not(target_os = "macos"))]
        let base = self.0.join("agent-screen");
        std::fs::create_dir_all(&base).unwrap();
        tokio::net::UnixListener::bind(base.join("control-diagnostics-test.sock")).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn cli(mut command: Command) -> Value {
    let output = tokio::task::spawn_blocking(move || {
        command.args(["diagnostics", "--json"]).output().unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn cli_and_current_mcp_share_reports_without_creating_a_display() {
    let fixture = Fixture::new();
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut lines = BufReader::new(r).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "getDiagnostics");
            assert_eq!(request["params"], json!({}));
            let reply = control_reply(
                request["id"].clone(),
                request["method"].as_str().unwrap(),
                env!("CARGO_PKG_VERSION"),
            )
            .unwrap();
            w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            // The diagnostics connection closes. No other control method was sent.
            assert_eq!(lines.next_line().await.unwrap(), None);
        }
    });
    let cli_report = cli(fixture.command()).await;
    let mut command = fixture.command();
    let mcp_report = tokio::task::spawn_blocking(move || {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"diagnostics-test","version":"1"}}})).unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let init: Value = serde_json::from_str(&line).unwrap();
        assert!(init["result"]["instructions"].as_str().unwrap().contains("get_diagnostics"));
        writeln!(input, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_diagnostics","arguments":{}}})).unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        let result: Value = serde_json::from_str(&line).unwrap();
        assert!(result.get("error").is_none(), "{result}");
        let report = result["result"]["structuredContent"].clone();
        let text_report: Value = serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report, text_report);
        drop(input);
        assert!(child.wait().unwrap().success());
        report
    }).await.unwrap();
    server.await.unwrap();
    assert_eq!(cli_report["collector"], "cli");
    assert_eq!(cli_report["mcp_status"], "not_collected");
    assert_eq!(mcp_report["collector"], "mcp");
    assert_eq!(mcp_report["mcp_status"], "collected");
    for field in [
        "local",
        "app",
        "comparison",
        "instance",
        "os",
        "report_schema_version",
    ] {
        assert_eq!(cli_report[field], mcp_report[field], "{field}");
    }
    assert_eq!(cli_report["instance"], "custom_redacted");
    assert_eq!(cli_report["app"]["status"], "available");
    assert!(!cli_report.to_string().contains(fixture.0.to_str().unwrap()));
}

#[tokio::test]
async fn absent_app_is_reported_without_launching_or_creating_endpoint_directories() {
    let fixture = Fixture::new();
    let report = cli(fixture.command()).await;
    assert_eq!(report["app"]["status"], "unavailable");
    assert_eq!(report["comparison"], "unknown");
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    for args in [
        ["diagnostics", "--help"].as_slice(),
        ["--version"].as_slice(),
        ["--help"].as_slice(),
    ] {
        let out = fixture.command().args(args).output().unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("get_diagnostics"), "{text}");
    }
    assert!(
        !fixture
            .command()
            .args(["diagnostics", "--upload"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
}

#[tokio::test]
async fn legacy_and_timeout_diagnostics_leave_the_existing_mcp_transport_open() {
    let fixture = Fixture::new();
    let listener = fixture.listener();
    let server = tokio::spawn(async move {
        // This endpoint only emulates the app. No display exists.
        let (session, _) = listener.accept().await.unwrap();
        let (r, mut w) = session.into_split();
        let mut session_lines = BufReader::new(r).lines();
        let create: Value =
            serde_json::from_str(&session_lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(create["method"], "createDisplay");
        let created =
            json!({"id":create["id"],"result":{"displayId":7,"width":1280,"height":800,"scale":1}});
        w.write_all(format!("{created}\n").as_bytes())
            .await
            .unwrap();
        for status in ["legacy", "timeout"] {
            let (diagnostic, _) = listener.accept().await.unwrap();
            let (r, mut diagnostic_writer) = diagnostic.into_split();
            let mut diagnostic_lines = BufReader::new(r).lines();
            let request: Value =
                serde_json::from_str(&diagnostic_lines.next_line().await.unwrap().unwrap())
                    .unwrap();
            assert_eq!(request["method"], "getDiagnostics");
            if status == "legacy" {
                let legacy = json!({"id":request["id"],"error":{"code":"invalid","message":"unknown method getDiagnostics"}});
                diagnostic_writer
                    .write_all(format!("{legacy}\n").as_bytes())
                    .await
                    .unwrap();
            }
            assert_eq!(diagnostic_lines.next_line().await.unwrap(), None);
            // A normal tool still uses the original connection after either failure.
            let list: Value =
                serde_json::from_str(&session_lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(list["method"], "listWindows");
            assert_eq!(list["params"]["displayId"], 7);
            let listed = json!({"id":list["id"],"result":{"windows":[]}});
            w.write_all(format!("{listed}\n").as_bytes()).await.unwrap();
        }
        assert_eq!(session_lines.next_line().await.unwrap(), None);
    });
    let mut command = fixture.command();
    tokio::task::spawn_blocking(move || {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        fn call(input: &mut impl Write, output: &mut impl BufRead, request: Value) -> Value {
            writeln!(input, "{request}").unwrap();
            input.flush().unwrap();
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["id"], request["id"]);
            assert!(reply.get("error").is_none(), "{reply}");
            assert_ne!(reply["result"]["isError"], true, "{reply}");
            reply["result"].clone()
        }
        call(&mut input, &mut output, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"diagnostics-test","version":"1"}}}));
        writeln!(input, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
        let created = call(&mut input, &mut output, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_display","arguments":{}}}));
        assert_eq!(created["structuredContent"]["display_id"], 7);
        for (id, expected) in [(3,"unsupported"),(5,"timeout")] {
            let report = call(&mut input, &mut output, json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"get_diagnostics","arguments":{}}}));
            assert_eq!(report["structuredContent"]["app"]["status"], expected);
            let listed = call(&mut input, &mut output, json!({"jsonrpc":"2.0","id":id+1,"method":"tools/call","params":{"name":"list_windows","arguments":{"display_id":7}}}));
            assert_eq!(listed["structuredContent"]["windows"], json!([]));
        }
        drop(input);
        assert!(child.wait().unwrap().success());
    }).await.unwrap();
    server.await.unwrap();
}
