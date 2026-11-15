# 状态机 04：交互请求生命周期（Help / Takeover / WindowAccess / WindowRequest）

代理向用户或其他会话发起的四类协作请求共享统一的挂起、通知、裁决、撤回与断连重放模型（`crates/model/src/request.rs`），确保任何改变用户窗口归属或需要人工协助的决策均经过显式确认。

## 状态图

```mermaid
stateDiagram-v2
    [*] --> Pending : "发起 Help / Takeover / WindowAccess / WindowRequest"

    Pending --> Notified : "刷新 UI 横幅、菜单栏角标与 UNUserNotificationCenter"
    Notified --> ResolvedContinue : "Help: 用户点击 Continue 或 Takeover: 点击 Return Control"
    Notified --> ResolvedReply : "Help: 用户提交文本 Reply"
    Notified --> ResolvedGiveWindow : "Help (Give) / WindowAccess / WindowRequest: 批准移交窗口"
    Notified --> Declined : "WindowAccess / WindowRequest: 用户点击拒绝"
    Notified --> AutoTransferred : "WindowRequest: 原持有会话主动释放窗口或过期"

    ResolvedReply --> UnheardBuffered : "答复生成时 MCP 连接正处于闪断期"
    UnheardBuffered --> Delivered : "2000ms 内会话重连补发"
    UnheardBuffered --> DroppedOnExpire : "会话超过宽限期彻底过期"

    ResolvedContinue --> Delivered : "连接在线直接写回 NDJSON 响应"
    ResolvedReply --> Delivered : "连接在线直接写回 NDJSON 响应"
    ResolvedGiveWindow --> Delivered : "完成 windows::settle 后写回窗口信息"
    Declined --> Delivered : "返回结构化拒绝原因"
    AutoTransferred --> Delivered : "原子迁移 Held 记录并返回"

    Pending --> Withdrawn : "请求方会话断连过期或显示器被关闭"
    Notified --> Withdrawn : "请求方会话断连过期或目标窗口已销毁"

    Delivered --> [*]
    DroppedOnExpire --> [*]
    Withdrawn --> [*]
```

## 状态与转换明细表

| 节点 | 我方代码 | macOS 组件与内部行为 | 限制与故障模式 | 关联问题 | 验证状态 |
| --- | --- | --- | --- | --- | --- |
| 1. `Pending -> Notified` | `crates/model/src/request.rs:18-115`、`apps/macos/src/ui/shell.rs:840-960` | 模型层分配唯一请求 ID 并关联会话与显示器；UI 层更新状态栏徽标、主面板待办卡片，并通过 macOS `UNUserNotificationCenter` 投递系统通知（不抢夺当前前台窗口焦点）。 | 系统处于勿扰模式或专注模式时横幅静默收纳进通知中心，用户仅能通过菜单栏徽标感知。 | `#206`、ADR 0015 | `[已验证]` `crates/model/src/request.rs:18-115` |
| 2. `Notified -> ResolvedContinue / ResolvedReply` | `crates/model/src/request.rs:116-195`、`apps/macos/src/control/agent.rs:762-910` | 用户在 UI 点选 `Continue`、`Return Control` 或输入文本点击 `Reply`；纯模型将该请求移出待决表并唤醒挂起的异步 oneshot 通道。 | 若在 `Takeover` 状态下等待超过 `15 s` 无输入，界面展示闲置提醒但不自动替用户点完成。 | ADR 0004、ADR 0015 | `[已验证]` `apps/macos/src/control/agent.rs:847-910` |
| 3. `Notified -> ResolvedGiveWindow / AutoTransferred` | `crates/model/src/access.rs:75-155`、`apps/macos/src/ui/shell.rs:910-1040` | 针对 `Help::Give`、`WindowAccess` 与跨会话 `WindowRequest`，在校验目标 `CGWindowID` 仍存活后更新 `Held` 所有权、写入 AeroSpace `claims/<pid>.json` 并调用 `windows::settle` 移入请求方显示器。 | 审批等待期间目标窗口若已被用户手动关闭，转换转入 `Withdrawn` 并向代理返回目标已消失错误；僵尸持有会话（`#189`）会阻止无用户干预的 `AutoTransferred`。 | `#189`、`#205`、ADR 0017 | `[已验证]` `crates/model/src/access.rs:75-155` |
| 4. `ResolvedReply -> UnheardBuffered -> Delivered` | `crates/model/src/request.rs:196-260`、`crates/model/src/session.rs:70-120` | 若用户提交答复瞬间控制连接刚断开但仍在 `2000 ms` 宽限期内，答复暂存在会话 `unheard` 缓冲区，重连握手后立即消费投递。 | 宽限期满未重连则转入 `DroppedOnExpire` 随会话一并销毁。 | ADR 0015 | `[已验证]` `crates/model/src/request.rs:196-260` |
| 5. `Notified -> Declined / Withdrawn` | `crates/model/src/request.rs:140-195` | 用户显式拒绝时返回拒绝状态且保留拒绝记忆，防止代理用同一请求立刻重复弹窗骚扰；请求方会话过期或目标显示器被关闭时自动置为 `Withdrawn` 清理 UI 横幅。 | 无系统级副作用，纯内存清理与 UI 重绘。 | `#206` | `[已验证]` `crates/model/src/request.rs:140-195` |
