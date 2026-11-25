//! Register Agent Screen's MCP server with the agent clients on this Mac,
//! by editing each client's own config file. One table describes every
//! client: how to tell it is installed, where its config is, and what an
//! entry looks like. The setup window and `AgentScreen mcp` share it.
//!
//! Every edit keeps what the user wrote: other servers, the user's keys in
//! Agent Screen's entry, comments. A config that can't be read is never
//! written. The development build registers as `agent-screen-dev`, beside
//! the installed app.

pub mod cli;
mod codex;
mod fs;
mod goose;
mod json;
pub mod jsonc;
pub mod skills;

pub use skills::{SKILL, Skill};

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where a client's files are: the home folder and the variables that move
/// them. Tests give a temporary home and no variables.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    vars: Vec<(String, OsString)>,
    /// Folders that hold apps.
    pub app_dirs: Vec<PathBuf>,
    /// Folders to look for a client's command line in.
    pub bin_dirs: Vec<PathBuf>,
}

const VARS: [&str; 5] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_CONFIG_HOME",
    "COPILOT_HOME",
    "AGENTSCREEN_INSTANCE",
];

/// Where installers put a CLI, relative to home. An app opened from Finder
/// gets only /usr/bin:/bin:/usr/sbin:/sbin as PATH, so these are looked in
/// too, without asking a login shell.
const HOME_BINS: [&str; 7] = [
    ".local/bin",
    ".claude/local",
    ".npm-global/bin",
    ".bun/bin",
    ".opencode/bin",
    ".cargo/bin",
    ".volta/bin",
];

impl Env {
    /// This process's home and variables.
    pub fn current() -> Env {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let vars = VARS
            .iter()
            .filter_map(|k| Some((k.to_string(), std::env::var_os(k)?)))
            .collect();
        let mut bin_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        bin_dirs.extend(HOME_BINS.iter().map(|d| home.join(d)));
        bin_dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        Env {
            app_dirs: vec![PathBuf::from("/Applications"), home.join("Applications")],
            bin_dirs,
            vars,
            home,
        }
    }

    /// Only `home`: apps in its Applications folder, commands in its usual
    /// folders, no variables.
    pub fn for_home(home: impl Into<PathBuf>) -> Env {
        let home = home.into();
        Env {
            app_dirs: vec![home.join("Applications")],
            bin_dirs: HOME_BINS.iter().map(|d| home.join(d)).collect(),
            vars: Vec::new(),
            home,
        }
    }

    pub fn with_var(mut self, key: &str, value: impl Into<OsString>) -> Env {
        self.vars.retain(|(k, _)| k != key);
        self.vars.push((key.to_string(), value.into()));
        self
    }

    fn var(&self, key: &str) -> Option<&OsString> {
        self.vars
            .iter()
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v)
    }

    fn claude_home(&self) -> PathBuf {
        self.var("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".claude"))
    }

    fn codex_home(&self) -> PathBuf {
        self.var("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".codex"))
    }

    fn config_home(&self) -> PathBuf {
        self.var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".config"))
    }

    fn app_support(&self) -> PathBuf {
        self.home.join("Library/Application Support")
    }

    fn has_app(&self, bundle: &str) -> bool {
        self.app_dirs.iter().any(|d| d.join(bundle).is_dir())
    }

    fn has_cli(&self, name: &str) -> bool {
        self.bin_dirs.iter().any(|d| d.join(name).is_file())
    }
}

enum Format {
    Json(&'static json::Shape),
    Toml,
    Yaml,
}

/// An agent client Agent Screen can register with.
pub struct Client {
    /// The stable name `--client` takes.
    pub id: &'static str,
    /// Other names `--client` takes for it.
    pub aliases: &'static [&'static str],
    pub name: &'static str,
    /// It loads servers only when it starts: quit it and open it again.
    pub restart: bool,
    format: Format,
    config: fn(&Env) -> PathBuf,
    /// App bundles, any of which shows it is installed.
    apps: &'static [&'static str],
    /// Command lines, any of which shows it is installed.
    clis: &'static [&'static str],
    /// Files or folders, any of which shows it is installed.
    marks: fn(&Env) -> Vec<PathBuf>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

