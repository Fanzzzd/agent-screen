# Agent Screen macOS 核心操作流与状态机图谱

本目录收录 Agent Screen 在 macOS 上的 10 条端到端操作流水线、5 个领域状态机与 1 份外部架构对比（共 16 份正文文档、27 张 Mermaid 图表）。每张表格均包含 `验证证据` 列，逐行引用本工作区源码行号与原文片段、macOS 26 共享缓存静态符号地址、只读运行日志行号或 MacOSX SDK 头文件行号；缺乏直接可引用证据的因果推断或人工实机回归项单列标记为 `未验证`。

## 第三轮逐行核查台账

针对第二轮提交 (`701768c`，宣称 544 行已验证、0 行未验证) 在 33 项独立抽检中暴露出的 8 项错误与 4 项引用偏差（虚构标识符与常数、混淆调试构建专属环境变量、轮询次数与超时失实），第三轮对全部 17 份文档逐行核对源码并重算计数：

1. **当前表格声明总行数**：**572 行**（分布在 16 份正文文档、27 张 Mermaid 图表中）。
2. **附源码/SDK/符号/日志原文引用的已验证行数 (`已验证`)**：**567 行**。
3. **移除的第二轮旧行数**：**85 行**（含基于虚构枚举、虚构工具名、虚构错误码或错误状态拆分的旧记录）。
4. **修订保留与新增行数**：保留并逐行订正补齐原文引用的记录 **459 行**，按真实代码分支与独立待验项新增记录 **113 行**（`544 - 85 + 113 = 572`）。主要订正包括：
   - 删除代码中不存在的标识符与常数：`AGENTSCREEN_NO_AUTOSTART`、`AGENTSCREEN_MAX_MADE`、`MENU_TIMEOUT`、`Origin::Adopted`、`Stop::ApprovalEnded`、独立工具名 `press_element` / `press_menu_item`、错误码 `invalid_params` / `user_takeover`、函数名 `set_screen_edge` / `hide_screen_edge` / `notify::post` / `notify::retract` / `check_initial`、以及 Cua 侧不存在的错误码 `1003`、`launch_app.rs` 与 `VZMacOSConfiguration`。
   - 修正轮询参数与条件编译边界：`Control::start_app` 执行 `/usr/bin/open -g` 并按 `75 * 200ms = 15s` 重试 (`crates/mcp/src/control.rs:368-386`)；`park_owned` 为 `0..75 * 20ms = 1.5s`、`settle_mode` 为 `25 * 20ms = 500ms` (`apps/macos/src/mac/display.rs:317, 442`)；`AGENTSCREEN_NEW_DISPLAYS`、`AGENTSCREEN_DISCONNECT_GRACE_MS`、`AGENTSCREEN_IDLE_EXIT_MS`、`AGENTSCREEN_STREAM_WATCHDOG_MS` 均受 `cfg!(debug_assertions)` 限制，发行版不生效；`launch_app` 已运行应用分支按 `RUNNING_APP_TURN = 4000ms` 总截止时间轮询菜单栏与 `AXPress` (`apps/macos/src/control/agent.rs:6400`)；`#206` 四边红框 `native::show_borders` 与底部 `InputBar` 仅在用户点击 `Allow` 建立 `Grant` 后点亮 (`apps/macos/src/ui/shell.rs:1298-1320`)；`HelpCard` 使用 `WindowKind::PopUp` 与 `LEVEL_POP_UP_MENU = 101` (`apps/macos/src/ui/help_card.rs:73-93`)；`Requests.waiters` 为 `Vec<Waiter>` 多等待者广播列表 (`crates/model/src/request.rs:316, 421`)；`displays-e2e.log:180, 249` 的 `misroute` 来自另一台虚拟屏而非主物理屏；`waitForChange` 对大于 `30s` 的等待启用 `EARLY_CANCEL = 30s` 提前退出复查 (`apps/macos/src/control/agent.rs:3348`)；Cua `WindowKeyScope` 单轮焦点切换固定睡眠 `3 * 120ms = 360ms` (`skylight.rs:241, 272, 277, 289`)。
