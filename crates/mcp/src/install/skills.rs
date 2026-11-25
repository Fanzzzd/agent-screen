//! The agent-screen skill this copy of the app carries, so the skill
//! describes the tools this version has. One copy goes in the shared
//! ~/.agents/skills, which Codex and other agents read; Claude Code reads
//! only its own folder.

use super::Env;
use std::path::{Path, PathBuf};

/// The agent-screen skill, as this version of the app describes its tools.
pub const SKILL: &str = include_str!("../../../../skills/agent-screen/SKILL.md");

/// What installing the skill did.
#[derive(Debug, PartialEq)]
pub enum Skill {
    /// Written there, new or replacing an older copy.
    Written(PathBuf),
    /// Already this version there.
    Current(PathBuf),
    /// Claude Code's agent-screen plugin carries it.
    Plugin,
    /// A link there points to a copy another installer keeps, or the skills
    /// installer lists it: left alone.
    Linked(PathBuf),
    /// The client reads the shared copy there.
    Shared(PathBuf),
}

/// A skill copy's state, for `mcp status`.
#[derive(Debug, PartialEq)]
pub enum Copy {
    Current,
    Outdated,
    Missing,
    /// Another installer's: a link, or listed in the skills installer's lock.
    Linked,
    Plugin,
}

pub fn shared(env: &Env) -> PathBuf {
    env.home.join(".agents/skills/agent-screen/SKILL.md")
}

pub fn claude(env: &Env) -> PathBuf {
    env.claude_home().join("skills/agent-screen/SKILL.md")
}

pub fn codex(env: &Env) -> PathBuf {
    env.codex_home().join("skills/agent-screen/SKILL.md")
}

/// The file, its folder, or the skills folder above it is a link, as the
/// skills installer makes for each agent.
fn linked(path: &Path) -> bool {
    path.ancestors().take(3).any(|p| {
        p.symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    })
}

/// Whether `npx skills` installed agent-screen in ~/.agents/skills: its
/// lock file lists the skills it keeps.
fn skills_installer_keeps(env: &Env) -> bool {
    std::fs::read_to_string(env.home.join(".agents/.skill-lock.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|lock| lock["skills"].get("agent-screen").is_some())
}

pub fn write(path: &Path) -> Result<Skill, String> {
    if linked(path) {
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

pub fn write_shared(env: &Env) -> Result<Skill, String> {
    let path = shared(env);
    if skills_installer_keeps(env) {
        return Ok(Skill::Linked(path));
    }
    write(&path)
}

pub fn write_claude(env: &Env) -> Result<Skill, String> {
    if claude_plugin_installed(env) {
        Ok(Skill::Plugin)
    } else {
        write(&claude(env))
    }
}

/// Codex reads ~/.agents/skills: a second copy in its own folder would
/// repeat the skill.
pub fn write_codex(env: &Env) -> Result<Skill, String> {
    let shared = shared(env);
    if shared.exists() {
        Ok(Skill::Shared(shared))
    } else {
        write(&codex(env))
    }
}

pub fn copy_state(env: &Env, path: &Path) -> Copy {
    if path == claude(env) && claude_plugin_installed(env) {
        return Copy::Plugin;
    }
    if linked(path) || (path == shared(env) && skills_installer_keeps(env)) {
        return Copy::Linked;
    }
    match std::fs::read_to_string(path) {
        Ok(text) if text == SKILL => Copy::Current,
        Ok(_) => Copy::Outdated,
        Err(_) => Copy::Missing,
    }
}

/// Bring the copies an earlier install wrote up to this version. Links and
/// the skills installer's copy stay; a missing copy stays missing.
pub fn refresh(env: &Env) -> Vec<PathBuf> {
    [claude(env), codex(env), shared(env)]
        .into_iter()
        .filter(|path| {
            copy_state(env, path) == Copy::Outdated && matches!(write(path), Ok(Skill::Written(_)))
        })
        .collect()
}

/// Whether Claude Code has the agent-screen plugin from this repository's
/// marketplace, which carries the skill: a second copy would only repeat it.
fn claude_plugin_installed(env: &Env) -> bool {
    let read = |p: &str| std::fs::read_to_string(env.claude_home().join(p)).unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> Env {
        let dir =
            std::env::temp_dir().join(format!("agent-screen-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Env::for_home(dir)
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
        let env = home("once");
        let path = env.home.join("skills/agent-screen/SKILL.md");
        assert!(matches!(write(&path), Ok(Skill::Written(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL);
        assert!(matches!(write(&path), Ok(Skill::Current(_))));
        std::fs::write(&path, "an older skill").unwrap();
        assert!(matches!(write(&path), Ok(Skill::Written(_))));
        // A folder linked by another installer keeps that installer's copy.
        let elsewhere = env.home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("SKILL.md"), "theirs").unwrap();
        let linked = env.home.join("linked/agent-screen");
        std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();
        assert!(matches!(
            write(&linked.join("SKILL.md")),
            Ok(Skill::Linked(_))
        ));
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("SKILL.md")).unwrap(),
            "theirs"
        );
        std::fs::remove_dir_all(&env.home).unwrap();
    }

    #[test]
    fn the_shared_copy_serves_codex_and_respects_the_skills_installer() {
        let env = home("shared");
        assert!(
            matches!(write_codex(&env), Ok(Skill::Written(_))),
            "no shared copy yet"
        );
        assert!(matches!(write_shared(&env), Ok(Skill::Written(_))));
        assert_eq!(write_codex(&env), Ok(Skill::Shared(shared(&env))));
        std::fs::write(shared(&env), "npx's").unwrap();
        std::fs::write(
            env.home.join(".agents/.skill-lock.json"),
            r#"{"version":3,"skills":{"agent-screen":{}}}"#,
        )
        .unwrap();
        assert_eq!(write_shared(&env), Ok(Skill::Linked(shared(&env))));
        assert_eq!(copy_state(&env, &shared(&env)), Copy::Linked);
        assert!(refresh(&env).is_empty());
        assert_eq!(std::fs::read_to_string(shared(&env)).unwrap(), "npx's");
        std::fs::remove_dir_all(&env.home).unwrap();
    }

    #[test]
    fn refreshes_only_older_copies_it_wrote() {
        let env = home("refresh");
        for path in [claude(&env), shared(&env)] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "older").unwrap();
        }
        assert_eq!(refresh(&env), vec![claude(&env), shared(&env)]);
        assert_eq!(copy_state(&env, &codex(&env)), Copy::Missing);
        assert_eq!(copy_state(&env, &shared(&env)), Copy::Current);
        assert!(refresh(&env).is_empty());
        std::fs::remove_dir_all(&env.home).unwrap();
    }
}
