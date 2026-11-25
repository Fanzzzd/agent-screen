//! `AgentScreen mcp status | install | uninstall`. English only: the output
//! goes to a terminal or a script.

use super::{
    CLIENTS, Client, Done, Env, Options, Removed, Skill, State, client, install, server_name,
    skills, status, uninstall,
};
use serde_json::{Value, json};
use std::path::Path;

pub const USAGE: &str = "  AgentScreen mcp status [--json]
                          which agent clients this Mac has and whether they run this copy
  AgentScreen mcp install [--client <id,...>] [--no-skill] [--dry-run] [--json]
                          register the MCP server with every client found, or the ones named
  AgentScreen mcp uninstall [--client <id,...>] [--dry-run] [--json]
                          remove the server's entry, leaving other servers and settings
";

pub struct Output {
    pub text: String,
    /// 0 done, 1 a client failed, 2 the arguments are wrong.
    pub code: i32,
}

#[derive(Default)]
struct Args {
    clients: Option<Vec<&'static Client>>,
    no_skill: bool,
    dry_run: bool,
    json: bool,
}

fn parse(args: &[String], allowed: &[&str]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        if !allowed.contains(&flag) {
            return Err(format!("unknown argument {arg}"));
        }
        match flag {
            "--json" => parsed.json = true,
            "--no-skill" => parsed.no_skill = true,
            "--dry-run" => parsed.dry_run = true,
            "--client" => {
                let list = inline
                    .or_else(|| rest.next().cloned())
                    .ok_or("--client needs a list of clients")?;
                let mut named = parsed.clients.take().unwrap_or_default();
                for id in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let found = client(id).ok_or_else(|| {
                        let ids: Vec<&str> = CLIENTS.iter().map(|c| c.id).collect();
                        format!("unknown client {id}; the clients are {}", ids.join(", "))
                    })?;
                    if !named.contains(&found) {
                        named.push(found);
                    }
                }
                if named.is_empty() {
                    return Err("--client needs a list of clients".into());
                }
                parsed.clients = Some(named);
            }
            _ => unreachable!("allowed flags are handled"),
        }
    }
    Ok(parsed)
}

/// Run `mcp <command> <args>` for `exe`. None when `command` isn't one of
/// these, as for the pipe itself.
pub fn run(command: &str, args: &[String], exe: &Path, env: &Env) -> Option<Output> {
    let result = match command {
        "status" => parse(args, &["--json"]).map(|a| status_output(exe, env, a.json)),
        "install" => parse(args, &["--client", "--no-skill", "--dry-run", "--json"])
            .map(|a| install_output(exe, env, a)),
        "uninstall" => {
            parse(args, &["--client", "--dry-run", "--json"]).map(|a| uninstall_output(exe, env, a))
        }
        _ => return None,
    };
    Some(result.unwrap_or_else(|e| Output {
        text: format!("Agent Screen mcp {command}: {e}\n\nUsage:\n{USAGE}"),
        code: 2,
    }))
}

fn pretty(value: Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    )
}

fn state_json(state: &State) -> Value {
    match state {
        State::StalePath(from) => json!({"state": state.key(), "command": from}),
        State::InvalidConfig(e) => json!({"state": state.key(), "error": e}),
        _ => json!({"state": state.key()}),
    }
}

fn skill_key(copy: &skills::Copy) -> &'static str {
    match copy {
        skills::Copy::Current => "current",
        skills::Copy::Outdated => "outdated",
        skills::Copy::Missing => "missing",
        skills::Copy::Linked => "linked",
        skills::Copy::Plugin => "plugin",
    }
}

