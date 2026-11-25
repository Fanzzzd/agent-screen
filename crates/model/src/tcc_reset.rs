//! Reset Permissions: the helper that clears Agent Screen's own privacy
//! entries once the app has quit, then opens it again at Settings.
//!
//! `tccutil reset <service>` without a bundle id clears that service for
//! every app on the Mac, so the bundle id is checked here before any
//! command exists: only Agent Screen's own ids pass, and they reach the
//! shell as arguments, never as script text.

use std::path::{Path, PathBuf};

/// The privacy services reset, as `tccutil` names them: Screen Recording,
/// Accessibility and Input Monitoring.
pub const SERVICES: [&str; 3] = ["ScreenCapture", "Accessibility", "ListenEvent"];

/// The release app, the development bundle `scripts/bundle.sh` builds.
pub const OUR_IDS: [&str; 2] = [
    "app.agentscreen.AgentScreen",
    "app.agentscreen.AgentScreen.dev",
];

/// Test copies of the app, as `app.agentscreen.e2e-ungranted`: this prefix
/// and a lowercase name.
pub const TEST_PREFIX: &str = "app.agentscreen.e2e-";

pub const TCCUTIL: &str = "/usr/bin/tccutil";
pub const OPEN: &str = "/usr/bin/open";

/// Why no helper was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// macOS gave no bundle id: resetting would clear every app's entries.
    NoBundleId,
    /// Not one of Agent Screen's ids.
    NotOurs(String),
    /// The bundle to reopen isn't an absolute `.app` path, or holds a
    /// character that has no place in one.
    BadBundle(PathBuf),
    /// No process to wait for.
    BadPid(u32),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoBundleId => write!(f, "this copy has no bundle id; nothing was reset"),
            Refused::NotOurs(id) => write!(
                f,
                "{id:?} is not Agent Screen's bundle id; nothing was reset"
            ),
            Refused::BadBundle(path) => write!(
                f,
                "{} is not an app bundle; nothing was reset",
                path.display()
            ),
            Refused::BadPid(pid) => write!(f, "no process {pid} to wait for"),
        }
    }
}

/// Whether `id` is Agent Screen's: one of `OUR_IDS` exactly, or a test
/// copy's.
pub fn is_ours(id: &str) -> bool {
    if OUR_IDS.contains(&id) {
        return true;
    }
    id.strip_prefix(TEST_PREFIX).is_some_and(|name| {
        (1..=40).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// The programs the helper runs: macOS's own, or a test's stand-ins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tools {
    pub tccutil: PathBuf,
    pub open: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Tools {
            tccutil: TCCUTIL.into(),
            open: OPEN.into(),
        }
    }
}

/// A `/bin/sh` command line: the script is fixed text, and everything
/// that varies comes in as its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// What it runs once `pid` has exited, each as an argv, for the log
    /// and the tests.
    pub steps: Vec<Vec<String>>,
}

/// Waits for $1 to exit, resets each service for $3 with $2, then opens
/// $5 with $4 at Settings.
const BODY: &str = r#"while /bin/kill -0 "$1" 2>/dev/null; do /bin/sleep 0.1; done
for service in ScreenCapture Accessibility ListenEvent; do "$2" reset "$service" "$3"; done
"$4" "$5" --args --setup"#;

