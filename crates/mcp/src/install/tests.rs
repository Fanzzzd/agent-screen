//! Every client, in a temporary home: detect, install, install again,
//! repair a stale path, keep the user's servers, refuse a broken file,
//! keep the development build apart, uninstall.

use super::*;

const EXE: &str = "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen";
const DEV: &str = "/Users/me/agent-screen/target/Agent Screen Dev.app/Contents/MacOS/AgentScreen";
const OLD: &str = "/Volumes/Agent Screen/Agent Screen.app/Contents/MacOS/AgentScreen";

struct Home(Env);

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!(
            "agent-screen-install-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(Env::for_home(dir))
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0.home);
    }
}

fn only(client: &'static Client) -> Options {
    Options {
        clients: Some(vec![client]),
        skill: false,
        dry_run: false,
    }
}

fn state(client: &Client, env: &Env, exe: &str) -> State {
    read_state(client, env, exe).1
}

/// A config holding another server, in the client's own format.
fn with_other_server(client: &Client) -> &'static str {
    match client.id {
        "codex" => "# mine\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"other\"\n",
        "goose" => {
            "# mine\nGOOSE_MODEL: x\nextensions:\n  other:\n    type: stdio\n    name: other\n    cmd: other\n"
        }
        "zed" => {
            "// mine\n{\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"other\": {\"command\": \"other\", \"args\": []},\n  },\n}\n"
        }
        "opencode" => {
            "{\n  // mine\n  \"mcp\": {\n    \"servers\": {\n      \"other\": {\"type\": \"local\", \"command\": [\"other\"]}\n    }\n  }\n}\n"
        }
        "vscode" => {
            "{\n  \"mine\": true,\n  \"mcpServers\": {\n    \"other\": {\"type\": \"stdio\", \"command\": \"other\"}\n  }\n}\n"
        }
        _ => {
            "{\n  \"theme\": \"mine\",\n  \"mcpServers\": {\n    \"other\": {\"command\": \"other\", \"args\": []}\n  }\n}\n"
        }
    }
}

/// A file the client couldn't read.
fn broken(client: &Client) -> &'static str {
    match client.id {
        "codex" => "[mcp_servers\ncommand = ",
        "goose" => "extensions: [\n",
        _ => "{\"mcpServers\": {,}",
    }
}

#[test]
fn every_client_installs_repairs_and_uninstalls_without_touching_the_rest() {
    for client in CLIENTS {
        let home = Home::new(client.id);
        let env = &home.0;
        let config = client.config(env);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let before = with_other_server(client);
        std::fs::write(&config, before).unwrap();
        assert!(client.detected(env), "{client:?}: its config shows it");
        assert_eq!(state(client, env, EXE), State::NotConnected, "{client:?}");

        let report = install(Path::new(EXE), env, &only(client));
        assert_eq!(
            report.outcomes[0].result,
            Ok(Done::AddedTo(config.clone())),
            "{client:?}"
        );
        let installed = std::fs::read_to_string(&config).unwrap();
        assert_eq!(
            state(client, env, EXE),
            State::Connected,
            "{client:?}:\n{installed}"
        );
        assert!(
            installed.contains("other") && installed.contains("mine"),
            "{client:?} lost the user's lines:\n{installed}"
        );

        let again = install(Path::new(EXE), env, &only(client));
        assert_eq!(again.outcomes[0].result, Ok(Done::Already), "{client:?}");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            installed,
            "{client:?}"
        );

        // A copy run from the disk image, since moved to Applications.
        std::fs::write(&config, installed.replace(EXE, OLD)).unwrap();
        assert_eq!(
            state(client, env, EXE),
            State::StalePath(OLD.into()),
            "{client:?}"
        );
        let repaired = install(Path::new(EXE), env, &only(client));
        assert_eq!(
            repaired.outcomes[0].result,
            Ok(Done::Repointed {
                from: OLD.into(),
                config: config.clone()
            }),
            "{client:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            installed,
            "{client:?}"
        );

        // The development build registers beside the installed app.
        install(Path::new(DEV), env, &only(client));
        assert_eq!(state(client, env, EXE), State::Connected, "{client:?}");
        assert_eq!(state(client, env, DEV), State::Connected, "{client:?}");
        let removed = uninstall(Path::new(DEV), env, Some(&[client]), false);
        assert_eq!(
            removed[0].result,
            Ok(Removed::From(config.clone())),
            "{client:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            installed,
            "{client:?}"
        );

        let removed = uninstall(Path::new(EXE), env, None, false);
        assert_eq!(removed.len(), 1, "{client:?}: only the client that has it");
        assert_eq!(
            removed[0].result,
            Ok(Removed::From(config.clone())),
            "{client:?}"
        );
        let after = std::fs::read_to_string(&config).unwrap();
        assert_eq!(state(client, env, EXE), State::NotConnected, "{client:?}");
        assert!(
            after.contains("other") && after.contains("mine"),
            "{client:?}:\n{after}"
        );
        assert_eq!(
            uninstall(Path::new(EXE), env, Some(&[client]), false)[0].result,
            Ok(Removed::NotThere)
        );
    }
}

