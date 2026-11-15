# 2026 年 11 月开放议题、Viewer 拖拽缺陷与显示器日志根因调研报告

全部 7 个开放议题、Viewer 标题栏拖拽逃逸缺陷与 3 类显示器日志异常可收敛为四条底层错配 `[已验证]`：

1. 代码把显示器视口坐标当作窗口归属本身，且允许移动已停靠的虚拟显示器 `[已验证]`。`WindowServer` 以全局桌面坐标保存窗口矩形，应用调用 `CGConfigureDisplayOrigin` 移动虚拟显示器原点时不会平移窗口，同时 `WindowServer` 拒绝保留悬空不连通的显示器布局 `[已验证]`。议题 `#204`、议题 `#189` 的同日复现及部分 `misroute` 均由此产生 `[已验证]`。
2. 代码假设通过 `SkyLight` 向后台进程投递鼠标事件即可在虚拟显示器内完成原生标题栏拖拽，且窗口越界一定在松开鼠标的同一时刻发生 `[已验证]`。实际上 `WindowServer` 标题栏拖拽会把窗口吸附到用户物理屏幕上的真实硬件指针 `[已验证]`，而自定义拖拽应用会在松开鼠标后异步提交窗口位移 `[架构推断，具体商业应用毫秒区间未验证]`。Viewer 标题栏拖拽偶发落到主屏幕即由此产生 `[已验证]`。
3. 代码假设 Agent Display 上的交互目标都是宿主进程名下 `layer == 0` 的顶层 `AXWindow` `[已验证]`。实际上 AppKit Sheet 的无障碍角色是 `AXSheet`，macOS 系统设置面板运行在独立的 ExtensionKit `.appex` 进程中，弹出菜单位于 `layer == 101`，后台点击菜单外部也不会自动退出 `NSMenu` 追踪循环 `[已验证]`。议题 `#205` 与 `#207` 由此产生 `[已验证]`。
4. 代码对 `CGVirtualDisplay` 与 `ScreenCaptureKit` 生命周期做了三项过强假设：假设释放低单元号备用显示器后新建显示器会自动回填单元号空洞，假设备用池显示器不会发生分辨率回退，假设空闲采集流在 Peek 提速时总有最新帧可用 `[已验证]`。`unit twin`、`800x500` 模式回退与议题 `#190` 均与这组生命周期边界有关 `[部分已验证]`。

下表汇总全部 11 个调研对象的验证结论与推荐处置。文中代码行号默认指向基线 `origin/main` 提交 `dab5e39`，涉及分支 `PR #208` 首轮审阅提交 `e239e65` 与次轮审阅提交 `9de473c` 时单独注明。每项事实与推断均标注 `[已验证]` 或 `[未验证]`。

| 编号或主题 | 核心根因归类 | 根因验证状态 | 落地与验证结论 |
| --- | --- | --- | --- |
| `#204` 新显示器抢占原点导致窗口串会话 | 视口重排与几何归属错配 | `[已验证]` | 已落地连通性守卫、非阻塞停靠加 `ReparkState` 串行平移重试与 `effective_agent_display` 统一归属，模型与适配层单测通过 |
| Viewer 标题栏拖拽偶发落回主屏幕 | `WindowServer` 指针吸附、高窗口中点过滤与异步越界 | `[已验证]` | 已落地超大窗口中心夹紧、单窗口节流检测与窗口级巡检暂停，`viewerdrag` 实测 `8/9` 通过，`CustomDrag` 唤醒点击待复测 |
| `#189` 另一会话占用窗口无法收回且缺标签 | `#204` 视口平移、断连残留与错误文案缺标签 | `[已验证]` | 已补全持有者标签、重连与重叠连接 `InputQueue` 续绑及已关闭连接墓碑守卫，`sessiondisplays` 实测 `13/14` 通过 |
| `#190` Peek 画出窗口边框但无画面内容 | `0.1.31` 缺串流自愈，外加 `replayd` 空闲停流 | `[部分已验证]` | 已落地 `retry_at` 退避守卫与温热帧保留并通过 `Stream::apply_want` 单测，保持 `Refs #190` |
| `#205` Sheet 与系统设置弹出菜单四连环失败 | `AXSheet`、`.appex` 进程与 `layer == 101` 菜单未建模 | `[已验证]` | 已识别 `.appex` 与 `AXSheet` 并放行同屏关联弹出菜单，`menuchoice` 实测红转绿 |
| `#207` Takeover 下 Viewer 点击菜单卡死 | `NSMenu` 子节点命中偏移与外部点击不关菜单 | `[已验证]` | 已限制 `AXParent` 查找层级与候选宿主进程并支持外部点击 `AXCancel`，Takeover `viewerdrag` 实测通过 |
| `#206` 等待审批时增强提醒可见性 | 全屏红框与 ADR 0013 指针接管红框语义冲突 | `[已验证]` | 拒绝全屏红框与染色，保持 `Refs #206`，落地菜单栏橙色待办徽标、琥珀色高亮卡片与 `Note::Input` 通知音 |
| `#110` Dia 浏览器按键短期失效与链接无响应 | 同进程多窗口焦点守卫与 Chromium 无障碍语义 | `[部分已验证]` | 已保持 `focused_window_id` 原语义不变，留待用户在真实 Dia 实例人工验收 |
| `unit twin` 虚拟显示器双生创建 | `WindowServer` 按在线数量分配单元号，遇空洞撞号 | `[已验证]` | 已落地最小单元号优先复用与高单元号优先淘汰，`spares.rs` 单测通过 |
| `misroute` 采集流画面错位日志 | 自发 `repark` 重排与物理显示器拓扑切换混合触发 | `[已验证]` | 已固定已停靠显示器消除自发错位，保留 `stream.rs` 错位自愈应对拓扑切换 |
| `800x500` 分辨率回退 | `e2e` 实例复用序列号触发模式回退，备用池未巡检 | `[已验证]` | 已将 `Core::hold_modes` 扩展到备用池显示器，`holdmode` 实测通过 |

---

## 1. 议题 #204：新显示器停靠到已占用原点导致窗口转移到新会话

### 症状

用户于 `0.1.34` 版本提交 `487ef90` 报告：会话 A 在显示器 `display=379` 上放置了 Grok Bot 窗口 `50839`，全局坐标为 `X=4500, Y=-770, 855x770` `[已验证]`。当另一终端会话 B 调用 `create_display` 创建新显示器 `display=638` 后，会话 A 调用 `list_windows` 返回空数组，再次调用 `place_window` 报错 `not_found: its windows on screen are on other Agent Displays, which other agent sessions hold` `[已验证]`。窗口并未回到用户主屏幕，且会话 A 从未释放它 `[已验证]`。

### 证据

`~/Library/Logs/Agent Screen/displays.log` 完整记录了 2026-11-13 至 2026-11-14 的坐标迁移链条 `[已验证]`：

- 第 `3001` 行记录 `2026-11-13 21:00:34.435 created display=379 unit=10 at=5568,-1700 size=1280x800 serial=5` `[已验证]`。
- 第 `3019` 行记录 `2026-11-13 21:04:06.422 created display=381 unit=11 at=-1280,0 size=1280x800 serial=6` `[已验证]`。
- 早先占用 `serial=1..4` 的空闲备用显示器在夜间过期释放后，编号 `1` 回到空闲池，`379` 与 `381` 则停在 `4288,-800` 与 `5568,-800`，见第 `4206` 行 `2026-11-14 11:22:10.792 stream-stopped display=379 unit=10 at=4288,-800 size=1280x800` `[已验证]`。
- 第 `4208` 行记录 `2026-11-14 11:27:01.853 created display=638 unit=8 at=-1280,0 size=1280x800 serial=1` `[已验证]`。
- 第 `4211` 行记录 `2026-11-14 11:27:03.020 parked display=638 unit=8 at=4288,-800 size=1280x800 tries=1` `[已验证]`。
- 第 `4219` 至 `4221` 行记录 `11:32:51` 时三台显示器的停靠位置已变为：`638` 位于 `4288,-800`，`379` 被推到 `5568,-800`，`381` 被推到 `6848,-800` `[已验证]`。
- `~/Library/Logs/Agent Screen/displays-e2e.log` 第 `304` 行还记录了另一条重排路径：当另一实例的固定显示器 `Role::OtherInstance` 停在 `4288,-800` 与 `5568,-800` 时，`plan` 重新移动已停靠的 `573 [ours] 9856,-800 -> 9376,-800` `[已验证]`。

对应 `origin/main` 代码路径 `[已验证]`：

- `crates/model/src/model.rs:588` 的 `lowest_free_serial` 与 `apps/macos/src/mac/display.rs:52` 的 `Serial::lease` 每次分配当前最小的空闲整数序列号 `[已验证]`。
- `crates/model/src/arrangement.rs:71-145` 的 `arrangement::plan` 将全部 `Role::Ours` 显示器按 `serial` 与 `id` 升序排序，从用户屏幕右上角锚点依次重分槽位，不区分新创建显示器与已承载窗口的在用显示器 `[已验证]`。
- `apps/macos/src/mac/windows.rs:143-189` 的 `belongs` 与 `on_display` 完全按窗口当前全局矩形落入哪台显示器的视口来判定窗口归属 `[已验证]`。

### macOS 与相关组件真实行为

1. `WindowServer` 在全局桌面坐标系中存储每个窗口的 `kCGWindowBounds`，来源为 Apple `CGWindow.h` 文档 `[已验证]`。应用调用 `CGConfigureDisplayOrigin` 只移动虚拟显示器视口在全局坐标系中的原点，不会移动原本位于该全局坐标区域内的窗口，来源为 `displays.log` 第 `4206` 至 `4221` 行与议题 `#204` 现场记录：显示器 `379` 从 `4288,-800` 移至 `5568,-800` 后，窗口 `50839` 仍留在 `4500,-770` `[已验证]`。至于 macOS 自身在桥接显示器释放或物理显示器热插拔时移动虚拟显示器视口是否会自动平移其上窗口，本次未单独触发硬件热插拔验证 `[未验证]`，`Core::repark` 已在平移前记录 `old -> now` 及新旧矩形上的窗口数量以便观测。只有当某台显示器被销毁下线且窗口彻底失去宿主视口时，`WindowServer` 才会把孤儿窗口迁回在线显示器 `[已验证]`。
2. `WindowServer` 要求所有在线显示器组成的几何图保持边或角连通，不允许某台显示器悬空脱离主连通分量，来源为仓库 `docs/DEVELOPMENT.md:164` 与实测显示器重排日志 `[已验证]`。若配置事务提交了悬空显示器，`WindowServer` 会自行修改显示器原点将其拉回接触位置 `[逻辑已由文档验证，硬件悬空拉回轨迹本次未单独复现，标为未验证]`。

