//! Phase 1 Linux end-to-end test:
//! Real `agent-screen mcp` (stdio JSON-RPC) -> auto-started `agent-screen`
//! daemon -> isolated headless `sway` Agent Display beside a simulated host
//! `sway`, verifying capability pruning, Web Preview, GTK4 Wayland + Xwayland
//! launches, `computer` clicks/drags/Chinese typing/`ctrl+c`/`ctrl+v`, zero
//! host interference, and daemon `SIGKILL` survival + `adoptGroups` recovery.

#![cfg(target_os = "linux")]

use agent_screen_linux_e2e::{
    McpStdioClient, SimulatedHostSway, control_call, snapshot_physical_desktop, workspace_root,
};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn linux_mvp_daemon_and_mcp_end_to_end() -> Result<()> {
    run_suite("systemd", &[]).await
}

#[tokio::test]
async fn setsid_fallback_survives_daemon_sigkill() -> Result<()> {
    run_suite("setsid", &[("AGENTSCREEN_FORCE_SETSID", "1")]).await
}

async fn run_suite(tag: &str, extra_env: &[(&str, &str)]) -> Result<()> {
    let physical_before = snapshot_physical_desktop();

    let scratch = PathBuf::from(format!("/tmp/as-linux-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;

    // Keep real XDG_RUNTIME_DIR when present so `systemd-run --user --scope`
    // talks to the live user systemd bus; isolate all sockets/files via a
    // unique AGENTSCREEN_INSTANCE.
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

    let fixture_py = workspace_root().join("tests/linux-e2e/fixtures/gtk4_e2e.py");

    // 1. Start simulated host `sway` with a focused user window to verify zero focus theft.
    let mut sim_host = SimulatedHostSway::start(&scratch)?;
    let host_gtk_state = scratch.join("host-gtk-state.json");
    sim_host.launch_host_gtk_window(&fixture_py, &host_gtk_state)?;
    assert_eq!(
        sim_host.focused_window_title().as_deref(),
        Some("Host User Window")
    );

    let run_res = run_e2e_flow(
        &runtime_dir,
        &state_home,
        &scratch,
        &instance,
        &control_sock,
        &web_state_file,
        &fixture_py,
        &sim_host,
        extra_env,
    )
    .await;

    // Cleanup daemon & instance artifacts regardless of test outcome.
    let _ = control_call(&control_sock, "debugQuit", json!({})).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    let _ = std::fs::remove_file(&control_sock);
    let _ = std::fs::remove_file(&web_state_file);
    let _ = std::fs::remove_dir_all(as_state.join(format!("displays-{instance}")));
    let _ = std::fs::remove_dir_all(&scratch);
    drop(sim_host);

    run_res?;

    let physical_after = snapshot_physical_desktop();
    assert_eq!(
        physical_before.atspi_bus0_inode, physical_after.atspi_bus0_inode,
        "physical host /run/user/*/at-spi/bus_0 inode must never change"
    );
    assert_eq!(
        physical_before.hypr_active, physical_after.hypr_active,
        "physical host Hyprland active window must never change"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_e2e_flow(
    runtime_dir: &Path,
    state_home: &Path,
    scratch: &Path,
    instance: &str,
    control_sock: &Path,
    web_state_file: &Path,
    fixture_py: &Path,
    sim_host: &SimulatedHostSway,
    extra_env: &[(&str, &str)],
) -> Result<()> {
    // 2. Spawn `agent-screen mcp` BEFORE any daemon is running!
    let mut mcp = McpStdioClient::spawn(runtime_dir, state_home, instance, extra_env)?;

    // `list_tools` must succeed immediately without starting the daemon and
    // expose the 15 Linux-pruned tools (no window_access, no input_queue).
    let tools = mcp.list_tools()?;
    assert!(
        !control_sock.exists(),
        "list_tools must not block to auto-start the daemon"
    );
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        tools.len(),
        15,
        "expected 15 Linux tools, got {tool_names:?}"
    );
    assert!(!tool_names.contains(&"window_access"));
    assert!(!tool_names.contains(&"input_queue"));
    let comp = tools.iter().find(|t| t["name"] == "computer").unwrap();
    assert!(
        comp["inputSchema"]["properties"]
            .get("real_input")
            .is_none()
    );
    assert!(
        comp["inputSchema"]["properties"]
            .get("via_input_method")
            .is_none()
    );

    // 3. `create_display` auto-starts the `agent-screen` daemon, negotiates
    //    `agentReady`, creates isolated sway display 1, and returns `preview_url`.
    let created = mcp.call_tool("create_display", json!({ "title": "Linux MVP Test" }))?;
    let c_obj = &created["structuredContent"];
    let display_id = c_obj["display_id"].as_i64().context("display_id")?;
    assert_eq!(display_id, 1);
    assert_eq!(c_obj["width"], 1280);
    assert_eq!(c_obj["height"], 800);
    assert_eq!(c_obj["scale"], 1.0);
    let preview_url = c_obj["preview_url"]
        .as_str()
        .context("create_display must return preview_url")?
        .to_string();
    assert!(
        preview_url.starts_with("http://127.0.0.1:"),
        "{preview_url}"
    );

    // Verify `get_diagnostics` matches the running daemon's compiled identity.
    let diag = mcp.call_tool("get_diagnostics", json!({}))?;
    assert_eq!(diag["structuredContent"]["app"]["status"], "available");

    // 4. Verify Web Preview HTML, `/frame.jpg`, `/stream` MJPEG, and 404 on bad token.
    check_web_preview(&preview_url).await?;

    // 5. Launch both a native Wayland GTK4 app and an Xwayland X11 GTK4 app!
    let wayland_app_state = scratch.join("agent-gtk-wayland.json");
    let launched_wl = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "python3",
            "arguments": [
                fixture_py.to_str().unwrap(),
                wayland_app_state.to_str().unwrap(),
                "Agent Wayland GTK4"
            ],
            "environment": {
                "GDK_BACKEND": "wayland"
            }
        }),
    )?;
    let wl_win_id = launched_wl["structuredContent"]["window"]["windowId"]
        .as_u64()
        .context("Wayland windowId")?;

    let x11_app_state = scratch.join("agent-gtk-x11.json");
    let launched_x11 = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "python3",
            "arguments": [
                fixture_py.to_str().unwrap(),
                x11_app_state.to_str().unwrap(),
                "Agent Xwayland X11"
            ],
            "environment": {
                "GDK_BACKEND": "x11"
            }
        }),
    )?;
    let x11_win_id = launched_x11["structuredContent"]["window"]["windowId"]
        .as_u64()
        .context("Xwayland windowId")?;
    assert_ne!(wl_win_id, x11_win_id);

    wait_json_field(&x11_app_state, |v| {
        v["backend"]
            .as_str()
            .is_some_and(|b| b.to_ascii_lowercase().contains("x11"))
    })
    .await
    .context("Xwayland fixture did not report X11 backend")?;

    // `list_windows` shows both windows on display 1.
    let win_list = mcp.call_tool("list_windows", json!({ "display_id": display_id }))?;
    let wins = win_list["structuredContent"]["windows"]
        .as_array()
        .context("windows array")?;
    assert_eq!(wins.len(), 2, "expected 2 windows, got {wins:?}");

    // `place_window` positions and focuses the Wayland GTK4 window at (20, 20, 640, 440).
    let placed = mcp.call_tool(
        "place_window",
        json!({
            "display_id": display_id,
            "window_id": wl_win_id,
            "x": 20,
            "y": 20,
            "width": 640,
            "height": 440
        }),
    )?;
    assert_eq!(placed["structuredContent"]["placed"], true);

    // 6. Drive `computer` tool: click, drag, Chinese UTF-8 type, and ctrl+c/ctrl+v!
    let click_res = mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "left_click",
            "coordinate": [200, 68]
        }),
    )?;
    verify_tool_screenshot(&click_res)?;
    wait_json_field(&wayland_app_state, |v| v["clicks"].as_u64() == Some(1))
        .await
        .context("GTK4 button click did not register")?;

    let drag_res = mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "left_click_drag",
            "start_coordinate": [120, 160],
            "coordinate": [290, 190]
        }),
    )?;
    verify_tool_screenshot(&drag_res)?;
    wait_json_field(&wayland_app_state, |v| {
        v["drag_delta"][0].as_f64().unwrap_or(0.0) >= 120.0
    })
    .await
    .context("GTK4 drag did not register")?;

    // Click entry field (around y = 250) and type Chinese + ASCII UTF-8.
    mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "left_click",
            "coordinate": [200, 250]
        }),
    )?;
    let typed_phrase = "你好 Agent 2026";
    mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "type",
            "text": typed_phrase
        }),
    )?;
    wait_json_field(&wayland_app_state, |v| {
        v["entry_text"].as_str() == Some(typed_phrase)
    })
    .await
    .context("Chinese UTF-8 typing did not reach GTK4 entry")?;

    // Exercise in-display clipboard shortcuts: ctrl+a -> ctrl+c -> Right -> ctrl+v
    mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "key",
            "text": "ctrl+a ctrl+c Right ctrl+v"
        }),
    )?;
    let doubled = format!("{typed_phrase}{typed_phrase}");
    wait_json_field(&wayland_app_state, |v| {
        v["entry_text"].as_str() == Some(doubled.as_str())
    })
    .await
    .context("in-display ctrl+c / ctrl+v failed")?;

    // Verify `zoom` screenshot works.
    let zoom_res = mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "zoom",
            "region": [0, 0, 320, 240]
        }),
    )?;
    verify_tool_screenshot(&zoom_res)?;

    // Simulated host compositor window MUST still hold focus!
    assert_eq!(
        sim_host.focused_window_title().as_deref(),
        Some("Host User Window"),
        "simulated host window must never lose focus during agent operations"
    );

    // 7. Daemon SIGKILL crash survival + `adoptGroups` reattachment!
    let web_meta: Value = serde_json::from_str(&std::fs::read_to_string(web_state_file)?)?;
    let old_daemon_pid = web_meta["pid"].as_u64().context("daemon pid in web.json")? as i32;
    assert_eq!(unsafe { libc::kill(old_daemon_pid, libc::SIGKILL) }, 0);
    // Wait until old daemon process is reaped/gone and socket stops answering.
    let kill_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < kill_deadline {
        if tokio::net::UnixStream::connect(control_sock).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    // Using the SAME MCP client (which holds `orphans` for `e2e-session-main`),
    // call `create_display` -> auto-restarts `agent-screen`, sends `adoptGroups`,
    // and reattaches display 1 with both open windows intact!
    let reattached = mcp.call_tool("create_display", json!({ "title": "Linux MVP Test" }))?;
    let r_obj = &reattached["structuredContent"];
    assert_eq!(r_obj["display_id"], display_id);
    assert_eq!(r_obj["reused"], true);
    assert!(
        r_obj["note"]
            .as_str()
            .unwrap_or_default()
            .contains("reattached"),
        "expected reattach note, got {r_obj}"
    );
    let new_web_meta: Value = serde_json::from_str(&std::fs::read_to_string(web_state_file)?)?;
    let new_daemon_pid = new_web_meta["pid"].as_u64().unwrap_or(0) as i32;
    assert_ne!(new_daemon_pid, old_daemon_pid);

    let after_reattach = mcp.call_tool("list_windows", json!({ "display_id": display_id }))?;
    let surviving_wins = after_reattach["structuredContent"]["windows"]
        .as_array()
        .context("surviving windows")?;
    assert_eq!(
        surviving_wins.len(),
        2,
        "both Wayland and Xwayland windows must survive daemon SIGKILL"
    );

    // 8. `set_disposition` + `end_display` -> Review Display keeps windows alive
    //    while immediately expiring Web Preview token (404)!
    let fresh_preview_url = r_obj["preview_url"]
        .as_str()
        .context("reattached preview_url")?
        .to_string();
    mcp.call_tool(
        "set_disposition",
        json!({
            "display_id": display_id,
            "window_id": wl_win_id,
            "disposition": "keep"
        }),
    )?;
    mcp.call_tool("end_display", json!({ "display_id": display_id }))?;

    let status_after_end = http_get_status(&fresh_preview_url).await?;
    assert_eq!(
        status_after_end, 404,
        "preview_url must return 404 immediately after end_display"
    );
    // GTK4 process must still be alive in the Review Display (ADR 0020)!
    let wl_state_json: Value = serde_json::from_str(&std::fs::read_to_string(&wayland_app_state)?)?;
    let wl_pid = wl_state_json["pid"].as_i64().unwrap_or(0) as i32;
    assert_eq!(
        unsafe { libc::kill(wl_pid, 0) },
        0,
        "kept window in Review Display must stay running after end_display"
    );

    // User dismisses the Review Display -> terminates scope and closes windows.
    control_call(
        control_sock,
        "debugDismissDisplay",
        json!({ "displayId": display_id }),
    )
    .await?;

    Ok(())
}