5. **仍处于未验证状态的行数 (`未验证`)**：**5 行**，每行均写明验证方法：
   - [01-create-display.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/01-create-display.md)：`CAWSManager::restore_color_preferences` (`0x1870e15b4`) 在多屏切换期引发 `WindowServer` `> 50ms` 停顿的直接耗时归因（需在显示重排期用 Instruments Time Profiler 采样 `WindowServer` 验证）。
   - [01-create-display.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/01-create-display.md)：`~/Library/Logs/Agent Screen/hang 2026-11-15-11-08-52.015.txt` 主线程 `273ms` `CA::Transaction::commit` / `NSStatusBarButtonCell` 采样与 `Core::adopt` 状态栏刷新的因果对应（需带时间戳关联 `create_display` 调用链复现验证）。
   - [05-computer-actions.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/05-computer-actions.md)：`#110` 商业版 Dia 浏览器 (`company.thebrowser.dia`) 在用户物理屏与 Agent Display 双窗并存时的实机互不串扰回归（需在用户空闲时启动商业版 Dia 双窗实测）。
   - [06-input-queue-and-real-input.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/06-input-queue-and-real-input.md)：`#205` `real_input: true` 驱动真实物理指针点击 Sheet 与 `layer == 101` 弹出菜单的实机端到端回归（需在允许移动真实指针时运行 `AGENTSCREEN_INSTANCE=e2e cargo e2e run real_input` 验证）。
   - [07-viewer-input.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/07-viewer-input.md)：`#207` 用户在 Viewer `Takeover` 模式下点击真实 `.appex` `NSOpenPanel` 弹出菜单项的手工实机回归（需在虚拟屏内打开系统文件面板并在 Viewer 点击弹出菜单验证）。
6. **Mermaid 11.17.2 渲染校验结果**：全套 **27 张** Mermaid 图表经 `/tmp/flowview/build.py` 生成并在 Orca 内置浏览器实测加载，`svg[aria-roledescription=error]` 计数为 **`0`**。

---

## 文档导航目录

### 一、十大端到端操作流水线 (`01` .. `10`)