impl PartialEq for Client {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Client {
    pub fn config(&self, env: &Env) -> PathBuf {
        (self.config)(env)
    }

    /// Whether this Mac has the client: its app, its command line, its
    /// config or its folder.
    pub fn detected(&self, env: &Env) -> bool {
        self.apps.iter().any(|a| env.has_app(a))
            || self.clis.iter().any(|c| env.has_cli(c))
            || self.config(env).exists()
            || (self.marks)(env).iter().any(|p| p.exists())
    }

    fn state_of(&self, text: &str, exe: &str) -> State {
        match self.format {
            Format::Json(shape) => json::state(shape, text, exe),
            Format::Toml => codex::state(text, exe),
            Format::Yaml => goose::state(text, exe),
        }
    }

    fn with_entry(&self, text: &str, exe: &str) -> Result<String, String> {
        match self.format {
            Format::Json(shape) => json::with_entry(shape, text, exe),
            Format::Toml => codex::with_entry(text, exe),
            Format::Yaml => goose::with_entry(text, exe),
        }
    }

    fn without_entry(&self, text: &str, exe: &str) -> Result<String, String> {
        match self.format {
            Format::Json(shape) => json::without_entry(shape, text, exe),
            Format::Toml => codex::without_entry(text, exe),
            Format::Yaml => goose::without_entry(text, exe),
        }
    }
}

/// VS Code's extension storage, where Cline and Roo Code keep settings.
fn vscode_storage(env: &Env, extension: &str) -> PathBuf {
    env.app_support()
        .join("Code/User/globalStorage")
        .join(extension)
}

/// Antigravity 2 keeps its config, shared by the app and `agy`, in
/// ~/.gemini/config. Antigravity 1 kept it in ~/.gemini/antigravity, and
/// still does where there is no ~/.gemini/config.
fn antigravity_config(env: &Env) -> PathBuf {
    let shared = env.home.join(".gemini/config");
    let legacy = env.home.join(".gemini/antigravity/mcp_config.json");
    if !shared.is_dir() && legacy.is_file() {
        legacy
    } else {
        shared.join("mcp_config.json")
    }
}

/// OpenCode reads opencode.jsonc when it is there, else opencode.json.
fn opencode_config(env: &Env) -> PathBuf {
    let dir = env.config_home().join("opencode");
    let jsonc = dir.join("opencode.jsonc");
    if jsonc.is_file() {
        jsonc
    } else {
        dir.join("opencode.json")
    }
}

/// The clients, in the order the setup window and `mcp status` list them.
pub static CLIENTS: &[Client] = &[
    Client {
        id: "claude-code",
        aliases: &["claude"],
        name: "Claude Code",
        restart: false,
        format: Format::Json(&json::CLAUDE_CODE),
        config: |env| match env.var("CLAUDE_CONFIG_DIR") {
            Some(dir) => PathBuf::from(dir).join(".claude.json"),
            None => env.home.join(".claude.json"),
        },
        apps: &[],
        clis: &["claude"],
        marks: |env| vec![env.claude_home()],
    },
    Client {
        id: "codex",
        aliases: &[],
        name: "Codex",
        restart: false,
        format: Format::Toml,
        config: |env| env.codex_home().join("config.toml"),
        apps: &["Codex.app"],
        clis: &["codex"],
        marks: |env| vec![env.codex_home()],
    },
    Client {
        id: "cursor",
        aliases: &[],
        name: "Cursor",
        restart: false,
        format: Format::Json(&json::MCP_SERVERS),
        config: |env| env.home.join(".cursor/mcp.json"),
        apps: &["Cursor.app"],
        clis: &["cursor-agent"],
        marks: |env| vec![env.home.join(".cursor")],
    },
    Client {
        id: "claude-desktop",
        aliases: &[],
        name: "Claude Desktop",
        restart: true,
        format: Format::Json(&json::MCP_SERVERS),
        config: |env| env.app_support().join("Claude/claude_desktop_config.json"),
        apps: &["Claude.app"],
        clis: &[],
        marks: |_| Vec::new(),
    },
    Client {
        id: "antigravity",
        aliases: &["antigravity-ide", "antigravity-cli", "agy"],
        name: "Antigravity",
        restart: false,
        format: Format::Json(&json::MCP_SERVERS),
        config: antigravity_config,
        apps: &["Antigravity.app"],
        clis: &["agy"],
        marks: |env| {
            vec![
                env.home.join(".gemini/antigravity"),
                env.home.join(".gemini/antigravity-cli"),
            ]
        },
    },
    Client {
        id: "gemini-cli",
        aliases: &["gemini"],
        name: "Gemini CLI",
        restart: false,
        format: Format::Json(&json::GEMINI),
        config: |env| env.home.join(".gemini/settings.json"),
        apps: &[],
        clis: &["gemini"],
        marks: |_| Vec::new(),
    },
    Client {
        id: "opencode",
        aliases: &[],
        name: "OpenCode",
        restart: false,
        format: Format::Json(&json::OPENCODE),
        config: opencode_config,
        apps: &["OpenCode.app"],
        clis: &["opencode"],
        marks: |env| vec![env.config_home().join("opencode")],
    },
    Client {
        id: "vscode",
        aliases: &["vscode-insiders", "copilot"],
        name: "VS Code and Copilot CLI",
        restart: false,
        format: Format::Json(&json::COPILOT),
        config: |env| {
            env.var("COPILOT_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| env.home.join(".copilot"))
                .join("mcp-config.json")
        },
        apps: &[
            "Visual Studio Code.app",
            "Visual Studio Code - Insiders.app",
        ],
        clis: &["copilot"],
        marks: |env| vec![env.home.join(".copilot")],
    },
    Client {
        id: "windsurf",
        aliases: &[],
        name: "Windsurf",
        restart: false,
        format: Format::Json(&json::MCP_SERVERS),
        config: |env| env.home.join(".codeium/windsurf/mcp_config.json"),
        apps: &["Windsurf.app"],
        clis: &[],
        marks: |env| vec![env.home.join(".codeium/windsurf")],
    },
    Client {
        id: "devin",
        aliases: &[],
        name: "Devin",
        restart: false,
        format: Format::Json(&json::MCP_SERVERS),
        config: |env| env.home.join(".config/devin/mcp_config.json"),
        apps: &["Devin.app"],
        clis: &["devin"],
        marks: |env| vec![env.home.join(".config/devin")],
    },
    Client {
        id: "zed",
        aliases: &[],
        name: "Zed",
        restart: false,
        format: Format::Json(&json::ZED),
        config: |env| env.home.join(".config/zed/settings.json"),
        apps: &["Zed.app", "Zed Preview.app"],
        clis: &[],
        marks: |env| vec![env.home.join(".config/zed")],
    },
    Client {
        id: "cline",
        aliases: &[],
        name: "Cline",
        restart: false,
        format: Format::Json(&json::SECONDS_TIMEOUT),
        config: |env| {
            vscode_storage(env, "saoudrizwan.claude-dev").join("settings/cline_mcp_settings.json")
        },
        apps: &[],
        clis: &[],
        marks: |env| vec![vscode_storage(env, "saoudrizwan.claude-dev")],
    },
    Client {
        id: "roo-code",
        aliases: &["roo"],
        name: "Roo Code",
        restart: false,
        format: Format::Json(&json::SECONDS_TIMEOUT),
        config: |env| {
            vscode_storage(env, "rooveterinaryinc.roo-cline").join("settings/mcp_settings.json")
        },
        apps: &[],
        clis: &[],
        marks: |env| vec![vscode_storage(env, "rooveterinaryinc.roo-cline")],
    },
    Client {
        id: "goose",
        aliases: &[],
        name: "Goose",
        restart: false,
        format: Format::Yaml,
        config: |env| env.config_home().join("goose/config.yaml"),
        apps: &["Goose.app"],
        clis: &["goose"],
        marks: |env| vec![env.config_home().join("goose")],
    },
];

/// The client `--client` names, by id or alias.
pub fn client(id: &str) -> Option<&'static Client> {
    CLIENTS
        .iter()
        .find(|c| c.id == id || c.aliases.contains(&id))
}

/// The client whose MCP sessions introduce themselves as `info`, the
/// `clientInfo.name` from `initialize` as the app hears it (Claude Code
/// and Codex already given their product names). Matched by the words in
/// it, so "claude-ai" is Claude Desktop and "Visual Studio Code" is VS
/// Code; None when no client's words are there.
pub fn client_for_info(info: &str) -> Option<&'static Client> {
    let squashed: String = info
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let has = |word: &str| squashed.contains(word);
    let id = if has("claudecode") {
        "claude-code"
    } else if has("claudeai") || has("claudedesktop") || squashed == "claude" {
        "claude-desktop"
    } else if has("codex") {
        "codex"
    } else if has("cursor") {
        "cursor"
    } else if has("antigravity") || squashed == "agy" {
        "antigravity"
    } else if has("gemini") {
        "gemini-cli"
    } else if has("opencode") {
        "opencode"
    } else if has("visualstudiocode") || has("vscode") || has("copilot") {
        "vscode"
    } else if has("windsurf") || has("codeium") {
        "windsurf"
    } else if has("devin") {
        "devin"
    } else if has("zed") {
        "zed"
    } else if has("roo") {
        "roo-code"
    } else if has("cline") {
        "cline"
    } else if has("goose") {
        "goose"
    } else {
        return None;
    };
    client(id)
}

