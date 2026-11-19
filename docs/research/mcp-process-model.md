# MCP 服务的进程模型与生命周期

问题：本机 MCP 服务应当每个客户端会话一个 stdio 进程（现状），还是所有会话共用一个服务，例如应用内的本机 Streamable HTTP 端点？生命周期的哪些环节应当自动处理：应用未运行时自动启动、父进程退出或空闲时退出、应用更新后旧进程仍跑旧版本、清理孤儿进程。

本文结论来自 2026-11-18 对 MCP 规范、客户端文档与源码、同类服务源码，以及本仓库代码（`8af70ea`）的调研，出处列在各节和文末。各客户端会更新，实现前应复核「客户端现状」一节。

## 结论

保留 stdio 作为客户端接入方式，不改成应用内共用的 HTTP 端点。同时把 `AgentScreen mcp` 改成不含工具逻辑的转发管道：它只把 JSON-RPC 行原样转给应用，工具列表、工具实现和会话识别都移进应用。这样应用更新后，所有会话立即用上新版本，旧进程不再停留在旧构建上。

理由：

1. **四个主流客户端都支持 stdio，HTTP 不能覆盖全部。** Claude 桌面版的自定义连接器从 Anthropic 云端连接，本机地址无法使用；本机服务只能走 stdio。Claude Code、Codex、Cursor 支持本机 `url`。
2. **HTTP 下拿不到 Claude Code 的会话 ID。** Claude Code 只通过环境变量 `CLAUDE_CODE_SESSION_ID` 把会话 ID 交给 stdio 子进程；对 HTTP 服务不发送任何会话标识（issue #41836，未解决）。Agent Screen 的 Agent Session、Window Authorization 和断线宽限都以会话键为单位（ADR 0016），改成 HTTP 后 Claude Code 的会话将无法区分。
3. **规范对本机服务推荐 stdio。** 安全最佳实践要求本机服务「Use the `stdio` transport to limit access to just the MCP client」；若用 HTTP，必须校验 `Origin`、只监听 127.0.0.1，并应要求令牌。共用端点要新增这些防护，rmcp 默认不校验 `Origin`，也没有内置令牌认证。
4. **当前的问题出在 stdio 进程里带着工具逻辑，而不是 stdio 本身。** 本机实测 47 个 `AgentScreen mcp` 进程中，28 个运行的是已被替换的二进制文件（见下文「本机实测」）。工具定义和工具代码在这些进程里，所以它们向新应用发送旧格式的请求，并向模型提供旧的工具列表。改成转发管道后，进程里只剩稳定的转发代码。JetBrains 的 stdio 选项就是这种纯管道，Xcode 的 `mcpbridge` 也是同一类桥接。
5. **进程数量的成本不高，而且客户端负责回收。** 47 个进程的物理占用（`footprint`）合计约 217 MB，平均每个 4.6 MB。它们的父进程都还活着（44 个 `claude`，1 个 Codex 后台进程，1 个 grok），没有一个被过继给 launchd。数量多是因为长期运行的 Claude Code 会话多，不属于泄漏。

生命周期建议：

- 应用未运行时自动启动：保留现有做法。
- stdin 关闭即退出：保留。另外增加父进程退出监听，作为保险。
- 不做空闲退出：Claude Code 交互模式下 stdio 服务退出后标记为失败，需要用户在 `/mcp` 手动 Reconnect。
- 版本不一致：转发管道模式下由应用统一处理；过渡期由 MCP 进程在重连后比对构建，不一致时明确告诉 agent。
- 孤儿清理：应用只清理父进程已退出的 MCP 进程，不杀父进程仍在的进程。

可选的后续步骤：为 Codex 声明 `codex/connection-lease`，让一个 Codex 进程的多个对话共用一个 Agent Screen 进程。Agent Screen 已支持一个连接服务多个会话（ADR 0016）。这一项的语义未完全核实，见「待定问题」。

## 本仓库现状

以下均为代码阅读结果，未运行应用。

