# Viewer 后台自绘标题栏窗口首击与拖拽失效根因调研报告

用户在 Agent Screen 的 Viewer 中按住虚拟显示器上的后台自绘标题栏窗口并拖动时，首次按压与拖拽失效由两条连续的系统机制共同造成 `[已验证]`。第一，目标进程未处于系统前台且目标窗口无空置标准标题栏可供预点击唤醒时，`SSFocusWithoutRaise` 只在目标进程内把 `NSApp.isActive` 置为 `YES`，不会将目标窗口设为关键窗口 `_isKeyWindow` `[已验证]`。紧随其后的首个 `LeftMouseDown` 进入 `-[NSWindow(NSEventRouting) _handleMouseDownEvent:isDelayedEvent:]` 后，因窗口此前非关键且自绘视图默认 `-[NSView acceptsFirstMouse:]` 返回 `NO`，`AppKit` 将 `self->_lastLeftHit` 清零为 `nil`，跳过 `[hitView mouseDown:]`，并在 `__routeMouseDraggedEvent` 中丢弃后续全部 `LeftMouseDragged` 事件 `[已验证]`。第二，即便绕过首击门控，现代自绘标题栏应用在 `mouseDown:` 中普遍调用 `-[NSWindow performWindowDragWithEvent:]`，经 `_SLSDragWindowRelativeToMouse` 进入 `WindowServer` 的 `__CGXPackagesDragWindowRelativeToMouse` `[已验证]`。该函数先检查全局会话硬件鼠标左键按下标志 `[CGXSessionPackageData + 0x43] == 1`，无物理按压时直接返回错误码 `0x3ec` 原地不动；用户真实物理按住鼠标时则直接读取用户屏幕上的物理硬件光标坐标 `_CGXGetCurrentCursorLocation`，完全忽略 `SLEventPostToPid` 投递的虚拟显示器坐标 `[已验证]`。标准标题栏窗口如 `Calculator` 与带原生无障碍工具栏的 `TallDrag` 之所以成功，是因为 `Worker::grab` 在发送任何鼠标事件前命中了 `ax::on_title_bar`，改由 Agent Screen 通过无障碍接口直接写 `AXPosition` 完成窗口平移 `[已验证]`。推荐在不激活应用、不抬升用户屏幕窗口、不移动物理光标的前提下实施双层修复：第一层在 `Worker::focus_waking` 无空置标题栏可唤醒时，于 `SSFocusWithoutRaise` 之后补发窗口外侧 `(-1, -1)` 有限坐标预点击使目标窗口在进程内成为关键窗口，并保留同进程用户窗口焦点归还逻辑；第二层在 `ax::on_title_bar` 中对无红绿灯按钮的 `AXStandardWindow` 放行顶部 `52 pt` 非控件区域进入 `Grab` 路径，并配合现有拖拽中途越界转 `Grab` 与松手巡检机制处理跨屏拉扯。

---

## 1. 分析对象与证据来源

本次静态逆向与受控探针实测均在当前机器 macOS `26.2` 版号 `25C56`、`arm64e` 架构下完成，未关闭 SIP，未附加系统进程或用户运行中应用，未移动物理指针，也未新建虚拟显示器 `[已验证]`。

- 系统二进制与 REA 证据：`WindowServer` 位于 `/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/Resources/WindowServer`，版本 `919.8.7`，UUID `23D83CC9-EADC-38B0-8795-598637D70223`，SHA-256 `bfe87fc78b759699abea1d85c283772a50aaf589c2c2a6f2d43dde60217153ad`，REA `inspect-macho` 证据编号为 `ev_5ef7a8b65dacc646f3bc11634dfd693ce5d198d4ff0deaaa0117528fcd2385dc` `[已验证]`。该可执行文件仅导入 `SkyLight` 的 `_SLXServer`，服务端事件路由与窗口拖拽实现均位于 dyld 共享缓存内的 `SkyLight.framework` `[已验证]`。
- 共享缓存提取镜像：通过 `ipsw dyld extract` 提取当前系统 `/System/Library/dyld/dyld_shared_cache_arm64e` 中的 `AppKit.framework/Versions/C/AppKit`，版本 `2685.60.104`，UUID `4E909AEC-68BC-3FC9-A87A-DE928E1E36E1`；以及 `SkyLight.framework/Versions/A/SkyLight`，版本 `600.0.0`，UUID `E7D21A4B-D787-3703-9CF3-7A6AD756224B` `[已验证]`。下文所有十六进制虚拟地址均对应此共享缓存基址 `[已验证]`。
- 公开头文件与文档：核对 Xcode macOS 26 SDK 中的 `AppKit.framework/Headers/NSView.h`、`NSWindow.h` 与 `CoreGraphics.framework/Headers/CGEventTypes.h` `[已验证]`。
- 受控离线探针：在 `/tmp/customdrag-probe` 编译独立离屏探针目标 `target` 与驱动器 `driver`，`target` 的 REA `inspect-macho` 证据编号为 `ev_33f93ac7d3c90706ebebdeee90f9951d02bc65ba367970316bf97a41c0b4d7b5` `[已验证]`。探针窗口置于 `(-18000, -18000)` 且透明度 `0.01`、`collectionBehavior = [.stationary, .ignoresCycle]`，完整复刻 `apps/macos/src/mac/input.rs:2334` 的 `stamp_routing` 字段布局进行实测 `[已验证]`。
- 外部仓库静态对照：在 `/tmp/cua` 浅克隆 `https://github.com/trycua/cua` 并静态审阅 `libs/lume/src/os/macos/` 与 `blog/inside-macos-window-internals.md`，全程未执行其代码 `[已验证]`。

---

## 2. 分支 `fanzzzd/root-causes` 历史复现核查

审阅分支 `fanzzzd/root-causes` 从提交 `5fc77ed` 到 `1c87b26` 的演进记录及 `docs/research/root-causes-2026-11.md` 第 2 节，可确认此前 `viewerdrag` 中 `CustomDrag` 的历史运行包含两类不同原因 `[已验证]`：

