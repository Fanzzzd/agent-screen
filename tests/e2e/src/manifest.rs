//! The one list of the scripts in `src/bin`: what each covers, how it
//! drives the app, and what it costs to run. `suite` picks scripts from it,
//! `run` reads its sign and its build from it, and a unit test fails when a
//! file in `src/bin` has no entry or an entry has no file.

/// A part of Agent Screen a script covers, in CONTEXT.md's words. A change
/// to the app maps to areas (`select::PATHS`), and areas map to scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Area {
    ViewerInput,
    InputMethod,
    AgentInput,
    InputQueue,
    Displays,
    Windows,
    WindowAccess,
    Takeover,
    Peek,
    Viewer,
    Elements,
    Browser,
    Screenshots,
    Recording,
    Settings,
    Release,
    Performance,
}

impl Area {
    pub const ALL: &[Area] = &[
        Area::ViewerInput,
        Area::InputMethod,
        Area::AgentInput,
        Area::InputQueue,
        Area::Displays,
        Area::Windows,
        Area::WindowAccess,
        Area::Takeover,
        Area::Peek,
        Area::Viewer,
        Area::Elements,
        Area::Browser,
        Area::Screenshots,
        Area::Recording,
        Area::Settings,
        Area::Release,
        Area::Performance,
    ];

    /// The name `--area` takes and the docs' headings use.
    pub fn name(self) -> &'static str {
        match self {
            Area::ViewerInput => "viewer-input",
            Area::InputMethod => "input-method",
            Area::AgentInput => "agent-input",
            Area::InputQueue => "input-queue",
            Area::Displays => "displays",
            Area::Windows => "windows",
            Area::WindowAccess => "window-access",
            Area::Takeover => "takeover",
            Area::Peek => "peek",
            Area::Viewer => "viewer",
            Area::Elements => "elements",
            Area::Browser => "browser",
            Area::Screenshots => "screenshots",
            Area::Recording => "recording",
            Area::Settings => "settings",
            Area::Release => "release",
            Area::Performance => "performance",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Area::ViewerInput => "the user's clicks, keys, drags and scrolls in the Viewer",
            Area::InputMethod => {
                "composing with an input method in the Viewer, and the agent's Keyboard Borrow"
            }
            Area::AgentInput => {
                "the agent's clicks, keys, typing and scrolls, sent in the background"
            }
            Area::InputQueue => {
                "Input Requests, the Input Queue, Input Grants and Input Loans, which use the user's real pointer"
            }
            Area::Displays => {
                "an Agent Display's life, from creation and parking to spares and review"
            }
            Area::Windows => "Placement, launch_app, and whose a new window is",
            Area::WindowAccess => "Window Requests, Handovers and Window Authorization",
            Area::Takeover => "Takeover, Help Requests, Control Requests and conversation control",
            Area::Peek => "Peek and the menu bar icon",
            Area::Viewer => "the Viewer's picture, header and Agent Pointer",
            Area::Elements => "get_window_state and act_on_element on menus, selects and controls",
            Area::Browser => "web pages in Chrome, Safari and Electron, and the DevTools pipe",
            Area::Screenshots => "the agent's screenshots",
            Area::Recording => "a Recording's video, sound and timeline",
            Area::Settings => "the Settings window and permission setup",
            Area::Release => "self-update and the release bundle",
            Area::Performance => "what displays, launches and input cost, in numbers",
        }
    }

    pub fn named(name: &str) -> Option<Area> {
        Area::ALL.iter().copied().find(|a| a.name() == name)
    }
}

/// When `suite` runs a script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The small set that gives fast confidence across every area.
    Core,
    /// Every script `suite` runs; the core tier is part of it.
    Full,
    /// Never run by `suite`: a tool, a measurement or recorder with no
    /// checks, or a script with needs of its own, which `note` names.
    Manual,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Core => "core",
            Tier::Full => "full",
            Tier::Manual => "manual",
        }
    }
}

/// How a script drives the path it checks. It takes the first that fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drive {
    /// Real HID events, or a real input method, on the path it checks:
    /// what the user's hands and the system do. Other steps of the script
    /// may still go through a debug method.
    Real,
    /// A debug control method plays the user's part, so the code between
    /// the user's hand and that method never runs, and a pass says nothing
    /// about it (#117).
    Hook,
    /// Only what an agent sends, over the control socket or MCP. For an
    /// agent's tools this is the real path.
    Agent,
}