#[test]
fn every_client_writes_a_new_config_it_can_read() {
    for client in CLIENTS {
        let home = Home::new(&format!("new-{}", client.id));
        let env = &home.0;
        let report = install(Path::new(EXE), env, &only(client));
        assert!(
            report.outcomes[0].result.is_ok(),
            "{client:?}: {:?}",
            report.outcomes[0].result
        );
        assert_eq!(state(client, env, EXE), State::Connected, "{client:?}");
    }
}

#[test]
fn a_broken_config_is_never_written() {
    for client in CLIENTS {
        let home = Home::new(&format!("broken-{}", client.id));
        let env = &home.0;
        let config = client.config(env);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, broken(client)).unwrap();
        assert!(
            matches!(state(client, env, EXE), State::InvalidConfig(_)),
            "{client:?}"
        );
        let report = install(Path::new(EXE), env, &only(client));
        assert!(report.outcomes[0].result.is_err(), "{client:?}");
        let removed = uninstall(Path::new(EXE), env, Some(&[client]), false);
        assert!(removed[0].result.is_err(), "{client:?}");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            broken(client),
            "{client:?}"
        );
    }
}

#[test]
fn the_expected_entry_for_each_json_client() {
    let home = Home::new("entries");
    let env = &home.0;
    let entry = |id: &str, pointer: &str| {
        let client = client(id).unwrap();
        install(Path::new(EXE), env, &only(client));
        let text = std::fs::read_to_string(client.config(env)).unwrap();
        jsonc::read(&text)
            .unwrap()
            .pointer(pointer)
            .cloned()
            .unwrap()
    };
    use serde_json::json;
    assert_eq!(
        entry("claude-code", "/mcpServers/agent-screen"),
        json!({"type": "stdio", "command": EXE, "args": ["mcp"]})
    );
    assert_eq!(
        entry("cursor", "/mcpServers/agent-screen"),
        json!({"command": EXE, "args": ["mcp"]})
    );
    assert_eq!(
        entry("gemini-cli", "/mcpServers/agent-screen/timeout"),
        json!(700_000)
    );
    assert_eq!(
        entry("cline", "/mcpServers/agent-screen/timeout"),
        json!(700)
    );
    assert_eq!(
        entry("roo-code", "/mcpServers/agent-screen/timeout"),
        json!(700)
    );
    assert_eq!(
        entry("opencode", "/mcp/servers/agent-screen"),
        json!({"type": "local", "command": [EXE, "mcp"]})
    );
    assert_eq!(
        entry("vscode", "/mcpServers/agent-screen"),
        json!({"type": "stdio", "command": EXE, "args": ["mcp"], "tools": ["*"]})
    );
    assert_eq!(
        entry("zed", "/context_servers/agent-screen"),
        json!({"command": EXE, "args": ["mcp"], "env": {}})
    );
}