| 提交版本 | 夹具实现与测试顺序 | 实测表现 | 失效或通过原因 |
| --- | --- | --- | --- |
| `5fc77ed` | `CustomDragBar: NSView` 未重写 `acceptsFirstMouse:`，且 `CustomDrag` 排在最前执行 | 停在 `340,220` 未移动 `[已验证]` | `launchApp` 将 `TallDrag`、`CustomDrag`、`SheetDrag` 全部居中在 `x = 335..340`，顶层窗口遮挡按压点，且 `CustomDragBar` 默认 `acceptsFirstMouse:` 返回 `NO` `[已验证]` |
| `2700536` | `CustomDragBar` 增加 `acceptsFirstMouse:` 返回 `true`，先移走 `TallDrag`，但 `SheetDrag` 仍在 `CustomDrag` 之后 | 停在 `340,220` 未移动 `[已验证]` | `menuchoice.swift` 的 `beginSheet` 将居中的 `SheetDrag` 及其模态 Sheet 排在 `CustomDrag` 前方，按压点 `(400, 250)` 落在 `SheetDrag` 矩形 `335..565, 205..395` 上 `[已验证]` |
| `3350f82` | 改为 `CustomDragWindow: NSWindow` 重写 `sendEvent:`，在 `mouseUp` 后 `80 ms` 异步跳往 `stageOrigin`，仍排在 `SheetDrag` 之前 | 停在 `340,220` 未移动 `[已验证]` | 除遮挡与初始化重置因素外，该夹具只在 `NSWindow.sendEvent:` 内记录标志位并在 `mouseUp` 后跳屏，自身不在 `mouseDragged:` 中移动窗口 `[已验证]` |
| `7d3684e` | 先移走 `TallDrag` 与 `SheetDrag`，同时给窗口加 `CustomDragView` 返回 `acceptsFirstMouse = true`，并在脚本中额外先发一次 `click` 预唤醒 | 检查通过 `[已验证]` | 额外 `click` 提前消耗首击并完成窗口关键态切换，掩盖了单次按压拖拽缺陷，已在审阅 `R4-M3` 中指出 `[已验证]` |
| `1c87b26` | 保持先移走 `TallDrag` 与 `SheetDrag`，按 `R4-M3` 回退额外 `click` 与 `CustomDragView`，在 `y + 60` 单次拖拽 | 停在 `340,220` 未移动 `[已验证]` | 单次拖拽恢复复现，且第 4 轮实测中前序 `SheetDrag` 仅固定等待 `300 ms` 尚停在 `17/20` 步 `[已验证]` |

核查同时揭示了测试夹具与真实自绘标题栏应用的一项关键差异 `[已验证]`。`tests/e2e/tools/menuchoice.swift:36-57` 的 `CustomDragWindow` 直接重写了 `NSWindow.sendEvent(_:)`，在 `super.sendEvent(event)` 之前读取 `leftMouseDown`、`leftMouseDragged`、`leftMouseUp`，仅用于在松手 `80 ms` 后把窗口跳到 `stageOrigin` 测试 `keep_on_display` 的拉回能力 `[已验证]`。真实 macOS 自绘标题栏应用，包括使用 `fullSizeContentView` 自绘拖拽条的原生 `NSView` 应用、SwiftUI、GPUI、Tauri 以及使用 `-webkit-app-region: drag` 的 Electron 与 Chromium 应用，都在 `super.sendEvent(event)` 之后的视图分发层处理事件，并在 `mouseDown:` 或 `mouseDragged:` 中调用 `-[NSWindow performWindowDragWithEvent:]` 或自行更新窗口坐标 `[已验证]`。因此真实场景同时受制于 `AppKit` 视图首击门控与 `WindowServer` 硬件指针拖拽两道拦截 `[已验证]`。

---

## 3. 逐层机制与反汇编证据

### 3.1 `SkyLight` 与 `WindowServer` 如何处理发往后台非关键窗口的事件

Agent Screen 的 Viewer 输入在未命中原生标题栏 `Grab` 时，由 `apps/macos/src/mac/input.rs:1823-1824` 与 `2317-2346` 构造 `CGEvent`，经 `stamp_routing` 写入私有路由字段后调用 `SLEventPostToPid(target.pid, event)` `[已验证]`。

在 `SkyLight` 客户端与 `WindowServer` 服务端，整条调用链如下 `[已验证]`：

1. `0x187121048` 处的 `_SLEventPostToPid` 在栈上构造 `{1, pid}` 后调用 `0x187120f54` 处的 `_SLEventPostToPSN` `[已验证]`。后者以投递样式 `CGSPostEventStyle = 2` 调用 `0x18711fcdc` 处的 `postEventsWithStyle`，经 `_CGSEncodeEventRecord` 序列化后通过 Mach RPC `__CGSPostFilteredEventTapData` 发往 `WindowServer`，入口地址为 `0x1871297c4` `[已验证]`。
2. `WindowServer` 入口 `_XPostFilteredEventTapData` 位于 `0x187250b8c`，转调 `0x186fd8d60` 处的 `post_filtered_event_tap_data` `[已验证]`。在 `0x186fd91bc` 处，服务端按投递样式分支：

```armasm
; SkyLight: post_filtered_event_tap_data
0x186fd91b4: cmp  w19, #0x1
0x186fd91b8: b.eq loc_186fd9220               ; style == 1 -> SLXPostEventRecord
0x186fd91bc: cmp  w19, #0x2
0x186fd91c0: b.ne loc_186fd91fc               ; Global HID -> _CGXFilterEvent
0x186fd91c4: ldr  w0, [sp, #0x30]
0x186fd91d0: bl   __CPXPostEventToProcessOnly ; style == 2 -> target PID port only
```

这段反汇编直接回答了第二个核心问题的前半部分 `[已验证]`：

- `WindowServer` 收到 `SLEventPostToPid` 即 `style == 2` 投递的事件时，直接走 `_CPXPostEventToProcessOnly` 将 `SLSEventRecord` 写入目标进程的专属事件端口，完全绕过全局 `_CGXFilterEvent`、全局屏幕坐标窗口命中测试、窗口自动置顶排序、进程自动前台激活以及 `WindowServer` 自身的标题栏拖拽状态机 `[已验证]`。
- `WindowServer` 不会因为目标窗口非关键或非主窗口而丢弃该事件，不会改写 `kCGMouseEventClickState`，不会抹除事件里的窗口号，也不会在事件记录上打额外的首击标记 `[已验证]`。
- 在目标进程内，`0x1850b13ec` 处的 `-[NSEvent _initWithCGEvent:eventRef:]` 在 `0x1850b14d0` 从 `SLSEventRecord + 0x3c` 读取 `_windowNumber`，对应 `stamp_routing` 写入 `CGEventField 51` 的 `wid`；在 `0x1850b1560` 从 `+0x20/+0x28` 读取 `_locationInWindow`，对应 `SSEventSetWindowLocation` 写入的窗口左下角局部坐标 `[已验证]`。
- 随后 `0x185454364` 处的 `-[NSApplication(NSEventRouting) sendEvent:]` 对 `0x1854546d0` 的 `LeftMouseDown`、`0x185454788` 的 `LeftMouseUp` 与 `0x185454844` 的 `LeftMouseDragged` 均直接调用 `[self windowWithWindowNumber:[event windowNumber]]`，找到目标 `NSWindow` 后在 `0x18545485c` 调用 `[window sendEvent:event]` `[已验证]`。

