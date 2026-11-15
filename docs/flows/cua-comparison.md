# Agent Screen 与 trycua/cua 逐项对比及差距收敛方案

本文基于 `trycua/cua` 仓库源码静态分析（核心路径为 `libs/cua-driver/rust/crates/platform-macos`、`libs/cua-spacesd/crates/cua-spacesd-desktop`、`libs/lume` 与 `libs/spaces-app-swift`），逐项对比 Agent Screen 的十项操作与状态机，并给出按收益与可行性排序的改进建议。

## 一、顶层架构分水岭：虚拟显示器 vs 宿主同屏后台 + 重量级虚拟机

Agent Screen 与 `cua` 虽然在底层共享了 `SkyLight` 后台事件投递、`SLSEventAuthenticationMessage` 键盘鉴权与 `SLPSPostEventRecordTo` 无抬升聚焦，但顶层隔离模型走了两条截然不同的路线：

1. **Agent Screen 采用宿主进程内 `CGVirtualDisplay` 虚拟显示器**：代理应用运行在用户当前 macOS 登录会话内，直接复用用户已登录的浏览器 Profile、钥匙串与本地文件，同时拥有独立于用户主屏的无遮挡像素空间，因此 Chromium 不会因窗口被遮挡而停止合成渲染。代价是必须直面 `WindowServer`、ColorSync 与 `ScreenCaptureKit` 在多虚拟屏热配置下的底层缺陷：`unit twin`、`misroute`、`800x500` 模式漂移与 `#204` 物理屏重排偏移。
2. **`cua` 完全不使用 `CGVirtualDisplay`**：全仓检索确认 `cua-driver` 与 `cua-spacesd` 均无任何虚拟显示器代码。`cua` 拆分为两种形态：
   - **宿主同屏后台驱动** `libs/cua-driver/rust/crates/platform-macos`：目标窗口直接留在用户真实显示器上，靠 `(pid, CGWindowID)` 精确作用域在后台操作。它彻底规避了虚拟屏重排类缺陷，但承受三大硬伤：窗口被用户窗口遮挡或最小化后 Chromium 停止刷新像素、窗口切到其他 Space 后从 `AXWindows` 消失而沦为只读、后台弹窗或菜单展开时直接顶到用户真实桌面最上层（需 `key_focus_guard.rs` 事后强压 Z 序，仍有 1 至 2 帧闪烁）。
   - **独立虚拟机隔离** `libs/lume` + `libs/cua-spacesd`：通过 Apple `Virtualization.framework` 启动完整 macOS 客户机，经 `ScreenCaptureKit` + VideoToolbox H.264 + QUIC 推流到 `LiveStreamView.swift`。虽然彻底隔离了焦点与光标，但消耗数 GB 内存与磁盘，且客户机没有宿主的登录态，迫使 `libs/cua-spacesd/crates/cua-spacesd-teleport` 专门为 Chrome、Firefox、Electron、Claude Code 等应用编写 Cookie 与钥匙串搬运器。

## 二、十项操作逐项对比

