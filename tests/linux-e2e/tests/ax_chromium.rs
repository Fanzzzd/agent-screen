//! Phase 2 Chromium AT-SPI2 + `--remote-debugging-pipe` E2E test:
//! verifies isolated Chromium launch, AT-SPI2 web tree, CDP `<select>`
//! `set_value`, async `wait_for_change`, password `focus_secrecy`, and HTML5
//! drag-and-drop routed over `--remote-debugging-pipe` (`route: "devtools"`).

#![cfg(target_os = "linux")]

use agent_screen_linux_e2e::{
    IsolatedTestEnv, McpStdioClient, control_call, find_element_center, find_element_id,
    first_text_block, snapshot_physical_desktop, workspace_root,
};
use anyhow::{Context, Result};
use serde_json::json;
use std::process::Command;
use std::time::{Duration, Instant};

fn find_chromium_binary() -> Option<&'static str> {
    for bin in [
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
    ] {
        if Command::new("sh")
            .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
            .status()
            .is_ok_and(|s| s.success())
        {
            // Skip Snap-confined chromium in headless container CI without snapd
            let is_snap = std::fs::read_link(format!("/usr/bin/{bin}"))
                .ok()
                .is_some_and(|t| t.to_string_lossy().contains("snap"));
            if !is_snap {
                return Some(bin);
            }
        }
    }
    None
}

