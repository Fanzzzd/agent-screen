# 在网页里预览 Agent Display

目标：用户在 Codex、Claude 等客户端里和 agent 对话时，不打开 Agent Screen 的 Viewer 也能看到 agent 的屏幕。本文给出做法，尚未写代码。

## 结论

分三期做。第一期只做只读预览：Agent Screen 在本机回环地址上开一个 HTTP 服务，每个 Agent Display 一个带随机令牌的网址，浏览器打开即看到实时画面。`create_display` 的返回值附上这个网址，agent 把链接发给用户。第二期用 MCP Apps 把同一画面直接嵌进支持它的聊天窗口。第三期再考虑在网页里点击和打字，以及手机等其他设备访问。

## 第一期：本机只读网页

### 画面从哪来

不新建采集。Peek 和 Viewer 已经为每个 Agent Display 开着 ScreenCaptureKit 流（`mac/capture.rs` 的 `Capture::start`，最新一帧存在 `Latest`）。网页服务读同一个 `Latest`，有人在看时才编码，没人看时不耗电。若该屏当前没有流，就按 Peek 的方式以低帧率启动一条，最后一个网页断开后停掉。

### 怎么送到浏览器

首选 MJPEG：`multipart/x-mixed-replace`，每帧一张 JPEG。

- 任何浏览器的 `<img>` 都能直接播放，网页端不需要脚本、解码器或 WebRTC。
- 编码用现有的 `screenshot::encode(.., Format::Jpeg)`（ImageIO）。默认缩到宽 1280 像素、每秒 5 到 10 帧，画面不变时不发新帧（`Latest.changes` 已经在数可见变化）。
- 带宽估算：1280 宽的界面截图每帧约 60 到 150 KB，按 8 fps 计约 0.5 到 1.2 MB/s。本机回环不受影响，远程访问时才需要换方案（见第三期）。

备选：WebSocket 推 JPEG，好处是能顺带回传点击，留到第三期。H.264 加 WebRTC 码率最低，但要接 VideoToolbox 和信令，第一期不值得。

### 服务和网址

- 只监听 `127.0.0.1`，端口由系统分配，写进 `~/Library/Application Support/Agent Screen/` 下的状态文件，供 `AgentScreen mcp` 读取。
- 网址形如 `http://127.0.0.1:<port>/d/<token>`。`token` 为每个显示器单独生成的 128 位随机数，显示器结束即作废。打开后是一个只含画面和显示器名的页面；`/d/<token>/stream` 是 MJPEG 流；`/d/<token>/frame.jpg` 返回单帧，供不支持流的客户端。
- 令牌错误或显示器已结束，一律返回 404，不区分两种情况。
- 实现：`apps/macos` 已依赖 tokio（含 `net`），加一个精简的 HTTP 库（`hyper` 1.x 或 `tiny_http`）即可。新模块放在 `apps/macos/src/web/`，令牌的生成、作废和查找是纯逻辑，放进 `crates/model` 做单测。

### agent 和用户怎么拿到网址

- `create_display` 的返回值加 `preview_url` 字段，工具说明里写明「把这个链接发给用户，用户可在浏览器里看你的屏幕」。
- 菜单栏每个显示器的菜单加「复制网页链接」。
- 可选：设置里加一个总开关，默认开启，关掉后不监听端口、不返回 `preview_url`。

### 隐私边界

画面里可能有用户交给 agent 的窗口，包括登录页。所以：只绑回环地址；令牌不可猜且随显示器作废；网页只读，不能操作；用户自己的屏幕永远不在可访问范围内，只有 Agent Display。这与 ADR 0001 不冲突：网页不移动指针、不激活 app。按惯例应新增一份 ADR 记下「网页预览只读、只绑本机、按显示器发令牌」。

### 测试

- 单测：令牌生成、作废、查找。
- e2e 脚本 `webpreview`，在 `AGENTSCREEN_INSTANCE=e2e` 下运行，最多建 1 个显示器：
  - 取 `preview_url`，用 HTTP 客户端读 `/stream`，断言收到至少 3 个 JPEG 分段且尺寸正确。
  - 在显示器上打开 Calculator 后，再取一帧，断言画面变化。
  - 错误令牌返回 404；`end_display` 之后原网址返回 404。
  - 不连网页时，CPU 不比基线高（无编码在跑）。

工作量估计：服务、令牌和 MJPEG 约 400 到 600 行，加 e2e 脚本。

## 第二期：嵌进聊天窗口（MCP Apps）

MCP Apps 是 MCP 的官方扩展，2026-01-26 上线，并已进入 2026-07-28 版规范。工具定义里用 `_meta.ui.resourceUri` 指向一个 `ui://` 资源（`mimeType: text/html;profile=mcp-app`），客户端在沙箱 iframe 里渲染它。

- 做法：给 `create_display`（或新增的 `show_display` 工具）挂一个 `ui://agent-screen/display` 资源。资源里的页面加载第一期的 `/d/<token>/stream`，于是画面直接显示在对话里。
- 客户端支持情况（据 MCP 官方博客和第三方文章，未逐一实测）：Claude 网页版、桌面版、移动端，ChatGPT，VS Code Copilot 等支持渲染；Codex CLI 是终端，渲染不了 iframe，会退回文字结果；Codex 并入 ChatGPT 桌面版后，在桌面版里可以渲染。所以工具结果必须同时带纯文字的 `preview_url`。
- 待验证：iframe 沙箱是否允许加载 `http://127.0.0.1` 的图片流，各客户端的 CSP 可能拦截。若被拦，改为资源页面通过 MCP Apps 的消息通道向 MCP server 拉取单帧 JPEG（base64），降到每秒 1 到 2 帧。这一点要先做最小原型再定。

## 第三期：交互与远程

- 网页内操作：点击、滚动、键盘经 WebSocket 回传，走 Viewer 现有的输入路径（`mac/input.rs`），语义等同 Takeover，需用户显式开启，并遵守 ADR 0014（Viewer 的按压不让 agent 看到）。
- 其他设备访问（手机看 agent 进度）：默认关闭。开启时要配对码，或走用户自己的隧道。带宽上 MJPEG 不够用，届时改为 VideoToolbox H.264 加 WebRTC。

## 参考

- cua 给虚拟机用 noVNC 在网页里看画面；macOS 自带的屏幕共享（VNC）共享的是整个用户屏幕，不能只给 Agent Display，不可用。
- [MCP Apps 发布说明](https://blog.modelcontextprotocol.io/posts/2026-01-26-mcp-apps/)，[MCP Apps 文档](https://modelcontextprotocol.io/extensions/apps/overview)，[Codex 与 MCP Apps（第三方）](https://codex.danielvaughan.com/2026/07/31/mcp-2026-07-28-final-specification-stateless-core-tasks-apps-codex-cli-migration/)。
