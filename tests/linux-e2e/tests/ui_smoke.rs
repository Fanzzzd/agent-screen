//! Phase 4 GPUI Linux UI smoke test (`docs/research/linux-port.md` §4 Phase 4):
//! Runs `agent-screen` in full GPUI Wayland GUI mode against an isolated
//! `SimulatedHostSway` compositor and private session D-Bus (never touching the
//! developer's physical Hyprland/GNOME/KDE session) to verify:
//! 1. GNOME fallback compact `StatusWindow` auto-opens when `StatusNotifierWatcher` is absent.
//! 2. `debugShow` `peek` opens via `WindowKind::LayerShell` on `sway`.
//! 3. `debugShow` `viewer` opens GPUI `ViewerView`, streams live `RenderImage` frames,
//!    and `swapFrames` across 40 consecutive frames drops every superseded texture
//!    (`residentTextures <= 1`, `uploadedFrames - droppedFrames <= 1`).
//! 4. `requestHelp` auto-surfaces `HelpCardView` with the 3 Linux answers
//!    (`take-over`, `continue`, `reply` + `later`, no `give-window`), and
//!    `card-reply` / `card-take-over` transition Takeover state cleanly.
//! 5. ADR 0014 `viewer_keys` password hold in `ViewerView`: focusing GTK4's
//!    password entry settles `secrecy == "secure"` off the main thread
//!    (`mainThreadLookups == 0`) and blocks IME composition (`secureFound >= 1`).
//! 6. Review Display `keep-review` and `dismiss` cleanly retain or shut down
//!    ended displays with open windows.

#![cfg(target_os = "linux")]

use agent_screen_linux_e2e::{
    McpStdioClient, SimulatedHostSway, control_call, find_element_center, first_text_block,
    snapshot_physical_desktop, workspace_root,
};
use anyhow::{Context, Result};
use serde_json::json;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct PrivateSessionBus {
    address: String,
    child: Child,
}

impl PrivateSessionBus {
    fn start(dir: &Path) -> Result<Self> {
        let sock = dir.join("sim-host-dbus.sock");
        let cfg = dir.join("sim-host-dbus.conf");
        let addr = format!("unix:path={}", sock.display());
        std::fs::write(
            &cfg,
            format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\"\n \
                 \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
                 <busconfig>\n  <type>session</type>\n  <listen>{addr}</listen>\n  \
                 <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    \
                 <allow eavesdrop=\"true\"/>\n    <allow own=\"*\"/>\n  </policy>\n</busconfig>\n"
            ),
        )?;
        let child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", cfg.display()))
            .arg("--nofork")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn private host dbus-daemon")?;

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !sock.exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Self {
            address: addr,
            child,
        })
    }
}

impl Drop for PrivateSessionBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpui_linux_ui_smoke_against_simulated_host_sway() -> Result<()> {
    let host_before = snapshot_physical_desktop();