#[test]
fn config_paths_follow_the_clients_variables() {
    let home = Home::new("vars");
    let env = home
        .0
        .clone()
        .with_var("CLAUDE_CONFIG_DIR", home.0.home.join("cc"))
        .with_var("CODEX_HOME", home.0.home.join("cx"))
        .with_var("XDG_CONFIG_HOME", home.0.home.join("xdg"))
        .with_var("COPILOT_HOME", home.0.home.join("cp"));
    let h = &home.0.home;
    assert_eq!(
        client("claude-code").unwrap().config(&env),
        h.join("cc/.claude.json")
    );
    assert_eq!(
        client("codex").unwrap().config(&env),
        h.join("cx/config.toml")
    );
    assert_eq!(
        client("opencode").unwrap().config(&env),
        h.join("xdg/opencode/opencode.json")
    );
    assert_eq!(
        client("goose").unwrap().config(&env),
        h.join("xdg/goose/config.yaml")
    );
    assert_eq!(
        client("vscode").unwrap().config(&env),
        h.join("cp/mcp-config.json")
    );
    assert_eq!(
        client("claude-code").unwrap().config(&home.0),
        h.join(".claude.json")
    );
    // OpenCode reads opencode.jsonc first when there is one.
    std::fs::create_dir_all(h.join("xdg/opencode")).unwrap();
    std::fs::write(h.join("xdg/opencode/opencode.jsonc"), "{}").unwrap();
    assert_eq!(
        client("opencode").unwrap().config(&env),
        h.join("xdg/opencode/opencode.jsonc")
    );
}

#[test]
fn antigravity_follows_where_its_version_keeps_the_config() {
    let home = Home::new("agy");
    let env = &home.0;
    let agy = client("antigravity-cli").unwrap();
    assert_eq!(agy.id, "antigravity");
    assert_eq!(
        agy.config(env),
        env.home.join(".gemini/config/mcp_config.json")
    );
    let legacy = env.home.join(".gemini/antigravity/mcp_config.json");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "").unwrap();
    assert_eq!(agy.config(env), legacy, "Antigravity 1 only");
    install(Path::new(EXE), env, &only(agy));
    assert_eq!(
        state(agy, env, EXE),
        State::Connected,
        "an empty file reads as no settings"
    );
    std::fs::create_dir_all(env.home.join(".gemini/config")).unwrap();
    assert_eq!(
        agy.config(env),
        env.home.join(".gemini/config/mcp_config.json")
    );
}

#[test]
fn detects_by_app_command_line_or_folder() {
    let home = Home::new("detect");
    let env = &home.0;
    assert!(
        CLIENTS.iter().all(|c| !c.detected(env)),
        "an empty home has none"
    );
    std::fs::create_dir_all(env.home.join("Applications/Cursor.app")).unwrap();
    std::fs::create_dir_all(env.home.join(".local/bin")).unwrap();
    std::fs::write(env.home.join(".local/bin/goose"), "").unwrap();
    std::fs::create_dir_all(vscode_storage(env, "rooveterinaryinc.roo-cline")).unwrap();
    let found: Vec<&str> = CLIENTS
        .iter()
        .filter(|c| c.detected(env))
        .map(|c| c.id)
        .collect();
    assert_eq!(found, ["cursor", "roo-code", "goose"]);
    let report = install(
        Path::new(EXE),
        env,
        &Options {
            skill: false,
            ..Options::default()
        },
    );
    let ids: Vec<&str> = report.outcomes.iter().map(|o| o.client.id).collect();
    assert_eq!(ids, ["cursor", "roo-code", "goose"]);
    assert_eq!(report.missing.len(), CLIENTS.len() - 3);
}

#[test]
fn a_dry_run_writes_nothing() {
    let home = Home::new("dry");
    let env = &home.0;
    let cursor = client("cursor").unwrap();
    let report = install(
        Path::new(EXE),
        env,
        &Options {
            dry_run: true,
            ..only(cursor)
        },
    );
    assert_eq!(
        report.outcomes[0].result,
        Ok(Done::AddedTo(cursor.config(env)))
    );
    assert!(!cursor.config(env).exists());
    assert!(report.shared_skill.is_none());
}

