mod dbus_atspi;
mod wayland;

use anyhow::{Context, Result, anyhow, bail};
use dbus_atspi::{
    AccessibleAppSummary, get_xwayland_atspi_bus, inspect_atspi_bus, set_xwayland_atspi_bus,
    start_notifications_stub, write_dbus_config,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use wayland::{BTN_LEFT, WaylandSession};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedDisplayState {
    scope_unit: String,
    base_dir: PathBuf,
    runtime_dir: PathBuf,
    wayland_socket: PathBuf,
    sway_socket: PathBuf,
    x11_display: String,
    session_bus_socket: PathBuf,
    session_bus_address: String,
    atspi_bus_socket: PathBuf,
    atspi_bus_address: String,
    gtk4_state_file: PathBuf,
    sway_pid: u32,
    gtk4_pid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct HostSnapshot {
    wayland_sockets: Vec<String>,
    atspi_bus0_inode: Option<u64>,
    atspi_bus0_mtime: Option<i64>,
    x0_inode: Option<u64>,
    hypr_cursor: Option<String>,
    hypr_active_window: Option<String>,
    host_atspi_apps: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SpikeReport {
    os_release: String,
    kernel: String,
    sway_version: String,
    wayland_globals: BTreeMap<String, u32>,
    probe1_systemd_sigkill_survival: Value,
    probe2_screencopy_and_virtual_input: Value,
    probe3_atspi_isolation_and_xwayland: Value,
    probe4_private_dbus_compensations: Value,
    probe5_host_zero_interference: Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--daemon-helper" {
        return run_daemon_helper(Path::new(&args[2])).await;
    }

    let spike_root = spike_manifest_dir();
    let host_xdg = PathBuf::from(
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string()),
    );
    let base_dir = host_xdg.join(format!("agent-screen-spike-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base_dir);
    std::fs::create_dir_all(&base_dir)?;
    set_mode_0700(&base_dir)?;

    let before_host = snapshot_host(&host_xdg).await?;

    let result = run_all_probes(&spike_root, &host_xdg, &base_dir, &before_host).await;

    // Always clean up transient scope and runtime directory.
    if let Ok(raw) = std::fs::read_to_string(base_dir.join("state.json"))
        && let Ok(state) = serde_json::from_str::<PersistedDisplayState>(&raw)
    {
        cleanup_scope(&state);
    }
    let _ = std::fs::remove_dir_all(&base_dir);

    let report = result?;
    let after_host = snapshot_host(&host_xdg).await?;
    verify_host_untouched(&before_host, &after_host)?;

    let json_text = serde_json::to_string_pretty(&report)?;
    let out_path = spike_root.join("last-run-report.json");
    std::fs::write(&out_path, &json_text)?;
    println!("{json_text}");
    Ok(())
}

async fn run_all_probes(
    spike_root: &Path,
    host_xdg: &Path,
    base_dir: &Path,
    before_host: &HostSnapshot,
) -> Result<SpikeReport> {
    let exe = std::env::current_exe().context("resolve current_exe")?;

    // =========================================================================
    // PROBE 1: systemd --user transient scope survival across daemon SIGKILL
    // =========================================================================
    let t_spawn = Instant::now();
    let mut daemon = Command::new(&exe)
        .arg("--daemon-helper")
        .arg(base_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn --daemon-helper")?;
    let daemon_pid = daemon.id();

    let state_file = base_dir.join("state.json");
    wait_until(Duration::from_secs(12), || state_file.exists())
        .await
        .context("timed out waiting for daemon helper to write state.json")?;
    let state: PersistedDisplayState =
        serde_json::from_str(&std::fs::read_to_string(&state_file)?)?;
    wait_until(Duration::from_secs(10), || state.gtk4_state_file.exists())
        .await
        .context("timed out waiting for GTK4 fixture initial state")?;
    let startup_ms = t_spawn.elapsed().as_secs_f64() * 1000.0;

    // Send uncatchable SIGKILL to the daemon helper process!
    let rc = unsafe { libc::kill(daemon_pid as i32, libc::SIGKILL) };
    if rc != 0 {
        bail!("kill(SIGKILL) failed: {}", std::io::Error::last_os_error());
    }
    let exit_status = daemon.wait().context("wait for SIGKILLed daemon")?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let scope_active = is_unit_active(&state.scope_unit);
    let sway_alive = pid_alive(state.sway_pid);
    let gtk4_alive = pid_alive(state.gtk4_pid);
    if !scope_active || !sway_alive || !gtk4_alive {
        bail!(
            "scope survival failed: scope_active={scope_active}, sway_alive={sway_alive}, gtk4_alive={gtk4_alive}"
        );
    }

    // Reattach as a new daemon instance: reconnect notifications stub + Sway IPC + Wayland!
    let (_notif_conn, notif_stats) = start_notifications_stub(&state.session_bus_address).await?;
    let sway_tree_before = swaymsg_json(&state.sway_socket, &["-t", "get_tree"])?;
    let surviving_titles = collect_window_titles(&sway_tree_before);
    if !surviving_titles
        .iter()
        .any(|t| t.contains("Spike GTK4 Fixture"))
    {
        bail!("GTK4 window missing after daemon SIGKILL: {surviving_titles:?}");
    }

    // =========================================================================
    // PROBE 2: wlr-screencopy-unstable-v1 + virtual pointer/drag + UTF-8 keyboard + clipboard
    // =========================================================================
    // Position the GTK4 window deterministically at (20, 20) size 640x440 and focus it.
    swaymsg(
        &state.sway_socket,
        &[
            r#"[title="Spike GTK4 Fixture"] floating enable, move position 20 20, resize set 640 440, focus"#,
        ],
    )?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut wl = WaylandSession::connect(&state.wayland_socket)?;
    let globals = wl.globals().clone();

    let frame_before = wl.capture_frame()?;
    if frame_before.width != 1280 || frame_before.height != 800 {
        bail!(
            "unexpected screencopy dimensions {}x{}",
            frame_before.width,
            frame_before.height
        );
    }
    // Button sits near top of window (20, 20) + margin 16 => around (200, 68)
    let btn_px_before = frame_before.pixel(200, 68);

    // Click the green button at (200, 68)
    wl.click(200, 68, 1280, 800, BTN_LEFT)?;
    wait_until(Duration::from_secs(3), || {
        read_gtk4_state(&state.gtk4_state_file)
            .and_then(|v| v["clicks"].as_u64())
            .unwrap_or(0)
            >= 1
    })
    .await
    .context("GTK4 button click did not increment counter")?;

    tokio::time::sleep(Duration::from_millis(120)).await;
    let frame_after_click = wl.capture_frame()?;
    let btn_px_after = frame_after_click.pixel(200, 68);

    // Drag across the blue Drag Zone (around y = 160, from x = 120 to x = 280, y = 160 to 190)
    wl.drag((120, 160), (280, 190), (1280, 800), 16)?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let gtk_after_drag =
        read_gtk4_state(&state.gtk4_state_file).ok_or_else(|| anyhow!("missing gtk4 state"))?;
    let drag_dx = gtk_after_drag["drag_delta"][0].as_f64().unwrap_or(0.0);
    let drag_dy = gtk_after_drag["drag_delta"][1].as_f64().unwrap_or(0.0);
    if drag_dx < 100.0 {
        bail!("virtual pointer drag delta too small: ({drag_dx}, {drag_dy})");
    }

    // Click Normal Entry (around y = 250) and type ASCII + Chinese UTF-8 via dynamic XKB keymap!
    wl.click(200, 250, 1280, 800, BTN_LEFT)?;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let sample_text = "Hello 中文 42";
    wl.type_utf8(sample_text)?;
    wait_until(Duration::from_secs(3), || {
        read_gtk4_state(&state.gtk4_state_file)
            .and_then(|v| v["entry_text"].as_str().map(|s| s == sample_text))
            .unwrap_or(false)
    })
    .await
    .context("GTK4 entry did not receive UTF-8 typed string")?;

    // Test in-display clipboard shortcuts: ctrl+a -> ctrl+c -> Right -> ctrl+v
    wl.press_chord(true, false, "a")?;
    wl.press_chord(true, false, "c")?;
    wl.press_chord(false, false, "Right")?;
    wl.press_chord(true, false, "v")?;
    let doubled = format!("{sample_text}{sample_text}");
    wait_until(Duration::from_secs(3), || {
        read_gtk4_state(&state.gtk4_state_file)
            .and_then(|v| v["entry_text"].as_str().map(|s| s == doubled))
            .unwrap_or(false)
    })
    .await
    .context("in-display ctrl+c / ctrl+v failed in GTK4 entry")?;

    let copied_in_display = wl.read_clipboard_text()?.unwrap_or_default();
    if copied_in_display != sample_text {
        bail!(
            "zwlr_data_control read mismatch: got {copied_in_display:?}, expected {sample_text:?}"
        );
    }

    // Test writing clipboard from host bridge (Takeover simulation) via zwlr_data_control
    wl.write_clipboard_text("桥接粘贴 BridgeOK")?;
    wl.press_chord(true, false, "a")?;
    // Spawn a tiny paste helper or dispatch pending while GTK reads our data_source pipe!
    let wayland_sock = state.wayland_socket.clone();
    let paste_check = tokio::task::spawn_blocking(move || -> Result<String> {
        let mut reader_wl = WaylandSession::connect(&wayland_sock)?;
        Ok(reader_wl.read_clipboard_text()?.unwrap_or_default())
    });
    for _ in 0..10 {
        wl.dispatch_pending()?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let bridged_roundtrip = paste_check.await??;
    if bridged_roundtrip != "桥接粘贴 BridgeOK" {
        bail!("zwlr_data_control write roundtrip mismatch: {bridged_roundtrip:?}");
    }

    // =========================================================================
    // PROBE 3: AT-SPI2 isolation under dbus-broker + Xwayland root AT_SPI_BUS atom
    // =========================================================================
    let qt_bin = compile_qt6_fixture(spike_root)?;

    // 3a. BEFORE writing AT_SPI_BUS to Xwayland root window, verify root property is absent
    // and launch an X11 Qt6 client with AT_SPI_BUS_ADDRESS unset to confirm Trap #3!
    let xprop_before = get_xwayland_atspi_bus(&state.x11_display)?;
    let mut qt_x11_no_atom = spawn_isolated_client(
        &qt_bin,
        &["--spike-title", "Qt6 X11 Without Atom"],
        &state,
        false, // unset WAYLAND_DISPLAY
        false, // unset AT_SPI_BUS_ADDRESS so it relies on X11 AT_SPI_BUS root property
        &[("QT_QPA_PLATFORM", "xcb")],
    )?;
    tokio::time::sleep(Duration::from_millis(900)).await;
    let apps_before_atom = inspect_atspi_bus(&state.atspi_bus_address).await?;
    let x11_registered_without_atom = apps_before_atom
        .iter()
        .any(|a| a.sample_nodes.iter().any(|n| n.contains("Without Atom")));
    let _ = qt_x11_no_atom.kill();
    let _ = qt_x11_no_atom.wait();

    // 3b. Write AT_SPI_BUS atom onto Xwayland root window and verify readback!
    set_xwayland_atspi_bus(&state.x11_display, &state.atspi_bus_address)?;
    let xprop_after = get_xwayland_atspi_bus(&state.x11_display)?;
    if xprop_after.as_deref() != Some(state.atspi_bus_address.as_str()) {
        bail!("Xwayland AT_SPI_BUS property mismatch: {xprop_after:?}");
    }

    // 3c. Launch Qt6 Wayland, Qt6 Xwayland (still with AT_SPI_BUS_ADDRESS unset, proving X11 atom discovery!),
    // and Chromium Wayland (--force-renderer-accessibility --password-store=basic)!
    let mut qt_wayland = spawn_isolated_client(
        &qt_bin,
        &["--spike-title", "Spike Qt6 Wayland"],
        &state,
        true,
        true,
        &[("QT_QPA_PLATFORM", "wayland")],
    )?;
    let mut qt_xwayland = spawn_isolated_client(
        &qt_bin,
        &["--spike-title", "Spike Qt6 Xwayland"],
        &state,
        false, // X11 only
        false, // NO AT_SPI_BUS_ADDRESS env var -> MUST discover via Xwayland AT_SPI_BUS atom!
        &[("QT_QPA_PLATFORM", "xcb")],
    )?;

    let html_fixture = base_dir.join("spike-page.html");
    std::fs::write(
        &html_fixture,
        r#"<!doctype html><html><head><title>Spike Chromium Page</title></head>
        <body><h1>Chromium A11y Ready</h1>
        <input aria-label="Web Username" type="text" value="alice"/>
        <input aria-label="Web Secret" type="password" value="hunter2"/>
        <button>Sign In</button></body></html>"#,
    )?;
    let chrome_profile = base_dir.join("chromium-profile");
    std::fs::create_dir_all(&chrome_profile)?;
    set_mode_0700(&chrome_profile)?;
    let t_chrome = Instant::now();
    let mut chromium = spawn_isolated_client(
        Path::new("chromium"),
        &[
            "--ozone-platform=wayland",
            "--force-renderer-accessibility",
            "--password-store=basic",
            "--no-first-run",
            "--no-default-browser-check",
            &format!("--user-data-dir={}", chrome_profile.display()),
            &format!("file://{}", html_fixture.display()),
        ],
        &state,
        true,
        true,
        &[],
    )?;

    // Wait up to 8s for GTK4, Qt6 Wayland, Qt6 Xwayland, and Chromium to publish trees on atspi.sock
    let mut isolated_apps: Vec<AccessibleAppSummary> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        isolated_apps = inspect_atspi_bus(&state.atspi_bus_address)
            .await
            .unwrap_or_default();
        let has_gtk = isolated_apps
            .iter()
            .any(|a| a.toolkit.to_ascii_lowercase().contains("gtk") || a.name.contains("gtk4"));
        let has_qt_wl = isolated_apps.iter().any(|a| {
            a.sample_nodes
                .iter()
                .any(|n| n.contains("Spike Qt6 Wayland"))
        });
        let has_qt_x11 = isolated_apps.iter().any(|a| {
            a.sample_nodes
                .iter()
                .any(|n| n.contains("Spike Qt6 Xwayland"))
        });
        let has_chromium = isolated_apps
            .iter()
            .any(|a| a.toolkit.to_ascii_lowercase().contains("chrom") && a.has_password_role);
        if has_gtk && has_qt_wl && has_qt_x11 && has_chromium {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let chromium_a11y_ready_ms = t_chrome.elapsed().as_secs_f64() * 1000.0;

    let mid_host = snapshot_host(host_xdg).await?;

    // =========================================================================
    // PROBE 4: Private D-Bus compensations (Notifications stub + fast fail on Secret/Portal)
    // =========================================================================
    let t_notify = Instant::now();
    let session_conn = zbus::connection::Builder::address(state.session_bus_address.as_str())?
        .build()
        .await?;
    let notify_id: u32 = session_conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "spike-client",
                0u32,
                "",
                "Isolated Alert",
                "Never shown on host",
                Vec::<String>::new(),
                std::collections::HashMap::<&str, zbus::zvariant::Value<'_>>::new(),
                1000i32,
            ),
        )
        .await?
        .body()
        .deserialize()?;
    let notify_latency_ms = t_notify.elapsed().as_secs_f64() * 1000.0;

    let t_secret = Instant::now();
    let secret_res = session_conn
        .call_method(
            Some("org.freedesktop.secrets"),
            "/org/freedesktop/secrets",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
        .await;
    let secret_fast_fail_ms = t_secret.elapsed().as_secs_f64() * 1000.0;

    let t_portal = Instant::now();
    let portal_res = session_conn
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
        .await;
    let portal_fast_fail_ms = t_portal.elapsed().as_secs_f64() * 1000.0;

    let _ = qt_wayland.kill();
    let _ = qt_xwayland.kill();
    let _ = chromium.kill();
    let _ = qt_wayland.wait();
    let _ = qt_xwayland.wait();
    let _ = chromium.wait();

    // Validate AT-SPI assertions
    let password_role_detected = isolated_apps.iter().any(|a| a.has_password_role);
    let qt_x11_discovered_via_atom = isolated_apps.iter().any(|a| {
        a.sample_nodes
            .iter()
            .any(|n| n.contains("Spike Qt6 Xwayland"))
    });
    let all_have_password_role =
        isolated_apps.len() >= 4 && isolated_apps.iter().all(|a| a.has_password_role);
    if !all_have_password_role {
        eprintln!(
            "DEBUG isolated_apps: {}",
            serde_json::to_string_pretty(&isolated_apps).unwrap_or_default()
        );
        bail!("ATSPI_ROLE_PASSWORD_TEXT (role_id=40) was not detected across all 4 toolkits");
    }
    if !qt_x11_discovered_via_atom {
        bail!("Qt6 Xwayland client failed to register via X11 root AT_SPI_BUS property");
    }
    if x11_registered_without_atom {
        bail!("expected X11 client without AT_SPI_BUS atom or env var not to find private bus");
    }

    // Verify zero cross-contamination with host AT-SPI bus
    let leaked_to_host: Vec<&String> = mid_host
        .host_atspi_apps
        .iter()
        .filter(|name| {
            let l = name.to_ascii_lowercase();
            l.contains("spike") || l.contains("gtk4_app")
        })
        .collect();
    if !leaked_to_host.is_empty() {
        bail!("isolated apps leaked onto host AT-SPI bus: {leaked_to_host:?}");
    }
    verify_host_untouched(before_host, &mid_host)?;

    cleanup_scope(&state);

    Ok(SpikeReport {
        os_release: read_os_pretty_name(),
        kernel: run_cmd_trimmed("uname", &["-sr"]),
        sway_version: run_cmd_trimmed("sway", &["--version"]),
        wayland_globals: globals,
        probe1_systemd_sigkill_survival: json!({
            "passed": true,
            "scope_unit": state.scope_unit,
            "startup_ms": round2(startup_ms),
            "daemon_killed_pid": daemon_pid,
            "daemon_exit": format!("{exit_status}"),
            "scope_active_after_sigkill": scope_active,
            "sway_pid_survived": state.sway_pid,
            "gtk4_pid_survived": state.gtk4_pid,
            "reattached_windows": surviving_titles,
        }),
        probe2_screencopy_and_virtual_input: json!({
            "passed": true,
            "capture_resolution": [frame_before.width, frame_before.height],
            "first_capture_ms": round2(frame_before.capture_ms),
            "second_capture_ms": round2(frame_after_click.capture_ms),
            "button_rgba_before_click": btn_px_before,
            "button_rgba_after_click": btn_px_after,
            "drag_delta_points": [drag_dx, drag_dy],
            "typed_utf8": sample_text,
            "in_display_ctrl_c_ctrl_v_result": doubled,
            "data_control_readback": copied_in_display,
            "data_control_write_roundtrip": bridged_roundtrip,
        }),
        probe3_atspi_isolation_and_xwayland: json!({
            "passed": true,
            "isolated_xwayland_display": state.x11_display,
            "xwayland_at_spi_bus_before_write": xprop_before,
            "x11_registered_without_root_atom": x11_registered_without_atom,
            "xwayland_at_spi_bus_after_write": xprop_after,
            "x11_registered_after_root_atom": qt_x11_discovered_via_atom,
            "password_role_detected": password_role_detected,
            "chromium_a11y_ready_ms": round2(chromium_a11y_ready_ms),
            "isolated_atspi_apps": isolated_apps,
            "host_atspi_bus0_inode_before": before_host.atspi_bus0_inode,
            "host_atspi_bus0_inode_during": mid_host.atspi_bus0_inode,
            "host_atspi_apps_during": mid_host.host_atspi_apps,
        }),
        probe4_private_dbus_compensations: json!({
            "passed": true,
            "notifications_stub_reply_id": notify_id,
            "notifications_stub_latency_ms": round2(notify_latency_ms),
            "notifications_captured": notif_stats.summaries.lock().unwrap().clone(),
            "secret_service_absent_fast_fail": secret_res.is_err(),
            "secret_service_fail_ms": round2(secret_fast_fail_ms),
            "portal_absent_fast_fail": portal_res.is_err(),
            "portal_fail_ms": round2(portal_fast_fail_ms),
            "snap_present_on_host": Path::new("/snap/bin").exists(),
            "flatpak_present_on_host": which_exists("flatpak"),
        }),
        probe5_host_zero_interference: json!({
            "passed": true,
            "before": before_host,
            "during_active_display": mid_host,
        }),
    })
}

async fn run_daemon_helper(base_dir: &Path) -> Result<()> {
    let runtime_dir = base_dir.join("runtime");
    std::fs::create_dir_all(&runtime_dir)?;
    set_mode_0700(&runtime_dir)?;

    let session_bus_socket = runtime_dir.join("session-bus.sock");
    let session_bus_conf = base_dir.join("session-dbus.conf");
    write_dbus_config(&session_bus_conf, &session_bus_socket)?;
    let session_bus_address = format!("unix:path={}", session_bus_socket.display());

    let atspi_bus_socket = runtime_dir.join("atspi-bus.sock");
    let atspi_bus_conf = base_dir.join("atspi-dbus.conf");
    write_dbus_config(&atspi_bus_conf, &atspi_bus_socket)?;
    let atspi_bus_address = format!("unix:path={}", atspi_bus_socket.display());

    let sway_socket = runtime_dir.join("sway-ipc.sock");
    let env_dump = runtime_dir.join("sway-env.json");
    let sway_cfg = base_dir.join("sway.cfg");
    std::fs::write(
        &sway_cfg,
        format!(
            "xwayland enable\n\
             output HEADLESS-1 resolution 1280x800 position 0,0 scale 1\n\
             default_border pixel 2\n\
             for_window [all] floating enable\n\
             bar {{\n  mode invisible\n}}\n\
             exec python3 -c 'import json, os; open({:?}, \"w\").write(json.dumps({{\"WAYLAND_DISPLAY\": os.environ.get(\"WAYLAND_DISPLAY\",\"\"), \"DISPLAY\": os.environ.get(\"DISPLAY\",\"\"), \"SWAYSOCK\": os.environ.get(\"SWAYSOCK\",\"\")}}))'\n",
            env_dump.display()
        ),
    )?;

    let scope_unit = format!("agent-screen-spike-{}", std::process::id());
    let scope_script = base_dir.join("run-scope.sh");
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let path = format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin");
    let renderer_line = if Path::new("/dev/dri/renderD128").exists() {
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
             for _ in $(seq 1 50); do\n\
               [ -S {:?} ] && [ -S {:?} ] && break\n\
               sleep 0.02\n\
             done\n\
             DBUS_SESSION_BUS_ADDRESS={session_bus_address:?} AT_SPI_BUS_ADDRESS={atspi_bus_address:?} /usr/lib/at-spi2-registryd >/dev/null 2>&1 &\n\
             export DBUS_SESSION_BUS_ADDRESS={session_bus_address:?}\n\
             export AT_SPI_BUS_ADDRESS={atspi_bus_address:?}\n\
             export WLR_BACKENDS=headless\n\
             export WLR_LIBINPUT_NO_DEVICES=1\n\
             {renderer_line}\
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
    )?;
    set_mode_0700(&scope_script)?;

    let mut cmd = Command::new("systemd-run");
    cmd.args([
        "--user",
        "--scope",
        "--collect",
        &format!("--unit={scope_unit}"),
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
    let scope_child = cmd.spawn().context("spawn systemd-run --user --scope")?;
    let sway_pid = scope_child.id();

    wait_until(Duration::from_secs(10), || env_dump.exists())
        .await
        .context("sway did not write sway-env.json")?;
    let env_json: Value = serde_json::from_str(&std::fs::read_to_string(&env_dump)?)?;
    let wl_name = env_json["WAYLAND_DISPLAY"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("empty WAYLAND_DISPLAY"))?;
    let x11_display = env_json["DISPLAY"].as_str().unwrap_or_default().to_string();
    let wayland_socket = runtime_dir.join(wl_name);

    let (_notif_conn, _notif_stats) = start_notifications_stub(&session_bus_address).await?;

    // Launch GTK4 fixture inside the isolated display scope via swaymsg exec so its process
    // lives inside the transient scope alongside sway!
    let gtk4_script = spike_manifest_dir().join("fixtures/gtk4_app.py");
    let gtk4_state_file = base_dir.join("gtk4-state.json");
    let exec_cmd = format!(
        "exec env GTK_USE_PORTAL=0 GTK_A11Y=atspi GDK_BACKEND=wayland WAYLAND_DISPLAY={wl_name} XDG_RUNTIME_DIR={} DBUS_SESSION_BUS_ADDRESS={session_bus_address} AT_SPI_BUS_ADDRESS={atspi_bus_address} python3 {} {}",
        runtime_dir.display(),
        gtk4_script.display(),
        gtk4_state_file.display()
    );
    swaymsg(&sway_socket, &[&exec_cmd])?;
    wait_until(Duration::from_secs(10), || gtk4_state_file.exists())
        .await
        .context("GTK4 fixture did not start inside sway scope")?;
    let gtk4_json: Value = serde_json::from_str(&std::fs::read_to_string(&gtk4_state_file)?)?;
    let gtk4_pid = gtk4_json["pid"]
        .as_u64()
        .ok_or_else(|| anyhow!("missing gtk4 pid"))? as u32;

    let persisted = PersistedDisplayState {
        scope_unit: format!("{scope_unit}.scope"),
        base_dir: base_dir.to_path_buf(),
        runtime_dir,
        wayland_socket,
        sway_socket,
        x11_display,
        session_bus_socket,
        session_bus_address,
        atspi_bus_socket,
        atspi_bus_address,
        gtk4_state_file,
        sway_pid,
        gtk4_pid,
    };
    let tmp_state = base_dir.join("state.json.tmp");
    std::fs::write(&tmp_state, serde_json::to_string_pretty(&persisted)?)?;
    std::fs::rename(&tmp_state, base_dir.join("state.json"))?;

    // Stay alive until parent test sends SIGKILL.
    futures_forever().await;
    Ok(())
}

async fn futures_forever() {
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

fn spawn_isolated_client(
    program: &Path,
    args: &[&str],
    state: &PersistedDisplayState,
    with_wayland: bool,
    with_atspi_env: bool,
    extra_env: &[(&str, &str)],
) -> Result<Child> {
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let path = format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin");

    let mut cmd = Command::new(program);
    cmd.env_clear()
        .env("HOME", &home)
        .env("USER", &user)
        .env("PATH", &path)
        .env("XDG_RUNTIME_DIR", &state.runtime_dir)
        .env("DBUS_SESSION_BUS_ADDRESS", &state.session_bus_address)
        .env("DISPLAY", &state.x11_display)
        .env("GTK_USE_PORTAL", "0")
        .env("GTK_MODULES", "gail:atk-bridge")
        .env("GTK_A11Y", "atspi")
        .env("QT_ACCESSIBILITY", "1")
        .env("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if with_wayland && let Some(name) = state.wayland_socket.file_name() {
        cmd.env("WAYLAND_DISPLAY", name);
    }
    if with_atspi_env {
        cmd.env("AT_SPI_BUS_ADDRESS", &state.atspi_bus_address);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.spawn()
        .with_context(|| format!("spawn {}", program.display()))
}

fn compile_qt6_fixture(spike_root: &Path) -> Result<PathBuf> {
    let src = spike_root.join("fixtures/qt6_app.cpp");
    let out_dir = spike_root.join("target");
    std::fs::create_dir_all(&out_dir)?;
    let bin = out_dir.join("qt6_fixture");
    if bin.exists()
        && let (Ok(bm), Ok(sm)) = (bin.metadata(), src.metadata())
        && bm.mtime() >= sm.mtime()
    {
        return Ok(bin);
    }
    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "Qt6Widgets"])
        .output()
        .context("pkg-config Qt6Widgets")?;
    if !flags.status.success() {
        bail!("pkg-config Qt6Widgets failed");
    }
    let flag_str = String::from_utf8_lossy(&flags.stdout);
    let mut cmd = Command::new("g++");
    cmd.arg("-O2").arg("-fPIC").arg(&src).arg("-o").arg(&bin);
    for token in flag_str.split_whitespace() {
        cmd.arg(token);
    }
    let status = cmd.status().context("compile qt6_app.cpp")?;
    if !status.success() {
        bail!("g++ failed compiling qt6_app.cpp");
    }
    Ok(bin)
}

async fn snapshot_host(host_xdg: &Path) -> Result<HostSnapshot> {
    let mut wayland_sockets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(host_xdg) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("wayland-") && !name.ends_with(".lock") {
                wayland_sockets.push(name);
            }
        }
    }
    wayland_sockets.sort();

    let bus0 = host_xdg.join("at-spi/bus_0");
    let (atspi_bus0_inode, atspi_bus0_mtime) = match std::fs::symlink_metadata(&bus0) {
        Ok(m) => (Some(m.ino()), Some(m.mtime())),
        Err(_) => (None, None),
    };
    let x0_inode = std::fs::symlink_metadata("/tmp/.X11-unix/X0")
        .ok()
        .map(|m| m.ino());

    let (hypr_cursor, hypr_active_window) = read_hyprland_readonly(host_xdg);

    let host_atspi_apps = if bus0.exists() {
        let addr = format!("unix:path={}", bus0.display());
        inspect_atspi_bus(&addr)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|a| format!("{}:{}", a.toolkit, a.name))
            .collect()
    } else {
        Vec::new()
    };

    Ok(HostSnapshot {
        wayland_sockets,
        atspi_bus0_inode,
        atspi_bus0_mtime,
        x0_inode,
        hypr_cursor,
        hypr_active_window,
        host_atspi_apps,
    })
}

fn read_hyprland_readonly(host_xdg: &Path) -> (Option<String>, Option<String>) {
    let hypr_dir = host_xdg.join("hypr");
    let Some(sig) = std::fs::read_dir(&hypr_dir)
        .ok()
        .and_then(|mut it| it.find_map(|e| e.ok()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
    else {
        return (None, None);
    };
    if !which_exists("hyprctl") {
        return (None, None);
    }
    let cursor = Command::new("hyprctl")
        .env("XDG_RUNTIME_DIR", host_xdg)
        .env("HYPRLAND_INSTANCE_SIGNATURE", &sig)
        .arg("cursorpos")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let active = Command::new("hyprctl")
        .env("XDG_RUNTIME_DIR", host_xdg)
        .env("HYPRLAND_INSTANCE_SIGNATURE", &sig)
        .args(["activewindow", "-j"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
        .map(|v| {
            format!(
                "{}:{}",
                v["address"].as_str().unwrap_or(""),
                v["pid"].as_i64().unwrap_or(0)
            )
        });
    (cursor, active)
}

fn verify_host_untouched(before: &HostSnapshot, after: &HostSnapshot) -> Result<()> {
    if before.wayland_sockets != after.wayland_sockets {
        bail!(
            "host Wayland sockets changed: {:?} -> {:?}",
            before.wayland_sockets,
            after.wayland_sockets
        );
    }
    if before.atspi_bus0_inode != after.atspi_bus0_inode
        || before.atspi_bus0_mtime != after.atspi_bus0_mtime
    {
        bail!(
            "host /run/user/*/at-spi/bus_0 was modified or unlinked! {:?}/{:?} -> {:?}/{:?}",
            before.atspi_bus0_inode,
            before.atspi_bus0_mtime,
            after.atspi_bus0_inode,
            after.atspi_bus0_mtime
        );
    }
    if before.x0_inode != after.x0_inode {
        bail!(
            "host X0 socket changed: {:?} -> {:?}",
            before.x0_inode,
            after.x0_inode
        );
    }
    if before.hypr_active_window != after.hypr_active_window {
        bail!(
            "host Hyprland active window changed: {:?} -> {:?}",
            before.hypr_active_window,
            after.hypr_active_window
        );
    }
    Ok(())
}

fn cleanup_scope(state: &PersistedDisplayState) {
    let _ = swaymsg(&state.sway_socket, &["exit"]);
    let _ = Command::new("systemctl")
        .args(["--user", "stop", &state.scope_unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn is_unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .is_ok_and(|s| s.success())
}

fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn swaymsg(sock: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("swaymsg")
        .arg("-s")
        .arg(sock)
        .args(args)
        .output()
        .context("invoke swaymsg")?;
    if !out.status.success() {
        bail!(
            "swaymsg {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn swaymsg_json(sock: &Path, args: &[&str]) -> Result<Value> {
    let raw = swaymsg(sock, args)?;
    Ok(serde_json::from_str(&raw)?)
}

fn collect_window_titles(node: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_titles_rec(node, &mut out);
    out
}

fn collect_titles_rec(node: &Value, out: &mut Vec<String>) {
    let is_leaf_con = node["pid"].as_i64().unwrap_or(0) > 0;
    if is_leaf_con && let Some(name) = node["name"].as_str() {
        out.push(name.to_string());
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(arr) = node[key].as_array() {
            for child in arr {
                collect_titles_rec(child, out);
            }
        }
    }
}

fn read_gtk4_state(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

async fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    if cond() {
        Ok(())
    } else {
        bail!("condition did not hold within {:?}", timeout)
    }
}

fn spike_manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn set_mode_0700(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn read_os_pretty_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .unwrap_or("\"Linux\"")
        .trim_matches('"')
        .to_string()
}

fn run_cmd_trimmed(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn which_exists(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn round2(val: f64) -> f64 {
    (val * 100.0).round() / 100.0
}
