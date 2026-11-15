# Agent Screen 跨平台移植规划：先 Linux，后 Windows

## 决策摘要

- **显示模型**：每台 Agent Display 运行在独立的 `systemd --user` 瞬态作用域内，独占一个无头 Wayland 合成器、配套 rootless Xwayland 与隔离的 D-Bus 及 AT-SPI 总线。首版调用外部 `sway` 进程，成熟阶段在同一端口下替换为内嵌 `Smithay` 合成器。作用域独立于守护进程存活，守护进程崩溃或升级不中断智能体应用，重启后通过 `createDisplay` 的 `adoptGroups` 重新挂载。
- **核心端口**：摒弃按 `mac::*` 目录平铺的浅层接口，收敛为 3 个深层领域端口：`Display` 封装单台隔离显示器的生命周期、画面采集、录屏编码、输入注入、剪贴板桥接、窗口摆位与应用启动；`Access` 封装无障碍树提取、命中检测与语义元素动作；`Shell` 封装宿主系统托盘、交互通知与环境自检。
- **能力握手**：扩展 NDJSON 控制协议的 `agentReady` 握手，由守护进程返回当前平台支持的特性集合。`AgentScreen mcp` 据此裁剪暴露给模型的工具列表、参数 schema 与引导文案，在 Linux 上直接省去 `window_access`、`input_queue`、`real_input` 与 `via_input_method`，避免模型反复调用必败接口。
- **演进顺序**：先直接基于 `crates/model` 与 NDJSON 协议编写独立的 `apps/linux` 守护进程跑通真实路径，待 `apps/macos` 与 `apps/linux` 双端并存后，再把真正重复的协议传输、网页预览、赋值循环与 CDP 管道抽入 `crates/app-core`，避免单凭 macOS 一端盲目拆分大文件。
- **能力取舍与新 ADR**：由于 Wayland 客户端连接绑定于创建时的合成器，窗口既不能从宿主桌面迁入 Agent Display，也不能在两台 Agent Display 之间互迁。Linux 版放弃跨屏借窗 `Handover`、跨会话 `Window Request`、同会话跨显示器移窗以及销毁显示器时把窗口迁回主屏，审阅态下的「保留窗口」改为保持该 Review Display 存活直至用户关闭窗口，并单独立项 ADR 0020 记录此架构边界。

---

## 1. macOS 能力清单与系统接缝

当前仓库包含 `crates/model`、`crates/mcp` 与 `apps/macos` 三个主要单元。其中 `apps/macos/src/control/agent.rs` 共 8240 行且含 94 处 `mac::` 调用，`apps/macos/src/ui/shell.rs` 共 5498 行且含 53 处 `mac::` 调用，平台机制与状态协调深度交织。以下按系统接缝列出全部 macOS 专有能力。

### 1.1 虚拟显示器与排列预算

- `apps/macos/src/mac/display.rs`：通过 `objc2` 调用 CoreGraphics 私有类 `CGVirtualDisplay`、`CGVirtualDisplayDescriptor`、`CGVirtualDisplayMode` 与 `CGVirtualDisplaySettings` 创建虚拟显示器，注册 1x 与 2x HiDPI 模式。调用 `CGBeginDisplayConfiguration`、`CGConfigureDisplayOrigin` 与 `CGCompleteDisplayConfiguration` 将显示器固定在用户物理屏幕对角线外侧。销毁时后台轮询 `cg::is_online` 直至 WindowServer 释放序列号。
- `apps/macos/src/mac/cg.rs`：调用 `CGGetOnlineDisplayList`、`CGGetActiveDisplayList` 与 `CGDisplayRegisterReconfigurationCallback` 监听拓扑变化。按厂商号 `0x5353` 识别自有显示器，按厂商号 `0x4145` 与型号 `1` 过滤 AeroSpace 的 `closet` 显示器，通过 `unit_twin` 检测镜像，通过 `menubar_height` 读取顶部菜单栏高度。
- `apps/macos/src/mac/colorsync.rs` 与 `crates/model/src/budget.rs`：遵循 ADR 0009，通过 `/usr/bin/osascript` 提权安装 `launchd` root 守护进程 `/Library/PrivilegedHelperTools/com.fanzzzd.agentscreen.colorsync`，清理陈旧 ColorSync 配置文件，把 WindowServer 重排耗时控制在每台显示器 40 毫秒预算内。

### 1.2 画面采集、单帧截图、直播渲染与录制合成

- `apps/macos/src/mac/capture.rs` 与 `apps/macos/src/ui/stream.rs`：使用 `ScreenCaptureKit` 的 `SCShareableContent`、`SCContentFilter`、`SCStream` 与 `SCStreamConfiguration` 采集指定 `CGDirectDisplayID` 的视频帧与音频流。视频帧以 `CVPixelBuffer` 存入 `Latest` 并抽样计算画面变化计数，音频流开启 `excludesCurrentProcessAudio` 经 `AVAssetWriter` 写入 `.m4a`。
- `apps/macos/src/mac/screenshot.rs`：通过 `SCScreenshotManager` 及 `CGDisplayCreateImage` 降级路径抓帧，调用 `ImageIO` 的 `CGImageDestination` 编码 PNG 与 JPEG，并支持点击诊断所需的局部像素采样。
- `apps/macos/src/ui/viewer.rs`、`apps/macos/src/ui/peek.rs`、`apps/macos/src/ui/pointer.rs` 与 `vendor/gpui_apple/`：Viewer 与 Peek 将 `CVPixelBuffer` 传入 `gpui::surface`，由 `CVMetalTextureCache` 零拷贝映射为 Metal 纹理，配合 `vendor/gpui_apple/src/metal_renderer.rs` 的独立 `Presenter` 线程提交上屏。`ui/pointer.rs` 基于纯 GPUI `canvas` 与 `crates/model/src/pointer_geometry.rs` 绘制智能体指针弧线动画，不含任何 `mac::` 调用。
- `apps/macos/src/mac/recording.rs`、`apps/macos/src/mac/mux.rs`、`apps/macos/src/mac/playback.rs` 与 `apps/macos/native/Playback.m`：通过 `AVAssetWriter` 编码 H.264 `.mp4` 与时间轴 JSONL，用 `AVMutableComposition` 免重编码混流音视频，并用 `AVPlayerLayer` 在 Viewer 内回放。

### 1.3 后台输入、点击诊断、真实输入队列、Viewer 按键隐私与剪贴板

- `apps/macos/src/mac/input.rs`、`apps/macos/native/AgentScreenPrivate.h` 与 `apps/macos/native/SkyLight.m`：遵循 ADR 0001，每台显示器运行独立工作线程，调用 SkyLight 私有函数 `SLEventPostToPid` 向目标进程投递 `CGEvent`，配合 `_SLPSSetFrontProcessWithOptions` 与 `SSFocusWithoutRaise` 在不抬升窗口、不抢夺用户前台焦点的前提下完成焦点握手，并用 `CGEventKeyboardSetUnicodeString` 输入 Unicode 文本。
- `apps/macos/src/mac/click_diagnostics.rs` 与 `crates/model/src/diagnostics.rs`：记录每次点击前后的区域像素采样、命中窗口与阶段耗时，供 `getDiagnostics` 排查后台点击失效原因。
- `apps/macos/src/mac/devtools.rs`：遵循 ADR 0010，在启动 Chromium 与 Electron 应用时注入 `--remote-debugging-pipe` 挂载文件描述符 3 与 4，通过 CDP `Input.dispatchMouseEvent` 与 `Input.dispatchDragEvent` 在渲染器内部完成网页拖拽。
- `apps/macos/src/mac/real_input.rs` 与 `apps/macos/src/input_queue.rs`：遵循 ADR 0013 输入队列设计，用 `CGEventTap` 监听用户键鼠活动，在授权窗口期调用 `CGWarpMouseCursorPosition` 瞬移真实光标执行系统拖拽后立即归还，用户一碰键鼠立刻中止。
- `apps/macos/src/mac/ime.rs`：遵循 ADR 0008，调用 Carbon 框架 `TISCopyCurrentKeyboardInputSource` 与 `TISSelectInputSource` 切换输入源，检测用户空闲满 2 秒后短暂借用前台焦点完成输入法组字。
- `apps/macos/src/mac/viewer_input.rs`：共 2292 行，挂载本地 `NSEvent` 监听器绕过 GPUI 事件裁剪，遵循 ADR 0008 与 ADR 0014 在未知目标焦点是否为密码框时暂存按键最多 `viewer_keys::HOLD_MS`，一旦确认处于安全输入或密码框则绕过输入法直传按键且隐去诊断回显。
- `apps/macos/src/mac/native.rs`、`apps/macos/src/ui/reply_field.rs` 与 `crates/mcp/src/tools.rs`：macOS 全局共用同一个 `NSPasteboard::generalPasteboard()`，但后台应用无法响应 `Cmd+C` 与 `Cmd+V` 菜单快捷键，因此 `tools.rs` 引导智能体改用无障碍动作 `read_text` 与 `insert_text`。