| 环节 | 现在的行为 | 出处 |
| --- | --- | --- |
| 进程模型 | 每个客户端会话启动一个 `AgentScreen mcp`，即应用二进制本身，rmcp 3.5.0 over stdio | `apps/macos/src/main.rs:210-216`，`crates/mcp/src/lib.rs:151-160`，`Cargo.lock:4617-4618` |
| 会话识别 | 依次取 `CLAUDE_CODE_SESSION_ID` 环境变量、Codex 每次调用 `_meta` 里的 `x-codex-turn-metadata.session_id`，都没有时用本进程 pid | `crates/mcp/src/session.rs:15-51` |
| 协议版本 | 结果带 `resultType`，`tools/list` 带 `ttlMs` 3,600,000 和 `cacheScope: public` | `crates/mcp/src/lib.rs:81-87`，`:139` |
| 应用未运行 | 首次工具调用时连接失败，执行 `/usr/bin/open -g <所在 .app>`，每 200 ms 重试，最多 15 秒；失败返回 `unavailable` 并请用户打开应用。非 macOS 直接报错 | `crates/mcp/src/control.rs:306-333`，`:351-387` |
| 应用退出或重启 | 读取端读到 EOF 后标记连接关闭，未完成的请求返回「The Agent Screen app closed the connection.」。下一次调用重新连接，必要时重新启动应用。旧显示器的调用改报「随应用结束」 | `crates/mcp/src/control.rs:389-430`，`crates/mcp/src/lib.rs:113-128` |
| 后台找应用 | 客户端加载工具后，若应用未运行，每 2 秒尝试连接一次（不启动应用），连上后补发 `agentReady` | `crates/mcp/src/control.rs:243-275` |
| stdin 关闭 | rmcp 读到 EOF 后结束服务循环，最多等 5 秒让进行中的响应写完，`run()` 返回，进程退出 | rmcp 3.5.0 `src/service.rs:1420-1427`，`:1757-1764` |
| SIGINT、SIGTERM | 没有注册信号处理，按系统默认动作立即终止 | `crates/mcp/src` 中无 `signal` 相关代码 |
| 父进程退出 | 没有单独监听。父进程死亡后，只有 stdin 管道写端随之关闭时才会退出 | 同上 |
| 空闲退出 | 没有 | 同上 |
| 旧构建 | 不检测。`get_diagnostics` 能对比本 MCP 进程与应用的构建信息，但只在 agent 调用时报告。`CONTROL_PROTOCOL_VERSION` 固定为 1，只用于报告，不参与协商 | `crates/mcp/src/diagnostics.rs:13`，`:57-72`，`:209`；ADR 0005 |
| 应用一侧 | 连接关闭后会话进入 2 秒断线宽限，期间同一会话键的任何请求可接回显示器；同一会话已有另一连接时立即转移 | ADR 0016 2026-11-15 段，`apps/macos/src/core.rs:40` |
| 更新 | Sparkle 在两次相隔 30 秒的检查都没有 Agent Display 时安装并重启应用；ADR 写明「Agents' MCP servers reconnect after the restart」，但重连的是旧二进制 | ADR 0012 |

### 本机实测（2026-11-18，只读）

- `ps` 找到 47 个 `/Applications/Agent Screen.app/Contents/MacOS/AgentScreen mcp`，RSS 合计约 287 MB，`footprint` 合计约 217 MB。RSS 含共享库页面，`footprint` 更接近实际占用。
- 父进程：44 个 `claude`，1 个 `~/.codex/packages/app-server-daemon/.../codex`，1 个 grok。父进程为 1（launchd）的有 0 个。
- 用 `lsof -d txt` 比较每个进程映射的可执行文件 inode 与磁盘上当前文件的 inode：19 个一致，28 个不一致（分属两个旧 inode，25 个和 3 个）。也就是 28 个进程运行的是已被替换的旧构建。

## 证据

### MCP 规范

当前版本是 2026-07-28（<https://modelcontextprotocol.io/specification/versioning>）。相对 2025-11-25 的主要变化（<https://modelcontextprotocol.io/specification/2026-07-28/changelog>）：

- 移除协议层会话和 `Mcp-Session-Id`。需要跨调用状态的服务用自己生成的句柄，作为普通工具参数传递（SEP-2567）。
- 移除 `initialize` 握手。每个请求在 `_meta` 里带协议版本和客户端能力，客户端「SHOULD」带 `clientInfo`。新增服务端必须实现的 `server/discover`（SEP-2575）。
- `tools/list` 等列表结果必须带 `ttlMs` 和 `cacheScope`（SEP-2549）。
- 移除 Streamable HTTP 的 GET 流和 `Last-Event-ID` 续传，长期通知改用 `subscriptions/listen`。

stdio（<https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio>）：