标准标题栏窗口如 `Calculator`、带模态 Sheet 的标准标题栏宿主窗口 `SheetDrag` 以及带 `AXToolbar` 子视图的 `TallDrag` 之所以在后台首次拖动即可移动，是因为 `apps/macos/src/mac/input.rs:595` 在 `mouse_down` 入口先执行 `w.grab(at, button)` `[已验证]`。`Grab::title_bar` 调 `ax::on_title_bar` 命中标题栏后，Agent Screen 根本不向目标进程发送拖拽期的 `LeftMouseDown` 与 `LeftMouseDragged`，直接在 `apps/macos/src/mac/windows.rs:188-192` 的 `Grab::drag_to` 中通过无障碍接口写 `kAXPositionAttribute` 移动窗口 `[已验证]`。

### 3.2 为何 `SSFocusWithoutRaise` 未能将自绘标题栏窗口设为关键窗口

当按压未命中 `Grab` 时，`Worker::mouse_down` 在 `apps/macos/src/mac/input.rs:1805` 调用 `self.focus(target)`，进入 `input.rs:1620-1660` 的 `focus_waking(target, true)` `[已验证]`。

`focus_waking` 先调用 `title_bar_point(target, bounds)` 寻找空置标题栏点以发送一次预唤醒点击 `wake_at` `[已验证]`。`title_bar_point` 依赖 `apps/macos/src/mac/ax.rs:1373-1427` 的 `ax::on_title_bar`，该函数同时要求两项前置条件 `[已验证]`：

1. 按压点距离窗口顶边不得超过 `52.0 pt` 的 `TITLE_BAR_MAX_PT` `[已验证]`。
2. 第 `1388-1396` 行的 `has_title_bar(window)` 要求窗口必须存在 `AXCloseButton`、`AXMinimizeButton`、`AXZoomButton` 或 `AXFullScreenButton` 四个标准按钮属性之一，且无障碍命中元素的角色不得是 `AXButton`、`AXWebArea`、`AXTextField` 等交互节点 `[已验证]`。

对于隐藏红绿灯的 `fullSizeContentView` 窗口如 `CustomDrag`，探针实测 `close=0 fs=0`；对于顶部铺满 `AXWebArea` 的 Electron 窗口，或位于 `52 pt` 以下的自绘拖拽区，`title_bar_point` 一律返回 `None` `[已验证]`。代码记录 `input: no empty title bar to wake`，退回调用 `send_focus`，执行 `apps/macos/native/SkyLight.m:142-160` 的 `SSFocusWithoutRaise(target.pid, target.window_id)` `[已验证]`。

`SSFocusWithoutRaise` 调用 `0x186ebc164` 处的 `_SLPSPostEventRecordTo` 向目标进程投递一条 `248` 字节原始事件记录，其中 `rec[0x08] = 0x0d` 代表 `NSEventTypeAppKitDefined`，`rec[0x3c] = wid`，`rec[0x8a] = 0x01` 代表 `NSApplicationActivatedEventType` `[已验证]`。该记录在目标进程触发 `0x184ef50f0` 处的 `-[NSApplication _handleAppKitDefinedEvent:]`，并转入 `0x184ef1564` 处的 `-[NSApplication _handleActivatedEvent:]` `[已验证]`。反汇编证明无论目标进程此前处于冷未激活态还是同进程另一窗口已唤醒态，该记录都不会把 `target.window_id` 设为关键窗口 `[已验证]`：

```armasm
; AppKit: -[NSApplication _handleActivatedEvent:]
0x184ef197c: bl   _objc_msgSend$setIsActiveFlag:         ; NSApp.isActive = YES
...
0x184ef1a30: tbnz w28, #0x2, loc_184ef1bc8               ; Case B: already active -> skip key selection!
0x184ef1a4c: cbz  x21, loc_184ef1a68                     ; event.windowNumber == 0?
0x184ef1a54: bl   _objc_msgSend$_visibleAndCanBecomeKey
0x184ef1a5c: cbz  w0, loc_184ef1a68
0x184ef1a64: cbz  w0, loc_184ef1b18                      ; Case A: jump over _findKeyWindow!
...
0x184ef1b18: mov  w2, #0x1
0x184ef1b1c: bl   _objc_msgSend$_setMouseActivationInProgress:
```

两种现场均已在 `/tmp/customdrag-probe` 中实测复现 `[已验证]`：

- 场景 A，目标进程首次后台交互，进入前 `NSApp.isActive == NO`：`0x184ef197c` 将 `NSApp.isActive` 置为 `YES`，但因 `rec[0x3c]` 填入了非零 `wid` 且目标窗口 `_visibleAndCanBecomeKey == YES`，`0x184ef1a64` 直接跳过 `0x184ef1a88` 的 `_findKeyWindowConsideringSpaces...`，转入 `0x184ef1b18` 调用 `[self _setMouseActivationInProgress:1]` `[已验证]`。`AppKit` 假定 `WindowServer` 发来带窗口号的激活事件是因为紧接着会有一条真实鼠标点击完成激活，因此不提前选定关键窗口 `[已验证]`。探针 `results.log:18-19` 实测收到 `type=13 sub=1` 后状态为 `active=true pubKey=0 _hasKeyFocus=false w1._isKey=false` `[已验证]`。
- 场景 B，同进程已有另一窗口处于关键态，进入前 `NSApp.isActive == YES`：`w28` 的第 `2` 位保存进入函数前的 `isActive` 值，`0x184ef1a30` 的 `tbnz w28, #0x2, loc_184ef1bc8` 直接跳过整个关键窗口选择分支 `[已验证]`。探针 `results2.log:22,25-29` 实测兄弟窗口 `w1._isKey=true` 保持不变，目标窗口仍为 `_isKey=false` `[已验证]`。

