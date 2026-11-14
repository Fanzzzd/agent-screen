# 2026 年 11 月开放议题与显示器日志根因分析

本次排查覆盖全部 7 个开放议题与 `~/Library/Logs/Agent Screen/displays.log` 中的 3 类异常。五个缺陷与三类日志异常源于三个共同的错误假设：

1. 代码假设已停靠的虚拟显示器可以再次平移，且平移后窗口仍留在原显示器上。实际上 `WindowServer` 以全局坐标保存窗口位置，`CGConfigureDisplayOrigin` 只移动显示器视口，不平移窗口。
2. 代码假设 `WindowServer` 会回填已释放的虚拟显示器单元号，且重用 `serial` 不会继承历史分辨率。实际上 `WindowServer` 按当前在线虚拟显示器数量分配单元号，并按厂商号、产品号与序列号持久化 `800x500` 等回退模式。
3. 代码假设 Agent Display 上的交互目标都是宿主进程名下的 `layer == 0` 顶层 `AXWindow`，且 `SCStream::update_configuration` 会在静止画面上立即补发帧。实际上 AppKit Sheet、ExtensionKit `.appex` 视图、`layer == 101` 弹出菜单与 `ScreenCaptureKit` 空闲流都违反这一假设。

下文逐项说明证据、macOS 真实行为、选定修复与否决方案。

## 1. 议题 #204：新显示器停靠到已占用原点导致窗口归属转移

### 证据

`~/Library/Logs/Agent Screen/displays.log` 记录了 2026-11-14 11:27:43 的完整重排过程：

- 旧显示器 `379` 持有 `serial=5`，停在 `4288,-800 1280x800`。旧显示器 `381` 持有 `serial=6`，停在 `5568,-800 1280x800`。
- 早期空闲备用显示器过期释放后，编号 `1` 回到空闲池。
- 新会话创建显示器 `638` 时，`Serial::lease` 分配了最小空闲编号 `serial=1`。
- `arrangement::plan` 按 `serial` 与 `id` 升序重排全部 `Role::Ours` 显示器，把 `638` 放进第一槽位 `4288,-800`，把运行中的 `379` 推到 `5568,-800`，把 `381` 推到 `6848,-800`。
- 另在 `displays-e2e.log` 第 488 行，当用户固定显示器位于虚拟显示器右侧时，`arrangement::plan` 只读取固定显示器的最大 `X` 值，也会重排已停靠的显示器。

### macOS 真实行为与错误假设

`WindowServer` 用全局桌面坐标记录每个窗口的矩形。显示器只是全局桌面上的视口。调用 `CGConfigureDisplayOrigin` 移动显示器原点时，macOS 不会跟随视口平移该区域内的窗口。`windows::on_display` 按窗口矩形与各显示器视口的重叠面积判定归属。`repark` 把 `379` 的视口从 `4288,-800` 移走并把 `638` 放进 `4288,-800` 后，留在全局坐标 `4288,-800` 内的窗口立即变成 `638` 的窗口。

### 选定修复与否决方案

选定修复：在 `crates/model/src/arrangement.rs` 中将已经停在安全区且不与固定显示器或其他已停靠显示器重叠的 `Role::Ours` 显示器视为固定锚点，原位保留其坐标。未停靠的新显示器只放入避开全部固定显示器与已停靠显示器的空闲槽位。若外部配置变更迫使 `Core::repark` 移动某台显示器的原点，`Core::repark` 按视口位移同步平移该显示器上的窗口，并在 `tuck_away_restoring` 中去除重复的原点写入。

否决方案：改为递增分配 `serial` 而不重用编号。该方案会让 `ColorSync` 与 `WindowServer` 无限累积显示器配置项，触发 ADR 0009 记录的注册表膨胀问题，且仍无法防止右侧新增固定显示器时发生重排。

## 2. 日志异常：27 次 `misroute` 与 `800x500` 模式回退

### 证据

