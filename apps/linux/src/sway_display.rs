//! Isolated per-display headless `sway` compositor managed inside a
//! `systemd --user` transient scope (with `setsid` fallback in containers),
//! direct Sway IPC client, window placement, and crash-surviving reattachment.

use crate::dbus_stubs::{
    NotificationStats, set_xwayland_atspi_bus, spawn_silent_notifications,
    write_isolated_dbus_config,
};
use crate::launcher::resolve_launch_command;
use crate::wayland_worker::{RawRgbaFrame, WaylandWorker};
use agent_screen_app_core::devtools_pipe;
use agent_screen_app_core::ports::{
    ClipboardPayload, ClipboardSync, Display, DisplayManager, DisplayMetrics, DisplaySpec,
    EncodedImage, FramePatch, InputOp, InputOutcome, LaunchRequest, LaunchedApp, PlaceOutcome,
    PortError, RecoveredDisplay, SnapshotOpts, WindowInfo,
};
use agent_screen_mcp::capabilities::Capabilities;
use agent_screen_model::model::{DisplayId, Rect, WindowTarget};
use agent_screen_model::session::Orphans;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const IPC_MAGIC: &[u8; 6] = b"i3-ipc";
const IPC_RUN_COMMAND: u32 = 0;
const IPC_GET_TREE: u32 = 4;
const IPC_GET_VERSION: u32 = 7;