    let scratch = std::path::PathBuf::from(format!("/tmp/asu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;
    let state_home = scratch.join("state");
    std::fs::create_dir_all(&state_home)?;

    let sim_host = SimulatedHostSway::start(&scratch)?;
    let sim_dbus = PrivateSessionBus::start(&scratch)?;
    let wl_name = sim_host.wayland_display_name();
    let instance = format!("ui-{}", std::process::id());
    let control_sock = sim_host
        .dir
        .join("agent-screen")
        .join(format!("control-{instance}.sock"));

    let extra_env = [
        ("AGENTSCREEN_HEADLESS", "0"),
        ("WAYLAND_DISPLAY", wl_name.as_str()),
        ("DBUS_SESSION_BUS_ADDRESS", sim_dbus.address.as_str()),
        ("AGENTSCREEN_NO_SYSTEMD", "1"),
        ("AGENTSCREEN_DEBUG", "1"),
    ];
    let mut mcp = McpStdioClient::spawn(&sim_host.dir, &state_home, &instance, &extra_env)?;

    // 1. Create an Agent Display and launch GTK4 fixture inside it.
    let created = mcp.call_tool("create_display", json!({ "title": "UI Smoke" }))?;
    let display_id = created["structuredContent"]["display_id"]
        .as_u64()
        .context("display_id")?;

    let fixture_py = workspace_root().join("tests/linux-e2e/fixtures/gtk4_e2e.py");
    let gtk_state = scratch.join("gtk-ui-smoke.json");
    let _launched = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "python3",
            "arguments": [
                fixture_py.to_str().unwrap(),
                gtk_state.to_str().unwrap(),
                "GTK4 UI Smoke"
            ],
            "environment": { "GDK_BACKEND": "wayland" }
        }),
    )?;

    // 2. Verify GNOME fallback compact StatusWindow opens when SNI watcher is absent.
    let mut status_opened = false;
    for _ in 0..30 {
        let icon = control_call(&control_sock, "debugIcon", json!({})).await?;
        if icon["statusWindowOpen"] == true && icon["sniOnline"] == false {
            assert_eq!(icon["state"], "working");
            status_opened = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        status_opened,
        "compact GNOME fallback StatusWindow should open when StatusNotifierWatcher is offline"
    );

    // 3. Verify `debugShow` `peek` uses `LayerShell` on `sway` (`zwlr_layer_shell_v1`).
    let peek = control_call(&control_sock, "debugShow", json!({ "surface": "peek" })).await?;
    assert_eq!(
        peek["layerShell"], true,
        "Peek should bind zwlr_layer_shell_v1 on sway: {peek}"
    );
    let _ = control_call(&control_sock, "debugShow", json!({ "surface": "hide" })).await?;

    // 4. Open Viewer and verify live frame streaming + 40 swaps without texture leaks.
    let _ = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "viewer", "displayId": display_id }),
    )
    .await?;
    let mut got_live_frame = false;
    for _ in 0..30 {
        let v = control_call(
            &control_sock,
            "debugViewer",
            json!({ "displayId": display_id }),
        )
        .await?;
        if v["uploadedFrames"].as_u64().unwrap_or(0) >= 1 {
            assert!(v["residentTextures"].as_u64().unwrap_or(99) <= 1);
            got_live_frame = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(90)).await;
    }
    assert!(
        got_live_frame,
        "Viewer should upload initial live RenderImage frame"
    );

    let swapped = control_call(
        &control_sock,
        "debugViewerInput",
        json!({
            "displayId": display_id,
            "kind": "swapFrames",
            "count": 40
        }),
    )
    .await?;
    let uploaded = swapped["uploadedFrames"].as_u64().unwrap_or(0);
    let dropped = swapped["droppedFrames"].as_u64().unwrap_or(0);
    let resident = swapped["residentTextures"].as_u64().unwrap_or(99);
    assert!(
        uploaded >= 40,
        "expected >=40 uploaded frames, got {swapped}"
    );
    assert_eq!(
        uploaded - dropped,
        1,
        "every superseded RenderImage must be dropped via window.drop_image: {swapped}"
    );
    assert_eq!(resident, 1);

    // 5. Locate the password field first while the agent holds control, then open a real
    //    MCP `request_help` call (which blocks waiting for the user's Help Card answer!).
    let win_list = mcp.call_tool("list_windows", json!({ "display_id": display_id }))?;
    let first_win = &win_list["structuredContent"]["windows"][0];
    let win_id = first_win["windowId"]
        .as_u64()
        .or_else(|| first_win["window_id"].as_u64())
        .context("windowId")?;
    let state_res = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": win_id }),
    )?;
    let tree_txt = first_text_block(&state_res);
    let (pw_x, pw_y) = find_element_center(&tree_txt, "textfield(secure)")
        .or_else(|| find_element_center(&tree_txt, "Password Input"))
        .context("locate password field coordinates")?;

    let help_rpc_id = mcp.send_tool_call(
        "request_help",
        json!({
            "display_id": display_id,
            "reason": "Sign in to continue",
            "steps": ["Enter password", "Click Sign In"]
        }),
    )?;

    let mut card = json!({});
    for _ in 0..30 {
        card = control_call(&control_sock, "debugCard", json!({})).await?;
        if card["highlighted"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    assert_eq!(card["highlighted"], true, "{card}");
    assert!(card["buttons"].get("take-over").is_some(), "{card}");
    assert!(card["buttons"].get("continue").is_some(), "{card}");
    assert!(card["buttons"].get("reply").is_some(), "{card}");
    assert!(card["buttons"].get("give-window").is_none(), "{card}");

    let reply_open = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "card-reply", "displayId": display_id }),
    )
    .await?;
    assert_eq!(reply_open["replyOpen"], true);
    let _ = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "card-reply-cancel", "displayId": display_id }),
    )
    .await?;

    // Transition into Takeover via `card-take-over`.
    let took_over = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "card-take-over", "displayId": display_id }),
    )
    .await?;
    assert_eq!(took_over["answered"], "takeOver");

    // 6. Verify ADR 0014 password focus secrecy in Viewer during Takeover:
    let _ = control_call(
        &control_sock,
        "debugViewerInput",
        json!({
            "displayId": display_id,
            "kind": "click",
            "source": "inputMethod",
            "x": pw_x,
            "y": pw_y
        }),
    )
    .await?;

    // While secrecy is still `unknown`, an InputMethod key is held first!
    let held_key = control_call(
        &control_sock,
        "debugViewerInput",
        json!({
            "displayId": display_id,
            "kind": "key",
            "source": "inputMethod",
            "key": "s",
            "characters": "s"
        }),
    )
    .await?;
    assert_eq!(held_key["route"], "held", "{held_key}");

    // Wait for off-main-thread `focus_secrecy` lookup to settle as `"secure"`.
    let mut settled_secure = false;
    for _ in 0..30 {
        let v = control_call(
            &control_sock,
            "debugViewer",
            json!({ "displayId": display_id }),
        )
        .await?;
        if v["secrecy"] == "secure" {
            assert_eq!(v["secure"], true);
            assert_eq!(v["mainThreadLookups"], 0);
            settled_secure = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    assert!(
        settled_secure,
        "Viewer focus_secrecy should settle to secure on password entry"
    );

    // Verify IME mark/commit is blocked while `secrecy == "secure"`.
    let blocked_mark = control_call(
        &control_sock,
        "debugViewerInput",
        json!({
            "displayId": display_id,
            "kind": "mark",
            "text": "ni hao"
        }),
    )
    .await?;
    assert!(blocked_mark["marked"].is_null(), "{blocked_mark}");
    assert!(blocked_mark["secureFound"].as_u64().unwrap_or(0) >= 1);

    // Hand control back -> wakes the waiting `request_help` MCP call, then end the display!
    let _ = control_call(
        &control_sock,
        "debugTakeover",
        json!({ "displayId": display_id, "active": false }),
    )
    .await?;
    let help_done = mcp.read_tool_reply(help_rpc_id)?;
    assert_ne!(
        help_done.get("isError"),
        Some(&serde_json::Value::Bool(true))
    );
    let _ = mcp.call_tool("end_display", json!({ "display_id": display_id }))?;

    let v_review = control_call(
        &control_sock,
        "debugViewer",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(v_review["ended"], true);
    assert_eq!(v_review["keptReview"], false);

    // User clicks "Keep Open" (`keep-review`), then "Close Display" (`dismiss`).
    let kept = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "keep-review", "displayId": display_id }),
    )
    .await?;
    assert_eq!(kept["keptReview"], true);

    let v_kept = control_call(
        &control_sock,
        "debugViewer",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(v_kept["keptReview"], true);

    let _ = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "dismiss", "displayId": display_id }),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let _ = control_call(&control_sock, "debugQuit", json!({})).await;
    drop(mcp);
    drop(sim_dbus);
    drop(sim_host);
    let _ = std::fs::remove_dir_all(&scratch);

    // 7. Assert zero interference with the developer's physical Hyprland session!
    let host_after = snapshot_physical_desktop();
    assert_eq!(
        host_before, host_after,
        "GPUI Linux UI smoke test must never touch the physical desktop cursor, focus, or AT-SPI bus"
    );
    Ok(())
}