### 1.4 窗口列表、Catch 归属拦截、共享窗口与 AeroSpace 协同

- `apps/macos/src/mac/windows.rs` 与 `apps/macos/src/mac/cg.rs`：通过 `CGWindowListCopyWindowInfo`、`SLSCopyWindowsWithOptionsAndTags` 与 `SLSMoveWindowsToManagedSpace` 枚举窗口并跨显示器迁移。
- `apps/macos/src/mac/catch.rs`：遵循 ADR 0007，结合 `NSWorkspace` 激活观察者与辅助功能通知拦截新窗口，将智能体冷启动产生的新窗口抓回对应 Agent Display，同时保护用户刚从 Dock 点击打开的窗口不被误抓。
- `apps/macos/src/mac/shared_app.rs`、`apps/macos/src/control/shared_app.rs` 与 `apps/macos/src/dismissal.rs`：遵循 ADR 0004、ADR 0013、ADR 0016、ADR 0017 与 ADR 0018，管理跨显示器与跨会话窗口借入 `Handover`、授权 `Window Authorization`、持久化所有权记录以及结束审阅时的归还或关闭计划。
- `apps/macos/src/mac/aerospace.rs` 与 `apps/macos/src/mac/claims.rs`：遵循 ADR 0006，维护 `~/.aerospace.toml` 的 `ignored-monitors` 与 `claims.json`。

### 1.5 辅助功能树、元素动作与应用菜单栏代理

- `apps/macos/src/mac/ax.rs`：封装 `AXUIElement` 属性读写、动作执行、坐标命中检测、`AXEnhancedUserInterface` 与 `AXManualAccessibility` 激活及窗口移动缩放。
- `apps/macos/src/mac/menubar.rs`：读取虚拟显示器顶层应用的 `AXMenuBar`，在 Viewer 顶部绘制代理菜单条并触发 `AXPress`。
- `apps/macos/src/mac/alerts.rs` 与 `crates/model/src/alerts.rs`：识别落入虚拟显示器的 macOS 系统授权弹窗进程。

### 1.6 应用启动、系统托盘、通知、权限与打包传输

- `apps/macos/src/mac/app.rs` 与 `apps/macos/src/mac/devtools.rs`：通过 `NSWorkspaceOpenConfiguration` 后台启动 `.app`，或用 `posix_spawn` 配合 `responsibility_spawnattrs_setdisclaim` 剥离 TCC 权限归属。
- `apps/macos/src/mac/native.rs`、`apps/macos/src/mac/notify.rs`、`apps/macos/src/mac/permissions.rs`、`apps/macos/src/mac/assist.rs`、`apps/macos/src/mac/hang.rs` 与 `apps/macos/src/mac/update.rs`：实现 `NSStatusItem` 托盘、非激活 `NSPanel` 悬浮窗、`UNUserNotificationCenter` 行内回复通知、TCC 权限探测、主线程卡死监控与 Sparkle 自动更新。
- `apps/macos/src/control/server.rs`、`apps/macos/src/control/agent.rs`、`apps/macos/src/web/mod.rs` 与 `crates/mcp/src/`：监听 Unix 域套接字承载 NDJSON 协议；`createDisplay` 接收 `adopt` 与 `adoptGroups` 参数领养应用重启前遗留的智能体子进程；`web/mod.rs` 提供本机回环 MJPEG 预览。

---

## 2. 领域设计与深层端口抽象

### 2.1 现有领域层与真实耦合评估

`crates/model` 的 38 个纯算法模块已可在 Linux 与 Windows 上直接编译并通过测试，覆盖会话状态 `session.rs`、显示器几何 `arrangement.rs`、按键解析 `keychord.rs`、Viewer 按键隐私路由 `viewer_keys.rs`、稳定元素编号 `numbers.rs`、指针轨迹几何 `pointer_geometry.rs`、网页预览策略 `web_preview.rs` 等纯规则。其中 `alerts.rs`、`budget.rs`、`claims.rs`、`screen_edge.rs` 与 `launch.rs` 残留少量 macOS 常量，可在后续按平台参数化。

与此相对，`apps/macos/src/control/agent.rs` 与 `apps/macos/src/ui/shell.rs` 绝不可能在未验证第二平台形态前「零改动平移」。原因有三：第一，这两处文件混合了 GPUI `AsyncApp` 调度、`MainThreadMarker` 主线程约束、AeroSpace 轮询守卫与 `catch.rs` 冷启动窗口拦截；第二，macOS 的许多控制分支是为了绕开单窗口服务器限制而存在的补偿逻辑，在拥有独立合成器的 Linux 上根本不存在；第三，在只有一个平台实现时强行抽离 `crates/app-core`，极易把 `cross_host_move` 或 `needs_host_input_queue` 这类 macOS 痕迹固化进通用接口。

因此采取务实的分层路线：`crates/mcp` 与各平台守护进程之间以扩展了能力握手的 NDJSON 控制协议作为首要边界；先让 `apps/linux` 直接复用 `crates/model` 独立跑通，待双端代码对照清晰后，再将真正完全一致的 NDJSON 传输帧、回环 `web_preview` HTTP 服务、`values::set_value` 循环、`devtools` 管道客户端与录制 JSONL 写入器沉淀到 `crates/app-core`。

### 2.2 三个深层领域端口：名称、方法与不变量

应用核心仅面向以下 3 个以内聚领域概念划分的深层端口编程，不再按 `mac::*` 源文件一拆七。

#### 1. `Display`：单台隔离显示器及其内部世界

```rust
pub trait Display: Send + Sync {
    fn id(&self) -> DisplayId;
    fn metrics(&self) -> DisplayMetrics;
    fn resize(&self, size: SizePoints, hi_dpi: bool) -> BoxFuture<Result<DisplayMetrics, PortError>>;
    fn subscribe_frames(&self, config: StreamConfig, sink: Arc<FrameSink>) -> Result<Box<dyn ActiveStream>, PortError>;
    fn screenshot(&self, opts: SnapshotOpts) -> BoxFuture<Result<EncodedImage, PortError>>;
    fn start_recording(&self, out: &Path) -> BoxFuture<Result<Box<dyn ActiveRecording>, PortError>>;
    fn act_input(&self, op: InputOp) -> BoxFuture<Result<InputOutcome, PortError>>;
    fn sync_clipboard(&self, dir: ClipboardSync) -> BoxFuture<Result<(), PortError>>;
    fn launch_app(&self, req: LaunchRequest) -> BoxFuture<Result<LaunchedApp, PortError>>;
    fn list_windows(&self) -> BoxFuture<Vec<WindowInfo>>;
    fn place_window(&self, target: WindowTarget, frame: Option<Rect>) -> BoxFuture<Result<PlaceOutcome, PortError>>;
    fn focus_window(&self, target: WindowTarget) -> BoxFuture<Result<(), PortError>>;
    fn close_window(&self, target: WindowTarget) -> BoxFuture<Result<(), PortError>>;
    fn close(self: Box<Self>) -> BoxFuture<Result<(), PortError>>;
}

pub trait DisplayManager: Send + Sync {
    fn capabilities(&self) -> PlatformCapabilities;
    fn create(&self, spec: DisplaySpec, orphans: &[Orphans]) -> BoxFuture<Result<Box<dyn Display>, PortError>>;
    fn reattach_surviving(&self) -> BoxFuture<Vec<RecoveredDisplay>>;
    fn host_windows(&self) -> Option<&dyn HostWindowBridge>;
}
```

