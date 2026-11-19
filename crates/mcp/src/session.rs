//! Who is calling: enough to find or label the session's Agent Display.
//!
//! The app identifies the session from what the pipe saw in the client's
//! environment (ADR 0021), so nothing here reads this process's own.
//!
//! A session key comes from the first of these that names one:
//!
//! 1. The tool call's `_meta`: Codex names its session on every call in
//!    `x-codex-turn-metadata`, and one Codex process can serve several.
//! 2. A variable its client sets for stdio servers, trusted only from that
//!    client: `CLAUDE_CODE_SESSION_ID` when the client says it is Claude
//!    Code. A process Claude Code's shell starts, another client among
//!    them, inherits the variable and must not join Claude Code's session.
//! 3. The connection: the pipe's own id, the same for every call over one
//!    pipe and never another pipe's. A client with no hint, such as Cursor,
//!    Gemini CLI, OpenCode, Zed, VS Code or Claude Desktop, gets one session
//!    per process it starts.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

pub struct Session {
    /// Stable for one client session; the app keeps one display per key.
    pub key: String,
    /// Client name for the Display Label.
    pub client: String,
    /// Fallback task title: the project folder.
    pub folder: String,
}

/// What the process the client started saw: its environment, working
/// directory and pid. The pipe sends it in its hello; a v1 process reads its
/// own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientEnv {
    pub pid: u32,
    /// The pipe's id, random and fixed for its life, so a session keyed to
    /// the connection survives the pipe's reconnects and no other pipe,
    /// even one that later gets the same pid, shares it. `None` for a v1
    /// process, whose pid serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(default)]
    pub cwd: String,
    /// `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PROJECT_DIR`, when set.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

/// The variables a client sets for its stdio servers that name the session.
pub const SESSION_ENV: &[&str] = &["CLAUDE_CODE_SESSION_ID", "CLAUDE_PROJECT_DIR"];

/// The `clientInfo.name` Claude Code gives.
const CLAUDE_CODE: &str = "claude-code";

impl ClientEnv {
    /// This process's own.
    pub fn current() -> Self {
        Self {
            pid: std::process::id(),
            connection: None,
            cwd: std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
            env: SESSION_ENV
                .iter()
                .filter_map(|k| {
                    std::env::var(k)
                        .ok()
                        .filter(|v| !v.is_empty())
                        .map(|v| (k.to_string(), v))
                })
                .collect(),
        }
    }