`displays.log` 在 `0.1.34` 版本记录了 27 条 `misroute` 日志，大多紧跟在后台 `make_spare` 分配低 `serial` 并触发 `repark` 平移在用显示器之后。同一日志多次记录显示器或复用中的备用显示器掉到 `800x500`。

### macOS 真实行为与错误假设

1. `repark` 平移在用显示器的原点时，旧原点上在途的 `ScreenCaptureKit` 帧与新显示器布局发生短暂错位，触发 `ui/stream.rs` 的 `misroute` 校验并反复重建采集流。
2. `WindowServer` 在重配置压力下会把虚拟显示器切回 `800x500` 低分辨率模式，并按厂商号、产品号与序列号记住该模式。此前 `Core::hold_modes` 只巡检 `self.displays.list`，不巡检 `self.spares`，导致备用池中的显示器长期停在 `800x500` 或在复用瞬间暴露旧模式。

### 选定修复与否决方案

选定修复：固定已停靠显示器位置以消除人为 `repark` 引发的假性 `misroute`。同时扩展 `Core::hold_modes`，对 `self.spares` 与在用显示器执行相同的目标分辨率巡检与恢复。

否决方案：关闭 `misroute` 检测。锁屏唤醒后 `ScreenCaptureKit` 仍可能把物理屏幕画面混入虚拟显示器流，保留检测才能自愈真实串流。

## 3. 日志异常：`VirtualDisplay::create` 产生 `unit twin` 双生显示器

### 证据

`displays.log` 自 2026-11-14 02:39 起多次记录 `unit twin`：每次新建 `CGVirtualDisplay` 时，系统同时出现两台同单元号或相邻单元号的虚拟显示器，代码不得不丢弃其中一台并重试。

### macOS 真实行为与错误假设

`WindowServer` 为新建 `CGVirtualDisplay` 分配单元号时，按起始单元号加上当前在线虚拟显示器数量计算，不扫描并回填中间已释放的空洞。此前 `Core::tidy` 清理超量或过期的备用显示器时，通过 `old.pop()` 保留最新创建的高单元号显示器，释放较早创建的低单元号显示器。只要高单元号显示器仍在线，低单元号空洞就会让下一次新建撞上在线单元号，触发 `unit twin`。

### 选定修复与否决方案

选定修复：新增纯模型模块 `crates/model/src/spares.rs`。`pick_spare` 优先复用单元号最小的匹配备用显示器。`keep_spares` 按单元号升序保留备用池，且只要某台过期备用显示器的单元号低于任何在线显示器或保留备用显示器的单元号，就在 `max_spares` 上限内继续保留它，不提前制造单元号空洞。

否决方案：彻底禁用备用显示器过期回收。禁用回收会永久占用虚拟显示器配额与内核显存。

## 4. 议题 #189：另一会话持有的窗口无法取回且无法识别持有者

### 证据

用户报告会话 A 上的 Chrome 窗口消失，`place_window` 拒绝移回窗口，提示窗口归另一会话所有但未给出对方标签。用户在菜单栏中找不到任何持有该窗口的显示器，约 10 分钟后窗口自行回到主屏幕。

### macOS 真实行为与错误假设

这是 `#204` 视口平移与备用池隐藏显示器的组合结果：

1. 后台 `make_spare` 租用低 `serial` 触发 `repark`，把某台备用显示器移到原会话视口坐标，而原显示器视口右移。Chrome 窗口留在原全局坐标，落入隐藏的 `Spare` 显示器视口内。
2. 备用显示器不在 `Core::displays.list` 中，菜单栏与 Peek 均不展示它。`place_window` 只搜索用户屏幕与当前显示器，找不到落入备用显示器的窗口。其他路径遇到持有冲突或请求超时时，错误文本也未列出持有显示器的标签。
3. 备用显示器存活时间 `SPARE_LIFE` 恰好为 10 分钟。10 分钟后 `tidy` 销毁该备用显示器，`WindowServer` 将失去显示器的窗口迁回用户主屏幕，表现为窗口自行出现。

### 选定修复与否决方案

选定修复：

