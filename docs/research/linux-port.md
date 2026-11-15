# Agent Screen 跨平台移植规划：先 Linux，后 Windows

结论先行：Linux 版采用「每个 Agent Display 独占一个无头 Wayland 合成器进程、配套 rootless Xwayland 与独立 D-Bus 会话总线」的隔离架构。第一阶段调用发行版自带的 `sway` 无头后端快速交付完整产品功能与自动化端到端测试。第二阶段在同一组 Rust 端口 trait 下替换为进程内嵌的 `Smithay` 合成器，实现单文件免依赖分发。Windows 版复用同一跨平台核心 crate `crates/app-core` 与同一套 NDJSON 控制协议，在支持 Windows Agent Workspace 的系统上优先采用隔离工作区，在通用 Windows 10 与 Windows 11 系统上采用经微软证明签名的 `IddCx` 间接显示驱动。

本文基于当前仓库源码、`docs/adr/` 全部架构决策记录、`docs/feasibility/` 前期可行性实验以及各平台主规范撰写，仅包含调研与架构设计，不修改产品代码。

---

## 1. macOS 能力清单与系统接缝

当前仓库包含三个主要代码单元：`crates/model` 存放无系统调用的纯领域规则，`crates/mcp` 编译 `AgentScreen mcp` 标准输入输出服务器，`apps/macos` 承载 macOS 原生能力、GPUI 界面与控制套接字服务端。下表按操作系统接缝梳理 `apps/macos`、`crates/mcp` 与 `vendor/gpui_apple` 使用的全部 macOS 专有能力。

### 1.1 虚拟显示器与排列预算

- `apps/macos/src/mac/display.rs`：通过 `objc2` 动态调用 CoreGraphics 私有类 `CGVirtualDisplay`、`CGVirtualDisplayDescriptor`、`CGVirtualDisplayMode` 与 `CGVirtualDisplaySettings` 创建虚拟显示器，注册 1x 与 2x HiDPI 显示模式。调用 `CGBeginDisplayConfiguration`、`CGConfigureDisplayOrigin` 与 `CGCompleteDisplayConfiguration` 将虚拟显示器固定在用户物理屏幕对角线外侧，避免真实光标误滑入。销毁时在后台线程轮询 `cg::is_online` 直到 WindowServer 彻底释放序列号。
- `apps/macos/src/mac/cg.rs`：调用 `CGGetOnlineDisplayList`、`CGGetActiveDisplayList`、`CGDisplayRegisterReconfigurationCallback` 监听拓扑变化。按厂商号 `0x5353` 识别 Agent Screen 自有显示器，按厂商号 `0x4145` 与型号 `1` 过滤 AeroSpace 的 `closet` 显示器，通过 `unit_twin` 检测系统镜像状态，通过 `menubar_height` 读取 macOS 屏幕顶部菜单栏高度。
- `apps/macos/src/mac/colorsync.rs` 与 `crates/model/src/budget.rs`：遵循 ADR 0009，通过 `/usr/bin/osascript` 申请管理员权限安装 `launchd` 特权守护进程 `/Library/PrivilegedHelperTools/com.fanzzzd.agentscreen.colorsync`，清理积累的陈旧虚拟显示器 ColorSync 色彩配置文件，将每次拓扑变更导致的 WindowServer 冻结控制在每台显示器 40 毫秒预算之内。

### 1.2 画面采集、单帧截图、直播渲染与录制合成

- `apps/macos/src/mac/capture.rs` 与 `apps/macos/src/ui/stream.rs`：使用 `ScreenCaptureKit` 的 `SCShareableContent`、`SCContentFilter`、`SCStream` 与 `SCStreamConfiguration` 按需采集指定 `CGDirectDisplayID` 的视频流与音频流。视频帧以 `CMSampleBuffer` 内的 `CVPixelBuffer` 保存在 `Latest` 缓存中，抽样像素计算画面变化计数供 `settle` 与 `wait_for_change` 使用。音频流开启 `excludesCurrentProcessAudio` 并通过 `AVAssetWriter` 写入 `.m4a`。
- `apps/macos/src/mac/screenshot.rs`：使用 `SCScreenshotManager` 抓取单帧并保留 `CGDisplayCreateImage` 作为降级路径，通过 `ImageIO` 的 `CGImageDestination` 编码 PNG 与 JPEG，提供缩放裁剪及点击诊断所需的区域像素采样。
- `apps/macos/src/ui/viewer.rs`、`apps/macos/src/ui/peek.rs` 与 `vendor/gpui_apple/`：Viewer 与 Peek 直接将 `CVPixelBuffer` 传入 `gpui::surface` 元素，底层通过 `CVMetalTextureCache` 零拷贝映射为 Metal 纹理。`vendor/gpui_apple/src/metal_renderer.rs` 维护了针对 Zed 上游的补丁，在独立 `Presenter` 线程提交 `CAMetalLayer` 帧以规避主线程卡顿。
- `apps/macos/src/mac/recording.rs`、`apps/macos/src/mac/mux.rs`、`apps/macos/src/mac/playback.rs` 与 `apps/macos/native/Playback.m`：使用 `AVAssetWriter` 将 `CVPixelBuffer` 硬件编码为 H.264 `.mp4` 并写出时间轴 JSONL，停止录制时用 `AVMutableComposition` 与 `AVAssetExportSession` 免重编码混流音视频轨道，回放时桥接 `AVPlayer` 与 `AVPlayerLayer`。

### 1.3 后台输入、真实输入队列与输入法借用

- `apps/macos/src/mac/input.rs`、`apps/macos/native/AgentScreenPrivate.h` 与 `apps/macos/native/SkyLight.m`：遵循 ADR 0001，每个 Agent Display 运行独立工作线程，通过 SkyLight 私有函数 `SLEventPostToPid` 直接向目标进程投递 `CGEvent`。调用 `_SLPSSetFrontProcessWithOptions` 与 `CPS` 焦点握手函数 `SSFocusWithoutRaise` 在不抬升窗口、不改变用户前台应用的前提下赋予目标窗口键盘焦点，写入 `CGEvent` 私有整数字段设置窗口局部坐标，调用 `CGEventKeyboardSetUnicodeString` 输入任意 Unicode 文本，并读取系统光标图形。
- `apps/macos/src/mac/devtools.rs`：遵循 ADR 0010，识别应用包 `Contents/Frameworks` 下的 Chromium 框架特征，在 `posix_spawn` 启动时注入 `--remote-debugging-pipe` 并挂接文件描述符 3 与 4，通过 Chrome DevTools Protocol 的 `Input.dispatchMouseEvent` 与 `Input.dispatchDragEvent` 在浏览器内部完成网页拖拽，避免改动 WindowServer 全局鼠标按键状态。
- `apps/macos/src/mac/real_input.rs` 与 `apps/macos/src/input_queue.rs`：遵循 ADR 0013 输入队列设计，针对不响应 `SLEventPostToPid` 的原生拖拽与系统控件，注册 `CGEventTap` 监听用户键鼠活动，在获得授权或空闲窗口期保存真实光标位置、调用 `CGWarpMouseCursorPosition` 瞬移至虚拟显示器投递真实 HID 事件后立即归还光标，用户一碰键鼠即刻中止。
- `apps/macos/src/mac/ime.rs`：遵循 ADR 0008，调用 Carbon 框架 `TISCopyCurrentKeyboardInputSource`、`TISCreateInputSourceList`、`TISSelectInputSource` 切换输入源，用 `CGEventSourceSecondsSinceLastEventType` 检测用户空闲满 2 秒后短暂将目标应用切为前台以驱动中文或日文输入法组字，并用 `IsSecureEventInputEnabled` 检测安全输入锁定。
- `apps/macos/src/mac/viewer_input.rs`：在 Viewer 窗口聚焦时挂载 `CGEventTap` 拦截快捷键，遵循 ADR 0014 在目标焦点处于密码框时隐去按键回显，并通过 GPUI 的 `ElementInputHandler` 对接 macOS `NSTextInputClient` 原生输入法组字条。

### 1.4 窗口列表、归属判定、放置与平铺窗口管理器协同

- `apps/macos/src/mac/windows.rs` 与 `apps/macos/src/mac/cg.rs`：调用 `CGWindowListCopyWindowInfo` 及 SkyLight 私有函数 `SLSCopyWindowsWithOptionsAndTags`、`SLSMoveWindowsToManagedSpace` 枚举窗口几何信息、层级、标题与所属 Space，并在虚拟显示器与用户屏幕之间跨屏迁移窗口。
- `apps/macos/src/mac/catch.rs`：遵循 ADR 0007，结合 `NSWorkspace` 激活通知与辅助功能观察者捕获新打开的窗口，把智能体触发的新窗口拉回对应 Agent Display，同时防止智能体冷启动应用时误抢用户刚点击 Dock 打开的窗口。
- `apps/macos/src/mac/shared_app.rs` 与 `apps/macos/src/control/shared_app.rs`：遵循 ADR 0013 窗口授权、ADR 0017 与 ADR 0018，追踪用户与多个会话共享同一应用进程时的窗口所有权、借入授权与归还位置。
- `apps/macos/src/mac/aerospace.rs` 与 `apps/macos/src/mac/claims.rs`：遵循 ADR 0006，解析 `~/.aerospace.toml` 并调用 `aerospace` 命令行维护 `ignored-monitors` 规则与 `claims.json`，阻止平铺窗口管理器把虚拟显示器上的窗口强行拉回主屏。

### 1.5 辅助功能树、元素动作与应用菜单栏代理