#[test]
fn installs_the_skills_for_a_release_build_only() {
    let home = Home::new("skills");
    let env = &home.0;
    let both = || Options {
        clients: Some(vec![
            client("claude-code").unwrap(),
            client("codex").unwrap(),
        ]),
        ..Options::default()
    };
    let report = install(Path::new(EXE), env, &both());
    assert_eq!(
        report.shared_skill,
        Some(Ok(Skill::Written(skills::shared(env))))
    );
    assert_eq!(
        report.outcomes[0].skill,
        Some(Ok(Skill::Written(skills::claude(env))))
    );
    assert_eq!(
        report.outcomes[1].skill,
        Some(Ok(Skill::Shared(skills::shared(env))))
    );
    assert!(!skills::codex(env).exists(), "Codex reads the shared copy");
    let dev = install(Path::new(DEV), env, &both());
    assert!(dev.shared_skill.is_none() && dev.outcomes.iter().all(|o| o.skill.is_none()));
    let no_skill = install(
        Path::new(EXE),
        env,
        &Options {
            skill: false,
            ..both()
        },
    );
    assert!(no_skill.shared_skill.is_none());
}

#[test]
fn refreshes_skills_only_for_the_installed_app() {
    let home = Home::new("refresh");
    let env = &home.0;
    let shared = skills::shared(env);
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    std::fs::write(&shared, "older").unwrap();
    assert!(refresh_skills_in(Path::new(DEV), env).is_empty());
    assert!(
        refresh_skills_in(
            Path::new("/Users/me/Downloads/Agent Screen.app/Contents/MacOS/AgentScreen"),
            env
        )
        .is_empty()
    );
    let e2e = env.clone().with_var("AGENTSCREEN_INSTANCE", "e2e");
    assert!(refresh_skills_in(Path::new(EXE), &e2e).is_empty());
    assert_eq!(refresh_skills_in(Path::new(EXE), env), vec![shared.clone()]);
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), SKILL);
}

#[test]
fn every_id_and_alias_names_one_client() {
    let mut names: Vec<&str> = CLIENTS
        .iter()
        .flat_map(|c| std::iter::once(c.id).chain(c.aliases.iter().copied()))
        .collect();
    let count = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), count);
    assert_eq!(client("vscode-insiders").unwrap().id, "vscode");
}

