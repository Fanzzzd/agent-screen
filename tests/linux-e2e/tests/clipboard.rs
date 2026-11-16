//! Phase 2 clipboard isolation and Takeover bridge E2E test:
//! verifies background clipboard isolation against simulated host `sway`,
//! in-display `ctrl+c`/`ctrl+v`, Takeover + Viewer-focus text and PNG bridging,
//! and immediate disconnect when Takeover ends.

#![cfg(target_os = "linux")]

use agent_screen_linux_e2e::{
    IsolatedTestEnv, McpStdioClient, SimulatedHostSway, control_call, find_element_id,
    first_text_block, snapshot_physical_desktop, workspace_root,
};
use anyhow::{Context, Result};
use base64::Engine as _;
use serde_json::json;

#[tokio::test]
async fn clipboard_isolated_in_background_and_bridged_only_during_takeover() -> Result<()> {
    let physical_before = snapshot_physical_desktop();
    let env = IsolatedTestEnv::new("clipboard")?;
    let mut sim_host = SimulatedHostSway::start(&env.scratch)?;
    let fixture_py = workspace_root().join("tests/linux-e2e/fixtures/gtk4_e2e.py");
    let host_state = env.scratch.join("host-clip.json");
    sim_host.launch_host_gtk_window(&fixture_py, &host_state)?;

    let res = run_clipboard_suite(&env, &fixture_py).await;
    drop(sim_host);
    env.cleanup().await;
    res?;

    let physical_after = snapshot_physical_desktop();
    assert_eq!(
        physical_before.atspi_bus0_inode,
        physical_after.atspi_bus0_inode
    );
    assert_eq!(physical_before.hypr_active, physical_after.hypr_active);
    Ok(())
}

async fn run_clipboard_suite(env: &IsolatedTestEnv, fixture_py: &std::path::Path) -> Result<()> {
    let mut mcp = McpStdioClient::spawn(&env.runtime_dir, &env.state_home, &env.instance, &[])?;
    let created = mcp.call_tool("create_display", json!({ "title": "Clipboard E2E" }))?;
    let display_id = created["structuredContent"]["display_id"]
        .as_i64()
        .context("display_id")?;

    let agent_state = env.scratch.join("agent-clip.json");
    let launched = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "python3",
            "arguments": [
                fixture_py.to_str().unwrap(),
                agent_state.to_str().unwrap(),
                "Agent Clipboard GTK4"
            ]
        }),
    )?;
    let win_id = launched["structuredContent"]["window"]["windowId"]
        .as_i64()
        .context("windowId")?;

    let state = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": win_id }),
    )?;
    let entry_id =
        find_element_id(&first_text_block(&state), "Text Input").context("missing Text Input")?;

    // 1. Background mode (`takeover = false`): host <-> display bridge MUST refuse to sync!
    let blocked_in = control_call(
        &env.control_sock,
        "debugClipboardBridge",
        json!({
            "displayId": display_id,
            "viewerFocused": true,
            "direction": "hostToDisplay",
            "text": "HostPrivatePassword-DO-NOT-LEAK"
        }),
    )
    .await?;
    assert_eq!(blocked_in["synced"], false);
    assert_eq!(blocked_in["maySync"], false);

    // Agent copies inside display 1 -> stays strictly inside display 1, and display->host bridge refuses!
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": entry_id,
            "action": "set_value",
            "value": "AgentOnlyClip",
            "result": "none"
        }),
    )?;
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": entry_id,
            "action": "focus",
            "result": "none"
        }),
    )?;
    mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "key",
            "text": "ctrl+a ctrl+c"
        }),
    )?;
    let disp_clip = control_call(
        &env.control_sock,
        "debugClipboard",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(disp_clip["text"], "AgentOnlyClip");

    let blocked_out = control_call(
        &env.control_sock,
        "debugClipboardBridge",
        json!({
            "displayId": display_id,
            "direction": "displayToHost"
        }),
    )
    .await?;
    assert_eq!(blocked_out["synced"], false);

    // 2. Enter Takeover WITH focused Viewer -> bidirectional text + PNG bridge activates!
    control_call(
        &env.control_sock,
        "debugTakeover",
        json!({
            "displayId": display_id,
            "active": true,
            "viewerFocused": true
        }),
    )
    .await?;

    let pulled = control_call(
        &env.control_sock,
        "debugClipboardBridge",
        json!({
            "displayId": display_id,
            "direction": "displayToHost"
        }),
    )
    .await?;
    assert_eq!(pulled["synced"], true);
    assert_eq!(pulled["text"], "AgentOnlyClip");

    let pushed_text = control_call(
        &env.control_sock,
        "debugClipboardBridge",
        json!({
            "displayId": display_id,
            "direction": "hostToDisplay",
            "text": "HostTakeoverPaste 中文"
        }),
    )
    .await?;
    assert_eq!(pushed_text["synced"], true);
    let read_bridged = control_call(
        &env.control_sock,
        "debugClipboard",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(read_bridged["text"], "HostTakeoverPaste 中文");

    // Push a 1x1 PNG image through the Takeover clipboard bridge and verify round-trip!
    let tiny_png: [u8; 67] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    ];
    let b64_png = base64::engine::general_purpose::STANDARD.encode(tiny_png);
    let pushed_png = control_call(
        &env.control_sock,
        "debugClipboardBridge",
        json!({
            "displayId": display_id,
            "direction": "hostToDisplay",
            "pngBase64": b64_png
        }),
    )
    .await?;
    assert_eq!(pushed_png["synced"], true);
    let read_png = control_call(
        &env.control_sock,
        "debugClipboard",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(read_png["kind"], "image/png");
    assert_eq!(read_png["pngBase64"], b64_png);

    // 3. Hand control back (`active: false`) -> bridge disconnects immediately!
    control_call(
        &env.control_sock,
        "debugTakeover",
        json!({
            "displayId": display_id,
            "active": false
        }),
    )
    .await?;
    let after_end = control_call(
        &env.control_sock,
        "debugClipboardBridge",
        json!({
            "displayId": display_id,
            "direction": "hostToDisplay",
            "text": "PostTakeoverBlocked"
        }),
    )
    .await?;
    assert_eq!(after_end["synced"], false);
    assert_eq!(after_end["maySync"], false);

    control_call(
        &env.control_sock,
        "debugDismissDisplay",
        json!({ "displayId": display_id }),
    )
    .await?;
    Ok(())
}
