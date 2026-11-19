//! Real MCP clients installed on this Mac load Agent Screen's tools through
//! the pipe from this build (ADR 0021): each one gets a throwaway project
//! whose own config names `AgentScreen mcp` on the test instance, and lists
//! the tools or reports the server connected. The user's client configs are
//! never read or written: Cursor's CLI runs with a throwaway home, where it
//! keeps its approval of the project's server. A client that isn't installed
//! is skipped.
//!
//! Only clients with a command that lists one server's tools without a
//! model run here. Codex's `codex mcp list` reads the config and connects
//! to nothing. OpenCode's `opencode mcp list` also starts every server in
//! the user's own config, and Claude Code's `claude mcp list` checks them
//! all, so neither runs here.
//!
//!     AGENTSCREEN_INSTANCE=e2e cargo e2e mcpclients
//!
//! Makes no display and calls no model.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use agent_screen_tests::{
    check, cut, debug_app, instance, json, run, start_test_app, stop_test_app,
};
use anyhow::{Context, Result};

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

const SERVER: &str = "agent-screen-e2e";

fn binary() -> String {
    format!("{}/Contents/MacOS/AgentScreen", debug_app().display())
}

/// A client's command line, found on PATH or in its usual install place.
fn find(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut places: Vec<PathBuf> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .collect();
    places.push(Path::new(&home).join(".local/bin").join(name));
    places.into_iter().find(|p| p.is_file())
}

/// A fresh project folder for one client, a git repository outside this
/// checkout.
fn project(client: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir()
        .join(format!("agent-screen-mcpclients-{}", std::process::id()))
        .join(client);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .status()?;
    Ok(dir)
}

/// Run `command` in `dir` for at most `seconds`; its stdout and stderr.
fn output(mut command: Command, dir: &Path, seconds: u64) -> Result<String> {
    let log = dir.join("output.log");
    let file = std::fs::File::create(&log)?;
    let mut child = command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file)
        .spawn()?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(seconds) {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(std::fs::read_to_string(&log).unwrap_or_default())
}

fn test() -> Result<()> {
    // The pipe finds the test instance's socket under the real home, even
    // when its client runs with another.
    let home = std::env::var("HOME").unwrap_or_default();
    let env = json!({ "AGENTSCREEN_INSTANCE": "e2e", "HOME": home });
    let mut ran = 0;

    // Cursor's CLI reads the project's .cursor/mcp.json.
    if let Some(cursor) = find("cursor-agent") {
        let dir = project("cursor")?;
        std::fs::create_dir_all(dir.join(".cursor"))?;
        std::fs::write(
            dir.join(".cursor/mcp.json"),
            json!({ "mcpServers": { SERVER: { "command": binary(), "args": ["mcp"], "env": env } } })
                .to_string(),
        )?;
        let own_home = dir.join("home");
        std::fs::create_dir_all(&own_home)?;
        let cli = |args: &[&str]| {
            let mut command = Command::new(&cursor);
            command.args(args).env("HOME", &own_home);
            command
        };
        // A project's server loads only once approved.
        let approved = output(cli(&["mcp", "enable", SERVER]), &dir, 60)?;
        println!("     {}", approved.trim());
        let text = output(cli(&["mcp", "list-tools", SERVER]), &dir, 60)?;
        check(
            "Cursor's CLI lists create_display",
            text.contains("create_display"),
            cut(&text, 400),
        );
        ran += 1;
    } else {
        println!("     Cursor's CLI (cursor-agent) is not installed");
    }

    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("agent-screen-mcpclients-{}", std::process::id())),
    );
    check("at least one real client was installed", ran > 0, "")
        .then_some(())
        .context("no client to run")?;
    Ok(())
}