pub fn shot_scale((w, h): (u32, u32)) -> f64 {
    let longest = w.max(h).max(1) as f64;
    (1568.0 / longest).min(1.0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedDisplay {
    pub display_id: DisplayId,
    pub width: u32,
    pub height: u32,
    pub session_key: String,
    pub client: String,
    pub title: String,
    pub scope_unit: Option<String>,
    pub sway_pid: u32,
    pub base_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub wayland_socket: PathBuf,
    pub sway_socket: PathBuf,
    pub x11_display: String,
    pub session_bus_address: String,
    pub atspi_bus_address: String,
    pub started_pids: Vec<i32>,
}

pub struct SwayDisplay {
    meta: Mutex<PersistedDisplay>,
    worker: WaylandWorker,
    _notifications_conn: zbus::Connection,
    _notifications_stats: Arc<NotificationStats>,
}

impl SwayDisplay {
    #[allow(dead_code)]
    pub async fn capture_raw(&self) -> Result<RawRgbaFrame, PortError> {
        self.worker.capture_raw().await
    }

    fn snapshot_meta(&self) -> PersistedDisplay {
        self.meta.lock().unwrap().clone()
    }

    fn persist_meta(&self) {
        let meta = self.snapshot_meta();
        let _ = write_state_atomic(&meta);
    }
}

impl Display for SwayDisplay {
    fn id(&self) -> DisplayId {
        self.meta.lock().unwrap().display_id
    }

    fn metrics(&self) -> DisplayMetrics {
        let m = self.meta.lock().unwrap();
        DisplayMetrics {
            width: m.width,
            height: m.height,
            scale: shot_scale((m.width, m.height)),
        }
    }

    fn atspi_bus_address(&self) -> String {
        self.meta.lock().unwrap().atspi_bus_address.clone()
    }

    fn session_bus_address(&self) -> String {
        self.meta.lock().unwrap().session_bus_address.clone()
    }

    fn runtime_dir(&self) -> PathBuf {
        self.meta.lock().unwrap().runtime_dir.clone()
    }

    fn wayland_socket(&self) -> PathBuf {
        self.meta.lock().unwrap().wayland_socket.clone()
    }

    fn settle(&self) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async move { self.worker.settle().await })
    }

    fn capture_raw(&self) -> BoxFuture<'_, Result<RawRgbaFrame, PortError>> {
        Box::pin(async move { self.worker.capture_raw().await })
    }

    fn screenshot(&self, opts: SnapshotOpts) -> BoxFuture<'_, Result<EncodedImage, PortError>> {
        let (w, h) = {
            let m = self.meta.lock().unwrap();
            (m.width, m.height)
        };
        Box::pin(async move { self.worker.screenshot((w, h), opts).await })
    }

    fn act_input(&self, op: InputOp) -> BoxFuture<'_, Result<InputOutcome, PortError>> {
        let (extent, sway_sock) = {
            let m = self.meta.lock().unwrap();
            ((m.width, m.height), m.sway_socket.clone())
        };
        Box::pin(async move {
            let click_pt = match &op {
                InputOp::Click { point, .. } => Some(*point),
                _ => None,
            };
            let drag_op = matches!(op, InputOp::Drag { .. });

            let hit_window = if let Some(pt) = click_pt {
                let wins = list_sway_windows(&sway_sock).await.unwrap_or_default();
                wins.into_iter()
                    .find(|w| w.bounds.contains(pt))
                    .map(|w| w.target.window_id)
            } else {
                None
            };

            self.worker.act_input(op, extent).await?;

            Ok(InputOutcome {
                window_id: hit_window,
                route: drag_op.then_some("wayland_seat"),
                note: None,
            })
        })
    }

    fn clipboard(&self, op: ClipboardSync) -> BoxFuture<'_, Result<ClipboardPayload, PortError>> {
        Box::pin(async move { self.worker.clipboard(op).await })
    }

    fn launch_app(&self, req: LaunchRequest) -> BoxFuture<'_, Result<LaunchedApp, PortError>> {
        let meta = self.snapshot_meta();
        Box::pin(async move {
            let resolved = resolve_launch_command(meta.display_id, &req)?;
            let before_windows = list_sway_windows(&meta.sway_socket).await?;
            let before_ids: BTreeSet<u32> =
                before_windows.iter().map(|w| w.target.window_id).collect();
            let app_stem = resolved
                .program
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let was_running = before_windows
                .iter()
                .any(|w| w.app.to_ascii_lowercase().contains(&app_stem) && !app_stem.is_empty());

            let home = std::env::var("HOME").unwrap_or_default();
            let user = std::env::var("USER").unwrap_or_default();
            let path = std::env::var("PATH")
                .unwrap_or_else(|_| format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin"));
            let wl_name = meta
                .wayland_socket
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("wayland-1");

            let pipe_setup = if resolved.is_chromium_like
                && !resolved
                    .args
                    .iter()
                    .any(|a| a.starts_with("--remote-debugging-"))
            {
                devtools_pipe::PipeSetup::create().ok()
            } else {
                None
            };
            let child_pipe_fds = pipe_setup.as_ref().map(|p| p.child_fds());

            let stderr_stdio = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(meta.base_dir.join("apps.stderr.log"))
                .map(Stdio::from)
                .unwrap_or_else(|_| Stdio::null());

            let mut cmd = Command::new(&resolved.program);
            cmd.env_clear()
                .env("HOME", &home)
                .env("USER", &user)
                .env("PATH", &path)
                .env("XDG_RUNTIME_DIR", &meta.runtime_dir)
                .env("WAYLAND_DISPLAY", wl_name)
                .env("DISPLAY", &meta.x11_display)
                .env("DBUS_SESSION_BUS_ADDRESS", &meta.session_bus_address)
                .env("AT_SPI_BUS_ADDRESS", &meta.atspi_bus_address)
                .env("GTK_USE_PORTAL", "0")
                .env("GTK_MODULES", "gail:atk-bridge")
                .env("GTK_A11Y", "atspi")
                .env("QT_ACCESSIBILITY", "1")
                .env("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(stderr_stdio);

            if let Some(cfg_dir) = &resolved.isolated_xdg_config {
                cmd.env("XDG_CONFIG_HOME", cfg_dir);
            }

            if pipe_setup.is_some() {
                cmd.arg("--remote-debugging-pipe");
            }
            cmd.args(&resolved.args);

            if let Ok(lang) = std::env::var("LANG") {
                cmd.env("LANG", lang);
            }
            for (k, v) in &req.environment {
                cmd.env(k, v);
            }
            unsafe {
                cmd.pre_exec(move || {
                    libc::setsid();
                    let min_close = if let Some((r3, w4)) = child_pipe_fds {
                        devtools_pipe::pre_exec_dup_fds(r3, w4)?;
                        5u32
                    } else {
                        3u32
                    };
                    let _ = libc::syscall(
                        libc::SYS_close_range,
                        min_close,
                        u32::MAX,
                        libc::CLOSE_RANGE_CLOEXEC,
                    );
                    Ok(())
                });
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    if let Some(p) = pipe_setup {
                        p.abort();
                    }
                    return Err(PortError::failed(format!(
                        "failed to start {}: {e}",
                        resolved.program.display()
                    )));
                }
            };
            let pid = child.id() as i32;
            if let Some(p) = pipe_setup {
                devtools_pipe::register_spawned(meta.display_id, pid, p);
            }
            if let Some(scope) = &meta.scope_unit {
                attach_pid_to_scope(scope, child.id());
            }
            self.record_started_pids(&[pid]);

            // Reap exited short-lived launcher wrappers in the background so zombies never accumulate.
            std::thread::spawn(move || {
                let _ = child.wait();
            });

            // Wait up to 8 seconds for a new window to map inside this isolated sway instance.
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut found_window: Option<WindowInfo> = None;
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(80)).await;
                let current = list_sway_windows(&meta.sway_socket)
                    .await
                    .unwrap_or_default();
                if let Some(new_win) = current
                    .into_iter()
                    .find(|w| !before_ids.contains(&w.target.window_id))
                {
                    // Ensure newly opened window fits comfortably inside the display.
                    let area = Rect::new(0.0, 0.0, meta.width as f64, meta.height as f64);
                    let fitted = new_win.bounds.kept_inside(&area);
                    if (fitted.x - new_win.bounds.x).abs() > 1.0
                        || (fitted.y - new_win.bounds.y).abs() > 1.0
                        || (fitted.width - new_win.bounds.width).abs() > 1.0
                        || (fitted.height - new_win.bounds.height).abs() > 1.0
                    {
                        let _ = sway_run_command(
                            &meta.sway_socket,
                            &format!(
                                "[con_id={}] floating enable, resize set {} {}, move position {} {}",
                                new_win.target.window_id,
                                fitted.width.round() as i64,
                                fitted.height.round() as i64,
                                fitted.x.round() as i64,
                                fitted.y.round() as i64,
                            ),
                        )
                        .await;
                        tokio::time::sleep(Duration::from_millis(60)).await;
                        found_window = list_sway_windows(&meta.sway_socket)
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .find(|w| w.target.window_id == new_win.target.window_id)
                            .or(Some(new_win));
                    } else {
                        found_window = Some(new_win);
                    }
                    break;
                }
                if !pid_alive(pid as u32) && resolved.is_snap {
                    return Err(PortError::failed(format!(
                        "{} exited without connecting to the isolated Wayland display. Snap AppArmor confinement blocks private compositor sockets; install a native .deb or distro package (such as google-chrome-stable, chromium, or Mozilla's APT firefox) instead.",
                        resolved.program.display()
                    )));
                }
            }

            if found_window.is_none() && was_running {
                let current = list_sway_windows(&meta.sway_socket)
                    .await
                    .unwrap_or_default();
                if let Some(existing) = current
                    .into_iter()
                    .find(|w| w.app.to_ascii_lowercase().contains(&app_stem))
                {
                    return Ok(LaunchedApp {
                        pid: Some(pid),
                        window: Some(existing),
                        was_running: true,
                        reason: Some("already_running"),
                    });
                }
            }

            Ok(LaunchedApp {
                pid: Some(pid),
                reason: if found_window.is_some() {
                    None
                } else {
                    Some("still_starting")
                },
                window: found_window,
                was_running,
            })
        })
    }

    fn list_windows(&self) -> BoxFuture<'_, Result<Vec<WindowInfo>, PortError>> {
        let sock = self.meta.lock().unwrap().sway_socket.clone();
        Box::pin(async move { list_sway_windows(&sock).await })
    }

    fn place_window(
        &self,
        target: WindowTarget,
        frame: Option<FramePatch>,
    ) -> BoxFuture<'_, Result<PlaceOutcome, PortError>> {
        let (sock, w, h) = {
            let m = self.meta.lock().unwrap();
            (m.sway_socket.clone(), m.width as f64, m.height as f64)
        };
        Box::pin(async move {
            let current = list_sway_windows(&sock)
                .await?
                .into_iter()
                .find(|win| win.target.window_id == target.window_id)
                .ok_or_else(|| {
                    PortError::not_found(format!("window {} closed", target.window_id))
                })?;

            let Some((ax, ay, aw, ah)) = frame else {
                let _ =
                    sway_run_command(&sock, &format!("[con_id={}] focus", target.window_id)).await;
                return Ok(PlaceOutcome {
                    window: current,
                    constrained: false,
                });
            };

            let raw_rect = Rect::new(
                ax.unwrap_or(current.bounds.x),
                ay.unwrap_or(current.bounds.y),
                aw.unwrap_or(current.bounds.width),
                ah.unwrap_or(current.bounds.height),
            );
            let area = Rect::new(0.0, 0.0, w, h);
            let clamped = raw_rect.kept_inside(&area);
            let constrained = (clamped.x - raw_rect.x).abs() > 0.5
                || (clamped.y - raw_rect.y).abs() > 0.5
                || (clamped.width - raw_rect.width).abs() > 0.5
                || (clamped.height - raw_rect.height).abs() > 0.5;

            sway_run_command(
                &sock,
                &format!(
                    "[con_id={}] floating enable, resize set {} {}, move position {} {}, focus",
                    target.window_id,
                    clamped.width.round().max(1.0) as i64,
                    clamped.height.round().max(1.0) as i64,
                    clamped.x.round() as i64,
                    clamped.y.round() as i64,
                ),
            )
            .await?;

            // Wait up to 400 ms until two consecutive geometry reads agree.
            let mut prev = None;
            let mut final_win = current;
            for _ in 0..8 {
                tokio::time::sleep(Duration::from_millis(45)).await;
                if let Some(now) = list_sway_windows(&sock)
                    .await?
                    .into_iter()
                    .find(|win| win.target.window_id == target.window_id)
                {
                    let rect = now.bounds;
                    final_win = now;
                    if prev == Some(rect) {
                        break;
                    }
                    prev = Some(rect);
                }
            }

            Ok(PlaceOutcome {
                window: final_win,
                constrained,
            })
        })
    }

    fn focus_window(&self, target: WindowTarget) -> BoxFuture<'_, Result<(), PortError>> {
        let sock = self.meta.lock().unwrap().sway_socket.clone();
        Box::pin(async move {
            sway_run_command(&sock, &format!("[con_id={}] focus", target.window_id)).await
        })
    }

    fn close_window(&self, target: WindowTarget) -> BoxFuture<'_, Result<(), PortError>> {
        let sock = self.meta.lock().unwrap().sway_socket.clone();
        Box::pin(async move {
            sway_run_command(&sock, &format!("[con_id={}] kill", target.window_id)).await
        })
    }

    fn record_started_pids(&self, pids: &[i32]) {
        {
            let mut m = self.meta.lock().unwrap();
            for &pid in pids {
                if pid > 0 && !m.started_pids.contains(&pid) {
                    m.started_pids.push(pid);
                }
            }
        }
        self.persist_meta();
    }

    fn shutdown(&self) -> BoxFuture<'_, Result<(), PortError>> {
        let meta = self.snapshot_meta();
        devtools_pipe::remove_display_channels(meta.display_id);
        self.worker.shutdown();
        Box::pin(async move {
            terminate_persisted_display(&meta).await;
            Ok(())
        })
    }
}

