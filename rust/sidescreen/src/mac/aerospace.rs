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

    /// Every workspace's monitor, what each monitor shows, and the focused
    /// workspace, in one call.
    pub fn layout(&self) -> Result<Layout, String> {
        let mut layout = Layout::default();
        let list = self.run(&[
            "list-workspaces",
            "--all",
            "--format",
            "%{workspace}|%{monitor-name}|%{workspace-is-visible}|%{workspace-is-focused}",
        ])?;
        for line in list.lines() {
            let parts: Vec<&str> = line.split('|').collect();
            let [workspace, monitor, visible, focused] = parts[..] else {
                continue;
            };
            layout
                .monitor_of
                .insert(workspace.to_string(), monitor.to_string());
            if visible == "true" {
                layout
                    .shown
                    .insert(monitor.to_string(), workspace.to_string());
            }
            if focused == "true" {
                layout.focused = Some(workspace.to_string());
            }
        }
        Ok(layout)
    }

    /// Note where the user's workspaces are now, for `restore`. Workspaces
    /// on Agent Displays keep the monitor noted before. Call it only when no
    /// Agent Display shows a workspace of the user's.
    pub fn remember(&self) {
        let Ok(now) = self.layout() else { return };
        self.keep(now);
    }

    /// `remember`, while no Agent Display is there to have taken anything:
    /// before the first one opens.
    pub fn remember_before_first(&self) {
        let Ok(now) = self.layout() else { return };
        if !now.monitor_of.values().any(|m| is_agent_display(m)) {
            self.keep(now);
        }
    }

    fn keep(&self, now: Layout) {
        let mut remembered = REMEMBERED.lock().unwrap();
        *remembered = Some(merged(remembered.take(), now));
    }

    /// Where the user had a workspace, as a monitor AeroSpace can move it
    /// to now: the remembered monitor if it is still there, else the main.
    pub fn home_of(&self, workspace: &str) -> String {
        let monitor = REMEMBERED
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|l| l.monitor_of.get(workspace).cloned());
        monitor
            .and_then(|name| self.monitor_id(&name).ok().flatten())
            .unwrap_or_else(|| "main".to_string())
    }

    /// After workspaces moved back from Agent Displays: show on each of the
    /// user's monitors the workspace it showed before, and focus the one
    /// that was focused.
    pub fn restore(&self) {
        let Some(before) = REMEMBERED.lock().unwrap().clone() else {
            return;
        };
        let Ok(now) = self.layout() else { return };
        for workspace in to_show(&before, &now) {
            log::info!("aerospace: showing workspace {workspace} where it was");
            let _ = self.focus_workspace(&workspace);
        }
        if let Some(focused) = before.focused
            && now.focused.as_ref() != Some(&focused)
            && !is_agent_display(before.monitor_of.get(&focused).map_or("", |m| m))
        {
            let _ = self.focus_workspace(&focused);
        }
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

// MARK: The user's workspaces

/// Where AeroSpace keeps workspaces, by monitor name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub monitor_of: std::collections::HashMap<String, String>,
    /// The workspace each monitor shows.
    pub shown: std::collections::HashMap<String, String>,
    pub focused: Option<String>,
}

static REMEMBERED: std::sync::Mutex<Option<Layout>> = std::sync::Mutex::new(None);

fn is_agent_display(monitor: &str) -> bool {
    monitor.starts_with("Sidescreen")
}

/// `now`, except that what sits on Agent Displays keeps its place in
/// `before`: AeroSpace put it there, the user didn't.
fn merged(before: Option<Layout>, now: Layout) -> Layout {
    let before = before.unwrap_or_default();
    let mut layout = Layout {
        focused: now
            .focused
            .clone()
            .filter(|w| !now.monitor_of.get(w).is_some_and(|m| is_agent_display(m)))
            .or(before.focused),
        ..Layout::default()
    };
    for (workspace, monitor) in now.monitor_of {
        let monitor = if is_agent_display(&monitor) {
            match before.monitor_of.get(&workspace) {
                Some(m) => m.clone(),
                None => continue,
            }
        } else {
            monitor
        };
        layout.monitor_of.insert(workspace, monitor);
    }
    for (monitor, workspace) in now.shown {
        if !is_agent_display(&monitor) {
            layout.shown.insert(monitor, workspace);
        }
    }
    layout
}

/// Workspaces to show again: each of the user's monitors showed one before
/// and shows another now, though the workspace is back on that monitor.
fn to_show(before: &Layout, now: &Layout) -> Vec<String> {
    let mut show: Vec<String> = before
        .shown
        .iter()
        .filter(|(monitor, workspace)| {
            now.shown.get(*monitor) != Some(*workspace)
                && now.monitor_of.get(*workspace) == Some(*monitor)
        })
        .map(|(_, workspace)| workspace.clone())
        .collect();
    show.sort();
    show
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
    let answer = read_ignoring();
    IGNORES.store(
        answer == Some(Ignoring::Yes),
        std::sync::atomic::Ordering::Relaxed,
    );
    answer
}

static IGNORES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether AeroSpace left Agent Displays alone when last checked: then
/// nothing needs moving away from it.
pub fn ignores_now() -> bool {
    IGNORES.load(std::sync::atomic::Ordering::Relaxed)
}

fn read_ignoring() -> Option<Ignoring> {
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
    configs().into_iter().next()
}