与此同时，`WindowServer` 从未向该后台进程发送 `subtype = 0x1000` 的关键焦点通知 `-[NSApplication _handleKeyFocusNotification:withEvent:]`，函数地址为 `0x184ef5b20`，因此 `NSApp._hasKeyFocus` 始终保持 `NO` `[已验证]`。

### 3.3 第一道拦截：`AppKit` 首击门控清零 `_lastLeftHit` 并丢弃后续全部拖拽

紧跟 `SSFocusWithoutRaise` 发出的 `LeftMouseDown` 进入 `0x18571bf48` 处的 `-[NSWindow(NSEventRouting) sendEvent:]` `[已验证]`。

首先，在 `0x18571c09c` 至 `0x18571c100` 处，若窗口挂载了尚未处于活动态的 `NSGestureRecognizer`，例如 SwiftUI `DragGesture` 或 `NSPanGestureRecognizer`，`sendEvent:` 在 `LeftMouseDown` 分支依次检查 `0x18571c0ec` 的 `[self isMainWindow]` 与 `0x18571c0f8` 的 `[self _isKeyWindow]`；两者均为 `NO` 时直接跳过 `sendEventToGestureRecognizers:requireAcceptsFirstMouse:` `[已验证]`。

随后，`sendEvent:` 调用 `0x18571c440` 处的 `_reallySendEvent:isDelayedEvent:`，转入 `0x18571e958` 处的核心函数 `-[NSWindow(NSEventRouting) _handleMouseDownEvent:isDelayedEvent:]` `[已验证]`。其精确判定顺序如下 `[已验证]`：

1. 视图命中测试：在 `0x18571edac` 调用 `[self _hitTestWithHysteresisCheck:locationInWindow forEvent:event allowWindowDragging:&allowDrag]`，返回值存入实例变量 `self->_lastLeftHit` `[已验证]`。
2. 提前锁存旧关键态：在 `0x18571efd8` 调用 `[self _isKeyWindow]` 与 `0x18571efe8` 的 `[self _hostWindowIsKey]`，其中 `-[NSWindow _isKeyWindow]` 位于 `0x184a066e4`，直接返回 `(_wFlags >> 4) & 1` `[已验证]`。因目标窗口此时 `_wFlags & 0x10 == 0`，栈变量 `[sp, #0x14]` 即 `wasAlreadyKey` 在 `0x18571f018` 被锁存为 `0`，随后读入 `w21` `[已验证]`。
3. 进程内延迟升为关键窗口：在 `0x18571f1cc`、`0x18571f2fc` 或 `0x18571f414` 调用 `0x1856810c8` 处的 `[self _changeKeyAndMainLimitedOK:]` `[已验证]`。由于 `SSFocusWithoutRaise` 已把 `NSApp.isActive` 置为 `YES`，此处会将当前窗口升为关键窗口并置位 `_wFlags |= 0x10`，同时清除 `_mouseActivationInProgress = 0`，但 `w21` 中的 `wasAlreadyKey` 仍保持升格前锁存的 `0` `[已验证]`。
4. 询问视图是否接受首击：在 `0x18571f42c` 调用 `0x18571fa24` 处的 `-[NSWindow(NSEventRouting) view:acceptsFirstMouseEvent:initialKeyWindow:]`，内部调用 `[hitView acceptsFirstMouse:event]` `[已验证]`。Apple 公开头文件 `NSView.h:294` 声明且 `0x18564d5a4` 反汇编证实 `-[NSView acceptsFirstMouse:]` 默认直接返回 `0`，指令为 `mov w0, #0; ret`；Chromium 的 `RenderWidgetHostViewCocoa` 默认同样对未激活点击返回 `NO` `[已验证]`。
5. 清零 `_lastLeftHit` 并跳过 `mouseDown:`：在 `0x18571f440` 至 `0x18571f4e0` 处，当 `wasAlreadyKey == 0`、`[self canBecomeKeyWindow] == YES`、`acceptsFirstMouse == NO`、非无需成为关键窗口的浮动面板且 `[event clickCount] == 1` 时，执行以下指令 `[已验证]`：

```armasm
; AppKit: -[NSWindow(NSEventRouting) _handleMouseDownEvent:isDelayedEvent:]
0x18571f4b8: bl  _objc_msgSend$clickCount
0x18571f4bc: cmp x0, #0x1
0x18571f4c0: b.ne loc_18571f4e8
0x18571f4cc: bl  _objc_msgSend$_shouldStartWindowDragForEvent: ; w23 = theme titlebar hit
0x18571f4d4: str xzr, [x19, x27]                               ; self->_lastLeftHit = nil!
0x18571f4dc: orr x8, x8, #0x40000                              ; _wFlags |= ignoredFirstMouse
...
0x18571f628: ldr x21, [x19, x27]                               ; x21 = self->_lastLeftHit (0)
0x18571f62c: cbz x21, loc_18571f7c0                            ; skip [hitView mouseDown:]!
```

其中 `0x185673728` 处的 `-[NSWindow _shouldStartWindowDragForEvent:]` 转调 `0x185591388` 处的 `-[NSThemeFrame shouldStartWindowDragForEvent:]`，仅当按压点落在系统原生 `_currentTitlebarContainerViewFrame` 即常规顶部 `28 pt` 内时返回 `YES` `[已验证]`。对于 `fullSizeContentView` 下位于 `28 pt` 以外的自绘拖拽区或无边框窗口，`w23` 为 `0`，`self->_lastLeftHit` 被清零为 `nil`，`0x18571f62c` 直接跳过 `0x18571f7a4` 的 `[hitView mouseDown:event]` `[已验证]`。

6. 丢弃后续全部 `LeftMouseDragged`：当后续拖拽事件到达 `_reallySendEvent:isDelayedEvent:` 并分发到 `0x18571cdfc` 处的 `__routeMouseDraggedEvent` 时，`0x18571ce40` 至 `0x18571ce48` 直接读取 `self->_lastLeftHit` `[已验证]`：