/// What to paste for a client Agent Screen doesn't know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snippet {
    /// A JSON config with `mcpServers`, as most clients take.
    Json,
    /// OpenCode's JSON.
    OpenCode,
    /// Codex's TOML.
    Toml,
    /// The command line that starts the server.
    Command,
}

/// `kind` for this copy at `exe`: the entry the registry writes, in a
/// config of its own.
pub fn snippet(kind: Snippet, exe: &Path) -> String {
    let exe_text = exe.to_string_lossy();
    let from = |id: &str| {
        client(id)
            .and_then(|c| c.with_entry("", &exe_text).ok())
            .unwrap_or_default()
    };
    match kind {
        Snippet::Json => from("cursor"),
        Snippet::OpenCode => from("opencode"),
        Snippet::Toml => from("codex"),
        Snippet::Command => {
            let quoted = format!("'{}'", exe_text.replace('\'', r"'\''"));
            format!("{quoted} mcp\n")
        }
    }
}

/// The server's name in the clients' configs: the development build
/// registers beside the installed app, not over it.
pub fn server_name(exe: &str) -> &'static str {
    if crate::endpoint::is_dev_exe(Path::new(exe)) {
        "agent-screen-dev"
    } else {
        "agent-screen"
    }
}

/// What a client's config says about this copy of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// The entry runs this copy.
    Connected,
    /// The entry runs another copy: moved, deleted, or on a disk image.
    StalePath(PathBuf),
    NotConnected,
    /// The file is there but can't be read; it is never written.
    InvalidConfig(String),
}

