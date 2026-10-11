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
