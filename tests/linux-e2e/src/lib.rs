//! Shared helpers for Linux end-to-end tests: MCP JSON-RPC stdio client,
//! control socket debug client, simulated host `sway`, and host safety checks.

#![cfg(target_os = "linux")]

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

pub fn agent_screen_bin() -> PathBuf {
    if let Ok(p) = std::env::var("AGENTSCREEN_LINUX_BIN") {
        return PathBuf::from(p);
    }
    let test_exe = std::env::current_exe().expect("current_exe");
    let target_dir = test_exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target profile dir");
    let bin = target_dir.join("agent-screen");
    if !bin.is_file() {
        let status = Command::new("cargo")
            .args(["build", "-q", "-p", "agent-screen-linux"])
            .current_dir(workspace_root())
            .status()
            .expect("cargo build agent-screen-linux");
        assert!(status.success(), "building agent-screen-linux failed");
    }
    bin
}

pub struct McpStdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpStdioClient {
    pub fn spawn(
        xdg_runtime_dir: &Path,
        state_home: &Path,
        instance: &str,
        extra_env: &[(&str, &str)],
    ) -> Result<Self> {
        let bin = agent_screen_bin();
        let mut cmd = Command::new(&bin);
        cmd.arg("mcp")
            .env("AGENTSCREEN_HEADLESS", "1")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env("XDG_RUNTIME_DIR", xdg_runtime_dir)
            .env("XDG_STATE_HOME", state_home)
            .env("AGENTSCREEN_INSTANCE", instance)
            .env("AGENTSCREEN_DAEMON", &bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {} mcp", bin.display()))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        let _init = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "linux-e2e", "version": "0.1.0" }
            }),
        )?;
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}")?;
        self.stdin.flush()?;
        Ok(())
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}")?;
        self.stdin.flush()?;
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line)?;
        if n == 0 {
            bail!("MCP server closed stdout on {method}");
        }
        let resp: Value = serde_json::from_str(line.trim_end())?;
        if let Some(err) = resp.get("error") {
            bail!("MCP JSON-RPC error on {method}: {err}");
        }
        Ok(resp["result"].clone())
    }

    pub fn list_tools(&mut self) -> Result<Vec<Value>> {
        let res = self.request("tools/list", json!({}))?;
        Ok(res["tools"].as_array().cloned().unwrap_or_default())
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.call_tool_with_session(name, arguments, "e2e-session-main")
    }

    pub fn send_tool_call(&mut self, name: &str, arguments: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
                "_meta": { "session_id": "e2e-session-main" }
            }
        });
        writeln!(self.stdin, "{msg}")?;
        self.stdin.flush()?;
        Ok(id)
    }

    pub fn read_tool_reply(&mut self, expected_id: u64) -> Result<Value> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line)?;
        if n == 0 {
            bail!("MCP server closed stdout waiting for reply {expected_id}");
        }
        let resp: Value = serde_json::from_str(line.trim_end())?;
        assert_eq!(resp["id"].as_u64(), Some(expected_id));
        if let Some(err) = resp.get("error") {
            bail!("MCP JSON-RPC error: {err}");
        }
        Ok(resp["result"].clone())
    }

    pub fn call_tool_with_session(
        &mut self,
        name: &str,
        arguments: Value,
        session_id: &str,
    ) -> Result<Value> {
        let res = self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
                "_meta": { "session_id": session_id }
            }),
        )?;
        if res.get("isError") == Some(&Value::Bool(true)) {
            let msg = res["content"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|c| c["text"].as_str())
                .unwrap_or("tool error");
            bail!("tool {name} failed: {msg}");
        }
        Ok(res)
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub async fn control_call(socket_path: &Path, method: &str, params: Value) -> Result<Value> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connect control socket {}", socket_path.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_vec(&json!({
        "id": 1,
        "method": method,
        "params": params,
    }))?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;

    let mut lines = tokio::io::BufReader::new(reader).lines();
    let reply_line = lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow!("EOF from control socket"))?;
    let reply: Value = serde_json::from_str(&reply_line)?;
    if let Some(err) = reply.get("error") {
        bail!("control {method} failed: {err}");
    }
    Ok(reply["result"].clone())
}

