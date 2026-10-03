//! Who is calling: enough to find or label the session's Agent Display.

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

/// Claude Code passes its session ID to stdio servers as an environment
/// variable. Codex sends it with every tool call in `_meta`. Anything else is
/// keyed by this process, which lives exactly as long as the client keeps us.
pub fn identify(meta: &Map<String, Value>, client_name: Option<&str>) -> Session {
    if let Some(claude) = std::env::var("CLAUDE_CODE_SESSION_ID").ok().filter(|s| !s.is_empty()) {
        let dir = std::env::var("CLAUDE_PROJECT_DIR").ok();
        return Session {
            key: format!("claude-code:{claude}"),
            client: "Claude Code".into(),
            folder: folder(dir.as_deref()),
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
            folder: folder(workspace),
        };
    }

    Session {
        key: format!("pid:{}", std::process::id()),
        client: pretty_client_name(client_name),
        folder: folder(None),
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

/// The last path component of `dir`, or of the working directory.
fn folder(dir: Option<&str>) -> String {
    let path = match dir {
        Some(dir) => Path::new(dir).to_path_buf(),
        None => std::env::current_dir().unwrap_or_default(),
    };
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn pretty_client_name(name: Option<&str>) -> String {
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
        let as_object = json!({ "x-codex-turn-metadata": { "session_id": "abc", "workspaces": ["/w/proj"] } });
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
}
