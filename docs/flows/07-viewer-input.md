# 操作 07：Viewer 用户输入、标题栏抓取、输入法与边界拖拽

当用户在 Agent Screen 的 `Viewer` 窗口中直接点击、拖拽、滚动或打字时，系统在不移动代理屏上真实硬件光标的前提下，将用户交互实时映射到虚拟显示器上的目标窗口。该链路同时负责保护第三方应用弹出层不被全局监听器误关、接管标准标题栏移动、钳制越界拖拽以及驱动系统中文输入法。

## 端到端流程

```mermaid
flowchart TD
    U["用户在 Viewer 画布按下鼠标或键盘"] --> TP["private-press 线程主动式 HIDEventTap (viewer_input.rs : 1815)]
    TP --> TP1{"命中的是否为无遮挡且无展开菜单的 Viewer PICTURE 画布?"}
    TP1 -- "否" --> OS["原样放行给 WindowServer 与 GPUI 常规 UI"]
    TP1 -- "是" --> TP2["向 WindowServer 返回 NULL 吞掉全局广播 (ADR 0014), 期间按键暂存 KeyHold <= 100ms"]

    TP2 --> LM["主线程 NSEvent 本地监听器保留精确滚轮相位与修饰键"]
    LM --> MAP["viewer_press : ：map_point 校验 CGDisplayBounds 非 (0,0,0,0) 并换算代理屏坐标]
    MAP --> HIT["命中代理屏顶层目标窗口, 计算覆盖率决定激活时序"]

    HIT --> FR{"目标窗覆盖同 PID 下层窗比例 <= 60%? (#41 临时浮层)"}
    FR -- "是: Front::AfterRelease" --> FA["先向目标 PID 发送按下与抬起, 释放后再 make_key + SSMakeSelfFront"]
    FR -- "否: Front::BeforePress" --> FB["先 make_key 再 SSMakeSelfFront(0x200) 激活自身 NSTextInputContext"]

    FB --> TB{"左键是否落在标准标题栏可拖区? (ax : ：on_title_bar <= 52pt, 0.15s 超时)}
    TB -- "是" --> GRAB["Worker : ：grab 接管拖拽： dragged_window_origin 写 AXPosition, 限制超大窗中心留在屏内]
    TB -- "否" --> SKY["SSFocusWithoutRaise + SSPostEventToPid 发送指针序列; 越出画布时 clamp 到 [0.0, 1.0]"]
    SKY --> GUARD["注册 10s ViewerDragGuard 暂停 guard_placed, 结束按 [0,40,150,350,700]ms 执行 keep_on_display"]

    U --> KEY["用户在 Viewer 敲击键盘"]
    KEY --> IME{"当前输入源是否为 InputMethod 且焦点保密性 Secrecy : ：Unknown?}
    IME -- "是 (#117)" --> HOLD["暂存按键与 Shift/CapsLock <= 400ms, Probe 线程轮询 ax : ：text_focus]
    HOLD --> SEC{"探测结果"}
    SEC -- "Secrecy::Secure (密码框)" --> RAW["丢弃已产生的预编辑串, 将 COMPOSED_KEYS 按原始键码重放至目标 PID"]
    SEC -- "Secrecy::Open (普通文本)" --> NST["送入 GPUI NSTextInputContext, IMP swizzle 捕获 doCommandBySelector:, 候选窗定位至 caret_in_window"]
    IME -- "否 (纯英文键盘或已知状态)" --> DIRECT["直接按 Secrecy 分流至 NSTextInputContext 或原始 SkyLight 键盘投递"]
```

## 节点明细表