| 文档 | 覆盖的 MCP 工具与内部管线 | 图表数 | 重点分支与机制 |
| :--- | :--- | :--- | :--- |
| [01-create-display.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/01-create-display.md) | `create_display`、`open -g` 自启动、备用池复用、对角停泊 | 3 | 会话 `MAX_LIVE=4`、`SPARES=3`、`BUDGET_MS=40ms`、`MadeLimit 10/10min`、`unit_twin` 规避、`park_owned` `0..75` x `20ms` 停泊校验、`#204` 外来窗疏散 |
| [02-end-display-and-sessions.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/02-end-display-and-sessions.md) | `end_display`、`2000ms` 断线宽限、借窗归还、备用屏老化与 `60s` 空闲退出 | 2 | 四类结束路径、`Origin::HandedOver` 迁回 `Home`、残留窗留待查看态、`15s` `tidy` 周期预热与回收 |
| [03-launch-app-and-new-window.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/03-launch-app-and-new-window.md) | `launch_app`、Chromium `FD 3/4` 管道、专用 `CFRunLoop` `Catch`、`ADR 0007` | 2 | 冷启动隐藏拉起 vs 已运行应用 `RUNNING_APP_TURN=4000ms` 后台 `AXPress` 新建窗口、`3s` 动作窗与 `30s` 冷启动裁决、`60s` `Step::GiveUp` |
| [04-place-window-and-handover.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/04-place-window-and-handover.md) | `place_window`、`window_access`、跨会话过户 (`#189`) | 2 | 本屏缩放、同会话兄弟屏调拨、用户屏借窗 Help Card、外会话 `Displays::transfer` 过户、`Lanes` 归还计数屏障 |
| [05-computer-actions.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/05-computer-actions.md) | `computer`、`act_on_element`、`set_value` | 2 | `SkyLight` 局部事件合成、`shared_app` 同进程隔离、标题栏 AX 帧移动、CDP 网页拖拽、`ime` 主线程 TIS 切换、`#205`/`#110` |
| [06-input-queue-and-real-input.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/06-input-queue-and-real-input.md) | `input_queue`、`real_input: true`、批准后四边红框 (`#206`) | 2 | `Allow` 后点亮 `BORDER_LEVEL=1000` 红框与底部 `InputBar`、`30s` 空闲租约、`10%` 像素防篡改复核、`TAG=0x41535121` 瞬移与碰鼠急停 |
| [07-viewer-input.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/07-viewer-input.md) | Viewer 鼠标归一化、`front_for_press`、本地 IME 与密码框暂扣 | 2 | `LIST_SHARE=0.6` 浮层延后提权 (`BeforePress` vs `AfterRelease`)、`.appex` `press_menu_item_at` (`#207`)、`HOLD_MS=400ms` 密码探针暂扣、候选窗跟随 |
| [08-takeover-and-help-requests.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/08-takeover-and-help-requests.md) | `request_help`、Viewer `Takeover`、`wait_for_control` | 2 | `WindowKind::PopUp` (`101`) Help Card + 系统通知、接管仅用户可交还、`unheard` 与 `superseded` 防丢答复、`waiters: Vec<Waiter>` 广播唤醒 |
| [09-peek-and-stream-pipeline.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/09-peek-and-stream-pipeline.md) | Peek 菜单栏预览、`Streams::want` 多订阅聚合、`SCStream` 自愈 | 2 | `300ms`/`500ms` 防抖、`RestartFresh` 放大配置推首帧、`SCStreamFrameInfoScreenRect` 拦截虚拟屏间 `misroute`、`160x100` 静止检测、`#190` |
| [10-window-queries-and-wait.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/10-window-queries-and-wait.md) | `list_windows`、`get_window_state`、`wait_for_change` | 2 | `strays` 未收编窗查询、Chromium `AXManualAccessibility` 按需唤醒、`changesOnly` 行级 Diff、`500ms..2000ms` 轮询与 `EARLY_CANCEL=30s` |

### 二、五大核心领域状态机 (`sm-01` .. `sm-05`)

| 文档 | 状态机模型位置 | 图表状态数 | 核心不变量 |
| :--- | :--- | :--- | :--- |
| [sm-01-display-lifecycle.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/sm-01-display-lifecycle.md) | `crates/model/src/model.rs`, `apps/macos/src/core.rs` | 9 | 未通过 `is_tucked` 停泊校验不入 `Live`；`Takeover` 唯用户可退回 `Live`；有残留窗口必留 `ReviewDisplay` |
| [sm-02-window-ownership.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/sm-02-window-ownership.md) | `crates/model/src/new_window.rs`, `crates/model/src/held.rs`, `crates/model/src/returns.rs` | 12 | 用户 `3s` 内操作过必归 `UserOwned`；`Origin` 仅四个变体；借出窗（含跨会话过户后）结束必迁回最初 `Home` |
| [sm-03-session-and-connection.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/sm-03-session-and-connection.md) | `crates/model/src/session.rs`, `crates/mcp/src/control.rs` | 7 | 单会话上限 `4` 台活跃屏；断线 `2000ms` 宽限期内任意带键请求自动重绑；`CallGuard` 回收取消调用的 `in_flight` 计数 |
| [sm-04-request-lifecycle.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/sm-04-request-lifecycle.md) | `crates/model/src/request.rs` | 10 | 每屏按 `Help` / `Control` / `Window` 分槽独立打开；`unheard` 与 `Woken.superseded` 保留未消费人工答复 |
| [sm-05-input-queue-lifecycle.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/sm-05-input-queue-lifecycle.md) | `crates/model/src/input_queue.rs`, `apps/macos/src/ui/shell.rs` | 14 | 全局单并发真实输入；未批排队期不亮红框；硬件键鼠触碰立即撤销租约且跳过 `restore` 保留用户当前光标 |