pub struct SimulatedHostSway {
    pub dir: PathBuf,
    pub sway_socket: PathBuf,
    pub wayland_socket: PathBuf,
    sway_pid: u32,
    host_app: Option<Child>,
}

impl SimulatedHostSway {
    pub fn start(base: &Path) -> Result<Self> {
        let dir = base.join("sim-host-runtime");
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let sway_socket = dir.join("host-sway.sock");
        let ready = dir.join("ready.env");
        let cfg = base.join("host-sway.cfg");
        std::fs::write(
            &cfg,
            format!(
                "xwayland disable\n\
                 output HEADLESS-1 resolution 1024x768 position 0,0 scale 1\n\
                 for_window [all] floating enable\n\
                 bar {{\n  mode invisible\n}}\n\
                 exec sh -c 'printf \"%s\" \"$WAYLAND_DISPLAY\" > {:?}'\n",
                ready.display()
            ),
        )?;

        let home = std::env::var("HOME").unwrap_or_default();
        let path = std::env::var("PATH")
            .unwrap_or_else(|_| format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin"));
        let mut cmd = Command::new("sway");
        cmd.env_clear()
            .env("HOME", &home)
            .env("PATH", &path)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("WLR_RENDERER", "pixman")
            .env("SWAYSOCK", &sway_socket)
            .arg("-c")
            .arg(&cfg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn().context("spawn simulated host sway")?;
        let sway_pid = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });

        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && !ready.exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        let wl_name = std::fs::read_to_string(&ready).context("simulated host sway ready")?;
        let wayland_socket = dir.join(wl_name.trim());
        Ok(Self {
            dir,
            sway_socket,
            wayland_socket,
            sway_pid,
            host_app: None,
        })
    }

