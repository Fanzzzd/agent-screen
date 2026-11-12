//! From a change to the scripts that cover it: changed files map to areas
//! (`PATHS`), and areas to the scripts the manifest lists under them. A
//! change the map can't place runs the core tier and says so, so nothing
//! is ever left out without a word.

use crate::manifest::{Area, SCRIPTS, Script, Tier, find};
use Area::*;

/// Source paths and the areas they belong to, as globs from the repository
/// root: `*` stays inside one folder, `**` crosses folders. A file takes
/// the areas of every glob it matches. An empty list marks code that cuts
/// across areas, such as the control socket's dispatch or the shell: a
/// change there runs the core tier, unless another glob gives the file
/// areas of its own.
pub const PATHS: &[(&str, &[Area])] = &[
    // The app.
    ("apps/macos/src/main.rs", &[]),
    ("apps/macos/src/core.rs", &[Displays]),
    ("apps/macos/src/lifecycle.rs", &[Displays]),
    ("apps/macos/src/dismissal.rs", &[Displays, Windows]),
    ("apps/macos/src/i18n.rs", &[]),
    ("apps/macos/src/input_queue.rs", &[InputQueue]),
    ("apps/macos/src/control/agent.rs", &[]),
    ("apps/macos/src/control/mod.rs", &[]),
    ("apps/macos/src/control/server.rs", &[]),
    ("apps/macos/src/control/shared_app.rs", &[AgentInput]),
    ("apps/macos/src/control/values.rs", &[Elements]),
    ("apps/macos/src/mac/mod.rs", &[]),
    ("apps/macos/src/mac/aerospace.rs", &[Windows]),
    ("apps/macos/src/mac/alerts.rs", &[Windows]),
    ("apps/macos/src/mac/app.rs", &[Windows]),
    ("apps/macos/src/mac/assist.rs", &[Settings]),
    ("apps/macos/src/mac/ax.rs", &[Elements]),
    ("apps/macos/src/mac/capture.rs", &[Screenshots, Viewer]),
    ("apps/macos/src/mac/catch.rs", &[Windows]),
    ("apps/macos/src/mac/cg.rs", &[Displays]),
    ("apps/macos/src/mac/claims.rs", &[Windows]),
    ("apps/macos/src/mac/click_diagnostics.rs", &[ViewerInput]),
    ("apps/macos/src/mac/colorsync.rs", &[Displays]),
    ("apps/macos/src/mac/devtools.rs", &[Browser]),
    ("apps/macos/src/mac/display.rs", &[Displays]),
    ("apps/macos/src/mac/hang.rs", &[Performance]),
    ("apps/macos/src/mac/ime.rs", &[InputMethod]),
    ("apps/macos/src/mac/input.rs", &[AgentInput, ViewerInput]),
    ("apps/macos/src/mac/menubar.rs", &[Peek]),
    ("apps/macos/src/mac/mux.rs", &[Recording]),
    ("apps/macos/src/mac/native.rs", &[Peek]),
    ("apps/macos/src/mac/notify.rs", &[Takeover, WindowAccess]),
    ("apps/macos/src/mac/permissions.rs", &[Settings]),
    ("apps/macos/src/mac/playback.rs", &[Recording]),
    ("apps/macos/src/mac/real_input.rs", &[InputQueue]),
    ("apps/macos/src/mac/recording.rs", &[Recording]),
    ("apps/macos/src/mac/screenshot.rs", &[Screenshots]),
    ("apps/macos/src/mac/shared_app.rs", &[AgentInput]),
    ("apps/macos/src/mac/update.rs", &[Release]),
    (
        "apps/macos/src/mac/viewer_input.rs",
        &[ViewerInput, InputMethod],
    ),
    ("apps/macos/src/mac/windows.rs", &[Windows]),
    ("apps/macos/src/ui/mod.rs", &[]),
    ("apps/macos/src/ui/shell.rs", &[]),
    ("apps/macos/src/ui/theme.rs", &[]),
    (
        "apps/macos/src/ui/help_card.rs",
        &[Takeover, WindowAccess, InputQueue],
    ),
    ("apps/macos/src/ui/input_bar.rs", &[InputQueue]),
    ("apps/macos/src/ui/peek.rs", &[Peek]),
    ("apps/macos/src/ui/pointer.rs", &[Viewer]),
    ("apps/macos/src/ui/recordings.rs", &[Recording]),
    ("apps/macos/src/ui/setup.rs", &[Settings]),
    ("apps/macos/src/ui/status.rs", &[Peek, Viewer]),
    ("apps/macos/src/ui/stream.rs", &[Viewer, Peek]),
    ("apps/macos/src/ui/viewer.rs", &[ViewerInput, Viewer]),
    ("apps/macos/native/SkyLight.m", &[AgentInput, ViewerInput]),
    ("apps/macos/native/Playback.m", &[Recording]),
    ("apps/macos/native/AgentScreenPrivate.h", &[]),
    ("apps/macos/tests/**", &[Recording]),
    ("apps/macos/assets/**", &[]),
    ("apps/macos/assets/status/**", &[Peek]),
    ("apps/macos/scripts/**", &[Release]),
    // The domain rules.
    ("crates/model/Cargo.toml", &[]),
    ("crates/model/examples/**", &[InputQueue]),
    ("crates/model/src/lib.rs", &[]),
    ("crates/model/src/model.rs", &[]),
    ("crates/model/src/diagnostics.rs", &[]),
    ("crates/model/src/access.rs", &[WindowAccess]),
    ("crates/model/src/alerts.rs", &[Windows]),
    ("crates/model/src/arrangement.rs", &[Displays]),
    ("crates/model/src/budget.rs", &[Displays]),
    (
        "crates/model/src/card.rs",
        &[Takeover, WindowAccess, InputQueue],
    ),
    ("crates/model/src/claims.rs", &[Windows]),
    ("crates/model/src/devtools.rs", &[Browser]),
    ("crates/model/src/input_queue.rs", &[InputQueue]),
    ("crates/model/src/key_window.rs", &[AgentInput]),
    ("crates/model/src/keychord.rs", &[AgentInput]),
    ("crates/model/src/launch.rs", &[Windows]),
    ("crates/model/src/menu_press.rs", &[Elements, InputQueue]),
    ("crates/model/src/new_window.rs", &[Windows]),
    ("crates/model/src/numbers.rs", &[Elements]),
    ("crates/model/src/peek.rs", &[Peek]),
    ("crates/model/src/placement.rs", &[Windows]),
    ("crates/model/src/pointer_geometry.rs", &[Viewer]),
    ("crates/model/src/real_click.rs", &[InputQueue]),
    ("crates/model/src/real_drag.rs", &[InputQueue]),
    ("crates/model/src/recording_playback.rs", &[Recording]),
    ("crates/model/src/returns.rs", &[WindowAccess]),
    ("crates/model/src/screen_edge.rs", &[InputQueue]),
    ("crates/model/src/selection.rs", &[Elements]),
    ("crates/model/src/shared_app.rs", &[AgentInput]),
    ("crates/model/src/substitute.rs", &[Elements, Browser]),
    ("crates/model/src/tree.rs", &[Elements]),
    ("crates/model/src/typing.rs", &[AgentInput]),
    (
        "crates/model/src/viewer_keys.rs",
        &[ViewerInput, InputMethod],
    ),
    ("crates/model/src/watch.rs", &[Elements]),
    ("crates/model/testdata/**", &[InputQueue]),
    // The MCP server carries every tool, and the helper keeps ColorSync's
    // registry short for new displays.
    ("crates/mcp/**", &[]),
    ("crates/colorsync/**", &[Displays]),
    // The scripts' fixtures, and the helpers every script runs on.
    ("tests/e2e/fixtures/**", &[]),
    ("tests/e2e/fixtures/web/**", &[Browser, Elements]),
    ("tests/e2e/fixtures/electron/**", &[Browser, AgentInput]),
    ("tests/e2e/fixtures/login/**", &[ViewerInput]),
    ("tests/e2e/fixtures/trackpad.log", &[ViewerInput]),
    ("tests/e2e/fixtures/workspace/**", &[Elements]),
    ("tests/e2e/src/*.rs", &[]),
    ("tests/e2e/tools/**", &[]),
    ("tests/e2e/Cargo.toml", &[]),
    // What every build takes in.
    ("apps/macos/Cargo.toml", &[]),
    ("apps/macos/build.rs", &[]),
    ("apps/macos/resources/**", &[]),
    ("vendor/**", &[]),
    ("Cargo.toml", &[]),
    ("Cargo.lock", &[]),
    (".cargo/**", &[]),
    ("rust-toolchain.toml", &[]),
];