### 现有代码的错误假设与 PR #208 审阅问题

`origin/main` 存在两条错误假设 `[已验证]`：

1. 假设 `arrangement::plan` 可以随时重排已停靠的 `Role::Ours` 显示器而不影响其上的窗口 `[已验证]`。
2. 假设窗口归属可以随时由当前几何视口反推，无需在显示器被动位移时修正窗口坐标或核对放置记录 `[已验证]`。

首轮审阅报告 `/tmp/review-208.md` 指出提交 `e239e65` 的三处设计缺陷，次轮审阅报告 `/tmp/review-208-r2.md` 进一步指出提交 `9de473c` 的两处并发与统一归属缺陷 `[已验证]`：

1. 悬空空洞缺陷 `R1-H1`：`e239e65` 在 `crates/model/src/arrangement.rs:110-117` 无条件钉死所有已停靠显示器，并在单元测试 `arrangement.rs:502-509` 断言当首槽位 `4288,-800` 的显示器释放后，`5568,-800` 与 `6848,-800` 原地不动 `[代码逻辑已验证]`。这会使右侧两台显示器不接触任何用户显示器，触发 `WindowServer` 拒绝悬空布局并强制拉回视口，`#204` 会在显示器结束时重现 `[连接性约束已由文档验证，真机拉回表现未验证]`。
2. 窗口平移读错起点 `R1-H2` 与并发重入竞态 `R2-H1`：`e239e65` 等外部重排改变显示器坐标后才调用 `windows::on_display(id)` 读取窗口，读错起点且阻塞配置线程 `[已验证]`。`9de473c` 虽改为异步读取 `parked_rects`，但未串行化并发触发的 `Core::repark`：多台显示器紧邻上线产生两次重排通知时，后一轮会在前一轮 AX 位移尚未落盘时读取旧窗口位置再次叠加位移，且无条件用旧快照覆盖新建显示器的 `parked_rects` `[已验证]`。
3. 归属规则分散与冲突裁决不一 `R1-M6`、`R2-M4`：`9de473c` 在 `held_by_bounds_at` 引入了登记归属，但 `windows::on_display`、`counts`、`Worker::hit`、待审清扫与备用池清空仍按纯几何矩形判断，且 `recorded_owners` 与 `recorded_display_of` 对同一窗口跨活跃显示器与待审显示器的裁决顺序不一致 `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：停止重用低序列号，改为单调递增分配 `serial`。否决该方案：ADR 0009 已证明单调递增 `serial` 会让 `WindowServer` 与 `ColorSync` 无限累积显示器配置记录并拖慢主线程，且无法解决物理显示器热插拔或右侧固定显示器引发的视口移动 `[已验证]`。
- 方案 B：无条件钉死已停靠显示器，不做连通性校验。否决该方案：即审阅指出的 `R1-H1`，首槽位显示器销毁后会留下悬空显示器，被 `WindowServer` 强制搬移 `[已验证]`。
- 方案 C：已落地 ADR 0018 的三步组合方案 `[已验证]`。第一，在 `crates/model/src/arrangement.rs` 通过纯函数 `connected(screens)` 保持已停靠且连通的显示器原地不动，桥接显示器释放时自动紧缩右侧显示器保证连通。第二，将 `Core::repark` 中的 `display::configure` 即时停靠与 `ReparkState` 窗口平移解耦，窗口平移在锁外按 `0.25 s` 无障碍超时、超时进程同轮跳过与单轮 `1 s` 总预算执行，失败目标回填 `ReparkState` 退避重试并记录首次告警，消除 `R1-H2`、`R2-H1`、`R3-M3` 与 `R3-L2`。第三，在 `Displays` 内绑定自动发布 `RECORDED_OWNERS`，提炼 `arrangement::effective_agent_display` 供 `list_windows`、`place_window`、`held_by`、`counts`、备用池探测与待审清扫共用，对尚未落入目标显示器矩形的已登记窗口在 `list_windows` 中标注 `offDisplay: true` 且省略坐标，在 `Worker::hit` 保持视觉 Z 序命中的同时拒绝代理点击其他显示器登记的顶层窗口并标明持有者，备用池同时检查登记与物理几何占用，消除 `R1-M6`、`R2-M4`、`R3-M2`、`R3-L3` 与 `R3-L4`。

### 真实路径测试方案

1. 模型层与适配层单元测试：在 `crates/model/src/arrangement.rs`、`held.rs`、`crates/model/tests/round3_regressions.rs`、`apps/macos/src/mac/windows.rs`、`input.rs` 与 `core.rs` 测试低 `serial` 新显示器不挤占高 `serial` 在用显示器、桥接释放后 `connected(&planned) == true`、`ReparkState` 并发重排只平移未登记窗口一次、失败平移跨二次位移换算重试、卡死进程同轮跳过与总耗时封顶、Takeover 期间拒绝会话移动与授权、视觉 Z 序顶层跨显示器窗口拦截、`offDisplay: true` 序列化以及备用池物理几何占用拦截 `[已验证]`。
2. 真实 `e2e` 路径说明：`sessiondisplays` 只验证多显示器会话归属而不触发已停靠显示器原点平移，受虚拟显示器创建硬预算限制，未额外编写强制重排显示器的真机 `e2e` 脚本 `[未验证]`。

---

## 2. Viewer 标题栏拖拽缺陷：在 Viewer 内拖动窗口标题栏偶发落到用户主屏幕

### 症状

用户在 Agent Screen 的 Viewer 窗口内按住目标窗口标题栏拖动时，窗口有时不会留在虚拟显示器内，反而直接跳到用户物理主屏幕上 `[已验证]`。该现象并非每次必现，取决于被拖窗口的几何尺寸、应用框架实现以及松开鼠标时的时序 `[已验证]`。

### 证据

检查 `origin/main` 的 Viewer 输入链路与拖拽实现，并结合次轮审阅提交 `9de473c` 的边界分析，定位到五处直接证据 `[已验证]`：

1. 抓取几何条件排除了高窗口：`apps/macos/src/mac/input.rs:1735` 的 `Worker::grab` 调用 `apps/macos/src/mac/windows.rs:56-92` 的 `Grab::on_display -> Grab::title_bar -> Grab::held` `[已验证]`。第 `85` 行要求 `area.contains(bounds.mid())`，即窗口矩形的几何中心点必须落在虚拟显示器矩形内 `[已验证]`。当窗口高度超过显示器高度两倍，例如在 `900x600` 显示器上打开高 `1300 pt` 的长文档或设置窗口，标题栏虽在 `y=40`，但 `bounds.mid().y` 达到 `690 > 600`，`Grab::held` 返回 `None` `[已验证]`。一旦 `w.grab` 为 `None`，`input.rs:1790-1850` 的 `mouse_dragged` 就退回向目标进程直接发送后台 `SLEventPostToPid` 的 `LeftMouseDown` 与 `LeftMouseDragged` `[已验证]`。
2. 超大窗口拖拽夹紧未约束窗口中心 `R2-H2`：`9de473c` 放行高宽窗口抓取后，`dragged_window_origin` 仅保证窗口边缘保留 `240 pt` 可见宽度与顶部不越上沿，当窗口宽度或高度大于显示器尺寸时，拖向边缘仍会让 `bounds.mid()` 越入相邻显示器或用户屏幕 `[已验证]`。
3. 标题栏无障碍命中超时与标签组误判 `R2-L7`、`R2-L9`：`origin/main` 的 `ax::on_title_bar` 使用默认 `1.0 s` 超时且未识别工具栏内标签栏；若无条件放行全部 `"AXTabGroup"`，靠近窗口顶部 `52 pt` 内的正文 `NSTabView` 也会被误判为标题栏 `[已验证]`。
4. 拖拽途中全量枚举窗口与松手 `0 ms` 早退 `R2-M3`：`origin/main` 的 `keep_on_display` 在 `mouse_up` 当刻若窗口尚未越界便直接 `return`，漏掉松手后的异步跳变，且 `9de473c` 在每次 `mouse_dragged` 事件上调用 `windows::all()` 全量枚举桌面窗口造成高频开销 `[已验证]`。同时 `guard_placed` 在 Takeover 期间未暂停巡检，可能与 Viewer 拖拽位置恢复竞争 `[已验证]`。
5. 离线显示器瞬态零矩形映射：`apps/macos/src/mac/viewer_input.rs:1318` 的 `remote_point` 将 Viewer 归一化坐标映射到 `cg::display_bounds(id)` `[已验证]`。当显示器重配置瞬间 `CGDisplayBounds` 返回宽高为 `0` 的主屏原点矩形 `(0, 0, 0, 0)` 时，`remote_point` 会算出 `(0, 0)` 并把点击投递到用户主屏幕左上角 `[已验证]`。

实测证据：在 `AGENTSCREEN_INSTANCE=e2e` 上使用 `CustomDrag` 与 `TallDrag` 两个测试夹具复现 `[已验证]`。在 `origin/main` 提交 `dab5e39` 上运行该测试，`TallDrag` 窗口高 `1300 pt` 位于 `900x600` 显示器，按住工具栏拖拽后失败退出，`CustomDrag` 在 `mouseUp` 后 `80 ms` 异步跳到主屏 `(80, 80)` 也未被 `keep_on_display` 拦住 `[已验证]`。

### macOS 与相关组件真实行为

1. `WindowServer` 原生标题栏拖拽吸附硬件指针：当目标进程收到标题栏区域的真实或 `SLEventPostToPid` 鼠标按下并启动系统窗口移动循环，无论是 AppKit 默认标题栏拖拽还是调用 `-[NSWindow performWindowDragWithEvent:]` 以及 Chromium 的 `-webkit-app-region: drag`，`WindowServer` 都会接管窗口移动并将窗口锚定到物理桌面上的真实 HID 硬件鼠标指针位置，完全忽略后续后台 `CGEvent` 中填写的虚拟显示器坐标 `[已验证]`。来源为 Apple `NSWindow.h` 中 `performWindowDragWithEvent:` 说明及本仓库实测确认 `[已验证]`。正因如此，Agent Screen 才设计了 AX `Grab` 在用户层拦截标题栏拖拽，改为直接写 `AXPosition` `[已验证]`。
2. 异步拖拽提交时序：Electron、CEF 与部分自绘标题栏应用在主线程收到拖拽与 `mouseUp` 后，经跨进程 IPC 再调用窗口位置更新，窗口跳往真实硬件光标坐标可晚于 `mouseUp` 发生，常见估算区间约为 `20 ms` 至 `120 ms` `[架构推断，商业应用具体毫秒区间未逐一测量，标为未验证]`。
3. Takeover 联动行为：通过 `debugViewerInput` 在 Viewer 内发起任何点击或拖拽都会自动触发 `ADR 0004` 规定的用户接管状态 Takeover `[已验证]`。

### 现有代码的错误假设

1. 假设属于虚拟显示器的窗口一定满足 `area.contains(bounds.mid())`，或只需保留 `240 pt` 边缘即可避免超大窗口中心越界 `[已验证]`。
2. 假设 `WindowServer` 或应用框架对窗口的跨屏拉扯一定在 `mouseUp` 返回前同步完成 `[已验证]`。
3. 假设 `cg::display_bounds(id)` 在流画面存活期间永远返回非空矩形 `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：拦截所有 Viewer 鼠标拖拽，无论是否点在标题栏都改为移动整个窗口。否决该方案：这会破坏窗口内文本划选、滚动条拖拽、分栏拖拽与画布拖拽 `[已验证]`。
- 方案 B：仅在 `keep_on_display` 中无条件把越界窗口重置回起拖前的 `initial` 坐标。否决该方案：当用户在 Viewer 内把窗口往显示器边缘拖动少许超界时，窗口会弹回起拖起点，拖拽手感断裂 `[已验证]`。
- 方案 C：已落地组合修复 `[已验证]`。第一，在 `viewer_press::map_viewer_point` 中拒绝宽高 `<= 0` 的显示器矩形。第二，在 `viewer_press::can_grab_window` 中对按压点位于显示器内且交集宽高均 `>= 40 pt` 的高宽窗口放行，在 `ax::on_title_bar` 中设置 `0.15 s` 超时、最多向上遍历 `12` 层 `AXParent`，且仅当 `"AXTabGroup"` 位于 `"AXToolbar"` 内时视为标题栏标签区，消除 `R2-L7` 与 `R2-L9`。第三，在 `viewer_press::dragged_window_origin` 中同时施加边缘可见夹紧、超大窗口几何中心留屏约束以及 `height > 2 * area.height - 2` 超高窗口居中兜底，消除 `R2-H2` 与 `R3-L1`，并通过 `ax::set_frame_asked` 保持原窗口尺寸不变。第四，`Grab::left_display` 改查单窗口 `cg::window_bounds` 并在拖拽途中按 `50 ms` 节流，消除 `R2-M3`；松手后只要拖拽发生了移动即启动 `[0, 40, 150, 350, 700] ms` 后台巡检按拖拽目标落点恢复，并通过 `ViewerDragGuard` 仅在活动拖拽与 `keep_on_display` 巡检窗口期间暂停该窗口的 `guard_placed`，消除 `R3-L5`。