- 由客户端启动服务子进程。
- 关闭顺序：客户端「SHOULD」先关闭子进程的 stdin，等待退出，超时再强制终止，POSIX 上从 SIGTERM 升级到 SIGKILL。
- 「Servers SHOULD exit promptly when their standard input is closed」，这是「the only portable」的正常关闭信号。
- 服务意外退出时客户端「SHOULD restart it」，因为协议无状态，进行中的请求可直接重试。
- 同一套换行分隔的 JSON-RPC 帧可以直接用于 Unix 域套接字等字节流，只有进程生命周期部分是 stdio 特有的。

Streamable HTTP（<https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http>）：

- 「the server operates as an independent process that can handle multiple client connections」。
- 服务端「MUST」校验 `Origin`，不合法时返回 403；本机运行时「SHOULD」只监听 127.0.0.1；「SHOULD」对所有连接做认证。理由是防止 DNS rebinding 让远程网页访问本机 MCP 服务。
- 每个 POST 必须带 `MCP-Protocol-Version`、`Mcp-Method`，`tools/call` 还要带 `Mcp-Name`。收到旧客户端的 `Mcp-Session-Id` 时忽略，不生成也不回显。

授权（<https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization>）：授权是可选的；stdio 实现「SHOULD NOT」走这套 OAuth 流程，应从环境取凭据。

安全最佳实践（<https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices>，Local MCP Server Compromise 一节）：本机服务应防止其他进程滥用，做法是「Use the `stdio` transport to limit access to just the MCP client」，或者 HTTP 时要求授权令牌，或者用权限受限的 Unix 域套接字等 IPC。同一页 State Handle Hijacking 一节要求无认证服务的句柄用安全随机数生成、不可预测。

SEP-2567（<https://modelcontextprotocol.io/seps/2567-sessionless-mcp>）：

- 删除会话的原因：各客户端对会话的范围不一致，「some scope them per tool call, some per application launch, some per page load」。
- 对 stdio 服务：仍可依赖进程生命周期，但「SHOULD NOT rely on process-lifetime state and SHOULD migrate to explicit handles」，因为进程对应一次对话还是一次应用启动由客户端决定。
- 关于清理：会话结束本就不可靠，服务应使用写在工具说明里的过期策略（TTL）。

2025-11-25 相对 2025-06-18 的传输变化很少：明确 `Origin` 不合法时返回 403（PR #1439），允许服务端随时断开 SSE 流由客户端轮询（SEP-1699），明确 stdio 服务可以用 stderr 输出任何日志（PR #670）。出处：<https://modelcontextprotocol.io/specification/2025-11-25/changelog>。会话、握手和进程模型的实质变化都在 2026-07-28。

2025-11-25 的传输总览写有「Clients SHOULD support stdio whenever possible」（<https://modelcontextprotocol.io/specification/2025-11-25/basic/transports>）。2026-07-28 的传输总览页未见这句话，我没有找到它被删除的说明。

对 Agent Screen 的含义：显示器已经用句柄寻址（ADR 0002），符合 2026-07-28。「连接关闭即结束显示器」属于进程生命周期状态，规范不禁止，但它只在 stdio 下有效；改成 HTTP 后没有连接可关闭，显示器只能靠空闲超时结束。

### 客户端现状

| 客户端 | stdio 进程单位 | 结束时如何关闭 | 已知遗留进程 | 本机 HTTP（`url`） | 会话标识 |
| --- | --- | --- | --- | --- | --- |
| Claude Code | 每个 `claude` 进程一套，`/clear` 后不重启 | macOS：SIGINT，100 ms 后 SIGTERM，再 400 ms 后 SIGKILL | 有，多个 issue | 支持 `type: "http"`，OAuth 可选 | stdio：环境变量 `CLAUDE_CODE_SESSION_ID`；HTTP：无 |
| Codex | 每个对话（thread）一套；支持 2026-07-28 并声明租约的服务可跨对话共用 | 进程组 SIGTERM，2 秒后 SIGKILL；`kill_on_drop` | 有，后台进程每个对话保留一套 | 支持 | 每次 `tools/call` 的 `_meta` 带 `threadId`、`sessionId`、`x-codex-turn-metadata` |
| Claude 桌面版 | 应用启动时启动（文档推断为一次启动一套） | 未公开 | 有 | 自定义连接器从云端连接，本机地址不可用 | 未查到 |
| Cursor | 未公开，论坛日志路径显示每个窗口一套（推断） | 未公开 | 官方人员确认关窗口不清理 | 支持，文档示例为 `http://localhost:3000/mcp` | 未查到 |

Claude Code：

