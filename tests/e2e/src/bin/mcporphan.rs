//! The app ends an MCP process on its control socket once the client that
//! started it has exited, and logs it; one whose client still runs stays.
//! A pipe ends by itself when its parent exits, even with stdin still open
//! (ADR 0021).
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e mcporphan
//!
//! Makes no display. Takes up to 35 s: the app looks for orphans every 30 s
//! and whenever a connection opens.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    Control, McpClient, check, cut, debug_app, instance, json, run, sleep_ms, start_test_app,
    stop_test_app,
};
use anyhow::{Context, Result, anyhow};

fn main() -> ExitCode {
    if instance().is_none() {
        eprintln!("run with AGENTSCREEN_INSTANCE=e2e");
        return ExitCode::FAILURE;
    }
    run(|| {
        start_test_app(&[("AGENTSCREEN_LANG", "en")])?;
        let result = test();
        stop_test_app();
        result
    })
}

/// A shell that starts `AgentScreen mcp ARGS` on the shell's own stdin,
/// which this script holds, and prints its pid: the server's parent is the
/// shell, and killing the shell leaves the server's stdin open.
struct Parent {
    shell: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    server: i32,
}

impl Parent {
    fn start(args: &str) -> Result<Parent> {
        let binary = format!("{}/Contents/MacOS/AgentScreen", debug_app().display());
        let mut shell = Command::new("/bin/sh")
            .args([
                "-c",
                &format!(r#"exec 3<&0; "$0" {args} <&3 & echo $!; wait"#),
                &binary,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = shell.stdin.take().context("stdin")?;
        let stdout = shell.stdout.take().context("stdout")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let server = lines.recv_timeout(Duration::from_secs(5))?.trim().parse()?;
        Ok(Parent {
            shell,
            stdin,
            lines,
            server,
        })
    }

    /// Write a JSON-RPC request and wait for the server's reply line.
    fn request(&mut self, message: serde_json::Value) -> Result<String> {
        writeln!(self.stdin, "{message}")?;
        self.lines
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| anyhow!("no reply to {message}"))
    }

    fn notify(&mut self, message: serde_json::Value) -> Result<()> {
        writeln!(self.stdin, "{message}")?;
        Ok(())
    }

    fn handshake(&mut self) -> Result<String> {
        let init = self.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"mcporphan","version":"0"}}}))?;
        self.notify(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        Ok(init)
    }

    /// Kill the shell; the server stays, its stdin held open here.
    fn kill_shell(&mut self) {
        let _ = self.shell.kill();
        let _ = self.shell.wait();
    }
}

impl Drop for Parent {
    fn drop(&mut self) {
        self.kill_shell();
        // SAFETY: the server this script started.
        unsafe { libc::kill(self.server, libc::SIGKILL) };
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn parent_of(pid: i32) -> String {
    agent_screen_tests::sh_lenient("/bin/ps", &["-o", "ppid=", "-p", &pid.to_string()])
        .trim()
        .to_string()
}

/// Wait up to `seconds` for `pid` to exit.
fn ends(pid: i32, seconds: u64) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        if !alive(pid) {
            return Some(start.elapsed());
        }
        sleep_ms(100);
    }
    None
}

fn test() -> Result<()> {
    // A pipe ends with its parent.
    let mut pipe = Parent::start("mcp")?;
    let init = pipe.handshake()?;
    check(
        "the pipe answers initialize from the app",
        init.contains("agent-screen"),
        cut(&init, 200),
    );
    pipe.kill_shell();
    let ended = ends(pipe.server, 5);
    check(
        "the pipe ends within 5 s of its parent, stdin still open",
        ended.is_some(),
        format!("{ended:?}"),
    );

    // A v1 process has no parent watch: once its client is gone, the app
    // ends it. Another whose client runs stays.
    let staying = McpClient::start_v1(&[])?;
    staying.request("tools/list", json!({}))?;
    let mut orphan = Parent::start("mcp --v1")?;
    orphan.handshake()?;
    // tools/list makes it greet the app on a connection it keeps open.
    orphan.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))?;
    sleep_ms(500);
    let log_before = log_text();
    orphan.kill_shell();
    sleep_ms(200);
    println!(
        "     the v1 process's parent is now {}",
        parent_of(orphan.server)
    );
    // A new connection makes the app look at once.
    let _poke = Control::connect()?;
    let ended = ends(orphan.server, 35);
    check(
        "the app ends the v1 process whose client exited",
        ended.is_some(),
        format!("{ended:?}"),
    );
    let log = log_text();
    let added = log.get(log_before.len()..).unwrap_or(&log);
    check(
        "the app logs the orphan it ended",
        added.contains(&format!("ended orphaned MCP process {}", orphan.server)),
        cut(added, 400),
    );
    check(
        "the v1 process whose client runs is still there",
        alive(staying.pid() as i32),
        "",
    );
    staying.close();
    Ok(())
}

fn log_text() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let name = format!("displays-{}.log", instance().unwrap_or_default());
    std::fs::read_to_string(format!("{home}/Library/Logs/Agent Screen/{name}")).unwrap_or_default()
}