### 真实路径测试方案

1. 模型层与输入层单元测试：在 `crates/model/src/viewer_press.rs`、`crates/model/tests/round3_regressions.rs` 与 `apps/macos/src/mac/input.rs` 测试 `map_viewer_point` 零矩形拒绝、`normalize_viewer_point` 画布外松手夹紧、`can_grab_window` 对超高超宽与半出界窗口放行且排除最小化、全屏与附着 Sheet、超大窗口与超两倍屏高窗口 `1700 pt` 中心留屏夹紧、`rebase_grab_for_display` 拖拽中途显示器重排坐标换算、`should_watch_after_release` 松手后异步越界巡检触发条件以及 `ViewerDragGuard` 窗口级引用计数守卫 `[已验证]`。
2. 真实 `e2e` 路径测试：在最终应用代码上以 `AGENTSCREEN_INSTANCE=e2e` 运行 `viewerdrag`，`9` 项检查中通过 `8` 项：系统 `Calculator` 原生标题栏拖拽从 `335,96` 移至 `469,156`、`TallDrag` 高 `1080 pt` 窗口沿对角线拖向 `toX = 1.2, toY = t_from.1 + 0.4` 从 `340,30` 同时夹紧横坐标与纵向中心上界至 `680,59` 且保持 `1080 pt` 高度、`SheetDrag` 附着模态 Sheet 宿主窗口从 `550,260` 拖至 `262,320`、两项 Takeover `.appex` 菜单点击与外部取消检查，全程用户主屏幕零窗口泄漏 `[已验证]`。`CustomDrag` 因无空标准标题栏后台窗口首击被 AppKit 消费而未触发拖拽，已在 `viewerdrag.rs` 补入预唤醒点击，因该次运行触发 `15 s` 空闲预热创建第 `3` 台显示器触及单次预算上限，按停机规则停止复跑，该测试改动保持 `[未验证]`。

---

## 3. 议题 #189：另一会话占用的窗口无法取回且无法识别持有者

### 症状

议题 `#189` 记录了两次用户现场故障 `[已验证]`：

1. 首次报告发生于 2026-11-12 10:08，版本 `0.1.31` 提交 `656ac75`：用户打开了 28 个 Claude Code 会话，主屏幕上的企业微信窗口消失，当前会话调用 `place_window` 报错窗口位于另一 Agent Display 上但未说明是哪一个会话或显示器标签，用户检查菜单栏未看到持有该窗口的显示器，约 1 至 3 分钟后在 10:11 评论反馈企业微信自行回到了主屏幕 `[已验证]`。
2. 第二次报告发生于 2026-11-14 11:36，版本 `0.1.34` 提交 `487ef90`：用户打开了 56 个 Claude Code 会话，Grok Bot 窗口被另一显示器持有且无法识别持有者，即议题 `#204` 的同一次现场事件 `[已验证]`。

用户同时提出三项诉求：第一优先级是已死亡或退出的会话自动释放显示器并将窗口归还用户。第二是在错误消息与菜单栏 UI 中标明持有每个窗口的会话身份。第三是提供一键或工具调用强制收回窗口的途径 `[已验证]`。

### 证据

1. 2026-11-12 现场日志核查：`~/Library/Logs/Agent Screen/displays.log` 第 `1990` 至 `2001` 行显示，09:54 至 09:55 在线显示器数在 `8 -> 10 -> 8` 之间波动，第 `1998` 行 `10:05:30.418 kept display=7 unit=6 at=4288,-800 size=1280x800 spare` 将 `display=7` 转入隐藏备用池，第 `2000` 行 `10:09:33.410 released display=50 unit=8 at=6848,-800 size=1280x800 ended, released while idle` 与第 `2001` 行 `10:09:33.436 released display=53 unit=9 at=4288,-1600 size=1280x800 spare unused` 恰好发生在用户报告议题后 1 分钟、评论"自行回到主屏"前 2 分钟 `[已验证]`。由于当时日志未记录各窗口坐标快照，企业微信当时究竟停在 `display=50` 还是隐藏备用显示器 `display=53` 上无法从日志单独锁定 `[未验证]`。
2. 版本时间线核查：`0.1.31` 与 `0.1.34` 均早于 2026-11-14 00:53 合入的 `PR #202` 提交 `c3a11ef` `[已验证]`。`c3a11ef` 已经实现了 ADR 0017 跨会话 `Window Request` 授权卡片，允许会话请求另一会话或待审显示器上的窗口并展示持有者名称 `[已验证]`。
3. 断连回收代码核查：`origin/main` 的 `apps/macos/src/core.rs:712` 函数 `connection_closed` 在 MCP 控制套接字断开时已经会调用 `end_session_displays` 结束该连接名下的显示器，并通过 `core.rs:580` 的 `return_handovers` 将用户移交的 `Handover` 窗口迁回用户屏幕 `[已验证]`。但仍存在三个盲区：第一，28 至 56 个打开但闲置的 Claude Code 终端进程并未退出，其 `AgentScreen mcp` 子进程保持套接字常开，`connection_closed` 不会触发 `[已验证]`。第二，`return_handovers` 只归还 `Handover` 窗口，而 `place_window` 迁入的 `Placement` 窗口在会话断开且显示器无改动退出时虽会随显示器销毁回到主屏，一旦显示器留作 Review 待审态就会继续留在虚拟显示器上 `[已验证]`。第三，跨实例 `0x5353` 显示器如并行运行的 `e2e` 实例在 `crates/model/src/held.rs:145` 中归为 `Holder::Other`，`place_window` 直接拒绝且不出卡片 `[已验证]`。
4. 错误文案核查：`crates/model/src/placement.rs:104-230` 的 `no_window_named` 与 `no_app_window_named` 在匹配窗口落到多台其他显示器或落到隐藏备用显示器时，未逐一列出持有显示器的标签 `[已验证]`。

审阅报告 `/tmp/review-208.md` 指出的 `PR #208` 提交 `e239e65` 问题 `[已验证]`：

- `H3`：`e239e65` 在 `crates/model/src/spares.rs:79` 让带有残留窗口的备用显示器永久保留不释放，而 `CONTEXT.md` 规定备用显示器在 UI 中完全不可见 `[已验证]`。更严重的是 `apps/macos/src/core.rs:283` 的 `take_spare` 硬编码传入 `has_windows: false`，会把带有残留窗口的备用显示器直接分给新会话，造成跨会话窗口泄露 `[已验证]`。
- `M1`：`e239e65` 提交说明声称修改了 `held_by` 识别跨实例 `0x5353` 显示器，实际代码并未修改 `held.rs`，且未解决大量闲置会话长期占住窗口的问题 `[已验证]`。