- 不变量：
  - 一台 `Display` 实例代表一个完整的视觉、音频、输入与窗口容器。录屏编码与音频混流作为画面采集的消费者归属 `Display::start_recording`，不混入宿主桌面外壳。
  - `act_input` 在任何平台上都承诺「未经用户显式放行，绝不移动宿主真实光标、绝不抢夺宿主窗口焦点」。至于底层走 SkyLight 后台事件、CDP 渲染器管道、Wayland 私有席位，还是在 macOS 与 Windows `IddCx` 上退入内部 `InputQueue` 返回 `InputOutcome::Pending`，全部封装在平台 `Display` 实现内部，协调器无需感知平台输入缺陷标志。
  - 跨宿主或跨显示器的既有窗口借入、归还能力单独收纳于可选子接口 `HostWindowBridge`，仅在 `capabilities()` 声明支持该特性的平台上返回 `Some`，Linux 直接返回 `None`。

#### 2. `Access`：窗口无障碍树与语义元素动作

```rust
pub trait Access: Send + Sync {
    fn window_state(&self, display: DisplayId, target: Option<WindowTarget>, scope: TreeScope) -> BoxFuture<Result<WindowStateSnapshot, PortError>>;
    fn focus_secrecy(&self, display: DisplayId, at: Option<Point>) -> BoxFuture<Secrecy>;
    fn act_on_element(&self, display: DisplayId, req: ElementActionReq) -> BoxFuture<Result<ElementActionReply, PortError>>;
}
```

- 不变量：
  - 统一输出 `agent_screen_model::tree::Node` 归一化节点树，确保 `crates/model::numbers` 元素编号在三端一致；在窗口菜单栏独立于窗口内容的 macOS 上，`WindowStateSnapshot` 附带菜单栏节点，在菜单位于窗口内部的 Linux 与 Windows 上自然包含于常规子树。
  - `focus_secrecy` 为 ADR 0014 Viewer 按键隐私提供跨平台查询入口，用于在 `viewer_keys::HOLD_MS` 时限内判定当前焦点是否位于密码框。

#### 3. `Shell`：宿主桌面存在感、通知与诊断

```rust
pub trait Shell: Send + Sync {
    fn set_presence(&self, state: PresenceState);
    fn post_notification(&self, display: DisplayId, note: Note);
    fn withdraw_notification(&self, display: DisplayId);
    fn platform_diagnostics(&self) -> Value;
}
```

- 不变量：`Shell` 只负责宿主桌面上的托盘或状态小窗、系统通知以及平台环境自检数据；所有用户点击与通知回复均转为统一的领域事件投递回协调器，被动弹出的提示界面绝不抢占宿主输入焦点。

### 2.3 macOS 现有模块向三端口的完整映射

| 端口或共享层 | `apps/macos` 对应模块 | 职责归属说明 |
| --- | --- | --- |
| `DisplayManager` + `Display` | `mac::display`、`mac::cg`、`mac::colorsync`、`mac::capture`、`mac::screenshot`、`mac::recording`、`mac::mux`、`mac::input`、`mac::devtools`、`mac::real_input`、`input_queue`、`mac::ime`、`mac::click_diagnostics`、`mac::app`、`mac::windows`、`mac::catch`、`mac::aerospace`、`mac::claims` | 备用显示器池、ColorSync 预算、SCK 采集与 AVFoundation 混流、SkyLight 投递、点击像素前后采样诊断、`InputQueue` 门控真实输入、输入法借用、`catch.rs` 冷启动新窗口归属拦截与 AeroSpace 协同全部内聚在 macOS 的 `Display` 实现内部 |
| `HostWindowBridge` | `mac::shared_app`、`control::shared_app`、`dismissal` 的 `GiveBack` 分支 | 仅 macOS 与未来的 Windows `IddCx` 实现此接口，承载跨屏借窗 `Handover`、跨显示器移窗与关闭显示器时将窗口退回主屏 |
| `Access` | `mac::ax`、`mac::menubar`、`control::values` | AX 树提取、Viewer 密码框 `Secrecy` 检测、顶部代理菜单栏读取与 `set_value` 校验状态机 |
| `Shell` | `mac::native`、`mac::notify`、`mac::permissions`、`mac::assist`、`mac::hang`、`mac::update` | 菜单栏图标、非激活 `NSPanel`、带行内回复的系统通知、TCC 权限检查、主线程卡死监控与 Sparkle 更新 |
| GPUI 视图与 Viewer 输入 | `ui::viewer`、`ui::peek`、`ui::pointer`、`ui::help_card`、`ui::recordings`、`ui::reply_field`、`mac::viewer_input`、`mac::playback` | `ui/pointer.rs`、卡片布局与主题为纯 GPUI 代码可跨端共享；`viewer_input.rs` 保留在 macOS 侧处理 `NSEvent` 滚动相位与修饰键，Linux 侧用 GPUI 输入事件加 `Access::focus_secrecy` 对等实现 ADR 0014 |

### 2.4 Cargo 工作区最终目录结构

```text
crates/
  model/          # 纯领域规则与状态机，零系统调用
  mcp/            # AgentScreen mcp 二进制，按 agentReady 能力集动态生成工具与说明
  app-core/       # 阶段 3 从双端重复代码中抽出的 NDJSON 服务端、Web Preview、CDP 管道与三端口定义
  ui-common/      # 阶段 4 抽出的纯 GPUI 组件：Agent Pointer 动画、Help Card、主题与状态文案
apps/
  macos/          # macOS 守护进程与 GPUI 界面，实现 Display、HostWindowBridge、Access、Shell
  linux/          # Linux 守护进程与 GPUI 界面，实现 Display、Access、Shell
  windows/        # 后续 Windows 守护进程与 GPUI 界面
```

### 2.5 `agentReady` 能力握手与 Linux 能力边界

#### 协议能力协商机制

当前 `crates/mcp/src/lib.rs` 在 `list_tools` 时异步发送 `agentReady` 并固定返回同一套工具定义。改造后，`agentReady` 请求与响应升级为双向能力协商：

```json
{
  "platform": "linux",
  "features": [
    "multi_display",
    "surviving_adoption",
    "web_preview",
    "accessibility",
    "devtools_pipe",
    "recording",
    "in_display_clipboard_shortcuts"
  ]
}
```

macOS 额外返回 `"host_window_handover"`、`"cross_display_move"`、`"input_queue"`、`"ime_borrow"` 与 `"separate_menubar"`。若 `list_tools` 调用时守护进程尚未启动，`AgentScreen mcp` 先采用编译目标平台的静态默认特性集立即响应客户端，保证 `list_tools` 不因拉起守护进程而阻塞；一旦连接建立则以守护进程实报特性集为准。

基于该特性集，`AgentScreen mcp` 在 Linux 上执行以下裁剪与文案适配：

1. 直接从 `list_tools` 移除 `window_access` 与 `input_queue` 两个工具。
2. 从 `computer` 工具参数中移除 `real_input`、`via_input_method` 与 `input_source`；从 `act_on_element` 工具参数中移除 `real_input`。
3. 改写 `INSTRUCTIONS`、`computer` 与 `act_on_element` 说明文案：将 `open -a`、`AppleScript`、`Finder` 替换为 Linux 桌面启动命令说明；删除顶部 25pt macOS 菜单栏禁区描述；修正 `crates/mcp/src/tools.rs` 第 207 行与第 558 行关于复制粘贴的说法。在 Linux 私有合成器内目标窗口拥有真实席位焦点，`ctrl+c`、`ctrl+v`、`ctrl+z` 在显示器内部完全有效，只需告知模型该显示器的剪贴板与用户宿主剪贴板相互隔离。