impl Drive {
    pub fn name(self) -> &'static str {
        match self {
            Drive::Real => "real",
            Drive::Hook => "hook",
            Drive::Agent => "agent",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Script {
    /// The file in `src/bin`, without `.rs`.
    pub name: &'static str,
    /// What it covers, the main area first. Empty for a tool.
    pub areas: &'static [Area],
    pub tier: Tier,
    pub drive: Drive,
    /// It moves the user's pointer, takes their keyboard or input source,
    /// mutes their sound, or puts windows, alerts, cards or focus on their
    /// screen. Its sign asks the user to keep their hands off.
    pub hands_off: bool,
    /// `run` builds the debug app for it before the countdown.
    pub app: bool,
    /// The most displays it makes. `suite` holds it back until the display
    /// budget has room for that many.
    pub displays: usize,
    /// Environment the script refuses to run without.
    pub env: &'static [(&'static str, &'static str)],
    /// For the manual tier: why `suite` leaves it to the user.
    pub note: &'static str,
}

impl Script {
    const fn hands_off(mut self) -> Script {
        self.hands_off = true;
        self
    }

    const fn no_app(mut self) -> Script {
        self.app = false;
        self
    }

    const fn env(mut self, env: &'static [(&'static str, &'static str)]) -> Script {
        self.env = env;
        self
    }

    const fn manual(mut self, note: &'static str) -> Script {
        self.tier = Tier::Manual;
        self.note = note;
        self
    }

    pub fn covers(&self, area: Area) -> bool {
        self.areas.contains(&area)
    }
}

const fn script(
    name: &'static str,
    areas: &'static [Area],
    tier: Tier,
    drive: Drive,
    displays: usize,
) -> Script {
    Script {
        name,
        areas,
        tier,
        drive,
        hands_off: false,
        app: true,
        displays,
        env: &[],
        note: "",
    }
}

use Area::*;
use Drive::{Agent, Hook, Real};
use Tier::{Core, Full};

const MEASURES: &str = "a measurement: it prints numbers and checks nothing";
const NEEDS_APP: &str = "it needs a debug build already running, and starts none";

/// Every script, by name.
#[rustfmt::skip]
pub const SCRIPTS: &[Script] = &[
    script("agentshots", &[Screenshots], Core, Agent, 1),
    script("alerts", &[Windows], Full, Hook, 1).hands_off(),
    script("appwhere", &[Peek, WindowAccess], Full, Hook, 1).hands_off(),
    script("arrangement", &[Displays], Full, Agent, 2),
    script("asksave", &[Displays, Windows], Full, Hook, 1),
    script("barleak", &[Screenshots], Full, Agent, 1),
    script("browsers", &[Browser, AgentInput], Full, Agent, 1).hands_off(),
    script("changecost", &[Performance, Displays], Full, Agent, 1).manual(MEASURES),
    script("chrome", &[Browser, AgentInput], Full, Agent, 2).hands_off(),
    script("claims", &[Windows], Full, Hook, 1).hands_off(),
    script("closetwindow", &[WindowAccess, Windows], Full, Hook, 1).hands_off(),
    script("coldflash", &[Windows, Browser], Full, Agent, 1).hands_off(),
    script("compactstate", &[Elements], Core, Agent, 1),
    script("control", &[Takeover], Full, Hook, 1).no_app().manual(NEEDS_APP),
    script("conversation", &[Takeover, WindowAccess], Full, Hook, 1),
    script("cursormenu", &[Elements], Full, Agent, 1),
    script("demo", &[], Full, Hook, 3)
        .manual("it records the README's demo and writes docs/media/demo.gif"),
    script("displaychange", &[Peek, Displays], Full, Hook, 2).hands_off(),
    script("electron", &[AgentInput, Windows, Browser], Full, Agent, 1),
    script("endhandover", &[WindowAccess, Windows], Full, Hook, 3),
    script("filedrag", &[InputQueue, AgentInput, Browser], Full, Real, 1).hands_off(),
    script("firstclick", &[Browser, AgentInput], Full, Agent, 1),
    script("flash", &[Performance, Windows], Full, Agent, 1).hands_off().manual(MEASURES),
    script("handoverfront", &[WindowAccess, AgentInput], Full, Hook, 2),
    script("helpcard", &[Takeover], Full, Hook, 1).hands_off(),
    script("holdmode", &[Displays], Full, Agent, 1),
    script("ime", &[InputMethod, AgentInput], Core, Real, 1).hands_off(),
    script("inputqueue", &[InputQueue], Full, Hook, 1).hands_off(),
    script("keys", &[AgentInput, Browser], Full, Agent, 1).hands_off(),
    script("lastapp", &[Displays, Windows], Full, Agent, 1),
    script("latewindow", &[Windows], Full, Agent, 1).hands_off(),
    script("launchagain", &[Windows], Full, Agent, 1),
    script("launchhere", &[Windows, ViewerInput], Full, Hook, 2).hands_off(),
    script("load", &[Performance], Full, Agent, 12).hands_off().manual(MEASURES),
    script("loanfocus", &[InputQueue, Windows], Full, Hook, 1).hands_off(),
    script("menuchoice", &[Elements], Full, Hook, 1).hands_off(),
    script("menuclosed", &[Elements, Browser], Full, Agent, 1).hands_off(),
    script("menupress", &[Elements, InputQueue], Full, Real, 1).hands_off(),
    script("nativeclick", &[ViewerInput], Full, Hook, 2),
    script("oneclick", &[ViewerInput], Core, Real, 2).hands_off(),
    script("openpanel", &[AgentInput, Elements], Full, Agent, 1).hands_off(),
    script("parksafe", &[Displays], Full, Agent, 2),
    script("peekanim", &[Peek], Full, Hook, 2).hands_off(),
    script("peekhover", &[Peek], Core, Hook, 1).hands_off(),
    script("peekrapid", &[Peek, Displays], Full, Hook, 4).hands_off(),
    script("perf", &[Performance], Full, Agent, 6).manual(MEASURES),
    script("placebrowser", &[Windows, Browser], Full, Agent, 1),
    script("placeheld", &[Windows], Full, Agent, 2).hands_off(),
    script("placesize", &[Windows], Core, Agent, 1),
    script("pointer", &[Viewer], Full, Hook, 2)
        .manual("it records the Agent Pointer to judge by eye and checks nothing"),
    script("realclick", &[InputQueue, AgentInput], Core, Real, 1).hands_off(),
    script("realdrag", &[InputQueue], Full, Real, 1).hands_off(),
    script("recording", &[Recording], Core, Hook, 1),
    script("reopened", &[Windows], Full, Agent, 1).hands_off(),
    script("requestcontrol", &[Takeover], Full, Hook, 1).hands_off(),
    script("requestwindow", &[WindowAccess, Windows], Full, Hook, 2).hands_off(),
    script("restart", &[Displays, Windows], Full, Hook, 2).hands_off(),
    script("robust", &[AgentInput, Windows, Screenshots, Takeover], Core, Hook, 2).hands_off(),
    script("run", &[ViewerInput, Peek, Settings], Full, Real, 4).hands_off().manual(
        "it quits the installed Agent Screen and moves your pointer: run it when the user is away or asks",
    ),
    script("samebundle", &[Windows], Full, Agent, 1).hands_off(),
    script("scrolljump", &[AgentInput, Browser], Full, Agent, 1),
    script("secondwindow", &[Windows], Full, Agent, 1).hands_off(),
    script("selectvalue", &[Elements, Browser], Full, Agent, 1).hands_off(),
    script("selfupdate", &[Release], Full, Hook, 1).no_app().manual(
        "it needs SPARKLE_ED_KEY_FILE and builds the release bundle, which takes minutes",
    ),
    script("serials", &[Displays], Full, Agent, 6),
    script("settings", &[Settings], Core, Hook, 0).hands_off().env(&[("E2E_NOW", "1")]),
    script("shared", &[AgentInput], Full, Hook, 1).hands_off(),
    script("sharedbrowser", &[AgentInput, Browser, Elements], Full, Hook, 1).hands_off(),
    script("signupcontrols", &[Elements, Browser], Full, Agent, 1),
    script("smoke", &[AgentInput, Screenshots], Full, Agent, 1).no_app().manual(NEEDS_APP),
    script("sound", &[Recording], Full, Agent, 1).hands_off(),
    script("spares", &[Displays], Core, Agent, 2),
    script("sparesize", &[Displays], Full, Agent, 2),
    script("statusicon", &[Peek], Full, Agent, 1).hands_off(),
    script("stop", &[], Full, Agent, 0)
        .no_app()
        .manual("a tool: it quits a test instance a stopped script left running"),
    script("subtreestate", &[Elements], Full, Agent, 1),
    script("suite", &[], Full, Agent, 0).no_app().manual("a tool: it runs the others"),
    script("takeover", &[Takeover, ViewerInput], Core, Hook, 2),
    script("timing", &[Performance, Displays], Full, Agent, 11).hands_off().manual(MEASURES),
    script("typedrops", &[AgentInput, Browser], Full, Agent, 1).hands_off(),
    script("viewerdrag", &[ViewerInput], Full, Hook, 2),
    script("viewerheader", &[Viewer, Takeover], Full, Hook, 1).hands_off(),
    script("viewerime", &[InputMethod, ViewerInput], Core, Hook, 2),
    script("viewerkey", &[ViewerInput], Full, Hook, 2),
    script("viewerpicture", &[Viewer, Screenshots, Displays], Core, Hook, 3),
    script("viewerpopup", &[ViewerInput], Core, Real, 2).hands_off(),
    script("viewerwetype", &[InputMethod, ViewerInput], Core, Real, 1).hands_off(),
    script("waitchange", &[Elements], Full, Agent, 1),
    script("warmspare", &[Displays], Full, Agent, 3),
    script("webdrag", &[Browser, AgentInput, InputQueue, Elements], Core, Hook, 1).hands_off(),
    script("windowaccess", &[WindowAccess], Core, Hook, 2),
];

pub fn find(name: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|s| s.name == name)
}

/// Whether the script's sign asks the user to keep their hands off. A name
/// the manifest doesn't know asks too.
pub fn hands_off(name: &str) -> bool {
    find(name).is_none_or(|s| s.hands_off)
}

/// Whether `run` builds the debug app for the script.
pub fn needs_app(name: &str) -> bool {
    find(name).is_none_or(|s| s.app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn files() -> BTreeSet<String> {
        let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin");
        std::fs::read_dir(bin)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .collect()
    }

    /// A script without an entry would never be picked, and a renamed one
    /// would lose its hands-off sign.
    #[test]
    fn every_script_has_one_entry_and_every_entry_a_script() {
        let files = files();
        let entries: BTreeSet<String> = SCRIPTS.iter().map(|s| s.name.to_string()).collect();
        let unlisted: Vec<_> = files.difference(&entries).collect();
        let gone: Vec<_> = entries.difference(&files).collect();
        assert!(
            unlisted.is_empty(),
            "no manifest entry for src/bin/{unlisted:?}: add one to SCRIPTS in manifest.rs"
        );
        assert!(
            gone.is_empty(),
            "manifest entries with no file in src/bin: {gone:?}"
        );
        assert_eq!(entries.len(), SCRIPTS.len(), "a script is listed twice");
    }

    #[test]
    fn every_script_the_suite_runs_covers_an_area() {
        for s in SCRIPTS.iter().filter(|s| s.tier != Tier::Manual) {
            assert!(!s.areas.is_empty(), "{} covers no area", s.name);
        }
    }

    #[test]
    fn a_manual_script_says_why() {
        for s in SCRIPTS {
            assert_eq!(s.tier == Tier::Manual, !s.note.is_empty(), "{}", s.name);
        }
    }

    /// The core tier reaches every area the suite has a script for.
    #[test]
    fn the_core_tier_covers_every_area() {
        for &area in Area::ALL {
            let runs = |tier| SCRIPTS.iter().any(|s| s.tier == tier && s.covers(area));
            assert!(
                runs(Tier::Core) || !runs(Tier::Full),
                "no core script covers {}",
                area.name()
            );
        }
    }

    /// AGENTS.md asks for a line per script in docs/DEVELOPMENT.md. The
    /// list there is grouped by area: a `# area: …` line, then its scripts.
    #[test]
    fn the_docs_list_every_script_under_its_first_area() {
        let doc = std::fs::read_to_string(crate::root().join("docs/DEVELOPMENT.md")).unwrap();
        let mut area = None;
        let mut grouped = std::collections::BTreeMap::new();
        let mut mentioned = BTreeSet::new();
        for line in doc.lines() {
            if line.starts_with("```") {
                area = None;
            } else if let Some(heading) = line.strip_prefix("# ") {
                area = heading
                    .split_once(": ")
                    .and_then(|(name, _)| Area::named(name));
            } else if let Some(name) = line
                .split_once("cargo e2e ")
                .map(|(_, rest)| rest.split(|c: char| !c.is_ascii_alphanumeric()).next())
                // `cargo suite` is the alias for the suite script.
                .or_else(|| line.contains("cargo suite").then_some(Some("suite")))
            {
                let name = name.unwrap_or_default().to_string();
                if let Some(area) = area {
                    grouped.entry(name.clone()).or_insert(area);
                }
                mentioned.insert(name);
            }
        }
        for s in SCRIPTS {
            assert!(
                mentioned.contains(s.name),
                "docs/DEVELOPMENT.md has no `cargo e2e {}` line (`cargo suite` for the suite)",
                s.name
            );
            if let Some(&first) = s.areas.first() {
                assert_eq!(
                    grouped.get(s.name).map(|a| a.name()),
                    Some(first.name()),
                    "docs/DEVELOPMENT.md should list {} under `# {}: …`",
                    s.name,
                    first.name()
                );
            }
        }
    }

    #[test]
    fn area_names_round_trip() {
        for &area in Area::ALL {
            assert_eq!(Area::named(area.name()), Some(area));
        }
        assert_eq!(Area::named("nothing"), None);
    }

    /// The scripts' own doc comments say which need an instance or take the
    /// pointer; these are the facts `run` used to keep in two lists.
    #[test]
    fn the_sign_and_the_build_follow_the_manifest() {
        assert!(hands_off("oneclick"));
        assert!(!hands_off("takeover"));
        assert!(hands_off("a script nobody listed"));
        assert!(needs_app("takeover"));
        for name in ["control", "selfupdate", "smoke", "stop", "suite"] {
            assert!(!needs_app(name), "{name}");
        }
    }
}