1. 固定已停靠显示器位置，并在 `spares` 规则中禁止回收或转入备用池时丢弃仍有窗口残留的显示器。
2. 在 `placeWindow`、`where_now` 与 `strays` 中纳入 `spare_ids()`，使落入备用显示器的窗口可被正常寻址、回收与迁移。
3. 在 `crates/model/src/placement.rs` 与 `crates/model/src/held.rs` 中补全 `no_window_named`、`no_app_window_named`、`declined_message` 与 `timed_out_message`，当窗口位于另一会话显示器、待审显示器或请求超时未决时，明确返回持有显示器的 `Display Label`。

否决方案：允许会话无需确认直接抢占未命名显示器的窗口。这会违反 ADR 0017 的跨会话窗口授权边界。

## 5. 议题 #190：Peek 卡片画出窗口轮廓但不显示画面内容

### 证据

用户悬停菜单栏打开 Peek 时，卡片按真实位置画出了窗口边框，但底图为空白占位态，打开 Viewer 后画面立即恢复。

### macOS 真实行为与错误假设

窗口边框来自 `CGWindowListCopyWindowInfo` 的实时查询，而底图来自 `ui/stream.rs` 维护的 `ScreenCaptureKit` 采集流：

1. 显示器空闲时，流运行在 `1 fps` 保活档位，缩略图尺寸与 Peek 相同。打开 Peek 请求 `12 fps` 缩略图流时，尺寸未增大，代码只调用 `SCStream::update_configuration` 提高帧率。在画面完全静止的显示器上，`replayd` 不会因 `update_configuration` 主动推送新 `Complete` 帧。打开 Viewer 时请求全分辨率，尺寸增大，代码重建 `SCStream`，`startCapture` 立即送出首帧，因此打开 Viewer 能恢复画面。
2. `replayd` 在画面长时间静止时会以 `stream-stopped` 停止空闲流。旧代码在重试超限后清空缓存帧，并进入退避等待，导致 Peek 打开时既无缓存帧也无新帧。

### 选定修复与否决方案

选定修复：在 `apps/macos/src/ui/stream.rs` 中新增 `needs_fresh_stream` 与 `drop_cached_frame_on_break`。当订阅需求从 `<= 1 fps` 提升到交互帧率 `> 1 fps` 且无 `waitForChange` 观察者占用流时，直接重启采集流以获取 `startCapture` 首帧，重启期间保留上一帧缓存避免闪烁。仅在连续 `misroute` 时丢弃缓存帧，空闲 `stream-stopped` 保留最后一帧并在新订阅到来时立即唤醒重连。

否决方案：让所有后台显示器常驻 `12 fps` 采集。常驻高帧率会持续消耗 `WindowServer` 与 `replayd` 算力。

## 6. 议题 #205 与 #207：Sheet 弹窗、`.appex` 扩展视图与弹出菜单交互失败

### 证据

用户在操作“系统设置 -> 通用 -> 时间机器 -> 选项...”弹窗中的“备份频率”弹出按钮时遇到四连环失败：

1. `act_on_element` 执行 `set_value` 报错，无法识别控件所属窗口或焦点。
2. 改用 `real_input` 拖拽触发菜单后，菜单弹出的瞬间拖拽报错中止。
3. 菜单打开后，对菜单项发起 `real_input` 点击被拒绝，提示目标点不在会话窗口内。
4. 在后台模式下点击菜单项偶发无效，点击菜单外部也无法收起卡住的菜单。

### macOS 真实行为与错误假设

