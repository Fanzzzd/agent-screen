# 在网页里预览 Agent Display

目标：用户在 Codex、Claude 等客户端里和 agent 对话时，不打开 Agent Screen 的 Viewer 也能看到 agent 的屏幕，之后再在网页里操作，并从其他设备访问。

分三期：

| 期 | 内容 | 状态 |
| --- | --- | --- |
| 第一期 | 本机只读网页，`create_display` 返回 `preview_url` | 已发布，0.1.37，ADR 0019 |
| 第二期 | 用 MCP Apps 把画面嵌进对话窗口 | 本文给出设计，尚未实现 |
| 第三期 | 网页内操作；手机等其他设备访问 | 本文给出设计，尚未实现 |

本文中关于 MCP Apps 和各客户端行为的结论，来自 2026-11-16 对规范、SDK 源码和官方文档的调研，出处列在文末。各客户端会更新，实现前应复核第二期的「客户端现状」一节。

## 第一期：本机只读网页（已发布）

实现见 `apps/macos/src/web/`、`crates/model/src/web_preview.rs` 和 ADR 0019。要点：

- 服务只监听 `127.0.0.1`，端口由系统分配，写进状态文件 `~/Library/Application Support/Agent Screen/web.json`（e2e 实例为 `web-e2e.json`），权限 0600，退出时删除。
- 每个 Agent Display 一个 128 位随机令牌。网址为 `/d/<token>`（页面）、`/d/<token>/stream`（MJPEG）、`/d/<token>/frame.jpg`（单帧）。令牌错误和显示器已结束返回相同的 404。
- 画面复用 Peek 和 Viewer 的 ScreenCaptureKit 流。有新帧（`Latest.delivered` 增加）才编码，最多每秒 8 帧，宽度不超过 1280 像素，编码不在主线程。没人看时不编码。
- 写入超时 10 秒；显示器结束时切断已打开的流；`/frame.jpg` 复用正在运行的画面流，否则保留 5 秒宽限，避免轮询反复重启采集。
- 页面只读，内容安全策略（CSP）以响应头发送。
- e2e 脚本 `webpreview` 覆盖以上行为，共 12 项检查。

## 第二期：嵌进对话窗口（MCP Apps）

### 结论

画面经 MCP 本身传输，不让对话窗口直接加载本机网址。对话窗口里的 MCP App 页面定时调用一个只给页面用的工具 `display_frame`，拿到一张缩小的 JPEG（base64），显示为 `data:` 图片。每秒 1 到 2 帧。工具的文字结果继续带 `preview_url`，渲染不了 MCP Apps 的客户端（Codex CLI、Claude Code）照常看到链接。

理由：Claude 桌面版和网页版、Codex 桌面版都拦截对本机 HTTP 地址的加载，而 `data:` 图片在所有已知客户端的 CSP 下都允许。见下文「为什么不直接加载本机网址」。

### MCP Apps 的机制

- 工具在 `_meta.ui.resourceUri` 里指向一个 `ui://` 资源。资源的 `mimeType` 为 `text/html;profile=mcp-app`，客户端在沙箱 iframe 里渲染它。
- 资源在 `_meta.ui.csp` 里声明要访问的外部来源（`connectDomains`、`resourceDomains`、`frameDomains`、`baseUriDomains`），客户端可以拒绝。
- 页面通过 postMessage 上的 JSON-RPC 与客户端通信：可以调用服务器的工具、读取资源、收到触发它的工具的输入和结果、请求全屏或画中画。服务器不能主动向页面推送，页面只能轮询。
- 工具可以标记 `_meta.ui.visibility: ["app"]`。客户端对模型隐藏这种工具，只让页面调用。
- 工具结果的 `structuredContent` 不进入模型上下文，`content` 是给模型和纯文字客户端的文字。

### 为什么不直接加载本机网址

| 客户端 | 能否在 iframe 里加载 `http://127.0.0.1:<端口>` |
| --- | --- |
| Claude 桌面版、网页版 | 不能。社区实测其 CSP 写死为 `img-src 'self' data: blob: https://assets.claude.ai; connect-src 'self'; block-all-mixed-content`，本地 stdio 服务器同样如此 |
| Codex 桌面版 | 不能。即使声明了确切的 `http://127.0.0.1:端口` 也被拦截（openai/codex#45913，未关闭） |
| ChatGPT | 不适用。它只连远程 HTTPS 服务器，不运行本地 stdio 服务器 |
| Goose | 能，须声明确切来源，不接受端口通配符 |
| VS Code Copilot | 代码里原样采用声明的来源，应能加载，未实测 |

