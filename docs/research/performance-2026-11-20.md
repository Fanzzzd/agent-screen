# Performance investigation, November 2026

This report measures where Agent Screen spends main-thread time, tool latency, and idle wakeups on macOS, and records measurements for four code pull requests: [#244](https://github.com/Fanzzzd/agent-screen/pull/244) (merged), [#246](https://github.com/Fanzzzd/agent-screen/pull/246) (merged), [#248](https://github.com/Fanzzzd/agent-screen/pull/248), and [#250](https://github.com/Fanzzzd/agent-screen/pull/250).

Four findings drove the changes:

1. In local display logs (`7,920` parsed `main thread busy` records), `97.9%` of stalls happened inside macOS frames (`CGSSnarfAndDispatchDatagrams`, display reconfiguration, Metal `waitUntilScheduled`, Spaces, CoreAnimation fences, ColorSync). The remaining `2.1%` (`164` stalls) pointed to a small set of our own frames. Four runtime frames that can safely move off the main thread or be cached (`recover_observer`, `screen_name`, `on_screen_windows`/`list_raw`, `hold_modes`, totaling `62` stalls and `28.0 s`) are addressed in PR #248. `label_display` (`14` stalls, `20.5 s`) was kept on the main thread because Apple requires `NSWorkspace.setDesktopImageURL` on the main thread. `Shell::icon_rect` (`9` stalls, `4.1 s`) reads AppKit's `NSStatusItem` button frame directly and is untouched.
2. Each agent click previously ran `cg::remote_panels()` (an `OptionAll` window scan across the whole Mac) three times: in `guard_agent_point`, `window_at`, and `mouse_down`. In back-to-back same-load A/B measurements (`loadavg 30.8` to `32.0`), combining the pre-click checks and reusing the 500 ms `PANEL_CACHE` for `mouse_down` reduced the **uncached per-click hit-test sequence** (clicks > 500 ms apart) from `34.14 / 34.96 / 45.45 ms` p50 to `12.67 / 12.18 / 15.98 ms` p50 (`-63%` to `-65%`, one `OptionAll` scan instead of three). A single uncached `window_at` call is unchanged (`10.73` to `14.96 ms` before vs `10.78` to `13.75 ms` cold after); the sub-2 ms figure (`1.04` to `2.08 ms`) applies only to warm cache hits within 500 ms on an unchanged window layout. `foreign_window_at` skips `remote_panels()` unconditionally and dropped from `13.05` to `13.73 ms` to `1.80` to `2.21 ms` uncached (`-83%` to `-86%`).
3. `windowState` ran the AX tree walk and window capture sequentially, and `act_on_element` with `result: "none"` still captured and discarded a full window image. Within the same run (`n=72` Calculator calls with picture, no cross-run load confounder), concurrent execution via `futures::future::join` completed in `30.0 ms` p50 vs `49.0 ms` p50 for the same calls' sequential sum (`walk + capture`), saving `20.0 ms` p50 (`-39%`, mean `22.2 ms`). For `result: "none"` (`n=10`), skipping the image avoided `10` captures completely (`20.5 ms` p50 walk-only vs `~29 ms` picture capture saved). Parsing `CFBundleURLTypes` in process with `CFPropertyList` instead of spawning `/usr/bin/plutil` cut `read_handles_web` from `3.33` to `3.79 ms` to `0.194` to `0.213 ms` p50 (`-94%`) under matched load.
4. With zero Agent Displays open, every connected in-process MCP host spawned a 2-second reconnect polling loop (`Control::greet`), while `cursor_guard` (500 ms) and `watch_alerts` (1 s) polled continuously. Removing those loops reduced idle context switches per second with 10 Pipes in direction-only sampling (`295.5` down to `115.5` to `149.5` csw/s; see load caveat below).

## Method

Measurements ran on one macOS Apple Silicon host on 2026-11-20 using four methods:

1. **Display log aggregation**: Parsed every `main thread busy N ms ... in X of Y samples: <stack> | ours: <frame>` line in `~/Library/Logs/Agent Screen/displays.log`, `displays.old`, `displays-e2e.log`, and `displays-e2e.old`. Records whose `ours:` field was empty or `AgentScreen::main` (the run-loop base frame) were classified under macOS by top stack frame.
2. **Interleaved same-load zero-display A/B microbenchmarks**: Three consecutive runs of live macOS primitives (`cg`, `windows`, `input`, `app`, `base64`) built with `CARGO_INCREMENTAL=0` and run serially under `lockf /tmp/agent-screen-build.lock` with zero virtual displays. Each Before and After operation ran interleaved inside the same loop at matched host `loadavg` (`31.99` -> `30.79`). Cold hit-test measurements explicitly cleared `PANEL_CACHE` before each iteration to reflect real clicks > 500 ms apart; warm cache hits were timed separately.
3. **Zero-display idle sampling (direction only)**: Built `target/Agent Screen Dev.app` and ran it as a separate `AGENTSCREEN_INSTANCE=e2e` instance under `lockf /tmp/agent-screen-e2e.lock`. Sampled CPU and context switches per second (`csw/s`) with `top -l 11 -s 1` (10 one-second deltas) with 0 Pipes and with 10 idle `AgentScreen mcp` Pipes connected, plus 50-call control socket RPC timings (`diagnostics`, `debugIcon`, `debugStreams`).
4. **End-to-end virtual-display MCP benchmark (confounded across runs; within-run breakdown valid)**: Ran `AGENTSCREEN_INSTANCE=e2e` under `lockf /tmp/agent-screen-e2e.lock`, created one Agent Display, launched Calculator, and timed MCP tool calls over a live `AgentScreen mcp` Pipe plus app `stderr` internal breakdowns (`windowState` walk vs picture, `window picture` list vs capture).

### Measurement caveats

- **Cross-run host load confounder (not directly comparable)**: Both the single-display Baseline run and the idle Baseline run executed during heavy host load (`loadavg 53.37` to `85.69`), while the After runs executed during lighter host load (`loadavg 8.00` to `15`). Cross-run before/after percentages from those two tables are **confounded and not directly comparable**. All verified speedup claims come from (a) the interleaved same-load zero-display A/B benchmark (`loadavg 30.8` to `32.0`), and (b) the within-run `windowState` paired comparison (`walk_ms + pic_capture_ms` vs `actual_total_ms` on the same 72 calls in the same run).
- **Direct control-socket script buffering**: The direct control-socket helper in the virtual-display script used unbuffered `makefile("rwb", buffering=0)` (1-byte reads), inflating client wall times for large-payload direct calls (`ctrl_screenshot_*`, `ctrl_window_state`). Buffered MCP Pipe calls (`mcp_*`), small-payload direct calls, and app `stderr` timestamps were unaffected.

## Main-thread stall logs

Aggregating all four logs under `~/Library/Logs/Agent Screen/`:

| Log file | Parsed stalls | macOS stalls | Our stalls | All p50 | All p95 | All max | macOS total | Our total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `displays.log` | 1,157 | 1,148 (99.2%) | 9 (0.8%) | 374 ms | 1,278 ms | 5,411 ms | 598.6 s | 4.6 s |
| `displays.old` | 3,821 | 3,763 (98.5%) | 58 (1.5%) | 295 ms | 1,044 ms | 60,022 ms | 1,620.6 s | 25.3 s |
| `displays-e2e.log` | 1,012 | 982 (97.0%) | 30 (3.0%) | 369 ms | 1,831 ms | 16,241 ms | 596.6 s | 27.4 s |
| `displays-e2e.old` | 1,930 | 1,863 (96.5%) | 67 (3.5%) | 304 ms | 1,800 ms | 24,535 ms | 966.1 s | 84.5 s |
| **Total** | **7,920** | **7,756 (97.9%)** | **164 (2.1%)** | | | | **3,781.9 s** | **141.8 s** |

Top macOS categories across all four logs:

| macOS category | Count | Primary stacks |
| --- | ---: | --- |
| WindowServer event pull | 3,066 | `CGSSnarfAndDispatchDatagrams < SLSGetNextEventRecordInternal` |
| WindowServer display reconfig | 2,085 | `SLSDisplayChangedSeed`, `SLSCopyDisplayColorSpace`, `_NSCGSDisplayConfigurationUpdate` |
| CoreAnimation fence | 715 | `_CASCreateFencePort` |
| Spaces query | 686 | `SLSCopyManagedDisplaySpaces` |
| Metal `waitUntilScheduled` | 482 | `-[_MTLCommandBuffer waitUntilScheduled]` from `MetalRenderer::draw` (`presents_with_transaction`) |
| ColorSync file lock | 134 | `displays.old` only |
| `os_log` XPC | 75 | `displays-e2e.*` only |
| Dock / DesktopPicture | 48 | `NSWorkspace` desktop picture XPC |
| StatusItem layout | 20 | AppKit status item layout |

Top frames belonging to Agent Screen across all four logs:

| Frame | Stalls | p50 range across logs | Max | Total time | Status |
| --- | ---: | ---: | ---: | ---: | --- |
| `mac::real_input::recover_observer` | 41 | 209 to 515 ms | 2,247 ms | 19.7 s | Fixed in PR #248 |
| `ui::shell::start` | 26 | 383 to 618 ms | 1,804 ms | 12.7 s | One-time app launch |
| `app_menu` | 17 | 227 to 403 ms | 7,758 ms | 13.9 s | One-time app launch |
| `mac::app::label_display` | 14 | 265 to 861 ms | 11,060 ms | 20.5 s | Kept on main thread (Apple requires main thread for `setDesktopImageURL`) |
| `mac::app::screen_name` | 12 | 166 to 364 ms | 1,446 ms | 4.2 s | Fixed in PR #248 (`display_change_stamp` cache) |
| `ui::shell::Shell::icon_rect` | 9 | 179 to 545 ms | 645 ms | 4.1 s | Untouched (`NSStatusItem` button frame on main thread) |
| `mac::cg::on_screen_windows` / `list_raw` | 8 | 194 to 1,104 ms | 1,104 ms | 3.9 s | Fixed in PR #248 (`Shell::sync`, `open_viewer_from`) |
| `<Logger as log::Log>::log` | 5 | 507 ms | 1,611 ms | 3.5 s | Blocking `stderr` write in e2e only |
| `mac::cg::is_closet` | 3 | 14,296 ms | 24,535 ms | 42.0 s | Old log (`displays-e2e.old` only) |
| `mac::native::show_borders` | 3 | 169 to 228 ms | 228 ms | 0.6 s | Rare AppKit window update |
| `mac::native::order_out` | 3 | 228 to 763 ms | 763 ms | 1.6 s | Rare AppKit window order |
| `mac::native::follow_pointer` | 2 | 218 ms | 218 ms | 0.4 s | Hidden windows skipped in PR #250 |
| `core::Core::hold_modes` | 1 | 182 ms | 182 ms | 0.2 s | Fixed in PR #248 |

## Interleaved same-load A/B microbenchmarks (zero displays)

All pairs ran interleaved inside the same binary under `lockf /tmp/agent-screen-build.lock` (`CARGO_INCREMENTAL=0`, zero virtual displays) at matched host `loadavg` (`31.99` start to `30.79` end across 3 runs).

### PR #244 (merged): hit tests and window lists

| Operation | n | Before p50 (3 runs) | After p50 (3 runs) | Before p95 range | After p95 range | Measured result |
| --- | ---: | --- | --- | --- | --- | --- |
| **Per-click hit-test sequence, cold (>500 ms apart)** | 20 | 34.14 / 34.96 / 45.45 ms | 12.67 / 12.18 / 15.98 ms | 51.6 to 221.8 ms | 18.0 to 37.4 ms | **-63% to -65% (2.7x to 2.9x)**; 1 `OptionAll` scan instead of 3 |
| Single `window_at`, **cold (cache miss)** | 20 | 10.73 / 10.46 / 14.96 ms | 11.24 / 10.78 / 13.75 ms | 13.9 to 38.0 ms | 17.2 to 28.6 ms | No speedup on cold miss (`remote_panels` dominates) |
| Single `window_at`, **warm (<500 ms hit, same layout)** | 20 | 10.73 / 10.46 / 14.96 ms | 1.15 / 1.04 / 2.08 ms | 13.9 to 38.0 ms | 5.3 to 9.1 ms | Warm cache hit only (applies to `mouse_down` inside a click or <500 ms bursts) |
| `foreign_window_at`, **uncached** | 20 | 13.26 / 13.05 / 13.73 ms | 1.80 / 2.21 / 1.92 ms | 15.5 to 32.7 ms | 8.7 to 15.5 ms | **-83% to -86% (6.0x to 7.4x)**; skips `OptionAll` unconditionally |
| `windows::all` (uncached) | 40 | 0.82 / 0.92 / 0.83 ms | 0.77 / 0.90 / 0.88 ms | 1.1 to 6.1 ms | 0.9 to 7.9 ms | Within run-to-run noise; avoids string allocs for filtered windows |
| `windows::menus_on` (uncached) | 40 | 0.83 / 0.92 / 0.84 ms | 0.80 / 0.93 / 0.82 ms | 1.0 to 4.2 ms | 1.0 to 4.3 ms | Within run-to-run noise; 0 string allocs |

Why the per-click cold cost drops from ~35 ms to ~13 ms even when clicks are seconds apart: previously one click called `hits()` three times (`guard_agent_point`, `window_at`, `mouse_down`), running three `OptionAll` scans. After PR #244, `click_check_at` does one cold `OptionAll` scan (combining foreign-window guard and top-window lookup) and populates the 500 ms cache, so `mouse_down` ~1 ms later hits the cache and runs only `hits_on_screen()`.

### PR #246 (merged): app metadata, process scan, and base64 off-main

| Operation | n | Before p50 (3 runs) | After p50 (3 runs) | Before p95 range | After p95 range | Measured result |
| --- | ---: | --- | --- | --- | --- | --- |
| `read_handles_web` (`plutil` vs `CFPropertyList`, Safari) | 15 | 3.33 / 3.59 / 3.79 ms | 0.194 / 0.203 / 0.213 ms | 3.52 to 4.07 ms | 0.23 to 0.26 ms | **-94% (17x to 18x)**, 0 subprocess spawns |
| `app::processes_of(Calculator)` | 20 | 2.36 / 2.22 / 2.56 ms (earlier 3 runs) | 2.28 / 2.06 / 2.24 ms | 3.31 to 4.15 ms | 2.56 to 2.88 ms | Wall time within noise (kernel `proc_pidpath` dominates); removes ~1,470 4 KiB heap allocs/call |
| `base64` encode moved off main (43 KB JPEG / 95 KB PNG) | 40 | 0.012 ms / 0.025 ms on main | 0.012 ms / 0.025 ms on bg | 0.012 to 0.015 ms | 0.024 to 0.027 ms | Negligible wall time (<0.03 ms); main-thread hygiene only |

## Within-run `windowState` parallelism proof (PR #246, no cross-run confounder)

Because the app logs both `window picture ... captured by C ms` and `windowState ... elements in W ms, picture by T ms` for every call, we can compare each call's sequential sum (`W + C`) against its actual concurrent total (`T`) inside the **same run** at the exact same host load:

| Run and call group | n | Walk `W` p50 / mean | Picture `C` p50 / mean | Same-call sequential `W + C` p50 / mean | Actual total `T` p50 / mean | Within-run overlap saved `W + C - T` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline Run 2 (sequential, `loadavg 53-86`) | 82 | 24.5 / 26.2 ms | 34.5 / 44.0 ms | 60.5 / 70.3 ms | 62.0 / 72.5 ms | -1.0 ms p50 (0 overlap, +1 to 2 ms glue) |
| After Run 3 with picture (concurrent, `loadavg 8-11`) | 72 | 20.5 / 23.7 ms | 29.0 / 38.1 ms | **49.0 / 61.8 ms** | **30.0 / 39.6 ms** | **20.0 ms p50 (-39%), mean 22.2 ms (range 8 to 68 ms)** |
| After Run 3 `result="none"` (no picture, `loadavg 8-11`) | 10 | 20.5 ms | skipped (0 captures) | ~49.5 ms (if captured) | **20.5 ms** | Skipped 10 of 10 captures (~29 ms p50 saved/call) |

## Idle cost and control socket overhead (zero displays, direction only)

Measured on `target/Agent Screen Dev.app` (`AGENTSCREEN_INSTANCE=e2e`, `top -l 11 -s 1`, 10 one-second deltas).

**Confounder warning (direction only)**: Baseline ran during heavy host load (`loadavg` ~53 to 85), whereas After runs executed during lighter host load (`loadavg` ~8 to 15), and host pointer moves raise instantaneous `csw/s`. Treat the table below as direction only, not a matched-load ratio:

| Scenario | Baseline csw/s (median / mean / floor) | PR #250 Run 1 csw/s | PR #250 Run 2 csw/s | Combined build csw/s |
| --- | --- | --- | --- | --- |
| Idle, 0 Pipes | 91.5 / 144.0 / 69 | 154.0 / 221.3 / 59 (mouse active) | 67.0 / 88.5 / 36 | 159.5 / 224.6 / 40 (mouse active) |
| Idle, 10 Pipes | 295.5 / 287.3 / 125 | 143.0 / 148.9 / 82 | 115.5 / 146.0 / 70 | 149.5 / 170.7 / 80 |

In the quiet run (PR #250 Run 2), idle CPU median was `0.1%` with 0 Pipes and `0.35%` with 10 Pipes; resident memory was `64M` to `73M`.

Zero-display control socket RPC latency (n=50 per run):

| Method | Response size | p50 range across runs | p95 range |
| --- | ---: | --- | --- |
| `debugStreams` | 34 B | 0.071 to 0.088 ms | 0.129 to 0.141 ms |
| `debugIcon` | 52 B | 0.104 to 0.108 ms | 0.143 to 0.217 ms |
| `diagnostics` | 76 B | 0.300 to 0.377 ms | 5.45 to 11.78 ms |

## End-to-end virtual-display MCP benchmark (confounded across runs)

**Confounder warning (not directly comparable across runs)**: One Agent Display running Calculator (`AGENTSCREEN_INSTANCE=e2e`). Baseline ran at host `loadavg 53.37` to `85.69`; Combined ran at host `loadavg 11.28` to `8.00`. Because host load differed by 5x to 8x, these raw cross-run times are recorded for completeness and payload sizing only, **not** as proof of speedup. Use the same-load A/B table and within-run `windowState` table above for verified speedup numbers.

| Operation | n | Payload (Before / After) | Baseline p50 / p95 (`loadavg 53-86`) | After p50 / p95 (`loadavg 8-11`) | Notes |
| --- | ---: | --- | --- | --- | --- |
| `mcp_get_window_state` | 20 | 38.3 KB / 40.8 KB (tree ~1.8 KB, JPEG b64 ~36 to 39 KB) | 56.34 / 122.64 ms | 35.33 / 63.69 ms | Confounded; within-run overlap is -39% |
| `mcp_get_window_state` (`max_depth=2`) | 10 | 38.3 KB / 40.8 KB | 49.46 / 54.06 ms | 38.38 / 59.32 ms | Confounded |
| `ctrl_click` (pure click) | 20 | JSON ack | 165.38 / 256.18 ms | 62.00 / 743.70 ms | Confounded; same-load hit-test is -63% to -65% |
| `mcp_list_windows` | 20 | 488 B / 488 B | 21.26 / 109.28 ms | 8.38 / 45.91 ms | Confounded |
| `ctrl_list_windows` | 20 | 200 B / 200 B | 8.67 / 34.09 ms | 7.27 / 17.00 ms | Confounded |
| `mcp_screenshot_jpeg` | 20 | 55.5 KB / 58.1 KB | 97.75 / 160.45 ms | 54.91 / 84.63 ms | Confounded |
| `mcp_computer` (`left_click`) | 10 | 55.5 KB / 58.1 KB | 576.97 / 675.61 ms | 565.02 / 620.24 ms | Dominated by 500 ms `wait_until_still` |
| `mcp_act_on_element` (`result="state"`) | 10 | 41.1 KB / 41.1 KB | 593.79 / 709.35 ms | 587.18 / 838.70 ms | Dominated by 500 ms `wait_until_still` |
| `mcp_act_on_element` (`result="changes"`) | 10 | 39.7 KB / 39.7 KB | 612.41 / 754.67 ms | 551.56 / 590.48 ms | Confounded; includes 500 ms settle |
| `mcp_act_on_element` (`result="none"`) | 10 | 81 B / 81 B | 627.23 / 715.51 ms | 542.99 / 563.50 ms | Confounded; skipped 10 of 10 window captures |
| `mcp_wait_for_change` (2 s timeout) | 3 | JSON timeout | 2,039.00 / 2,054.79 ms | 2,028.63 / 2,029.14 ms | Timeout bound |
| `create_display` (cold) | 1 | JSON | 9,625.43 ms | 3,624.22 ms | WindowServer reconfig + host load |
| `launch_app` (cold Calculator) | 1 | JSON | 4,666.86 ms | 3,133.92 ms | Host load |
| `end_display` | 1 | JSON | 8.47 ms | 1.76 ms | Host load |

Payload size breakdown on `get_window_state`: the AX tree text for Calculator is `1.8 KB`, while the base64-encoded JPEG image is `36.2` to `38.6 KB` (`94%` of total response bytes). Full-display JPEG screenshots are `55.5` to `58.1 KB` base64 (`120` to `127 KB` for PNG).

## Pull requests delivered

1. **[#244](https://github.com/Fanzzzd/agent-screen/pull/244) (`perf/window-list-hit-test`, merged)**: Filter window lists early (`cg::on_screen_where`, `cg::on_screen_ids_where`, `cg::on_screen_hit_candidates`), cache `cg::remote_panels()` in `input::hits()` by layout with 500 ms TTL, skip `remote_panels()` in `foreign_window_at()`, combine pre-click checks into `click_check_at`, reuse `within_display` in `mouse_down`.
2. **[#246](https://github.com/Fanzzzd/agent-screen/pull/246) (`perf/window-state-and-app-scan`, merged)**: Overlap `ax::snapshot` and `screenshot::window` in `windowState` via `futures::future::join`, move base64 encoding off the main thread, add `"image": false` for `act_on_element` `result: "none"` and pre-action `root` validation, reuse one `content.windows()` bridge call in `screenshot`, cache `CFArray<CFString>` in `ax::multiple_values`, batch `ax::list_menu` attributes, reuse stack buffer in `app::processes_of`, cache `main_executable`, parse `CFBundleURLTypes` in process with `CFPropertyList` instead of spawning `/usr/bin/plutil`.
3. **[#248](https://github.com/Fanzzzd/agent-screen/pull/248) (`perf/main-thread-stalls`, open)**: Move Input Queue `HIDEventTap` onto its own `CFRunLoop` thread (`agent-screen-input-watch`) and run `recover_observer()` off the main thread, cache `native::screens()` and `app::screen_name()` per `display_change_stamp()`, offload `windows::on_display` in `Shell::sync` and `Shell::open_viewer_from` (calling `refresh_counts` after `self.viewer_open = true`), and offload `cg::display_bounds` checks in `Core::hold_modes`.
4. **[#250](https://github.com/Fanzzzd/agent-screen/pull/250) (`perf/idle-wakeups`, open)**: Skip the 2-second reconnect polling loop in `Control::greet` for `AppLink::InProcess` hosts, make `cursor_guard` and `watch_alerts` event-driven via `cx.observe(&core, ...)` over a bounded `channel(1)` (`core::wake_channel`, drained each active loop), cache `display_bounds` in `cursor_guard`, skip hidden windows in `native::follow_pointer` and refresh `follow_pointer` after `orderFrontRegardless()`, check `parent == Some(1)` before reading peer paths and arguments in `sweep_orphans`, slow idle permission and shell timers, and reuse a single `Condvar` + sequence counter in `hang::watch`.

## Remaining bottlenecks and rejected changes

- **Moving `mac::app::label_display` off the main thread (reverted for safety)**: `label_display` accounted for `14` stalls (`20.5 s` total) across the logs. An initial draft moved `NSWorkspace.desktopImageURLForScreen` and `setDesktopImageURL_forScreen_options_error` onto a background thread with `unsafe impl Send`, but Apple requires `setDesktopImageURL` to be called on the main thread. That change was reverted from PR #248.
- **`Shell::icon_rect` (`9` stalls, `4.1 s`)**: Queries AppKit's `NSStatusItem` button frame directly on the main thread. Neither PR #248 nor PR #250 modifies it.
- **macOS WindowServer display reconfiguration and Metal `waitUntilScheduled`**: Together these remain over `70%` of all logged main-thread stalls (`5,633` of `7,920` stalls including `CGSSnarfAndDispatchDatagrams`). `waitUntilScheduled` comes from GPUI's `MetalRenderer::draw` with `presents_with_transaction` enabled for border/panel sync. Changing GPUI's presentation mode risks visual tearing or misalignment and was not touched.
- **500 ms post-action settle (`wait_until_still`)**: `computer` (`left_click`) and `act_on_element` wait up to 500 ms for a window's pixels to settle (behavior required by issue #16). This accounts for ~85% to 92% of total `act_on_element` and `computer` wall time. Reducing it would change user-visible timing semantics and was left untouched.
- **`Shell::show_window_menu`**: Still queries `windows::on_display` synchronously when the user explicitly clicks the menu bar display submenu. Because it runs only on direct user click and must populate an `NSMenu` before returning, changing it would not help background or idle latency.
- **`app::processes_of` kernel floor (`~2.0 ms`)**: After eliminating per-PID heap allocations, remaining time is in `proc_pidpath` syscalls and wall-time delta is within run-to-run noise. `launch_app` calls it on a 20 ms poll only while waiting for a newly launched app's window to appear.
- **`hang::watch` 10 Hz probe**: The main-thread watchdog still wakes every 100 ms (`10` csw/s) to detect stalls >= 150 ms. Removing the per-tick channel allocation eliminated heap churn; widening the probe interval would change stall detection resolution, so it was kept at 100 ms.
- **JSON serialization of base64 screenshots**: Base64 encoding takes `< 0.03 ms` for 43 to 95 KB payloads. Replacing JSON/base64 over the local Unix socket would break the NDJSON contract (ADR 0005) and saves less than run-to-run noise.
