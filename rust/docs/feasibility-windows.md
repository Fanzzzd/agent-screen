# Sidescreen on Windows: feasibility (2026-10-03)

Desk research only; no Windows machine was available. The research read the source of the
open-source virtual display drivers and of GPUI's Windows backend.

## Main finding

Windows can do virtual displays, capture, window management and the tray.
Background input is the blocker. Windows has no supported equivalent of
`SLEventPostToPid`. Keyboard input always goes to the single foreground window,
and modern apps ignore posted mouse messages.

## Capabilities

| Capability | Feasibility | Mechanism |
|---|---|---|
| Create and remove an agent display at runtime | Works, with a driver install | Indirect Display Driver (IddCx), with IOCTL add/remove |
| Install without test-signing | Partial | Attestation signing under our own EV cert |
| Keep the cursor out | Partial | Corner-only placement plus a `WH_MOUSE_LL` hook |
| Capture at 60 fps | Works | `Windows.Graphics.Capture` `CreateForMonitor` |
| No yellow border | Works on Windows 11 | `IsBorderRequired = false` |
| Mouse to a background window | Partial | `PostMessage`: Win32 yes, Chromium/Electron/WinUI unreliable |
| Keyboard to a background window | Blocked in general | Keyboard input goes to the foreground window only |
| Semantic actions | Works for many apps | UI Automation patterns (Invoke, Value, Scroll) |
| Separate session for the agent | Blocked for third parties | Windows Agent Workspace is a private preview |
| Move and enumerate windows | Works | `SetWindowPos`, `EnumWindows`, `MonitorFromWindow` |
| Accessibility tree | Works | UI Automation |
| Tray | Works | `tray-icon` crate |
| Panel that never takes focus | Needs a GPUI patch | `PopUp` lacks `WS_EX_NOACTIVATE`; `WM_MOUSEACTIVATE` returns `MA_ACTIVATE` |

## Drivers

- **SudoVDA** (MIT, used by Apollo). Its IOCTLs add and remove single displays
  (`IOCTL_ADD_VIRTUAL_DISPLAY` / `IOCTL_REMOVE_VIRTUAL_DISPLAY`, taking width, height, refresh and GUID).
  A watchdog removes displays when pings stop. This is the closest match to CGVirtualDisplay.
- **Virtual-Display-Driver (MttVDD)** (MIT). It only reloads its whole
  configuration, re-plugging every monitor, and its last release was Oct 2024.
- **Parsec VDD**. The driver binary belongs to Parsec, and we found no license to redistribute it.

Shipping without test mode means attestation signing under our own EV cert and
a Hardware Dev Center account, plus an admin install.

## Recommended architecture

- **Driver.** A SudoVDA fork, attestation-signed and installed once. Rust
  drives it through `DeviceIoControl`: ADD with a stable GUID per agent, a ping every
  second, REMOVE on teardown.
- **Topology.** Place the agent display corner-adjacent with `SetDisplayConfig`, plus a mouse hook.
- **Capture.** WGC on that monitor with the border off.
- **Input, tiered.**
  1. UI Automation patterns.
  2. CDP for Chromium and Electron apps the agent launches.
  3. `PostMessage` for classic Win32 apps.
  4. A visible "borrow foreground" `SendInput` mode for the rest, used only
     while the user is idle.
- **UI.** A GPUI panel with a small no-activate patch, plus `tray-icon`.

## Risks

1. **Background input.** Typing into Chromium, Electron and WinUI apps
   while the user works is not reliable. The Windows build will be weaker
   here than macOS.
2. **Driver signing and distribution.** An EV cert costs money every year,
   installs need admin rights, and company policies can block them.
3. **Driver maintenance.** We would own a fork of a hobby-maintained driver.
4. **Cursor containment.** Other apps' `SetCursorPos` calls bypass the hook.
5. **GPUI.** We would carry a patch for the no-activate panel.
6. **Agent Workspace.** Microsoft may ship this product itself.

## Spike plan (Windows 11 24H2, Secure Boot on)

1. **Background input matrix (days 1–3).** Use Notepad, a Win32 app,
   Explorer, Edge, VS Code, Settings and Slack. For each, try `PostMessage`,
   UIA and CDP while the user types in another window. This step is the go/no-go.
2. **Driver (days 3–5).** Test-sign SudoVDA and drive ADD/PING/REMOVE from
   Rust. Start the EV cert application in parallel, since it takes weeks.
3. **Capture (day 6).** WGC into a GPUI view, measuring fps.
4. **Cursor containment (day 7).**
5. **UI (day 8).** The GPUI no-activate patch, checked with `GetForegroundWindow` logs.

If UIA and CDP cover less than about 70% of agent tasks, decide on the
borrow-foreground model before investing in the driver.

## Sources

- https://learn.microsoft.com/en-us/windows-hardware/drivers/display/iddcx-versions
- https://github.com/SudoMaker/SudoVDA (`Common/Include/sudovda-ioctl.h`)
- https://github.com/VirtualDrivers/Virtual-Display-Driver
- https://github.com/nomi-san/parsec-vdd
- https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired
- https://devblogs.microsoft.com/oldnewthing/20050530-11/?p=35513
- https://support.microsoft.com/en-us/windows/ai/ai-features/experimental-agentic-features
- https://github.com/zed-industries/zed/blob/main/crates/gpui_windows/src/window.rs