- `apps/macos/src/mac/ax.rs`：调用 `ApplicationServices` 的 `AXUIElementCreateApplication`、`AXUIElementCopyAttributeValue`、`AXUIElementSetAttributeValue`、`AXUIElementPerformAction` 与 `AXUIElementCopyElementAtPosition`，递归提取窗口无障碍角色、标题、值与局部包围盒，向 Electron 与 Chromium 进程写入 `AXEnhancedUserInterface` 与 `AXManualAccessibility` 激活无障碍树，并通过设置 `AXPosition` 与 `AXSize` 调整窗口位置。
- `apps/macos/src/mac/menubar.rs`：由于 macOS 在所有显示器顶部统一绘制用户当前前台应用的菜单栏，该模块通过 AX 读取虚拟显示器顶层应用自身的 `AXMenuBar` 结构，在 Viewer 顶部自绘代理菜单栏并触发菜单项的 `AXPress` 动作。
- `apps/macos/src/mac/alerts.rs` 与 `crates/model/src/alerts.rs`：按 `UserNotificationCenter`、`CoreServicesUIAgent`、`SecurityAgent` 等进程名识别落入虚拟显示器的系统权限弹窗与钥匙串弹窗。

### 1.6 应用启动与 TCC 职责剥离

- `apps/macos/src/mac/app.rs` 与 `apps/macos/src/mac/devtools.rs`：通过 `NSWorkspace` 与 `/Applications` 目录解析 `.app` 包，常规启动采用 `NSWorkspaceOpenConfiguration` 并关闭 `activates` 与 `addsToRecentItems`。需要挂载 DevTools 管道时改用 `posix_spawn` 配合 `POSIX_SPAWN_SETSID` 及 macOS 私有属性 `responsibility_spawnattrs_setdisclaim`，确保子进程向系统申请摄像头或麦克风权限时以自身名义弹窗，不继承 Agent Screen 的隐私授权。此外调用 `NSWorkspace` 设置显示器壁纸并在角落渲染显示器编号浮窗。

### 1.7 菜单栏图标、非激活悬浮面板、系统通知与权限引导

- `apps/macos/src/mac/native.rs` 与 `apps/macos/src/ui/shell.rs`：创建 `NSStatusBar` 托盘图标、手势跟踪与 `NSMenu` 原生菜单，将 Peek 与 Help Card 宿主窗口设为 `NSWindowStyleMaskNonactivatingPanel` 配合 `CanJoinAllSpaces` 与 `FullScreenAuxiliary`，保证用户悬停或点击悬浮卡片时不打断当前全屏应用或输入焦点。在无普通窗口打开时切换为 `NSApplicationActivationPolicyAccessory` 隐藏 Dock 图标，并注册全局 `NSEvent` 鼠标监听器驱动屏幕边缘唤醒。
- `apps/macos/src/mac/notify.rs`：使用 `UNUserNotificationCenter` 发送支持替换与撤回的通知，遵循 ADR 0015 注册 `Take Over`、`Continue` 动作按钮及 `UNTextInputNotificationAction` 行内文本回复。
- `apps/macos/src/mac/permissions.rs` 与 `apps/macos/src/mac/assist.rs`：调用 `CGPreflightScreenCaptureAccess`、`CGRequestScreenCaptureAccess` 与 `AXIsProcessTrustedWithOptions` 检查并引导开启屏幕录制与辅助功能权限，通过 `x-apple.systempreferences:` 打开系统设置对应页面。
- `apps/macos/src/mac/hang.rs`：后台守护线程定时探测主线程 RunLoop 响应延迟并记录卡死堆栈。

### 1.8 自动更新、打包签名与控制传输

- `apps/macos/src/mac/update.rs`、`apps/macos/build.rs` 与 `apps/macos/scripts/`：遵循 ADR 0012 链接 `Sparkle.framework` 的 `SPUStandardUpdaterController` 实现应用内自动更新，脚本完成 `.app` 组装、Developer ID `codesign` 签名、Apple 公证与 DMG 打包。
- `apps/macos/src/control/server.rs`、`apps/macos/src/web/mod.rs` 与 `crates/mcp/src/`：控制面监听 `~/Library/Application Support/Agent Screen/control.sock` Unix 域套接字，`web/mod.rs` 调用 `arc4random_buf` 生成 128 位令牌并在本机回环端口提供 MJPEG 网页预览。`crates/mcp/src/control.rs` 在套接字未就绪时调用 `/usr/bin/open -g` 后台拉起 `/Applications/Agent Screen.app`。

---

## 2. 领域设计与跨平台抽象

### 2.1 `crates/model` 现状与反向清理

`crates/model` 目前拥有 38 个纯 Rust 模块，零操作系统调用，已在 CI 的 `ubuntu-latest` 上编译与测试。以下核心领域规则已完整沉淀在 `crates/model` 中，可直接供 Linux 与 Windows 复用：

- 显示器与会话生命周期：`model.rs`、`session.rs`、`spares.rs`、`arrangement.rs`、`placement.rs`。
- 窗口所有权与共享协调：`new_window.rs`、`key_window.rs`、`shared_app.rs`、`held.rs`、`returns.rs`、`access.rs`。
- 输入编码、无障碍树编号与队列策略：`keychord.rs`、`typing.rs`、`tree.rs`、`numbers.rs`、`selection.rs`、`devtools.rs`、`input_queue.rs`、`real_click.rs`、`real_drag.rs`。
- 用户交互状态机：`request.rs`、`peek.rs`、`card.rs`、`viewer_keys.rs`、`viewer_press.rs`、`recording_playback.rs`、`web_preview.rs`、`diagnostics.rs`、`watch.rs`。

动工移植前，需对 `crates/model` 中残留的 5 处 macOS 具象假设做小幅参数化清理：

1. `alerts.rs`：当前硬编码了 `SecurityAgent` 等 macOS 守护进程名。应改为接收平台适配器提供的系统弹窗匹配规则表。
2. `budget.rs`：当前直接计算 macOS ColorSync 每台显示器 40 毫秒重排耗时。应保留通用拓扑重排预算数据结构，将 ColorSync 估算公式下移至 macOS 适配器。
3. `claims.rs` 与 `DisplayRole::from_hardware`：AeroSpace `0x4145` 过滤与 `claims.json` 格式属于 macOS 平铺窗口管理器适配细节，应归入 `crates/model::macos` 子模块或 `apps/macos`。
4. `screen_edge.rs`：MacBook 刘海屏顶部避让几何计算应标记为平台可选安全区参数。
5. `launch.rs`：在保留 `LAUNCHD` 与 `.app` 规则的同时，补充 Linux `.desktop` 启动参数清洗与环境变量过滤纯函数。

### 2.2 应在移植前从 `apps/macos` 与 `crates/mcp` 下沉的通用逻辑

当前 `apps/macos` 内部包含大量不依赖 AppKit 或 SkyLight 的协调器状态机与协议服务。在引入任何 Linux 代码前，应新建跨平台核心 crate `crates/app-core`，将以下 7 块逻辑从 `apps/macos` 原样迁入：

1. **真实输入队列调度器**：`apps/macos/src/input_queue.rs` 仅依赖 `std::time::Instant` 与 `agent_screen_model::input_queue`，且 `crates/mcp/tests/queue_adapter.rs` 已验证其可独立编译。直接迁入 `crates/app-core/src/input_queue.rs`。
2. **元素赋值状态机**：`apps/macos/src/control/values.rs` 已基于 `trait ValueAccess` 抽象了 `set_value` 的写入、回读校验与清空重输状态机。直接迁入 `crates/app-core/src/values.rs`。
3. **双语文案选择**：`apps/macos/src/i18n.rs` 中仅 `preferred_languages` 读取了 `NSLocale`，其余 `Lang`、`choose`、`t` 与 `tf!` 纯属字符串选择逻辑。迁入 `crates/app-core/src/i18n.rs`，由平台启动入口注入语言列表。
4. **NDJSON 控制服务端传输层**：`apps/macos/src/control/server.rs` 负责行分隔 JSON 编解码、请求并发派发与断连事件通知。将其迁入 `crates/app-core/src/control_server.rs`，底层接收器在 Unix 系统上绑定 `UnixListener`，在 Windows 上绑定命名管道。
5. **本机回环网页预览服务**：`apps/macos/src/web/mod.rs` 实现了 ADR 0019 的 HTTP/1.1 服务端、令牌路由、MJPEG 流式推送与 5 秒单帧保活租约。将 `arc4random_buf` 替换为跨平台 `getrandom` crate，将 JPEG 帧编码抽象为闭包回调后，整块迁入 `crates/app-core/src/web_preview.rs`。
6. **会话协调器与工具路由**：将 `apps/macos/src/core.rs`、`apps/macos/src/dismissal.rs` 与 `apps/macos/src/control/agent.rs` 中的会话连接绑定、2 秒断连宽限期 `DISCONNECT_GRACE`、每台显示器的无障碍元素编号表缓存、`settle` 帧稳定等待循环、`wait_for_change` 轮询器、结束审阅清单 `Dismissal` 计算以及录制事件 JSONL 写入器抽入 `crates/app-core`。
7. **`crates/mcp` 跨平台收尾**：`crates/mcp/src/endpoint.rs` 已实现 Linux `$XDG_RUNTIME_DIR/agent-screen/control.sock` 与 Windows 命名管道路径解析，但仍有 4 处遗留点需一并清理：`control.rs:315` 的 `start_app` 补充 Linux 后台拉起 `agent-screen` 守护进程与 Windows `CreateProcessW` 分支；`endpoint.rs:24` 补充非 `.app` 开发态二进制识别；`install.rs` 修正 `/bin/zsh` 硬编码、`USERPROFILE` 回退及第 441 行单元测试的 `#[cfg(unix)]` 条件编译；`lib.rs` 与 `tools.rs` 按目标平台生成工具说明文案。

### 2.3 核心端口设计：Trait 名称、方法与不变量

`crates/app-core` 只依赖以下 7 个最小化的平台端口 trait。所有平台复杂性封装在适配器内部，不向协调器泄漏。

#### 1. `DisplayBackend`：虚拟显示器生命周期

```rust
pub trait DisplayBackend: Send {
    fn create(&mut self, spec: DisplaySpec) -> Result<PlatformDisplay, BackendError>;
    fn resize(&mut self, id: DisplayId, size: SizePoints, hi_dpi: bool) -> Result<DisplayMetrics, BackendError>;
    fn destroy(&mut self, id: DisplayId) -> Result<(), BackendError>;
    fn host_monitors(&self) -> Vec<HostMonitor>;
}
```