/// The helper that resets `bundle_id`'s privacy entries once process `pid`
/// has quit, then reopens `bundle`. Refused unless `bundle_id` is Agent
/// Screen's own.
pub fn reset_commands(
    bundle_id: &str,
    bundle: &Path,
    pid: u32,
    tools: &Tools,
) -> Result<Script, Refused> {
    if bundle_id.trim().is_empty() {
        return Err(Refused::NoBundleId);
    }
    if !is_ours(bundle_id) {
        return Err(Refused::NotOurs(bundle_id.to_string()));
    }
    let bundle_text = bundle.to_str().unwrap_or_default();
    let sane = |path: &str| {
        path.starts_with('/') && !path.chars().any(|c| c.is_control()) && !path.contains("/../")
    };
    if !sane(bundle_text) || bundle.extension().is_none_or(|e| e != "app") {
        return Err(Refused::BadBundle(bundle.to_path_buf()));
    }
    if pid <= 1 {
        return Err(Refused::BadPid(pid));
    }
    let tccutil = tools.tccutil.to_string_lossy().into_owned();
    let open = tools.open.to_string_lossy().into_owned();
    let mut steps: Vec<Vec<String>> = SERVICES
        .iter()
        .map(|s| {
            vec![
                tccutil.clone(),
                "reset".into(),
                s.to_string(),
                bundle_id.into(),
            ]
        })
        .collect();
    steps.push(vec![
        open.clone(),
        bundle_text.into(),
        "--args".into(),
        "--setup".into(),
    ]);
    Ok(Script {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            BODY.into(),
            // $0, the name the shell reports errors under.
            "agent-screen-reset".into(),
            pid.to_string(),
            tccutil,
            bundle_id.into(),
            open,
            bundle_text.into(),
        ],
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "/Applications/Agent Screen.app";

    fn build(id: &str) -> Result<Script, Refused> {
        reset_commands(id, Path::new(BUNDLE), 4242, &Tools::default())
    }

    #[test]
    fn an_empty_or_blank_bundle_id_makes_no_command() {
        assert_eq!(build(""), Err(Refused::NoBundleId));
        assert_eq!(build("   "), Err(Refused::NoBundleId));
    }

    #[test]
    fn only_agent_screens_ids_pass() {
        for id in [
            "app.agentscreen.AgentScreen",
            "app.agentscreen.AgentScreen.dev",
            "app.agentscreen.e2e-ungranted",
            "app.agentscreen.e2e-reset-2",
        ] {
            assert!(build(id).is_ok(), "{id}");
        }
        for id in [
            "com.apple.Safari",
            "app.agentscreen",
            "app.agentscreen.",
            "app.agentscreen.colorsync",
            "app.agentscreen.AgentScreen.updtest",
            "app.agentscreen.AgentScreenX",
            "app.agentscreen.agentscreen",
            "app.agentscreen.e2e-",
            "app.agentscreen.e2e.field",
            "app.agentscreen.e2e-UPPER",
            " app.agentscreen.AgentScreen",
            "app.agentscreen.AgentScreen ",
            "app.agentscreen.AgentScreen\n",
            "app.agentscreen.AgentScreen;rm -rf ~",
            "app.agentscreen.e2e-a;b",
            "app.agentscreen.e2e-a b",
            "app.agentscreen.e2e-$(id)",
            "app.agentscreen.e2e-`id`",
            "app.agentscreen.e2e-a'b",
            "app.agentscreen.e2e-a\"b",
            "app.agentscreen.e2e-a|b",
            "app.agentscreen.e2e-a&b",
            "app.agentscreen.e2e-a*",
            "app.agentscreen.e2e-a\0",
        ] {
            assert!(
                matches!(build(id), Err(Refused::NotOurs(_))),
                "{id:?} must be refused"
            );
        }
        let long = format!("{TEST_PREFIX}{}", "a".repeat(41));
        assert!(build(&long).is_err());
    }

    #[test]
    fn the_steps_reset_three_services_for_that_id_then_reopen_settings() {
        let script = build("app.agentscreen.AgentScreen").unwrap();
        let id = "app.agentscreen.AgentScreen".to_string();
        assert_eq!(
            script.steps,
            vec![
                vec![
                    TCCUTIL.into(),
                    "reset".into(),
                    "ScreenCapture".into(),
                    id.clone()
                ],
                vec![
                    TCCUTIL.into(),
                    "reset".into(),
                    "Accessibility".into(),
                    id.clone()
                ],
                vec![TCCUTIL.into(), "reset".into(), "ListenEvent".into(), id],
                vec![
                    OPEN.into(),
                    BUNDLE.into(),
                    "--args".into(),
                    "--setup".into()
                ],
            ]
        );
        // Every tccutil step names the id: none resets every app.
        assert!(
            script.steps[..3]
                .iter()
                .all(|s| s.len() == 4 && s[3] == "app.agentscreen.AgentScreen")
        );
    }

    #[test]
    fn what_varies_is_an_argument_never_script_text() {
        let bundle = "/Users/me/Applications/Agent Screen $(x) `y`.app";
        let script = reset_commands(
            "app.agentscreen.AgentScreen",
            Path::new(bundle),
            4242,
            &Tools::default(),
        )
        .unwrap();
        assert_eq!(script.program, Path::new("/bin/sh"));
        assert_eq!(script.args[0], "-c");
        assert_eq!(script.args[1], BODY);
        assert!(!script.args[1].contains("agentscreen"));
        assert!(!script.args[1].contains("4242"));
        assert_eq!(
            &script.args[3..],
            ["4242", TCCUTIL, "app.agentscreen.AgentScreen", OPEN, bundle]
        );
        // The body quotes each argument it uses.
        for n in 1..=5 {
            let bare = format!(" ${n} ");
            assert!(!script.args[1].contains(&bare), "${n} unquoted");
        }
        assert!(script.args[1].contains("reset \"$service\" \"$3\""));
    }

    #[test]
    fn the_bundle_must_be_an_absolute_app_path() {
        for bundle in [
            "Agent Screen.app",
            "/Applications/Agent Screen",
            "/Applications/Agent\nScreen.app",
            "/Applications/../tmp/x.app",
            "",
        ] {
            assert!(
                matches!(
                    reset_commands(
                        "app.agentscreen.AgentScreen",
                        Path::new(bundle),
                        4242,
                        &Tools::default()
                    ),
                    Err(Refused::BadBundle(_))
                ),
                "{bundle:?}"
            );
        }
    }

    #[test]
    fn it_waits_for_a_real_process() {
        for pid in [0, 1] {
            assert_eq!(
                reset_commands(
                    "app.agentscreen.AgentScreen",
                    Path::new(BUNDLE),
                    pid,
                    &Tools::default()
                ),
                Err(Refused::BadPid(pid))
            );
        }
    }

    #[test]
    fn tests_swap_the_programs() {
        let tools = Tools {
            tccutil: "/tmp/fake-tccutil".into(),
            open: "/tmp/fake-open".into(),
        };
        let script = reset_commands(
            "app.agentscreen.e2e-reset",
            Path::new("/tmp/x/Agent Screen.app"),
            99,
            &tools,
        )
        .unwrap();
        assert_eq!(script.steps[0][0], "/tmp/fake-tccutil");
        assert_eq!(script.steps[3][0], "/tmp/fake-open");
    }
}
