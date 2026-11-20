//! Register Agent Screen's MCP server with the agent clients on this Mac:
//! Claude Code through its own CLI, Codex through its config file. Each
//! client also gets the agent-screen skill this copy of the app carries, so
//! the skill describes the tools this version has.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Outcome {
    pub client: &'static str,
    /// What happened, or why it couldn't; None when the client isn't installed.
    pub result: Option<Result<Done, String>>,
    /// The skill for this client; None when the client isn't installed.
    pub skill: Option<Result<Skill, String>>,
}

/// What installing the skill did.
pub enum Skill {
    /// Written there, new or replacing an older copy.
    Written(PathBuf),
    /// Already this version there.
    Current(PathBuf),
    /// Claude Code's agent-screen plugin carries it.
    Plugin,
    /// A link there points to a copy another installer keeps, left alone.
    Linked(PathBuf),
}

/// The agent-screen skill, as this version of the app describes its tools.
pub const SKILL: &str = include_str!("../../../skills/agent-screen/SKILL.md");

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

/// Whether Claude Code's user config has a agent-screen server running `exe`.
fn claude_runs(config: &str, exe: &str) -> bool {
    let pointer = format!("/mcpServers/{}/command", server_name(exe));
    serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|v| v.pointer(&pointer).cloned())
        .is_some_and(|command| command.as_str() == Some(exe))
}

/// Whether Codex's config has an agent-screen server running `exe`.
fn codex_runs(config: &str, exe: &str) -> bool {
    config
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| {
            let command = doc
                .get("mcp_servers")?
                .get(server_name(exe))?
                .get("command")?;
            Some(command.as_str() == Some(exe))
        })
        .unwrap_or(false)
}

/// Register `exe mcp` with every client found. Safe to run again: an entry
/// pointing elsewhere is replaced, one already right is left alone.
pub fn install(exe: &Path) -> Vec<Outcome> {
    let claude_found = claude_cli();
    let codex_found = codex_config();
    // A development build's skill may describe tools no release has, and
    // the skill folders are shared with the installed app.
    let skills = !crate::endpoint::is_dev_exe(exe);
    vec![
        Outcome {
            client: "Claude Code",
            skill: claude_found.as_ref().filter(|_| skills).map(|_| {
                if claude_plugin_installed() {
                    Ok(Skill::Plugin)
                } else {
                    write_skill(&claude_skill())
                }
            }),
            result: claude_found.map(|cli| claude(&cli, exe)),
        },
        Outcome {
            client: "Codex",
            skill: codex_found.as_ref().filter(|_| skills).map(|_| {
                // Codex reads ~/.agents/skills too, where the skills
                // installer puts it: a second copy would repeat it.
                let shared = home().join(".agents/skills/agent-screen/SKILL.md");
                if shared.exists() {
                    Ok(Skill::Linked(shared))
                } else {
                    write_skill(&codex_skill())
                }
            }),
            result: codex_found.map(|config| codex(&config, exe)),
        },
    ]
}

/// Bring the skill copies an earlier `install` wrote up to this version,
/// as the app updates itself. A client without a copy is left alone, and so
/// is a development build, whose skill may not be released yet. Returns the
/// copies rewritten.
pub fn refresh_skills(exe: &Path) -> Vec<PathBuf> {
    // Only the installed app: tests launch builds of a branch, some under
    // other names, and their skill may not be released.
    let installed =
        exe.starts_with("/Applications") || exe.starts_with(home().join("Applications"));
    if !installed
        || crate::endpoint::is_dev_exe(exe)
        || std::env::var_os("AGENTSCREEN_INSTANCE").is_some()
    {
        return Vec::new();
    }
    [claude_skill(), codex_skill()]
        .into_iter()
        .filter(|path| {
            std::fs::read_to_string(path).is_ok_and(|text| text != SKILL)
                && matches!(write_skill(path), Ok(Skill::Written(_)))
        })
        .collect()
}

