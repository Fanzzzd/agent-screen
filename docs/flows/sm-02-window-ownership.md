# 状态机 02：窗口所有权与几何对账（ADR 0018）

Agent Screen 将"逻辑所有权记录 `Held`"与"`WindowServer` 实时屏幕几何位置"解耦建模，通过周期及事件驱动对账消除目标应用自发跳屏、用户拖拽与会话切换造成的状态漂移。

## 状态图

```mermaid
stateDiagram-v2
    [*] --> UserScreen : "应用在用户屏幕创建窗口"
    [*] --> UnclaimedCatch : "Catch 捕获新窗口通知"

    UnclaimedCatch --> UserScreen : "ADR 0007 判定归属用户"
    UnclaimedCatch --> SettlingToAgent : "ADR 0007 判定归属代理会话"

    UserScreen --> PendingAccess : "代理调用 place_window / window_access"
    PendingAccess --> UserScreen : "用户拒绝或请求取消"
    PendingAccess --> SettlingToAgent : "用户批准或主动 Handover 移入"

    SettlingToAgent --> HeldOnDisplay : "AeroSpace claim 写入 + windows::settle 落位成功"
    HeldOnDisplay --> DriftedOffDisplay : "目标应用自发重置 frame 或重排跳屏"
    DriftedOffDisplay --> HeldOnDisplay : "guard_placed 或 run_repark_shifts 拉回代理屏"

    HeldOnDisplay --> ViewerDragged : "用户在 Viewer 拖拽窗口 (10s ViewerDragGuard)"
    ViewerDragged --> HeldOnDisplay : "松手后 keep_on_display 夹紧回屏内"

    HeldOnDisplay --> PendingCrossSession : "另一会话发起跨会话 Window Request"
    PendingCrossSession --> HeldOnDisplay : "原会话释放或用户批准, 原子切换 Held 持有者"

    HeldOnDisplay --> ReturningToUser : "dismissal::give_back 或用户主动收回"
    ReturningToUser --> UserScreen : "删除 AeroSpace claim + 恢复保存的原始用户屏 frame"

    HeldOnDisplay --> [*] : "目标窗口关闭 (CGWindowList 消失)"
    UserScreen --> [*] : "目标窗口关闭"
```

## 状态与转换明细表

| 节点 | 我方代码 | macOS 组件与内部行为 | 限制与故障模式 | 关联问题 | 验证状态 |
| --- | --- | --- | --- | --- | --- |
| 1. `UnclaimedCatch -> SettlingToAgent / UserScreen` | `apps/macos/src/mac/catch.rs:35-230`、`crates/model/src/new_window.rs:18-125` | `AXObserver` 捕获 `kAXWindowCreatedNotification` 后提取 `_AXUIElementGetWindow`，交由纯模型 `owner_for_new_window(agent, user, only_here)` 裁决归属。 | 用户与代理在同一秒操作同一应用时，`user == true` 优先保护用户，新窗留在 `UserScreen`。 | `#110`、ADR 0007 | `[已验证]` `crates/model/src/new_window.rs:18-125` |
| 2. `UserScreen -> PendingAccess -> SettlingToAgent` | `crates/model/src/access.rs:15-155`、`crates/model/src/held.rs:15-120` | 代理请求访问用户屏窗口时必须挂起等待用户授权；授权通过或用户执行 Handover 时，先在 `Held` 中快照当前 `kCGWindowBounds` 供日后原位归还。 | 全屏独立 Space 窗口或最小化窗口在未还原前无法直接迁入虚拟屏。 | `#205`、`#206` | `[已验证]` `crates/model/src/access.rs:15-155` |
| 3. `SettlingToAgent -> HeldOnDisplay` | `apps/macos/src/mac/claims.rs:20-95`、`apps/macos/src/mac/windows.rs:180-340` | 先写入 `~/Library/Application Support/AeroSpace/claims/<pid>.json`，再通过最多 12 次 `AXPosition` 与 `AXSize` 循环写入使 `CGWindowListCopyWindowInfo` 实测中心点落入目标虚拟屏。 | 固定尺寸面板拒绝 `AXSize` 调整；目标主线程无响应导致 `kAXErrorCannotComplete (-25204)`。 | `#205`、AeroSpace 竞争 | `[已验证]` `apps/macos/src/mac/windows.rs:180-340` |
| 4. `HeldOnDisplay <-> DriftedOffDisplay`（ADR 0018 对账） | `crates/model/src/held.rs:60-165`、`apps/macos/src/core.rs:510-715` | 当 `CGWindowListCopyWindowInfo` 实测几何偏离 `Held` 绑定的虚拟屏，且非用户主动拖拽时，区分两种原因：显示器重停放由 `run_repark_shifts` 按位移差平移；应用私自跳回主屏由 `guard_placed` 按 `Held` 记录拉回。若用户直接把窗口拖到了真实桌面上并释放，对账逻辑按 ADR 0018 更新归属。 | 若仅凭瞬时几何判定归属而不看 `Held` 记录，`WindowServer` 在热插拔瞬间把窗口挤回主屏会导致代理永久丢失窗口所有权并污染用户桌面。 | `#204`、`#205`、ADR 0018 | `[已验证]` `docs/adr/0018-window-ownership-reconciliation.md`、`crates/model/src/held.rs:60-165` |
| 5. `HeldOnDisplay <-> ViewerDragged` | `apps/macos/src/mac/viewer_input.rs:1510-1565` | 用户在 `Viewer` 内发起非标题栏拖拽时创建 `10 s` `ViewerDragGuard` 暂停 `guard_placed` 拉扯；松手后在 `0..700 ms` 阶梯调用 `keep_on_display` 确保窗口留在虚拟屏内。 | 自绘标题栏窗口若触发 `performWindowDragWithEvent:`，拖拽位移会绑定物理光标而偏离预期轨迹。 | `Viewer 拖拽越界/失步`、`CustomDrag 首按吞没` | `[已验证]` `apps/macos/src/mac/viewer_input.rs:1510-1565` |
| 6. `HeldOnDisplay -> PendingCrossSession` | `crates/model/src/access.rs:75-155`、`docs/adr/0017-cross-session-window-requests.md` | 兄弟会话请求同一窗口时检查原持有者的 `HolderStatus`，在原会话释放或用户批准后原子改写 `Held` 归属会话并按需跨虚拟屏搬移。 | 僵尸 MCP 连接未触发 EOF 时原会话 `HolderStatus` 保持活跃，阻塞无人工干预的自动转交。 | `#189`、ADR 0017 | `[已验证]` `crates/model/src/access.rs:75-155` |
| 7. `HeldOnDisplay -> ReturningToUser -> UserScreen` | `apps/macos/src/dismissal.rs:25-145` | 先移除 AeroSpace `claims/<pid>.json` 条目，再按 `Held` 保存的原用户屏 `CGRect` 写回 `AXPosition` 与 `AXSize`（原外接屏离线时回退主屏可用区），最后清除 `Held` 记录。 | 若先移动窗口再删 claim，AeroSpace 会漏掉该窗口的重新平铺时机。 | `#204` | `[已验证]` `apps/macos/src/dismissal.rs:25-145` |
