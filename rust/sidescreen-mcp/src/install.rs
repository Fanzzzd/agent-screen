//! Register Sidescreen's MCP server with the agent clients on this Mac:
//! Claude Code through its own CLI, Codex through its config file.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Outcome {
    pub client: &'static str,
    /// What happened, or why it couldn't; None when the client isn't installed.
    pub result: Option<Result<String, String>>,
}

/// Register `exe mcp` with every client found. Safe to run again: an entry
/// pointing elsewhere is replaced, one already right is left alone.
pub fn install(exe: &Path) -> Vec<Outcome> {
    vec![
        Outcome {
            client: "Claude Code",
            result: claude_cli().map(|cli| claude(&cli, exe)),
        },
        Outcome {
            client: "Codex",
            result: codex_config().map(|config| codex(&config, exe)),
        },
    ]
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// An app opened from Finder has a short PATH, so look where the installers
/// put the CLI too.
fn claude_cli() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("claude"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let usual = [
        home().join(".local/bin/claude"),
        home().join(".claude/local/claude"),
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
    ];
    on_path.into_iter().chain(usual).find(|p| p.is_file())
}

fn claude(cli: &Path, exe: &Path) -> Result<String, String> {
    let run = |args: &[&str]| {
        Command::new(cli)
            .args(args)
            .output()
            .map_err(|e| format!("could not run {}: {e}", cli.display()))
    };
    let exe = exe.to_string_lossy();
    let current = run(&["mcp", "get", "sidescreen"])?;
    let listed = String::from_utf8_lossy(&current.stdout).to_string();
    if current.status.success() && listed.contains(&*exe) {
        return Ok("already connected".into());
    }
    if current.status.success() {
        // Registered elsewhere, such as an older copy of the app.
        run(&["mcp", "remove", "--scope", "user", "sidescreen"])?;
    }
    let added = run(&[
        "mcp",
        "add",
        "--scope",
        "user",
        "sidescreen",
        "--",
        &exe,
        "mcp",
    ])?;
    if added.status.success() {
        Ok("connected for all projects".into())
    } else {
        Err(String::from_utf8_lossy(&added.stderr).trim().to_string())
    }
}

fn codex_config() -> Option<PathBuf> {
    let dir = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"));
    dir.is_dir().then(|| dir.join("config.toml"))
}

const HEADER: &str = "[mcp_servers.sidescreen]";

fn codex(config: &Path, exe: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let (text, done) = with_codex_entry(&text, &exe.to_string_lossy());
    if !done {
        return Ok("already connected".into());
    }
    std::fs::write(config, text)
        .map_err(|e| format!("could not write {}: {e}", config.display()))?;
    Ok(format!("added to {}", config.display()))
}

/// The config with Sidescreen's entry pointing at `exe`, and whether it
/// changed. Other entries and comments stay as they are.
fn with_codex_entry(text: &str, exe: &str) -> (String, bool) {
    let entry = format!(
        "{HEADER}\ncommand = {exe:?}\nargs = [\"mcp\"]\n# wait_for_control can wait up to 10 minutes for the user.\ntool_timeout_sec = 700\n"
    );
    let Some(start) = text.find(HEADER) else {
        let gap = if text.is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return (format!("{text}{gap}{entry}"), true);
    };
    // The entry runs to the next table that isn't one of its own subtables.
    let body = &text[start + HEADER.len()..];
    let end = body
        .match_indices("\n[")
        .find(|(i, _)| !body[i + 1..].starts_with("[mcp_servers.sidescreen."))
        .map_or(text.len(), |(i, _)| start + HEADER.len() + i + 1);
    let old = &text[start..end];
    if old.contains(&format!("command = {exe:?}")) {
        return (text.to_string(), false);
    }
    let blank = if end < text.len() { "\n" } else { "" };
    (
        format!("{}{entry}{blank}{}", &text[..start], &text[end..]),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/Applications/Sidescreen.app/Contents/MacOS/Sidescreen";

    #[test]
    fn appends_to_a_config_without_it() {
        let (text, changed) = with_codex_entry("model = \"o3\"\n", EXE);
        assert!(changed);
        assert!(text.starts_with("model = \"o3\"\n\n[mcp_servers.sidescreen]\n"));
        assert!(text.contains(&format!("command = {EXE:?}")));
    }

    #[test]
    fn leaves_a_correct_entry_alone() {
        let (once, _) = with_codex_entry("", EXE);
        assert_eq!(with_codex_entry(&once, EXE), (once.clone(), false));
    }

    #[test]
    fn replaces_an_entry_pointing_elsewhere_and_keeps_the_rest() {
        let before = "[a]\nx = 1\n\n[mcp_servers.sidescreen]\ncommand = \"/old\"\n\n[mcp_servers.sidescreen.env]\nA = \"1\"\n\n[b]\ny = 2\n";
        let (text, changed) = with_codex_entry(before, EXE);
        assert!(changed);
        assert!(text.starts_with("[a]\nx = 1\n\n[mcp_servers.sidescreen]\n"));
        assert!(!text.contains("/old") && !text.contains("A = \"1\""));
        assert!(text.ends_with("tool_timeout_sec = 700\n\n[b]\ny = 2\n"));
    }
}