#[tokio::test]
async fn chromium_atspi_cdp_select_and_html5_drag() -> Result<()> {
    let Some(browser) = find_chromium_binary() else {
        eprintln!(
            "skipping chromium_atspi_cdp_select_and_html5_drag: no native Chromium installed"
        );
        return Ok(());
    };

    let physical_before = snapshot_physical_desktop();
    let env = IsolatedTestEnv::new("ax-chromium")?;
    let res = run_chromium_suite(&env, browser).await;
    if res.is_err() {
        let log_path = env
            .runtime_dir
            .join("agent-screen")
            .join(format!("displays-{}", env.instance))
            .join("d1/apps.stderr.log");
        if let Ok(log) = std::fs::read_to_string(&log_path) {
            eprintln!("=== Chromium apps.stderr.log ===\n{log}");
        }
    }
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

async fn run_chromium_suite(env: &IsolatedTestEnv, browser: &str) -> Result<()> {
    let mut mcp = McpStdioClient::spawn(&env.runtime_dir, &env.state_home, &env.instance, &[])?;
    let created = mcp.call_tool("create_display", json!({ "title": "Chromium CDP E2E" }))?;
    let display_id = created["structuredContent"]["display_id"]
        .as_i64()
        .context("display_id")?;

    let html = workspace_root().join("tests/linux-e2e/fixtures/web_e2e.html");
    let url = format!("file://{}", html.display());
    let extra_args: Vec<&str> =
        if std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
            .is_ok_and(|s| s.trim() == "1")
        {
            vec!["--no-sandbox"]
        } else {
            Vec::new()
        };
    let launched = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": browser,
            "open": url,
            "arguments": extra_args
        }),
    )?;
    let win_id = launched["structuredContent"]["window"]["windowId"]
        .as_i64()
        .context("Chromium windowId")?;

    // Wait until the web document populates its AT-SPI2 nodes ("Agent Screen Web Fixture").
    let mut tree = String::new();
    let mut last_seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match mcp.call_tool(
            "get_window_state",
            json!({ "display_id": display_id, "window_id": win_id }),
        ) {
            Ok(state) => {
                let t = first_text_block(&state);
                last_seen = t.clone();
                if t.contains("Web Text Input") && t.contains("Drag Chip") {
                    tree = t;
                    break;
                }
            }
            Err(e) => {
                let lw = mcp
                    .call_tool("list_windows", json!({ "display_id": display_id }))
                    .ok();
                last_seen = format!("ERR: {e:#} | list_windows={lw:?}");
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !tree.contains("Web Text Input") {
        anyhow::bail!("Chromium web tree did not populate in time. Last seen:\n{last_seen}");
    }

    let text_id = find_element_id(&tree, "textfield \"Web Text Input\"")
        .or_else(|| find_element_id(&tree, "entry \"Web Text Input\""))
        .with_context(|| format!("missing Web Text Input:\n{tree}"))?;
    let pw_id = find_element_id(&tree, "textfield(secure)")
        .with_context(|| format!("missing Web Password Input:\n{tree}"))?;
    let select_id = find_element_id(&tree, "combobox")
        .with_context(|| format!("missing Plan Selector:\n{tree}"))?;
    let load_btn_id = find_element_id(&tree, "button \"Load Report\"")
        .or_else(|| find_element_id(&tree, "Load Report"))
        .with_context(|| format!("missing Load Report:\n{tree}"))?;

    // 1. `set_value` + `read_text` on web `<input type="text">`
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": text_id,
            "action": "set_value",
            "value": "Browser Value 123",
            "result": "none"
        }),
    )?;
    let read_back = mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": text_id,
            "action": "read_text"
        }),
    )?;
    assert!(
        first_text_block(&read_back).contains("Browser Value 123"),
        "{read_back}"
    );

    // 2. `set_value` on `<select>` via `--remote-debugging-pipe` (`CHOOSE_JS`)!
    let after_select = mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": select_id,
            "action": "set_value",
            "value": "Enterprise",
            "result": "state"
        }),
    )?;
    assert!(
        first_text_block(&after_select).contains("Plan: Enterprise"),
        "expected Plan: Enterprise in tree:\n{}",
        first_text_block(&after_select)
    );

    // 3. Password secrecy (`GetRole() == 40`) on `<input type="password">`
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": pw_id,
            "action": "focus",
            "result": "none"
        }),
    )?;
    let sec = control_call(
        &env.control_sock,
        "debugFocusSecrecy",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(sec["secrecy"], "secure");

    // 4. Async button press + `wait_for_change` (`appears: { text: "Report Loaded" }`)
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "element": load_btn_id,
            "action": "press",
            "result": "none"
        }),
    )?;
    let waited = mcp.call_tool(
        "wait_for_change",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "appears": { "text": "Report Loaded" },
            "timeout_seconds": 5.0
        }),
    )?;
    assert!(
        first_text_block(&waited).starts_with("appeared"),
        "{}",
        first_text_block(&waited)
    );

    // 5. HTML5 drag-and-drop routed over `--remote-debugging-pipe` (`route: "devtools"`)!
    let fresh_state = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": win_id }),
    )?;
    let fresh_tree = first_text_block(&fresh_state);
    let (from_x, from_y) = find_element_center(&fresh_tree, "Drag Chip")
        .with_context(|| format!("missing Drag Chip center:\n{fresh_tree}"))?;
    let (to_x, to_y) = find_element_center(&fresh_tree, "Drop Zone")
        .or_else(|| find_element_center(&fresh_tree, "Drop Here"))
        .with_context(|| format!("missing Drop Zone center:\n{fresh_tree}"))?;

    let drag_res = mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "left_click_drag",
            "start_coordinate": [from_x, from_y],
            "coordinate": [to_x, to_y]
        }),
    )?;
    let drag_txt = first_text_block(&drag_res);
    assert!(
        drag_txt.contains("via DevTools") || drag_txt.contains("devtools"),
        "expected drag routed via DevTools pipe, got: {drag_txt}"
    );

    let waited_drop = mcp.call_tool(
        "wait_for_change",
        json!({
            "display_id": display_id,
            "window_id": win_id,
            "appears": { "text": "Dropped: chip-payload-42" },
            "timeout_seconds": 4.0
        }),
    )?;
    assert!(
        first_text_block(&waited_drop).contains("Dropped: chip-payload-42"),
        "HTML5 drop did not complete: {}",
        first_text_block(&waited_drop)
    );

    control_call(
        &env.control_sock,
        "debugDismissDisplay",
        json!({ "displayId": display_id }),
    )
    .await?;
    Ok(())
}