另外，Chrome 142 起的本地网络访问（Local Network Access）权限会在页面访问本机地址时弹出授权，而且要在每层嵌套 iframe 上逐层委托。基于 Electron 的桌面客户端是否同样受此限制，尚未确认；没有客户端说明做了这种委托。所以本机网址只能作为可选的加速：页面启动时检查客户端返回的 `hostCapabilities.sandbox.csp`，列出了本机来源才改用 `/stream`，否则用 `display_frame`。

### 客户端现状

渲染 MCP Apps 的：Claude 网页版和桌面版（包括本地 stdio 服务器）、VS Code Copilot、Goose、Cursor、ChatGPT（仅远程服务器）、M365 Copilot。

不渲染的：Codex CLI、Claude Code。它们看到工具的文字结果，所以 `create_display` 的文字里必须保留 `preview_url`。

Codex 桌面版：渲染功能在 `enable_mcp_apps` 开关后，开源核心中默认关闭，且有未关闭的 bug。Codex 用户短期内仍以 `preview_url` 为主。

### 设计

```text
create_display ──结果──> 客户端
  _meta.ui.resourceUri = ui://agent-screen/display
  content: 文字，含 preview_url
  structuredContent: { displayId, previewUrl }

客户端渲染 ui://agent-screen/display（内联 HTML，无外部资源）
  页面每 500 ms 到 1000 ms 调用 display_frame(displayId, lastSeq)
    AgentScreen mcp ──HTTP──> 127.0.0.1:<端口>/d/<token>/frame.jpg
      响应头 X-Frame-Seq: <feed 编号>.<帧序号>
    AgentScreen mcp 缩到宽 960 后返回
      structuredContent: { image: base64 JPEG, seq }，seq 未变时不带 image
      content: 一行文字
  页面更新 <img src="data:image/jpeg;base64,...">
```

各部分：

- **资源。** `AgentScreen mcp` 在 `get_info` 里开启 resources，实现 `list_resources` 和 `read_resource`，返回一个内联 HTML 页面 `ui://agent-screen/display`。页面不加载任何外部脚本，CSP 不声明额外来源。现用的 rmcp 3.5.0 已支持工具和资源上的 `_meta`、`list_resources`、`read_resource` 和 base64 blob，不需要换库。
- **`create_display` 的结果。** 加 `_meta.ui.resourceUri`。`structuredContent` 放 `displayId` 和 `previewUrl`，页面从中拿到要显示哪个屏。`content` 的文字不变。
- **`display_frame` 工具。** 标记 `visibility: ["app"]`。图片放在 `structuredContent`，`content` 只有一行文字。参数为显示器句柄，只接受本连接创建的显示器，其他一律报「未找到」，与第一期的 404 规则一致。Claude Code 和 Codex CLI 不实现 `visibility`，模型会看到这个工具，所以只在客户端声明支持 MCP Apps 扩展（`io.modelcontextprotocol/ui`）时才列出它。
- **令牌从哪来。** 状态文件只有端口和进程号，没有令牌。`AgentScreen mcp` 从自己收到的 `create_display` 回复里取 `preview_url`，按显示器句柄存在内存里，因此「只接受本连接的显示器」由这份缓存自然保证。`AgentScreen mcp` 重启后，新连接接手的旧显示器没有缓存的令牌。为此，应用在回复里列出被接手的显示器时一并带上 `preview_url`。这是控制套接字回复里新增的字段，按 ADR 0005 属于向后兼容的扩展。
- **帧的大小。** 宽 960 像素，JPEG 质量约 0.6，目标每帧 100 KB 以内，base64 后约 135,000 字符。Claude 文档说明，工具结果超过约 150,000 字符且代码执行沙箱开启时，结果会写成文件，页面就拿不到数据。每个显示器只有一路画面流，尺寸固定，`/stream` 正在以 1280 宽运行时无法共用一次 960 宽的编码。所以 web 服务照常返回 1280 宽的帧，由 `AgentScreen mcp` 用 ImageIO 缩到 960 宽后再编码。缩放只在页面轮询时发生，每秒最多 2 次。
- **去重。** web 服务在 `frame.jpg` 的响应头 `X-Frame-Seq` 里返回 `<feed 编号>.<帧序号>`。帧序号在每次新建画面流时从 0 重新开始，所以加上 feed 编号（每次新建画面流时取一个随机数），避免页面隐藏超过 5 秒、画面流结束又重建后，新流的序号和旧流相同，页面误以为画面没变。`display_frame` 带上次的 `seq`，相同时只返回 `{ seq }`，不传图片，空闲的屏不耗带宽。
- **轮询节奏。** 页面可见时每 500 ms 调用一次，页面隐藏时（`visibilitychange`）停止。连续 3 次报错后降到每 5 秒一次，并显示「画面已断开」。显示器结束后 `display_frame` 返回「未找到」，页面显示「显示器已结束」，停止轮询。
- **页面内容。** 显示器名、实时画面、「在浏览器中打开」按钮（经 MCP Apps 的打开链接请求打开 `preview_url`）、全屏按钮。界面文字跟随客户端传入的语言，中英两套。
- **只读。** 第二期页面不发送任何输入。