pub struct SwayDisplayManager {
    displays_root: PathBuf,
    use_systemd_scopes: bool,
}

impl SwayDisplayManager {
    pub fn new() -> Self {
        let base = agent_screen_mcp::endpoint::state_dir();
        let instance_dir = match agent_screen_mcp::endpoint::instance() {
            Some(inst) => format!("displays-{inst}"),
            None => "displays".to_string(),
        };
        let displays_root = base.join(instance_dir);
        let _ = std::fs::create_dir_all(&displays_root);
        let _ = std::fs::set_permissions(&displays_root, std::fs::Permissions::from_mode(0o700));
        let use_systemd_scopes = detect_user_systemd();
        Self {
            displays_root,
            use_systemd_scopes,
        }
    }

    #[allow(dead_code)]
    pub fn uses_systemd_scopes(&self) -> bool {
        self.use_systemd_scopes
    }

    async fn connect_persisted(
        &self,
        meta: PersistedDisplay,
    ) -> Result<Arc<SwayDisplay>, PortError> {
        let (notif_conn, notif_stats) = spawn_silent_notifications(&meta.session_bus_address)
            .await
            .map_err(|e| PortError::failed(e.to_string()))?;
        let _ = set_xwayland_atspi_bus(&meta.x11_display, &meta.atspi_bus_address);
        let worker = WaylandWorker::spawn(meta.wayland_socket.clone())
            .map_err(|e| PortError::failed(e.to_string()))?;
        Ok(Arc::new(SwayDisplay {
            meta: Mutex::new(meta),
            worker,
            _notifications_conn: notif_conn,
            _notifications_stats: notif_stats,
        }))
    }
}

