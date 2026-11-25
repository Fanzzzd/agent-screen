//! The clients whose config is JSON or JSONC: where each keeps its servers
//! and what an entry holds. Edits go through `jsonc`, so a file keeps its
//! comments and layout.

use super::jsonc::{self, Node, Out};
use super::{State, server_name};
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct Shape {
    /// Where the client reads entries, as keys from the top. An entry found
    /// in any is ours to repoint or remove.
    pub roots: &'static [&'static [&'static str]],
    /// The root a new entry goes in, by what the file already holds.
    pub preferred: fn(&Value) -> usize,
    /// The entry's keys for the root it is in.
    pub entry: fn(&str, usize) -> Vec<Field>,
}

pub struct Field {
    pub key: &'static str,
    pub value: Value,
    /// Ours: set to this value every time. Otherwise the user's: added when
    /// missing and left as the user set it.
    pub ours: bool,
}

pub fn ours(key: &'static str, value: Value) -> Field {
    Field {
        key,
        value,
        ours: true,
    }
}

pub fn default(key: &'static str, value: Value) -> Field {
    Field {
        key,
        value,
        ours: false,
    }
}

fn first(_: &Value) -> usize {
    0
}

/// Claude Code's ~/.claude.json, as `claude mcp add --scope user` writes it.
pub const CLAUDE_CODE: Shape = Shape {
    roots: &[&["mcpServers"]],
    preferred: first,
    entry: |exe, _| {
        vec![
            ours("type", json!("stdio")),
            ours("command", json!(exe)),
            ours("args", json!(["mcp"])),
        ]
    },
};

/// `mcpServers` with command and args: Cursor, Claude Desktop, Antigravity,
/// Windsurf, Devin.
pub const MCP_SERVERS: Shape = Shape {
    roots: &[&["mcpServers"]],
    preferred: first,
    entry: |exe, _| vec![ours("command", json!(exe)), ours("args", json!(["mcp"]))],
};

/// Gemini CLI stops a call after 600,000 ms by default, the longest wait.
pub const GEMINI: Shape = Shape {
    roots: &[&["mcpServers"]],
    preferred: first,
    entry: |exe, _| {
        vec![
            ours("command", json!(exe)),
            ours("args", json!(["mcp"])),
            default("timeout", json!(700_000)),
        ]
    },
};

/// Cline and Roo Code stop a call after 60 s by default; timeout is in
/// seconds.
pub const SECONDS_TIMEOUT: Shape = Shape {
    roots: &[&["mcpServers"]],
    preferred: first,
    entry: |exe, _| {
        vec![
            ours("command", json!(exe)),
            ours("args", json!(["mcp"])),
            default("timeout", json!(700)),
        ]
    },
};

/// GitHub Copilot's own file, which VS Code and Copilot CLI read.
pub const COPILOT: Shape = Shape {
    roots: &[&["mcpServers"]],
    preferred: first,
    entry: |exe, _| {
        vec![
            ours("type", json!("stdio")),
            ours("command", json!(exe)),
            ours("args", json!(["mcp"])),
            default("tools", json!(["*"])),
        ]
    },
};

/// Zed's settings.json: `context_servers`, command and args side by side.
pub const ZED: Shape = Shape {
    roots: &[&["context_servers"]],
    preferred: first,
    entry: |exe, _| {
        vec![
            ours("command", json!(exe)),
            ours("args", json!(["mcp"])),
            default("env", json!({})),
        ]
    },
};

/// OpenCode 2 reads `mcp.servers`; earlier versions read servers straight
/// under `mcp`, which OpenCode 2 still accepts. A file already written the
/// earlier way gets the entry that way too.
pub const OPENCODE: Shape = Shape {
    roots: &[&["mcp", "servers"], &["mcp"]],
    preferred: |doc| {
        let mcp = &doc["mcp"];
        let legacy = mcp
            .as_object()
            .is_some_and(|m| m.keys().any(|k| k != "servers" && k != "timeout"));
        if mcp["servers"].is_object() || !legacy {
            0
        } else {
            1
        }
    },
    entry: |exe, root| {
        let mut fields = vec![
            ours("type", json!("local")),
            ours("command", json!([exe, "mcp"])),
        ];
        if root == 1 {
            fields.push(default("enabled", json!(true)));
        }
        fields
    },
};

/// The command an entry runs, written as a string, the first item of an
/// array (OpenCode), or `path` in an object (Zed before 2025).
fn command(entry: &Value) -> Option<&str> {
    let command = &entry["command"];
    command
        .as_str()
        .or_else(|| command.get(0).and_then(Value::as_str))
        .or_else(|| command["path"].as_str())
}

fn at<'a>(node: &'a Node, path: &[&str]) -> Option<&'a Node> {
    path.iter()
        .try_fold(node, |node, key| Some(&node.get(key)?.value))
}