### 待验证的问题

实现前先做最小原型，在 Claude 桌面版里确认以下几点，再定参数：

1. 页面发起的工具调用是否受约 150,000 字符阈值限制，是否会写进对话记录。
2. 每秒 2 次工具调用时，客户端是否限流或卡顿。
3. 对话里多次调用 `create_display` 时，每个结果各自渲染一个页面，还是客户端复用页面。

原型只需要 `ui://` 资源和 `display_frame`，用 e2e 实例的显示器，在用户的 Claude 桌面版里加一个临时的 MCP 服务器配置。这一步改动用户的客户端配置，需要用户同意后进行。

### 测试

- 单测：`display_frame` 只接受本连接的显示器；`seq` 未变时不带图片；画面流重建后 `seq` 不与旧流重复；客户端未声明 MCP Apps 扩展时不列出 `display_frame`。
- e2e 脚本 `mcpapp`：作为 MCP 客户端列出资源，读出 `ui://agent-screen/display`，确认 `create_display` 的结果带 `resourceUri`；调用 `display_frame` 得到可解码的 JPEG，宽 960，大小在 100 KB 以内；打开 Calculator 后 `seq` 增加、画面变化；画面不变时只返回 `seq`；`end_display` 后返回「未找到」。
- 人工验收：在 Claude 桌面版里调用 `create_display`，对话里出现实时画面。

## 第三期：网页内操作与其他设备访问

### 网页内操作

用户在网页里点击、滚动、打字，作用到 Agent Display。

- **控制权。** 网页操作等同 Takeover：开始操作时 agent 失去控制，工具返回 takeover 错误，直到用户交还（ADR 0004）。Viewer 的按压不让 agent 看到（ADR 0014），网页同样遵守。
- **操作权限靠用户在场，不靠保密的令牌。** `preview_url` 会交给 agent，所以拿着它的不一定是用户。agent 若能通过网页发送输入，就能冒充用户接管和交还，绕过 Takeover 的规则。任何以当前用户身份保存的秘密，agent 都可能读到：剪贴板可用 `pbpaste` 读取，浏览器历史会记下网址里的令牌，状态文件也在用户目录下。所以不能把「控制链接不经过 MCP」当作安全边界。做法：
  - 观看令牌（第一期的令牌）永远只读。
  - 网页上的「请求操作」会在 Agent Screen 自己的界面里弹出确认卡片，样式和 Window Request 相同，写明哪个显示器、哪个浏览器。用户点「允许」后，这个浏览器才获得操作权限。agent 通过 Agent Screen 只能操作自己的 Agent Display，点不到用户屏幕上的卡片。这与 Window Request 依赖同一个前提。
  - 允许后，服务器发一个 HttpOnly、SameSite=Strict 的会话 cookie，不把令牌放进网址。会话 10 分钟无操作即失效，显示器结束时失效，用户可在菜单里随时撤销。
