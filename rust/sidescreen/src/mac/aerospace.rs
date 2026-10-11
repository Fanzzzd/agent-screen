//! The `aerospace` command line client.
//!
//! Every call blocks, so run them off the main thread: AeroSpace answers by
//! querying every app's windows over AX, ours included, and blocking the
//! main thread on it deadlocks until AX times out. A stuck CLI is killed
//! after 5 s.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct AeroSpace {
    executable: &'static str,
}

impl AeroSpace {
    /// Some while AeroSpace is running and its CLI is installed.
    pub fn current() -> Option<AeroSpace> {
        if !super::app::is_running("bobko.aerospace") {
            return None;
        }
        ["/opt/homebrew/bin/aerospace", "/usr/local/bin/aerospace"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
            .map(|executable| AeroSpace { executable })
    }

    pub fn move_to_monitor(&self, window: u32, monitor: &str) -> Result<(), String> {
        let workspace = self
            .visible_workspace(monitor)?
            .ok_or_else(|| format!("no visible workspace on monitor {monitor}"))?;
        log::debug!("aerospace: window {window} to workspace {workspace} on monitor {monitor}");
        self.move_window(window, &workspace)
    }

    pub fn move_window(&self, window: u32, workspace: &str) -> Result<(), String> {
        self.run(&[
            "move-node-to-workspace",
            "--window-id",
            &window.to_string(),
            workspace,
        ])
        .map(|_| ())
    }

    /// The workspace a window belongs to.
    pub fn workspace_of(&self, window: u32) -> Result<Option<String>, String> {
        let id = window.to_string();
        Ok(self
            .run(&[
                "list-windows",
                "--all",
                "--format",
                "%{window-id}|%{workspace}",
            ])?
            .lines()
            .filter_map(|line| line.split_once('|'))
            .find(|(w, _)| *w == id)
            .map(|(_, workspace)| workspace.to_string()))
    }

    /// Windows in the workspace shown on the monitor named `name`, in one
    /// call: right after a display change AeroSpace answers each call late.
    pub fn visible_windows_on(&self, name: &str) -> Result<Vec<u32>, String> {
        Ok(self
            .run(&[
                "list-windows",
                "--all",
                "--format",
                "%{window-id}|%{monitor-name}|%{workspace-is-visible}",
            ])?
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '|');
                let (id, monitor, visible) = (parts.next()?, parts.next()?, parts.next()?);
                (monitor == name && visible == "true")
                    .then(|| id.parse().ok())
                    .flatten()
            })
            .collect())
    }

    /// The monitor AeroSpace files a window under, and whether its workspace
    /// shows there, from one snapshot. None for a window AeroSpace doesn't
    /// manage, such as a panel.
    pub fn whereabouts(&self, window: u32) -> Result<Option<(String, bool)>, String> {
        let id = window.to_string();
        Ok(self
            .run(&[
                "list-windows",
                "--all",
                "--format",
                "%{window-id}|%{monitor-name}|%{workspace-is-visible}",
            ])?
            .lines()
            .find_map(|line| {
                let mut parts = line.splitn(3, '|');
                (parts.next()? == id).then(|| {
                    (
                        parts.next().unwrap_or("").to_string(),
                        parts.next() == Some("true"),
                    )
                })
            }))
    }

    pub fn monitor_id(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .run(&["list-monitors", "--format", "%{monitor-id}|%{monitor-name}"])?
            .lines()
            .filter_map(|line| line.split_once('|'))
            .find(|(_, n)| *n == name)
            .map(|(id, _)| id.to_string()))
    }

    pub fn visible_workspace(&self, monitor: &str) -> Result<Option<String>, String> {
        let name = self.run(&["list-workspaces", "--monitor", monitor, "--visible"])?;
        let name = name.trim();
        Ok((!name.is_empty()).then(|| name.to_string()))
    }

    pub fn focused_workspace(&self) -> Result<Option<String>, String> {
        let name = self.run(&["list-workspaces", "--focused"])?;
        let name = name.trim();
        Ok((!name.is_empty()).then(|| name.to_string()))
    }

    pub fn window_ids(&self, workspace: &str) -> Result<Vec<u32>, String> {
        Ok(self
            .run(&[
                "list-windows",
                "--workspace",
                workspace,
                "--format",
                "%{window-id}",
            ])?
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect())
    }

    pub fn move_workspace(&self, workspace: &str, monitor: &str) -> Result<(), String> {
        log::debug!("aerospace: workspace {workspace} to monitor {monitor}");
        self.run(&[
            "move-workspace-to-monitor",
            "--workspace",
            workspace,
            monitor,
        ])
        .map(|_| ())
    }

    pub fn focus_workspace(&self, workspace: &str) -> Result<(), String> {
        self.run(&["workspace", workspace]).map(|_| ())
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut child = Command::new(self.executable)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        // Drain stdout while waiting, so a long window list cannot fill the pipe.
        let reader = child.stdout.take().map(|mut stdout| {
            std::thread::spawn(move || {
                let mut output = String::new();
                let _ = stdout.read_to_string(&mut output);
                output
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(format!(
                    "aerospace {} timed out",
                    args.first().unwrap_or(&"")
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let output = reader.and_then(|r| r.join().ok()).unwrap_or_default();
        if !status.success() {
            let mut errors = String::new();
            child
                .stderr
                .take()
                .map(|mut s| s.read_to_string(&mut errors));
            return Err(errors.trim().to_string());
        }
        Ok(output)
    }
}

// MARK: ignored-monitors

/// Whether AeroSpace leaves Agent Displays alone, through its
/// `ignored-monitors` option (docs/adr/0006).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ignoring {
    /// The config ignores Agent Displays.
    Yes,
    /// The running AeroSpace has the option and the config doesn't use it.
    Available,
    /// The running AeroSpace has no such option, as stock AeroSpace.
    Unsupported,
}

/// What every Agent Display's name starts with.
const DISPLAY_NAME: &str = "Sidescreen 1";
const PATTERN: &str = "'^Sidescreen'";

/// None while AeroSpace isn't running. Reads AeroSpace's binary once per
/// build, about 20 MB: run it off the main thread.
pub fn ignoring() -> Option<Ignoring> {
    let bundle = super::app::running_bundle("bobko.aerospace")?;
    if !supports_ignored_monitors(&bundle.join("Contents/MacOS/AeroSpace")) {
        return Some(Ignoring::Unsupported);
    }
    let ignores = config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| ignores_agent_displays(&text));
    Some(if ignores {
        Ignoring::Yes
    } else {
        Ignoring::Available
    })
}

fn supports_ignored_monitors(binary: &std::path::Path) -> bool {
    use std::sync::Mutex;
    static SEEN: Mutex<Option<(std::path::PathBuf, std::time::SystemTime, bool)>> =
        Mutex::new(None);
    let modified = std::fs::metadata(binary).and_then(|m| m.modified()).ok();
    let mut seen = SEEN.lock().unwrap();
    if let (Some((path, at, answer)), Some(modified)) = (seen.as_ref(), modified)
        && path == binary
        && *at == modified
    {
        return *answer;
    }
    let needle = b"ignored-monitors";
    let answer =
        std::fs::read(binary).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle));
    if let Some(modified) = modified {
        *seen = Some((binary.to_path_buf(), modified, answer));
    }
    answer
}

/// The config AeroSpace reads: `~/.aerospace.toml`, else the one under
/// `$XDG_CONFIG_HOME`, by default `~/.config`.
pub fn config_path() -> Option<std::path::PathBuf> {
    [
        Some(home().join(".aerospace.toml")),
        Some(xdg_config_path()),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.exists())
}

fn home() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn xdg_config_path() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"))
        .join("aerospace/aerospace.toml")
}