```armasm
; AppKit: __routeMouseDraggedEvent
0x18571ce40: ldrsw x8, [x8, #0x850] ; OBJC_IVAR_$_NSWindow._lastLeftHit
0x18571ce44: ldr   x19, [x21, x8]
0x18571ce48: cbz   x19, loc_18571cfb4 ; nil -> drop LeftMouseDragged immediately!
```

因首击已将 `_lastLeftHit` 置为 `nil`，整串 `LeftMouseDragged` 被逐条静默丢弃 `[已验证]`。在 `/tmp/customdrag-probe/results2.log:58-73` 中，向默认返回 `acceptsFirstMouse = false` 的 `ManualDragView` 发送 `SSFocusWithoutRaise` 加 `6` 步拖拽序列后，`APP_EVENT` 完整收到了 `type=1`、`6` 条 `type=6` 与 `type=2`，窗口在首击后变为 `wm._isKey=true`，但 `VIEW_MANUAL_DOWN` 与 `VIEW_MANUAL_DRAGGED` 一次都未触发 `[已验证]`。

针对问题 1 中涉及的全部 `CGEvent` 与 `SLEvent` 字段，逐项反汇编核查结论如下 `[已验证]`：

| 事件字段或状态 | 读取位置 | 对首击是否进入视图的影响 |
| --- | --- | --- |
| `windowNumber` 即字段 `51/91/92` 与 `SSEventSetWindowLocation` | `_initWithCGEvent:eventRef:` 的 `0x1850b14d0`、`0x1850b1560` 及 `NSApplication sendEvent:` 的 `0x1854546d4` | 决定事件路由到哪一个 `NSWindow` 及窗口内命中坐标，不改变 `_wFlags & 0x10` 决定的 `wasAlreadyKey` `[已验证]` |
| 目标 PID 字段 `40`、手势分组字段 `58`、触摸子类型字段 `7 = 3` | `SkyLight` 事件解码与触控辅助分支 | 不影响 `0x18571efd8` 的 `_isKeyWindow` 与 `0x18571f440` 的首击门控 `[已验证]` |
| `kCGMouseEventClickState` 即 `[event clickCount]` | `_handleMouseDownEvent:` 的 `0x18571f4b8` | 仅当 `clickCount == 1` 时进入清零 `_lastLeftHit` 分支；伪造 `clickCount = 2` 虽可跳过 `0x18571f4d4`，但会在 `0x18571ee04` 触发标题栏双击缩放 `_shouldSnapSizeOnDoubleClick`，或触发 Electron 标题栏双击最大化与文本双击选词 `[已验证]` |
| `NSEvent.eventNumber` 与 `_DoDeferredOrdering` | `0x18571f158`、`0x18571f2b8` 传给 `_SLSBlockWindowOrdering` 与 `__NSDoDeferredWindowOrdering` | 仅关联 `WindowServer` 延迟窗口排序令牌，且晚于 `0x18571efd8` 锁存 `wasAlreadyKey`，不影响首击丢弃 `[已验证]` |
| `Control + Command` 修饰键即 `modifierFlags & 0x140000 == 0x140000` | `0x185673b40` 处的 `-[NSWindow _tryWindowDragWithEvent:]` | 在 `sendEvent:` 入口直接转调 `_SLSDragWindowRelativeToMouse`，立即撞上下述第二道拦截 `[已验证]` |
| `WindowServer` 窗口前后序置顶 | `AppKit` 不查询全局窗口 Z 序判定首击 | `AppKit` 只检查本进程 `NSWindow._wFlags` 第 `4` 位 `_isKeyWindow`，不要求窗口在 `WindowServer` 全局排在最前 `[已验证]` |

### 3.4 第二道拦截：`performWindowDragWithEvent:` 与 `WindowServer` 硬件指针绑定

即便第一道拦截已解除，例如窗口此前已是关键窗口、视图重写了 `acceptsFirstMouse:` 返回 `YES`、按压落入 `NSThemeFrame` 顶部 `28 pt` 令 `0x18571f4cc` 的 `_shouldStartWindowDragForEvent:` 返回 `YES`，或命中 `0x18571f6b8` 的 `mouseDownCanMoveWindow` 与 `0x18571f714` 的 `isMovableByWindowBackground`，自绘标题栏拖拽仍会撞上第二道拦截 `[已验证]`。

Apple 在 macOS 10.11 引入公开头文件 `NSWindow.h:901` 声明的 `-[NSWindow performWindowDragWithEvent:]` 作为自绘标题栏标准拖窗接口；SwiftUI、GPUI、Tauri 以及 Electron 与 Chromium 的 `-webkit-app-region: drag` 均通过该接口驱动窗口移动 `[已验证]`。反汇编追踪其完整调用链如下 `[已验证]`：

1. `0x185673490` 处的 `-[NSWindow performWindowDragWithEvent:]` 调用 `0x185673508` 处的 `_dragWindowRelativeToMouseDown:options:completionHandler:`，注册本地事件监听器监听 `LeftMouseUp` 后，在 `0x18567348c` 调用 `SkyLight` 的 `_SLSPackagesDragWindowRelativeToMouse`，等同于 `0x18710ca20` 处的 `_SLSDragWindowRelativeToMouse` `[已验证]`。
2. `WindowServer` 服务端执行 `0x186eaea0c` 处的 `__CGXPackagesDragWindowRelativeToMouse` `[已验证]`：

```armasm
; SkyLight (WindowServer): __CGXPackagesDragWindowRelativeToMouse
0x186eaeadc: ldrb w8, [x22, #0x43]              ; session hardware left mouse button down?
0x186eaeae0: cmp  w8, #0x1
0x186eaeae4: b.ne loc_186eaebc0                 ; != 1 -> return 0x3ec (kCGErrorCannotComplete)!
...
0x186eaeb3c: bl   __CGXGetCurrentCursorLocation ; reads real physical cursor on user screen!
0x186eaeb48: bl   __ZL19_prepareToDragWindowP16CGXWindowSubList7CGPoint
```

该实现带来两项直接后果 `[已验证]`：

