//! Register Sidescreen's MCP server with the agent clients on this Mac:
//! Claude Code through its own CLI, Codex through its config file.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Outcome {
    pub client: &'static str,
    /// What happened, or why it couldn't; None when the client isn't installed.
    pub result: Option<Result<Done, String>>,
}

/// What registering did, for the caller to put in the user's language.
pub enum Done {
    Already,
    /// Claude Code, for every project.
    Connected,
    /// Codex, in this config file.
    AddedTo(PathBuf),
}

impl std::fmt::Display for Done {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Done::Already => write!(f, "already connected"),
            Done::Connected => write!(f, "connected for all projects"),
            Done::AddedTo(path) => write!(f, "added to {}", path.display()),
        }
    }
}

/// The clients whose config runs `exe` as Sidescreen's server, by reading
/// their files: Claude Code's ~/.claude.json and Codex's config.toml. An
/// entry for another copy, moved or deleted since, doesn't count.
pub fn registered(exe: &Path) -> Vec<&'static str> {
    let exe = exe.to_string_lossy();
    let mut found = Vec::new();
    let claude = std::fs::read_to_string(home().join(".claude.json")).unwrap_or_default();
    if claude_runs(&claude, &exe) {
        found.push("Claude Code");
    }
    let codex = codex_config()
        .and_then(|config| std::fs::read_to_string(config).ok())
        .unwrap_or_default();
    if codex_runs(&codex, &exe) {
        found.push("Codex");
    }
    found
}

/// Whether Claude Code's user config has a sidescreen server running `exe`.
fn claude_runs(config: &str, exe: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|v| v.pointer("/mcpServers/sidescreen/command").cloned())
        .is_some_and(|command| command.as_str() == Some(exe))
}

fn codex_runs(config: &str, exe: &str) -> bool {
    !with_codex_entry(config, exe).1
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

/// The PATH the user's terminal has. An app opened from Finder gets only
/// /usr/bin:/bin:/usr/sbin:/sbin, which misses node for an npm-installed
/// CLI and the version managers' directories. Asks an interactive login
/// shell, as a terminal would start, and gives up after 5 s.
fn user_path() -> Option<std::ffi::OsString> {
    static PATH: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
        let marker = "__SIDESCREEN_PATH__";
        let mut child = Command::new(shell)
            .args(["-ilc", &format!("printf '\\n{marker}%s' \"$PATH\"")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let started = std::time::Instant::now();
        while child.try_wait().ok()?.is_none() {
            if started.elapsed() > std::time::Duration::from_secs(5) {
                let _ = child.kill();
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut out = String::new();
        std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
        let path = out.rsplit(marker).next()?.trim().to_string();
        (!path.is_empty() && out.contains(marker)).then(|| path.into())
    })
    .clone()
}

/// The PATH to run the CLI with: the user's, else the app's.
fn path() -> Option<std::ffi::OsString> {
    user_path().or_else(|| std::env::var_os("PATH"))
}

/// Look on the user's PATH, and where the installers put the CLI too.
fn claude_cli() -> Option<PathBuf> {
    let on_path = path()
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

fn claude(cli: &Path, exe: &Path) -> Result<Done, String> {
    let run = |args: &[&str]| {
        let mut command = Command::new(cli);
        if let Some(path) = path() {
            command.env("PATH", path);
        }
        command
            .args(args)
            .output()
            .map_err(|e| format!("could not run {}: {e}", cli.display()))
    };
    let exe = exe.to_string_lossy();
    let current = run(&["mcp", "get", "sidescreen"])?;
    let listed = String::from_utf8_lossy(&current.stdout).to_string();
    if current.status.success() && listed.contains(&*exe) {
        return Ok(Done::Already);
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
        Ok(Done::Connected)
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

fn codex(config: &Path, exe: &Path) -> Result<Done, String> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let (text, done) = with_codex_entry(&text, &exe.to_string_lossy());
    if !done {
        return Ok(Done::Already);
    }
    std::fs::write(config, text)
        .map_err(|e| format!("could not write {}: {e}", config.display()))?;
    Ok(Done::AddedTo(config.to_path_buf()))
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
    fn counts_only_an_entry_that_runs_this_copy() {
        let claude = |command: &str| {
            format!(
                r#"{{"projects":{{"/x":{{"disabledMcpServers":["sidescreen"]}}}},"mcpServers":{{"sidescreen":{{"command":"{command}","args":["mcp"]}}}}}}"#
            )
        };
        assert!(claude_runs(&claude(EXE), EXE));
        assert!(!claude_runs(&claude("/Users/me/old/Sidescreen"), EXE));
        assert!(
            !claude_runs(
                r#"{"projects":{"/x":{"disabledMcpServers":["sidescreen"]}}}"#,
                EXE
            ),
            "a project list naming it isn't a registration"
        );
        let (codex, _) = with_codex_entry("", EXE);
        assert!(codex_runs(&codex, EXE));
        assert!(!codex_runs(&codex, "/Users/me/old/Sidescreen"));
        assert!(!codex_runs("", EXE));
    }

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