    pub fn launch_host_gtk_window(
        &mut self,
        fixture_script: &Path,
        state_file: &Path,
    ) -> Result<()> {
        let wl_name = self
            .wayland_socket
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("wayland-1");
        let child = Command::new("python3")
            .arg(fixture_script)
            .arg(state_file)
            .arg("Host User Window")
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("WAYLAND_DISPLAY", wl_name)
            .env("GDK_BACKEND", "wayland")
            .env("GTK_USE_PORTAL", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn host GTK4 window")?;
        self.host_app = Some(child);

        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if self.focused_window_title().as_deref() == Some("Host User Window") {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("host GTK4 window did not gain focus on simulated host sway")
    }

    pub fn wayland_display_name(&self) -> String {
        self.wayland_socket
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("wayland-1")
            .to_string()
    }

    pub fn focused_window_title(&self) -> Option<String> {
        let out = Command::new("swaymsg")
            .arg("-s")
            .arg(&self.sway_socket)
            .args(["-t", "get_tree"])
            .output()
            .ok()?;
        let tree: Value = serde_json::from_slice(&out.stdout).ok()?;
        find_focused_title(&tree)
    }

    pub fn sway_tree(&self) -> Option<Value> {
        let out = Command::new("swaymsg")
            .arg("-s")
            .arg(&self.sway_socket)
            .args(["-t", "get_tree"])
            .output()
            .ok()?;
        serde_json::from_slice(&out.stdout).ok()
    }
}

impl Drop for SimulatedHostSway {
    fn drop(&mut self) {
        if let Some(mut app) = self.host_app.take() {
            let _ = app.kill();
            let _ = app.wait();
        }
        let _ = Command::new("swaymsg")
            .arg("-s")
            .arg(&self.sway_socket)
            .arg("exit")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        unsafe {
            let _ = libc::kill(-(self.sway_pid as i32), libc::SIGTERM);
        }
    }
}

fn find_focused_title(node: &Value) -> Option<String> {
    if node.get("focused") == Some(&Value::Bool(true))
        && node.get("pid").and_then(Value::as_i64).unwrap_or(0) > 0
    {
        return node.get("name").and_then(Value::as_str).map(String::from);
    }
    for key in ["floating_nodes", "nodes"] {
        if let Some(arr) = node.get(key).and_then(Value::as_array) {
            for child in arr {
                if let Some(found) = find_focused_title(child) {
                    return Some(found);
                }
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalDesktopCheck {
    pub atspi_bus0_inode: Option<u64>,
    pub hypr_cursor: Option<String>,
    pub hypr_active: Option<String>,
}

pub fn snapshot_physical_desktop() -> PhysicalDesktopCheck {
    let real_xdg = Path::new("/run/user/1000");
    let atspi_bus0_inode = std::fs::symlink_metadata(real_xdg.join("at-spi/bus_0"))
        .ok()
        .map(|m| m.ino());
    let hypr_sig = std::fs::read_dir(real_xdg.join("hypr"))
        .ok()
        .and_then(|mut it| it.find_map(|e| e.ok()))
        .map(|e| e.file_name().to_string_lossy().into_owned());
    let (hypr_cursor, hypr_active) = if let Some(sig) = hypr_sig {
        let cursor = Command::new("hyprctl")
            .env("XDG_RUNTIME_DIR", real_xdg)
            .env("HYPRLAND_INSTANCE_SIGNATURE", &sig)
            .arg("cursorpos")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let active = Command::new("hyprctl")
            .env("XDG_RUNTIME_DIR", real_xdg)
            .env("HYPRLAND_INSTANCE_SIGNATURE", &sig)
            .args(["activewindow", "-j"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .map(|v| format!("{}:{}", v["address"], v["pid"]));
        (cursor, active)
    } else {
        (None, None)
    };
    PhysicalDesktopCheck {
        atspi_bus0_inode,
        hypr_cursor,
        hypr_active,
    }
}

pub struct IsolatedTestEnv {
    pub scratch: PathBuf,
    pub runtime_dir: PathBuf,
    pub state_home: PathBuf,
    pub instance: String,
    pub control_sock: PathBuf,
    pub web_state_file: PathBuf,
}

impl IsolatedTestEnv {
    pub fn new(tag: &str) -> Result<Self> {
        let scratch = PathBuf::from(format!("/tmp/as-linux-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch)?;

        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| scratch.join("xdg-runtime"));
        std::fs::create_dir_all(&runtime_dir)?;
        let state_home = scratch.join("state-home");
        std::fs::create_dir_all(&state_home)?;
        let instance = format!("e2e-{tag}-{}", std::process::id());
        let as_state = runtime_dir.join("agent-screen");
        let control_sock = as_state.join(format!("control-{instance}.sock"));
        let web_state_file = as_state.join(format!("web-{instance}.json"));

        Ok(Self {
            scratch,
            runtime_dir,
            state_home,
            instance,
            control_sock,
            web_state_file,
        })
    }

    pub async fn cleanup(&self) {
        let _ = control_call(&self.control_sock, "debugQuit", json!({})).await;
        tokio::time::sleep(Duration::from_millis(120)).await;
        let _ = std::fs::remove_file(&self.control_sock);
        let _ = std::fs::remove_file(&self.web_state_file);
        let _ = std::fs::remove_dir_all(
            self.runtime_dir
                .join("agent-screen")
                .join(format!("displays-{}", self.instance)),
        );
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

pub fn first_text_block(res: &Value) -> String {
    res["content"]
        .as_array()
        .and_then(|arr| arr.iter().find(|c| c["type"] == "text"))
        .and_then(|c| c["text"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Find the `[n]` element number of the first tree line matching `needle`.
pub fn find_element_id(tree_text: &str, needle: &str) -> Option<i64> {
    for line in tree_text.lines() {
        if !line.contains(needle) {
            continue;
        }
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix('[')?;
        let (num, _) = rest.split_once(']')?;
        if let Ok(n) = num.parse::<i64>() {
            return Some(n);
        }
    }
    None
}

/// Parse `at (cx,cy)` from the first tree line matching `needle`.
pub fn find_element_center(tree_text: &str, needle: &str) -> Option<(i64, i64)> {
    for line in tree_text.lines() {
        if !line.contains(needle) {
            continue;
        }
        let idx = line.rfind(" at (")?;
        let coords = &line[idx + 5..];
        let end = coords.find(')')?;
        let (xs, ys) = coords[..end].split_once(',')?;
        let x = xs.trim().parse::<i64>().ok()?;
        let y = ys.trim().parse::<i64>().ok()?;
        return Some((x, y));
    }
    None
}