#[test]
fn the_cli_reports_installs_and_refuses_unknown_clients() {
    let home = Home::new("cli");
    let env = &home.0;
    let exe = Path::new(EXE);
    let run = |cmd: &str, args: &[&str]| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        cli::run(cmd, &args, exe, env).unwrap()
    };
    assert!(
        cli::run("--v1", &[], exe, env).is_none(),
        "not a command of these"
    );
    let bad = run("install", &["--client", "nope"]);
    assert_eq!(bad.code, 2);
    assert!(bad.text.contains("unknown client nope"), "{}", bad.text);
    assert_eq!(run("status", &["--frobnicate"]).code, 2);

    let out = run("install", &["--client=cursor,zed", "--no-skill"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains(&format!(
            "Cursor: added to {}",
            client("cursor").unwrap().config(env).display()
        )),
        "{}",
        out.text
    );
    let json: serde_json::Value = serde_json::from_str(&run("status", &["--json"]).text).unwrap();
    let cursor = json["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "cursor")
        .unwrap();
    assert_eq!(cursor["state"], "connected");
    assert_eq!(json["server"], "agent-screen");

    let again: serde_json::Value =
        serde_json::from_str(&run("install", &["--client", "cursor", "--json"]).text).unwrap();
    assert_eq!(again["clients"][0]["result"], "already");

    let out = run("uninstall", &["--dry-run"]);
    assert!(
        out.text.contains("Cursor: would be removed from")
            && out.text.contains("Zed: would be removed"),
        "{}",
        out.text
    );
    assert!(
        run("status", &[])
            .text
            .contains("Cursor (cursor): connected")
    );
    let out = run("uninstall", &["--client", "cursor"]);
    assert!(out.text.contains("Cursor: removed from"), "{}", out.text);
    assert!(
        run("status", &[])
            .text
            .contains("Cursor (cursor): not connected")
    );

    std::fs::write(client("zed").unwrap().config(env), "{").unwrap();
    let out = run("install", &["--client", "zed"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text.contains("Zed: failed:") && out.text.contains("left as it is"),
        "{}",
        out.text
    );
}

#[cfg(unix)]
#[test]
fn json_output_survives_a_path_that_isnt_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let home = Home::new("lossy");
    let odd = home.0.home.join(std::ffi::OsStr::from_bytes(b"codex-\xff"));
    let env = home.0.clone().with_var("CODEX_HOME", &odd);
    let exe = Path::new(EXE);
    let run = |cmd: &str, args: &[&str]| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        cli::run(cmd, &args, exe, &env).unwrap()
    };
    // APFS refuses such a name, so the write fails; the report still comes.
    let installed = run("install", &["--client", "codex", "--no-skill", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&installed.text).unwrap();
    assert!(
        json["clients"][0]["config"]
            .as_str()
            .unwrap()
            .contains("codex-\u{fffd}")
    );
    let status: serde_json::Value = serde_json::from_str(&run("status", &["--json"]).text).unwrap();
    let codex = status["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "codex")
        .unwrap();
    assert!(codex["config"].as_str().unwrap().contains("codex-\u{fffd}"));
    assert!(serde_json::from_str::<serde_json::Value>(&run("uninstall", &["--json"]).text).is_ok());
}

#[test]
fn sessions_are_matched_to_their_client_by_name() {
    let id = |info: &str| client_for_info(info).map(|c| c.id);
    for (info, want) in [
        ("Claude Code", "claude-code"),
        ("claude-code", "claude-code"),
        ("claude-ai", "claude-desktop"),
        ("Codex", "codex"),
        ("codex-mcp-client", "codex"),
        ("cursor-vscode", "cursor"),
        ("Cursor", "cursor"),
        ("antigravity-client", "antigravity"),
        ("gemini-cli-mcp-client", "gemini-cli"),
        ("opencode", "opencode"),
        ("Visual Studio Code", "vscode"),
        ("github-copilot-cli", "vscode"),
        ("windsurf-client", "windsurf"),
        ("Devin", "devin"),
        ("Zed", "zed"),
        ("Cline", "cline"),
        ("Roo Code", "roo-code"),
        ("goose", "goose"),
    ] {
        assert_eq!(id(info), Some(want), "{info}");
    }
    for info in ["", "Agent", "test", "my-own-client"] {
        assert_eq!(id(info), None, "{info}");
    }
}

#[test]
fn each_snippet_is_the_entry_the_registry_writes() {
    let exe = Path::new(EXE);
    let json: serde_json::Value = serde_json::from_str(&snippet(Snippet::Json, exe)).unwrap();
    assert_eq!(json["mcpServers"]["agent-screen"]["command"], EXE);
    assert_eq!(
        json["mcpServers"]["agent-screen"]["args"],
        serde_json::json!(["mcp"])
    );
    let opencode: serde_json::Value =
        serde_json::from_str(&snippet(Snippet::OpenCode, exe)).unwrap();
    assert!(opencode.to_string().contains(EXE));
    let toml: toml_edit::DocumentMut = snippet(Snippet::Toml, exe).parse().unwrap();
    assert_eq!(
        toml["mcp_servers"]["agent-screen"]["command"].as_str(),
        Some(EXE)
    );
    assert_eq!(snippet(Snippet::Command, exe), format!("'{EXE}' mcp\n"));
    assert_eq!(
        snippet(Snippet::Command, Path::new("/x/it's/AgentScreen")),
        "'/x/it'\\''s/AgentScreen' mcp\n"
    );
    // The development build's entry keeps its own name.
    let dev: serde_json::Value =
        serde_json::from_str(&snippet(Snippet::Json, Path::new(DEV))).unwrap();
    assert!(dev["mcpServers"]["agent-screen-dev"].is_object());
}