#### 诚实列出单显示器独立合成器在 Linux 上失去的产品特性与新 ADR 语义

由于 Wayland 客户端在启动时即绑定到特定合成器套接字且协议层不支持跨合成器热迁移连接，采用每台 Agent Display 一个独立合成器的架构后，**不仅宿主桌面的既有窗口无法移入 Agent Display，同一会话的多台 Agent Display 之间以及不同会话的 Agent Display 之间也无法互相移动窗口**。这直接影响现有 5 项设计，需单独立项 **ADR 0020：独立合成器后端的显示器绑定窗口模型** 明确其 Linux 语义：

| 受影响概念与原 ADR | macOS 行为 | Linux 行为与重定义语义 |
| --- | --- | --- |
| 同会话多显示器移窗，见 ADR 0016 | 单个会话最多拥有 4 台显示器，可调用 `place_window` 把自己在显示器 1 上的窗口移到显示器 2 | 单个会话仍可同时创建最多 4 台 Agent Display 并按句柄操作，但窗口终身绑定在启动它的那台显示器上；`place_window` 仅用于调整窗口在所属显示器内的位置与尺寸，省略目标显示器切换语义 |
| 跨会话请求窗口，见 ADR 0017 | 会话 B 可通过 `Window Request` 请求借用会话 A 显示器或 Review Display 上的窗口 | 不支持。会话不能请求接管其他显示器上的既有窗口，只能用 `launch_app` 在自身显示器内启动应用实例 |
| 跨重排持久化所有权记录，见 ADR 0018 | WindowServer 增删显示器重排坐标时，依靠 `Placement` 与 `Handover` 记录判定窗口归属 | 独立合成器无全局坐标重排问题，窗口归属由其连接的合成器实例天然唯一决定，全部窗口来源固定为 `Origin::Launched` |
| 宿主窗口借入 `Handover`、`Window Request` 与 `Give`，见 `CONTEXT.md`、ADR 0013、ADR 0015 | 用户可通过 Viewer 的 `Give` 按钮或 Help Card 第四种回答把自己的浏览器窗口交给智能体 | 不提供 `Give` 按钮与 `Window Authorization`；Help Card 与系统通知提供其余三种回答：`Take Over`、`Continue` 与 `Reply`；需要登录态时由用户在 Viewer 中对智能体显示器执行一次 `Takeover` 完成登录，登录态持久化在智能体专属配置目录中供后续会话复用 |
| 审阅态 Review Display 与销毁时的「保留窗口」，见 `CONTEXT.md`、ADR 0004、`dismissal.rs` | 会话结束但留有窗口时进入 Review Display；用户在关闭面板中选择「保留」会把窗口迁回用户主屏幕再销毁虚拟显示器 | 会话结束且留有窗口时同样转为 Review Display，停止接受智能体输入。由于无法把窗口迁回宿主合成器，`Disposition::Keep` 的含义定义为「保留该窗口在 Review Display 中不关闭」，显示器持续存活供用户在 Viewer 中查阅、保存文件或复制内容；仅当用户关闭其内窗口或显式确认「全部关闭并移除显示器」时才终止该合成器作用域 |
| 应用重启后的孤儿进程领养 `createDisplay` 的 `adopt` 与 `adoptGroups`，见 `control/agent.rs:2964` | 守护进程退出导致虚拟显示器消失、窗口散落主屏；新守护进程收到 `createDisplay` 携带的 `adoptGroups` 后按原显示器分组领养存活 PID 并拉回新虚拟显示器 | **注意修正前稿笔误：`adopt` 与 `adoptGroups` 是 `createDisplay` 的参数而非 `endDisplay` 参数**。配合 3.1 节的 `systemd --user` 瞬态作用域，守护进程崩溃或升级时合成器与子应用原地存活；守护进程重启后扫描运行时目录重连存活作用域，在收到 `createDisplay` 的 `adoptGroups` 时直接将该会话原显示器句柄重新绑定回对应的存活合成器 |

---

## 3. Linux 逐接缝技术设计

### 3.1 显示模型与 `systemd --user` 故障存活作用域