- 关闭顺序和 `/mcp` Reconnect、`/mcp reconnect all`：<https://code.claude.com/docs/en/mcp>。stdio 服务中途崩溃后，交互模式标记为失败，需用户 Reconnect；`claude -p` 在下次工具调用时重启。
- `CLAUDE_CODE_SESSION_ID` 在 stdio 子进程中设置，保留启动时的 ID，`/clear` 后不更新：<https://code.claude.com/docs/en/env-vars>。
- HTTP 服务无法识别会话：<https://github.com/anthropics/claude-code/issues/41836>（未解决）。评论里的变通做法是 `headersHelper` 加 `SessionStart` hook，Agent Screen 不宜依赖。
- 遗留进程：<https://github.com/anthropics/claude-code/issues/1935>（退出不终止，macOS，未解决），<https://github.com/anthropics/claude-code/issues/79740>（改配置后 Reconnect 丢下旧进程，未解决），<https://github.com/anthropics/claude-code/issues/37482>（stdio 进程被过继给 PID 1）。

Codex（源码固定在 `f09fb197f517acb3052ae3976a7fc3b5a5a8897d`，路径前缀 <https://github.com/openai/codex/blob/f09fb197f517acb3052ae3976a7fc3b5a5a8897d/>）：

- 客户端基于 rmcp 3.6.0，但自己启动子进程：`codex-rs/rmcp-client/src/stdio_server_launcher.rs:474`（新进程组），`codex-rs/utils/pty/src/child_command.rs:99-104`（`kill_on_drop`、`env_clear`）。
- 终止：`stdio_server_launcher.rs:356`、`:669-716`，SIGTERM 到进程组，2 秒后 SIGKILL。MCP 启动器未使用 Linux 的父进程死亡信号。
- 跨对话共用：只限「compatible local stdio servers using MCP 2026-07-28」，通过能力 `codex/connection-lease` 和 `_meta` 键 `codex/connectionLease` 选择加入（`codex-rs/codex-mcp/src/connection_pool.rs:1-5`，`:40-43`，`:74`，`:325-326`）。
- 会话标识：`codex-rs/core/src/mcp_tool_call.rs:656-661`，`:1608-1626`。
- 遗留进程：<https://github.com/openai/codex/issues/30408>（25 个对话后 133 个进程、9.3 GB），<https://github.com/openai/codex/issues/56937>。
- `url` 配置：<https://learn.chatgpt.com/docs/extend/mcp?surface=cli>。

Claude 桌面版：

- 本机服务写在 `claude_desktop_config.json`，应用启动时启动，改配置需完全退出重开：<https://modelcontextprotocol.io/docs/2026-07-28/develop/connect-local-servers>。
- 自定义连接器「connects to your remote MCP server from Anthropic's cloud infrastructure」，必须公网可达且用 443 端口：<https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp>。
- 遗留进程：<https://github.com/anthropics/claude-code/issues/83218>，<https://github.com/anthropics/claude-code/issues/52859>。

Cursor：

- 文档只写 stdio「Cursor manages」，改配置后重启 Cursor：<https://cursor.com/docs/context/mcp>。
- 官方人员确认「MCP child processes don't get cleaned up when you close the window or restart the extension host」，并建议服务监听 stdin 挂断后退出：<https://forum.cursor.com/t/mcp-server-process-leak/151615>。

小结：四个客户端都有遗留 stdio 进程的路径，所以服务自己必须在 stdin 关闭和收到 SIGTERM、SIGINT 时退出。Agent Screen 已经满足这两点。

### 同类本机服务

| 服务 | 进程模型 | 宿主未运行时 | 本机 HTTP 的防护 |
| --- | --- | --- | --- |
| JetBrains IDE（2025.2 起） | IDE 内置 SSE/HTTP 服务；stdio 选项是每个客户端一个 JVM 纯管道进程 | 管道进程找不到端口时报错退出 | 校验 Host，有 Origin、Referer 时校验，要求 `IJ_MCP_AUTH_TOKEN` |
| Xcode（`xcrun mcpbridge`） | 每个客户端一个 stdio 桥接进程，连到正在运行的 Xcode | 报错退出，不启动 Xcode（本机 Xcode 26.6 实测） | 不适用 |
| Figma 桌面版 | 应用内 `http://127.0.0.1:3845/mcp`，无 stdio | 文档只要求保持应用打开 | 文档未提令牌或 Origin 校验 |
| Playwright MCP | 默认 stdio，每个客户端一个浏览器；`--port` 时一个 HTTP 服务承载多会话 | 自己启动浏览器 | 只校验 Host，无 Origin 校验、无令牌 |
| Chrome DevTools MCP | 只有 stdio，每个对话一个 | 首次工具调用时启动 Chrome；连接已有 Chrome 的模式下报错 | 不适用 |
| Docker MCP gateway | 默认 stdio，每个客户端一个；`--transport streaming` 时多客户端共用 | 未核实 | 默认要求 Bearer 令牌，只接受 localhost 的 Origin，但默认监听所有网卡 |

