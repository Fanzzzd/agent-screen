//! Codex's config.toml, edited with toml_edit so comments and the user's
//! own keys stay.

use super::{State, server_name};
use std::path::PathBuf;
use toml_edit::{Array, DocumentMut, Item, Table, value};

/// Codex waits 60 s for a tool call by default, and wait_for_control can
/// wait 10 minutes for the user.
const TOOL_TIMEOUT: i64 = 700;
/// Codex waits 10 s for a server to start by default, and the pipe waits
/// up to 15 s for an app it starts (ADR 0021).
const STARTUP_TIMEOUT: i64 = 30;

fn parse(text: &str) -> Result<DocumentMut, String> {
    text.parse()
        .map_err(|e| format!("it isn't valid TOML: {e}"))
}

pub fn state(text: &str, exe: &str) -> State {
    let doc = match parse(text) {
        Ok(doc) => doc,
        Err(e) => return State::InvalidConfig(e),
    };
    let Some(entry) = doc.get("mcp_servers").and_then(|s| s.get(server_name(exe))) else {
        return State::NotConnected;
    };
    match entry.get("command").and_then(Item::as_str) {
        Some(command) if command == exe => State::Connected,
        command => State::StalePath(PathBuf::from(command.unwrap_or_default())),
    }
}

/// The config with Agent Screen's entry running `exe`. Sets only command
/// and args, and each timeout the entry lacks: the user's own keys and
/// subtables in it (env, approvals, enabled, a timeout they chose), other
/// entries and comments stay as they are. A config that isn't valid TOML is
/// left alone.
pub fn with_entry(text: &str, exe: &str) -> Result<String, String> {
    let mut doc = parse(text)?;
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
    // A comment line inside an inline table is valid only from TOML 1.1,
    // and Codex may read TOML 1.0: comments go in a standard table only.
    let commented = servers.get(name).is_some_and(Item::is_table);
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
        if let Some(mut key) = entry.key_mut("tool_timeout_sec").filter(|_| commented) {
            key.leaf_decor_mut()
                .set_prefix("# wait_for_control can wait up to 10 minutes for the user.\n");
        }
    }
    if !entry.contains_key("startup_timeout_sec") {
        entry.insert("startup_timeout_sec", value(STARTUP_TIMEOUT));
        if let Some(mut key) = entry.key_mut("startup_timeout_sec").filter(|_| commented) {
            key.leaf_decor_mut()
                .set_prefix("# A cold start of the app can take 15 s.\n");
        }
    }
    Ok(doc.to_string())
}

/// The config without Agent Screen's entry, its subtables included.
pub fn without_entry(text: &str, exe: &str) -> Result<String, String> {
    let mut doc = parse(text)?;
    let name = server_name(exe);
    let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut) else {
        return Ok(text.to_string());
    };
    if servers.remove(name).is_none() {
        return Ok(text.to_string());
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen";
    const DEV: &str =
        "/Users/me/agent-screen/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";

    fn runs(text: &str, exe: &str) -> bool {
        state(text, exe) == State::Connected
    }

    #[test]
    fn counts_only_an_entry_that_runs_this_copy() {
        let codex = with_entry("", EXE).unwrap();
        assert!(runs(&codex, EXE));
        assert_eq!(
            state(&codex.replace(EXE, "/Volumes/Agent Screen/x"), EXE),
            State::StalePath("/Volumes/Agent Screen/x".into())
        );
        assert_eq!(state("", EXE), State::NotConnected);
    }

    #[test]
    fn the_development_build_registers_beside_the_installed_app() {
        let text = with_entry("", EXE).unwrap();
        let both = with_entry(&text, DEV).unwrap();
        assert!(
            both.contains("[mcp_servers.agent-screen]")
                && both.contains("[mcp_servers.agent-screen-dev]")
        );
        assert!(runs(&both, EXE) && runs(&both, DEV));
        let back = without_entry(&both, DEV).unwrap();
        assert_eq!(
            back, text,
            "removing the dev entry leaves the installed one"
        );
    }

    #[test]
    fn appends_to_a_config_without_it() {
        let text = with_entry("model = \"o3\"\n", EXE).unwrap();
        assert_eq!(
            text,
            format!(
                "model = \"o3\"\n\n[mcp_servers.agent-screen]\ncommand = {EXE:?}\nargs = [\"mcp\"]\n# wait_for_control can wait up to 10 minutes for the user.\ntool_timeout_sec = 700\n# A cold start of the app can take 15 s.\nstartup_timeout_sec = 30\n"
            )
        );
    }

    #[test]
    fn leaves_a_correct_entry_alone() {
        let once = with_entry("", EXE).unwrap();
        assert_eq!(with_entry(&once, EXE).unwrap(), once);
    }

    #[test]
    fn repoints_an_entry_and_keeps_the_users_settings() {
        let before = "[a]\nx = 1\n\n[mcp_servers.agent-screen]\ncommand = \"/old\"\nargs = [\"mcp\"]\nenabled = false\ntool_timeout_sec = 900\n\n[mcp_servers.agent-screen.env]\nA = \"1\"\n\n[mcp_servers.agent-screen.tools.computer]\napproval_mode = \"approve\"\n\n[b]\ny = 2\n";
        let text = with_entry(before, EXE).unwrap();
        assert!(!text.contains("/old"));
        assert!(runs(&text, EXE));
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
        assert!(runs(&before, EXE), "it already runs this copy");
        let text = with_entry(&before, EXE).unwrap();
        assert!(text.starts_with(&before) && text.ends_with("startup_timeout_sec = 30\n"));
    }

    #[test]
    fn finds_a_quoted_or_inline_entry_instead_of_adding_another() {
        for before in [
            "[mcp_servers.\"agent-screen\"]\ncommand = \"/old\"\n".to_string(),
            "[mcp_servers]\nagent-screen = { command = \"/old\", args = [\"mcp\"] }\n".to_string(),
        ] {
            let text = with_entry(&before, EXE).unwrap();
            assert_eq!(text.matches("agent-screen").count(), 1, "{text}");
            assert!(runs(&text, EXE), "{text}");
        }
    }

    #[test]
    fn an_inline_entry_gets_no_comment_lines() {
        let before = "[mcp_servers]\nagent-screen = { command = \"/old\", args = [\"mcp\"] }\n";
        let text = with_entry(before, EXE).unwrap();
        assert!(!text.contains('#'), "{text}");
        assert_eq!(
            text.lines().count(),
            2,
            "the entry stays on one line: {text}"
        );
        assert!(runs(&text, EXE), "{text}");
    }

    #[test]
    fn removes_only_its_entry_and_subtables() {
        let before = "# mine\n[mcp_servers.other]\ncommand = \"x\"\n\n[mcp_servers.agent-screen]\ncommand = \"/old\"\n\n[mcp_servers.agent-screen.env]\nA = \"1\"\n\n[b]\ny = 2\n";
        let text = without_entry(before, EXE).unwrap();
        assert_eq!(
            text,
            "# mine\n[mcp_servers.other]\ncommand = \"x\"\n\n[b]\ny = 2\n"
        );
        assert_eq!(without_entry(&text, EXE).unwrap(), text);
    }

    #[test]
    fn leaves_a_config_it_cannot_read_alone() {
        assert!(with_entry("[mcp_servers\ncommand = ", EXE).is_err());
        assert!(without_entry("[mcp_servers\n", EXE).is_err());
        assert!(matches!(
            state("[mcp_servers\n", EXE),
            State::InvalidConfig(_)
        ));
    }
}