- 不变量：`DisplayId` 在单台 `PlatformDisplay` 存活期内唯一且不变；`create`、`resize` 与 `destroy` 绝不改变宿主用户当前聚焦窗口，也不移动宿主真实光标；`destroy` 返回后底层合成器或系统显示设备资源必定释放或进入受控异步回收。

#### 2. `CaptureBackend`：实时帧流与单帧快照

```rust
pub trait CaptureBackend: Send + Sync {
    fn start_stream(
        &self,
        id: DisplayId,
        config: StreamConfig,
        sink: Arc<FrameSink>,
    ) -> Result<Box<dyn ActiveStream>, BackendError>;
    fn snapshot(&self, id: DisplayId, opts: SnapshotOpts) -> Result<EncodedImage, BackendError>;
}
```

- 不变量：仅采集目标 `DisplayId` 的画面与关联音频，绝不包含宿主物理显示器画面或 Agent Screen 自身界面；帧数据通过统一的 `DisplayFrame` 枚举承载，在 macOS 上包裹零拷贝 `CVPixelBuffer`，在 Linux 与 Windows 上包裹 `Arc<gpui::RenderImage>` 或共享内存 BGRA 帧及单调递增的帧序号与变化计数。

#### 3. `InputBackend`：隔离输入注入

```rust
pub trait InputBackend: Send + Sync {
    fn capabilities(&self) -> InputCapabilities;
    fn move_pointer(&self, id: DisplayId, to: Point, mods: Modifiers) -> Result<(), BackendError>;
    fn mouse_button(&self, id: DisplayId, pos: Point, btn: MouseButton, state: ElementState, count: u32, mods: Modifiers) -> Result<(), BackendError>;
    fn drag(&self, id: DisplayId, req: DragRequest) -> Result<DragOutcome, BackendError>;
    fn scroll(&self, id: DisplayId, pos: Point, delta: Vector, unit: ScrollUnit, mods: Modifiers) -> Result<(), BackendError>;
    fn type_text(&self, id: DisplayId, text: &str, opts: TypeOpts) -> Result<TypeOutcome, BackendError>;
    fn key_chord(&self, id: DisplayId, chord: KeyChord, repeat: u32) -> Result<(), BackendError>;
    fn hold_key(&self, id: DisplayId, chord: KeyChord, duration: Duration) -> Result<(), BackendError>;
    fn cursor_state(&self, id: DisplayId) -> CursorState;
}
```

- 不变量：后台输入方法绝不移动宿主会话光标、绝不抢占宿主键盘焦点、绝不向非目标显示器窗口泄漏按键。当某平台对原生拖拽或输入法组字确需借用宿主真实输入设备时，由 `capabilities().needs_host_input_queue` 声明为 `true`，统一由 `crates/app-core::InputQueue` 排队并在用户触碰键鼠时立即中断。Linux 私有合成器将该能力位声明为 `false`，所有点击与拖拽在私有席位内立即完成。

#### 4. `WindowBackend`：窗口枚举与摆位

```rust
pub trait WindowBackend: Send + Sync {
    fn capabilities(&self) -> WindowCapabilities;
    fn list_windows(&self, scope: WindowScope) -> Vec<WindowInfo>;
    fn place_window(&self, target: WindowTarget, dest: DisplayId, frame: Option<Rect>) -> Result<PlaceOutcome, BackendError>;
    fn focus_window(&self, dest: DisplayId, target: WindowTarget) -> Result<(), BackendError>;
    fn close_window(&self, target: WindowTarget) -> Result<(), BackendError>;
}
```

- 不变量：所有几何矩形均采用目标显示器左上角为原点的局部逻辑点坐标；`WindowCapabilities::cross_host_move` 明确标识平台是否支持在宿主桌面与 Agent Display 之间迁移既有窗口。在 `cross_host_move == false` 的 Linux 私有合成器后端上，针对宿主窗口的迁移请求确定性返回 `UnsupportedCapability` 错误并提示调用 `launch_app`。

#### 5. `AccessBackend`：无障碍树读取与元素动作

```rust
pub trait AccessBackend: Send + Sync {
    fn window_tree(&self, id: DisplayId, target: Option<WindowTarget>) -> Result<AccessTree, BackendError>;
    fn element_at(&self, id: DisplayId, point: Point) -> Result<Option<AccessNode>, BackendError>;
    fn perform_action(&self, node: &AccessNodeRef, op: ElementOp) -> Result<ActionOutcome, BackendError>;
}
```

- 不变量：产出的节点角色、状态位与局部逻辑坐标统一归一化为 `agent_screen_model::tree::Node`，确保 `crates/model::numbers` 的稳定编号算法与 `crates/app-core::values::set_value` 赋值状态机在三端行为一致。

#### 6. `AppLauncher`：应用解析与隔离启动

```rust
pub trait AppLauncher: Send + Sync {
    fn launch(&self, id: DisplayId, req: LaunchRequest) -> Result<LaunchedApp, BackendError>;
    fn running_apps(&self, id: DisplayId) -> Vec<AppProcess>;
    fn terminate(&self, pid: u32) -> Result<(), BackendError>;
}
```

- 不变量：被启动应用的全部窗口必定落入目标 `DisplayId`，绝不在宿主桌面抢焦点弹窗；子进程环境变量自动剥离 `AGENTSCREEN_*` 内部变量；当目标二进制属于 Chromium 或 Electron 家族时，自动按 ADR 0010 挂载私有 DevTools 管道。

#### 7. `DesktopShell`：宿主托盘、通知、权限与录屏编码

```rust
pub trait DesktopShell: Send + Sync {
    fn set_tray(&self, state: TrayState);
    fn post_notification(&self, id: DisplayId, note: Note);
    fn withdraw_notification(&self, id: DisplayId);
    fn permissions(&self) -> PermissionReport;
    fn start_recorder(&self, id: DisplayId, path: &Path, size: SizePixels) -> Result<Box<dyn VideoRecorder>, BackendError>;
}
```

- 不变量：所有托盘点击、菜单选择与通知按钮响应均转换为与平台无关的 `ShellEvent` 投递给 `app-core` 事件通道；被动弹出的 Peek 或提醒卡片绝不抢占用户当前正在输入的窗口焦点。

### 2.4 macOS 适配器映射验证

将现有 `apps/macos/src/mac/*` 映射到上述 7 个 trait 时，macOS 现有运行路径保持零行为变更：

| 端口 Trait | `apps/macos` 实现模块 | 保留的 macOS 专有机制 |
| --- | --- | --- |
| `DisplayBackend` | `mac::display`、`mac::cg`、`mac::colorsync` | `CGVirtualDisplay` 创建、备用显示器池 `spares`、对角线固定排列、ColorSync 守护进程健康检查 |
| `CaptureBackend` | `mac::capture`、`mac::screenshot` | `ScreenCaptureKit` 视频与音频采集、`CVPixelBuffer` 零拷贝透传、`SCScreenshotManager` 单帧抓取 |
| `InputBackend` | `mac::input`、`mac::devtools`、`mac::real_input`、`mac::ime` | `SLEventPostToPid` 后台投递、CDP 管道网页拖拽、`InputQueue` 门控的真实 HID 点击拖拽、Carbon 输入法借用 |
| `WindowBackend` | `mac::windows`、`mac::catch`、`mac::shared_app`、`mac::aerospace` | `CGWindowList` 枚举、SkyLight Space 迁移、新窗口归属拦截、AeroSpace `ignored-monitors` 与 `claims.json` |
| `AccessBackend` | `mac::ax`、`mac::menubar` | `AXUIElement` 递归树读取、`AXManualAccessibility` 激活、Viewer 顶部代理菜单栏读取与点击 |
| `AppLauncher` | `mac::app`、`mac::devtools` | `NSWorkspace` 启动、`posix_spawn` 配合 `responsibility_spawnattrs_setdisclaim` 剥离 TCC 归属 |
| `DesktopShell` | `mac::native`、`mac::notify`、`mac::permissions`、`mac::recording`、`mac::mux`、`mac::update` | `NSStatusItem`、非激活 `NSPanel`、`UNUserNotificationCenter` 行内回复、TCC 探测、`AVAssetWriter` 混流、Sparkle 更新 |

### 2.5 Cargo 工作区目录结构提案

```text
crates/
  model/          # 纯领域规则，零系统调用，三端共享
  app-core/       # 7 个端口 trait、会话协调器、NDJSON 服务端、Web Preview、InputQueue、多语言与录制时间轴
  mcp/            # AgentScreen mcp 二进制，三端通用
  ui-common/      # 基于 GPUI 的跨平台视图组件：Viewer 工具栏、Peek 布局、Help Card、主题与状态徽标
apps/
  macos/          # macOS 适配器实现、AppKit 原生桥接与 vendor/gpui_apple 零拷贝渲染
  linux/          # Linux 适配器实现、无头合成器管理、AT-SPI2、ksni 托盘与 gpui_linux 窗口
  windows/        # 后续 Windows 适配器实现、IddCx 或 Agent Workspace、WGC、UIA 与 gpui_windows 窗口
```

### 2.6 NDJSON 控制协议契约与 macOS 专有字段处理

ADR 0005 定义的 NDJSON 控制套接字协议继续作为 `AgentScreen mcp` 与各平台应用进程之间的唯一跨平台契约。Linux 与 Windows 使用 `crates/mcp/src/endpoint.rs` 已实现的端点路径规则。协议中现有 macOS 专有方法与字段在 Linux 上的兼容语义如下：

