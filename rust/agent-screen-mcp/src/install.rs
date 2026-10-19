//! Register Agent Screen's MCP server with the agent clients on this Mac:
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

/// The clients whose config runs `exe` as Agent Screen's server, by reading
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

/// The server's name in the clients' configs: the development build
/// registers beside the installed app, not over it.
fn server_name(exe: &str) -> &'static str {
    if crate::endpoint::is_dev_exe(Path::new(exe)) {
        "agent-screen-dev"
    } else {
        "agent-screen"
    }
}

/// The name Sidescreen, the app's former name, registered its server under,
/// for the build `exe` is: registering removes that entry.
fn old_server_name(exe: &str) -> &'static str {
    if crate::endpoint::is_dev_exe(Path::new(exe)) {
        "sidescreen-dev"
    } else {
        "sidescreen"
    }
}

/// Whether `command` runs a Sidescreen app's binary, as an entry from before
/// the app's new name does: `…/Sidescreen.app/Contents/MacOS/Sidescreen`, or
/// the same in `Sidescreen Dev.app`. An entry pointing anywhere else is the
/// user's, and stays.
fn is_sidescreen_exe(command: &str) -> bool {
    let mut parts = Path::new(command)
        .components()
        .rev()
        .map(|c| c.as_os_str().to_str().unwrap_or(""));
    parts.next() == Some("Sidescreen")
        && parts.next() == Some("MacOS")
        && parts.next() == Some("Contents")
        && matches!(parts.next(), Some("Sidescreen.app" | "Sidescreen Dev.app"))
}

/// Whether Claude Code's user config has Sidescreen's entry for the build
/// `exe` is, running a Sidescreen app.
fn claude_has_old_entry(config: &str, exe: &str) -> bool {
    let pointer = format!("/mcpServers/{}/command", old_server_name(exe));
    serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|v| v.pointer(&pointer).cloned())
        .is_some_and(|command| command.as_str().is_some_and(is_sidescreen_exe))
}

/// Whether Claude Code's user config has an agent-screen server running `exe`.
fn claude_runs(config: &str, exe: &str) -> bool {
    let pointer = format!("/mcpServers/{}/command", server_name(exe));
    serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|v| v.pointer(&pointer).cloned())
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
        let marker = "__AGENTSCREEN_PATH__";
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
    let name = server_name(&exe);
    let current = run(&["mcp", "get", name])?;
    let listed = String::from_utf8_lossy(&current.stdout).to_string();
    if current.status.success() && listed.contains(&*exe) {
        remove_old_claude_entry(&run, &exe);
        return Ok(Done::Already);
    }
    if current.status.success() {
        // Registered elsewhere, such as an older copy of the app.
        run(&["mcp", "remove", "--scope", "user", name])?;
    }
    let added = run(&["mcp", "add", "--scope", "user", name, "--", &exe, "mcp"])?;
    if !added.status.success() {
        return Err(String::from_utf8_lossy(&added.stderr).trim().to_string());
    }
    remove_old_claude_entry(&run, &exe);
    Ok(Done::Connected)
}

/// Remove Sidescreen's entry once Agent Screen's is in, so the user's
/// agents don't get both servers. Only one that runs a Sidescreen app.
fn remove_old_claude_entry(
    run: &dyn Fn(&[&str]) -> Result<std::process::Output, String>,
    exe: &str,
) {
    let config = std::fs::read_to_string(home().join(".claude.json")).unwrap_or_default();
    if claude_has_old_entry(&config, exe) {
        let _ = run(&["mcp", "remove", "--scope", "user", old_server_name(exe)]);
    }
}

fn codex_config() -> Option<PathBuf> {
    let dir = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"));
    dir.is_dir().then(|| dir.join("config.toml"))
}

fn codex(config: &Path, exe: &Path) -> Result<Done, String> {
    let exe = exe.to_string_lossy();
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let (text, added) = with_codex_entry(&text, &exe);
    let (text, removed) = without_old_codex_entry(&text, &exe);
    if !added && !removed {
        return Ok(Done::Already);
    }
    std::fs::write(config, text)
        .map_err(|e| format!("could not write {}: {e}", config.display()))?;
    Ok(Done::AddedTo(config.to_path_buf()))
}

/// The config with Agent Screen's entry pointing at `exe`, and whether it
/// changed. Other entries and comments stay as they are.
fn with_codex_entry(text: &str, exe: &str) -> (String, bool) {
    let name = server_name(exe);
    let header = format!("[mcp_servers.{name}]");
    let header = header.as_str();
    let entry = format!(
        "{header}\ncommand = {exe:?}\nargs = [\"mcp\"]\n# wait_for_control can wait up to 10 minutes for the user.\ntool_timeout_sec = 700\n"
    );
    let Some(table) = codex_table(text, name) else {
        let gap = if text.is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return (format!("{text}{gap}{entry}"), true);
    };
    let (start, end) = (table.start, table.end);
    if text[start..end].contains(&format!("command = {exe:?}")) {
        return (text.to_string(), false);
    }
    let blank = if end < text.len() { "\n" } else { "" };
    (
        format!("{}{entry}{blank}{}", &text[..start], &text[end..]),
        true,
    )
}