| 操作域 | `cua` 实现方式与源码路径 | 双方共享机制 | `cua` 已解决而我方尚未解决的问题 | 我方已解决而 `cua` 未解决（或双方均受限）的问题 |
| --- | --- | --- | --- | --- |
| 01. `create_display` | 宿主模式无虚拟屏，`cua-spacesd-desktop/src/macos_display.rs:14-51` 仅枚举现有 `CGDisplay::active_displays()`；隔离模式由 `libs/lume/src/Virtualization/VMVirtualizationService.swift` 配置虚拟机显卡。 | CoreGraphics `CGDisplay` 几何与 HiDPI 缩放换算。 | 无需操作 `CGConfigureDisplayOrigin`，天然不存在 `unit twin`、`800x500` 与 ColorSync `40 ms` 预算问题。 | **我方独有优势**：零虚拟机开销下提供真实无遮挡独立屏幕，不污染用户桌面视野，Chromium 后台持续满帧渲染。 |
| 02. `end_display` 与会话生命周期 | `cua-driver` 工具调用无状态，按 `(pid, window_id)` 单次寻址；`cua-spacesd-session/src/media/leases.rs` 对媒体与交互流实施显式租约控制。 | Unix 域套接字守护进程通信与会话级清理。 | 无状态 `(pid, window_id)` 寻址不受单一僵尸客户端长期独占虚拟屏困扰。 | **我方优势**：提供 `Review` 收尾检视（ADR 0004）、`2000 ms` 闪断重连宽限与兄弟会话移交（ADR 0016）。**共同短板**：MCP stdio 子进程若被宿主悬空保持打开，仅靠管道 EOF 无法感知上游代理死亡（我方 `#189`）。 |
| 03. `launch_app` 与新窗归属 | `platform-macos/src/apps/nsworkspace.rs:116-240` 直接调用 `NSWorkspace` 的 `openApplicationAtURL:configuration:`，设置 `activates = false`、`addsToRecentItems = false`、仅注入显式增量环境变量，并附带合成 `aevt/oapp` AppleEvent；`tools/launch_app.rs:183-235` 包裹三阶段 `FocusStealPreventer` 抑制目标自激活；支持 `creates_new_application_instance` 为并发多代理拉起独立实例。 | 后台无感拉起应用并轮询等待首个窗口注册。 | 1. 避免 `/usr/bin/open` 子进程及最近使用列表污染，保留 `/System/Cryptexes/App`（如 Safari）真实 `NSURL`；2. 合成 `aevt/oapp` 解决冷启动不建窗问题；3. 拦截并回滚目标应用在 `applicationDidFinishLaunching` 中的 `activateIgnoringOtherApps:` 自激活。 | **我方优势**：ADR 0007 `(agent, user, only_here)` 新窗归属规则 + `Catch` 实时 `AXObserver` 自动将代理新窗迁入虚拟屏并与 AeroSpace `claims` 联动。**`cua` 缺陷**：`focus_arbiter.rs:184-195` 直接读 `CGEventSourceSecondsSinceLastEventType` 判断用户近期点击，未扣除 `_SLEventPostToPSN` 内部 `_CGSTickleActivityMonitor` 造成的伪重置，我方已在 `ime.rs:64-80` 用 `note_posted()` 解决。 |
| 04. `place_window` / 跨会话 / Handover | `tools/set_window_frame.rs` 与 `cua-spacesd-desktop/src/macos_geometry.rs:48-131` 通过 `_AXUIElementGetWindow` 定位精确窗口写 `AXPosition`/`AXSize`；`CuaSpacesTeleport/WindowDrop.swift:12-136` 支持用户拖拽宿主窗口触发跨虚拟机应用状态迁移。 | `_AXUIElementGetWindow` 精确窗口匹配与 AX 几何轮询确认。 | `WindowDrop.swift` 实现了物理拖拽宿主窗口进入目标空间的直觉交互，并区分边缘缩放与整窗平移（`appDragClassify`）。 | **我方优势**：同机零拷贝跨屏移动窗口，无需重启应用或搬迁 Cookie；具备完整 `Held` 记录对账（ADR 0018）、跨会话 `Window Request`（ADR 0017）及原坐标精确归还 `give_back`。 |
| 05. `computer` 动作与后台焦点 | `input/skylight.rs:255-282, 654-716, 1257-1265`、`input/mouse.rs`、`input/electron_drag.rs:108-199`、`key_focus_guard.rs:1-280`、`tools/invoke_menu.rs:58-260`。 | `SLEventPostToPid`、`SLSEventAuthenticationMessage`（含 `class_respondsToSelector` macOS 14 防崩检查）、`SLPSPostEventRecordTo` 248 字节 `0x0D` 记录、Chromium CDP `Input.setInterceptDrags`。 | 1. **`WindowKeyScope` 精确切键焦**：发送 `0x0D` 记录后追加成对 `make_key_window_record`（`0x01`/`0x02`，`record[0x3A]=0x10`，`0x20..0x30` 填 `0xFF`），确认 `AXFocusedWindow` 命中目标后才发键，结束后自动把进程内 key 还给原兄弟窗口；2. **防后台菜单偷键焦**：`key_focus_guard.rs` 用进程级 EventTap 拦截 type-21 `0x4000` 事件并调 `SLPSReleaseKeyFocusWithID` 释放偷取；3. **免预启动管道的 Electron 拖拽**：`electron_js.rs:54-76` 发 `SIGUSR1` 打开 Node Inspector，注入 `webContents.debugger` 或 `webContents.sendInputEvent('leftButtonDown')`；4. **后台 `invoke_menu`**：直接遍历 `AXMenuBar` 执行菜单命令并对禁用 `Save` 回退脚本保存；5. **窗口局部坐标**：调用 `CGEventSetWindowLocation` 写入窗口局部点。 | **我方优势**：1. `shared_app.rs` 针对"用户正在前台编辑同一应用另一窗口"的极端场景，用 `Picked` AX 直写 + `check_focus` 守护避免打断用户；2. `ime.rs` 支持完整 TIS 输入法临时借用（ADR 0008）。**双方均受限**：原生跨窗口 Cocoa 拖放（`NSDraggingSession`）与 `performWindowDragWithEvent:` 均被 AppKit 硬编码绑定真实硬件光标，后台合成无解。 |
| 06. `Input Queue` 与真实输入 | `cua-driver` 提供调用方显式指定的 `delivery_mode: "foreground"`（`input/mouse.rs:153-220`、`tools/desktop_pointer_guard.rs`、`security_prompt.rs:refuse_point`），但无持久化排队授权、无 `30 s` 共享 `Grant`、无 L 形副屏菜单滑行。 | 光标跳变前后保存并恢复原光标位置与原前台焦点。 | `security_prompt.rs:refuse_point` 在全局 HID 点击发出前校验目标坐标是否落在 macOS TCC / 系统安全授权弹窗上，拒绝代理替用户偷点"允许"按钮！ | **我方大幅领先**：拥有完整的 ADR 0013 `InputQueue`、`0x4153_5121` 硬件抢占毫秒级撤权、`real_click` 前后双重 AX+RGB 一致性核验、跨窗口 `NSPasteboardNameDrag.changeCount` 同步与 `menu_press` 菜单遍历。 |
| 07. `Viewer` 用户输入 | `spaces-app-swift/Sources/CuaSpacesStreaming/LiveStreamView.swift:68-200`、`KeyCapture.swift:18-69`、`cua-driver/platform-macos/src/input/interactive.rs:110-350`。 | 归一化坐标映射、越界拖拽保留屏内有效点（`lastInFramePoint`）、保留精确触控板滚动相位与残差（`scroll_residual`）。 | 1. `KeyCapture.swift` 在 Viewer 聚焦时拦截 `Cmd+快捷键` 发往目标环境，并用单按 `Ctrl+Option` 释放键盘归还宿主；2. `LiveStreamView.swift:190-200` 用同一几何变换驱动本地光标指示圆点与实际发送坐标，彻底消除视觉偏差。 | **我方大幅领先**：1. `private-press` 主动式 `HIDEventTap` 吞掉物理点击防止企微/Qt 全局监听器提前关弹窗（ADR 0014）；2. `Front::AfterRelease` 保护同进程 `<60%` 浮层；3. `Worker::grab` 接管原生标题栏 `AXPosition` 拖拽；4. `viewer_keys.rs` 完整支持宿主中文输入法内嵌组词与 `AXSecureTextField` 密码框回放保护（`#117`）。 |
| 08. `Takeover` 与求助请求 | `cua-spacesd` 通过 `SessionPolicy::{ViewOnly, BackgroundOnly, AllowActivation}`（`driver_input.rs:47-62`）静态划分会话权限，无结构化求助与交窗流程。 | 视图只读与交互控制模式切换。 | 无。 | **我方独有能力**：`request_help`（`Continue` / `Reply` / `Give a window` 一步移交 + `unheard` 闪断重放，ADR 0015）与单向排他 `Takeover`（ADR 0004）。 |
| 09. `Peek` 与捕获流管线 | `cua-spacesd-desktop/src/macos_capture.rs:151-280`（`SCStream` 支持单窗或整屏 + VideoToolbox H.264 硬编）、`platform-macos/src/input/skylight.rs:867-927`（`SLSHWCaptureWindowList` 抓跨 Space 窗口）。 | `ScreenCaptureKit` 异步帧回调、帧状态过滤与尺寸一致性校验。 | `SLSHWCaptureWindowList`（选项 `1<<11 | 1<<8`）可在零切换、零激活的前提下直接抓取位于其他 Space 上的窗口真实像素（`ScreenCaptureKit` 整屏流对此无能为力）。 | **我方独有优势**：单屏多订阅者动态合流（`1/4/12/60 fps`）、GPUI `YCbCr_420f` 本地零拷贝直出（免 H.264 编解码延迟与色度损失）、`SCStreamFrameInfoScreenRect` `misroute` 自检与指数退避自愈（`#190`）。 |
| 10. 窗口查询与变化等待 | `ax/enablement.rs`、`ax/window_scope.rs`、`tools/get_window_state.rs`、`window_change_detector.rs:55-240`。 | `_AXUIElementGetWindow` 精确绑定、有界 AX 树遍历、快照增量 diff。 | 1. `ax/enablement.rs` 对 Electron/Chromium 自动写入 `AXManualAccessibility`（回退 `AXEnhancedUserInterface`）唤醒空 AX 树；2. `window_change_detector.rs` 在每个动作前后自动对比新开窗口（含 `post_action_wait: "none"` 的 `earlier_new_windows` 延迟尾随上报）并附在工具返回尾部；3. `get_window_state` 默认同包返回 AX 树与窗口截图，并对空树标记 `degraded: true`。 | **我方优势**：`wait_for_change` 原生融合 `160x100` 像素帧差网格计数与 `Catch` `AXObserver` 推流通知，无需轮询全屏截图即可低开销阻塞等待视觉变化。 |