出处：

- JetBrains：<https://www.jetbrains.com/help/idea/mcp-server.html>；源码 <https://github.com/JetBrains/intellij-community/blob/aa6d0be41f16ede03595240e5ba97e615b7bfa6e/plugins/mcp-server> 下 `mcpserver.stdio/src/McpStdioRunner.kt:20-104`、`impl/util/network/util.kt:41-90`、`McpServerServiceImpl.kt:434-475`。读的是 master，未确认这些校验已在 2025.2 发布。旧 npm 代理已弃用：<https://github.com/JetBrains/mcp-jetbrains/blob/main/README.md>。
- Xcode：`xcrun mcpbridge --help` 和关闭 Xcode 时运行的结果（研究代理在本机执行）。Apple 文档页 <https://developer.apple.com/documentation/xcode/giving-agentic-coding-tools-access-to-xcode> 需 JavaScript 渲染，未能读取正文。
- Figma：<https://developers.figma.com/docs/figma-mcp-server/local-server-installation>。
- Playwright：<https://github.com/microsoft/playwright-mcp/blob/16347fa7f878b33442b4b9d5ed34b76a99a5266c/README.md>；`packages/playwright-core/src/tools/utils/mcp/http.ts:51-71`（<https://github.com/microsoft/playwright/blob/8124fc39233c4c349b9dbebd4abc4103738eae81/>）。
- Chrome DevTools MCP：<https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/4b9acc9961a081f462d6a30040b8af729b1d7dc3/src/BrowserManager.ts>（`:225-233`，`:439-472`）。
- Docker：<https://github.com/docker/mcp-gateway/blob/7e0f34277c91855ad861eeff536b4fdc77420593/docs/security.md>（`:34-44`），`pkg/gateway/transport.go:51-79`，`pkg/gateway/run.go:529-537`。

小结：围绕一个长期运行的 GUI 应用的服务，都把状态放在应用里。给每个客户端一个 stdio 进程时，这个进程是纯管道（JetBrains）或桥接（Xcode），不含工具逻辑。直接在应用里开 HTTP 端点的（JetBrains、Figma），JetBrains 另加了 Host、Origin 校验和令牌。

### rmcp

- 最新版本 3.11.0（2026-11-17），本仓库锁定 3.5.0。支持 2024-11-05 到 2026-07-28 的协议版本，2026-07-28 下无 `initialize`、无会话。出处：<https://crates.io/crates/rmcp>，<https://github.com/modelcontextprotocol/rust-sdk/blob/165f8fd514afb8f6d1be3e86260ee9abfce58541/crates/rmcp/src/model.rs>（`:169-209`）。
- Streamable HTTP 服务端：`StreamableHttpService`，功能开关 `transport-streamable-http-server`。默认只接受 localhost 的 Host；`allowed_origins` 默认为空，即不校验 `Origin`，需设置列表或调用 `enforce_origin_validation()`；没有 `Origin` 头的请求总是放行。没有内置令牌认证。旧协议的会话模式默认开启，`LocalSessionManager` 空闲 5 分钟关闭会话。出处：同一仓库 `transport/streamable_http_server/tower.rs:219-352`、`:1473-1558`，`session/local.rs:1220-1239`。

## 生命周期：现状与建议