- **传输。** 在同一个本机服务上加 WebSocket 路由 `/d/<观看令牌>/control`，传鼠标、滚轮和键盘事件，要求带上述会话 cookie。浏览器对 WebSocket 不做跨域限制，所以服务器还要校验 `Host` 必须是 `127.0.0.1:<端口>` 或 Tailscale Serve 的主机名，`Origin` 必须与之同源，防止 DNS rebinding 和跨站连接。输入走 Viewer 现有的输入路径（`mac/input.rs`），不移动用户的指针，不激活 app（ADR 0001）。画面仍用第一期的 MJPEG。
- **MCP Apps 内的操作。** 第二期的页面不加操作。对话窗口里的页面由客户端托管，和 agent 处在同一个上下文里，不能视为用户本人。

### 其他设备访问

默认关闭，用户在设置里显式开启。

推荐顺序：

1. **Tailscale Serve（首选）。** 应用继续只监听本机。用户运行 `tailscale serve` 把端口以 HTTPS 暴露到自己的 tailnet。Tailscale 负责加密，并限定只有用户自己 tailnet 里的设备能连上。Serve 会给请求加上 `Tailscale-User-*` 身份头，但本机任何进程都能直连 127.0.0.1 并伪造这些头，所以应用不凭这些头授予任何权限。观看仍要令牌，操作仍要上面的确认卡片。Agent Screen 只需在设置里给出命令和说明。
2. **局域网直连（其次）。** 监听局域网地址。按 Apple TN3179，在 macOS 15 及以上，监听并接受入站 TCP 连接不会触发「本地网络」授权弹窗；用 Bonjour 广播或主动连局域网设备才会。必须配对：Mac 上显示 6 位配对码，设备输入后获得长期设备密钥，用户可在设置里逐个撤销。局域网上需要 HTTPS，自签证书会带来浏览器警告，这是这条路排在第二的主要原因。
3. **不做公网中继。** 不经过 Agent Screen 的服务器转发画面。

### 带宽

MJPEG 在 1280 宽、8 fps 下约 0.5 到 1.2 MB/s，tailnet 和局域网够用，手机流量不够。实测确实不够时，再加 VideoToolbox H.264 编码，浏览器端用 WebCodecs 解码，经 WebSocket 传输；仍不够再考虑 WebRTC。先测再做。

### 需要新增的 ADR

- 网页操作等同 Takeover；操作权限只能由用户在 Agent Screen 界面里逐个浏览器确认，不靠保密的令牌。
- 其他设备访问默认关闭，首选 Tailscale Serve，局域网直连必须配对。

## 不采用的方案

- macOS 自带的屏幕共享（VNC）：共享整个用户屏幕，不能只给 Agent Display。
- noVNC：需要在应用里实现 VNC 服务器，比 MJPEG 加 WebSocket 复杂，收益不大。websockify 的令牌只用来选择目标，不是身份验证，不能照搬。
- 第二期用本机网址：Claude 和 Codex 桌面版都拦截，见上文。

## 参考

- MCP Apps 规范（稳定版和草案）：<https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx>，<https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/draft/apps.mdx>
- MCP Apps 的 CSP 与 CORS：<https://github.com/modelcontextprotocol/ext-apps/blob/main/docs/csp-cors.md>
- 示例：system-monitor-server（2 秒轮询）、pdf-server（512 KB 分块）、video-resource-server（blob 资源），均在 <https://github.com/modelcontextprotocol/ext-apps/tree/main/examples>
- 客户端支持矩阵：<https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/extensions/client-matrix.mdx>
- Claude 的显示模式与排错（150,000 字符阈值）：<https://claude.com/docs/connectors/building/mcp-apps/display-modes>，<https://claude.com/docs/connectors/building/mcp-apps/troubleshooting>
- Claude 的 CSP 实测：<https://github.com/anthropics/claude-ai-mcp/issues/40>
- Codex 拦截本机来源：<https://github.com/openai/codex/issues/45913>；功能开关：<https://github.com/openai/codex/blob/main/codex-rs/features/src/lib.rs>
- ChatGPT 的 MCP 连接方式：<https://developers.openai.com/api/docs/guides/secure-mcp-tunnels>
- Chrome 本地网络访问：<https://developer.chrome.com/blog/local-network-access>
- Apple TN3179 本地网络隐私：<https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy>
- websockify 令牌：<https://github.com/novnc/websockify/wiki/Token-based-target-selection>