## 三、值得收敛的差距优先级清单与落地方案

以下按对现有已知缺陷的解决价值与改动收敛度从高到低排序：

### 优先级 1：补齐成对 `make_key_window_record` 与进程内 `WindowKeyScope`（直击 `CustomDrag` 首按吞没与 `#110` 同进程后台多窗）

- **现状差距**：我方 `SSFocusWithoutRaise`（`apps/macos/native/SkyLight.m:123-141`）仅发送 1 条 `0x0D` 聚焦记录（`record[0x8A]=0x01`）。实测与 `cua` 源码（`input/skylight.rs:243-282, 1257-1265`）共同证实：单独一条 `0x0D` 记录往往不足以令 AppKit 将目标 `NSWindow` 置为内部真 `isKeyWindow`，导致未重写 `-acceptsFirstMouse:` 的自绘标题栏视图在首次按下时被 AppKit 非活动窗口门控吞掉（`CustomDrag` 首按吞没）；同时当后台同一 PID 打开多个窗口（例如两个兄弟会话各持有一个文档窗口）时，发完按键后未将 key 状态归还给原窗口。
- **拟议方案**：
  1. 在 `apps/macos/native/SkyLight.m` 新增 `SSPostWindowKeyRecords(pid, prevWindowID, targetWindowID)`：在前台进程非目标进程的前提下，依次经 `SLPSPostEventRecordTo` 发送旧窗失焦记录（`0x0D`, `0x8A=0x02`）、目标窗聚焦记录（`0x0D`, `0x8A=0x01`）以及成对 `make_key_window_record`（`record[0x04]=0xF8`, `record[0x08]=0x01` 与 `0x02`, `record[0x3A]=0x10`, `record[0x20..0x30]=0xFF`, `record[0x3C..0x40]=targetWindowID`），并检查返回值是否为 `1002`（无效连接）。
  2. 在 `Viewer` 点击自绘标题栏窗口（`viewer_input.rs`）及代理后台多窗口键入（`input.rs`）前先用该序列确立精确窗口 key 态；对非共享前台的多窗后台 PID 引入 RAII 守卫在动作后将进程内 key 恢复给原窗口。