/// Where server `name`'s entry is in a Codex config: from its header to the
/// next table that isn't one of its own subtables.
fn codex_table(text: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let header = format!("[mcp_servers.{name}]");
    let start = text.find(&header)?;
    let body = &text[start + header.len()..];
    let end = body
        .match_indices("\n[")
        .find(|(i, _)| !body[i + 1..].starts_with(&format!("[mcp_servers.{name}.")))
        .map_or(text.len(), |(i, _)| start + header.len() + i + 1);
    Some(start..end)
}

/// The config without Sidescreen's entry for the build `exe` is, and
/// whether it changed. Only an entry whose command runs a Sidescreen app
/// goes; one pointing elsewhere is the user's.
fn without_old_codex_entry(text: &str, exe: &str) -> (String, bool) {
    let Some(table) = codex_table(text, old_server_name(exe)) else {
        return (text.to_string(), false);
    };
    let runs_sidescreen = text[table.clone()]
        .lines()
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter_map(toml_command)
        .any(|command| is_sidescreen_exe(&command));
    if !runs_sidescreen {
        return (text.to_string(), false);
    }
    let mut before = text[..table.start].to_string();
    let after = &text[table.end..];
    if after.is_empty() {
        // The entry was last: drop the blank line left before it.
        while before.ends_with("\n\n") {
            before.pop();
        }
    }
    (format!("{before}{after}"), true)
}