- 无物理按压时立即返回 `0x3ec` 拒绝移动：在 `debugViewerInput` 测试注入或代理会话调用 `drag` 工具时，用户物理鼠标左键并未按下，`[CGXSessionPackageData + 0x43]` 为 `0`，函数在 `0x186eaebc0` 直接返回 `kCGErrorCannotComplete` 即 `0x3ec` `[已验证]`。探针 `results2.log:150-152` 实测证实，即便目标窗口已满足 `wp._isKey=true` 且 `PerformDragView` 收到 `mouseDown:` 并调用了 `window.performDrag(with: event)`，窗口坐标仍为 `origin=(0,0) -> (0,0)`，一步都未移动 `[已验证]`。
- 有物理按压时绑定用户屏幕上的真实硬件指针：当用户在自己屏幕上的 Viewer 窗口内真实按住鼠标左键拖动时，`[x22 + 0x43]` 为 `1`，但 `WindowServer` 全程通过 `0x186eaeb3c` 与 `0x186eaec28` 的 `_CGXGetCurrentCursorLocation` 读取用户在主屏幕上的真实硬件光标坐标，完全忽略 Agent Screen 通过 `SLEventPostToPid` 发往目标进程的虚拟显示器坐标 `[已验证]`。随后在 `0x186eaeef0` 的 `dragWindowMoveToProposedOrigin` 与 `0x186eb01b0` 的 `mouseLocationAllowsDragMovement` 及其子调用 `0x186eb0244` 的 `managed_space_allows_window` 中，若系统多空间策略禁止跨显示器归属，窗口便卡在原地不动；若允许跨屏移动，`WindowServer` 则把窗口直接拉向用户物理屏幕 `[已验证]`。
- 与现有越界接管逻辑的关系：`apps/macos/src/mac/input.rs:628-644` 在非 `Grab` 拖拽途中每 `50 ms` 检查 `press.left_display()`，一旦发现窗口被 `WindowServer` 拉离虚拟显示器，便立即发送 `mouse_up` 中断系统拖拽、调用 `press.restore_to(to)` 拉回，并在剩余手势中切换为 `Grab` `[已验证]`。但该机制生效的前提是首个 `mouseDown:` 没有被第一道拦截吞掉，且硬件按键处于按下状态使 `WindowServer` 真正启动了跨屏移动 `[已验证]`。

---

## 4. 同类后台点击工具与 `trycua/cua` 对比

### 4.1 常见 macOS 后台点击方案对比

1. 无障碍动作 `AXPress`：调用 `AXUIElementPerformAction(elem, kAXPressAction)` 通过 Mach 无障碍 IPC 直接触发控件动作方法，不走 `NSWindow sendEvent:`，因而不受 `acceptsFirstMouse:` 与 `_isKeyWindow` 限制 `[已验证]`。但 `AXPress` 只能触发离散点击动作，无坐标参数，无法执行连续鼠标拖拽 `[已验证]`。
2. 全局 HID 注入工具如 `cliclick` 与 `CGEventPost(kCGHIDEventTap, ...)`：走 `WindowServer` 全局 `_CGXFilterEvent`，自动移动真实物理光标、激活目标应用并抬升目标窗口，直接违反 ADR 0001 `[已验证]`。
3. 私有前台切换函数 `_SLPSSetFrontProcessWithOptions`：位于 `0x186eba308`，窗口管理器如 `yabai` 常配合选项 `0x200` 调用该函数切换焦点窗口 `[已验证]`。反汇编 `0x186eba308` 至 `0x186eba5dc` 证实该函数向 `WindowServer` 提交全局前台进程切换请求，会抢占用户当前活跃应用的菜单栏与键盘焦点，同样违反 ADR 0001 `[已验证]`。

### 4.2 与 `trycua/cua` 的逐项对比

静态审阅 `/tmp/cua` 中的 `libs/lume/src/os/macos/input/{skylight.rs,mouse.rs}`、`tools/{click.rs,drag.rs}` 及 `blog/inside-macos-window-internals.md`，结论如下 `[已验证]`：

- 与 Agent Screen 共享的底层机制：`cua` 的后台模式同样使用 `SLPSPostEventRecordTo` 发送 `248` 字节激活记录加 `50 ms` 等待，使用 `SLEventPostToPid` 投递鼠标事件，并向 `CGEvent` 写入完全一致的私有路由字段 `51`、`91`、`92`、`40`、`58` 与 `CGEventSetWindowLocation`，见 `input/skylight.rs:196-310` 与 `input/mouse.rs:584-628` `[已验证]`。
- `cua` 针对首击问题的处理方式：
  1. 在 `tools/click.rs:1396-1461` 中，`cua` 对普通左键单击默认优先尝试 `AXPress`，仅在控件不支持 `AXPress` 时回退到 `SkyLight` 合成鼠标点击 `[已验证]`。
  2. 在 `input/mouse.rs:563-565` 中，`cua` 每次 `click_window_local` 在正式点击前先向窗口局部坐标 `(-1.0, -1.0)` 发送一对 `LeftMouseDown` 与 `LeftMouseUp` 预点击，在窗口内零命中的前提下触发 `AppKit` 完成关键窗口切换 `[已验证]`。
  3. 在 `input/skylight.rs:255-282` 中，`cua` 另实现了 `make_key_window_record`，将 `rec[0x08]` 置为 `0x01/0x02` 并将偏移 `0x20..0x30` 填满 `0xff`，仅在键盘输入的 `WindowKeyScope` 中调用 `[已验证]`。探针 `results.log:69-70` 实测该记录发现，全 `0xff` 字节在 IEEE-754 双精度浮点下解码为 `NaN`，会在目标进程产生 `locationInWindow = (NaN, NaN)` 的 `LeftMouseDown/Up` 事件；若目标窗口在 `sendEvent:` 中将坐标转为整型如 `Int(event.locationInWindow.x)` 且未先检查 `.isFinite`，会立即触发 Swift 运行时崩溃 `[已验证]`。
- `cua` 与 Agent Screen 均尚未解决的问题：`cua` 完全没有实现 Agent Screen 的无障碍 `Grab` 标题栏移动路径 `[已验证]`。在 `tools/drag.rs:248-340` 中，`cua` 的后台拖拽仅发送 `SSFocusWithoutRaise` 后紧跟 `LeftMouseDown`、`LeftMouseDragged` 与 `LeftMouseUp`，既未在 `drag` 中发送 `(-1, -1)` 预点击而撞上第一道拦截，也无法驱动 `performWindowDragWithEvent:` 而撞上第二道拦截返回 `0x3ec` `[已验证]`。`cua` 对此类拖拽唯一的退路是调用方显式传入 `delivery_mode: "foreground"` 调用 `with_foreground_hid_activation`，激活目标应用并移动真实物理光标，而这在 Agent Screen 的 ADR 0001 约束下被明确禁止 `[已验证]`。