### 三、外部架构对比

| 文档 | 对比对象 | 核心发现摘要 |
| :--- | :--- | :--- |
| [cua-comparison.md](file:///Users/fanzhende/orca/workspaces/sidescreen/flow-maps/docs/flows/cua-comparison.md) | `/tmp/cua` (`cua-driver`, `cua-spacesd`, `lume`) | Cua 全仓零 `CGVirtualDisplay`；`cua-driver` 的 `WindowKeyScope`（每次 `3 * 120ms = 360ms` 焦点记录切回）、`1002` 连接诊断 + `responsibility.rs` 自重启、16 UTF-16 分块打字与 `hotkey_no_auth` 可作参考；Agent Screen 在宿主机屏幕级隔离、免端口 CDP 管道、Viewer 本地输入法与批准后红框护栏上覆盖了 Cua 未处理的桌面共处问题 |

---

## 四类验证证据源索引

1. **源码行号与原文片段**：`crates/mcp`、`crates/model`、`crates/colorsync`、`apps/macos/src`、`apps/macos/native/SkyLight.m` 以及 `/tmp/cua/libs/cua-driver`、`/tmp/cua/libs/cua-spacesd`、`/tmp/cua/libs/lume`。
2. **macOS 26 (arm64e) 共享缓存框架静态符号 (`/private/tmp/dsc/SkyLight`, `/private/tmp/dsc/AppKit`)**：
   - `SkyLight`：`_WSVirtualDisplayCreate` (`0x186e6d154`)、`WS::Displays::Display::set_origin` (`0x186e6b304`)、`CAWSManager::restore_color_preferences` (`0x1870e15b4`，符号已核实，其引发 `> 50ms` `WindowServer` 停顿的耗时归因列为 `未验证`)、`_SLSCompleteDisplayConfiguration` (`0x186e51008`)、`_SLEventPostToPid` (`0x187121048`)、`_SLPSPostEventRecordTo` (`0x186ebc164`)、`_copy_primary_conn_send_port` (`0x186eb9934`)、`__SLPSSetFrontProcessWithOptions` (`0x186eba308`)、`+[SLSEventAuthenticationMessage messageWithEventRecord:pid:version:]` (`0x18707cf90`)、`_SLSDragWindowRelativeToMouse` (`0x18710ca20`)、`_SLSHWCaptureWindowList` (`0x186f436dc`)。
   - `AppKit`：`-[NSWindow(NSEventRouting) _handleMouseDownEvent:isDelayedEvent:]` (`0x18571e958`)、`-[NSWindow performWindowDragWithEvent:]` (`0x185673490`)。
3. **本机只读运行日志 (`~/Library/Logs/Agent Screen/`)**：
   - `displays-e2e.log:1`（`unit twin` 释放记录）、`displays-e2e.log:9`（`colorsync` JSON 清理记录）、`displays-e2e.log:37`（`created` 停泊记录）、`displays-e2e.log:180, 249`（`SCStream` 跨虚拟屏 `misroute` 记录）。
   - `hang 2026-11-15-11-08-52.015.txt`（主线程 `273ms` `CA::Transaction::commit` / `NSStatusBarButtonCell` 采样文件本身已核实，其与 `Core::adopt` 的因果对应列为 `未验证`）。
4. **Apple CommandLineTools SDK 头文件 (`/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`)**：`CGDisplayConfiguration.h`、`CGEvent.h`、`AXUIElement.h`、`SCStream.h`、`SCScreenshotManager.h`、`TextInputSources.h`、`NSWorkspace.h`、`UNUserNotificationCenter.h`。