pub fn state(shape: &Shape, text: &str, exe: &str) -> State {
    let doc = match jsonc::read(text) {
        Ok(doc) => doc,
        Err(e) => return State::InvalidConfig(format!("it isn't valid JSON: {e}")),
    };
    if !doc.is_object() && !doc.is_null() {
        return State::InvalidConfig("it doesn't hold a JSON object".into());
    }
    let name = server_name(exe);
    let found = shape.roots.iter().find_map(|root| {
        let servers = root.iter().try_fold(&doc, |v, k| v.get(k))?;
        servers.get(name)
    });
    match found.map(command) {
        None => State::NotConnected,
        Some(Some(command)) if command == exe => State::Connected,
        Some(command) => State::StalePath(PathBuf::from(command.unwrap_or_default())),
    }
}

fn parsed(text: &str) -> Result<Option<Node>, String> {
    let node = jsonc::parse(text).map_err(|e| format!("it isn't valid JSON: {e}"))?;
    match &node {
        Some(node) if node.members().is_none() => Err("it doesn't hold a JSON object".into()),
        _ => Ok(node),
    }
}

/// `text` with the entry running `exe`; the same text when it already does.
pub fn with_entry(shape: &Shape, text: &str, exe: &str) -> Result<String, String> {
    let name = server_name(exe);
    let Some(doc) = parsed(text)? else {
        let root = (shape.preferred)(&Value::Null);
        let entry = to_out(&(shape.entry)(exe, root));
        let fresh = jsonc::fresh(&nest(shape.roots[root], name, entry));
        // Only comments: they stay, above the new settings.
        return Ok(if text.trim().is_empty() {
            fresh
        } else {
            let sep = if text.ends_with('\n') { "" } else { "\n" };
            format!("{text}{sep}{fresh}")
        });
    };
    let root = shape
        .roots
        .iter()
        .position(|r| at(&doc, r).and_then(|s| s.get(name)).is_some())
        .unwrap_or_else(|| (shape.preferred)(&doc.to_value()));
    let path = shape.roots[root];
    let mut fields = (shape.entry)(exe, root);
    // The deepest object on the path that exists gets what is missing.
    let mut node = &doc;
    for (depth, key) in path.iter().enumerate() {
        match node.get(key) {
            None => {
                let rest = nest(&path[depth + 1..], name, to_out(&fields));
                return Ok(jsonc::insert(text, node, key, &rest));
            }
            Some(member) if member.value.members().is_none() => {
                return Err(format!("{} isn't an object", path[..=depth].join(".")));
            }
            Some(member) => node = &member.value,
        }
    }
    let existing = match node.get(name) {
        None => return Ok(jsonc::insert(text, node, name, &to_out(&fields))),
        Some(member) if member.value.members().is_none() => {
            return Err(format!("{}.{name} isn't an object", path.join(".")));
        }
        Some(member) => member.value.to_value(),
    };
    let mut target: Vec<&str> = path.iter().copied().chain([name]).collect();
    // Zed before 2025 wrote `"command": {"path", "args", "env"}`. Its env
    // moves beside the flat command; a key the flat form has no place for
    // keeps the object form, with this copy's path and args in it.
    if let Some(Value::Object(command)) = existing.get("command") {
        if command
            .keys()
            .any(|k| !matches!(k.as_str(), "path" | "args" | "env"))
        {
            target.push("command");
            fields = vec![ours("path", json!(exe)), ours("args", json!(["mcp"]))];
        } else if let Some(env) = command
            .get("env")
            .filter(|e| e.as_object().is_some_and(|e| !e.is_empty()))
            && existing
                .get("env")
                .is_none_or(|e| e.as_object().is_some_and(|e| e.is_empty()))
        {
            for field in fields.iter_mut().filter(|f| f.key == "env") {
                *field = ours("env", env.clone());
            }
        }
    }
    set_fields(text, &target, &fields)
}

/// `text` with `fields` set in the object at `target`, which is there.
fn set_fields(text: &str, target: &[&str], fields: &[Field]) -> Result<String, String> {
    let mut text = text.to_string();
    for field in fields {
        let doc = parsed(&text)?.expect("an object was here");
        let entry = at(&doc, target).expect("the entry was here");
        text = match entry.get(field.key) {
            None => jsonc::insert(&text, entry, field.key, &Out::Json(field.value.clone())),
            Some(member) if field.ours && member.value.to_value() != field.value => {
                jsonc::replace(&text, member, &Out::Json(field.value.clone()))
            }
            Some(_) => continue,
        };
    }
    Ok(text)
}