---

## 5. 候选修复方案与权衡

围绕"不激活用户屏幕上的应用、不抬升用户屏幕上的窗口、不移动真实物理光标"三条硬约束，对全部候选方案评估如下：

| 方案 | 作用层级 | 能否解决首击吞没 | 能否解决系统窗口拖拽 | 主要副作用与风险 | 裁决 |
| --- | --- | --- | --- | --- | --- |
| A. 修改 `CGEvent` 字段如 `clickState = 2` 或 `Ctrl+Cmd` 标志 | 事件字段层 | 仅 `clickCount = 2` 可绕过 `0x18571f4c0` `[已验证]` | 否，`Ctrl+Cmd` 仍调 `_SLSDragWindowRelativeToMouse` `[已验证]` | 伪造双击会触发标题栏缩放最大化、列表双击打开或文本选词 `[已验证]` | 否决 |
| B. 调用 `_SLPSSetFrontProcessWithOptions` | `WindowServer` 进程焦点层 | 是 | 否，仍缺物理按压或锚定物理光标 `[已验证]` | 改变系统全局前台应用，抢占用户菜单栏与键盘焦点，违反 ADR 0001 `[已验证]` | 否决 |
| C1. `SSFocusWithoutRaise` 加无障碍写 `kAXMainAttribute = true` | 进程内窗口关键态 | 仅在已清除 `_mouseActivationInProgress` 的热启动应用生效，冷启动应用首击仍被吞 `[已验证]` | 否 | `0x185696ed4` 处的 `-[NSWindow accessibilitySetMainAttribute:]` 调用 `makeKeyAndOrderFront:`，当冷启动 `_mouseActivationInProgress == 1` 时无法完成关键态升格 `[已验证]` | 仅作热态归还辅助，不可单独依赖 |
| C2. `SSFocusWithoutRaise` 加窗口外侧 `(-1, -1)` 有限坐标预点击，并保留 `restore_focus` | 进程内窗口关键态 | 冷启动与同进程多窗口热启动均 100% 生效 `[已验证]` | 否，需与方案 D 组合 | 向窗口局部坐标 `(-1, height + 1)` 投递一对按下抬起，`_hitTest` 返回 `nil` 不触碰任何控件，`NSPointInRect` 为 `0` 不触发背景拖窗，有限坐标不引发 `NaN` 崩溃 `[已验证]` | 采纳为第一层修复 |
| D1. 放宽 `ax::on_title_bar` 中 `AXStandardWindow` 的红绿灯前置条件 | 无障碍 `AXPosition` 层 | 命中 `Grab` 时直接绕过鼠标事件分发 `[已验证]` | 命中时完全绕过 `WindowServer` 拖拽，无物理按压也可在 `debugViewerInput` 下移动 `[已验证]` | 覆盖窗口顶部 `52 pt` 且无交互子节点的原生 `fullSizeContentView` 窗口；Electron 窗口整面暴露为 `AXWebArea`，Chromium 在 macOS 上未通过公开 AX 属性暴露 `-webkit-app-region: drag` `[部分已验证，基于 Chromium 架构与 AX 树结构，未对所有 Electron 版本逐一枚举私有属性]` | 采纳原生窗口部分；不对 `AXWebArea` 盲目启用 `Grab` 以免破坏网页文本划选 |
| D2. 保留拖拽中途 `press.left_display()` 越界转 `Grab` 并可选提供 Viewer 显式拖窗修饰手势 | `Grab` 兜底与交互层 | 依赖 C2 先让首个 `mouseDown:` 送达应用 `[已验证]` | 真实物理按压且允许跨屏时自动接管；对 `debugViewerInput` 或系统禁止跨空间移动的场景，显式拖窗手势可兜底 `[已验证]` | 无物理按键的脚本化测试若目标窗口不在 `mouseDragged:` 中改坐标而只调 `performWindowDragWithEvent:`，普通拖拽无法启动 `WindowServer` 拖窗循环 `[已验证]` | 采纳并与 C2、D1 组合 |

关键验证细节如下：

1. 窗口外侧 `(-1, -1)` 预点击安全性验证 `[已验证]`：在 `/tmp/customdrag-probe` 中，无论是冷启动进程 `STATE[cold_primer]` 还是已有兄弟窗口占据关键态的进程 `results2.log:83-106`，发送 `SSFocusWithoutRaise(pid, wid)` 后向全局 `(bounds.x - 1, bounds.y - 1)` 即局部坐标 `(-1, bounds.height + 1)` 发送一对 `LeftMouseDown` 与 `LeftMouseUp`，在 `_handleMouseDownEvent:isDelayedEvent:` 内表现一致：
   - `0x18571ea9c` 的 `NSPointInRect(locationInWindow, {0, 0, frame.size})` 返回 `0`，因此栈上 `[sp, #0x3f]` 的 `allowWindowDragging` 保持 `0`，不会在 `0x18571f690` 误触发 `performWindowDragWithEvent:` `[已验证]`。
   - `0x18571edac` 的 `_hitTestWithHysteresisCheck:` 返回 `nil`，不会点击窗口内任何按钮、网页或菜单 `[已验证]`。
   - `0x18571f1cc` 与 `0x18571f2fc` 正常调用 `_changeKeyAndMainLimitedOK:` 并清除 `_mouseActivationInProgress = 0`，将目标窗口在进程内升为 `_isKeyWindow == YES`，同时 `NSApp._hasKeyFocus` 保持 `false`，不会激活应用或抬升用户屏幕上的窗口 `[已验证]`。
   - 紧随其后的首次真实拖拽序列无需视图重写 `acceptsFirstMouse:`，即可完整触发 `VIEW_MANUAL_DOWN`、全部 `6` 步 `VIEW_MANUAL_DRAGGED` 与 `VIEW_MANUAL_UP` `[已验证]`。