| 协议方法或字段 | macOS 语义 | Linux 语义 |
| --- | --- | --- |
| `type.viaInputMethod`、`type.inputSource` | 临时激活目标窗口并切换 Apple TIS 输入源如 `com.apple.inputmethod.SCIM.ITABC` 驱动前台输入法 | 私有合成器直接通过动态 XKB 键映射或 `text-input-v3` 在后台输入任意 UTF-8 字符，无需借用宿主键盘；忽略 `com.apple.*` 输入源名并在返回中标记 `borrowedHostKeyboard: false` |
| `click.realInput`、`drag.realInput`、`inputQueue` | 排队等待用户空闲并短暂借用宿主真实光标执行原生拖拽 | 私有合成器的虚拟指针本身驱动真实 `wl_seat` 按键状态与原生 Wayland/X11 拖拽，立即在显示器内部执行且无需排队；`inputQueue` 恒返回队列为空 |
| `windowAccess`、针对宿主窗口的 `placeWindow` | 将用户屏幕上的既有窗口借入 Agent Display 或移回主屏 | 因 Wayland 客户端无法跨合成器迁移连接，返回结构化错误码 `unsupported_cross_compositor_move`，引导模型使用 `launch_app` 在当前显示器内启动应用 |
| `endDisplay.adopt`、`adoptGroups` | 销毁虚拟显示器前将存活窗口移回用户主显示器 | 私有合成器关闭时其内部连接随之终止，不支持把窗口迁往宿主合成器；若传入 `adopt: true` 则返回明确提示，默认随合成器正常终止内部进程 |
| `placeWindow.frameConstrained` | 因顶部 25pt 系统菜单栏遮挡而下移窗口时返回 `true` | 无头合成器无系统顶栏遮挡，恒为 `false` |
| `getDiagnostics` 中的 `permissions`、`colorsync`、`aerospace` | 汇报 TCC 授权、ColorSync 守护进程与 AeroSpace 配置状态 | `permissions` 恒报告已就绪，`colorsync` 与 `aerospace` 置空或省略，新增可选 `linux` 诊断对象记录合成器类型、Wayland 套接字、Xwayland 显示号与宿主桌面环境 |

---

## 3. Linux 逐接缝技术设计

### 3.1 显示模型对比与选型

选型结论：为每个 Agent Display 启动一个独立的无头 Wayland 合成器实例，内部挂载 rootless Xwayland 与独立的 `dbus-daemon --session` 会话总线。MVP 阶段采用外部子进程 `sway` 无头模式，产品成熟阶段演进为 `apps/linux` 进程内嵌 `Smithay` 合成器。

四个候选方案在关键指标上的对比如下：

| 维度 | 方案 A：宿主合成器虚拟输出 | 方案 B：纯 Xvfb 虚拟 X11 | 方案 C：每显示器独立无头 `sway` + Xwayland | 方案 D：每显示器内嵌 `Smithay` + Xwayland |
| --- | --- | --- | --- | --- |
| 输入与焦点隔离 | 不合格。共享宿主 `wl_seat`，注入输入会移动用户真实光标并抢焦点 | 合格。独立 X11 服务，不影响宿主 | 合格。独立 `wl_seat` 与独立 D-Bus，物理隔离宿主键鼠焦点 | 合格。进程内独立 `smithay::wayland::seat::Seat` 与独立 D-Bus |
| 应用兼容性 | 取决于宿主合成器 | 差。现代纯 Wayland 应用异常，GNOME 50 已移除 X11 会话 | 优。原生覆盖 GTK3/4、Qt5/6、Chromium/Electron 与旧 X11 应用 | 优。实现 `xdg-shell` 与 `XWayland` 后同方案 C |
| 跨桌面普适性 | 差。Mutter、KWin、sway、Hyprland 接口各异且需弹 Portal 授权 | 通用 | 优。不依赖宿主运行 GNOME、KDE、Hyprland 还是 X11 | 优。零外部合成器二进制依赖 |
| 采集开销 | 需走 PipeWire + Portal 协商 | `XShmGetImage` 内存拷贝 | `wlr-screencopy` 脏区通知，SHM 或 DMA-BUF 读取 | 渲染器内部直出像素或导出 DMA-BUF，零跨进程协议开销 |
| Viewer 呈现路径 | PipeWire 流解码上屏 | 轮询抓帧上屏 | 合成器帧事件推入 GPUI `RenderImage` | 内存帧或 Vulkan DMA-BUF 直通 GPUI |

