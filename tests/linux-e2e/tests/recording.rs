//! Phase 5a end-to-end test for Linux `record_display` (`ffmpeg` MP4 + version-1
//! `.timeline.jsonl` sidecar) and the GPUI `RecordingsView` playback window.

#![cfg(target_os = "linux")]

use agent_screen_linux_e2e::{
    McpStdioClient, SimulatedHostSway, control_call, snapshot_physical_desktop, workspace_root,
};
use agent_screen_model::recording_playback::{Support, Timeline};
use anyhow::{Context, Result};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linux_record_display_writes_mp4_and_timeline_and_loads_in_playback_ui() -> Result<()> {
    let host_before = snapshot_physical_desktop();

    let scratch = PathBuf::from(format!("/tmp/asr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let rec_dir = scratch.join("recordings");
    let state_home = scratch.join("state");
    std::fs::create_dir_all(&rec_dir)?;
    std::fs::create_dir_all(&state_home)?;

    let sim_host = SimulatedHostSway::start(&scratch)?;
    let wl_name = sim_host.wayland_display_name();
    let instance = format!("rec-{}", std::process::id());
    let control_sock = sim_host
        .dir
        .join("agent-screen")
        .join(format!("control-{instance}.sock"));

    let extra_env = [
        ("AGENTSCREEN_HEADLESS", "0"),
        ("WAYLAND_DISPLAY", wl_name.as_str()),
        ("AGENTSCREEN_RECORDING_DIR", rec_dir.to_str().unwrap()),
        ("AGENTSCREEN_PLAYBACK_DIR", rec_dir.to_str().unwrap()),
        ("AGENTSCREEN_NO_SYSTEMD", "1"),
        ("AGENTSCREEN_DEBUG", "1"),
    ];
    let mut mcp = McpStdioClient::spawn(&sim_host.dir, &state_home, &instance, &extra_env)?;

    // 1. Create display and launch GTK4 fixture.
    let created = mcp.call_tool("create_display", json!({ "title": "Record Demo" }))?;
    let display_id = created["structuredContent"]["display_id"]
        .as_u64()
        .context("display_id")?;

    let fixture_py = workspace_root().join("tests/linux-e2e/fixtures/gtk4_e2e.py");
    let gtk_state = scratch.join("gtk-rec.json");
    let _ = mcp.call_tool(
        "launch_app",
        json!({
            "display_id": display_id,
            "app": "python3",
            "arguments": [
                fixture_py.to_str().unwrap(),
                gtk_state.to_str().unwrap(),
                "GTK4 Record Target"
            ],
            "environment": { "GDK_BACKEND": "wayland" }
        }),
    )?;

    // 2. Start recording via real MCP `record_display` (`action: "start"`).
    let started = mcp.call_tool(
        "record_display",
        json!({
            "display_id": display_id,
            "action": "start"
        }),
    )?;
    let start_obj = &started["structuredContent"];
    assert_eq!(start_obj["selectedCodec"], "avc1");
    let video_path = PathBuf::from(start_obj["video"].as_str().context("video path")?);
    let timeline_path = PathBuf::from(start_obj["timeline"].as_str().context("timeline path")?);

    // 3. Perform agent pointer + typing actions while recording, plus a brief Takeover
    //    (verifying user pointer movements during Takeover are hidden per ADR).
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "left_click",
            "coordinate": [200, 68]
        }),
    )?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = mcp.call_tool(
        "computer",
        json!({
            "display_id": display_id,
            "action": "mouse_move",
            "coordinate": [480, 320]
        }),
    )?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let _ = control_call(
        &control_sock,
        "debugTakeover",
        json!({ "displayId": display_id, "active": true }),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = control_call(
        &control_sock,
        "debugTakeover",
        json!({ "displayId": display_id, "active": false }),
    )
    .await?;

    // 4. Stop recording via real MCP `record_display` (`action: "stop"`).
    let stopped = mcp.call_tool(
        "record_display",
        json!({
            "display_id": display_id,
            "action": "stop"
        }),
    )?;
    let stop_obj = &stopped["structuredContent"];
    assert!(
        stop_obj["problem"].is_null(),
        "recording reported a problem: {stop_obj}"
    );
    assert!(
        stop_obj["actions"].as_u64().unwrap_or(0) >= 2,
        "expected >=2 recorded actions: {stop_obj}"
    );

    // 5. Validate MP4 container via `ffprobe` and `.timeline.jsonl` via `Timeline::parse`.
    let video_meta = std::fs::metadata(&video_path).context("stat video.mp4")?;
    assert!(
        video_meta.len() > 512,
        "MP4 file is too small: {}",
        video_meta.len()
    );

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type,width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(&video_path)
        .output()
        .context("run ffprobe on recorded MP4")?;
    assert!(probe.status.success(), "ffprobe failed on recorded MP4");
    let probe_str = String::from_utf8_lossy(&probe.stdout);
    assert!(
        probe_str.contains("video") && probe_str.contains("1280") && probe_str.contains("800"),
        "unexpected ffprobe stream info: {probe_str}"
    );

    let tl_src = std::fs::read_to_string(&timeline_path).context("read .timeline.jsonl")?;
    let parsed_tl = Timeline::parse(Some(&tl_src));
    assert_eq!(
        parsed_tl.support,
        Support::Ready,
        "timeline sidecar failed validation:\n{tl_src}"
    );
    assert!(
        tl_src.contains("\"pointerTimelineComplete\":true"),
        "timeline missing complete recordingEnd:\n{tl_src}"
    );

    // 6. Open GPUI `RecordingsView` (`debugShow` `"recordings"`) and verify it discovers
    //    and loads the recorded MP4 + sidecar without crashing, then close it.
    let _ = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "recordings" }),
    )
    .await?;
    let mut found_in_library = false;
    for _ in 0..20 {
        let rec_ui = control_call(
            &control_sock,
            "debugShow",
            json!({ "surface": "recordings" }),
        )
        .await?;
        if rec_ui["entries"].as_u64().unwrap_or(0) >= 1 {
            found_in_library = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    assert!(
        found_in_library,
        "RecordingsView should list the newly recorded MP4"
    );
    let _ = control_call(
        &control_sock,
        "debugShow",
        json!({ "surface": "recordings-close" }),
    )
    .await?;

    let _ = control_call(
        &control_sock,
        "debugDismissDisplay",
        json!({ "displayId": display_id }),
    )
    .await;
    let _ = control_call(&control_sock, "debugQuit", json!({})).await;
    drop(mcp);
    drop(sim_host);
    let _ = std::fs::remove_dir_all(&scratch);

    let host_after = snapshot_physical_desktop();
    assert_eq!(host_before, host_after);
    Ok(())
}
