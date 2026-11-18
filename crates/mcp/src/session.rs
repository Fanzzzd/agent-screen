//! Who is calling: enough to find or label the session's Agent Display.
//!
//! The app identifies the session from what the pipe saw in the client's
//! environment (ADR 0021), so nothing here reads this process's own.

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
    #[serde(default)]
    pub cwd: String,
    /// `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PROJECT_DIR`, when set.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

/// The variables a client sets for its stdio servers that name the session.
pub const SESSION_ENV: &[&str] = &["CLAUDE_CODE_SESSION_ID", "CLAUDE_PROJECT_DIR"];

impl ClientEnv {
    /// This process's own.
    pub fn current() -> Self {
        Self {
            pid: std::process::id(),
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

/// Claude Code passes its session ID to stdio servers as an environment
/// variable. Codex sends it with every tool call in `_meta`. Anything else is
/// keyed by the client's process, which lives exactly as long as the client
/// keeps it.
pub fn identify(env: &ClientEnv, meta: &Map<String, Value>, client_name: Option<&str>) -> Session {
    if let Some(claude) = env.var("CLAUDE_CODE_SESSION_ID") {
        return Session {
            key: format!("claude-code:{claude}"),
            client: "Claude Code".into(),
            folder: folder(env.var("CLAUDE_PROJECT_DIR").unwrap_or(&env.cwd)),
        };
    }

    if let Some(turn) = codex_turn(meta)
        && let Some(session_id) = turn.get("session_id").and_then(Value::as_str)
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

    Session {
        key: format!("pid:{}", env.pid),
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
            cwd: "/Users/u/code/where-it-ran".into(),
            env: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn the_session_comes_from_the_clients_environment_not_the_apps() {
        let claude = identify(
            &env(&[
                ("CLAUDE_CODE_SESSION_ID", "s1"),
                ("CLAUDE_PROJECT_DIR", "/w/proj"),
            ]),
            &Map::new(),
            Some("claude-code"),
        );
        assert_eq!(claude.key, "claude-code:s1");
        assert_eq!(claude.client, "Claude Code");
        assert_eq!(claude.folder, "proj");
        // No project dir: the folder the client started the process in.
        let bare = identify(&env(&[("CLAUDE_CODE_SESSION_ID", "s1")]), &Map::new(), None);
        assert_eq!(bare.folder, "where-it-ran");
    }

    #[test]
    fn codex_names_its_session_per_call_and_others_get_the_client_pid() {
        let meta =
            json!({ "x-codex-turn-metadata": { "session_id": "c9", "workspaces": ["/w/app"] } });
        let codex = identify(
            &env(&[]),
            meta.as_object().unwrap(),
            Some("codex-mcp-client"),
        );
        assert_eq!(
            (codex.key.as_str(), codex.folder.as_str()),
            ("codex:c9", "app")
        );
        let other = identify(&env(&[]), &Map::new(), Some("zed"));
        assert_eq!(other.key, "pid:4242");
        assert_eq!(other.client, "zed");
        // An empty variable names no session.
        let empty = identify(&env(&[("CLAUDE_CODE_SESSION_ID", "")]), &Map::new(), None);
        assert_eq!(empty.key, "pid:4242");
    }
}