2. 同进程共享应用焦点归还验证 `[已验证]`：当目标 PID 同时在用户主屏幕上拥有关键窗口时，对应议题 `#110` 与 `#120` 场景，`apps/macos/src/mac/input.rs:1626-1637` 的 `Worker::focus_waking` 已通过 `!on_agent_display(other.window_id)` 记录 `self.remember(other)`，并在操作完成后由 `restore_focus()` 归还关键态 `[已验证]`。探针 `results2.log:140-143` 同时证实，对已唤醒进程调用 `AXUIElementSetAttributeValue(user_win, kAXMainAttribute, kCFBooleanTrue)` 可在不向用户窗口发送任何鼠标点击的情况下将进程内 `_isKeyWindow` 归还给 `w1` `[已验证]`。

---

## 6. 推荐方案与真实路径验证设计

### 6.1 推荐落地方案

建议在下一阶段编码轮采用 C2 加 D1 加 D2 的双层组合方案，改动收敛在 `apps/macos/src/mac/input.rs` 与 `apps/macos/src/mac/ax.rs`，全程遵守 ADR 0001：

1. 消除第一道拦截，位于 `apps/macos/src/mac/input.rs` 的 `Worker::focus_waking`：
   - 当 `!is_front(target.pid)` 且 `self.woken != Some((target.window_id, activations()))` 时，若 `title_bar_point` 找到空置标题栏点则保持现有 `wake_at`；若返回 `None` 且窗口位于 Agent Display 上，则在 `send_focus(target)` 发送 `SSFocusWithoutRaise` 后，向全局 `(bounds.x - 1.0, bounds.y - 1.0)` 即目标窗口局部坐标 `(-1.0, bounds.height + 1.0)` 补发一对 `LeftMouseDown` 与 `LeftMouseUp` 预唤醒事件，并记录 `self.woken = Some((target.window_id, activations()))`。
   - 对非点击型 `Grab` 调用 `focus_waking(hit.target, false)` 时，因无需向视图投递 `mouseDown:`，保持仅发 `SSFocusWithoutRaise` 即可。
   - 在 `restore_focus()` 归还同进程用户屏幕窗口关键态时，优先尝试无点击副作用的 `kAXMainAttribute = true`，失败再回退现有唤醒点击。
2. 消除第二道拦截，位于 `apps/macos/src/mac/ax.rs` 的 `on_title_bar` 与 `Grab` 协同：
   - 在 `apps/macos/src/mac/ax.rs:1388-1426` 的 `ax::on_title_bar` 中，将 `has_title_bar(window)` 的判定调整为：存在任一标准窗口按钮，或窗口 `AXSubrole == "AXStandardWindow"`，同时继续排除 `AXDialog`、`AXSystemDialog`、`AXUnknown` 等无边框弹出层与自动补全列表，防止误把列表首行当标题栏拖走。调整后，所有隐藏红绿灯的原生 `fullSizeContentView` 窗口在顶部 `52 pt` 非交互区都会直接进入 `Grab` 写 `AXPosition`，无论物理鼠标是否按下均可平滑移动。
   - 对顶部铺满 `AXWebArea` 的 Electron 与 Chromium 窗口及位于 `52 pt` 以下的自绘拖拽条，由第一层修复保证首个 `mouseDown:` 与 `mouseDragged:` 送达应用。用户在真实 Viewer 内物理按住拖动时，若应用在 `mouseDragged:` 中自行更新窗口坐标则直接在虚拟显示器内完成移动；若应用调用 `performWindowDragWithEvent:` 触发 `WindowServer` 将窗口拉向物理屏幕，则由现有 `input.rs:634` 的 `press.left_display()` 在 `50 ms` 内自动中断系统拖窗并切换为 `Grab`，松手后由 `keep_on_display` 的 `[0, 40, 150, 350, 700] ms` 巡检兜底。
   - 若后续还需支持在系统开启"显示器具有单独的空间"且阻止跨屏拖出时拖动 Electron `AXWebArea` 标题栏，可在 Viewer 中增加显式拖窗修饰手势，例如按住 `Option + Command` 在 Viewer 内拖动任意位置时直接走 `Grab::held` 写 `AXPosition`，不侵入普通点击与文本划选。

### 6.2 真实路径验证方案

下一轮编码落地时，按以下三层在真实路径验证，全程遵守虚拟显示器创建预算与隔离实例规则：

1. 模型层与无障碍纯逻辑单测，零虚拟显示器创建：
   - 为 `has_title_bar` 抽出纯判定函数放入 `crates/model`，覆盖有红绿灯标准窗口、无红绿灯 `AXStandardWindow`、`AXSystemDialog` 弹出面板、顶部含 `AXButton` 或 `AXWebArea` 子节点四组矩阵。
2. 夹具分类验证，在 `AGENTSCREEN_INSTANCE=e2e` 与 `AGENTSCREEN_IDLE_SECS=86400` 下单次运行 `viewerdrag`：
   - 将 `tests/e2e/src/bin/viewerdrag.rs:210` 中前序 `SheetDrag` 的固定 `sleep_ms(300)` 改为轮询等待到位，消除前序队列积压干扰。
   - 将 `tests/e2e/tools/menuchoice.swift` 中重写 `NSWindow.sendEvent:` 的 `CustomDragWindow` 替换为真实视图级自绘拖拽夹具 `CustomDragBarView: NSView`，保持默认 `acceptsFirstMouse` 返回 `false`，在 `mouseDown:` 与 `mouseDragged:` 中按 `event.locationInWindow` 实时更新窗口坐标并在 `mouseUp:` 后模拟异步越界跳变；在超出 `52 pt` 标准标题栏带的 `y + 60` 处不发预点击直接执行单次 `debugViewerInput` `drag`，断言单次拖拽即按位移落盘且主屏幕零窗口泄漏。
   - 新增无红绿灯 `fullSizeContentView` 窗口顶部 `y + 20` 单次拖拽检查，验证放宽 `AXStandardWindow` 后直接命中 `Grab` 成功移动。
3. 物理按压路径边界说明与人工验收：
   - 明确区分脚本注入与真实触控板按压：`debugViewerInput` 不按下硬件鼠标左键，`[CGXSessionPackageData + 0x43]` 为 `0`，无法驱动 `performWindowDragWithEvent:` 进入 `WindowServer` 移动循环 `[已验证]`。对调用 `performWindowDragWithEvent:` 的真实 Electron 应用或 `PerformDragView` 夹具，需在隔离 `e2e` 实例的 Viewer 窗口内用真实触控板单次按住自绘标题栏拖动一次，并配合 `sharedapp` 脚本验证同进程在主屏幕打开另一窗口时键盘焦点归还正常。