选型结论：每台 Agent Display 运行在一个独立的 `systemd --user` 瞬态作用域内，内部包含一个无头 Wayland 合成器、按需启动的 rootless [Xwayland](https://wayland.freedesktop.org/docs/book/Xwayland.html) 以及独立的会话 D-Bus 与 AT-SPI 总线。首版采用外部 `sway` 无头进程，后续在 `Display` 端口内部替换为内嵌 [Smithay](https://github.com/Smithay/smithay) 合成器。

四个候选方案的对比结论明确：

1. **宿主合成器虚拟输出**：不论是 GNOME Mutter、KDE KWin 还是 Hyprland 的虚拟输出，注入输入所用的 [libei](https://gitlab.freedesktop.org/libinput/libei) 或 Portal 均作用于宿主唯一的逻辑 `wl_seat`，必定移动用户真实光标并抢夺焦点，直接违反 ADR 0001。
2. **纯 [Xvfb](https://www.x.org/releases/current/doc/man/man1/Xvfb.1.xhtml)**：[GNOME 官方关于移除 X11 会话的说明](https://blogs.gnome.org/alatiera/2025/06/08/the-x11-session-removal/) 确认 GNOME 50 已移除 X11 会话，现代 GTK4 与 Wayland 原生工具链以 Wayland 为主路径，纯 X11 虚拟帧缓冲无法覆盖全生态。
3. **每显示器独立无头 [sway](https://github.com/swaywm/sway)**：相比仅支持单窗口全屏信息亭的 [cage](https://github.com/cage-kiosk/cage)，基于 [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots) 的 `sway` 支持 `WLR_BACKENDS=headless`、多窗口浮动坐标摆位、JSON 窗口树 IPC 以及 rootless Xwayland，可在数周内交付完整 MVP。每台显示器独立实例保证了各自的 `wl_seat` 修饰键、拖拽按下态与焦点互不串扰。
4. **每显示器进程外或受控子进程内嵌 [Smithay API](https://docs.rs/smithay/latest/smithay/)**：消除了要求用户系统安装 `sway` 的外部依赖，但在实现 [xdg-shell](https://wayland.app/protocols/xdg-shell) 与 Xwayland 窗口管理器胶水上工作量较大，适合在 `sway` 跑通全部测试后平滑替换。注意为保持「守护进程重启不杀死显示器内应用」的特性，即便采用 `Smithay`，每台显示器的合成器也应作为 `agent-screen --compositor-worker` 独立子进程运行在各自的 `systemd --user` 作用域内，而非与主界面守护进程绑死在同一进程生命周期里。

**用 `systemd --user` 瞬态作用域替代 `PR_SET_PDEATHSIG`**：绝不给合成器或智能体应用设置 `PR_SET_PDEATHSIG(SIGTERM)`，否则守护进程一旦崩溃或热更新就会瞬间杀死全部智能体打开的编辑器与浏览器，造成未保存数据丢失。正确做法是通过 D-Bus 或 [systemd-run](https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html) 将每台显示器放入 `agent-screen-display-<id>.scope` 瞬态作用域，并在 `$XDG_RUNTIME_DIR/agent-screen/displays/<id>/state.json` 持久化套接字路径与归属会话键。守护进程重启后扫描存活作用域并重连套接字：若原 MCP 会话仍在，下次 `createDisplay` 携带 `adoptGroups` 即可无缝续接；若原会话已退出，则直接恢复为 Review Display 供用户处置。仅在无 `systemd --user` 的精简 CI 容器内回退至 `setsid` 独立会话进程组配合 PID 文件管理。

### 3.2 画面采集与 GPUI 渲染链路

私有合成器内部无需部署 [PipeWire](https://docs.pipewire.org/) 与 Portal 权限守护进程。

- **`sway` 采集协议选型**：Ubuntu 24.04 LTS 官方仓库提供的 [Ubuntu Noble sway 软件包](https://packages.ubuntu.com/noble/sway) 版本为 1.9，早于引入 [ext-image-copy-capture-v1](https://wayland.app/protocols/ext-image-copy-capture-v1) 的 sway 1.11。因此 [wlr-screencopy-unstable-v1](https://wayland.app/protocols/wlr-screencopy-unstable-v1) 是面向 Ubuntu 24.04 CI 与 LTS 用户的**必选主实现**，在新版合成器上再优先协商 `ext-image-copy-capture-v1`。通过 `copy_with_damage` 仅在画面产生脏区时拷贝帧并递增变化计数。
- **GPUI 画面渲染**：当前仓库锁定的 [GPUI surface.rs 源码](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/elements/surface.rs) 仅接受 Apple `CVPixelBuffer`。Linux 版 Viewer 与 Peek 使用 `gpui::RenderImage` 配合 `img` 渲染 BGRA 帧，并在每次换帧时显式调用 `window.drop_image` 释放上一帧 Vulkan 图集槽位，杜绝显存泄漏。

### 3.3 后台输入、Viewer 按键隐私与跨合成器剪贴板桥接

- **后台输入**：`apps/linux` 以长连接客户端绑定 [wlr-virtual-pointer-unstable-v1](https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1) 与 [virtual-keyboard-unstable-v1](https://wayland.app/protocols/virtual-keyboard-unstable-v1)。指针点击、悬停与拖拽直接作用于私有席位；任意 UTF-8 文本通过动态上传单键 XKB keymap 输入，无需借用宿主键盘或宿主输入法。
- **Viewer 输入与密码框按键隐私**：用户在宿主桌面聚焦 Viewer 进行 Takeover 操作时，`gpui_linux` 通过 [text-input-unstable-v3](https://wayland.app/protocols/text-input-unstable-v3) 对接宿主桌面输入法。为落实 ADR 0014，Viewer 键盘处理器复用 `crates/model::viewer_keys` 状态机，在未知目标焦点类型时暂存按键最多 `HOLD_MS` 并调用 `Access::focus_secrecy` 查询 AT-SPI 焦点角色：若命中 `ATSPI_ROLE_PASSWORD_TEXT`，立即绕过宿主输入法预编辑直接发送原始按键，并禁止在任何日志或点击诊断中记录键值。
- **跨合成器剪贴板隔离与 Takeover 桥接**：每个 Wayland 合成器各自维护独立的 `wl_data_device` 剪贴板。若不加桥接，用户在 Takeover 时无法在宿主桌面与 Viewer 之间互相复制粘贴。设计上区分两种状态：
  1. 当显示器处于智能体后台自主运行状态时，严格保持剪贴板隔离，禁止显示器内应用读取用户宿主剪贴板中的密码或隐私文本，智能体在同一显示器内部可正常使用 `ctrl+c` 与 `ctrl+v`。
  2. 当且仅当处于用户 Takeover 期间且 Viewer 窗口获得宿主焦点时，`apps/linux` 通过绑定私有合成器的 [ext-data-control-v1](https://wayland.app/protocols/ext-data-control-v1) 或 [wlr-data-control-unstable-v1](https://wayland.app/protocols/wlr-data-control-unstable-v1) 与 GPUI 的宿主剪贴板接口做双向文本与图片同步；一旦用户交回控制权即刻断开同步。

### 3.4 辅助功能树与真实桌面 AT-SPI 隔离陷阱

无障碍层基于 [at-spi2-core](https://gitlab.gnome.org/GNOME/at-spi2-core) 协议，使用纯 Rust 库 [atspi](https://docs.rs/atspi/latest/atspi/) 与 [zbus](https://docs.rs/zbus/latest/zbus/) 调用 `org.a11y.atspi.Cache.GetItems` 批量拉取节点树。

**重要风险声明**：前期容器探针尚未在运行完整 GNOME 或 KDE 的真实物理桌面上验证 AT-SPI 隔离，此处标记为**尚未在真实桌面验证**，且上游源码揭示了三处必须显式处理的冲突机制。核查 [at-spi-bus-launcher.c 源码](https://gitlab.gnome.org/GNOME/at-spi2-core/-/blob/main/bus/at-spi-bus-launcher.c) 可知：

1. **套接字路径碰撞与误删**：第 649 至 653 行将无障碍套接字命名为 `$XDG_RUNTIME_DIR/at-spi/bus` 拼接 `$DISPLAY` 并将冒号替换为下划线，当 `DISPLAY` 未设置时退化为固定的 `$XDG_RUNTIME_DIR/at-spi/bus`，且第 483 行在 `dbus-broker` 模式下会直接 `unlink` 同名路径。若子会话未设置专属 `DISPLAY` 或共享同一 `at-spi` 目录，会直接撞上甚至摘除宿主桌面的无障碍套接字。
2. **宿主环境变量泄漏**：若宿主桌面本身设置了 `AT_SPI_BUS_ADDRESS`，子进程继承后会绕过私有会话直接连回宿主的无障碍总线。
3. **Xwayland 根窗口属性缺失**：第 686 行仅在 `DISPLAY != NULL && WAYLAND_DISPLAY == NULL` 时才向 X11 根窗口写入 `AT_SPI_BUS` 属性。在同时存在 `WAYLAND_DISPLAY` 与 `DISPLAY` 的无头 Wayland 会话内，`at-spi-bus-launcher` 不会写入该 X11 属性，导致走 Xwayland 的 X11 应用找不到私有无障碍总线。

针对以上三点的精确隔离方案：

- 每台 Agent Display 必须启动自带的 rootless Xwayland 并通过 `-displayfd` 获取唯一的 `DISPLAY` 编号。
- 守护进程派生显示器作用域前，必须彻底清除继承自宿主的 `AT_SPI_BUS_ADDRESS` 环境变量，并为每台显示器单独拉起监听 `$XDG_RUNTIME_DIR/agent-screen/displays/<id>/atspi.sock` 的专用无障碍 `dbus-daemon` 与 `at-spi2-registryd`，或者为该作用域配置独立的运行时子目录。
- 向该显示器内启动的所有子进程显式注入 `AT_SPI_BUS_ADDRESS=unix:path=.../atspi.sock`、`GTK_MODULES=gail:atk-bridge`、`GTK_A11Y=atspi`、`QT_ACCESSIBILITY=1`、`QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`，对 Chromium 与 Electron 追加 `--force-renderer-accessibility`，并在 Xwayland 就绪后主动向其根窗口写入 `AT_SPI_BUS` 原子属性，确保 Wayland 与 X11 应用无一遗漏地连入该显示器的专属无障碍总线。

### 3.5 窗口管理与启动隔离的配套桌面服务代偿

在私有 `sway` 内默认开启浮动窗口模式并隐藏状态栏，通过持久 `$SWAYSOCK` IPC 连接及 [wlr-foreign-toplevel-management-unstable-v1](https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1) 实现窗口枚举、聚焦与局部坐标摆位。应用查找遵循 [Freedesktop Desktop Entry 规范](https://specifications.freedesktop.org/desktop-entry-spec/latest/) 与 [XDG Base Directory 规范](https://specifications.freedesktop.org/basedir-spec/latest/)，并为浏览器与 Electron 应用默认挂载 `$XDG_STATE_HOME/agent-screen/profiles/<app-id>/` 隔离配置目录以规避 `SingletonLock` 冲突。

**隔离私有 `DBUS_SESSION_BUS_ADDRESS` 带来的三项宿主服务断联及代偿措施**：

1. **密码环服务断联**：私有 D-Bus 内不存在宿主的 [Freedesktop Secret Service](https://specifications.freedesktop.org/secret-service/latest/) 如 GNOME Keyring 或 KWallet。若不处理，Chromium 与部分应用在启动时会阻塞等待密码环超时或弹窗报错。解决方案是对隔离配置目录下的 Chromium 与 Electron 自动追加 `--password-store=basic`，使其凭据加密或明文保存在权限 `0700` 的智能体专属配置目录内，同时从物理上保证智能体绝不能读取用户宿主密码环中的个人密钥。
2. **Portal 文件对话框及 Snap/Flatpak 沙箱应用受阻**：切断宿主 D-Bus 同时切断了 [XDG Desktop Portal](https://flatpak.github.io/xdg-desktop-portal/docs/)。一方面，普通 GTK/Qt 应用若尝试调用 Portal 文件选择器会失败或等待超时，因此必须注入 `GTK_USE_PORTAL=0` 强制使用渲染在 Agent Display 内部的原生窗口内文件对话框；另一方面，受限于 AppArmor 与 Portal 架构的沙箱包如 **Ubuntu 24.04 默认预装的 Firefox Snap 包** 以及部分严格权限 Flatpak 应用，会强依赖宿主会话 D-Bus 与固定名称的宿主 Wayland 套接字，在私有总线与私有套接字下可能无法启动或将对话框弹到宿主桌面。该兼容性边界标记为**尚未在真实桌面验证**，阶段 0 必须在真实 Ubuntu 24.04 桌面实测 Snap 版 Firefox 行为，必要时在 `launch_app` 诊断与文档中引导使用原生 `.deb`、Mozilla 官方 APT 源包或 Chrome 原生包。
3. **应用内部通知超时**：智能体控制的应用若调用 `org.freedesktop.Notifications` 发送桌面通知，在空总线上会因 D-Bus 激活失败而卡顿。每台显示器的私有 D-Bus 内内置一个极简的 `org.freedesktop.Notifications` 静默接收协程，立即返回成功 ID 并丢弃弹窗，既防止应用阻塞，又避免应用通知打扰宿主用户。

### 3.6 宿主托盘、通知、GPUI 窗口与打包分发

- **宿主托盘与 GNOME 降级**：在宿主会话 D-Bus 上通过 [ksni](https://docs.rs/ksni/latest/ksni/) 实现 [StatusNotifierItem 规范](https://specifications.freedesktop.org/status-notifier-item/latest/)。运行时检测宿主是否注册 `StatusNotifierWatcher`；在默认无托盘的原生 GNOME 上，只要存在活跃或待审阅的显示器，自动保持一个紧凑的 GPUI 状态小窗作为视觉入口。
- **Peek 面板与通知**：在 KDE Plasma Wayland、sway 等支持 [wlr-layer-shell-unstable-v1](https://wayland.app/protocols/wlr-layer-shell-unstable-v1) 的合成器上使用 [GPUI platform.rs](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/platform.rs) 的 `WindowKind::LayerShell` 非激活悬浮窗；在 GNOME Mutter 上降级为点击唤出的普通窗口。系统通知通过宿主 D-Bus 的 [Freedesktop Desktop Notifications](https://specifications.freedesktop.org/notification-spec/latest/) 发送，动态探测 `inline-reply` 能力，不支持行内回复时点击跳回 Help Card 输入框。
- **打包策略**：首版发布 `.deb` 与 `.rpm` 系统原生包，直接访问宿主已安装软件并安装 `systemd --user` 单元；阶段 5b 内嵌 `Smithay` 后提供 [AppImage](https://docs.appimage.org/)；因 [Flatpak 沙箱权限](https://docs.flatpak.org/en/latest/sandbox-permissions.html) 隔离宿主 `/usr/bin` 应用空间，不发布 Flathub 包。

### 3.7 Linux 相比 macOS 变简单与变难的真实对照

- **变简单**：免除全部 TCC 隐私授权弹窗与应用重启；私有合成器席位使后台点击、悬停、拖拽及显示器内 `ctrl+c`/`ctrl+v` 原生可用，省去 SkyLight 私有 SPI、`InputQueue` 真实光标借用与输入法前台借用；无全局坐标重排与 ColorSync root 守护进程；无需伪造屏幕顶部菜单栏；核心守护进程与协议层可在无头 Linux CI 中自动化测试。
- **变难**：彻底失去跨宿主与跨显示器窗口迁移能力，无 `Handover`、无跨会话借窗、Review Display 保留窗口需长期驻留合成器；AT-SPI 总线在真实桌面存在套接字碰撞与 Xwayland 根属性缺失陷阱；私有 D-Bus 切断宿主密钥环与 Portal，导致 Ubuntu 默认的 Firefox Snap 等沙箱应用存在兼容性障碍；剪贴板按合成器隔离，必须在 Takeover 期间自建 `data-control` 桥接；GPUI 缺少 Linux 零拷贝视频表面且 GNOME 与 KDE 桌面外壳能力分裂。

---

## 4. 分阶段实施计划

演进顺序遵循「先在真实桌面上排雷 -> 先独立写出 `apps/linux` 守护进程跑通协议 -> 再基于双端真实重复抽离 `crates/app-core` -> 最后上层接 GPUI 界面与内嵌合成器」。

### 阶段 0：真实 Linux 桌面与无头探针排雷

- **范围**：编写独立验证程序，在真实 Ubuntu 24.04 GNOME 桌面、KDE Wayland 桌面及无头环境中逐一实测：第一，`systemd --user` 瞬态作用域在守护进程被 `SIGKILL` 后的存活与重连；第二，`sway` 1.9 下 `wlr-screencopy-unstable-v1` 帧读取与虚拟键鼠拖拽；第三，清除 `AT_SPI_BUS_ADDRESS`、独立 `DISPLAY` 与手写 Xwayland `AT_SPI_BUS` 根属性后，GTK4、Qt6、Chromium 与 X11 应用的无障碍树是否与宿主桌面零交叉；第四，私有 D-Bus 下原生 `.deb` 浏览器与 Ubuntu Firefox Snap 的实际启动表现。
- **涉及文件**：`docs/prototypes/linux-compositor-spike/`，完成后把实测结论回填至本文与 ADR 0020。
- **测试方式**：在真实 GNOME/KDE Linux 机器与 `ubuntu-24.04` 环境同时运行验证脚本。
- **主要风险**：Firefox Snap 受 AppArmor 限制拒绝连接非标准名称的私有 Wayland 套接字。若实测受阻，在 `AppLauncher` 中自动识别 `/snap/bin/*` 并给出清晰的可操作错误提示用户安装原生包，或按实测结果调整套接字命名规则。

### 阶段 1：独立 `apps/linux` 无界面守护进程 MVP 与 `crates/mcp` 能力握手

- **范围**：完全不改动 `apps/macos` 内部结构，新建 `apps/linux` 直接依赖 `crates/model` 实现 NDJSON 控制服务端与 `Display` 端口：完成 `systemd --user` 作用域管理、`createDisplay` 含 `adoptGroups` 重连、`launchApp`、`computer` 全部截图与键鼠动作、`listWindows`、`placeWindow`、`endDisplay` 以及本机回环网页预览 `preview_url`。同步升级 `crates/mcp` 实现 `agentReady` 能力握手、工具参数动态裁剪、Linux 剪贴板文案修正与守护进程后台自启，提交 ADR 0020。
- **涉及文件**：`docs/adr/0020-isolated-compositor-displays.md`、`apps/linux/Cargo.toml`、`apps/linux/src/*`、`crates/mcp/src/{lib,control,endpoint,install,tools}.rs`、`.github/workflows/ci.yml`。
- **真实路径测试**：新增 `tests/linux-e2e` 并接入 GitHub Actions `ubuntu-24.04` 任务。在模拟宿主 `sway` 旁通过标准输入输出驱动真实 `AgentScreen mcp` -> NDJSON 套接字 -> `apps/linux` -> 无头 `sway` 1.9，验证 GTK4 与 Xwayland 应用启动、点击拖拽、中文输入、`ctrl+c`/`ctrl+v` 复制粘贴、宿主焦点零干扰，以及强杀守护进程后重启通过 `adoptGroups` 找回存活显示器。
- **主要风险**：无 `systemd --user` 总线的容器环境无法调用 `systemd-run`。缓解措施是在 `DisplayManager` 启动时探测用户态 systemd，不可用时自动降级为 `setsid` 会话组加运行时状态文件，并在支持 systemd 的虚拟机或主机上验证作用域持久化。

### 阶段 2：Linux `Access` 端口、CDP 管道与剪贴板控制

- **范围**：在 `apps/linux` 中实现 `Access` 端口，基于 `atspi` 与 `zbus` 实现 `get_window_state`、`act_on_element`、`wait_for_change`、`settle` 与 `focus_secrecy`，接入 Chromium/Electron `--remote-debugging-pipe` 视口换算与网页原生拖拽，以及私有 D-Bus 内的通知静默接收器与 `wlr-data-control` 剪贴板读写原语。
- **涉及文件**：`apps/linux/src/{access_atspi,devtools_pipe,clipboard_wl,dbus_stubs}.rs`、`tests/linux-e2e/src/{ax_gtk,ax_chromium,clipboard}.rs`。
- **真实路径测试**：在 `ubuntu-24.04` CI 中实测 GTK4、Qt6 与 Chromium 的无障碍元素编号稳定性、`set_value` 回读校验、HTML5 拖拽事件以及后台运行态与宿主剪贴板互不串扰。
- **主要风险**：复杂页面首次打开时 AT-SPI 节点延迟注册。采用短退避重试并与 CDP 视口树配合。

### 阶段 3：基于双端真实重复抽离 `crates/app-core`

- **范围**：在 `apps/macos` 与 `apps/linux` 两套完整协议实现并存的前提下，对比两端的真实重复代码，新建 `crates/app-core` 抽离 NDJSON 套接字服务端、回环 `web_preview` 服务、`values::set_value` 通用循环、POSIX `devtools` 管道协议栈、元素编号缓存、会话 2 秒断连宽限管理与录制 JSONL 写入器，让双端共同收敛到 2.2 节的 `Display`、`Access`、`Shell` 三端口；同时清理 `crates/model` 中残留的 5 处 macOS 常量。
- **涉及文件**：`crates/app-core/src/*`、`crates/model/src/*`、`apps/macos/src/*`、`apps/linux/src/*`。
- **真实路径测试**：Linux 侧直接用阶段 1 与阶段 2 已建成的全套 `tests/linux-e2e` 守护零回归；macOS 侧运行 `pnpm check` 与 `AGENTSCREEN_INSTANCE=e2e cargo suite --changed` 逐项核验 `agent.rs` 与 `shell.rs` 拆分前后的行为一致性。
- **主要风险**：重构 `apps/macos/src/control/agent.rs` 时误伤 macOS 专有的共享应用窗口授权与 `InputQueue` 分支。由于此时无需强行把 `HostWindowBridge` 或 `InputQueue` 塞进 Linux，只搬移两端真正一致的模块，改动面大幅缩小且每步均有双端端到端测试兜底。

### 阶段 4：GPUI Linux 界面：Viewer、Peek、Takeover、按键隐私与托盘通知

- **范围**：抽离纯 GPUI 共享组件库 `crates/ui-common` 含 `ui/pointer.rs`、卡片与主题；在 `apps/linux` 接入 `gpui_linux` 交付 Viewer 直播窗口、`RenderImage` 帧纹理回收、ADR 0014 密码框按键隐私门控、Takeover 期间的宿主与显示器剪贴板同步、Review Display 保留存活交互、Help Card 三种回答、`ksni` 托盘与原生 GNOME 状态小窗降级、`LayerShell` 边缘 Peek 以及 `debugShow` 调试入口。
- **涉及文件**：`crates/ui-common/src/*`、`apps/linux/src/ui/*`、`apps/macos/src/ui/*`、`tests/linux-e2e/src/ui_smoke.rs`、`docs/DEVELOPMENT.md`。
- **测试边界**：**明确将 `ubuntu-24.04` CI 上的阶段 4 测试定位为冒烟测试**。云端运行器依靠 Mesa `lavapipe` 软渲染与模拟宿主 `sway`，仅能验证窗口不崩溃、`debugShow` 可达、Takeover 状态机流转与连续换帧无内存泄漏；真实 GPU 驱动帧率、GNOME Mutter 无托盘与无 `LayerShell` 降级、KDE Plasma 托盘菜单以及 Fcitx5/IBus 中文组字必须由开发者在真实 GNOME 与 KDE 硬件桌面上人工验收。
- **主要风险**：`gpui_linux` 在不同宿主合成器下的窗口焦点与 DPI 缩放差异。通过真机 GNOME 与 KDE 双环境清单逐项签收缓解。

### 阶段 5：录屏混流、发行包发布与内嵌 `Smithay` 演进

- **范围**：
  - 5a：在 `apps/linux` 的 `Display::start_recording` 下接入私有音频接收器与 `ffmpeg` 或 `GStreamer` 编码混流及 Viewer 回放；制作 `.deb` 与 `.rpm` 软件包并接入发布流水线；更新 `README.md` 与技能文档。
  - 5b：编写 `apps/linux` 专用的 `Smithay` 合成器工作进程实现，在 `Display` 端口内部替换外部 `sway` 依赖，并补充单文件 `AppImage` 发行包。
- **涉及文件**：`apps/linux/src/{recording,smithay_worker}/*`、`apps/linux/packaging/*`、`.github/workflows/release.yml`、`README.md`。
- **真实路径测试**：5a 在净室 Ubuntu 与 Fedora 容器及真机上验证安装包与录屏回放；5b 直接复用全套 `tests/linux-e2e` 对照测试 `Smithay` 工作进程。
- **主要风险**：`Smithay` 对老旧 X11 客户端边界行为兼容周期较长。5a 已可独立面向用户正式发布 Linux 版，5b 不构成首发阻塞项。

---

## 5. Windows 移植设计提纲

Windows 版在 Linux 阶段 4 稳定后启动，仅使用微软公开文档化且允许再分发的合法 API 与 MIT 开源组件。

### 5.1 显示模型两个合法选项

1. **Windows Agent Workspace 隔离会话**：Windows 11 的 [Experimental agentic features](https://support.microsoft.com/en-us/windows/ai/ai-features/experimental-agentic-features) 与 [Windows Agent Workspace 安全架构](https://blogs.windows.com/windowsexperience/2025/10/16/securing-ai-agents-on-windows/) 提供具备独立光标与输入焦点的受限智能体工作区。需注意传统 Win32 [CreateDesktopW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createdesktopw) 非活动桌面缺乏持续 DWM 合成，采集常黑屏，仅当系统暴露公开稳定的 Agent Workspace 接口时优先启用。此时其能力特征集与 Linux 类似，不声明 `host_window_handover`。
2. **IddCx 间接显示驱动**：通过微软用户态显示驱动框架 [IddCx](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview) 与 [IddCx 版本规范](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/iddcx-versions) 动态挂载虚拟显示器并固定在主屏对角线外侧，可基于 MIT 许可的 [SudoVDA](https://github.com/SudoMaker/SudoVDA) 派生定制驱动。生产分发必须使用企业 EV 证书注册微软硬件仪表盘完成 [Attestation Signing 证明签名](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation) 或 WHQL 认证。

### 5.2 采集与后台输入路由

- **画面采集**：采用 [Windows.Graphics.Capture](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture) 获取 D3D11 纹理，在 Windows 11 上将 [GraphicsCaptureSession.IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired) 设为 `false` 去除黄色边框后交由 `gpui_windows` 渲染。
- **输入限制与三层合规路由**：Windows 无按进程硬件输入注入 API；[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) 会移动系统光标并受 [Win32 前台键盘焦点模型](https://devblogs.microsoft.com/oldnewthing/20050530-11/?p=35513) 与 [Windows UAC 与 UIPI](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/architecture) 约束。因此 `IddCx` 模式下的 `WinDisplay::act_input` 在内部按三层路由消化复杂性：第一层对 Chromium、Electron 与 WebView2 应用走 CDP 管道免焦点输入；第二层对 Win32、WPF 与 WinUI 控件走 [Microsoft UI Automation](https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32) 模式接口，辅以有限子窗口 [PostMessageW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-postmessagew)；第三层在遇到自绘画布或跨窗口原生拖拽时，在 `agentReady` 中声明 `"input_queue"` 特性并复用 `InputQueue` 排队申请短暂借用真实 `SendInput`。

### 5.3 三端口与能力握手对 Windows 的适用性

第 2 节定义的 `Display`、`Access`、`Shell` 三个深层端口与 `agentReady` 能力握手无需修改即可同时表达 Windows 的两种运行模式：当运行在 `IddCx` 驱动模式时，Windows 守护进程实现 `HostWindowBridge` 并在 `agentReady` 中声明 `"host_window_handover"`、`"cross_display_move"` 与 `"input_queue"`，获得与 macOS 一致的跨屏借窗与输入队列能力；当运行在 Agent Workspace 隔离会话模式时，则声明与 Linux 相同的隔离特性集。`crates/mcp/src/endpoint.rs` 已内置 `\\.\pipe\agent-screen-control-{user}` 命名管道支持，控制层契约完全一致。

---

## 6. 待所有者决策的开放问题

1. **ADR 0020 对 Linux 失去跨屏移窗与 Review Display 保留语义的确认**：Linux 因独立合成器架构不支持宿主借窗 `Handover`、跨显示器移窗与销毁时窗口迁回主屏，审阅态选择保留窗口时保持该 Review Display 继续存活直至用户关闭窗口。是否批准以 ADR 0020 正式确立该产品语义？
   - 推荐回答：批准。这是换取任何桌面环境下绝对键鼠与焦点隔离的必然代价，通过 `agentReady` 能力握手隐藏不支持的工具后，智能体与用户心智均清晰自洽。
2. **智能体浏览器配置目录与登录态复用策略**：在缺少宿主窗口 `Give` 的前提下，智能体如何在需要账号登录的网站工作？
   - 推荐回答：`launch_app` 默认为浏览器与 Electron 应用挂载 `$XDG_STATE_HOME/agent-screen/profiles/<app-id>/` 持久化智能体配置目录并启用 `--password-store=basic`。首次遇到登录时智能体调用 `request_help`，用户在 Viewer 中 `Take Over` 登录一次，Cookie 即保存在智能体独立目录中供后续会话长期复用，且与用户宿主浏览器密码环彻底隔离。
3. **Ubuntu 默认 Firefox Snap 等沙箱包的处置口径**：若阶段 0 真机测试确认 Snap AppArmor 阻止 Firefox 连接私有 Wayland 套接字，采用何种支持策略？
   - 推荐回答：不为了迁就 Snap 沙箱而削弱私有套接字隔离。在 `launch_app` 检测到受限 Snap 应用启动失败时，返回明确错误提示引导安装原生 `.deb` 浏览器如 Chrome 或 Mozilla 官方 APT 版 Firefox。
4. **原生 GNOME 无系统托盘时的常驻形态**：当检测到宿主未安装 AppIndicator 扩展时如何维持存在感？
   - 推荐回答：存在活跃或待审阅的 Agent Display 时，自动显示极简可最小化的「Agent Screen 状态小窗」，并在设置页提示可选安装 AppIndicator 扩展。
5. **Windows 硬件驱动签名资质的启动时点**：何时启动企业 EV 证书与微软硬件仪表盘注册？
   - 推荐回答：在 Linux 阶段 2 前后并行提交资质审核，消化 2 至 6 周的外部审批周期，不占用当前 Linux 研发人力。

---

## 参考资料与主规范出处

- Wayland 核心协议与扩展：
  - `xdg-shell`：https://wayland.app/protocols/xdg-shell
  - `ext-image-copy-capture-v1`：https://wayland.app/protocols/ext-image-copy-capture-v1
  - `wlr-screencopy-unstable-v1`：https://wayland.app/protocols/wlr-screencopy-unstable-v1
  - `virtual-keyboard-unstable-v1`：https://wayland.app/protocols/virtual-keyboard-unstable-v1
  - `wlr-virtual-pointer-unstable-v1`：https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1
  - `ext-data-control-v1` 与 `wlr-data-control-unstable-v1`：https://wayland.app/protocols/ext-data-control-v1 ，https://wayland.app/protocols/wlr-data-control-unstable-v1
  - `wlr-layer-shell-unstable-v1`：https://wayland.app/protocols/wlr-layer-shell-unstable-v1
  - `wlr-foreign-toplevel-management-unstable-v1`：https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1
  - `text-input-unstable-v3`：https://wayland.app/protocols/text-input-unstable-v3
  - Xwayland 架构说明：https://wayland.freedesktop.org/docs/book/Xwayland.html
- Linux 合成器、进程作用域、无障碍与桌面规范：
  - `systemd-run` 瞬态作用域手册：https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html
  - `wlroots`、`sway` 与 Ubuntu 24.04 `sway` 1.9 包信息：https://gitlab.freedesktop.org/wlroots/wlroots ，https://github.com/swaywm/sway ，https://packages.ubuntu.com/noble/sway
  - `cage`：https://github.com/cage-kiosk/cage
  - `Smithay` 仓库与 API 文档：https://github.com/Smithay/smithay ，https://docs.rs/smithay/latest/smithay/
  - `Xvfb` 手册页：https://www.x.org/releases/current/doc/man/man1/Xvfb.1.xhtml
  - GNOME 移除 X11 会话官方技术博客：https://blogs.gnome.org/alatiera/2025/06/08/the-x11-session-removal/
  - `PipeWire` 与 `libei`：https://docs.pipewire.org/ ，https://gitlab.freedesktop.org/libinput/libei
  - `at-spi2-core` 与 `at-spi-bus-launcher.c` 源码：https://gitlab.gnome.org/GNOME/at-spi2-core ，https://gitlab.gnome.org/GNOME/at-spi2-core/-/blob/main/bus/at-spi-bus-launcher.c
  - Rust `atspi` 与 `zbus` 文档：https://docs.rs/atspi/latest/atspi/ ，https://docs.rs/zbus/latest/zbus/
  - Freedesktop Secret Service 与 XDG Desktop Portal 文档：https://specifications.freedesktop.org/secret-service/latest/ ，https://flatpak.github.io/xdg-desktop-portal/docs/
  - Freedesktop `StatusNotifierItem`、`ksni`、Notifications、Desktop Entry 与 XDG Base Directory：https://specifications.freedesktop.org/status-notifier-item/latest/ ，https://docs.rs/ksni/latest/ksni/ ，https://specifications.freedesktop.org/notification-spec/latest/ ，https://specifications.freedesktop.org/desktop-entry-spec/latest/ ，https://specifications.freedesktop.org/basedir-spec/latest/
  - Flatpak 沙箱权限与 AppImage 文档：https://docs.flatpak.org/en/latest/sandbox-permissions.html ，https://docs.appimage.org/
- GPUI 锁定提交 `c83abe7d0e060de08db386fbf86f7e95bfe6cb09` 源码：
  - 平台窗口与 `LayerShell`：https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/platform.rs
  - `surface` 元素 `CVPixelBuffer` 绑定：https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/elements/surface.rs
- Windows 官方文档与开源参考：
  - `IddCx` 间接显示驱动、版本规范与 Attestation Signing 证明签名：https://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview ，https://learn.microsoft.com/en-us/windows-hardware/drivers/display/iddcx-versions ，https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation
  - `SudoVDA` 参考实现：https://github.com/SudoMaker/SudoVDA
  - Windows 实验性智能体特性与 Agent Workspace 安全架构：https://support.microsoft.com/en-us/windows/ai/ai-features/experimental-agentic-features ，https://blogs.windows.com/windowsexperience/2025/10/16/securing-ai-agents-on-windows/
  - `CreateDesktopW`、`Windows.Graphics.Capture` 与 `IsBorderRequired`：https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createdesktopw ，https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture ，https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired
  - Microsoft UI Automation、`SendInput`、`PostMessageW`、前台键盘焦点模型与 UIPI：https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32 ，https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput ，https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-postmessagew ，https://devblogs.microsoft.com/oldnewthing/20050530-11/?p=35513 ，https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/architecture