impl State {
    pub fn key(&self) -> &'static str {
        match self {
            State::Connected => "connected",
            State::StalePath(_) => "stale",
            State::NotConnected => "not_connected",
            State::InvalidConfig(_) => "invalid",
        }
    }
}

pub struct Status {
    pub client: &'static Client,
    pub detected: bool,
    pub config: PathBuf,
    pub state: State,
}

fn read_state(client: &Client, env: &Env, exe: &str) -> (PathBuf, State) {
    let config = client.config(env);
    let state = match fs::read(&config) {
        Ok(None) => State::NotConnected,
        Ok(Some(text)) => client.state_of(&text, exe),
        Err(e) => State::InvalidConfig(e),
    };
    (config, state)
}

/// Every client, whether this Mac has it, and what its config says.
pub fn status(exe: &Path, env: &Env) -> Vec<Status> {
    let exe = exe.to_string_lossy();
    CLIENTS
        .iter()
        .map(|client| {
            let (config, state) = read_state(client, env, &exe);
            Status {
                client,
                detected: client.detected(env),
                config,
                state,
            }
        })
        .collect()
}

/// The names of the clients whose config runs `exe`. An entry for another
/// copy, moved or deleted since, doesn't count.
pub fn registered(exe: &Path) -> Vec<&'static str> {
    status(exe, &Env::current())
        .into_iter()
        .filter(|s| s.state == State::Connected)
        .map(|s| s.client.name)
        .collect()
}