fn verify_tool_screenshot(tool_res: &Value) -> Result<()> {
    let content = tool_res["content"].as_array().context("tool content")?;
    let img = content
        .iter()
        .find(|c| c["type"] == "image")
        .context("missing image block in computer output")?;
    let b64 = img["data"].as_str().context("image base64")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
    if bytes.len() < 100 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        bail!("invalid JPEG screenshot bytes (len={})", bytes.len());
    }
    Ok(())
}

async fn check_web_preview(preview_url: &str) -> Result<()> {
    let (addr, path) = split_http_url(preview_url)?;
    let (page_status, page_head, page_body) = http_get_raw(&addr, &path).await?;
    assert_eq!(page_status, 200);
    assert!(
        page_head
            .to_ascii_lowercase()
            .contains("content-security-policy:"),
        "{page_head}"
    );
    let body_str = String::from_utf8_lossy(&page_body);
    assert!(body_str.contains("/stream"), "{body_str}");

    let (jpg_status, _, jpg_body) = http_get_raw(&addr, &format!("{path}/frame.jpg")).await?;
    assert_eq!(jpg_status, 200);
    assert!(jpg_body.len() > 100 && jpg_body[0] == 0xFF && jpg_body[1] == 0xD8);

    // Read initial MJPEG frame from `/stream`
    let mut stream = TcpStream::connect(&addr).await?;
    let req = format!("GET {path}/stream HTTP/1.1\r\nHost: {addr}\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut buf = vec![0u8; 32768];
    let mut collected = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline && collected.len() < 512 {
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await??;
        if n == 0 {
            break;
        }
        collected.extend_from_slice(&buf[..n]);
        if collected.windows(7).any(|w| w == b"--frame") && collected.len() > 300 {
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&collected).contains("multipart/x-mixed-replace"),
        "expected multipart stream header"
    );
    drop(stream);

    let (bad_status, _, _) = http_get_raw(&addr, "/d/00000000000000000000000000000000").await?;
    assert_eq!(bad_status, 404);
    Ok(())
}

fn split_http_url(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("http://")
        .context("preview_url must start with http://")?;
    let (addr, path) = rest.split_once('/').context("preview_url path")?;
    Ok((addr.to_string(), format!("/{path}")))
}

async fn http_get_status(url: &str) -> Result<u16> {
    let (addr, path) = split_http_url(url)?;
    let (status, _, _) = http_get_raw(&addr, &path).await?;
    Ok(status)
}

async fn http_get_raw(addr: &str, path: &str) -> Result<(u16, String, Vec<u8>)> {
    let mut stream = TcpStream::connect(addr).await?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("HTTP response header boundary")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .context("HTTP status code")?;
    Ok((status, head, raw[split + 4..].to_vec()))
}

async fn wait_json_field(path: &Path, mut pred: impl FnMut(&Value) -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(raw) = std::fs::read_to_string(path)
            && let Ok(v) = serde_json::from_str::<Value>(&raw)
            && pred(&v)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let last = std::fs::read_to_string(path).unwrap_or_default();
    bail!("timed out waiting on {}: last={last}", path.display())
}