### 优先级 2：拦截后台弹出菜单跨屏抢夺用户键盘焦点（`SLPSStealKeyFocusReturningID` 防护）

- **现状差距**：即使目标窗口位于独立的虚拟显示器上，当代理点击原生 `NSPopUpButton`、HTML `<select>` 或右键上下文菜单时，AppKit 菜单跟踪循环会调用 `SLPSStealKeyFocusReturningID` 在整个 WindowServer 会话内强抢键盘焦点，导致用户在主屏编辑器里的打字突然中断甚至误触代理屏菜单项。
- **拟议方案**：移植 `cua` `key_focus_guard.rs:14-43` 的轻量级防护（但省去其同屏 Z 序重排逻辑，因为我方窗口已在虚拟屏上不会遮挡用户主屏）：在代理向后台 PID 执行点击或 `AXPress` 期间及随后 `3 s` 尾随窗口内，对用户当前前台 PID 挂载进程级 EventTap（`CGEventTapCreateForPid`），一旦捕获 type-21、subtype `0x4000` 且字段 `73` 为后台目标 PID 的焦点失窃通知，立即通过 `SLPSStealKeyFocusReturningID` 探知最新单调递增 ID 并调用 `SLPSReleaseKeyFocusWithID` 释放目标菜单偷走的键焦，同时丢弃该失焦通知，使用户打字零中断且虚拟屏菜单保持展开可点。

### 优先级 3：真实输入安全弹窗坐标禁区（防止 `InputQueue` 误点系统 TCC 授权框）