/// What registering did, for the caller to put in the user's language.
#[derive(Debug, PartialEq)]
pub enum Done {
    Already,
    /// A new entry, in this config file.
    AddedTo(PathBuf),
    /// The entry ran another copy, at `from`; now this one.
    Repointed {
        from: PathBuf,
        config: PathBuf,
    },
    /// The entry ran this copy; keys it lacked were added.
    Updated(PathBuf),
}

impl std::fmt::Display for Done {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Done::Already => write!(f, "already connected"),
            Done::AddedTo(path) => write!(f, "added to {}", path.display()),
            Done::Repointed { from, config } => write!(
                f,
                "repointed from {} in {}",
                from.display(),
                config.display()
            ),
            Done::Updated(path) => write!(f, "updated in {}", path.display()),
        }
    }
}

pub struct Outcome {
    pub client: &'static Client,
    pub config: PathBuf,
    pub result: Result<Done, String>,
    /// The skill for this client, when it has a folder of its own.
    pub skill: Option<Result<Skill, String>>,
}

pub struct Options {
    /// Only these; every client this Mac has when None.
    pub clients: Option<Vec<&'static Client>>,
    pub skill: bool,
    /// Say what would change, and change nothing.
    pub dry_run: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            clients: None,
            skill: true,
            dry_run: false,
        }
    }
}

pub struct Report {
    pub outcomes: Vec<Outcome>,
    /// The copy in ~/.agents/skills, which most agents read.
    pub shared_skill: Option<Result<Skill, String>>,
    /// The clients this Mac doesn't have, when none were named.
    pub missing: Vec<&'static Client>,
}

/// Edit `client`'s config with `edit`, reading it again when the client
/// saved it meanwhile.
fn edit(
    client: &Client,
    env: &Env,
    dry_run: bool,
    edit: impl Fn(&str) -> Result<String, String>,
) -> Result<bool, String> {
    let config = client.config(env);
    for _ in 0..3 {
        let before = fs::read(&config)?;
        let text = before.as_deref().unwrap_or_default();
        if let Some(e) = before
            .as_deref()
            .and_then(|t| match client.state_of(t, "") {
                State::InvalidConfig(e) => Some(e),
                _ => None,
            })
        {
            return Err(format!("{} was left as it is: {e}", config.display()));
        }
        let after =
            edit(text).map_err(|e| format!("{} was left as it is: {e}", config.display()))?;
        if after == text {
            return Ok(false);
        }
        if dry_run {
            return Ok(true);
        }
        match fs::write(&config, before.as_deref(), &after)? {
            fs::Written::Done => return Ok(true),
            fs::Written::Changed => continue,
        }
    }
    Err(format!(
        "{} kept changing while Agent Screen wrote it; try again",
        config.display()
    ))
}