| 环节 | 现状 | 建议 |
| --- | --- | --- |
| 进程模型 | 每会话一个 stdio 进程，进程内含全部工具逻辑 | 保留每会话一个 stdio 进程，改为纯转发管道：原样转发 JSON-RPC 行，并在连接开头附上会话信息（`CLAUDE_CODE_SESSION_ID`、`CLAUDE_PROJECT_DIR`、pid、父进程 pid）。工具列表、工具实现、会话识别都在应用里 |
| 共用 HTTP 端点 | 无 | 暂不做。若以后为 Cursor 等需要 `url` 的场景加上，必须只监听 127.0.0.1、拒绝非法 `Origin`、要求令牌，并接受 Claude Code 会话无法区分的问题 |
| 应用未运行 | 首次工具调用时 `open -g` 启动，最多等 15 秒 | 保留。管道模式下由管道在首个请求时启动应用，并缓存旧协议客户端的 `initialize`，连上后先重放给应用 |
| stdin 关闭 | rmcp 结束循环，最多 5 秒后退出 | 保留，管道模式同样在 stdin EOF 时退出 |
| SIGINT、SIGTERM | 默认动作立即终止 | 保留 |
| 父进程退出 | 不监听 | 增加：用 kqueue `EVFILT_PROC`/`NOTE_EXIT` 监听启动时的父进程，父进程退出即退出。stdin 管道被其他进程继承时这能兜底 |
| 空闲退出 | 无 | 不做。Claude Code 交互模式不会自动重启退出的 stdio 服务 |
| 应用更新或重启 | 重连到新应用，但进程仍是旧构建 | 管道模式下自然解决。过渡期：重连后用 `getDiagnostics` 比对构建，不一致时让工具结果提示 agent 请用户在客户端重连 Agent Screen |
| 工具列表变化 | `ttlMs` 1 小时、`cacheScope: public` | 应用更新后若工具列表变化，对支持的客户端发 `notifications/tools/list_changed`；`ttlMs` 可缩短。客户端是否处理该通知未核实 |
| 孤儿进程 | 不处理 | 应用通过套接字取对端 pid（`LOCAL_PEERPID`），在菜单或诊断里列出 MCP 进程数和旧构建数。只终止父进程已是 launchd 的进程，不终止父进程仍在的进程 |
| 显示器随连接结束 | 连接关闭后 2 秒宽限再结束 | 保留，作为 stdio 下的兜底。若加 HTTP 端点，需要另定空闲超时并写进 `create_display` 的工具说明 |
| Codex 进程数 | 每个对话一个进程 | 评估声明 `codex/connection-lease`，让一个 Codex 进程的多个对话共用一个进程 |

## 待定问题

1. 是否接受把 MCP 处理移进应用？这意味着控制套接字协议（ADR 0005）从「应用级 NDJSON 方法」变成「转发 MCP JSON-RPC 加少量控制消息」，需要新 ADR，并保留对旧 MCP 进程的兼容期。
2. 过渡期内，检测到旧构建时只提示，还是让 MCP 进程在两次请求之间用 `exec` 换成磁盘上的新二进制？后者对 2026-07-28 客户端可行，因为协议无状态；对仍用 `initialize` 的客户端，新进程没有握手状态，rmcp 可能拒绝后续请求，需实测。
3. 是否需要给 Cursor 等客户端提供 `url` 方式？若需要，令牌放在哪里（Keychain、0600 的状态文件），由谁写进客户端配置（`AgentScreen mcp install`）。
4. Codex 的 `codex/connection-lease` 具体约定什么（租约期限、会话如何区分），需要读 `connection_pool.rs` 全文后再定。
5. 是否升级 rmcp 到 3.11。本文建议不依赖它的 HTTP 服务端，但 stdio 部分的修复和 2026-07-28 支持程度未逐项比较。

## 未能核实

- Claude Code 的 SIGINT/SIGTERM 是否发给整个进程组；`headersHelper` 是否能读到 `CLAUDE_CODE_SESSION_ID`。
- Claude 桌面版的终止顺序，以及 `claude_desktop_config.json` 是否接受 `url` 条目。
- Cursor 的进程单位（每窗口一套是根据论坛日志路径的推断）。
- Claude Code、Codex 是否处理 `notifications/tools/list_changed` 并刷新缓存的工具列表。
- Figma 本机端点是否校验 `Origin` 或令牌；Docker 网关在 Docker Desktop 未运行时的行为。
- 28 个旧构建进程是否已在实际使用中出错。本文只确认它们映射的可执行文件已被替换，未检查它们的请求。
- 本文没有运行 `AgentScreen mcp` 验证 stdin EOF 后的退出时间，退出行为来自 rmcp 源码阅读。

## 参考

- MCP 规范 2026-07-28：<https://modelcontextprotocol.io/specification/2026-07-28/changelog>，<https://modelcontextprotocol.io/specification/2026-07-28/basic/transports>，<https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio>，<https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http>，<https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization>
- MCP 规范 2025-11-25 传输：<https://modelcontextprotocol.io/specification/2025-11-25/basic/transports>
- 版本说明：<https://modelcontextprotocol.io/specification/versioning>
- 安全最佳实践：<https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices>
- SEP-2567：<https://modelcontextprotocol.io/seps/2567-sessionless-mcp>
- 客户端与同类服务：见上文各节链接
- rmcp：<https://github.com/modelcontextprotocol/rust-sdk>