### macOS 与相关组件真实行为

1. MCP `stdio` 子进程生命周期：Claude Code 等宿主只要终端标签页未关闭，其派生的 `AgentScreen mcp` 进程便持续持有与主应用的 Unix Domain Socket 连接，操作系统不会产生 `EOF` 事件 `[已验证]`。
2. 隐藏备用显示器与窗口可见性：备用显示器仍是 `WindowServer` 中在线的 `CGVirtualDisplay` `[已验证]`。落在其视口内的窗口属于在线屏幕窗口，既不会触发 `WindowServer` 孤儿窗口回迁，也不会出现在用户物理屏幕或 Agent Screen 菜单栏列表中 `[已验证]`。

### 现有代码的错误假设

1. 假设只要监听套接字 `EOF` 就足以回收无主会话 `[已验证]`。实际上多会话重度用户常驻数十个空闲终端，连接存活不代表会话仍在工作 `[已验证]`。
2. 假设进入备用池的显示器上一定没有残留窗口，或有窗口时把备用显示器永久藏起来是安全的 `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：仅靠 `PR #202` 现有的跨会话 `Window Request` 卡片加上固定显示器坐标，声称彻底修复 `#189`。否决该方案：如审阅 `M1` 所述，这没有补齐多显示器冲突时的持有者标签列表，没有清理落入备用池的残留窗口，也没有提供菜单栏侧识别哪个会话持有哪个窗口的直观视图 `[已验证]`。
- 方案 B：对超过固定空闲时长如 15 分钟未发起工具调用的会话自动销毁显示器并把窗口弹回用户主屏幕。否决该方案：违反 ADR 0004"显示器以待审态结束且不打断用户当前屏幕"的约束，长任务等待用户回复期间被强行拆屏会破坏现场 `[已验证]`。
- 方案 C：已落地根因修复并保持 `Refs #189` 不直接关闭议题 `[已验证]`。第一，修复 `#204` 消除视口串位，通过后台线程探测备用池窗口 `probe_spares` 并在 `evacuate_spare_windows` 中按窗口指数退避 `2 s -> 60 s` 加首次失败告警迁回残留窗口，严禁跨会话复用带窗口备用显示器，消除 `R1-H3`、`R2-L2`、`R2-L3` 与 `R3-L4`。第二，在 `held.rs` 与 `placement.rs` 所有拒绝、超时及多窗口冲突错误文案中按真实持有者数量传入标签，输出客户端、显示器标题、`session` 键、空闲或断连秒数及 `connection open/closed` 状态，并将跨实例虚拟显示器标为 `display {id} [serial {serial}, another Agent Screen instance]`，消除 `R2-L4` 与 `R2-L10`。第三，实现 ADR 0016 同 `session_key` 多连接 `open_connections` 共享保活与 `2 s` 控制连接断连宽限期 `DISCONNECT_GRACE`，在宽限期续绑与重叠连接迁移两条路径上同步重新注册 `InputQueue` 所有者，并在 `Sessions` 与 `agent.rs` 双重拦截已关闭连接重绑，消除 `R2-M1`、`R3-H1` 与 `R3-M1`。菜单栏独立多会话释放列表属于后续产品特性，暂不关闭 `#189`。

### 真实路径测试方案

1. 模型层与回归集成测试：在 `crates/model/src/spares.rs`、`held.rs`、`model.rs`、`placement.rs` 与 `crates/model/tests/round3_regressions.rs` 测试备用池物理与登记窗口隔离、任意调用宽限期续绑与重叠连接保活、重连与重叠连接回退后的 `InputQueue::owner` 权限保持、已关闭连接晚到请求不复活已断连显示器、两个会话并发移交经 `decide_after_hand_over` 二次询问、死亡会话及跨实例 `0x5353` 错误文案 `[已验证]`。
2. 真实 `e2e` 路径测试：在最终应用代码上以 `AGENTSCREEN_INSTANCE=e2e` 运行 `sessiondisplays`，`14` 项检查中通过 `13` 项，实测验证跨会话请求同一窗口返回 `[session ..., idle 1s, connection open]`、断连宽限期内直接调用 `input_queue` 成功续绑显示器并持有队列所有权、宽限期过后显示器保持存活、关闭重叠连接后剩余连接继承显示器与 `InputQueue` 所有权 `[已验证]`。末项 `listWindows` 轮询因脚本在 `250 ms` 宽限期内遇到 `not_found` 提前跳出循环，已在 `sessiondisplays.rs` 修正为持续轮询至 `"ended"`，受虚拟显示器创建预算上限约束未再复跑该脚本改动 `[未验证]`；前序轮次还实测通过了 `windowaccess` `[前序提交已验证]`。

---

## 4. 议题 #190：Peek 卡片勾勒出窗口轮廓但不显示画面内容

### 症状

用户于 2026-11-12 10:08:35 在 `0.1.31` 版本提交 `656ac75` 报告，时间仅比议题 `#189` 晚 2 秒，针对同一个企业微信窗口：鼠标悬停菜单栏打开 Peek 时，Peek 缩略图按窗口真实几何画出了圆角高亮轮廓框，但底图只有深色占位背景与窗口标题文字，完全看不到窗口内容。点击打开 Viewer 后，画面内容显示正常 `[已验证]`。

### 证据

1. 版本历史关键证据：`0.1.31` 提交 `656ac75` 发布时尚未包含当天 12:42 合入的 `PR #193` 提交 `d7bfb40` `fix(app): restart a misrouted display stream so Peek is right` `[已验证]`。在 `0.1.31` 中，当 `ScreenCaptureKit` 采集流被系统停止或因显示器重排发生串流时，后台流不会立即自愈重启，而是等到下一次显示器通知或打开 Viewer 请求高分辨率新流时才重建 `[已验证]`。
2. 当前 `origin/main` 采集流日志证据：`~/Library/Logs/Agent Screen/displays.log` 第 `4206`、`4219-4224`、`4229-4233` 行显示，即使在 `0.1.34` 中，空闲虚拟显示器的 `1 fps` 采集流仍大约每 1 至 3 分钟就会收到 `SCStream error (System stopped the stream): Stream was stopped by the system` `[已验证]`。此外在显示器休眠或重排期间，日志还会连续记录 `No capture source provided` 启动失败 `[已验证]`。
3. 代码对照证据：Peek 窗口边框由 `apps/macos/src/ui/peek.rs` 直接读取 `CGWindowListCopyWindowInfo` 实时窗口几何绘制，不依赖 `ScreenCaptureKit` 帧缓存 `[已验证]`。底图纹理来自 `apps/macos/src/ui/stream.rs` 维护的 `last_frame` `[已验证]`。一旦 `stream.rs` 因连续中断或启动失败清空 `last_frame` 并处于指数退避等待窗口内，此时用户悬停打开 Peek，边框会立刻画出，底图则为空白占位态，直到打开 Viewer 触发尺寸变化或退避到期重启流 `[代码路径已验证，静止画面下仅靠 update_configuration 导致空白的假设未验证]`。

审阅报告 `/tmp/review-208.md` 指出的 `PR #208` 提交 `e239e65` 问题 `[已验证]`：

- `M4`：`e239e65` 假设"已收到首帧的 `1 fps` 温热流在 `update_configuration` 提速到 `12 fps` 时因不推新帧而变空白"，该前提无日志与实验支撑，因为温热流本身已有首帧缓存 `[未验证前提已被证伪]`。同时 `e239e65` 的 `needs_fresh_stream` 在每次打开 Peek 时强制重建所有在线虚拟显示器的 `SCStream`，6 台显示器每次悬停就会在 `replayd` 中触发 6 次耗时 `100 ms` 以上的流重建，严重拖慢 UI 响应 `[已验证]`。
- `M3`：`e239e65` 在 `ui/stream.rs:199-203` 当订阅帧率未变且流处于空闲停止态时直接清零 `failures` 并立即启动。由于 `waitForChange` 每次调用都执行 `want("agent", 30, ...)`，当屏幕录制权限被撤销或显示器不可采集时，每次调用都会绕过 `500 ms -> 30 s` 退避风暴式冲击 `replayd` `[已验证]`。
- `L2`：永久采集失败时不清空旧帧会导致 UI 把过期陈旧画面当作实时画面持续展示，与 `stream.rs:378-381` 注释承诺相悖 `[已验证]`。

### macOS 与相关组件真实行为

1. `ScreenCaptureKit` 与 `replayd` 推帧机制：新建 `SCStream` 调用 `startCapture` 后，无论屏幕内容是否变化，`replayd` 都会推送至少一帧 `SCFrameStatus::Complete` 初始帧 `[已验证]`。此后当画面完全静止时，`replayd` 只发 `Idle` 状态包而不重复拷贝像素缓冲。若静止持续约 1 至 3 分钟，`replayd` 常主动断开虚拟显示器流并返回 `System stopped the stream` `[已验证]`。
2. `CGWindowListCopyWindowInfo` 独立于录屏管道：窗口列表与矩形直接向 `WindowServer` 元数据表查询，即使 `replayd` 流已断开或无帧缓存，窗口边框依然立即可得 `[已验证]`。

### 现有代码的错误假设

1. `0.1.31` 旧代码假设 `SCStream` 一旦建立就会一直存活到显示器销毁，这一假设已在 `#193` 中修正 `[已验证]`。
2. `e239e65` 错误地把"缺少缓存帧"与"提升帧率"混为一谈，在已有缓存帧时每次打开 Peek 重建全部采集流，且在启动失败场景清零了 `failures` 退避计数 `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：每次打开 Peek 都销毁并重建全部显示器的 `SCStream`。否决该方案：即审阅 `M4`，多显示器悬停会造成显著的 `replayd` 重建卡顿 `[已验证]`。
- 方案 B：保留 `origin/main` 现状不做任何修改并直接关闭 `#190`。否决该方案：虽然 `#193` 已修复 `0.1.31` 的主要成因，但在 `origin/main` 上当空闲流刚被系统停止且处于长退避等待期时，新打开的 Peek 若无有效帧仍需等待退避计时器自然到期 `[已验证]`。
- 方案 C：推荐采用轻量唤醒修复，并在未复现真实空白卡片前保持 `Refs #190` 不关闭议题。具体规则：仅当新增交互订阅者如 `"peek"` 或 `"viewer"` 首次加入且当前采集流已停止时，才提前唤醒因空闲 `System stopped the stream` 进入等待的流。严禁清零连续启动失败计数 `failures`，确保权限缺失或无效源仍严格遵守 `500 ms -> 30 s` 指数退避，修复 `M3`。已有运行中的 `1 fps` 流且持有有效缓存帧时，打开 Peek 仅调用 `update_configuration` 原地提速，零流重建，修复 `M4`。若连续启动失败超过上限，按原约定清空过期缓存帧并显示等待画面提示，修复 `L2`。

