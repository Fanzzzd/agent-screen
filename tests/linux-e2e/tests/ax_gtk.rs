//! Phase 2 AT-SPI2 accessibility tests for GTK4 (Wayland) and Qt6 (Wayland + Xwayland):
//! element numbering stability across full/partial reads, `act_on_element`
//! (`press`, `set_value` with readback, `insert_text`, `read_text`),
//! `wait_for_change` (`appears` + `changed`), and `focus_secrecy` (`GetRole() == 40`).

#![cfg(target_os = "linux")]

use agent_screen_linux_e2e::{
    IsolatedTestEnv, McpStdioClient, control_call, find_element_id, first_text_block,
    snapshot_physical_desktop, workspace_root,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

#[tokio::test]
async fn gtk4_and_qt6_atspi_elements_actions_wait_and_secrecy() -> Result<()> {
    let physical_before = snapshot_physical_desktop();
    let env = IsolatedTestEnv::new("ax-gtk-qt")?;
    let res = run_ax_suite(&env).await;
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

async fn run_ax_suite(env: &IsolatedTestEnv) -> Result<()> {
    let mut mcp = McpStdioClient::spawn(&env.runtime_dir, &env.state_home, &env.instance, &[])?;
    let created = mcp.call_tool("create_display", json!({ "title": "AX GTK4 Qt6" }))?;
    let display_id = created["structuredContent"]["display_id"]
        .as_i64()
        .context("display_id")?;

    // 1. Launch GTK4 Wayland fixture
    let fixture_py = workspace_root().join("tests/linux-e2e/fixtures/gtk4_e2e.py");
    let gtk_state = env.scratch.join("gtk-ax.json");
    let launched = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "python3",
            "arguments": [
                fixture_py.to_str().unwrap(),
                gtk_state.to_str().unwrap(),
                "GTK4 AX Window"
            ],
            "environment": { "GDK_BACKEND": "wayland" }
        }),
    )?;
    let gtk_win_id = launched["structuredContent"]["window"]["windowId"]
        .as_i64()
        .context("gtk windowId")?;

    let state1 = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": gtk_win_id }),
    )?;
    let tree1 = first_text_block(&state1);
    let btn_id = find_element_id(&tree1, "Action Button")
        .with_context(|| format!("missing Action Button in tree:\n{tree1}"))?;
    let entry_id = find_element_id(&tree1, "Text Input")
        .with_context(|| format!("missing Text Input in tree:\n{tree1}"))?;
    let pw_id = find_element_id(&tree1, "textfield(secure)")
        .or_else(|| find_element_id(&tree1, "Password Input"))
        .with_context(|| format!("missing Password Input in tree:\n{tree1}"))?;
    let async_btn_id = find_element_id(&tree1, "Spawn Async Status")
        .with_context(|| format!("missing Spawn Async Status in tree:\n{tree1}"))?;

    // Shallow read (`max_depth: 1`) must preserve element numbers on the next full read!
    let _shallow = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": gtk_win_id, "max_depth": 1 }),
    )?;
    let state2 = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": gtk_win_id }),
    )?;
    let tree2 = first_text_block(&state2);
    assert_eq!(find_element_id(&tree2, "Action Button"), Some(btn_id));
    assert_eq!(find_element_id(&tree2, "Text Input"), Some(entry_id));
    assert_eq!(
        find_element_id(&tree2, "Spawn Async Status"),
        Some(async_btn_id)
    );

    // `act_on_element` `press` increments button click counter without moving pointer!
    let pressed = mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": btn_id,
            "action": "press",
            "result": "changes"
        }),
    )?;
    let gtk_json: Value = serde_json::from_str(&std::fs::read_to_string(&gtk_state)?)?;
    assert_eq!(
        gtk_json["clicks"].as_u64(),
        Some(1),
        "press reply: {}",
        first_text_block(&pressed)
    );

    // `set_value` + `read_text` + `insert_text`
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": entry_id,
            "action": "set_value",
            "value": "Hello AT-SPI",
            "result": "none"
        }),
    )?;
    let read_res = mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": entry_id,
            "action": "read_text"
        }),
    )?;
    assert!(
        first_text_block(&read_res).contains("Hello AT-SPI"),
        "{read_res}"
    );

    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": entry_id,
            "action": "insert_text",
            "value": " +UTF8",
            "result": "none"
        }),
    )?;
    let read_res2 = mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": entry_id,
            "action": "read_text"
        }),
    )?;
    assert!(
        first_text_block(&read_res2).contains("+UTF8"),
        "{read_res2}"
    );

    // Focus normal entry -> `focus_secrecy` must be `"open"`.
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": entry_id,
            "action": "focus",
            "result": "none"
        }),
    )?;
    let open_sec = control_call(
        &env.control_sock,
        "debugFocusSecrecy",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(open_sec["secrecy"], "open");

    // Focus password field -> `focus_secrecy` must be `"secure"`, and `read_text` must refuse!
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": pw_id,
            "action": "focus",
            "result": "none"
        }),
    )?;
    let pw_sec = control_call(
        &env.control_sock,
        "debugFocusSecrecy",
        json!({ "displayId": display_id }),
    )
    .await?;
    assert_eq!(pw_sec["secrecy"], "secure");
    assert!(
        mcp.call_tool(
            "act_on_element",
            json!({
                "display_id": display_id,
                "window_id": gtk_win_id,
                "element": pw_id,
                "action": "read_text"
            }),
        )
        .is_err(),
        "read_text on password field must fail closed"
    );

    // `wait_for_change`: trigger delayed async banner and wait for it to appear!
    mcp.call_tool(
        "act_on_element",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "element": async_btn_id,
            "action": "press",
            "result": "none"
        }),
    )?;
    let waited = mcp.call_tool(
        "wait_for_change",
        json!({
            "display_id": display_id,
            "window_id": gtk_win_id,
            "appears": { "text": "Async Banner Ready" },
            "timeout_seconds": 5.0
        }),
    )?;
    let waited_txt = first_text_block(&waited);
    assert!(
        waited_txt.starts_with("appeared"),
        "expected appeared outcome, got: {waited_txt}"
    );
    // Original button and entry MUST still keep their exact element numbers after a new element appeared!
    let state3 = mcp.call_tool(
        "get_window_state",
        json!({ "display_id": display_id, "window_id": gtk_win_id }),
    )?;
    let tree3 = first_text_block(&state3);
    assert_eq!(find_element_id(&tree3, "Action Button"), Some(btn_id));
    assert_eq!(find_element_id(&tree3, "Text Input"), Some(entry_id));

    // 2. If Qt6 development headers are installed, compile and test Qt6 on BOTH Wayland AND Xwayland!
    if let Some(qt_bin) = maybe_build_qt6_fixture(&env.scratch)? {
        for (platform, label) in [("wayland", "Qt6 Wayland"), ("xcb", "Qt6 Xwayland")] {
            let qt_state = env.scratch.join(format!("qt6-{platform}.json"));
            let mut launch_env = serde_json::Map::new();
            launch_env.insert("QT_QPA_PLATFORM".into(), json!(platform));
            let qt_launched = mcp.call_tool(
                "launch_app",
                json!({
                    "display_id": display_id,
                    "app": qt_bin.to_str().unwrap(),
                    "arguments": [qt_state.to_str().unwrap(), label],
                    "environment": launch_env
                }),
            )?;
            let qt_win_id = qt_launched["structuredContent"]["window"]["windowId"]
                .as_i64()
                .with_context(|| format!("{label} windowId"))?;

            let qt_tree_res = mcp.call_tool(
                "get_window_state",
                json!({ "display_id": display_id, "window_id": qt_win_id }),
            )?;
            let qt_tree = first_text_block(&qt_tree_res);
            let qt_btn = find_element_id(&qt_tree, "Qt6 Submit")
                .with_context(|| format!("{label} missing Qt6 Submit:\n{qt_tree}"))?;
            let qt_input = find_element_id(&qt_tree, "Qt6 Text Input")
                .with_context(|| format!("{label} missing Qt6 Text Input:\n{qt_tree}"))?;
            let qt_pw = find_element_id(&qt_tree, "textfield(secure)")
                .or_else(|| find_element_id(&qt_tree, "Qt6 Password Input"))
                .with_context(|| format!("{label} missing Qt6 Password Input:\n{qt_tree}"))?;

            mcp.call_tool(
                "act_on_element",
                json!({
                    "display_id": display_id,
                    "window_id": qt_win_id,
                    "element": qt_input,
                    "action": "set_value",
                    "value": format!("Qt6Val-{platform}"),
                    "result": "none"
                }),
            )?;
            mcp.call_tool(
                "act_on_element",
                json!({
                    "display_id": display_id,
                    "window_id": qt_win_id,
                    "element": qt_btn,
                    "action": "press",
                    "result": "none"
                }),
            )?;

            let qt_data: Value = serde_json::from_str(&std::fs::read_to_string(&qt_state)?)?;
            assert_eq!(qt_data["clicks"].as_u64(), Some(1), "{label}: {qt_data}");
            assert_eq!(
                qt_data["entry_text"].as_str(),
                Some(format!("Qt6Val-{platform}").as_str())
            );

            mcp.call_tool(
                "act_on_element",
                json!({
                    "display_id": display_id,
                    "window_id": qt_win_id,
                    "element": qt_pw,
                    "action": "focus",
                    "result": "none"
                }),
            )?;
            let qt_sec = control_call(
                &env.control_sock,
                "debugFocusSecrecy",
                json!({ "displayId": display_id }),
            )
            .await?;
            assert_eq!(qt_sec["secrecy"], "secure", "{label} password secrecy");
        }
    }

    control_call(
        &env.control_sock,
        "debugDismissDisplay",
        json!({ "displayId": display_id }),
    )
    .await?;
    Ok(())
}

fn maybe_build_qt6_fixture(scratch: &Path) -> Result<Option<std::path::PathBuf>> {
    let ok = Command::new("pkg-config")
        .args(["--exists", "Qt6Widgets"])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Ok(None);
    }
    let cflags = String::from_utf8(
        Command::new("pkg-config")
            .args(["--cflags", "Qt6Widgets"])
            .output()?
            .stdout,
    )?;
    let libs = String::from_utf8(
        Command::new("pkg-config")
            .args(["--libs", "Qt6Widgets"])
            .output()?
            .stdout,
    )?;
    let src = workspace_root().join("tests/linux-e2e/fixtures/qt6_e2e.cpp");
    let out = scratch.join("qt6_e2e_bin");
    let mut cmd = Command::new("g++");
    cmd.arg("-std=c++17")
        .arg("-fPIC")
        .arg(&src)
        .arg("-o")
        .arg(&out);
    for f in cflags.split_whitespace() {
        cmd.arg(f);
    }
    for l in libs.split_whitespace() {
        cmd.arg(l);
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("failed to compile qt6_e2e.cpp");
    }
    Ok(Some(out))
}