1. AppKit 通过 `NSWindow.beginSheet` 打开的 Sheet 在 `CGWindowListCopyWindowInfo` 中拥有独立 `CGWindowID`，在 `AXWindows` 中以 `AXRole` 为 `AXSheet` 的节点出现，其 `AXParent` 指向宿主 `AXWindow`。同时应用处于非前台状态时 `AXFocusedWindow` 常返回空值，真正接收按键的窗口只能沿 `AXFocusedUIElement` 的 `AXParent` 链定位到 `AXSheet` 或宿主 `AXWindow`。旧代码只认 `AXRole` 为 `AXWindow` 的节点且只读 `AXFocusedWindow`，导致置前与焦点判断失败。
2. macOS Ventura 及之后的系统设置面板运行在 ExtensionKit `.appex` 进程中，路径位于 `.appex/Contents/MacOS/`。旧代码 `is_xpc_service` 只匹配 `.xpc/Contents/MacOS/`，`app::bundle_of` 对无 `NSRunningApplication` 的 `.appex` 进程也返回空，导致 `belongs`、`popup_facts` 与键盘焦点投递全部失配。
3. AppKit 弹出菜单位于 `kCGPopUpMenuWindowLevel`，即 `layer == 101`。旧代码 `DragScene::under` 与 `window_at` 只接受 `layer == 0`，菜单一弹出就判定拖拽偏离窗口并拒绝后续 `real_input` 点击。
4. 后台点击弹出菜单时，命中点常落在 `AXMenuItem` 内部的文本或勾选标记子节点上，且可能跨宿主进程与 `.appex` 进程。此外 `NSMenu` 处于模态事件循环时忽略后台发往窗口的鼠标点击，点击菜单外部不会自动关闭菜单。

### 选定修复与否决方案

选定修复：

1. 将 `cg::is_xpc_service` 扩展为 `cg::is_remote_view_service`，同时识别 `.xpc/Contents/MacOS/` 与 `.appex/Contents/MacOS/`，并在 `app::bundle_of` 中加入基于 `proc_pidpath` 的包路径回退。
2. 在 `ax.rs`、`agent.rs` 与 `real_input.rs` 中新增 `host_window_element`、`enclosing_window_id` 与 `is_window_or_sheet_focused`，在 `AXFocusedWindow` 为空时从 `AXFocusedUIElement` 反查所属 `AXSheet` 或 `AXWindow`，使 Sheet 能完成置前、焦点校验与 `popup_window` 定位，并在 `choose_closed_popup` 中对 `.appex` 控件向其服务进程发送按键。
3. 更新 `DragScene::under`、`real_drag_window_at` 与 `real_click_at`，并同步修订 ADR 0013，允许同一显示器上宿主窗口所属的 Sheet、层级 `0..=8` 的面板及层级 `101` 的已打开弹出菜单通过包含性与遮挡校验。
4. 更新 `ax::press_menu_item_at` 沿 `AXParent` 向上查找 `AXMenuItem` 并覆盖显示器上的关联进程；新增 `ax::cancel_menu_at`，当后台点击落在同一显示器已打开的 `layer == 101` 菜单外部窗口或桌面空白区域时，对 `AXMenu` 执行 `AXCancel` 收起菜单。

否决方案：放行任意进程的 `layer == 101` 窗口进入 `real_input`。若不校验菜单归属进程与显示器边界，可能误点系统级弹出层。

## 7. 保持开启且本次不改动的议题

### 议题 #110：Dia 浏览器共享窗口的键盘与链接点击限制

原因：针对用户主屏幕与 Agent Display 同时打开同一浏览器不同窗口时的安全输入放行，代码已在 PR `#119`、`#129`、`#153` 与 `#185` 中落地。该议题当前仅剩在真实 Dia 窗口上的交互验收。由于任务安全约束禁止自动化脚本操作或点击用户正在运行的浏览器实例，本次不关闭 `#110`，留待人工验收后关闭。

### 议题 #206：等待审批时全屏显示红色发光边框与 8% 至 12% 红色遮罩

原因：该提议与 ADR 0013 第 27 行的安全语义直接冲突。ADR 0013 规定，屏幕边缘细红框仅在 Input Grant 生效期间展示，用来向用户传达唯一信号：Agent 此刻正握有真实鼠标指针，移动鼠标即可立即收回控制权。若在尚未授权的被动等待阶段也点亮红框并叠加全屏遮罩，既会混淆“等待审批”与“指针已被接管”两种状态，也会遮挡用户当前屏幕工作。该项属于需要产品与设计重新拍板的交互决策，本次不作代码改动。