- **现状差距**：我方 `InputQueue` 虽然在 `come_forward` 前后校验了目标窗口的 AX `Identity` 与局部 RGB 采样，但若系统 TCC / 钥匙串授权弹窗（归属 `UserNotificationCenter`、`SecurityAgent`、`CoreServicesUIAgent`）恰好浮在目标坐标上方或代理被提示注入诱导点击系统弹窗，缺乏显式的系统安全进程坐标黑名单。
- **拟议方案**：参考 `cua` `security_prompt.rs:refuse_point`，在 `apps/macos/src/mac/real_input.rs` 发送任何真实 HID 点击前，扫描 `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly)` 中位于目标点上方的一切窗口，一旦命中 `SecurityAgent`、`UserNotificationCenter`、`CoreServicesUIAgent`、`universalAccessAuthWarn` 等系统授权宿主进程，立即无条件拒绝本次真实输入并告警。

### 优先级 4：已运行 Electron 应用免重启后台拖拽与后台原生菜单执行（减少 `InputQueue` 借用频率）

- **现状差距**：当前网页/Electron 拖拽仅对由 Agent Screen 冷启动并挂载了 `--remote-debugging-pipe` 的 Chromium 生效（ADR 0010）；用户已提前打开的 Electron 应用（VS Code、Linear、Notion）内拖拽以及所有应用菜单栏点击均被迫进入 `InputQueue` 借用真实鼠标（且 `menu_press` 在用户关闭"显示器具有单独的空间"时彻底不可用，`#39`）。
- **拟议方案**：
  1. **原生菜单先走后台 AX**：参考 `cua` `tools/invoke_menu.rs:58-260`，新增或扩展后台菜单路径，先验证目标窗口为进程内焦点窗口后直接遍历 `AXMenuBar` 执行 `AXPress`（含 `"..."` 到 `"…"` 归一化与失败时 `AXCancel` 收起），仅对必须展开才动态填充或置灰的菜单项再回退 `InputQueue` 的 `menu_press`。
  2. **Electron 按需受控调试桥（可选开关）**：对未禁用 Node Inspector Fuse 的目标 Electron 进程，研究借鉴 `electron_drag.rs:108-199` 的 `webContents.sendInputEvent`（携带 `leftButtonDown` 修饰符）路径，在不借用真实鼠标的情况下完成窗口内拖拽。

### 优先级 5：改用原生 `NSWorkspaceOpenConfiguration` 替代 `open -g -j` 并补充启动期自激活回滚

- **现状差距**：`apps/macos/src/mac/app.rs` 依赖外部 `/usr/bin/open -g -j` 子进程，对冷启动不建窗应用与 Cryptex 路径兼容性不如原生 API，且当目标应用在启动回调里强行调用 `activateIgnoringOtherApps:YES` 时未能第一时间把前台还给用户。
- **拟议方案**：使用 `objc2-app-kit` 直接调用 `NSWorkspace.openApplicationAtURL:configuration:completionHandler:`（设 `activates = false`、`addsToRecentItems = false` 及 `aevt/oapp` AppleEvent，仅传递显式增量环境变量避免泄漏宿主密钥），并在启动窗口期挂载基于 `_SLPSGetFrontProcess` 与我方 `ime::user_idle()`（已免疫 `_CGSTickleActivityMonitor` 污染）的自激活回滚守卫。

### 优先级 6：Electron / Chromium 自动 `AXManualAccessibility` 激活与 MCP 父进程存活探测（直击 `#207` 与 `#189`）

- **现状差距**：1. 未开无障碍的 Electron 窗口在 `get_window_state` 时返回空树或残缺树（`#207` 相关体验）；2. 上游代理异常终止但宿主未关 `AgentScreen mcp` 的 `stdin` 管道时，会话永久假活（`#189`）。
- **拟议方案**：
  1. 参考 `cua` `ax/enablement.rs`，在 `apps/macos/src/mac/ax.rs` 首次遍历目标 PID 发现 Chromium/Electron 空树时，按 `(pid, proc_bsdinfo.pbi_start_tvsec)` 缓存写入一次 `AXManualAccessibility = true`（不支持时回退 `AXEnhancedUserInterface = true`）并短暂等待树填充。
  2. 针对 `#189`：在 `crates/mcp/src/session.rs` 启动时通过 `kqueue` 注册 `EVFILT_PROC` + `NOTE_EXIT` 监听初始 `getppid()`（若父进程退出被 launchd 收养立即主动关闭套接字退出），并在主应用 `HolderStatus` 中引入空闲未响应超时降级，允许跨会话 `Window Request`（ADR 0017）对长期静默且无活跃调用的会话执行安全抢占。