impl DisplayManager for SwayDisplayManager {
    fn capabilities(&self) -> Capabilities {
        Capabilities::linux()
    }

    fn create<'a>(
        &'a self,
        spec: DisplaySpec,
        orphans: &'a [Orphans],
    ) -> BoxFuture<'a, Result<Arc<dyn Display>, PortError>> {
        Box::pin(async move {
            let base_dir = self.displays_root.join(format!("d{}", spec.id));
            let _ = std::fs::remove_dir_all(&base_dir);
            let runtime_dir = base_dir.join("runtime");
            std::fs::create_dir_all(&runtime_dir).map_err(|e| PortError::failed(e.to_string()))?;
            set_mode_0700(&base_dir)?;
            set_mode_0700(&runtime_dir)?;

            let session_bus_socket = runtime_dir.join("session-bus.sock");
            let session_bus_conf = base_dir.join("session-dbus.conf");
            write_isolated_dbus_config(&session_bus_conf, &session_bus_socket)
                .map_err(|e| PortError::failed(e.to_string()))?;
            let session_bus_address = format!("unix:path={}", session_bus_socket.display());

            let atspi_bus_socket = runtime_dir.join("atspi-bus.sock");
            let atspi_bus_conf = base_dir.join("atspi-dbus.conf");
            write_isolated_dbus_config(&atspi_bus_conf, &atspi_bus_socket)
                .map_err(|e| PortError::failed(e.to_string()))?;
            let atspi_bus_address = format!("unix:path={}", atspi_bus_socket.display());

            let sway_socket = runtime_dir.join("sway-ipc.sock");
            let ready_file = runtime_dir.join("sway-ready.env");
            let sway_cfg = base_dir.join("sway.cfg");
            std::fs::write(
                &sway_cfg,
                format!(
                    "xwayland enable\n\
                     output HEADLESS-1 resolution {}x{} position 0,0 scale 1 bg #2b2e33 solid_color\n\
                     default_border pixel 2\n\
                     for_window [all] floating enable\n\
                     bar {{\n  mode invisible\n}}\n\
                     exec sh -c 'printf \"WAYLAND_DISPLAY=%s\\nDISPLAY=%s\\n\" \"$WAYLAND_DISPLAY\" \"$DISPLAY\" > {:?}.tmp && mv {:?}.tmp {:?}'\n",
                    spec.width,
                    spec.height,
                    ready_file.display(),
                    ready_file.display(),
                    ready_file.display(),
                ),
            )
            .map_err(|e| PortError::failed(e.to_string()))?;

            let scope_script = base_dir.join("run-display.sh");
            let home = std::env::var("HOME").unwrap_or_default();
            let user = std::env::var("USER").unwrap_or_default();
            let path = std::env::var("PATH")
                .unwrap_or_else(|_| format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin"));
            let registryd = find_atspi_registryd();
            let renderer_env = if Path::new("/dev/dri/renderD128").exists()
                && std::env::var_os("AGENTSCREEN_PIXMAN").is_none()
            {
                ""
            } else {
                "export WLR_RENDERER=pixman\n"
            };
            std::fs::write(
                &scope_script,
                format!(
                    "#!/usr/bin/env bash\n\
                     set -euo pipefail\n\
                     export HOME={home:?}\n\
                     export USER={user:?}\n\
                     export PATH={path:?}\n\
                     export XDG_RUNTIME_DIR={:?}\n\
                     unset WAYLAND_DISPLAY DISPLAY AT_SPI_BUS_ADDRESS HYPRLAND_INSTANCE_SIGNATURE\n\
                     dbus-daemon --config-file={:?} --nofork >/dev/null 2>&1 &\n\
                     dbus-daemon --config-file={:?} --nofork >/dev/null 2>&1 &\n\
                     for _ in $(seq 1 80); do\n\
                       [ -S {:?} ] && [ -S {:?} ] && break\n\
                       sleep 0.02\n\
                     done\n\
                     if [ -x {registryd:?} ]; then\n\
                       DBUS_SESSION_BUS_ADDRESS={session_bus_address:?} AT_SPI_BUS_ADDRESS={atspi_bus_address:?} {registryd:?} >/dev/null 2>&1 &\n\
                     fi\n\
                     export DBUS_SESSION_BUS_ADDRESS={session_bus_address:?}\n\
                     export AT_SPI_BUS_ADDRESS={atspi_bus_address:?}\n\
                     export WLR_BACKENDS=headless\n\
                     export WLR_LIBINPUT_NO_DEVICES=1\n\
                     {renderer_env}\
                     export SWAYSOCK={:?}\n\
                     exec sway -c {:?}\n",
                    runtime_dir.display(),
                    session_bus_conf.display(),
                    atspi_bus_conf.display(),
                    session_bus_socket.display(),
                    atspi_bus_socket.display(),
                    sway_socket.display(),
                    sway_cfg.display(),
                ),
            )
            .map_err(|e| PortError::failed(e.to_string()))?;
            set_mode_0700(&scope_script)?;

            let inst_tag = agent_screen_mcp::endpoint::instance()
                .map(|i| format!("{i}-"))
                .unwrap_or_default();
            let unit_stem = format!(
                "agent-screen-display-{inst_tag}{}-{}",
                spec.id,
                std::process::id()
            );

            let (sway_pid, scope_unit) = if self.use_systemd_scopes {
                let mut cmd = Command::new("systemd-run");
                cmd.args([
                    "--user",
                    "--scope",
                    "--collect",
                    &format!("--unit={unit_stem}"),
                ])
                .arg(&scope_script)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
                match cmd.spawn() {
                    Ok(mut child) => {
                        let pid = child.id();
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
                        (pid, Some(format!("{unit_stem}.scope")))
                    }
                    Err(_) => (spawn_setsid_script(&scope_script)?, None),
                }
            } else {
                (spawn_setsid_script(&scope_script)?, None)
            };

            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && !ready_file.exists() {
                if !pid_alive(sway_pid) {
                    return Err(PortError::failed(
                        "headless sway exited before publishing its Wayland socket",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            if !ready_file.exists() {
                return Err(PortError::failed(
                    "timed out waiting for headless sway to start",
                ));
            }

            let ready_text = std::fs::read_to_string(&ready_file)
                .map_err(|e| PortError::failed(e.to_string()))?;
            let mut wl_name = String::from("wayland-1");
            let mut x11_display = String::new();
            for line in ready_text.lines() {
                if let Some(v) = line.strip_prefix("WAYLAND_DISPLAY=")
                    && !v.is_empty()
                {
                    wl_name = v.to_string();
                } else if let Some(v) = line.strip_prefix("DISPLAY=") {
                    x11_display = v.to_string();
                }
            }
            let wayland_socket = runtime_dir.join(wl_name);

            let mut started_pids: Vec<i32> = orphans
                .iter()
                .flat_map(|g| g.pids.iter().copied())
                .filter(|&p| p > 0)
                .collect();
            started_pids.sort_unstable();
            started_pids.dedup();

            let meta = PersistedDisplay {
                display_id: spec.id,
                width: spec.width,
                height: spec.height,
                session_key: spec.session_key,
                client: spec.client,
                title: spec.title,
                scope_unit,
                sway_pid,
                base_dir,
                runtime_dir,
                wayland_socket,
                sway_socket,
                x11_display,
                session_bus_address,
                atspi_bus_address,
                started_pids,
            };
            write_state_atomic(&meta)?;
            let display = self.connect_persisted(meta).await?;
            Ok(display as Arc<dyn Display>)
        })
    }

    fn reattach_surviving(&self) -> BoxFuture<'_, Vec<RecoveredDisplay>> {
        Box::pin(async move {
            let mut out = Vec::new();
            let Ok(entries) = std::fs::read_dir(&self.displays_root) else {
                return out;
            };
            for entry in entries.flatten() {
                let state_path = entry.path().join("state.json");
                let Ok(raw) = std::fs::read_to_string(&state_path) else {
                    continue;
                };
                let Ok(meta) = serde_json::from_str::<PersistedDisplay>(&raw) else {
                    continue;
                };
                if !pid_alive(meta.sway_pid)
                    || sway_ipc_call(&meta.sway_socket, IPC_GET_VERSION, "")
                        .await
                        .is_err()
                {
                    terminate_persisted_display(&meta).await;
                    continue;
                }
                let spec = DisplaySpec {
                    id: meta.display_id,
                    width: meta.width,
                    height: meta.height,
                    session_key: meta.session_key.clone(),
                    client: meta.client.clone(),
                    title: meta.title.clone(),
                };
                let started_pids = meta.started_pids.clone();
                if let Ok(display) = self.connect_persisted(meta).await {
                    out.push(RecoveredDisplay {
                        spec,
                        started_pids,
                        display: display as Arc<dyn Display>,
                    });
                }
            }
            out.sort_by_key(|r| r.spec.id);
            out
        })
    }
}

async fn terminate_persisted_display(meta: &PersistedDisplay) {
    let _ = sway_ipc_call(&meta.sway_socket, IPC_RUN_COMMAND, "exit").await;
    if let Some(unit) = &meta.scope_unit {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", unit])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    } else if meta.sway_pid > 0 {
        unsafe {
            let _ = libc::kill(-(meta.sway_pid as i32), libc::SIGTERM);
        }
    }
    for &pid in &meta.started_pids {
        if pid > 0 {
            unsafe {
                let _ = libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&meta.base_dir);
}

fn write_state_atomic(meta: &PersistedDisplay) -> Result<(), PortError> {
    let path = meta.base_dir.join("state.json");
    let tmp = meta
        .base_dir
        .join(format!("state.json.tmp.{}", std::process::id()));
    let text = serde_json::to_string_pretty(meta).map_err(|e| PortError::failed(e.to_string()))?;
    std::fs::write(&tmp, text).map_err(|e| PortError::failed(e.to_string()))?;
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    std::fs::rename(&tmp, &path).map_err(|e| PortError::failed(e.to_string()))?;
    Ok(())
}

fn detect_user_systemd() -> bool {
    if std::env::var_os("AGENTSCREEN_FORCE_SETSID").is_some() {
        return false;
    }
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        && Command::new("systemd-run")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

fn spawn_setsid_script(script: &Path) -> Result<u32, PortError> {
    let mut cmd = Command::new(script);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| PortError::failed(e.to_string()))?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

fn attach_pid_to_scope(scope_unit: &str, pid: u32) {
    let _ = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "AttachProcessesToUnit",
            "ssau",
            scope_unit,
            "/",
            "1",
            &pid.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn find_atspi_registryd() -> &'static str {
    for candidate in [
        "/usr/lib/at-spi2-registryd",
        "/usr/libexec/at-spi2-registryd",
        "/usr/lib/at-spi2-core/at-spi2-registryd",
        "/usr/lib/x86_64-linux-gnu/at-spi2/at-spi2-registryd",
        "/usr/lib/aarch64-linux-gnu/at-spi2/at-spi2-registryd",
    ] {
        if Path::new(candidate).is_file() {
            return candidate;
        }
    }
    "/usr/lib/at-spi2-registryd"
}

fn set_mode_0700(path: &Path) -> Result<(), PortError> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| PortError::failed(e.to_string()))
}

fn pid_alive(pid: u32) -> bool {
    pid > 0 && unsafe { libc::kill(pid as i32, 0) == 0 }
}

async fn sway_ipc_call(sock: &Path, msg_type: u32, payload: &str) -> Result<Value, PortError> {
    let mut stream = UnixStream::connect(sock)
        .await
        .map_err(|e| PortError::failed(format!("connect sway IPC {}: {e}", sock.display())))?;
    let body = payload.as_bytes();
    let mut header = [0u8; 14];
    header[..6].copy_from_slice(IPC_MAGIC);
    header[6..10].copy_from_slice(&(body.len() as u32).to_le_bytes());
    header[10..14].copy_from_slice(&msg_type.to_le_bytes());
    stream
        .write_all(&header)
        .await
        .map_err(|e| PortError::failed(e.to_string()))?;
    if !body.is_empty() {
        stream
            .write_all(body)
            .await
            .map_err(|e| PortError::failed(e.to_string()))?;
    }
    stream
        .flush()
        .await
        .map_err(|e| PortError::failed(e.to_string()))?;

    let mut resp_head = [0u8; 14];
    stream
        .read_exact(&mut resp_head)
        .await
        .map_err(|e| PortError::failed(e.to_string()))?;
    let resp_len = u32::from_le_bytes(resp_head[6..10].try_into().unwrap()) as usize;
    let mut resp_buf = vec![0u8; resp_len];
    stream
        .read_exact(&mut resp_buf)
        .await
        .map_err(|e| PortError::failed(e.to_string()))?;
    serde_json::from_slice(&resp_buf).map_err(|e| PortError::failed(e.to_string()))
}

async fn sway_run_command(sock: &Path, command: &str) -> Result<(), PortError> {
    let res = sway_ipc_call(sock, IPC_RUN_COMMAND, command).await?;
    if let Some(arr) = res.as_array() {
        for item in arr {
            if item.get("success") == Some(&Value::Bool(false)) {
                let err = item
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("sway command failed");
                return Err(PortError::failed(err));
            }
        }
    }
    Ok(())
}

pub async fn list_sway_windows(sock: &Path) -> Result<Vec<WindowInfo>, PortError> {
    let tree = sway_ipc_call(sock, IPC_GET_TREE, "").await?;
    let mut wins = Vec::new();
    collect_sway_windows(&tree, &mut wins);
    // Sway lists focus_order in `focus` array (most-recently-focused first).
    let focus_order: Vec<u32> = tree
        .get("focus")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_u64)
                .filter_map(|id| u32::try_from(id).ok())
                .collect()
        })
        .unwrap_or_default();
    wins.sort_by_key(|w| {
        focus_order
            .iter()
            .position(|&id| id == w.target.window_id)
            .unwrap_or(usize::MAX)
    });
    Ok(wins)
}