/// The value of a `command = "…"` line, in either kind of TOML string.
fn toml_command(line: &str) -> Option<String> {
    let value = line
        .trim()
        .strip_prefix("command")?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    if let Some(literal) = value.strip_prefix('\'') {
        return literal.split('\'').next().map(str::to_string);
    }
    let mut out = String::new();
    let mut chars = value.strip_prefix('"')?.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen";

    #[test]
    fn counts_only_an_entry_that_runs_this_copy() {
        let claude = |command: &str| {
            format!(
                r#"{{"projects":{{"/x":{{"disabledMcpServers":["agent-screen"]}}}},"mcpServers":{{"agent-screen":{{"command":"{command}","args":["mcp"]}}}}}}"#
            )
        };
        assert!(claude_runs(&claude(EXE), EXE));
        assert!(!claude_runs(&claude("/Users/me/old/Agent Screen"), EXE));
        assert!(
            !claude_runs(
                r#"{"projects":{"/x":{"disabledMcpServers":["agent-screen"]}}}"#,
                EXE
            ),
            "a project list naming it isn't a registration"
        );
        let (codex, _) = with_codex_entry("", EXE);
        assert!(codex_runs(&codex, EXE));
        assert!(!codex_runs(&codex, "/Users/me/old/Agent Screen"));
        assert!(!codex_runs("", EXE));
    }

    #[test]
    fn the_development_build_registers_beside_the_installed_app() {
        let dev =
            "/Users/me/agent-screen/rust/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";
        let (text, _) = with_codex_entry("", EXE);
        let (both, changed) = with_codex_entry(&text, dev);
        assert!(changed);
        assert!(
            both.contains("[mcp_servers.agent-screen]")
                && both.contains("[mcp_servers.agent-screen-dev]")
        );
        assert!(codex_runs(&both, EXE) && codex_runs(&both, dev));
    }

    #[test]
    fn appends_to_a_config_without_it() {
        let (text, changed) = with_codex_entry("model = \"o3\"\n", EXE);
        assert!(changed);
        assert!(text.starts_with("model = \"o3\"\n\n[mcp_servers.agent-screen]\n"));
        assert!(text.contains(&format!("command = {EXE:?}")));
    }

    #[test]
    fn leaves_a_correct_entry_alone() {
        let (once, _) = with_codex_entry("", EXE);
        assert_eq!(with_codex_entry(&once, EXE), (once.clone(), false));
    }

    #[test]
    fn replaces_an_entry_pointing_elsewhere_and_keeps_the_rest() {
        let before = "[a]\nx = 1\n\n[mcp_servers.agent-screen]\ncommand = \"/old\"\n\n[mcp_servers.agent-screen.env]\nA = \"1\"\n\n[b]\ny = 2\n";
        let (text, changed) = with_codex_entry(before, EXE);
        assert!(changed);
        assert!(text.starts_with("[a]\nx = 1\n\n[mcp_servers.agent-screen]\n"));
        assert!(!text.contains("/old") && !text.contains("A = \"1\""));
        assert!(text.ends_with("tool_timeout_sec = 700\n\n[b]\ny = 2\n"));
    }

    const OLD_EXE: &str = "/Applications/Sidescreen.app/Contents/MacOS/Sidescreen";
    const OLD_DEV: &str =
        "/Users/me/sidescreen/rust/target/Sidescreen Dev.app/Contents/MacOS/Sidescreen";
    const DEV: &str =
        "/Users/me/agent-screen/rust/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";

    /// An entry as Sidescreen wrote it.
    fn old_codex_entry(name: &str, exe: &str) -> String {
        format!(
            "[mcp_servers.{name}]\ncommand = {exe:?}\nargs = [\"mcp\"]\n# wait_for_control can wait up to 10 minutes for the user.\ntool_timeout_sec = 700\n"
        )
    }

    #[test]
    fn knows_a_sidescreen_binary() {
        assert!(is_sidescreen_exe(OLD_EXE));
        assert!(is_sidescreen_exe(OLD_DEV));
        assert!(!is_sidescreen_exe(EXE));
        assert!(!is_sidescreen_exe("/usr/local/bin/sidescreen"));
        assert!(!is_sidescreen_exe("/Users/me/forks/Sidescreen"));
        assert!(!is_sidescreen_exe(
            "/Applications/Other.app/Contents/MacOS/Sidescreen"
        ));
    }

    #[test]
    fn codex_trades_sidescreens_entry_for_agent_screens() {
        let before = format!(
            "model = \"o3\"\n\n{}\n[mcp_servers.sidescreen.env]\nA = \"1\"\n\n[b]\ny = 2\n",
            old_codex_entry("sidescreen", OLD_EXE)
        );
        let (text, added) = with_codex_entry(&before, EXE);
        let (text, removed) = without_old_codex_entry(&text, EXE);
        assert!(added && removed);
        assert!(!text.contains("sidescreen") && !text.contains("Sidescreen"));
        assert!(text.starts_with("model = \"o3\"\n\n[b]\ny = 2\n\n[mcp_servers.agent-screen]\n"));
        assert!(codex_runs(&text, EXE));
        assert_eq!(
            without_old_codex_entry(&text, EXE),
            (text.clone(), false),
            "nothing left to remove"
        );
    }

    #[test]
    fn codex_removes_a_last_entry_without_leaving_a_gap() {
        let before = format!(
            "model = \"o3\"\n\n{}",
            old_codex_entry("sidescreen", OLD_EXE)
        );
        let (text, removed) = without_old_codex_entry(&before, EXE);
        assert!(removed);
        assert_eq!(text, "model = \"o3\"\n");
        let literal = "[mcp_servers.sidescreen]\ncommand = '/Applications/Sidescreen.app/Contents/MacOS/Sidescreen'\n";
        assert_eq!(without_old_codex_entry(literal, EXE), (String::new(), true));
    }

    #[test]
    fn codex_keeps_a_sidescreen_entry_pointing_elsewhere() {
        let mine = old_codex_entry("sidescreen", "/Users/me/bin/my-sidescreen");
        assert_eq!(without_old_codex_entry(&mine, EXE), (mine.clone(), false));
    }

    #[test]
    fn each_build_removes_only_its_own_old_entry() {
        let both = format!(
            "{}\n{}",
            old_codex_entry("sidescreen", OLD_EXE),
            old_codex_entry("sidescreen-dev", OLD_DEV)
        );
        let (dev, removed) = without_old_codex_entry(&both, DEV);
        assert!(removed);
        assert!(dev.contains("[mcp_servers.sidescreen]\n") && !dev.contains("sidescreen-dev"));
        let (release, removed) = without_old_codex_entry(&both, EXE);
        assert!(removed);
        assert!(release.starts_with("[mcp_servers.sidescreen-dev]\n"));
    }

    #[test]
    fn claude_finds_sidescreens_entry_only_when_it_runs_sidescreen() {
        let config = |name: &str, command: &str| {
            format!(r#"{{"mcpServers":{{"{name}":{{"command":"{command}","args":["mcp"]}}}}}}"#)
        };
        assert!(claude_has_old_entry(&config("sidescreen", OLD_EXE), EXE));
        assert!(claude_has_old_entry(
            &config("sidescreen-dev", OLD_DEV),
            DEV
        ));
        assert!(!claude_has_old_entry(
            &config("sidescreen-dev", OLD_DEV),
            EXE
        ));
        assert!(!claude_has_old_entry(
            &config("sidescreen", "/opt/other/sidescreen"),
            EXE
        ));
        assert!(!claude_has_old_entry(&config("agent-screen", EXE), EXE));
        assert!(!claude_has_old_entry("", EXE));
    }
}