### 真实路径测试方案

1. 单元测试：在 `apps/macos/src/ui/stream.rs` 同时覆盖纯规则与 `Stream::apply_want` 真实状态接线 `stream_apply_want_wires_backoff_warm_reconfigure_and_pixel_growth`，验证三项不变量：第一，连续 `startCapture` 失败时重复调用 `want("agent", 30, ...)` 遵守 `retry_at` 指数退避窗口而不触发风暴重试 `[已验证]`。第二，已有缓存帧的 `1 fps` 运行流在 `"peek"` 订阅 `12 fps` 时只更新配置而不重建采集流 `[已验证]`。第三，`replayd` 因空闲停止流时保留最后一帧给 Peek 使用，而连续启动失败超限时清空陈旧缓存帧 `[已验证]`。
2. 真实 `e2e` 路径说明：受虚拟显示器创建硬预算限制，流退避与温热帧状态机由 `stream.rs` 单元测试完整覆盖，未额外消耗虚拟显示器重跑 `peekmisroute` `[未验证]`。

---

## 5. 议题 #205：系统设置 Sheet 内的弹出按钮四条交互路径全部失败

### 症状

用户在 `0.1.35` 版本操作"系统设置 -> 通用 -> 时间机器 -> 选项..."弹窗 Sheet 内的"备份频率" `NSPopUpButton` 时，四条操作路径全部失败 `[已验证]`：

1. 调用 `act_on_element` 执行 `set_value` 选择 `"每天"` 报错 `window_not_found: could not bring the target window forward to choose from the pop-up button` `[已验证]`。
2. 改用 `real_input` 按住弹出按钮往下拖拽试图滑选菜单项时，菜单弹出的瞬间报错 `input_aborted: drag left the agent's window at (4842, -395)` `[已验证]`。
3. 弹出菜单保持打开状态下，调用 `real_input` 点击菜单项坐标 `(4842, -370)` 被拒绝，报错 `target_not_on_agent_display: (4842, -370) is not inside an agent window on this display` `[已验证]`。
4. 后台 `click` 虽然能点开弹出按钮，但后续后台点击菜单项偶尔无效，点击菜单外空白区域也无法关闭已打开的菜单 `[已验证]`。

### 证据

逐条核对 `origin/main` 源码定位到四处精确断点 `[已验证]`：

1. ExtensionKit `.appex` 进程未被识别为远程视图服务：macOS Ventura 及之后的系统设置各面板如 `TimeMachineSettings.appex` 运行在独立的 `.appex/Contents/MacOS/` 扩展进程中 `[已验证]`。`apps/macos/src/mac/cg.rs:268` 的 `is_xpc_service` 只检查路径是否包含 `".xpc/Contents/MacOS/"`，漏掉了 `".appex/Contents/MacOS/"` `[已验证]`。同时 `apps/macos/src/mac/app.rs:155` 的 `bundle_of` 对无常规应用包的扩展进程无法解析包路径，导致窗口归属 `belongs` 与控件反查宿主窗口失配 `[已验证]`。
2. `AXSheet` 角色与非前台焦点窗口查询失败：`apps/macos/src/mac/ax.rs:948` 附近窗口枚举与 `apps/macos/src/control/agent.rs:4573` 的 `popup_window` 只接受 `AXRole == "AXWindow"` 的顶层节点，拒绝了 `AXRole == "AXSheet"` 的模态表单 `[已验证]`。当应用不在前台时，AppKit 对含 Sheet 的后台应用查询 `kAXFocusedWindowAttribute` 常返回空值，导致 `ax.rs:241` 的 `focused_window_id` 返回 `None`，`choose_closed_popup` 判定置前失败报错 `window_not_found` `[已验证]`。此外对于 `.appex` 托管的弹出按钮，键盘上下键与回车必须投递给 `.appex` 服务进程的 `pid`，发给宿主 `System Settings` 进程无效 `[已验证]`。
3. `real_input` 拖拽中途遇到 `layer == 101` 菜单被判越界：AppKit 弹出菜单窗口位于 `kCGPopUpMenuWindowLevel` 即 `layer == 101` `[已验证]`。`apps/macos/src/mac/input.rs:2271` 的 `DragScene` 只收集 `layer == 0` 窗口，菜单在 `(4842, -395)` 弹出遮住底层 Sheet 后，`DragScene::under` 认为指针离开了代理窗口并立即中止拖拽 `[已验证]`。
4. `real_input` 点击菜单被 `window_at` 拦截：`apps/macos/src/mac/input.rs:2404` 的 `window_at` 同样只接受标准窗口层级，拒绝了对 `layer == 101` 已打开弹出菜单的点击授权 `[已验证]`。

审阅报告 `/tmp/review-208.md` 指出的 `PR #208` 提交 `e239e65` 问题 `[已验证]`：

- `M2`：`e239e65` 直接修改了全局函数 `ax::focused_window_id`，当 `AXFocusedWindow` 为空时一律回退到 `AXFocusedUIElement` 所在窗口，并在未设置 `0.1 s` 超时的父节点上最多向上遍历 16 层 `[已验证]`。这改变了所有既有调用方的语义，包括议题 `#110` 共享浏览器焦点守卫 `control/shared_app.rs:53, 230`、`key_window_now` 以及 `focus_waking`，还破坏了卡死应用 `0.1 s` 超时的承诺 `[已验证]`。
- `L5`：`e239e65` 的 `popup_window` 可能把宿主进程的 `pid` 与 `.appex` 进程名下的 `CGWindowID` 拼成一对不存在的 `WindowTarget` `[已验证]`。
- `M5`：`e239e65` 的 `menuchoice` 测试只测了单进程 AppKit 窗口，没有覆盖真实的 `.appex` 扩展进程 `[已验证]`。

### macOS 与相关组件真实行为

1. AppKit Sheet 层级结构：通过 `-[NSWindow beginSheet:completionHandler:]` 展示的 Sheet 在 `WindowServer` 中拥有独立的 `CGWindowID`，在无障碍树中表现为 `AXRole == "AXSheet"`，其 `AXParent` 指向宿主 `AXWindow` `[已验证]`。来源为 Apple `NSAccessibilityRoles` 文档及实测验证 `[已验证]`。
2. ExtensionKit 视图托管：系统设置宿主进程负责外框窗口，具体设置面板由 `.appex` 扩展进程通过远程视图桥接渲染。无障碍元素命中的 `pid` 是 `.appex` 进程，键盘事件也由 `.appex` 进程消费 `[已验证]`。
3. AeroSpace 对测试扩展进程的平铺干扰：实测发现若自定义 `.appex` 测试进程将 `NSApplication.ActivationPolicy` 设为 `.regular` 且创建 `layer == 0` 窗口，而其包路径不在 AeroSpace 豁免清单内，AeroSpace 会强行把该窗口平铺拉到用户主屏幕 `(1488, 33)` `[已验证]`。真实的远程视图扩展使用 `.accessory` 激活策略且不独立创建主屏标准窗口 `[已验证]`。
4. 弹出菜单窗口层级：Apple `CGWindowLevel.h` 定义 `kCGPopUpMenuWindowLevel = 101` `[已验证]`。所有原生 `NSMenu` 与 `NSPopUpButton` 弹出列表都运行在该层级 `[已验证]`。

### 现有代码的错误假设

1. 假设所有远程视图子进程都以 `.xpc` 结尾，忽略了现代 macOS 广泛采用的 `.appex` 扩展包 `[已验证]`。
2. 假设可交互的顶层表单只有 `AXWindow`，忽略了 `AXSheet` `[已验证]`。
3. 假设 `real_input` 期间代理窗口上方出现的任何 `layer == 101` 窗口都是外部危险遮挡 `[已验证]`。实际上只要该菜单窗口属于当前显示器上已授权宿主进程或其关联 `.appex/.xpc` 服务进程，且矩形落在该虚拟显示器内，它就是代理正在操作的合法弹出菜单 `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：放宽 `focused_window_id` 全局回退，并允许 `real_input` 点击虚拟显示器上任意进程的 `layer == 101` 窗口。否决该方案：全局修改 `focused_window_id` 会触发审阅指出的 `M2` 破坏 `#110` 焦点守卫。不校验进程归属放行 `layer == 101` 则可能误点系统通知或菜单栏全局弹层 `[已验证]`。
- 方案 B：推荐采用隔离式精准修复。第一，将 `cg::is_xpc_service` 扩展为 `cg::is_remote_view_service`，同时匹配 `.xpc/Contents/MacOS/` 与 `.appex/Contents/MacOS/`，并在 `app::bundle_of` 中通过 `proc_pidpath` 向上解析 `.app`、`.appex` 或 `.xpc` 包根目录。第二，保持 `ax::focused_window_id` 原有实现一行不改，保护 `#110` 与焦点恢复逻辑，直接消除 `M2`。新增专用函数 `ax::focused_window_or_sheet(pid)`，对每一步遍历的 `AXParent` 显式设置 `0.1 s` 超时，仅在 Sheet 置前与 `choose_closed_popup` 路径调用。第三，修正 `popup_window` 使返回的 `WindowTarget` 始终使用实际拥有该 `CGWindowID` 的进程 `pid` 或宿主窗口真实的 `(host_pid, host_wid)`，消除 `L5`，同时将按键发往控件所属的 `element.pid()`。第四，修订 ADR 0013 并更新 `DragScene`、`real_drag_window_at` 与 `real_click_at`，仅放行同一虚拟显示器内属于代理窗口进程或其关联远程视图进程的 `AXSheet`、`layer 0..=8` 浮动面板与 `layer == 101` 弹出菜单 `[原型修复已通过编译与 e2e 验证]`。

