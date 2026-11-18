//! The agent-screen skill is the AGENTS.md snippet with skill frontmatter on
//! top. docs/agents/AGENTS.md is the source: edit it, then run
//! `AGENTSCREEN_BLESS_SKILL=1 cargo test -p agent-screen-mcp --test skill`
//! to copy it into the skill.

use std::path::PathBuf;

const SNIPPET: &str = "docs/agents/AGENTS.md";
const SKILL: &str = "skills/agent-screen/SKILL.md";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The frontmatter, from the opening `---` through the closing one and the
/// blank line after it.
fn frontmatter(skill: &str) -> &str {
    assert!(skill.starts_with("---\n"), "SKILL.md starts with ---");
    let close = "\n---\n\n";
    let end = skill[3..]
        .find(close)
        .expect("--- and a blank line close the frontmatter");
    &skill[..3 + end + close.len()]
}

#[test]
fn the_skill_body_is_the_agents_snippet() {
    let snippet = std::fs::read_to_string(repo().join(SNIPPET)).unwrap();
    let skill = std::fs::read_to_string(repo().join(SKILL)).unwrap();
    let head = frontmatter(&skill);
    let expected = format!("{head}{snippet}");
    if std::env::var_os("AGENTSCREEN_BLESS_SKILL").is_some() {
        std::fs::write(repo().join(SKILL), &expected).unwrap();
        return;
    }
    assert!(
        skill == expected,
        "{SKILL} differs from {SNIPPET}. Edit {SNIPPET}, then run \
         AGENTSCREEN_BLESS_SKILL=1 cargo test -p agent-screen-mcp --test skill"
    );
}

#[test]
fn the_description_names_the_triggers() {
    let skill = std::fs::read_to_string(repo().join(SKILL)).unwrap();
    let head = frontmatter(&skill);
    assert!(head.contains("\nname: agent-screen\n"), "{head}");
    let description = head
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .expect("one-line description");
    // Anthropic's limit for a skill description.
    assert!(
        description.len() <= 1024,
        "{} characters",
        description.len()
    );
    for words in [
        "Mac app",
        "web",
        "click",
        "filling in",
        "screenshot",
        "logging in",
        "open -a",
        "AppleScript",
        "System Events",
        "screencapture",
    ] {
        assert!(description.contains(words), "{words}");
    }
}
