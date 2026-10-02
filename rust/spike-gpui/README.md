# spike-gpui

Stage 1 of the Rust plan: can GPUI carry Sidescreen's UI on macOS? This
binary creates a virtual display, captures it with ScreenCaptureKit, and
shows it in a GPUI Viewer opened from a menu bar icon. The header has
working `接手操作` / `窗口 1` buttons.

```sh
spike-gpui/bundle.sh                       # build, bundle, sign
SPIKE_BUSY=1 target/SidescreenSpike.app/Contents/MacOS/spike-gpui
```

`SPIKE_BUSY=1` starts `tools/busy.swift` on the virtual display: an AppKit
window that changes every refresh, standing in for the agent's app. The
spike logs capture and render rates every second with a `spike:` prefix.

The bundle reuses `dev.sidescreen.menubar` and the Apple Development
identity, so it inherits the Swift app's Screen Recording grant. Quit the
Swift app before running the spike.

## Results (2026-10-03, MacBook Pro, macOS 26.2)

| Check | Result |
| --- | --- |
| Menu bar icon | NSStatusItem through objc2-app-kit (GPUI has none). A real click closes and reopens the Viewer. |
| Viewer never takes focus | `WindowKind::PopUp` is a non-activating NSPanel. Opening it, a real HID click on its button, and clicks on the menu bar icon all leave the user's app frontmost; `NSRunningApplication.isActive` stays false. |
| Accessory app from launch | `Application::with_platform` with `ActivationPolicy::Accessory` set before launch: no Dock icon, no activation. |
| Live picture | `surface(CVPixelBuffer)` draws ScreenCaptureKit's 420f buffers without a copy. The Viewer rendered every captured frame: 60 of 60 when capture delivered 60. |
| Accessibility | The header buttons appear as `AXButton 接手操作` / `AXButton 窗口 1`. AXPress works, and the label updates after a click. |

CPU of the Viewer process, averaged over 8 one-second `ps` samples, against the
Swift app under the same `busy` load:

| | GPUI spike | Swift app |
| --- | --- | --- |
| Viewer open, no busy window | 2.1% | 1.4% |
| Viewer open, display at 45–60 fps | 2.7–4.5% | 1.6–2.0% |
| RSS | 72–80 MB | 55 MB |

The Swift Viewer hands frames to a CALayer and the window server composites
them. GPUI redraws its whole window through Metal on every frame, which
costs about 1–2.5 points more.

## Findings

- **Use GPUI from Zed's main branch, not crates.io.** 0.2.2 has no
  accessibility tree, so the Viewer would be invisible to VoiceOver and to
  computer-use agents. It also sets the Regular activation policy at launch,
  which gives the app a Dock icon. Main has AccessKit and an activation
  policy setting. Main needs Rust 1.98 (`rust-toolchain.toml`).
- **Set the activation policy on the platform before `run`.**
  `App::set_activation_policy` runs after launch. That is too late to avoid the
  Dock icon. `WindowingRequest::Headless` draws nothing.
- **Cached views lose their accessibility nodes.** `Entity::cached` reuses the
  previous frame's paint but not its AccessKit nodes, so a cached header's
  buttons vanished from the AX tree. Upstream has no issue or fix for this yet. The
  spike keeps every view uncached.
- **GPUI windows on a virtual display run at about 24 fps.** GPUI paces each
  window with a CVDisplayLink for its display, and the link on a
  CGVirtualDisplay ticks at about 24 Hz. This does not affect Sidescreen's own
  windows, which live on the user's screens. A GPUI app moved onto an Agent
  Display would be slow, though, so the busy load is AppKit.
- **Capture rate is capped by the window server, not GPUI.** With drawing
  off, ScreenCaptureKit delivered 36–54 fps from the virtual display while
  the busy window drew at 60. Load from other apps on the machine moved the cap.
- **In-process `NSApp.isActive` reads true for a non-activating panel even
  though the app is not active.** Check activation from another process
  (`NSRunningApplication(processIdentifier:)`) instead.
- **Building needs no Metal Toolchain** with `runtime_shaders`. The
  screencapturekit crate's Swift bridge needs `/usr/lib/swift` on the rpath
  (`build.rs`).