/// Folders and files whose changes can break the app or the scripts.
/// Anything else, the docs and the website among them, needs no script.
const CODE: &[&str] = &[
    "apps/macos/**",
    "crates/**",
    "tests/e2e/**",
    "vendor/**",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/**",
];

/// Whether `path` matches `glob`: `**` is any run of characters, `*` any
/// run without a slash, and everything else stands for itself.
pub fn matches(glob: &str, path: &str) -> bool {
    fn go(glob: &[u8], path: &[u8]) -> bool {
        match glob {
            [] => path.is_empty(),
            [b'*', b'*', rest @ ..] => (0..=path.len()).any(|i| go(rest, &path[i..])),
            [b'*', rest @ ..] => (0..=path.len())
                .take_while(|&i| i == 0 || path[i - 1] != b'/')
                .any(|i| go(rest, &path[i..])),
            [c, rest @ ..] => path.first() == Some(c) && go(rest, &path[1..]),
        }
    }
    go(glob.as_bytes(), path.as_bytes())
}

/// Where a changed file lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// Not code: no script covers it.
    Elsewhere,
    /// A script's own file.
    Script(&'static str),
    /// Code that belongs to these areas.
    Areas(Vec<Area>),
    /// Code that cuts across areas.
    Everywhere,
    /// Code the map doesn't list.
    Unmapped,
}

pub fn place(path: &str) -> Place {
    if !CODE.iter().any(|glob| matches(glob, path)) {
        return Place::Elsewhere;
    }
    let script = path
        .strip_prefix("tests/e2e/src/bin/")
        .and_then(|file| file.strip_suffix(".rs"))
        .and_then(find);
    if let Some(script) = script {
        return Place::Script(script.name);
    }
    let found: Vec<&[Area]> = PATHS
        .iter()
        .filter(|(glob, _)| matches(glob, path))
        .map(|(_, areas)| *areas)
        .collect();
    let mut areas: Vec<Area> = found.iter().flat_map(|a| a.iter().copied()).collect();
    areas.sort();
    areas.dedup();
    match (found.is_empty(), areas.is_empty()) {
        (true, _) => Place::Unmapped,
        (false, true) => Place::Everywhere,
        (false, false) => Place::Areas(areas),
    }
}

/// What to run: the scripts `suite` runs, the manual ones the same choice
/// takes in, and what the user should know about the choice.
#[derive(Debug, Default)]
pub struct Selection {
    pub scripts: Vec<&'static Script>,
    pub by_hand: Vec<&'static Script>,
    pub notes: Vec<String>,
}

/// What the user asked for. No changed files, no areas and no tier selects
/// every script.
#[derive(Debug, Default)]
pub struct Wanted {
    /// The files `--changed` found.
    pub changed: Option<Vec<String>>,
    pub areas: Vec<Area>,
    pub tier: Option<Tier>,
}

fn list(names: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut text = names[..names.len().min(SHOWN)].join(", ");
    if names.len() > SHOWN {
        text.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    text
}

/// The scripts a change calls for, by name, with the reasons.
fn changed(files: &[String], notes: &mut Vec<String>) -> Vec<&'static str> {
    let mut areas: Vec<Area> = Vec::new();
    let mut names: Vec<&'static str> = Vec::new();
    let (mut across, mut unmapped, mut elsewhere) = (Vec::new(), Vec::new(), 0);
    for file in files {
        match place(file) {
            Place::Elsewhere => elsewhere += 1,
            Place::Script(name) => names.push(name),
            Place::Areas(found) => areas.extend(found),
            Place::Everywhere => across.push(file.clone()),
            Place::Unmapped => unmapped.push(file.clone()),
        }
    }
    areas.sort();
    areas.dedup();
    match elsewhere {
        0 => {}
        1 => notes.push("1 changed file is not app or test code and needs no script.".into()),
        n => notes.push(format!(
            "{n} changed files are not app or test code and need no script."
        )),
    }
    if elsewhere == files.len() {
        notes.push("No app or test code changed: nothing to run.".into());
        return names;
    }
    if !areas.is_empty() {
        let found: Vec<&str> = areas.iter().map(|a| a.name()).collect();
        notes.push(format!("Areas changed: {}.", found.join(", ")));
    }
    names.extend(
        SCRIPTS
            .iter()
            .filter(|s| areas.iter().any(|&a| s.covers(a)))
            .map(|s| s.name),
    );
    let mut core = false;
    if !across.is_empty() {
        core = true;
        notes.push(format!(
            "{} {} across areas: adding the core tier.",
            list(&across),
            if across.len() == 1 { "cuts" } else { "cut" }
        ));
    }
    if !unmapped.is_empty() {
        core = true;
        notes.push(format!(
            "{} maps to no area in select.rs: adding the core tier.",
            list(&unmapped)
        ));
    }
    let runs = |name: &&str| find(name).is_some_and(|s| s.tier != Tier::Manual);
    if !core && !names.iter().any(runs) {
        core = true;
        notes.push("The suite runs no script for this change: adding the core tier.".into());
    }
    if core {
        names.extend(
            SCRIPTS
                .iter()
                .filter(|s| s.tier == Tier::Core)
                .map(|s| s.name),
        );
    }
    names
}

/// Pick the scripts for `wanted`, in the manifest's order.
pub fn select(wanted: &Wanted) -> Selection {
    let mut notes = Vec::new();
    let named = wanted
        .changed
        .as_ref()
        .map(|files| changed(files, &mut notes));
    let picked: Vec<&'static Script> = SCRIPTS
        .iter()
        .filter(|s| named.as_ref().is_none_or(|names| names.contains(&s.name)))
        .filter(|s| wanted.areas.is_empty() || wanted.areas.iter().any(|&a| s.covers(a)))
        .filter(|s| wanted.tier != Some(Tier::Core) || s.tier == Tier::Core)
        .collect();
    let (by_hand, scripts): (Vec<_>, Vec<_>) =
        picked.into_iter().partition(|s| s.tier == Tier::Manual);
    Selection {
        scripts,
        by_hand,
        notes,
    }
}

/// The order to run in: the scripts that leave the user alone first, then
/// the hands-off ones together, so the user steps away once.
pub fn ordered(scripts: &[&'static Script]) -> Vec<&'static Script> {
    let mut order = scripts.to_vec();
    order.sort_by_key(|s| s.hands_off);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(scripts: &[&'static Script]) -> Vec<&'static str> {
        scripts.iter().map(|s| s.name).collect()
    }

    fn for_change(files: &[&str]) -> Selection {
        select(&Wanted {
            changed: Some(files.iter().map(|f| f.to_string()).collect()),
            ..Wanted::default()
        })
    }

    fn core() -> Vec<&'static str> {
        SCRIPTS
            .iter()
            .filter(|s| s.tier == Tier::Core)
            .map(|s| s.name)
            .collect()
    }

    #[test]
    fn globs() {
        assert!(matches("crates/mcp/**", "crates/mcp/src/tools.rs"));
        assert!(matches("apps/macos/src/*.rs", "apps/macos/src/core.rs"));
        assert!(!matches("apps/macos/src/*.rs", "apps/macos/src/mac/ime.rs"));
        assert!(matches("Cargo.toml", "Cargo.toml"));
        assert!(!matches("Cargo.toml", "apps/macos/Cargo.toml"));
        assert!(!matches("crates/mcp/**", "crates/mcpx"));
        assert!(matches("a/**/c.rs", "a/b/b/c.rs"));
    }

    #[test]
    fn a_file_lands_in_its_areas() {
        assert_eq!(
            place("apps/macos/src/mac/viewer_input.rs"),
            Place::Areas(vec![ViewerInput, InputMethod])
        );
        assert_eq!(
            place("crates/model/src/arrangement.rs"),
            Place::Areas(vec![Displays])
        );
        assert_eq!(place("apps/macos/src/ui/shell.rs"), Place::Everywhere);
        assert_eq!(place("crates/mcp/src/tools.rs"), Place::Everywhere);
        assert_eq!(place("apps/macos/src/mac/brand_new.rs"), Place::Unmapped);
        assert_eq!(place("tests/e2e/src/lib.rs"), Place::Everywhere);
        assert_eq!(place("Cargo.lock"), Place::Everywhere);
        // A glob with areas wins over the folder's empty one.
        assert_eq!(
            place("apps/macos/assets/status/idle.svg"),
            Place::Areas(vec![Peek])
        );
        assert_eq!(place("apps/macos/assets/icons/give.svg"), Place::Everywhere);
        assert_eq!(place("crates/model/Cargo.toml"), Place::Everywhere);
        assert_eq!(
            place("tests/e2e/src/bin/viewerime.rs"),
            Place::Script("viewerime")
        );
        assert_eq!(place("apps/website/public/index.html"), Place::Elsewhere);
        assert_eq!(place("docs/DEVELOPMENT.md"), Place::Elsewhere);
    }

    /// A glob that matches no file is a rename the map missed: the file's
    /// changes would run the core tier instead of its own scripts.
    #[test]
    fn every_glob_matches_a_file() {
        fn walk(dir: &std::path::Path, root: &std::path::Path, files: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, files);
                } else if let Ok(rel) = path.strip_prefix(root) {
                    files.push(rel.to_string_lossy().into_owned());
                }
            }
        }
        let root = crate::root();
        let mut files = Vec::new();
        for dir in ["apps/macos", "crates", "tests/e2e", "vendor"] {
            walk(&root.join(dir), root, &mut files);
        }
        for file in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo/config.toml",
        ] {
            if root.join(file).exists() {
                files.push(file.into());
            }
        }
        for (glob, _) in PATHS {
            assert!(
                files.iter().any(|f| matches(glob, f)),
                "{glob} in select.rs matches no file"
            );
        }
    }

    #[test]
    fn a_viewer_input_change_runs_the_viewer_and_input_method_scripts() {
        let picked = for_change(&["apps/macos/src/mac/viewer_input.rs"]);
        assert_eq!(
            names(&picked.scripts),
            [
                "ime",
                "launchhere",
                "nativeclick",
                "oneclick",
                "takeover",
                "viewerdrag",
                "viewerime",
                "viewerkey",
                "viewerpopup",
                "viewerwetype"
            ]
        );
        assert_eq!(names(&picked.by_hand), ["run"]);
        assert_eq!(picked.notes, ["Areas changed: viewer-input, input-method."]);
    }

    #[test]
    fn a_model_change_runs_its_area() {
        let picked = for_change(&["crates/model/src/arrangement.rs"]);
        let displays: Vec<_> = SCRIPTS
            .iter()
            .filter(|s| s.covers(Displays) && s.tier != Tier::Manual)
            .map(|s| s.name)
            .collect();
        assert_eq!(names(&picked.scripts), displays);
        assert!(displays.contains(&"arrangement") && displays.contains(&"parksafe"));
        assert_eq!(names(&picked.by_hand), ["changecost", "timing"]);
    }

    #[test]
    fn a_change_outside_the_code_runs_nothing_and_says_so() {
        let picked = for_change(&["apps/website/public/index.html", "README.md"]);
        assert!(picked.scripts.is_empty() && picked.by_hand.is_empty());
        assert_eq!(
            picked.notes,
            [
                "2 changed files are not app or test code and need no script.",
                "No app or test code changed: nothing to run."
            ]
        );
    }

    #[test]
    fn an_unmapped_app_file_runs_the_core_tier_and_says_so() {
        let picked = for_change(&["apps/macos/src/mac/brand_new.rs"]);
        assert_eq!(names(&picked.scripts), core());
        assert_eq!(
            picked.notes,
            ["apps/macos/src/mac/brand_new.rs maps to no area in select.rs: adding the core tier."]
        );
    }

    #[test]
    fn code_that_cuts_across_areas_adds_the_core_tier_to_the_areas_scripts() {
        let picked = for_change(&["apps/macos/src/ui/shell.rs", "apps/macos/src/mac/update.rs"]);
        assert_eq!(names(&picked.scripts), core());
        assert_eq!(names(&picked.by_hand), ["selfupdate"]);
        assert!(picked.notes.iter().any(|n| n.contains("cuts across areas")));

        let picked = for_change(&["apps/macos/src/ui/shell.rs", "apps/macos/src/ui/peek.rs"]);
        let got = names(&picked.scripts);
        assert!(got.contains(&"peekanim"), "an area's full script");
        assert!(got.contains(&"spares"), "a core script of another area");
    }

    /// The release area has only a manual script: the change still runs
    /// something.
    #[test]
    fn an_area_the_suite_has_no_script_for_runs_the_core_tier() {
        let picked = for_change(&["apps/macos/scripts/bundle.sh"]);
        assert_eq!(names(&picked.scripts), core());
        assert_eq!(names(&picked.by_hand), ["selfupdate"]);
        assert!(picked.notes.iter().any(|n| n.contains("no script")));
    }

    #[test]
    fn a_changed_script_runs_itself() {
        let picked = for_change(&["tests/e2e/src/bin/serials.rs"]);
        assert_eq!(names(&picked.scripts), ["serials"]);
        let picked = for_change(&["tests/e2e/src/bin/perf.rs"]);
        assert_eq!(names(&picked.scripts), core(), "a manual script alone");
        assert_eq!(names(&picked.by_hand), ["perf"]);
    }

    #[test]
    fn an_area_and_a_tier_narrow_the_choice() {
        let all = select(&Wanted::default());
        assert_eq!(
            all.scripts.len(),
            SCRIPTS.iter().filter(|s| s.tier != Tier::Manual).count()
        );
        assert_eq!(all.scripts.len() + all.by_hand.len(), SCRIPTS.len());

        let peek = select(&Wanted {
            areas: vec![Peek],
            ..Wanted::default()
        });
        assert!(peek.scripts.iter().all(|s| s.covers(Peek)));
        assert_eq!(names(&peek.by_hand), ["run"]);

        let core_peek = select(&Wanted {
            areas: vec![Peek],
            tier: Some(Tier::Core),
            ..Wanted::default()
        });
        assert_eq!(names(&core_peek.scripts), ["peekhover"]);
        assert!(core_peek.by_hand.is_empty());

        let narrowed = select(&Wanted {
            changed: Some(vec!["apps/macos/src/mac/viewer_input.rs".into()]),
            tier: Some(Tier::Core),
            ..Wanted::default()
        });
        assert_eq!(
            names(&narrowed.scripts),
            [
                "ime",
                "oneclick",
                "takeover",
                "viewerime",
                "viewerpopup",
                "viewerwetype"
            ]
        );
    }

    #[test]
    fn hands_off_scripts_run_last_and_together() {
        let all = select(&Wanted::default());
        let order = ordered(&all.scripts);
        let first_hands_off = order.iter().position(|s| s.hands_off).unwrap();
        assert!(order[..first_hands_off].iter().all(|s| !s.hands_off));
        assert!(order[first_hands_off..].iter().all(|s| s.hands_off));
        assert_eq!(order.len(), all.scripts.len());
    }
}