/// The list of `ignored-monitors = [...]` in a config, as its span in
/// `text` between the brackets.
fn ignored_monitors_list(text: &str) -> Option<std::ops::Range<usize>> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let key = line.trim_start();
        if let Some(rest) = key.strip_prefix("ignored-monitors")
            && rest.trim_start().starts_with('=')
        {
            let open = offset + line.find('[')? + 1;
            let close = open + text[open..].find(']')?;
            return Some(open..close);
        }
        offset += line.len();
    }
    None
}

/// Whether a config's `ignored-monitors` matches Agent Displays. AeroSpace
/// takes each entry as a case-insensitive regex searched in the name.
pub fn ignores_agent_displays(text: &str) -> bool {
    let Some(list) = ignored_monitors_list(text) else {
        return false;
    };
    let entries = regex::Regex::new(r#"'([^']*)'|"((?:[^"\\]|\\.)*)""#).unwrap();
    entries.captures_iter(&text[list]).any(|c| {
        let pattern = c.get(1).or(c.get(2)).map_or("", |m| m.as_str());
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .is_ok_and(|r| r.is_match(DISPLAY_NAME))
    })
}

/// A config with Agent Displays added to `ignored-monitors`.
pub fn with_agent_displays_ignored(text: &str) -> String {
    if let Some(list) = ignored_monitors_list(text) {
        let separator = if text[list.clone()].trim().is_empty() {
            ""
        } else {
            ", "
        };
        return format!(
            "{}{PATTERN}{separator}{}",
            &text[..list.start],
            &text[list.start..]
        );
    }
    // A top-level key: before any table, so at the top.
    format!(
        "# Sidescreen's virtual displays are for AI agents: leave them alone.\n\
         ignored-monitors = [{PATTERN}]\n\n{text}"
    )
}

/// Add Agent Displays to `ignored-monitors` and reload AeroSpace's config.
/// Keeps the old config beside it and puts it back if AeroSpace refuses the
/// new one. A user without a config gets AeroSpace's default config, which
/// is what AeroSpace runs without one. Blocking; the config's path on
/// success, with the copy's path if there was a config.
pub fn ignore_agent_displays() -> Result<(std::path::PathBuf, Option<std::path::PathBuf>), String> {
    let aerospace = AeroSpace::current().ok_or("AeroSpace isn't running")?;
    let (path, old) = match config_path() {
        Some(path) => {
            let old = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            (path, Some(old))
        }
        None => (xdg_config_path(), None),
    };
    let base = match &old {
        Some(old) => old.clone(),
        None => {
            let bundle =
                super::app::running_bundle("bobko.aerospace").ok_or("AeroSpace isn't running")?;
            std::fs::read_to_string(bundle.join("Contents/Resources/default-config.toml"))
                .map_err(|e| format!("AeroSpace's default config: {e}"))?
        }
    };
    let backup = match &old {
        Some(old) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let mut backup = path.with_file_name(format!("{name}.before-sidescreen"));
            let mut n = 2;
            while backup.exists() {
                backup = path.with_file_name(format!("{name}.before-sidescreen-{n}"));
                n += 1;
            }
            std::fs::write(&backup, old).map_err(|e| e.to_string())?;
            Some(backup)
        }
        None => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            None
        }
    };
    std::fs::write(&path, with_agent_displays_ignored(&base)).map_err(|e| e.to_string())?;
    if let Err(refused) = aerospace.run(&["reload-config", "--no-gui"]) {
        let _ = match &old {
            Some(old) => std::fs::write(&path, old),
            None => std::fs::remove_file(&path),
        };
        let _ = aerospace.run(&["reload-config", "--no-gui"]);
        return Err(refused);
    }
    Ok((path, backup))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_agent_displays_in_ignored_monitors() {
        assert!(ignores_agent_displays(
            "ignored-monitors = ['^Sidescreen']\n"
        ));
        assert!(ignores_agent_displays(
            "ignored-monitors = [\n  'dummy',\n  \"sidescreen\",\n]\n"
        ));
        assert!(!ignores_agent_displays("ignored-monitors = ['dummy']\n"));
        assert!(!ignores_agent_displays(
            "# ignored-monitors = ['^Sidescreen']\n"
        ));
        assert!(!ignores_agent_displays("gaps.inner.horizontal = 8\n"));
    }

    /// Reads the AeroSpace running on this Mac; changes nothing.
    #[test]
    #[ignore]
    fn reads_the_running_aerospace() {
        println!("ignoring: {:?}, config: {:?}", ignoring(), config_path());
    }

    #[test]
    fn adds_agent_displays_to_a_config() {
        let fresh =
            with_agent_displays_ignored("start-at-login = true\n\n[gaps]\ninner.horizontal = 8\n");
        assert!(fresh.contains("ignored-monitors = ['^Sidescreen']\n\nstart-at-login"));
        assert!(ignores_agent_displays(&fresh));

        let listed = with_agent_displays_ignored("ignored-monitors = ['dummy']\n");
        assert_eq!(listed, "ignored-monitors = ['^Sidescreen', 'dummy']\n");

        let empty = with_agent_displays_ignored("ignored-monitors = []\n");
        assert_eq!(empty, "ignored-monitors = ['^Sidescreen']\n");
    }
}