fn status_output(exe: &Path, env: &Env, as_json: bool) -> Output {
    let exe_text = exe.to_string_lossy();
    let all = status(exe, env);
    let copies = [
        ("shared", skills::shared(env)),
        ("claude-code", skills::claude(env)),
        ("codex", skills::codex(env)),
    ]
    .map(|(who, path)| {
        let copy = skills::copy_state(env, &path);
        (who, path, copy)
    });
    if as_json {
        let clients: Vec<Value> = all
            .iter()
            .map(|s| {
                let mut v = json!({
                    "id": s.client.id,
                    "name": s.client.name,
                    "detected": s.detected,
                    "config": s.config,
                    "restart": s.client.restart,
                });
                if let (Value::Object(v), Value::Object(state)) = (&mut v, state_json(&s.state)) {
                    v.extend(state);
                }
                v
            })
            .collect();
        let skills: Vec<Value> = copies
            .iter()
            .map(|(who, path, copy)| json!({"for": who, "path": path, "state": skill_key(copy)}))
            .collect();
        return Output {
            text: pretty(json!({
                "server": server_name(&exe_text),
                "command": exe,
                "args": ["mcp"],
                "clients": clients,
                "skills": skills,
            })),
            code: 0,
        };
    }
    let mut text = format!("Server {}: {} mcp\n", server_name(&exe_text), exe.display());
    let mut missing = Vec::new();
    for s in &all {
        if !s.detected && s.state == State::NotConnected {
            missing.push(s.client.name);
            continue;
        }
        let state = match &s.state {
            State::Connected => "connected".to_string(),
            State::StalePath(from) => format!("stale, runs {}", from.display()),
            State::NotConnected => "not connected".to_string(),
            State::InvalidConfig(e) => format!("invalid config, left alone: {e}"),
        };
        text.push_str(&format!(
            "{} ({}): {state}, {}\n",
            s.client.name,
            s.client.id,
            s.config.display()
        ));
    }
    if !missing.is_empty() {
        text.push_str(&format!("Not found on this Mac: {}\n", missing.join(", ")));
    }
    for (who, path, copy) in &copies {
        if *copy != skills::Copy::Missing || *who == "shared" {
            text.push_str(&format!(
                "Skill ({who}): {}, {}\n",
                skill_key(copy),
                path.display()
            ));
        }
    }
    Output { text, code: 0 }
}

fn done_json(done: &Done) -> Value {
    match done {
        Done::Already => json!({"result": "already"}),
        Done::AddedTo(_) => json!({"result": "added"}),
        Done::Repointed { from, .. } => json!({"result": "repointed", "from": from}),
        Done::Updated(_) => json!({"result": "updated"}),
    }
}

fn skill_json(skill: &Result<Skill, String>) -> Value {
    match skill {
        Ok(Skill::Written(p)) => json!({"result": "written", "path": p}),
        Ok(Skill::Current(p)) => json!({"result": "current", "path": p}),
        Ok(Skill::Linked(p)) => json!({"result": "linked", "path": p}),
        Ok(Skill::Shared(p)) => json!({"result": "shared", "path": p}),
        Ok(Skill::Plugin) => json!({"result": "plugin"}),
        Err(e) => json!({"result": "failed", "error": e}),
    }
}

/// The skill line, as `mcp install` has printed it.
fn skill_line(who: &str, skill: &Result<Skill, String>) -> String {
    match skill {
        Ok(Skill::Written(path)) => format!("{who}: skill written to {}", path.display()),
        Ok(Skill::Current(path)) => format!("{who}: skill up to date in {}", path.display()),
        Ok(Skill::Linked(path)) => format!(
            "{who}: skill in {} is another installer's, left alone",
            path.display()
        ),
        Ok(Skill::Shared(path)) => format!("{who}: reads the shared skill in {}", path.display()),
        Ok(Skill::Plugin) => format!("{who}: skill comes from the agent-screen plugin"),
        Err(error) => format!("{who}: skill failed: {error}"),
    }
}