| 节点 | 我方代码 | macOS 组件与内部行为 | 限制与故障模式 | 关联问题 | 验证状态 |
| --- | --- | --- | --- | --- | --- |
| 1. 前置主动式 `private-press` 事件拦截（ADR 0014） | `apps/macos/src/mac/viewer_input.rs:1815-2052` | 独立 `"private-press"` 线程运行 `CGEventTapCreate(kCGHIDEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault, ...)`。当物理点击落在未被遮挡且无菜单展开的 `PICTURE` 画布内时，回调向 `WindowServer` 返回 `NULL` 并把点击转交主线程内部消化；同时通过 `KeyHold` 将紧随其后的硬件按键暂缓最多 `KEY_WAIT = 100 ms`，保证先点后敲的时序不反转。 | 若不拦截物理点击，`WindowServer` 会向所有注册了 `NSEvent.addGlobalMonitorForEventsMatchingMask` 的后台应用广播"点击了 Agent Screen"，导致企微、Qt、Electron 等自绘浮层在收到转发点击前先一步自毁关闭。若无输入监控或辅助功能权限，Tap 创建失败回退普通点击。 | `#41`、ADR 0014 | `[已验证]` `apps/macos/src/mac/viewer_input.rs:1815-2052`、`docs/adr/0014-viewer-popup-clicks.md` |
| 2. 本地 `NSEvent` 监听与触控板保真 | `apps/macos/src/mac/viewer_input.rs:2133-2161` | 调用 `NSEvent.addLocalMonitorForEventsMatchingMask` 在 GPUI 抽象层改写事件前截获原始 `NSEvent`，完整保留双指滚动像素增量、`phase`、`momentumPhase`、Ctrl 单击以及全部修饰键位。 | GPUI 自身事件模型会丢失部分 macOS 原生滚动惯性相位，绕过它可以避免目标网页或长列表出现阶跃式卡顿滚动。 | Viewer 滚动手感保真 | `[已验证]` `apps/macos/src/mac/viewer_input.rs:2133-2161` |
| 3. 浮层延迟前置策略（`Front::AfterRelease`） | `crates/model/src/viewer_press.rs:64-138`、`apps/macos/src/mac/viewer_input.rs:1410-1460` | 比较被点中窗口与同 PID 下层窗口的重叠面积：若被点窗口覆盖下层窗口不足 `60%`（典型如搜索下拉框、自动补全浮层），推迟至鼠标抬起后再把 Viewer 设为 key 并调用 `SSMakeSelfFront`；常规大窗口则采用 `Front::BeforePress`，且必须先执行 `make_key` 再调 `_SLPSSetFrontProcessWithOptions(&own, wid, 0x200)`（`SkyLight.m:164-172`），确保 AppKit 正确激活输入法上下文。 | 若对搜索浮层采用 `BeforePress`，Agent Screen 抢前台的瞬间部分应用会收起非模态子窗口；若颠倒 `make_key` 与 `SSMakeSelfFront` 顺序，AppKit 不会激活当前视图的 `NSTextInputContext`，中文输入法失效。 | `#41`、`#117` | `[已验证]` `crates/model/src/viewer_press.rs:64-138`、`apps/macos/src/mac/viewer_input.rs:1437-1445` |
| 4. 标准标题栏识别与 `Grab` 搬移 | `apps/macos/src/mac/ax.rs`（`on_title_bar`）、`apps/macos/src/mac/viewer_input.rs:1462-1540`、`crates/model/src/viewer_press.rs:140-228` | 左键按下位于窗口顶部 `52 pt` 内时，设 `0.15 s` AX 超时探测命中元素：排除关闭/最小化/缩放按钮与普通标签组（仅允许嵌套在 `AXToolbar` 内的 `AXTabGroup` 拖拽）。命中后进入 `Worker::grab`，每次鼠标移动由 `dragged_window_origin` 计算新窗口左上角并写入 `AXPosition`，强制超大窗口的中心点留在代理屏内；若拖拽途中发生 `ScreensChanged` 重停放，自动按新屏幕原点重算基线。 | **根本原因**：AppKit 原生标题栏拖拽内部调用 `-[NSWindow performWindowDragWithEvent:]`，直接读取硬件物理光标位移而非 `CGEvent` 坐标，纯 SkyLight 转发完全拖不动原生标题栏，必须由 `Worker::grab` 改写 `AXPosition`。 | `Viewer 拖拽越界/失步`、`#204` | `[已验证]` `AppKit` 反汇编 `performWindowDragWithEvent:` (`0x185673490`)、`viewer_press.rs:140-228` |
| 5. `CustomDrag` 自绘标题栏首按吞没与硬件锚定限制 | `apps/macos/src/mac/viewer_input.rs:1410-1495`、`docs/research/root-causes-2026-11.md:122` | 对无标准空标题栏区（`wake_at == None`）的自绘标题栏窗口（如 `NSWindowStyleMaskFullSizeContentView` 或 Electron `-webkit-app-region: drag`），直接通过 `SSFocusWithoutRaise` + `SSPostEventToPid` 发送 `LeftMouseDown` 与 `LeftMouseDragged`。 | **两重已知限制**：1. **首按吞没**：目标进程处于后台且该窗口尚未成为进程内真 `keyWindow` 时，若自绘视图未重写 `-acceptsFirstMouse:`（`NSView.h:225` 默认返回 `NO`），AppKit 的非活动窗口点击门控会把第一下 `LeftMouseDown` 仅用于激活窗口而吞掉事件，用户必须拖第二次才生效；2. **硬件光标锚定**：若自绘视图收到事件后调用 `-[NSWindow performWindowDragWithEvent:]`（`NSWindow.h:661`），内部 `_dragWindowRelativeToMouseDown:options:`（`0x185673628`）仍绑定用户在主屏上的真实光标位移，导致代理屏内窗口按错误参考系跳动或纹丝不动。 | `CustomDrag 首按吞没`、`Viewer 拖拽越界/失步` | `[已验证]` `NSView.h:225`、`NSWindow.h:661`、`AppKit` 反汇编 `0x185673490..0x185673628`、`root-causes-2026-11.md:122` |
| 6. 画布越界钳制与拖后守卫（`keep_on_display`） | `crates/model/src/viewer_press.rs:64-115`、`apps/macos/src/mac/viewer_input.rs:1510-1565` | 拖拽与抬起阶段设 `clamp = true`，将越出 Viewer 图像四周的坐标钳制在归一化区间 `[0.0, 1.0]` 内，并在 `CGDisplayBounds` 返回离线 `(0,0,0,0)` 时拒绝映射。非 `Grab` 拖拽启动 `10 s` `ViewerDragGuard` 暂停 `guard_placed`，松手后在 `[0, 40, 150, 350, 700] ms` 五个时间点运行 `keep_on_display`，将因应用惯性或松手重排滑出代理屏的窗口拉回屏内。 | 若不钳制坐标，用户鼠标移出小尺寸 Viewer 窗口时会把事件投递到虚拟屏外空白区甚至相邻屏幕；若不设 `ViewerDragGuard`，`guard_placed` 会在用户拖拽中途跟手抢写 `AXPosition` 造成窗口抖动。 | `Viewer 拖拽越界/失步`、`#205` | `[已验证]` `crates/model/src/viewer_press.rs:64-115`、`apps/macos/src/mac/viewer_input.rs:1510-1565` |
| 7. 输入法暂存探测与密码框保护（`viewer_keys`） | `crates/model/src/viewer_keys.rs:1-260`、`apps/macos/src/mac/viewer_input.rs:430-966` | 当用户使用中文等组合输入法（`Source::InputMethod`）且刚点击新位置导致焦点保密性处于 `Secrecy::Unknown` 时，暂存按键与 `Shift`/`CapsLock` 最多 `HOLD_MS = 400 ms`，由后台 `Probe` 线程调用 `ax::text_focus`（焦点未跳到点击处时最多等 `SETTLE_AWAY_MS = 300 ms`，需两次一致读数才确认）。若确认为 `AXSecureTextField`（`Secrecy::Secure`），立即绕过输入法，清空预编辑串并将已缓存的 `COMPOSED_KEYS` 还原为原始按键发往目标进程；若为普通输入框（`Secrecy::Open`），走 GPUI 视图 `NSTextInputClient`，通过 ObjC IMP swizzle 包装 `doCommandBySelector:` 识别韩文等"先上屏再发功能键"（`Outcome::TextThenKey`）场景，并用 `caret_in_window()` 定位候选词浮窗。 | 1. 目标应用主线程阻塞超过 `400 ms` 时探测超时，按默认策略放行；2. Canvas 自绘输入框不暴露 AX 光标矩形时，输入法候选窗回退至点击位置或窗口底部显示。 | `#117`、ADR 0008 | `[已验证]` `crates/model/src/viewer_keys.rs:1-260`、`apps/macos/src/mac/viewer_input.rs:430-966` |