### 真实路径测试方案

1. 单元测试：在 `crates/model/src/real_drag.rs` 与 `apps/macos/src/mac/input.rs` 测试 `resolve_drag_under`、`resolve_real_click` 与 `menu_owner_belongs_to_display` 对同显示器关联菜单放行、对跨显示器或无关进程菜单拦截，测试 `popup_window` 不会产出跨进程错配的 `WindowTarget` `[已验证]`。
2. 真实 `e2e` 路径测试：构建内嵌真实 `MenuPane.appex` 插件的宿主夹具 `PaneHost.app`，先在 `origin/main` 上运行 `menuchoice` 确认 `.appex` 弹出菜单与外部点击关闭失败退出 `exit=1`，再在分支代码上运行 `AGENTSCREEN_INSTANCE=e2e` 验证通过：Sheet 内 `NSPopUpButton` 的 `act_on_element` 选择成功、跨 `PaneHost -> MenuPane.appex` 的菜单点击选择成功、菜单外部点击 `AXCancel` 关闭成功、全程用户主屏幕零窗口污染 `[已验证]`。对于移动真实硬件指针的 `real_input` 路径，留待用户允许移动真实指针时运行 `[未验证]`。

---

## 6. 议题 #207：Takeover 接管模式下在 Viewer 中无法选择或关闭已打开的弹出菜单

### 症状

用户在 `0.1.35` 版本接管某台显示器进入 Takeover 模式后，在 Viewer 窗口内点击"系统设置 -> 时间机器 -> 选项..."中的"备份频率"弹出菜单：菜单包含"从不、每天、每周"等选项，用户在 Viewer 中点击任意菜单项均无反应，菜单既不选中也不收起。点击菜单外部空白处同样无法关闭菜单，导致整个弹窗被卡住，只能在真实屏幕上打开系统设置操作 `[已验证]`。

### 证据

核查 `origin/main` 中 Viewer 点击转发与后台菜单点击代码 `[已验证]`：

1. Viewer 点击走相同的 `InputWorker` 后台事件管道：在 Takeover 期间，用户在 Viewer 画布上的点击经 `apps/macos/src/mac/viewer_input.rs` 转换坐标后送入 `apps/macos/src/mac/input.rs:1565` 的 `Worker::mouse_down`，仅携带 `user_press = true` 标记，底层依然通过 `SkyLight` 后台事件与 AX 辅助动作驱动目标进程，不会移动真实鼠标指针 `[已验证]`。
2. 跨进程 `.appex` 菜单命中缺失：当弹出菜单由 `TimeMachineSettings.appex` 弹出时，`CGWindowListCopyWindowInfo` 顶层或底层窗口的宿主 `pid` 是 `System Settings`，而菜单项无障碍节点由 `.appex` 进程响应 `[已验证]`。旧代码只向单一命中 `pid` 查询 `element_at`，无法命中 `.appex` 菜单项 `[已验证]`。
3. 菜单项内部子节点未向上归并：点击落在菜单项文字标题或左侧对勾区域时，`AXUIElementCopyElementAtPosition` 有时返回内部文本子节点而非外层 `AXMenuItem`，直接对其调 `AXPress` 不生效 `[已验证]`。
4. 点击菜单外部无法退出 `NSMenu` 模态循环：当点击落在虚拟显示器的菜单外部区域或桌面空白处时，`input.rs:1565` 将鼠标按下发给底下窗口或因空白处无窗口而丢弃 `[已验证]`。处于后台非激活状态的 `NSMenu` 碳菜单追踪循环不会因后台 `SLEventPostToPid` 发往其他窗口的点击而自动收起菜单 `[已验证]`。

审阅报告 `/tmp/review-208.md` 指出的 `PR #208` 提交 `e239e65` 问题 `[已验证]`：

- `L6`：`e239e65` 新增的 `press_menu_item_at` 与 `cancel_menu_at` 向上遍历 8 层 `AXParent`，若用户点在子菜单内边距空白处，可能误触父级菜单项。同时它在输入工作线程内对该显示器上所有进程逐一调默认 `1.0 s` 超时的 AX 命中查询，有卡顿风险 `[已验证]`。
- `M5`：`e239e65` 只用代理的后台 `click` 工具测试，从未在 `debugTakeover` 开启状态下通过真实 Viewer 输入钩子 `debugViewerInput` 验证 `user_press = true` 的完整路径 `[已验证]`。

### macOS 与相关组件真实行为

1. `NSMenu` 追踪循环与无障碍动作：原生 `NSMenu` 打开后进入局部事件追踪循环。对于非激活进程，后台投递的普通鼠标事件常被菜单窗口忽略，但无障碍接口对 `AXRole == "AXMenuItem"` 执行 `kAXPressAction` 会可靠触发菜单选中并关闭菜单，对 `AXRole == "AXMenu"` 或 `"AXMenuBar"` 执行 `kAXCancelAction` 会可靠关闭已打开的菜单且不激活应用 `[已验证]`。来源为 Apple `NSAccessibilityActions` 文档及实测验证 `[已验证]`。

### 现有代码的错误假设

1. 假设向菜单外部投递后台 `LeftMouseDown` 就能像真实前台点击一样收起已打开的 `NSMenu` `[已验证]`。
2. 假设菜单窗口的进程 `pid` 一定等于底下宿主窗口的 `pid` `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：在 Takeover 点击菜单时临时激活目标应用或借用真实指针点击。否决该方案：直接违反 `CLAUDE.md` 与 ADR 0001 核心红线，用户在 Viewer 中点击绝不能抢走物理屏幕焦点或移动真实鼠标指针 `[已验证]`。
- 方案 B：推荐采用受限层级的 AX 菜单动作与外部点击取消机制。具体做法：第一，仅当命中或当前显示器存在 `layer == 101` 弹出菜单窗口时，才进入菜单命中与取消分支，日常点击零额外开销。第二，`resolve_menu_pids` 只收集该 `layer == 101` 菜单窗口自身的进程 `pid` 及其精确匹配的远程视图宿主进程，无显式远程面板匹配时仅取菜单矩形 `48 pt` 邻域内最顶层的一个宿主窗口进程，绝不向同屏无关应用发送 `AXCancel`，每个 `AXUIElement` 显式设置 `0.15 s` 消息超时，消除 `R1-L6` 与 `R2-L7`。第三，在 `press_menu_item_at` 中最多向上查找 `2` 层 `AXParent` 且遇到 `AXMenu` 立即停止，在 `cancel_menu_at` 中最多向上查找 `4` 层 `AXParent` 对 `AXMenu` 或 `AXMenuBar` 发送 `AXCancel` 收起菜单 `[已在 Takeover Viewer 点击路径验证通过]`。

### 真实路径测试方案

1. 真实 `e2e` 路径测试：在 `AGENTSCREEN_INSTANCE=e2e` 下启动 `PaneHost.app` 与内嵌 `MenuPane.appex`，先打开弹出菜单，调用 `debugTakeover: true` 进入真实的 Takeover 状态，再通过 `debugViewerInput` 发送 `"click"` 模拟用户在 Viewer 画布上的真实点击：第一步验证点击 `.appex` 菜单项能成功更新选中值并收起菜单。第二步再次点开菜单，验证在 Viewer 内点击菜单外空白区域能通过 `AXCancel` 立即关闭菜单且 `layer == 101` 窗口消失 `[已在 viewerdrag 实测绿灯通过]`。

---

## 7. 议题 #206：等待用户审批时在主屏幕显示全屏呼吸红框与红色遮罩

### 症状

议题 `#206` 是一项产品增强提议：当代理在后台发起权限审批请求，如 `Window Authorization`、`Window Request`、`Input Request` 或 `Help Request`，而用户正在全屏专注其他工作未留意右上角通知或菜单栏图标时，代理会一直挂起等待 `[已验证]`。提议希望在等待用户审批期间，在用户主屏幕四周展示 `6-10 px` 呼吸脉冲红框，并叠加 `8%-12%` 不透明度的全屏点击穿透红色遮罩，在用户点击批准或拒绝后立即消失 `[已验证]`。

### 证据

核查仓库架构决策记录与现有覆盖层实现 `[已验证]`：

1. ADR 语义冲突：`docs/adr/0013-real-input-grant.md` 第 `27` 行明确规定：当且仅当 `Input Grant` 处于生效执行期，即代理此刻正握有用户真实鼠标指针进行短暂前台拖拽或点击时，屏幕边缘才显示细红框，向用户传达唯一明确的安全契约：代理正在控制你的指针，晃动鼠标即可立即夺回控制权 `[已验证]`。
2. 现有代码实现：`apps/macos/src/ui/` 中的红框覆盖层严格绑定在真实输入执行期内，等待用户点击审批卡片的阶段没有任何全屏遮罩 `[已验证]`。

### macOS 与相关组件真实行为

在 macOS 上通过 `NSWindow` 配合 `ignoresMouseEvents = true`、`NSWindowCollectionBehaviorCanJoinAllSpaces | FullScreenAuxiliary` 及高窗口层级，技术上可以实现全屏点击穿透遮罩 `[已验证]`。但全屏 `8%-12%` 红色染色会改变整个物理屏幕所有应用包括设计稿、终端与视频的色彩呈现，且一旦与 `Input Grant` 共用红色边框视觉语言，用户将无法分辨"代理只是在安静等待你点按钮"与"代理正在移动你的鼠标" `[已验证]`。

### 现有代码的错误假设

此处并非现有代码实现缺陷，而是新提议假设"等待审批提示"可以复用"真实指针已被接管"的红色全屏警报通道 `[已验证]`。

### 根因修复方案对比与推荐决策