/// Every config file present. AeroSpace refuses to start with two.
pub fn configs() -> Vec<std::path::PathBuf> {
    [home().join(".aerospace.toml"), xdg_config_path()]
        .into_iter()
        .filter(|p| p.exists())
        .collect()
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
            return Some(open..open + list_end(&text[open..])?);
        }
        offset += line.len();
    }
    None
}

/// Where a TOML list of strings ends: the first `]` outside quotes.
fn list_end(text: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (Some(q), c) if c == q && !escaped => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, ']') => return Some(i),
            _ => {}
        }
        escaped = false;
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

/// What `ignore_agent_displays` did.
pub struct Ignored {
    pub config: std::path::PathBuf,
    /// The old config, kept beside it; None if there was none.
    pub backup: Option<std::path::PathBuf>,
}

/// Why `ignore_agent_displays` failed, and what the config is now.
pub struct NotIgnored {
    pub reason: String,
    /// The config as it was: nothing written, or the old one put back.
    pub unchanged: bool,
}

fn not_written(reason: impl Into<String>) -> NotIgnored {
    NotIgnored {
        reason: reason.into(),
        unchanged: true,
    }
}

/// Replace a file in one step, so AeroSpace never reads half of it.
fn replace(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.sidescreen"));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

/// Add Agent Displays to `ignored-monitors` and reload AeroSpace's config.
/// Keeps the old config beside it and puts it back if AeroSpace refuses the
/// new one. A user without a config gets AeroSpace's default config, which
/// is what AeroSpace runs without one. Blocking.
pub fn ignore_agent_displays() -> Result<Ignored, NotIgnored> {
    let aerospace = AeroSpace::current().ok_or_else(|| not_written("AeroSpace isn't running"))?;
    let present = configs();
    if present.len() > 1 {
        let names: Vec<String> = present.iter().map(|p| p.display().to_string()).collect();
        return Err(not_written(format!(
            "there are two configs, {}; AeroSpace reads neither until one is gone",
            names.join(" and ")
        )));
    }
    let (path, old) = match present.into_iter().next() {
        Some(path) => {
            let old = std::fs::read_to_string(&path).map_err(|e| not_written(e.to_string()))?;
            (path, Some(old))
        }
        None => (xdg_config_path(), None),
    };
    let base = match &old {
        Some(old) => old.clone(),
        None => {
            let bundle = super::app::running_bundle("bobko.aerospace")
                .ok_or_else(|| not_written("AeroSpace isn't running"))?;
            std::fs::read_to_string(bundle.join("Contents/Resources/default-config.toml"))
                .map_err(|e| not_written(format!("AeroSpace's default config: {e}")))?
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
            std::fs::write(&backup, old).map_err(|e| not_written(e.to_string()))?;
            Some(backup)
        }
        None => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| not_written(e.to_string()))?;
            }
            None
        }
    };
    replace(&path, &with_agent_displays_ignored(&base)).map_err(|e| not_written(e.to_string()))?;
    if let Err(refused) = aerospace.run(&["reload-config", "--no-gui"]) {
        let restored = match &old {
            Some(old) => replace(&path, old),
            None => std::fs::remove_file(&path),
        };
        let _ = aerospace.run(&["reload-config", "--no-gui"]);
        return Err(NotIgnored {
            reason: refused,
            unchanged: restored.is_ok(),
        });
    }
    IGNORES.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(Ignored {
        config: path,
        backup,
    })
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

    fn layout(rows: &[(&str, &str, bool)], focused: &str) -> Layout {
        let mut l = Layout {
            focused: Some(focused.to_string()),
            ..Layout::default()
        };
        for (w, m, shown) in rows {
            l.monitor_of.insert(w.to_string(), m.to_string());
            if *shown {
                l.shown.insert(m.to_string(), w.to_string());
            }
        }
        l
    }

    /// The case that hurt: workspace 11 showed on the Studio Display, an
    /// Agent Display took it, and the old guard moved it to the main monitor.
    #[test]
    fn puts_workspaces_back_where_they_were() {
        let before = layout(
            &[
                ("1", "Built-in", true),
                ("10", "Studio", false),
                ("11", "Studio", true),
            ],
            "1",
        );
        let remembered = merged(None, before.clone());
        assert_eq!(remembered, before);

        // AeroSpace shows 11 on the new display, and 10 on the Studio.
        let taken = layout(
            &[
                ("1", "Built-in", true),
                ("10", "Studio", true),
                ("11", "Sidescreen 1", true),
            ],
            "1",
        );
        let kept = merged(Some(remembered), taken.clone());
        assert_eq!(kept.monitor_of["11"], "Studio");
        assert_eq!(
            kept.shown["Studio"], "10",
            "what the user's monitor shows now"
        );
        assert!(!kept.shown.contains_key("Sidescreen 1"));

        // Moved back, 11 is on the Studio again but hidden behind 10.
        let back = layout(
            &[
                ("1", "Built-in", true),
                ("10", "Studio", true),
                ("11", "Studio", false),
            ],
            "1",
        );
        assert_eq!(to_show(&before, &back), vec!["11".to_string()]);
        assert!(to_show(&before, &before).is_empty());
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

        let bracket = with_agent_displays_ignored("ignored-monitors = ['DELL [0-9]']\n");
        assert_eq!(
            bracket,
            "ignored-monitors = ['^Sidescreen', 'DELL [0-9]']\n"
        );
        assert!(!ignores_agent_displays(
            "ignored-monitors = ['DELL [0-9]']\n"
        ));

        let empty = with_agent_displays_ignored("ignored-monitors = []\n");
        assert_eq!(empty, "ignored-monitors = ['^Sidescreen']\n");
    }
}