    fn var(&self, key: &str) -> Option<&str> {
        self.env
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

/// A new pipe's id: its pid and 128 random bits.
pub fn new_connection_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    // Each RandomState has its own random keys from the OS.
    let random = |salt: u64| {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        hasher.finish()
    };
    format!(
        "{}-{:016x}{:016x}",
        std::process::id(),
        random(1),
        random(2)
    )
}

/// The session a tool call belongs to, by the precedence in this module's
/// documentation. `client_name` is the client's `clientInfo.name`, from
/// `initialize` or the call's own `_meta`.
pub fn identify(env: &ClientEnv, meta: &Map<String, Value>, client_name: Option<&str>) -> Session {
    if let Some(turn) = codex_turn(meta)
        && let Some(session_id) = turn
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    {
        let workspace = turn
            .get("workspaces")
            .and_then(Value::as_array)
            .and_then(|w| w.first())
            .and_then(Value::as_str);
        return Session {
            key: format!("codex:{session_id}"),
            client: "Codex".into(),
            folder: folder(workspace.unwrap_or(&env.cwd)),
        };
    }

    if client_name == Some(CLAUDE_CODE)
        && let Some(claude) = env.var("CLAUDE_CODE_SESSION_ID")
    {
        return Session {
            key: format!("claude-code:{claude}"),
            client: "Claude Code".into(),
            folder: folder(env.var("CLAUDE_PROJECT_DIR").unwrap_or(&env.cwd)),
        };
    }

    Session {
        key: match &env.connection {
            Some(id) => format!("pipe:{id}"),
            None => format!("pid:{}", env.pid),
        },
        client: pretty_client_name(client_name),
        folder: folder(&env.cwd),
    }
}

/// `_meta["x-codex-turn-metadata"]`, sent either as an object or as JSON text.
fn codex_turn(meta: &Map<String, Value>) -> Option<Map<String, Value>> {
    match meta.get("x-codex-turn-metadata")? {
        Value::String(text) => serde_json::from_str(text).ok(),
        Value::Object(object) => Some(object.clone()),
        _ => None,
    }
}

/// The last path component of `dir`.
fn folder(dir: &str) -> String {
    Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn pretty_client_name(name: Option<&str>) -> String {
    match name {
        None => "Agent".into(),
        Some("claude-code") => "Claude Code".into(),
        Some("codex-mcp-client") => "Codex".into(),
        Some(other) => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_metadata_as_text_or_object() {
        let as_text = json!({ "x-codex-turn-metadata": "{\"session_id\":\"abc\",\"workspaces\":[\"/w/proj\"]}" });
        let as_object =
            json!({ "x-codex-turn-metadata": { "session_id": "abc", "workspaces": ["/w/proj"] } });
        for meta in [as_text, as_object] {
            let turn = codex_turn(meta.as_object().unwrap()).unwrap();
            assert_eq!(turn["session_id"], "abc");
        }
    }

    #[test]
    fn client_names() {
        assert_eq!(pretty_client_name(Some("codex-mcp-client")), "Codex");
        assert_eq!(pretty_client_name(None), "Agent");
        assert_eq!(pretty_client_name(Some("zed")), "zed");
    }

    fn env(pairs: &[(&str, &str)]) -> ClientEnv {
        ClientEnv {
            pid: 4242,
            connection: Some("4242-abc".into()),
            cwd: "/Users/u/code/where-it-ran".into(),
            env: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    const CLAUDE: &[(&str, &str)] = &[
        ("CLAUDE_CODE_SESSION_ID", "s1"),
        ("CLAUDE_PROJECT_DIR", "/w/proj"),
    ];

    #[test]
    fn claude_code_is_named_by_its_variable_from_the_clients_environment() {
        let claude = identify(&env(CLAUDE), &Map::new(), Some("claude-code"));
        assert_eq!(claude.key, "claude-code:s1");
        assert_eq!(claude.client, "Claude Code");
        assert_eq!(claude.folder, "proj");
        // No project dir: the folder the client started the process in.
        let bare = identify(
            &env(&[("CLAUDE_CODE_SESSION_ID", "s1")]),
            &Map::new(),
            Some("claude-code"),
        );
        assert_eq!(bare.folder, "where-it-ran");
    }

    #[test]
    fn another_client_started_from_claude_codes_shell_keeps_its_own_session() {
        // Gemini CLI, or a client that gave no name, inherited the variable.
        for name in [Some("gemini-cli-mcp-client"), None] {
            let other = identify(&env(CLAUDE), &Map::new(), name);
            assert_eq!(other.key, "pipe:4242-abc", "{name:?}");
        }
    }

    #[test]
    fn a_calls_own_session_comes_first() {
        let meta =
            json!({ "x-codex-turn-metadata": { "session_id": "c9", "workspaces": ["/w/app"] } });
        // Even with Claude Code's variable inherited.
        let codex = identify(
            &env(CLAUDE),
            meta.as_object().unwrap(),
            Some("codex-mcp-client"),
        );
        assert_eq!(
            (
                codex.key.as_str(),
                codex.client.as_str(),
                codex.folder.as_str()
            ),
            ("codex:c9", "Codex", "app")
        );
        // An empty id names no session.
        let empty = json!({ "x-codex-turn-metadata": { "session_id": "" } });
        let fallback = identify(
            &env(&[]),
            empty.as_object().unwrap(),
            Some("codex-mcp-client"),
        );
        assert_eq!(fallback.key, "pipe:4242-abc");
    }

    #[test]
    fn a_client_with_no_hint_is_its_connection() {
        let zed = identify(&env(&[]), &Map::new(), Some("zed"));
        assert_eq!(
            (zed.key.as_str(), zed.client.as_str()),
            ("pipe:4242-abc", "zed")
        );
        let empty = identify(
            &env(&[("CLAUDE_CODE_SESSION_ID", "")]),
            &Map::new(),
            Some("claude-code"),
        );
        assert_eq!(empty.key, "pipe:4242-abc");
        // A v1 process has no pipe id: its pid.
        let mut v1 = env(&[]);
        v1.connection = None;
        assert_eq!(identify(&v1, &Map::new(), None).key, "pid:4242");
    }

    #[test]
    fn every_pipe_gets_its_own_id() {
        let ids: std::collections::HashSet<String> =
            (0..100).map(|_| new_connection_id()).collect();
        assert_eq!(ids.len(), 100);
    }
}