fn claude_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
}

fn claude_skill() -> PathBuf {
    claude_home().join("skills/agent-screen/SKILL.md")
}

fn codex_skill() -> PathBuf {
    codex_home().join("skills/agent-screen/SKILL.md")
}

/// Whether Claude Code has the agent-screen plugin from this repository's
/// marketplace, which carries the skill: a second copy would only repeat it.
fn claude_plugin_installed() -> bool {
    let read = |p: &str| std::fs::read_to_string(claude_home().join(p)).unwrap_or_default();
    plugin_listed(
        &read("plugins/installed_plugins.json"),
        &read("settings.json"),
    )
}

/// Whether `installed` lists the agent-screen plugin for every project, and
/// the user's `settings` leave it on.
fn plugin_listed(installed: &str, settings: &str) -> bool {
    let settings = serde_json::from_str::<serde_json::Value>(settings).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(installed)
        .ok()
        .and_then(|v| v.get("plugins")?.as_object().cloned())
        .is_some_and(|plugins| {
            // A project's plugin carries the skill in that project only.
            plugins.iter().any(|(key, entries)| {
                key.starts_with("agent-screen@")
                    && settings["enabledPlugins"][key].as_bool() != Some(false)
                    && entries.as_array().is_some_and(|list| {
                        list.iter().any(|e| e["scope"].as_str() == Some("user"))
                    })
            })
        })
}

fn write_skill(path: &Path) -> Result<Skill, String> {
    // The skills installer links each agent's folder to its own copy.
    // The file, its folder, or the skills folder above it.
    let linked = path.ancestors().take(3).any(|p| {
        p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    });
    if linked {
        return Ok(Skill::Linked(path.to_path_buf()));
    }
    if std::fs::read_to_string(path).is_ok_and(|text| text == SKILL) {
        return Ok(Skill::Current(path.to_path_buf()));
    }
    let fail = |e: std::io::Error| format!("could not write {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    std::fs::write(path, SKILL).map_err(fail)?;
    Ok(Skill::Written(path.to_path_buf()))
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
        return Ok(Done::Already);
    }
    if current.status.success() {
        // Registered elsewhere, such as an older copy of the app.
        run(&["mcp", "remove", "--scope", "user", name])?;
    }
    let added = run(&["mcp", "add", "--scope", "user", name, "--", &exe, "mcp"])?;
    if added.status.success() {
        Ok(Done::Connected)
    } else {
        Err(String::from_utf8_lossy(&added.stderr).trim().to_string())
    }
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
}

fn codex_config() -> Option<PathBuf> {
    let dir = codex_home();
    dir.is_dir().then(|| dir.join("config.toml"))
}

fn codex(config: &Path, exe: &Path) -> Result<Done, String> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let (text, done) = with_codex_entry(&text, &exe.to_string_lossy())
        .map_err(|e| format!("could not update {}: {e}", config.display()))?;
    if !done {
        return Ok(Done::Already);
    }
    std::fs::write(config, text)
        .map_err(|e| format!("could not write {}: {e}", config.display()))?;
    Ok(Done::AddedTo(config.to_path_buf()))
}

/// Codex waits 60 s for a tool call by default, and wait_for_control can
/// wait 10 minutes for the user.
const TOOL_TIMEOUT: i64 = 700;
/// Codex waits 10 s for a server to start by default, and the pipe waits
/// up to 15 s for an app it starts (ADR 0021).
const STARTUP_TIMEOUT: i64 = 30;