fn collect_sway_windows(node: &Value, out: &mut Vec<WindowInfo>) {
    if node.get("name").and_then(Value::as_str) == Some("__i3_scratch") {
        return;
    }
    let pid = node.get("pid").and_then(Value::as_i64).unwrap_or(0).max(0);
    let con_id = node.get("id").and_then(Value::as_u64).unwrap_or(0);
    let is_view = node.get("app_id").is_some_and(|v| !v.is_null())
        || node.get("window").is_some_and(|v| !v.is_null());
    if is_view && let Ok(window_id) = u32::try_from(con_id) {
        let app = node
            .get("app_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                node.get("window_properties")
                    .and_then(|p| p.get("class").or_else(|| p.get("instance")))
                    .and_then(Value::as_str)
            })
            .unwrap_or("App")
            .to_string();
        let title = node
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let rect = &node["rect"];
        let deco = &node["deco_rect"];
        let x = rect["x"].as_f64().unwrap_or(0.0);
        let y = rect["y"].as_f64().unwrap_or(0.0) - deco["height"].as_f64().unwrap_or(0.0);
        let width = rect["width"].as_f64().unwrap_or(1.0).max(1.0);
        let height = (rect["height"].as_f64().unwrap_or(1.0)
            + deco["height"].as_f64().unwrap_or(0.0))
        .max(1.0);
        out.push(WindowInfo {
            target: WindowTarget {
                pid: pid as i32,
                window_id,
            },
            app,
            title,
            bounds: Rect::new(x, y, width, height),
        });
        return;
    }

    // Visit floating_nodes before tiling nodes so floating windows stay ahead when unranked.
    for key in ["floating_nodes", "nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                collect_sway_windows(child, out);
            }
        }
    }
}