否决方案 A 的原因明确：GNOME Mutter 的 `RecordVirtual`、KDE KWin 虚拟输出以及 `hyprctl output create headless` 虽然能在宿主会话内添加虚拟屏幕，但 Wayland 输入注入通道如 [libei](https://gitlab.freedesktop.org/libinput/libei) 或 RemoteDesktop Portal 作用于宿主会话的同一个逻辑席位，智能体移动指针或点击会立刻夺走用户光标与活动窗口焦点，直接违背 ADR 0001 的产品根基。

否决方案 B 作为主路径的原因同样明确：[GNOME 50 发布说明](https://release.gnome.org/50/) 确认已移除 X11 会话支持，主流 Linux 桌面工具包已全面转向 Wayland 优先，纯 [Xvfb](https://www.x.org/releases/current/doc/man/man1/Xvfb.1.xhtml) 在分数倍 HiDPI 缩放与混合合成上均落后于现代客户端预期。

在基于 [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots) 的无头合成器内部对比 [cage](https://github.com/cage-kiosk/cage) 与 [sway](https://github.com/swaywm/sway)：`cage` 定位为单窗口全屏信息亭合成器，不支持多窗口浮动坐标摆位，也不暴露窗口树查询 IPC；`sway` 原生支持 `WLR_BACKENDS=headless`、`WLR_LIBINPUT_NO_DEVICES=1`、动态创建与缩放无头输出、rootless [Xwayland](https://wayland.freedesktop.org/docs/book/Xwayland.html)，并提供结构化 JSON IPC 查询窗口树与精确移动缩放浮动窗口。仓库内 `docs/feasibility/linux-spike/` 已在 Debian Trixie 容器中实测验证双 `sway` 实例间的输入、窗口与 AT-SPI 隔离。

选择「每台 Agent Display 独立一个合成器实例」而非「所有 Agent Display 共用一个多输出无头合成器」的原因在于席位隔离：Wayland 的键盘修饰键状态、鼠标按下拖拽状态与输入焦点绑定在 `wl_seat` 上。每台显示器独立实例让并发运行的多个 Agent Session 各自拥有独立 `wl_seat` 与独立 D-Bus 无障碍注册表，两路智能体同时拖拽或按住 Shift 点击互不串扰，单台显示器崩溃也不会波及其他会话。

产品终态演进至 [Smithay](https://github.com/Smithay/smithay) 的收益在于去掉用户机器安装 `sway` 的外部包依赖，并在同一进程内基于 [Smithay API](https://docs.rs/smithay/latest/smithay/) 完成离屏合成与帧读取。由于 `Smithay` 需编写约两千行 [xdg-shell](https://wayland.app/protocols/xdg-shell)、数据设备剪贴板与 `smithay::xwayland` 窗口管理胶水代码，将其放在 `sway` 跑通全套端到端测试之后替换，工程风险最低。

### 3.2 画面采集与 GPUI 渲染链路

在私有无头合成器架构下，不采用 [PipeWire](https://docs.pipewire.org/) 加 `xdg-desktop-portal` 作为画面采集通道。PipeWire 与 Portal 的设计目的是在不可信客户端向宿主桌面录屏时实施权限仲裁，而在私有合成器内部再拉起一套 PipeWire 与 WirePlumber 守护进程只会徒增进程管理负担与启动延迟。

具体采集方案分两步落地：

1. **`sway` MVP 后端**：`apps/linux` 直接作为 Wayland 客户端连入目标显示器的私有 `WAYLAND_DISPLAY` 套接字，优先绑定标准化的 [ext-image-copy-capture-v1](https://wayland.app/protocols/ext-image-copy-capture-v1)，在旧版发行版 `sway` 上自动回退至 [wlr-screencopy-unstable-v1](https://wayland.app/protocols/wlr-screencopy-unstable-v1)。订阅带脏区追踪的帧复制请求，仅当应用重绘像素时才触发共享内存或 DMA-BUF 拷贝，并同步递增 `Latest` 变化计数。单帧 `screenshot` 直接读取最新缓存帧并用 `image` crate 编码 PNG 或 JPEG。
2. **`Smithay` 内嵌后端**：使用 `GlowRenderer` 配合 EGL surfaceless 上下文离屏合成客户端表面，通过 `smithay::backend::renderer::ExportMem` 的 `copy_framebuffer` 导出像素或直接导出 DMA-BUF 文件描述符。

**GPUI Linux 画面上屏关键细节**：经核查当前仓库锁定提交中的 [GPUI surface.rs 源码](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/elements/surface.rs)，`gpui::surface` 元素硬编码接收 Apple 专有的 `core_video::pixel_buffer::CVPixelBuffer`，在 `gpui_linux` 上不可用。因此在 Linux 的 Peek 与 Viewer 中采用两阶段渲染策略：

- 第一阶段使用 GPUI 通用的 `gpui::RenderImage` 配合 `img` 元素渲染 BGRA 帧。每次替换新帧时必须显式调用 `window.drop_image` 释放上一帧纹理，防止 `blade-graphics` Vulkan 图集持续膨胀泄漏显存。在 1440x900 分辨率、每秒 8 至 30 帧下，单帧约 5.2 MB 的主机内存上传耗时低于 1 毫秒。
- 第二阶段若在 4K HiDPI 60 帧全屏 Viewer 下实测 CPU 拷贝占比超过 5%，再参照 `vendor/gpui_apple` 的模式为 `gpui_linux` 增加基于 Vulkan `VK_EXT_external_memory_dma_buf` 的零拷贝表面导入；该优化收益在实际硬件剖析前标记为尚未验证。

### 3.3 后台输入与多语言输入法

在私有合成器内注入输入不经过 `libei`，避免额外部署 EIS 服务端。

- **`sway` MVP 后端**：`apps/linux` 通过长连接 Wayland 客户端绑定 [wlr-virtual-pointer-unstable-v1](https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1) 的 `zwlr_virtual_pointer_manager_v1` 与 [virtual-keyboard-unstable-v1](https://wayland.app/protocols/virtual-keyboard-unstable-v1) 的 `zwp_virtual_keyboard_manager_v1`，完全省去每次点击派生 `wlrctl` 或 `wtype` 子进程的开销。
  - 指针操作调用 `motion_absolute`、`button`、`axis` 与 `frame`。由于事件直接进入合成器席位，悬停状态、鼠标按键按下位以及跨控件原生拖拽与 HTML5 拖拽全部天然生效。
  - 快捷键与按键保持直接发送标准 XKB 键码与修饰键掩码。
  - 任意 Unicode 文本输入参照 `wtype` 机制，为目标字符动态生成临时单键 XKB keymap 上传至 `zwp_virtual_keyboard_v1` 后击键提交，无需在私有会话内配置复杂输入法引擎即可向 GTK、Qt、Chromium 与 Xwayland 应用后台输入中英文与符号。
- **`Smithay` 内嵌后端**：直接在合成器事件循环中调用 `PointerHandle` 与 `KeyboardHandle` 注入输入事件，并直接从 `CursorImageStatus` 读取当前光标形状供 Viewer 叠加绘制。
- **Viewer 人工接管时的输入法体验**：用户在宿主桌面打开 GPUI Viewer 进行 Takeover 打字时，`gpui_linux` 的 Wayland 客户端已对接 [text-input-unstable-v3](https://wayland.app/protocols/text-input-unstable-v3)，直接唤起用户宿主桌面的 Fcitx5 或 IBus 输入法候选框。用户选词上屏后，GPUI 的 `ElementInputHandler` 拿到最终 UTF-8 字符串，经 `InputBackend::type_text` 一次性送入 Agent Display。

### 3.4 辅助功能树与元素操作

Linux 无障碍层采用标准的 [at-spi2-core](https://gitlab.gnome.org/GNOME/at-spi2-core) D-Bus 协议，通过纯 Rust 异步库 [atspi](https://docs.rs/atspi/latest/atspi/) 与 [zbus](https://docs.rs/zbus/latest/zbus/) 接入。

- **会话级总线隔离**：每台 Agent Display 启动时拉起专属的 `dbus-daemon --session --nofork --nopidfile --print-address`，当首个支持无障碍的应用连入该总线查询 `org.a11y.Bus` 时，D-Bus 自动按需激活仅服务于该显示器的 `at-spi-bus-launcher` 与 `at-spi2-registryd`。`apps/linux` 连入同一私有 D-Bus 地址，天然只看到该显示器内的应用节点，与用户宿主桌面的屏幕阅读器及窗口完全隔离。
- **工具包无障碍开关自动注入**：`AppLauncher` 启动子进程时统一注入环境变量 `GTK_MODULES=gail:atk-bridge`、`GTK_A11Y=atspi`、`QT_ACCESSIBILITY=1` 与 `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`。对 Chromium 与 Electron 应用额外追加命令行参数 `--force-renderer-accessibility` 与 `--remote-debugging-pipe`，同时启用 AT-SPI2 无障碍树与 ADR 0010 的私有 CDP 管道。
- **整树批量拉取规避 D-Bus 往返延迟**：逐节点发起 D-Bus 调用在复杂网页上会产生数百次 IPC 往返。`LinuxAccessBackend` 优先调用 `org.a11y.atspi.Cache.GetItems` 一次性批量获取应用整棵可达节点缓存，再按需对目标节点调用 `org.a11y.atspi.Component.GetExtents`、`org.a11y.atspi.Action.DoAction`、`org.a11y.atspi.Value` 与 `org.a11y.atspi.EditableText.SetTextContents`，并将结果喂入复用的 `crates/app-core::values::set_value` 校验循环。

### 3.5 窗口列表、归属与放置

在私有合成器内，凡是连入该显示器 `WAYLAND_DISPLAY` 或 `DISPLAY` 的窗口按拓扑构造即 100% 归属于该 Agent Display。macOS 上用于拦截冷启动窗口的 `catch.rs` 竞态状态机与 AeroSpace `ignored-monitors` 补丁在 Linux 上完全不再需要。

- **`sway` MVP 后端**：启动无头 `sway` 时加载内联生成的最小化配置文件，关闭状态栏 `bar { mode invisible }`，开启 `for_window [all] floating enable` 使新窗口默认采用符合传统桌面直觉的自由浮动模式而非强制平铺分割。`apps/linux` 建立到 `$SWAYSOCK` 的持久 IPC 连接，发送 `GET_TREE` 与订阅 `window` 事件，实时获取每个窗口的 `con_id`、`pid`、`app_id`、Xwayland `window_properties.class`、标题、焦点状态与局部几何框；`place_window` 通过发送 `[con_id=N] floating enable, move position X Y, resize set W H` 精确摆位；亦可辅以 [wlr-foreign-toplevel-management-unstable-v1](https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1) 跟踪顶层窗口标题与状态。
- **`Smithay` 内嵌后端**：合成器进程内的 `smithay::desktop::Space<Window>` 直接维护所有 `xdg_toplevel` 与 `X11Surface` 的层叠顺序、坐标及客户端套接字凭证 `wl_client_get_credentials` 对应的进程 PID，查询与移动均为纯内存操作。

### 3.6 应用解析与单实例隔离启动

- **应用查找规范**：遵循 [Freedesktop Desktop Entry 规范](https://specifications.freedesktop.org/desktop-entry-spec/latest/) 与 [XDG Base Directory 规范](https://specifications.freedesktop.org/basedir-spec/latest/)，按优先级扫描 `$XDG_DATA_HOME/applications` 与 `$XDG_DATA_DIRS/applications` 下的 `.desktop` 文件，支持按 Desktop ID 如 `org.gnome.TextEditor`、本地化或英文 `Name`、`StartupWMClass` 以及 `$PATH` 可执行文件名解析启动命令，并剥离 `%u`、`%F` 等占位符。
- **显示后端路由环境变量**：向子进程注入该显示器专属的 `WAYLAND_DISPLAY`、`DISPLAY`、`DBUS_SESSION_BUS_ADDRESS`，并设置 `GDK_BACKEND=wayland,x11`、`QT_QPA_PLATFORM=wayland;xcb`、`MOZ_ENABLE_WAYLAND=1` 与 `ELECTRON_OZONE_PLATFORM_HINT=auto`，确保现代应用优先走 Wayland，老旧 X11 应用透明回退至该显示器自有的 Xwayland。
- **单实例应用配置锁隔离**：`docs/feasibility/linux-spike/isolation.sh` 实测表明，基于 D-Bus 单实例激活的 GTK `GApplication` 与 KDE 应用在隔离 `DBUS_SESSION_BUS_ADDRESS` 后即可独立启动；但 Firefox、Chrome、Chromium 及多数 Electron 应用会在 `$HOME/.config` 或 `$HOME/.mozilla` 下检查磁盘 `SingletonLock` 套接字锁，若用户宿主桌面已打开同一浏览器，子进程会报错退出或把新标签页发往用户的浏览器窗口。为此 `LinuxAppLauncher` 为浏览器与 Electron 应用默认分配归属 Agent Screen 管理的持久化智能体配置目录 `$XDG_STATE_HOME/agent-screen/profiles/<app-id>/`，自动追加 `--user-data-dir` 或 `-profile ... -no-remote`，彻底切断与宿主实例的锁冲突。

### 3.7 系统托盘与非激活悬浮界面

- **托盘协议**：在宿主会话 D-Bus 上实现 [StatusNotifierItem 规范](https://specifications.freedesktop.org/status-notifier-item/latest/)，选用纯 Rust 库 [ksni](https://docs.rs/ksni/latest/ksni/)。KDE Plasma 6、XFCE、Cinnamon、COSMIC 以及预装了 AppIndicator 扩展的 Ubuntu Desktop 均原生渲染该托盘图标与右键 `dbusmenu` 菜单。
- **原生 GNOME 无托盘降级策略**：上游原生 GNOME Shell 默认不提供系统托盘宿主。`LinuxDesktopShell` 启动时查询宿主 D-Bus 上 `org.kde.StatusNotifierWatcher` 的 `IsStatusNotifierHostRegistered` 属性；若返回 `false` 且当前存在活跃或待审阅的 Agent Display，则自动打开一个紧凑的 GPUI 状态条小窗作为常驻视觉锚点与快速入口，确保后台智能体绝不处于用户无法察觉的隐身状态。
- **Peek 与 Help Card 悬浮面板**：在支持 [wlr-layer-shell-unstable-v1](https://wayland.app/protocols/wlr-layer-shell-unstable-v1) 的合成器如 KDE Plasma Wayland、sway 与 Hyprland 上，直接使用 [GPUI platform.rs](https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/platform.rs) 已内置的 `WindowKind::LayerShell` 配合 `KeyboardInteractivity::None` 创建屏幕边缘贴靠悬浮窗，并在屏幕边缘放置 1 像素透明层表面实现光标触边唤醒 Peek，全程不抢夺宿主窗口焦点。在不支持 `wlr-layer-shell` 的 GNOME Mutter Wayland 上，退化为通过托盘图标、状态小窗或系统通知点击唤出常规浮动窗口。

### 3.8 系统通知

通过 `zbus` 调用宿主会话 D-Bus 上的 [Freedesktop Desktop Notifications](https://specifications.freedesktop.org/notification-spec/latest/) 服务 `org.freedesktop.Notifications`：

- 利用 `Notify` 返回的通知 ID 与 `replaces_id` 参数实现同一显示器状态通知的原地更新，利用 `CloseNotification` 在智能体撤回 Help Request 或 Control Request 时立即消隐通知。
- 利用 `actions` 数组注册 `Open`、`Take Over` 与 `Continue` 按钮，监听 `ActionInvoked` 信号路由回 `app-core`。
- 启动时调用 `GetCapabilities` 检测宿主通知守护进程是否支持 `inline-reply`。KDE Plasma 支持通知栏直接打字回复；GNOME 等不支持行内文本回复的桌面则在点击 `Reply` 按钮时直接呼出 Viewer 内的 Help Card 输入框完成回复。

### 3.9 权限、更新、打包与沙箱边界

- **系统权限**：零系统弹窗。在 `$XDG_RUNTIME_DIR` 下创建私有 Wayland 套接字、无头合成、席位输入注入与私有 D-Bus 总线均属于普通用户态进程权限范畴，无需 root 特权、无需 `udev` `/dev/uinput` 设备权限、无需 Portal 每次点击确认，也无需像 macOS 那样重启应用使 TCC 生效。
- **打包形态对比与选型**：
  - 首选 `.deb` 与 `.rpm` 原生系统包：安装 `/usr/bin/agent-screen`、`/usr/share/applications/agent-screen.desktop` 与 `systemd --user` 服务单元，声明运行时依赖 `sway`、`xwayland`、`dbus` 与 `at-spi2-core`，可直接访问宿主系统的全部已安装原生应用与 Flatpak 应用，并随系统包管理器自动更新。
  - 次选 [AppImage](https://docs.appimage.org/)：待阶段 5b 合入进程内嵌 `Smithay` 后提供单文件免安装版本，运行在宿主命名空间内，保留完整的宿主应用启动能力，辅以启动时的 GitHub Releases 新版本提示。
  - 明确排除 Flathub 严格沙箱作为主渠道：[Flatpak 沙箱权限机制](https://docs.flatpak.org/en/latest/sandbox-permissions.html) 将应用隔离在独立的挂载命名空间内，`/usr/bin` 下看不到用户宿主安装的各类桌面软件；若要任意启动宿主软件必须申请 `org.freedesktop.Flatpak.Development` 调用 `flatpak-spawn --host`，这等同于完全穿透沙箱，不符合 Flathub 审核导向且会破坏子进程环境变量传递。

### 3.10 GPUI 的 Linux 支持现状

当前仓库 `Cargo.lock` 在提交 `c83abe7d0e060de08db386fbf86f7e95bfe6cb09` 处已完整解析了 `gpui_linux`、`gpui_platform` 与 `gpui_wgpu` 依赖树。Zed Linux 正式版长期运行在 `gpui_linux` 之上，其能力边界如下：

- **可直接复用的能力**：原生 Wayland 与 X11 双客户端后端、基于 `blade-graphics` 的 Vulkan 硬件加速渲染、`text-input-v3` 输入法协议、剪贴板、`accesskit_unix` 自身界面无障碍暴露，以及 `WindowKind::LayerShell` 与 `WindowKind::PopUp`。macOS 专用目录 `vendor/gpui_apple` 在 Linux 构建目标下完全无需参与编译。
- **需在 `apps/linux` 适配的差异点**：
  1. `gpui::surface` 仅支持 macOS `CVPixelBuffer`，Linux 需封装基于 `RenderImage` 的流式帧组件并严格回收旧帧纹理。
  2. Wayland 协议出于安全设计禁止普通客户端监听全局鼠标坐标或自行设定顶层窗口全局屏幕坐标；屏幕边缘悬停唤醒与固定位置悬浮卡片仅在支持 `wlr-layer-shell` 的合成器上提供完全对等体验，在 GNOME Mutter 上按 3.7 节降级为显式唤出。

### 3.11 Linux 相比 macOS 变简单的部分与变难的部分

**比 macOS 更简单的 6 处：**

1. **彻底免除隐私授权引导**：无屏幕录制、辅助功能、自动化三道系统授权门槛，首次安装即可零配置直接供智能体调用。
2. **后台输入与拖拽天然完备**：独立合成器拥有专属 `wl_seat`，原生支持鼠标按住状态、悬停与跨控件拖拽，省去 SkyLight 未公开 SPI 逆向维护、省去 `InputQueue` 光标瞬移借用，也省去前台输入法键盘借用。
3. **窗口归属零歧义**：连入私有套接字的窗口即属该显示器，彻底消除新窗口归属竞态与平铺窗口管理器抢窗冲突。
4. **无系统重排卡顿与特权助手**：增删十几个无头合成器不触发宿主物理屏幕黑屏重排，无需维护带 root 权限的 ColorSync 清理守护进程。
5. **无需伪造屏幕顶部菜单栏**：Linux 应用菜单位于窗口内部，画面采集直接包含菜单栏并支持直接点击，省去 `menubar.rs` 整套 AX 菜单代理。
6. **无头 CI 端到端测试原生可跑**：标准 Linux 容器或云主机无需连接物理显示器、无需人工点选 TCC 弹窗即可在每次 Pull Request 中跑完真实图形栈回归测试。

**比 macOS 更困难的 5 处：**

1. **既有窗口无法跨合成器借入与领养**：Wayland 架构决定了宿主桌面已打开的应用窗口不能动态迁移进另一个合成器，macOS 的 `Handover` 借窗与结束时的 `adopt` 领养在 Linux 上无法对等实现。
2. **单实例浏览器的磁盘锁冲突**：必须针对 Chrome、Firefox 与 Electron 应用维护独立的智能体配置目录，否则启动请求会被转发到用户的宿主浏览器窗口。
3. **宿主桌面环境分化**：GNOME Mutter 缺失原生托盘与 `wlr-layer-shell`，需要针对 `StatusNotifierWatcher` 与层表面能力做运行时特征检测与界面降级。
4. **GPUI 直播帧与录屏管线需自建**：缺少 `CVPixelBuffer` 零拷贝表面与系统内置 `AVFoundation` 硬件混流器，需自行管理 GPUI 纹理生命周期并对接 `ffmpeg` 或 `GStreamer` 完成录制合成。
5. **旧式 X11 应用缺少无障碍树**：GTK4、Qt6 与开启无障碍标志的 Chromium 暴露完整的 AT-SPI2 树，但传统纯 Xlib/X11 工具往往完全不注册 AT-SPI 节点，智能体在此类应用上必须依赖截图视觉定位与坐标点击。

---

## 4. 分阶段实施计划

全流程拆分为 6 个独立可验证的小步阶段。阶段 1 在不触碰 macOS 用户行为的前提下完成跨平台分层，阶段 2 起在真实 Linux 无头环境中逐层交付并接入 CI。

### 阶段 0：Rust 协议探针验证

- **范围**：编写独立验证程序，在单个无头 `sway` 实例上用纯 Rust 跑通 `wlr-screencopy` 帧捕获、`zwlr_virtual_pointer_v1` 点击拖拽、`zwp_virtual_keyboard_v1` 动态 keymap 中文输入，以及 `atspi` `GetItems` 批量无障碍树读取，确认所选 crate 版本与 `Cargo.lock` 无冲突。
- **涉及文件**：`docs/prototypes/linux-compositor-spike/` 下的独立验证工程，不纳入生产工作区发布构建。
- **真实路径测试**：在本地或 GitHub Actions `ubuntu-24.04` 运行器上安装 `sway`、`xwayland`、`dbus-x11`、`at-spi2-core`、`gnome-text-editor` 与 `xterm`，运行探针断言截图非黑屏、点击按钮生效、中文文本写入成功且耗时低于预期阈值。
- **风险与缓解**：部分发行版构建的 `sway` 可能尚未开启 `ext-image-copy-capture-v1`。缓解措施是在阶段 0 同时实现并优先验证覆盖率 100% 的 `wlr-screencopy-unstable-v1`。

### 阶段 1：抽离 `crates/app-core` 与 `crates/mcp` 跨平台改造

- **范围**：按第 2 节创建 `crates/app-core` 与 `crates/ui-common`，将 `input_queue`、`values`、`i18n`、NDJSON `control/server`、回环 `web` 预览服务及会话协调器从 `apps/macos` 迁出，定义 7 个后端端口 trait 并让 `apps/macos` 实现全部适配器；清理 `crates/model` 的 5 处 macOS 耦合与 `crates/mcp` 的 4 处平台遗留假设。
- **涉及文件**：`Cargo.toml`、`crates/model/src/*`、`crates/app-core/src/*`、`crates/ui-common/src/*`、`crates/mcp/src/{control,endpoint,install,lib,tools}.rs`、`apps/macos/src/{core,control,web,ui}/*`、`.github/workflows/ci.yml`。
- **真实路径测试**：CI 的 `ubuntu-latest` 任务对 `crates/model`、`crates/app-core`、`crates/mcp` 执行完整 `cargo clippy` 与 `cargo test`；macOS 上运行 `pnpm check` 与 `AGENTSCREEN_INSTANCE=e2e cargo suite --changed` 确认既有 macOS 功能零回归。
- **风险与缓解**：`apps/macos/src/ui/shell.rs` 体量较大且直接调用了多处 `crate::mac::*`。缓解措施是分两次小提交推进：第一次先迁出无 UI 依赖的 `input_queue`、`values`、`control_server` 与 `web_preview`，第二次再收拢 `shell.rs` 对端口 trait 的调用。

### 阶段 2：Linux 无界面守护进程 MVP

- **范围**：新建 `apps/linux` 二进制，交付核心工具链：`create_display`、`launch_app`、`computer` 的 `screenshot`、`left_click`、`double_click`、`right_click`、`mouse_move`、`left_click_drag`、`scroll`、`type`、`key`、`hold_key`、`cursor_position`，以及 `list_windows`、`place_window`、`end_display` 与 ADR 0019 本机网页预览 `preview_url`。此时即使尚未编写原生 Viewer 窗口，用户已可通过浏览器打开 `preview_url` 实时观看智能体操作画面。
- **涉及文件**：`apps/linux/Cargo.toml`、`apps/linux/src/{main,display_sway,capture_screencopy,input_wl,windows_sway,launcher_xdg}.rs`、`crates/mcp/src/control.rs`。
- **真实路径测试**：新增 `tests/linux-e2e` Rust 端到端测试套件并接入 `.github/workflows/ci.yml` 的 `ubuntu-24.04` 任务。测试脚本先启动一个模拟用户宿主桌面的 `sway` 实例并打开焦点哨兵窗口，再通过标准输入输出驱动真实的 `AgentScreen mcp` 二进制调用 `create_display` -> `launch_app` 启动 `foot`、`gnome-text-editor` 与 X11 `xterm` -> `computer` 输入文本与拖拽 -> `screenshot` 校验像素变化 -> 断言宿主 `sway` 哨兵窗口焦点与光标位置全程未变 -> `end_display` 断言无残留 `sway`、`Xwayland` 或 `dbus-daemon` 孤儿进程。
- **风险与缓解**：守护进程异常退出可能遗留子合成器孤儿进程。缓解措施是在 Linux 上为子进程设置 `prctl(PR_SET_PDEATHSIG, SIGTERM)` 并将其放入独立进程组，启动时扫描清理陈旧运行时目录。

### 阶段 3：AT-SPI2 无障碍树、元素操作与 Chromium DevTools 管道

- **范围**：实现 `LinuxAccessBackend` 对接私有 D-Bus 上的 AT-SPI2 `Cache.GetItems`、`Component`、`Action`、`Value` 与 `EditableText`，交付 `get_window_state`、`act_on_element`、`wait_for_change` 与 `settle`；将 `apps/macos/src/mac/devtools.rs` 中的 POSIX 双管道与 CDP 协议栈抽入 `crates/app-core` 的 Unix 共享模块，使 Linux 启动 Chromium 与 Electron 应用时自动获得 `--remote-debugging-pipe` 网页视口坐标换算与 HTML5 原生拖拽支持。
- **涉及文件**：`apps/linux/src/access_atspi.rs`、`crates/app-core/src/devtools_pipe.rs`、`apps/macos/src/mac/devtools.rs`、`tests/linux-e2e/src/{ax_gtk,ax_chromium}.rs`。
- **真实路径测试**：在 `ubuntu-24.04` CI 中运行 `ax_gtk` 与 `ax_chromium` 端到端用例，验证 `get_window_state` 返回带稳定元素编号的按钮与输入框，验证 `act_on_element` 的 `click`、`focus`、`set_value` 在 GTK4 与无沙箱或用户命名空间 Chromium 中准确生效，验证 HTML5 拖拽测试页触发 `drop` 事件。
- **风险与缓解**：部分应用的 AT-SPI 树在首次启动后延迟数百毫秒才注册到总线。缓解措施是在 `window_tree` 首轮发现树为空但窗口已存在时执行短暂退避重试，与 macOS 上等待 Electron 构建 AX 树的策略保持一致。

### 阶段 4：GPUI Linux 原生界面：Viewer、Peek、Takeover 与托盘通知

- **范围**：在 `apps/linux` 中接入 `gpui_linux` 与 `crates/ui-common`，交付原生 Viewer 直播窗口、鼠标键盘与宿主输入法透传、Takeover 接管与归还状态机、Help Request 四种回答交互、`ksni` 系统托盘与原生 GNOME 状态小窗降级、`org.freedesktop.Notifications` 系统通知，以及 wlroots/KDE 下的 `LayerShell` 边缘 Peek 卡片。同步在 Linux 端实现全部 `debugShow` 与 `debugTakeover` 调试控制方法。
- **涉及文件**：`apps/linux/src/{shell,tray_sni,notify_dbus,viewer_surface}.rs`、`crates/ui-common/src/*`、`tests/linux-e2e/src/{viewer,takeover,help_request}.rs`、`docs/DEVELOPMENT.md`。
- **真实路径测试**：在 CI 的模拟宿主 `sway` 会话内启动带界面的 `agent-screen`，通过 `debugShow` 与 `zwlr_virtual_pointer_v1` 驱动宿主桌面上的 Viewer 与 Help Card，端到端验证 Takeover 期间智能体输入被拦截、用户点击归还后 `wait_for_control` 立即恢复、通知动作按钮正确应答 Help Request，并监测连续播放 500 帧后进程内存平稳无纹理泄漏。
- **风险与缓解**：GitHub Actions 标准云主机无硬件 GPU，Vulkan 走 Mesa `lavapipe` 软渲染。缓解措施是在 CI 安装 `mesa-vulkan-drivers` 跑通软渲染验证，并在至少一台物理 Intel/AMD Wayland 机器上人工复核帧率与混合圆角表现。

### 阶段 5：录屏回放、发行打包与内嵌 `Smithay` 演进

- **范围**：
  - 5a：对接私有 PulseAudio/PipeWire 虚拟声卡或静音视频流，通过 `ffmpeg` 子进程管道或 `GStreamer` 完成 H.264 `.mp4` 录制、音画混流与回放界面；编写 `.deb` 与 `.rpm` 打包脚本并接入 `.github/workflows/release.yml`；更新 `README.md` 与 `docs/agents/AGENTS.md`。
  - 5b：编写 `apps/linux/src/compositor_smithay/`，在 `DisplayBackend`、`CaptureBackend`、`InputBackend` 与 `WindowBackend` 背后用进程内嵌 `Smithay` + `XWayland` 替代外部 `sway` 进程，随后产出单文件 `AppImage`。
- **涉及文件**：`apps/linux/src/{recording,compositor_smithay}/*`、`apps/linux/packaging/*`、`.github/workflows/release.yml`、`README.md`。
- **真实路径测试**：阶段 2 至阶段 4 积累的全部 `tests/linux-e2e` 用例在不改一行断言的前提下直接切换至 `Smithay` 后端跑通，并增加 `.deb` 净室容器安装自检。
- **风险与缓解**：`Smithay` 对复杂 Xwayland 客户端的剪贴板与焦点边缘行为耗时可能超出预期。采用 5a 先以 `sway` 后端发布可用版本、5b 独立分支推进 `Smithay` 的解耦策略，使 `Smithay` 研发周期绝不阻塞 Linux 首版上线。

---

## 5. Windows 移植设计提纲

Windows 版在 Linux 完成阶段 4 后启动，严格限定仅使用微软公开文档化且允许再分发的合法 API 与开源组件，排除任何系统文件补丁、未公开内核钩子或灰产远控注入手法。

### 5.1 显示模型两个合法选项

1. **Windows Agent Workspace 隔离会话**：Windows 11 引入的 [Experimental agentic features](https://support.microsoft.com/en-us/windows/ai/ai-features/experimental-agentic-features) 与 [Windows Agent Workspace 安全架构](https://blogs.windows.com/windowsexperience/2025/10/16/securing-ai-agents-on-windows/) 为智能体创建与主桌面隔离的受限后台运行桌面，具备独立的输入焦点与光标上下文，在其中调用标准输入 API 不会干扰用户主屏幕。需注意传统的 Win32 [CreateDesktopW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createdesktopw) 在未切换为活动桌面时缺乏持续的 DWM 硬件合成，配合现代捕获接口常出现黑帧或初始化失败，因此只有当系统公开支持具备完整合成能力的 Agent Workspace 接口时才走独立工作区路径。
2. **IddCx 间接显示驱动**：通过微软官方的用户态显示驱动框架 [IddCx](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview) 与对应 [IddCx 版本规范](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/iddcx-versions) 在当前用户会话中按需挂载虚拟显示器，并用 `SetDisplayConfig` 将其固定在主屏幕对角线外侧。代码可基于 MIT 许可证开源的 [SudoVDA](https://github.com/SudoMaker/SudoVDA) 派生定制 IOCTL 接口。**驱动签名硬性门槛**：在开启安全启动的现代 Windows 10 与 Windows 11 上加载发布版驱动，组织必须持有硬件级 EV 代码签名证书注册 Microsoft Partner Center 硬件仪表盘，提交驱动包完成微软 [Attestation Signing 证明签名](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation) 或 WHQL 认证。

选型结论：运行时优先探测系统是否提供公开可用的 Agent Workspace 接口；在更广泛的 Windows 10 与 Windows 11 生产环境中，采用自有证明签名的 `IddCx` 驱动方案。

### 5.2 采集与后台输入方案

- **画面采集**：采用 [Windows.Graphics.Capture](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture) 采集虚拟显示器的 Direct3D 11 纹理，在 Windows 11 上将 [GraphicsCaptureSession.IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired) 设为 `false` 关闭黄色录屏边框，并将 D3D11 纹理交给 `gpui_windows` 的 DirectX 渲染管线。
- **后台输入的平台限制**：Windows 不存在类似 macOS `SLEventPostToPid` 的按进程硬件输入投递接口。[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) 总是作用于当前活动输入桌面，会直接移动用户的系统鼠标指针，且按 [Win32 前台键盘焦点模型](https://devblogs.microsoft.com/oldnewthing/20050530-11/?p=35513) 将键盘消息严格发往当前前台线程焦点窗口。此外，向高完整性级别进程发送输入受 [Windows UAC 与 UIPI](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/architecture) 机制拦截。
- **`IddCx` 模式下的三层合规输入路由**：
  1. **Chromium、Electron 与 WebView2 应用**：与 macOS 和 Linux 一样在启动时挂载 CDP 调试通道，通过 `Input.dispatchMouseEvent`、`Input.dispatchKeyEvent` 与 `Input.dispatchDragEvent` 在渲染进程内直接完成点击、打字与拖拽，不触碰系统光标与前台焦点，覆盖主流浏览器、VS Code、Slack 等高频工具。
  2. **原生 Win32、WPF 与 WinUI 控件**：通过 [Microsoft UI Automation](https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32) 的 `InvokePattern`、`ValuePattern`、`TogglePattern`、`SelectionItemPattern` 与 `ScrollPattern` 在后台直接触发控件动作与赋值；对极少数传统 Win32 子窗口辅以 [PostMessageW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-postmessagew) 投递窗口消息作为补充，但不依赖其处理复杂组合键或现代框架状态。
  3. **自定义自绘画布与跨窗口拖拽**：直接启用 `crates/app-core` 中现成的 `InputQueue` 用户授权输入队列。`WinInputBackend` 将 `capabilities().needs_host_input_queue` 设为 `true`，当且仅当 CDP 与 UIA 无法覆盖目标操作时，进入队列申请短暂借用真实 `SendInput` 并保存归还光标位置，用户一动键鼠即刻中止。而在 Agent Workspace 隔离会话模式下，会话内 `SendInput` 直接无排队执行。

### 5.3 第 2 节抽象接缝对 Windows 的适用性核查

第 2 节设计的 `crates/model`、`crates/app-core`、`crates/mcp`、NDJSON 控制协议以及全部 7 个端口 trait 签名对 Windows 保持 100% 原样适用，无需修改核心协调器接口：

| 组件或端口 | Windows 复用与适配情况 |
| --- | --- |
| NDJSON 控制传输与 `crates/mcp` | 原样复用。`crates/mcp/src/endpoint.rs` 已实现 `\\.\pipe\agent-screen-control-{user}` 命名管道路径解析 |
| `crates/app-core::InputQueue` | 原样复用。本来为 macOS 原生拖拽抽离的输入借用队列，恰好解决 Windows `IddCx` 模式下非 CDP/UIA 画布的输入借用问题 |
| `DisplayBackend` | 封装 `SudoVDA` 派生驱动的 `DeviceIoControl` 增删显示器调用，或封装 Agent Workspace 会话创建 |
| `CaptureBackend` | 封装 `Windows.Graphics.Capture` 帧池回调与 WIC/JPEG 编码 |
| `InputBackend` | 内部封装「CDP 优先 -> UIA 模式次之 -> `InputQueue` 门控 `SendInput` 兜底」路由，对 `app-core` 呈现完全相同的 trait 方法 |
| `WindowBackend` | `IddCx` 模式下用 `EnumWindows` 与 `SetWindowPos` 实现，`cross_host_move` 为 `true`，支持与 macOS 相同的跨屏借窗与领养；Agent Workspace 模式下 `cross_host_move` 为 `false` |
| `AccessBackend` | 封装 Windows `IUIAutomation` COM 接口，借助 `BuildUpdatedCache` 批量拉取节点树并归一化为 `tree::Node` |
| `AppLauncher` | 解析开始菜单 `.lnk`、AppUserModelId 与注册表 `App Paths`，用 `CreateProcessW` 配合 `STARTUPINFOW` 或 CDP 参数启动 |
| `DesktopShell` 与 GPUI | `gpui_windows` 已在 `Cargo.lock` 中就绪；托盘使用 `Shell_NotifyIconW`，非激活 Peek 面板使用 `WS_EX_NOACTIVATE` 与 `WS_EX_TOPMOST`，通知使用 Windows Toast XML API |

---

## 6. 待所有者决策的开放问题

以下 6 项决策涉及产品语义取舍或外部资质流程，每项均附推荐方案：

1. **Linux 首版合成器技术栈切入点**：是按阶段 2 先用外部 `sway` 无头进程交付完整功能、阶段 5b 再替换为内嵌 `Smithay`，还是从阶段 2 起直接投入编写内嵌 `Smithay` 合成器？
   - 推荐回答：先用外部 `sway` 交付阶段 2 至阶段 4，阶段 5b 再切换为内嵌 `Smithay`。这样在两周内即可让智能体与端到端 CI 跑在真实 Linux 显示器上，且阶段 2 至阶段 4 沉淀的全部测试可直接充当 `Smithay` 替换时的安全网。
2. **Linux 上宿主窗口借入与领养的协议表现**：面对 Wayland 无法跨合成器迁移既有宿主窗口的客观限制，`windowAccess` 工具与 `end_display` 的 `adopt` 参数应如何在 MCP 层面对智能体暴露？
   - 推荐回答：保留统一的 MCP 工具 schema 不做按平台删减，在 Linux 会话的 `agentReady` 引导文案中明确告知模型「Linux 显示器仅运行由 `launch_app` 在其内部启动的应用」，并在调用宿主窗口迁移或 `adopt: true` 时返回清晰的结构化错误与改用 `launch_app` 的提示。
3. **浏览器与 Electron 应用的默认配置目录策略**：Linux 版 `launch_app` 启动 Chrome、Firefox 或 Electron 应用时，是默认始终挂载归属 Agent Screen 的独立持久化配置目录，还是仅在检测到用户默认配置锁被占用时才回退到独立目录？
   - 推荐回答：默认始终使用归属 Agent Screen 的独立持久化配置目录 `$XDG_STATE_HOME/agent-screen/profiles/<app-id>/`，同时为 `launch_app` 提供可选参数允许用户显式指定自定义目录。这样既能彻底杜绝窗口误开到用户宿主浏览器，又能保护用户个人浏览器 Cookie 与会话安全，且智能体需要的站点登录态只需通过 Takeover 登录一次即可持久保留。
4. **原生 GNOME 桌面缺少系统托盘时的常驻形态**：当检测到宿主运行未安装 AppIndicator 扩展的原生 GNOME 时，如何呈现运行中的 Agent Display 状态？
   - 推荐回答：只要存在活跃或待审阅的 Agent Display，自动展示一个极简可最小化的「Agent Screen 状态小窗」，列出当前活动显示器数量与一键打开 Viewer 按钮，并在首次设置页提示可选安装 AppIndicator 扩展以获得顶栏托盘体验。
5. **Linux 首批官方发行包范围**：首版发布维护哪几种安装包格式与目标发行版？
   - 推荐回答：首版官方构建聚焦 Ubuntu 24.04 LTS / Debian 13 的 `.deb` 包与 Fedora 41+ 的 `.rpm` 包；待阶段 5b 合入内嵌 `Smithay` 后增加通用 `AppImage`；不发布受沙箱限制无法直接拉起宿主软件的 Flathub 包。
6. **Windows 硬件驱动签名资质的启动时点**：何时启动 EV 代码签名证书采购与 Microsoft Partner Center 硬件仪表盘企业认证？
   - 推荐回答：在 Linux 阶段 1 完成后即并行提交企业资质审核。硬件仪表盘与 EV 证书审核通常耗时 2 至 6 周，提前办妥证书可避免日后阻塞 Windows `IddCx` 驱动实机测试，而工程研发精力仍集中于先彻底做完 Linux 版。

---

## 参考资料与主规范出处

- Wayland 核心协议与扩展：
  - `xdg-shell`：https://wayland.app/protocols/xdg-shell
  - `ext-image-copy-capture-v1`：https://wayland.app/protocols/ext-image-copy-capture-v1
  - `wlr-screencopy-unstable-v1`：https://wayland.app/protocols/wlr-screencopy-unstable-v1
  - `virtual-keyboard-unstable-v1`：https://wayland.app/protocols/virtual-keyboard-unstable-v1
  - `wlr-virtual-pointer-unstable-v1`：https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1
  - `wlr-layer-shell-unstable-v1`：https://wayland.app/protocols/wlr-layer-shell-unstable-v1
  - `wlr-foreign-toplevel-management-unstable-v1`：https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1
  - `text-input-unstable-v3`：https://wayland.app/protocols/text-input-unstable-v3
  - Xwayland 架构说明：https://wayland.freedesktop.org/docs/book/Xwayland.html
- Linux 合成器、无障碍与桌面规范：
  - `wlroots`：https://gitlab.freedesktop.org/wlroots/wlroots
  - `sway`：https://github.com/swaywm/sway
  - `cage`：https://github.com/cage-kiosk/cage
  - `Smithay` 仓库与 API 文档：https://github.com/Smithay/smithay ，https://docs.rs/smithay/latest/smithay/
  - `Xvfb` 手册页：https://www.x.org/releases/current/doc/man/man1/Xvfb.1.xhtml
  - GNOME 50 移除 X11 会话发布说明：https://release.gnome.org/50/
  - `PipeWire` 文档：https://docs.pipewire.org/
  - `libei` 模拟输入库：https://gitlab.freedesktop.org/libinput/libei
  - `at-spi2-core` 规范与实现：https://gitlab.gnome.org/GNOME/at-spi2-core
  - Rust `atspi` 与 `zbus` 文档：https://docs.rs/atspi/latest/atspi/ ，https://docs.rs/zbus/latest/zbus/
  - Freedesktop `StatusNotifierItem` 规范与 `ksni` 库：https://specifications.freedesktop.org/status-notifier-item/latest/ ，https://docs.rs/ksni/latest/ksni/
  - Freedesktop Desktop Notifications 规范：https://specifications.freedesktop.org/notification-spec/latest/
  - Freedesktop Desktop Entry 规范：https://specifications.freedesktop.org/desktop-entry-spec/latest/
  - Freedesktop XDG Base Directory 规范：https://specifications.freedesktop.org/basedir-spec/latest/
  - Flatpak 沙箱权限文档：https://docs.flatpak.org/en/latest/sandbox-permissions.html
  - AppImage 官方文档：https://docs.appimage.org/
- GPUI 仓库锁定提交 `c83abe7d0e060de08db386fbf86f7e95bfe6cb09` 源码：
  - 平台窗口与 `LayerShell` 定义：https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/platform.rs
  - `surface` 元素 `CVPixelBuffer` 绑定：https://github.com/zed-industries/zed/blob/c83abe7d0e060de08db386fbf86f7e95bfe6cb09/crates/gpui/src/elements/surface.rs
- Windows 官方文档与开源参考：
  - `IddCx` 间接显示驱动概述与版本：https://learn.microsoft.com/en-us/windows-hardware/drivers/display/indirect-display-driver-model-overview ，https://learn.microsoft.com/en-us/windows-hardware/drivers/display/iddcx-versions
  - Windows 驱动 Attestation Signing 证明签名：https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation
  - `SudoVDA` 虚拟显示驱动参考实现：https://github.com/SudoMaker/SudoVDA
  - Windows 实验性智能体特性与 Agent Workspace 安全架构：https://support.microsoft.com/en-us/windows/ai/ai-features/experimental-agentic-features ，https://blogs.windows.com/windowsexperience/2025/10/16/securing-ai-agents-on-windows/
  - `CreateDesktopW` 文档：https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createdesktopw
  - `Windows.Graphics.Capture` 与无边框属性：https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture ，https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired
  - Microsoft UI Automation 文档：https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32
  - `SendInput`、`PostMessageW`、前台键盘焦点模型与 UIPI：https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput ，https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-postmessagew ，https://devblogs.microsoft.com/oldnewthing/20050530-11/?p=35513 ，https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/architecture