/// `text` without the entry `exe` would have, wherever the client reads it.
pub fn without_entry(shape: &Shape, text: &str, exe: &str) -> Result<String, String> {
    let name = server_name(exe);
    let mut text = text.to_string();
    for root in shape.roots {
        let Some(doc) = parsed(&text)? else {
            break;
        };
        let Some(servers) = at(&doc, root) else {
            continue;
        };
        let members = servers.members().unwrap_or_default();
        if let Some(index) = members.iter().rposition(|m| m.key == name) {
            text = jsonc::remove(&text, servers, index);
        }
    }
    Ok(text)
}

fn to_out(fields: &[Field]) -> Out {
    Out::Object(
        fields
            .iter()
            .map(|f| (f.key.to_string(), Out::Json(f.value.clone())))
            .collect(),
    )
}

/// `entry` under `name` inside objects named by `path`.
fn nest(path: &[&str], name: &str, entry: Out) -> Out {
    path.iter().rev().fold(
        Out::Object(vec![(name.to_string(), entry)]),
        |inner, key| Out::Object(vec![(key.to_string(), inner)]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen";
    const DEV: &str =
        "/Users/me/agent-screen/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";
    const OLD: &str = "/Volumes/Agent Screen/Agent Screen.app/Contents/MacOS/AgentScreen";

    fn value(text: &str) -> Value {
        jsonc::read(text).unwrap()
    }

    #[test]
    fn writes_a_new_file() {
        let text = with_entry(&MCP_SERVERS, "", EXE).unwrap();
        assert_eq!(
            value(&text),
            json!({"mcpServers": {"agent-screen": {"command": EXE, "args": ["mcp"]}}})
        );
        assert_eq!(state(&MCP_SERVERS, &text, EXE), State::Connected);
        assert!(text.find("command") < text.find("args"), "{text}");
    }

    #[test]
    fn keeps_other_servers_and_keys() {
        let before = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\" }\n  }\n}\n";
        let text = with_entry(&GEMINI, before, EXE).unwrap();
        assert_eq!(
            value(&text),
            json!({"theme": "dark", "mcpServers": {"other": {"command": "x"}, "agent-screen": {"command": EXE, "args": ["mcp"], "timeout": 700000}}})
        );
        assert!(text.starts_with("{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"other\": { \"command\": \"x\" },\n    \"agent-screen\": {\n      \"command\""), "{text}");
        assert_eq!(
            with_entry(&GEMINI, &text, EXE).unwrap(),
            text,
            "a second run changes nothing"
        );
    }

    #[test]
    fn repairs_a_stale_path_and_keeps_the_users_keys() {
        let before = format!(
            "{{\"mcpServers\": {{\"agent-screen\": {{\"command\": \"{OLD}\", \"args\": [\"mcp\"], \"env\": {{\"A\": \"1\"}}, \"disabled\": true, \"timeout\": 900}}}}}}"
        );
        assert_eq!(
            state(&SECONDS_TIMEOUT, &before, EXE),
            State::StalePath(OLD.into())
        );
        let text = with_entry(&SECONDS_TIMEOUT, &before, EXE).unwrap();
        assert_eq!(
            value(&text)["mcpServers"]["agent-screen"],
            json!({"command": EXE, "args": ["mcp"], "env": {"A": "1"}, "disabled": true, "timeout": 900})
        );
        assert_eq!(state(&SECONDS_TIMEOUT, &text, EXE), State::Connected);
    }

    #[test]
    fn removes_only_its_entry() {
        let before = "{\n  // servers\n  \"mcpServers\": {\n    \"agent-screen\": {\"command\": \"/x\"},\n    \"other\": {\"command\": \"y\"},\n  },\n}\n";
        let text = without_entry(&MCP_SERVERS, before, EXE).unwrap();
        assert_eq!(
            text,
            "{\n  // servers\n  \"mcpServers\": {\n    \"other\": {\"command\": \"y\"},\n  },\n}\n"
        );
        assert_eq!(without_entry(&MCP_SERVERS, &text, EXE).unwrap(), text);
        assert_eq!(without_entry(&MCP_SERVERS, "", EXE).unwrap(), "");
    }

    #[test]
    fn the_development_build_has_its_own_entry() {
        let text = with_entry(&CLAUDE_CODE, "{}", EXE).unwrap();
        let both = with_entry(&CLAUDE_CODE, &text, DEV).unwrap();
        assert_eq!(state(&CLAUDE_CODE, &both, EXE), State::Connected);
        assert_eq!(state(&CLAUDE_CODE, &both, DEV), State::Connected);
        assert_eq!(
            value(&both)["mcpServers"]["agent-screen-dev"]["command"],
            json!(DEV)
        );
        assert_eq!(without_entry(&CLAUDE_CODE, &both, DEV).unwrap(), text);
        assert_eq!(state(&CLAUDE_CODE, &text, DEV), State::NotConnected);
    }

    #[test]
    fn never_edits_a_broken_file() {
        for bad in [
            "{\"mcpServers\": {",
            "[1]",
            "{\"mcpServers\": 3}",
            "{\"mcpServers\": {\"agent-screen\": 1}}",
        ] {
            let edited = with_entry(&MCP_SERVERS, bad, EXE);
            assert!(edited.is_err(), "{bad}: {edited:?}");
        }
        assert!(matches!(
            state(&MCP_SERVERS, "{,}", EXE),
            State::InvalidConfig(_)
        ));
        assert!(without_entry(&MCP_SERVERS, "{,}", EXE).is_err());
    }

    #[test]
    fn opencode_writes_servers_and_follows_an_older_file() {
        let text = with_entry(&OPENCODE, "", EXE).unwrap();
        assert_eq!(
            value(&text),
            json!({"mcp": {"servers": {"agent-screen": {"type": "local", "command": [EXE, "mcp"]}}}})
        );
        let legacy = "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  // mine\n  \"mcp\": {\n    \"rea\": {\"type\": \"local\", \"command\": [\"rea\"], \"enabled\": true},\n  },\n}\n";
        let text = with_entry(&OPENCODE, legacy, EXE).unwrap();
        assert_eq!(
            value(&text)["mcp"]["agent-screen"],
            json!({"type": "local", "command": [EXE, "mcp"], "enabled": true})
        );
        assert!(text.contains("// mine") && text.contains("\"rea\""));
        assert_eq!(state(&OPENCODE, &text, EXE), State::Connected);
        let old = text.replace(EXE, OLD);
        assert_eq!(state(&OPENCODE, &old, EXE), State::StalePath(OLD.into()));
        assert_eq!(with_entry(&OPENCODE, &old, EXE).unwrap(), text);
        assert_eq!(
            value(&without_entry(&OPENCODE, &text, EXE).unwrap()),
            value(legacy)
        );
    }

    #[test]
    fn zed_reads_the_older_nested_command() {
        let nested = format!(
            "{{\"context_servers\": {{\"agent-screen\": {{\"command\": {{\"path\": \"{OLD}\", \"args\": [\"mcp\"]}}}}}}}}"
        );
        assert_eq!(state(&ZED, &nested, EXE), State::StalePath(OLD.into()));
        let text = with_entry(&ZED, &nested, EXE).unwrap();
        assert_eq!(
            value(&text)["context_servers"]["agent-screen"],
            json!({"command": EXE, "args": ["mcp"], "env": {}})
        );
    }

    #[test]
    fn keeps_a_comment_only_file_above_the_new_settings() {
        let before = "// my notes\n/* \"mcpServers\": {} */\n";
        let text = with_entry(&MCP_SERVERS, before, EXE).unwrap();
        assert!(text.starts_with(before), "{text}");
        assert_eq!(state(&MCP_SERVERS, &text, EXE), State::Connected);
        let text = with_entry(&MCP_SERVERS, "// no newline", EXE).unwrap();
        assert!(text.starts_with("// no newline\n{"), "{text}");
    }

    #[test]
    fn zed_keeps_the_env_of_the_older_nested_command() {
        let nested = format!(
            "{{\"context_servers\": {{\"agent-screen\": {{\"command\": {{\"path\": \"{OLD}\", \"args\": [\"mcp\"], \"env\": {{\"A\": \"1\"}}}}}}}}}}"
        );
        let text = with_entry(&ZED, &nested, EXE).unwrap();
        assert_eq!(
            value(&text)["context_servers"]["agent-screen"],
            json!({"command": EXE, "args": ["mcp"], "env": {"A": "1"}})
        );
        // A key the flat form has no place for keeps the object form.
        let extra = nested.replace("\"env\"", "\"cwd\": \"/x\", \"env\"");
        let text = with_entry(&ZED, &extra, EXE).unwrap();
        assert_eq!(
            value(&text)["context_servers"]["agent-screen"],
            json!({"command": {"path": EXE, "args": ["mcp"], "cwd": "/x", "env": {"A": "1"}}})
        );
        assert_eq!(state(&ZED, &text, EXE), State::Connected);
        assert_eq!(with_entry(&ZED, &text, EXE).unwrap(), text);
    }

    #[test]
    fn edits_jsonc_settings_in_place() {
        let before = "// Zed settings\n{\n  \"theme\": \"One Dark\", // mine\n  /* block */\n  \"buffer_font_size\": 15,\n}\n";
        let text = with_entry(&ZED, before, EXE).unwrap();
        assert!(text.starts_with("// Zed settings\n{\n  \"theme\": \"One Dark\", // mine\n  /* block */\n  \"buffer_font_size\": 15,\n  \"context_servers\": {"), "{text}");
        assert_eq!(state(&ZED, &text, EXE), State::Connected);
        assert_eq!(
            value(&without_entry(&ZED, &text, EXE).unwrap())["context_servers"],
            json!({})
        );
    }
}