- 方案 A：按议题原样实现等待期间全屏呼吸红框加 `8%-12%` 红色全屏遮罩。否决该方案：直接破坏 ADR 0013 的安全信号唯一性，且全屏染色严重干扰用户正常阅读与创作，多会话并发请求时还会造成主屏幕频繁变红闪烁 `[已验证]`。
- 方案 B：使用琥珀色或蓝色边缘窄光晕提示等待审批，不加全屏染色遮罩，保留红色专用于 `Input Grant` 指针接管期。该方案避免了与 ADR 0013 红框混淆，但仍需覆盖用户物理屏幕边缘 `[设计可行，需产品拍板]`。
- 方案 C：已按拍板决策落地非侵入式强提醒，在 PR 中以 `Refs #206` 关联议题并明确说明因 ADR 0013 拒绝全屏红框与全屏染色，消除 `R2-M5` `[已验证]`。当存在未决审批请求如 `Window Authorization`、`Window Request`、`Input Request` 或 `Help Request` 时，菜单栏图标切为醒目橙色并展示实时待办计数徽标，审批面板卡片渲染琥珀色高亮外框与顶部"待你批准"状态徽章，同时为 `Note::Input` 补齐受系统通知设置与专注模式统一管辖的 `UNNotificationSound` 提示音，并移除不受专注模式约束且会重复发声的额外 `NSSound` 提示音，消除 `R2-M2` 与 `R2-L6`。红色屏幕边框严格保留给 ADR 0013 的真实指针接管期。

### 真实路径测试方案

1. 模型层与调试接口验证：在 `crates/model/src/card.rs` 单测全部四类待审卡片的高亮判定，在 `AGENTSCREEN_INSTANCE=e2e` 下运行 `windowaccess`，通过 `debugIcon` 与 `debugCard` 实测验证发起审批时 `state == "asking"`、`askingCount == 1` 且 `highlighted == true`，审批完成后计数归零且按钮命中几何保持原位不变 `[已验证]`。

---

## 8. 议题 #110：Dia 浏览器位于 Agent Display 时按键数秒后被拒且链接点击静默失效

### 症状

用户于早期版本报告，将基于 Chromium 架构的 Dia 浏览器窗口放到 Agent Display 后出现两类异常 `[已验证]`：

1. 初始几秒内可以向地址栏或网页输入框发送键盘输入，几秒后按键开始被拒绝或失效 `[已验证]`。
2. 在网页内点击链接静默无响应，页面不跳转也不报错 `[已验证]`。

### 证据

核查仓库历史提交与当前 `origin/main` 代码 `[已验证]`：

1. 已合入的四轮根因修复：针对用户在主屏幕使用同一浏览器另一窗口时 Agent Display 窗口的共享进程输入隔离，仓库已在 `PR #119`、`#129`、`#153` 与 `#185` 中系统重构了 `apps/macos/src/control/shared_app.rs` 与 `apps/macos/src/mac/input.rs` `[已验证]`。具体包括：通过 `shared_app.rs:53, 230` 检查进程当前 `focused_window_id` 是否落在用户屏幕上，防止后台按键串入用户正在主屏输入的同进程浏览器窗口。以及针对 Chromium 多进程渲染树改进无障碍点击与坐标派发 `[已验证]`。
2. 审阅报告 `M2` 关联风险验证：在 `PR #208` 提交 `e239e65` 中，由于误改了 `ax::focused_window_id` 使其回退到 `AXFocusedUIElement`，`shared_app.rs:53, 230` 的焦点守卫行为被意外改变 `[已验证]`。保持 `focused_window_id` 原语义不变是守住 `#110` 既有修复的前提 `[已验证]`。
3. 真实 Dia 实例验证状态：仓库 `tests/e2e` 中的 `sharedbrowser` 脚本已在基于 Electron/Chromium 的独立测试浏览器夹具上验证了同进程双窗口隔离与点击跳转 `[已验证]`。但由于安全约束严禁自动化脚本触碰用户正在运行的真实浏览器会话，上述修复在最新版本 Dia 商业包上的表现尚未做最终人工回归 `[未验证]`。

### macOS 与相关组件真实行为

1. macOS 应用级键盘焦点与同进程多窗口：macOS 的主焦点以进程为单位，同一浏览器进程同时在用户物理屏幕与虚拟显示器各开一个窗口时，一旦用户在物理屏幕点击自己的浏览器窗口，该进程的 `AXFocusedWindow` 即切到用户窗口 `[已验证]`。若此时代理继续通过 `SLEventPostToPid` 向该进程投递键盘事件，按键会直接打进用户正在输入的窗口，因此 `shared_app.rs` 必须在检测到焦点切回用户窗口时拦截后台按键或改走安全的无障碍设值路径 `[已验证]`。
2. Chromium 无障碍按压与后台鼠标合成：Chromium 默认在收到无障碍树查询后才按需构建完整的 `AXWebArea` 子树，且部分自定义前端路由组件仅监听合成鼠标点击或需要目标渲染视图获得内部焦点才能响应跳转 `[已验证]`。

### 现有代码的错误假设

早期版本假设只要目标窗口位于虚拟显示器上，向其 `pid` 发送 `SLEventPostToPid` 键盘和鼠标事件就只会作用于该后台窗口，忽略了同进程用户窗口抢占 `AXFocusedWindow` 后的串窗问题以及 Chromium 渲染进程的焦点门控 `[已验证]`。该假设已在 `shared_app.rs` 系列 PR 中修正，但 Dia 特有标签栏或命令面板是否存在额外自定义拦截仍待实测 `[未验证]`。

### 根因修复方案对比与推荐决策

推荐决策：第一，在本次修复中严格保持 `ax::focused_window_id` 原语义不变，避免引入审阅 `M2` 回归。第二，不盲目关闭 `#110`，保留议题开启并列出明确的人工验收清单，待用户在空闲时用单个独立 Dia 窗口按清单验收通过后再关闭。若验收仍复现特定元素点击失效，再抓取该元素的 `inspect_ui` 角色树精准修补。

### 真实路径测试方案

1. 自动化回归测试：在 `AGENTSCREEN_INSTANCE=e2e` 下运行现有 `sharedbrowser` 脚本，验证同进程双窗口下用户主屏窗口聚焦时后台输入安全隔离且不丢字、链接点击正常跳转 `[既有套件已覆盖]`。
2. Dia 真实应用人工验收步骤：在 Agent Display 放入一个空白测试标签页的 Dia 窗口，同时在用户主屏打开另一个 Dia 窗口并点击主屏地址栏。让代理对虚拟显示器上的 Dia 窗口连续执行地址栏输入、等待 10 秒后再次输入网页文本框、点击普通 `<a>` 链接与 SPA 前端路由链接，确认主屏不串字且虚拟显示器页面成功跳转 `[待人工验收]`。

---

## 9. 日志异常一：`unit twin` 虚拟显示器双生创建

### 症状

`~/Library/Logs/Agent Screen/displays.log` 多次记录新建虚拟显示器时一次冒出多台显示器并以 `released ... unit twin` 立即销毁多余实例，每次创建耗时拉长并伴随主线程 `WindowServer` 通知阻塞 `[已验证]`。

### 证据

核查 `displays.log` 与 `apps/macos/src/mac/display.rs:167` 的 `VirtualDisplay::create` 实现 `[已验证]`：

1. 运行期单元号空洞触发撞号：第 `1608` 行记录 `display=210` 持有 `unit=8` 在线。随后 `unit=6` 在 08:06 前已释放留下空洞，此时在线虚拟显示器为 `unit=7` 与 `unit=8` `[已验证]`。第 `1656` 至 `1659` 行记录：
   - `2026-11-11 08:06:33.914 created display=218 unit=8 at=-1280,0 size=1280x800 serial=3`
   - `2026-11-11 08:06:34.345 created display=219 unit=9 at=-1280,0 size=1280x800 serial=4`
   - `2026-11-11 08:06:34.345 released display=218 unit=8 at=-2560,0 size=1280x800 unit twin`
   新建的 `display=218` 被分配了与在线 `display=210` 相同的 `unit=8`，`VirtualDisplay::create` 检测到撞号后继续创建占位显示器填洞直到拿到不冲突的 `unit=9`，再释放双生显示器 `218` `[已验证]`。
2. 应用刚启动无备用池时同样出现双生：审阅报告 `L7` 与 `L8` 指出的第 `1605` 至 `1615` 行证明，备用池淘汰顺序并非唯一成因 `[已验证]`：
   - 第 `1605` 行 `03:13:30.039 app started pid=88013 version=0.1.29`，此时应用刚启动，备用池为空 `[已验证]`。
   - 第 `1608` 行 `03:13:59.362 created display=210 unit=8`，说明系统中已有其他虚拟显示器如 AeroSpace 占用了部分低单元号且中间存在空洞 `[已验证]`。
   - 第 `1611` 至 `1615` 行在 `03:14:01` 创建备用显示器时连续触发 `display=211 unit=9` 与 `display=212 unit=10` 两次 `unit twin`，最终保留 `display=213 unit=11` `[已验证]`。
3. 备用池淘汰逻辑加剧了空洞：`origin/main` 的 `apps/macos/src/core.rs:301` 函数 `tidy` 使用 `old.pop()` 保留最后放入的高单元号显示器、释放较早创建的低单元号显示器，主动在低单元号位置制造空洞 `[已验证]`。

### macOS 与相关组件真实行为

`WindowServer` 为新建 `CGVirtualDisplay` 分配 `CGDisplayUnitNumber` 时，并不扫描最小未占用整数，而是按基础起始单元号加上当前在线虚拟显示器数量计算 `[已验证，由 08:06:33 撞上在线 unit=8 及 display.rs 填洞实测确认]`。只要低单元号存在释放留下的空洞，新分配出的单元号就会撞上仍在线的高单元号虚拟显示器，产生共享同一 `unit` 的双生显示器 `[已验证]`。两台同 `unit` 显示器在 `WindowServer` 与 `ScreenCaptureKit` 中会互相串流，必须靠创建临时填洞显示器跨越撞号区间后再销毁填洞者 `[已验证]`。

### 现有代码的错误假设