/// Register `exe mcp` with every client this Mac has, or the ones named.
/// Safe to run again: an entry pointing elsewhere is repointed, one
/// already right is left alone.
pub fn install(exe: &Path, env: &Env, options: &Options) -> Report {
    let exe_text = exe.to_string_lossy();
    let (targets, missing): (Vec<&'static Client>, Vec<&'static Client>) = match &options.clients {
        Some(named) => (named.clone(), Vec::new()),
        None => CLIENTS.iter().partition(|c| c.detected(env)),
    };
    // A development build's skill may describe tools no release has, and
    // the skill folders are shared with the installed app.
    let skills = options.skill && !options.dry_run && !crate::endpoint::is_dev_exe(exe);
    let shared_skill = (skills && !targets.is_empty()).then(|| skills::write_shared(env));
    let outcomes = targets
        .into_iter()
        .map(|client| {
            let (config, before) = read_state(client, env, &exe_text);
            let result = edit(client, env, options.dry_run, |text| {
                client.with_entry(text, &exe_text)
            })
            .map(|changed| match (changed, before) {
                (false, _) => Done::Already,
                (true, State::StalePath(from)) => Done::Repointed {
                    from,
                    config: config.clone(),
                },
                (true, State::Connected) => Done::Updated(config.clone()),
                (true, _) => Done::AddedTo(config.clone()),
            });
            let skill = skills
                .then(|| match client.id {
                    "claude-code" => Some(skills::write_claude(env)),
                    "codex" => Some(skills::write_codex(env)),
                    _ => None,
                })
                .flatten();
            Outcome {
                client,
                config,
                result,
                skill,
            }
        })
        .collect();
    Report {
        outcomes,
        shared_skill,
        missing,
    }
}

/// What removing did.
#[derive(Debug, PartialEq)]
pub enum Removed {
    /// Taken out of this config file.
    From(PathBuf),
    NotThere,
}

pub struct Removal {
    pub client: &'static Client,
    pub config: PathBuf,
    pub result: Result<Removed, String>,
}

/// Take `exe`'s entry out of the named clients, or out of every client
/// whose config has it. Other servers and settings stay.
pub fn uninstall(
    exe: &Path,
    env: &Env,
    clients: Option<&[&'static Client]>,
    dry_run: bool,
) -> Vec<Removal> {
    let exe_text = exe.to_string_lossy();
    let targets: Vec<&'static Client> = match clients {
        Some(named) => named.to_vec(),
        None => CLIENTS
            .iter()
            .filter(|c| {
                matches!(
                    read_state(c, env, &exe_text).1,
                    State::Connected | State::StalePath(_)
                )
            })
            .collect(),
    };
    targets
        .into_iter()
        .map(|client| {
            let config = client.config(env);
            let result = edit(client, env, dry_run, |text| {
                client.without_entry(text, &exe_text)
            })
            .map(|changed| {
                if changed {
                    Removed::From(config.clone())
                } else {
                    Removed::NotThere
                }
            });
            Removal {
                client,
                config,
                result,
            }
        })
        .collect()
}

/// Bring the skill copies an earlier `install` wrote up to this version,
/// as the app updates itself. A client without a copy is left alone, and so
/// is a development build, whose skill may not be released yet. Returns the
/// copies rewritten.
pub fn refresh_skills(exe: &Path) -> Vec<PathBuf> {
    refresh_skills_in(exe, &Env::current())
}

fn refresh_skills_in(exe: &Path, env: &Env) -> Vec<PathBuf> {
    // Only the installed app: tests launch builds of a branch, some under
    // other names, and their skill may not be released.
    let installed =
        exe.starts_with("/Applications") || exe.starts_with(env.home.join("Applications"));
    if !installed || crate::endpoint::is_dev_exe(exe) || env.var("AGENTSCREEN_INSTANCE").is_some() {
        return Vec::new();
    }
    skills::refresh(env)
}

#[cfg(test)]
mod tests;
