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

/// Whether any folder on the way to `path`, or the file, is a link: the
/// skills installer links each agent's folder, and a dotfiles manager may
/// link ~/.agents or ~/.claude itself. Folders above the home folder, or
/// above the top folder for a path outside it (such as /var), don't count.
fn linked(env: &Env, path: &Path) -> bool {
    let base = if path.starts_with(&env.home) {
        env.home.clone()
    } else {
        path.components().take(2).collect()
    };
    // The deepest entry that is there, a dangling link included.
    let Some(there) = path.ancestors().find(|p| p.symlink_metadata().is_ok()) else {
        return false;
    };
    if there
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return true;
    }
    let (Ok(real), Ok(real_base), Ok(rest)) = (
        std::fs::canonicalize(there),
        std::fs::canonicalize(&base),
        there.strip_prefix(&base),
    ) else {
        return false;
    };
    real != real_base.join(rest)
}

/// Whether `npx skills` installed agent-screen: its lock file in
/// ~/.agents lists the skills it keeps, in ~/.agents/skills and the copies
/// or links it makes in each agent's folder.
fn skills_installer_keeps(env: &Env) -> bool {
    std::fs::read_to_string(env.home.join(".agents/.skill-lock.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|lock| lock["skills"].get("agent-screen").is_some())
}

/// Another installer keeps the copy at `path`: leave it alone.
fn kept_elsewhere(env: &Env, path: &Path) -> bool {
    linked(env, path) || skills_installer_keeps(env)
}

pub fn write(env: &Env, path: &Path) -> Result<Skill, String> {
    if kept_elsewhere(env, path) {
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
    write(env, &shared(env))
}

pub fn write_claude(env: &Env) -> Result<Skill, String> {
    if claude_plugin_installed(env) {
        Ok(Skill::Plugin)
    } else {
        write(env, &claude(env))
    }
}

/// Codex reads ~/.agents/skills: a second copy in its own folder would
/// repeat the skill.
pub fn write_codex(env: &Env) -> Result<Skill, String> {
    let shared = shared(env);
    if shared.exists() {
        Ok(Skill::Shared(shared))
    } else {
        write(env, &codex(env))
    }
}

pub fn copy_state(env: &Env, path: &Path) -> Copy {
    if path == claude(env) && claude_plugin_installed(env) {
        return Copy::Plugin;
    }
    if linked(env, path) {
        return Copy::Linked;
    }
    match std::fs::read_to_string(path) {
        Err(_) => Copy::Missing,
        Ok(_) if skills_installer_keeps(env) => Copy::Linked,
        Ok(text) if text == SKILL => Copy::Current,
        Ok(_) => Copy::Outdated,
    }
}

/// Bring the copies an earlier install wrote up to this version. Links and
/// the skills installer's copies stay; a missing copy stays missing.
pub fn refresh(env: &Env) -> Vec<PathBuf> {
    [claude(env), codex(env), shared(env)]
        .into_iter()
        .filter(|path| {
            copy_state(env, path) == Copy::Outdated
                && matches!(write(env, path), Ok(Skill::Written(_)))
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
        assert!(matches!(write(&env, &path), Ok(Skill::Written(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL);
        assert!(matches!(write(&env, &path), Ok(Skill::Current(_))));
        std::fs::write(&path, "an older skill").unwrap();
        assert!(matches!(write(&env, &path), Ok(Skill::Written(_))));
        // A folder linked by another installer keeps that installer's copy.
        let elsewhere = env.home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("SKILL.md"), "theirs").unwrap();
        let linked = env.home.join("linked/agent-screen");
        std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();
        assert!(matches!(
            write(&env, &linked.join("SKILL.md")),
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
    fn leaves_every_copy_the_skills_installer_keeps() {
        let env = home("lock");
        for path in [claude(&env), codex(&env)] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "npx's copy").unwrap();
        }
        std::fs::create_dir_all(env.home.join(".agents")).unwrap();
        std::fs::write(
            env.home.join(".agents/.skill-lock.json"),
            r#"{"skills":{"agent-screen":{}}}"#,
        )
        .unwrap();
        assert_eq!(write_claude(&env), Ok(Skill::Linked(claude(&env))));
        assert_eq!(write_codex(&env), Ok(Skill::Linked(codex(&env))));
        assert_eq!(copy_state(&env, &claude(&env)), Copy::Linked);
        assert!(refresh(&env).is_empty());
        for path in [claude(&env), codex(&env)] {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "npx's copy");
        }
        std::fs::remove_dir_all(&env.home).unwrap();
    }

    #[test]
    fn never_writes_through_a_linked_folder_anywhere_on_the_path() {
        let env = home("dotfiles");
        // A dotfiles manager links ~/.agents itself.
        let dotfiles = env.home.join("dotfiles/agents");
        std::fs::create_dir_all(dotfiles.join("skills/agent-screen")).unwrap();
        std::fs::write(dotfiles.join("skills/agent-screen/SKILL.md"), "older").unwrap();
        std::os::unix::fs::symlink(&dotfiles, env.home.join(".agents")).unwrap();
        assert_eq!(write_shared(&env), Ok(Skill::Linked(shared(&env))));
        assert_eq!(copy_state(&env, &shared(&env)), Copy::Linked);
        assert!(refresh(&env).is_empty());
        assert_eq!(
            std::fs::read_to_string(dotfiles.join("skills/agent-screen/SKILL.md")).unwrap(),
            "older"
        );
        // A linked ~/.claude, with no skill there yet.
        let claude_real = env.home.join("dotfiles/claude");
        std::fs::create_dir_all(&claude_real).unwrap();
        std::os::unix::fs::symlink(&claude_real, env.home.join(".claude")).unwrap();
        assert_eq!(write_claude(&env), Ok(Skill::Linked(claude(&env))));
        assert!(!claude_real.join("skills").exists());
        // A dangling link as the file.
        let codex_dir = env.codex_home().join("skills/agent-screen");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::os::unix::fs::symlink(env.home.join("nowhere"), codex_dir.join("SKILL.md")).unwrap();
        std::fs::remove_dir_all(dotfiles.parent().unwrap().join("agents")).unwrap();
        assert_eq!(write_codex(&env), Ok(Skill::Linked(codex(&env))));
        assert!(!env.home.join("nowhere").exists());
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