1. `origin/main` 的 `Core::tidy` 假设释放哪一台备用显示器都等价，因而优先释放旧的低单元号备用显示器，人为制造单元号空洞 `[已验证]`。
2. `PR #208` 提交 `e239e65` 的变更日志声称"消除了重复虚拟显示器"，这属于过度承诺 `[已验证]`。如审阅 `L7` 所述，当在用会话显示器本身乱序结束或外部进程如 AeroSpace 在启动前留下空洞时，备用池排序无法阻止空洞产生，`VirtualDisplay::create` 现有的填洞重试机制依然是必不可少的底层防线 `[已验证]`。

### 根因修复方案对比与推荐决策

推荐决策：保留 `VirtualDisplay::create` 现有的填洞检测与双生释放机制不动。在备用池管理中采用修正了 `H3` 与 `L4` 的纯模型规则 `crates/model/src/spares.rs`，按最小单元号优先复用匹配的无窗口备用显示器，在淘汰超量或过期无窗口备用显示器时优先释放最高单元号的尾部显示器，减少自发制造的单元号空洞。同时在文档与变更日志中准确表述为"减少备用池回收引发的单元号碰撞重试"，绝不声称彻底消除 `unit twin`。

### 真实路径测试方案

1. 模型层单元测试：在 `crates/model/src/spares.rs` 测试 `pick_spare` 优先选取最小 `unit` 的无窗口备用显示器，测试 `keep_spares` 从高 `unit` 端向低 `unit` 端回收无窗口过期显示器 `[已验证]`。
2. 运行期观测验证：在 `AGENTSCREEN_INSTANCE=e2e` 复用备用显示器场景下检查 `displays-e2e.log`，确认连续创建、结束、再创建同尺寸显示器直接命中 `reused` 而不触发额外 `CGVirtualDisplay` 创建 `[已在 sessiondisplays 实测通过]`。

---

## 10. 日志异常二：`displays.log` 中的 30 次 `SCStream misroute` 画面错位

### 症状

用户环境日志 `~/Library/Logs/Agent Screen/displays.log` 在合入 `#193` 后记录了多条 `misroute` 警告，表明某台虚拟显示器的 `ScreenCaptureKit` 采集流收到的帧内容矩形 `shows=...` 与该显示器当前的期望视口矩形 `at=... size=...` 不一致，触发了采集流自动重启 `[已验证]`。

### 证据

精确统计并分类 `~/Library/Logs/Agent Screen/displays.log` 中的全部记录 `[已验证]`：

1. 准确数量纠正：任务背景与旧版调研文档写作 `27` 次，实测 `rg -c "misroute" ~/Library/Logs/"Agent Screen"/displays.log` 共匹配 **30 条**记录，分布在 2026-11-13 11:58 至 2026-11-14 00:04 之间 `[已验证]`。
2. 第一类成因是应用自身创建或重排显示器引发的视口跳变：如第 `2802` 行 `14:18:32`、第 `2863` 行 `16:42:24`、第 `2896` 行 `17:00:36` 及第 `3015` 行 `21:04:05.077 misroute display=379 unit=10 at=4288,-1700 size=1280x800 shows=5568,-1700 1280x800 twin=None`，紧邻新显示器创建与 `repark` 槽位重算 `[已验证]`。
3. 第二类成因是物理显示器拓扑切换引发的整体平移：如审阅报告 `L8` 所指出，大量记录显示整排虚拟显示器的 `X` 坐标精确平移了 `1728 pt`，恰好等于用户 MacBook 内置 Retina 显示器的逻辑宽度 `1728x1117` `[已验证]`。典型日志包括：
   - 第 `2935` 行 `18:51:09.357 misroute display=321 unit=6 at=4288,-900 size=1440x900 shows=1728,-900`
   - 第 `3295` 行 `22:05:56.775 misroute display=379 unit=10 at=6016,-800 size=1280x800 shows=4288,-800`，注意 `6016 - 4288 = 1728` `[已验证]`。
   - 第 `3326` 行 `22:08:12.346`、第 `3400` 行 `22:22:31.013`、第 `3926` 行 `00:04:14.190` 同样呈现 `4288 <-> 6016` 的 `1728 pt` 往返位移 `[已验证]`。
4. 第三类成因是双生显示器短暂共号：第 `3154` 行记录 `21:53:42.398 misroute display=381 unit=11 at=10016,-800 size=1280x800 shows=5568,-1700 1280x800 twin=Some(425)` `[已验证]`。

### macOS 与相关组件真实行为

`ScreenCaptureKit` 在每帧 `CMSampleBuffer` 附件 `SCStreamFrameInfoContentRect` 或显示矩阵中报告该帧捕获时的源屏幕坐标。当 `CGConfigureDisplayOrigin` 或外接显示器插拔开合盖改变桌面布局时，`replayd` 管道中在途的旧帧仍携带旧视口坐标，或者在物理拓扑重排后把采集流错绑到相邻屏幕 `[已验证]`。`PR #193` 引入的 `misroute` 检测正是对比帧元数据与当前显示器状态并在失配时重建 `SCStream` `[已验证]`。

### 现有代码的错误假设

旧版调研报告错误地将全部 `misroute` 都归咎于后台 `make_spare` 重排，忽略了 `1728 pt` 物理屏幕拓扑切换占了相当比例 `[已验证]`。因此固定已停靠显示器只能消除第一类自发重排带来的 `misroute`，不能替代 `ui/stream.rs` 的错位检测与自愈机制 `[已验证]`。

### 根因修复方案对比与推荐决策

推荐决策：通过修复 `#204` 固定已停靠显示器位置，消除应用自发重排制造的第一类 `misroute`。完整保留 `ui/stream.rs` 中的 `misroute` 检测与立即重建逻辑，用来应对外接显示器插拔、笔记本开合盖及唤醒导致的第二类真实拓扑平移。

### 真实路径测试方案

在 `AGENTSCREEN_INSTANCE=e2e` 下运行 `sessiondisplays` 与 `viewerdrag` 时检查 `displays-e2e.log`，确认新建第二台虚拟显示器时第一台已停靠显示器保持原坐标不动且零条 `misroute` 日志产生 `[已验证]`。`AGENTSCREEN_TEST_MISROUTE` 注入下的流重建自愈已在既有套件中验证，本轮受 `20` 台虚拟显示器硬预算限制未额外消耗显示器复跑 `peekmisroute` `[未验证]`。

---

## 11. 日志异常三：虚拟显示器掉回 `800x500` 分辨率模式

### 症状

虚拟显示器在创建或进入备用池后，逻辑分辨率从请求的 `1280x800` 或 `1600x1000` 掉回 `800x500` 低分辨率模式，并进而引发 `800x500` 与目标尺寸之间的 `misroute` 重试 `[已验证]`。

### 证据

核查全部日志文件与 `Core::hold_modes` 源码 `[已验证]`：

1. 日志来源纠正：如审阅报告 `L8` 指出，用户日常实例日志 `~/Library/Logs/Agent Screen/displays.log` 中 `800x500` 的匹配数为 **0 条**。全部 `800x500` 记录均出自测试实例日志 `~/Library/Logs/Agent Screen/displays-e2e.log` 与 `displays-e2e.old` `[已验证]`。典型记录见 `displays-e2e.log`：
   - 第 `306` 行：`2026-11-13 23:01:41.268 mode display=570 unit=9 at=8576,-800 size=1280x800 ran at 800x500, wants 1280x800, back: true` `[已验证]`。
   - 第 `361` 行：`2026-11-13 23:02:18.647 kept display=570 unit=9 at=8576,-800 size=800x500 spare` `[已验证]`。
   - 第 `376` 行：`2026-11-13 23:03:56.305 created display=581 unit=9 at=-800,0 size=800x500 serial=10002`，部分 `e2e` 脚本显式请求或复用了 `serial=10002` 的小尺寸模式 `[已验证]`。
   - 第 `756` 行：另一台备用显示器同样以 `size=800x500` 留在 `spare` 池中 `[已验证]`。
2. 代码盲区证据：`origin/main` 的 `apps/macos/src/core.rs:490` 函数 `hold_modes` 在收到显示器重配置通知时，只遍历 `self.displays.list` 中的在用与待审显示器，检查其当前模式是否偏离 `wanted_mode` 并切回目标 HiDPI 模式。`self.spares` 备用池中的显示器完全不在 `hold_modes` 巡检范围内 `[已验证]`。因此一旦显示器在转入备用池前后被 `WindowServer` 切到 `800x500` 或 `800x600`，它就会一直顶着错误分辨率留在备用池中，引发视口尺寸变化与复用意图失配 `[已验证]`。

### macOS 与相关组件真实行为

`WindowServer` 按显示器厂商号 `vendorNumber`、产品型号 `modelNumber` 与序列号 `serialNumber` 三元组在系统配置中记忆历史显示模式 `[已验证，见 ADR 0009 与 holdmode 实测]`。当同一 `serial` 曾运行在低分辨率模式，或在密集重配置压力下回退到非 HiDPI 低分辨率档位时，`WindowServer` 会把该虚拟显示器切回旧模式，必须显式调用 `CGDisplaySetDisplayMode` 切回期望的 `2x` HiDPI 模式 `[已验证]`。

### 现有代码的错误假设

`Core::hold_modes` 假设只有 `self.displays.list` 中的活跃显示器才需要维持目标分辨率，忽略了在线备用显示器同样参与桌面几何拼接与后续会话快速复用 `[已验证]`。

### 根因修复方案对比与推荐决策

推荐决策：扩展 `Core::hold_modes`，对 `self.spares` 中的每台备用显示器读取其 `wanted_mode()`，采用与在用显示器完全一致的模式校验与 `CGDisplaySetDisplayMode` 恢复逻辑，使备用显示器在空闲期间也始终保持目标 `2x` 分辨率 `[已验证]`。

### 真实路径测试方案

在 `AGENTSCREEN_INSTANCE=e2e` 下扩展现有 `tests/e2e/src/bin/holdmode.rs`。第一步验证在用显示器被手动切到 `800x600` 后自动恢复 `1600x1000` `2x`。第二步调用 `endDisplay` 让该显示器转入空闲备用池，再次将其模式强制切到 `800x600`，断言 `Core::hold_modes` 自动将这台备用显示器切回 `1600x1000` `2x` `[已在 holdmode 实测通过，displays-e2e.log 输出 ran at 800x600, wants 1600x1000, back: true]`。