/// The config with Agent Screen's entry running `exe`, and whether it
/// changed. Sets only command and args, and each timeout the entry lacks:
/// the user's own keys and subtables in it (env, approvals, enabled,
/// a timeout they chose), other entries and comments stay as they are.
/// A config that isn't valid TOML is left alone.
fn with_codex_entry(text: &str, exe: &str) -> Result<(String, bool), String> {
    use toml_edit::{Array, DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("it isn't valid TOML: {e}"))?;
    let name = server_name(exe);
    let servers = doc.entry("mcp_servers").or_insert_with(|| {
        // Only [mcp_servers.<name>] headers, as Codex's docs write them.
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    let servers = servers
        .as_table_like_mut()
        .ok_or("mcp_servers isn't a table")?;
    if !servers.contains_key(name) {
        let mut table = Table::new();
        if !text.trim().is_empty() {
            table.decor_mut().set_prefix("\n");
        }
        servers.insert(name, Item::Table(table));
    }
    let entry = servers
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| format!("mcp_servers.{name} isn't a table"))?;
    if entry.get("command").and_then(Item::as_str) != Some(exe) {
        entry.insert("command", value(exe));
    }
    let args = entry.get("args").and_then(Item::as_array);
    if args.is_none_or(|a| a.len() != 1 || a.get(0).and_then(|v| v.as_str()) != Some("mcp")) {
        entry.insert("args", value(Array::from_iter(["mcp"])));
    }
    if !entry.contains_key("tool_timeout_sec") {
        entry.insert("tool_timeout_sec", value(TOOL_TIMEOUT));
        if let Some(mut key) = entry.key_mut("tool_timeout_sec") {
            key.leaf_decor_mut()
                .set_prefix("# wait_for_control can wait up to 10 minutes for the user.\n");
        }
    }
    if !entry.contains_key("startup_timeout_sec") {
        entry.insert("startup_timeout_sec", value(STARTUP_TIMEOUT));
        if let Some(mut key) = entry.key_mut("startup_timeout_sec") {
            key.leaf_decor_mut()
                .set_prefix("# A cold start of the app can take 15 s.\n");
        }
    }
    let out = doc.to_string();
    let changed = out != text;
    Ok((out, changed))
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
        let (codex, _) = with_codex_entry("", EXE).unwrap();
        assert!(codex_runs(&codex, EXE));
        assert!(!codex_runs(&codex, "/Users/me/old/Agent Screen"));
        assert!(!codex_runs("", EXE));
    }

    #[test]
    fn the_development_build_registers_beside_the_installed_app() {
        let dev = "/Users/me/agent-screen/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";
        let (text, _) = with_codex_entry("", EXE).unwrap();
        let (both, changed) = with_codex_entry(&text, dev).unwrap();
        assert!(changed);
        assert!(
            both.contains("[mcp_servers.agent-screen]")
                && both.contains("[mcp_servers.agent-screen-dev]")
        );
        assert!(codex_runs(&both, EXE) && codex_runs(&both, dev));
    }

    #[test]
    fn finds_the_plugin_from_any_marketplace() {
        let installed =
            |key: &str| format!(r#"{{"version":2,"plugins":{{"{key}":[{{"scope":"user"}}]}}}}"#);
        assert!(plugin_listed(&installed("agent-screen@agent-screen"), ""));
        assert!(plugin_listed(&installed("agent-screen@my-fork"), "{}"));
        assert!(!plugin_listed(&installed("agent-screen-tools@other"), ""));
        assert!(!plugin_listed(
            r#"{"plugins":{"agent-screen@agent-screen":[{"scope":"project"}]}}"#,
            ""
        ));
        assert!(!plugin_listed("", ""));
        assert!(!plugin_listed(
            &installed("agent-screen@agent-screen"),
            r#"{"enabledPlugins":{"agent-screen@agent-screen":false}}"#
        ));
    }

    #[test]
    fn writes_the_skill_once_then_finds_it_current() {
        let dir = std::env::temp_dir().join(format!("agent-screen-skill-{}", std::process::id()));
        let path = dir.join("skills/agent-screen/SKILL.md");
        assert!(matches!(write_skill(&path), Ok(Skill::Written(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL);
        assert!(matches!(write_skill(&path), Ok(Skill::Current(_))));
        std::fs::write(&path, "an older skill").unwrap();
        assert!(matches!(write_skill(&path), Ok(Skill::Written(_))));
        // A folder linked by another installer keeps that installer's copy.
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("SKILL.md"), "theirs").unwrap();
        let linked = dir.join("linked/agent-screen");
        std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();
        assert!(matches!(
            write_skill(&linked.join("SKILL.md")),
            Ok(Skill::Linked(_))
        ));
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("SKILL.md")).unwrap(),
            "theirs"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn appends_to_a_config_without_it() {
        let (text, changed) = with_codex_entry("model = \"o3\"\n", EXE).unwrap();
        assert!(changed);
        assert_eq!(
            text,
            format!(
                "model = \"o3\"\n\n[mcp_servers.agent-screen]\ncommand = {EXE:?}\nargs = [\"mcp\"]\n# wait_for_control can wait up to 10 minutes for the user.\ntool_timeout_sec = 700\n# A cold start of the app can take 15 s.\nstartup_timeout_sec = 30\n"
            )
        );
    }

    #[test]
    fn leaves_a_correct_entry_alone() {
        let (once, _) = with_codex_entry("", EXE).unwrap();
        assert_eq!(with_codex_entry(&once, EXE), Ok((once.clone(), false)));
    }

    #[test]
    fn repoints_an_entry_and_keeps_the_users_settings() {
        let before = "[a]\nx = 1\n\n[mcp_servers.agent-screen]\ncommand = \"/old\"\nargs = [\"mcp\"]\nenabled = false\ntool_timeout_sec = 900\n\n[mcp_servers.agent-screen.env]\nA = \"1\"\n\n[mcp_servers.agent-screen.tools.computer]\napproval_mode = \"approve\"\n\n[b]\ny = 2\n";
        let (text, changed) = with_codex_entry(before, EXE).unwrap();
        assert!(changed);
        assert!(!text.contains("/old"));
        assert!(codex_runs(&text, EXE));
        for kept in [
            "[a]\nx = 1\n",
            "enabled = false\n",
            "tool_timeout_sec = 900\n",
            "[mcp_servers.agent-screen.env]\nA = \"1\"\n",
            "approval_mode = \"approve\"\n",
            "[b]\ny = 2\n",
        ] {
            assert!(text.contains(kept), "lost {kept:?} from:\n{text}");
        }
        assert!(!text.contains("tool_timeout_sec = 700"));
        assert!(text.contains("startup_timeout_sec = 30"));
    }

    #[test]
    fn adds_the_timeouts_an_older_entry_lacks() {
        let before = format!(
            "[mcp_servers.agent-screen]\ncommand = {EXE:?}\nargs = [\"mcp\"]\ntool_timeout_sec = 700\n"
        );
        assert!(codex_runs(&before, EXE), "it already runs this copy");
        let (text, changed) = with_codex_entry(&before, EXE).unwrap();
        assert!(changed);
        assert!(text.starts_with(&before) && text.ends_with("startup_timeout_sec = 30\n"));
    }

    #[test]
    fn finds_a_quoted_or_inline_entry_instead_of_adding_another() {
        for before in [
            "[mcp_servers.\"agent-screen\"]\ncommand = \"/old\"\n".to_string(),
            "[mcp_servers]\nagent-screen = { command = \"/old\", args = [\"mcp\"] }\n".to_string(),
        ] {
            let (text, changed) = with_codex_entry(&before, EXE).unwrap();
            assert!(changed);
            assert_eq!(text.matches("agent-screen").count(), 1, "{text}");
            assert!(codex_runs(&text, EXE), "{text}");
            assert!(text.parse::<toml_edit::DocumentMut>().is_ok());
        }
    }

    #[test]
    fn leaves_a_config_it_cannot_read_alone() {
        assert!(with_codex_entry("[mcp_servers\ncommand = ", EXE).is_err());
        assert!(!codex_runs("[mcp_servers\n", EXE));
    }
}