fn install_output(exe: &Path, env: &Env, args: Args) -> Output {
    let options = Options {
        clients: args.clients,
        skill: !args.no_skill,
        dry_run: args.dry_run,
    };
    let report = install(exe, env, &options);
    let failed = report.outcomes.iter().any(|o| o.result.is_err())
        || report.shared_skill.as_ref().is_some_and(|s| s.is_err());
    let code = i32::from(failed);
    if args.json {
        let clients: Vec<Value> = report
            .outcomes
            .iter()
            .map(|o| {
                let mut v = match &o.result {
                    Ok(done) => done_json(done),
                    Err(e) => json!({"result": "failed", "error": e}),
                };
                v["id"] = json!(o.client.id);
                v["name"] = json!(o.client.name);
                v["config"] = json!(o.config);
                v["restart"] = json!(o.client.restart);
                if let Some(skill) = &o.skill {
                    v["skill"] = skill_json(skill);
                }
                v
            })
            .collect();
        let missing: Vec<&str> = report.missing.iter().map(|c| c.id).collect();
        return Output {
            text: pretty(json!({
                "dry_run": options.dry_run,
                "server": server_name(&exe.to_string_lossy()),
                "clients": clients,
                "skill": report.shared_skill.as_ref().map(skill_json),
                "not_found": missing,
            })),
            code,
        };
    }
    let mut text = String::new();
    if options.dry_run {
        text.push_str("Dry run: nothing is written.\n");
    }
    if report.outcomes.is_empty() {
        text.push_str(&format!(
            "No agent client found. For another MCP client, add a stdio server with command\n{}\nand arguments [\"mcp\"]. See docs/clients.md.\n",
            exe.display()
        ));
    }
    for o in &report.outcomes {
        let line = match &o.result {
            Ok(done) if options.dry_run && *done != Done::Already => format!("would be {done}"),
            Ok(done) if o.client.restart && *done != Done::Already => {
                format!("{done}; quit {} and open it again", o.client.name)
            }
            Ok(done) => done.to_string(),
            Err(error) => format!("failed: {error}"),
        };
        text.push_str(&format!("{}: {line}\n", o.client.name));
        if let Some(skill) = &o.skill {
            text.push_str(&format!("{}\n", skill_line(o.client.name, skill)));
        }
    }
    if let Some(skill) = &report.shared_skill {
        text.push_str(&format!("{}\n", skill_line("Shared", skill)));
    }
    if !report.missing.is_empty() {
        let names: Vec<&str> = report.missing.iter().map(|c| c.name).collect();
        text.push_str(&format!("Not found on this Mac: {}\n", names.join(", ")));
    }
    Output { text, code }
}

fn uninstall_output(exe: &Path, env: &Env, args: Args) -> Output {
    let removals = uninstall(exe, env, args.clients.as_deref(), args.dry_run);
    let code = i32::from(removals.iter().any(|r| r.result.is_err()));
    if args.json {
        let clients: Vec<Value> = removals
            .iter()
            .map(|r| {
                let mut v = match &r.result {
                    Ok(Removed::From(_)) => json!({"result": "removed"}),
                    Ok(Removed::NotThere) => json!({"result": "not_connected"}),
                    Err(e) => json!({"result": "failed", "error": e}),
                };
                v["id"] = json!(r.client.id);
                v["name"] = json!(r.client.name);
                v["config"] = json!(r.config);
                v
            })
            .collect();
        return Output {
            text: pretty(json!({
                "dry_run": args.dry_run,
                "server": server_name(&exe.to_string_lossy()),
                "clients": clients,
            })),
            code,
        };
    }
    let mut text = String::new();
    if args.dry_run {
        text.push_str("Dry run: nothing is written.\n");
    }
    if removals.is_empty() {
        text.push_str("No client runs Agent Screen's server.\n");
    }
    for r in &removals {
        let line = match &r.result {
            Ok(Removed::From(path)) if args.dry_run => {
                format!("would be removed from {}", path.display())
            }
            Ok(Removed::From(path)) => format!("removed from {}", path.display()),
            Ok(Removed::NotThere) => "not connected".to_string(),
            Err(error) => format!("failed: {error}"),
        };
        text.push_str(&format!("{}: {line}\n", r.client.name));
    }
    Output { text, code }
}
