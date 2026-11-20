# macOS 发布流水线调研

## 结论

在 GitHub 托管 runner 上签名和公证是常见做法。现有方案方向正确，不必改回本地发布。Zed、Ghostty、WezTerm 和 tauri 官方示例都在 CI 中签名并公证 [19][20][21][22]。

两次故障需要两种修复：

- 协议过期的 403：Apple 只通知 Account Holder，CI 无法读取协议状态。我们的凭据预检已经能捕获这个错误，但它排在 15.5 分钟的 `cargo` 检查之后才运行 [18]。把预检移到发布 job 的第一步，再加一个每周运行的 Linux 定时检查。
- 15 分钟后取消：这是 GitHub 没有分配到 macOS arm64 runner，与我们的超时设置无关。仓库内没有任何 `timeout-minutes`，默认值是 360 分钟 [13][18]。修复是减少每次运行的 macOS job 数量，并对"未分配 runner"的 job 自动重跑一到两次。改用 `macos-15` 不能可靠解决，它属于同一个 arm64 资源池 [17]。

签名密钥应移入只允许 `main` 使用的 GitHub environment。不建议在每个 PR 上公证。

## 证据范围

2026-11-14 查阅。来源是 Apple 开发者文档、GitHub 文档与 Changelog、`actions/runner-images` 仓库、各项目 `main` 分支上的 workflow 文件、Sparkle 与 Homebrew 文档，以及本仓库的 Actions 运行记录。本仓库指 `.github/workflows/release.yml`、`ci.yml` 和 `apps/macos/scripts/release.sh`，提交 `dab5e39`。Apple 文档页面由 JavaScript 渲染，正文通过其 JSON 接口读取。在线文档会变化。无法核实的内容逐条标为"未核实"。

## 1. CI 签名与公证

Apple 把"在脚本化构建环境中加入公证"列为命令行公证的用途之一。文档要求用 `notarytool` 上传，`altool` 自 2023-11-01 起不再受支持。`--wait` 让命令等到公证结束，免去轮询。公证结果的日志应每次检查，成功时也要看警告。磁盘镜像可以 staple，ZIP 不行 [1]。

Apple 文档建议用钥匙串条目引用凭据，避免脚本中出现明文密码 [1]。本机 `notarytool` 1.1.2 的帮助显示三种认证方式：App Store Connect API 密钥，参数为 `--key`、`--key-id`、`--issuer`；Apple ID 加 App 专用密码；`--keychain-profile`。另有 `--keychain` 指定钥匙串文件 [1][23]。我们直接传 API 密钥参数，Zed 也这样做 [19]。Ghostty 先用 `store-credentials` 存成钥匙串 profile 再提交 [20]。两种都是 notarytool 支持的用法。Apple 没有在已读页面中明确说 CI 应优先用 API 密钥，这一点未核实。

API 密钥分团队密钥和个人密钥。只有 Admin 能生成团队密钥。私钥只能下载一次，Apple 不保留副本。Apple 要求不要把密钥放进代码仓库，泄露时立即吊销 [3]。公证所需的最低角色在已读页面中没有写明，未核实。

GitHub 的官方指南"Installing an Apple certificate on macOS runners for Xcode development"给出的流程与我们一致：p12 以 Base64 存入 secret，解码到 `$RUNNER_TEMP`，创建临时钥匙串，`set-keychain-settings -lut 21600`，导入后 `set-key-partition-list`，最后用 `if: always()` 删除钥匙串 [8]。

密钥存放：

- GitHub 建议用 environment 的 required reviewers 保护 secret。审批前 job 读不到 environment secret [9][10]。
- environment 可限定分支和标签。规则按运行的 `GITHUB_REF` 匹配 [9]。公开仓库在 Free 计划下可以使用 required reviewers 和 environment secret [9]。本仓库是公开仓库。
- GitHub 建议用 OIDC 换取短期凭据 [10]。Apple 的 App Store Connect API 用自签 JWT 认证 [4]。已读的 Apple 文档中没有 OIDC 或工作负载身份联合的入口，所以 OIDC 不适用。这一否定结论未核实。
- 参考项目都把证书和公证密钥放在普通仓库 secret 中，没有使用 environment [19][20][21]。

## 2. GitHub 托管 macOS runner

标签状态：

| 标签 | 架构 | 状态 | 系统与默认 Xcode | macOS SDK |
| --- | --- | --- | --- | --- |
| `macos-26`，即 `macos-latest` | arm64，3 核 M1，7 GB 内存 | 2026-02-26 起 GA [15] | macOS 26.7.2，Xcode 26.6 | 26.0 至 26.5 [16] |
| `macos-15` | arm64 | 正式提供，README 未标预览 [16] | macOS 15.8.2，Xcode 16.4 | 15.0 至 15.5，另有 26.0、26.1、26.2 [16] |
| `macos-26-intel`、`macos-15-intel` | x64，4 核，14 GB | 正式提供 [11][16] | | |
| `xcode-27` | arm64 | README 标为 preview [16] | | |

`macos-15` 镜像装有 Xcode 26.0.1、26.1.1、26.2 和 26.3，默认仍是 16.4。选中 Xcode 26.3 即可得到 macOS 26.2 SDK [16]。我们 2026-10-10 把 CI 改到 `macos-26`，理由是 `apple-metal` 需要 macOS 26 SDK，见提交 `f5b56b2`。当时用的是 `macos-15` 的默认 Xcode。用 `DEVELOPER_DIR` 切到 Xcode 26.3 能否构建本项目未验证。项目启用了 GPUI 的 `runtime_shaders`，构建时不编译 Metal 着色器，所以不依赖单独下载的 Metal Toolchain。runner-images 的支持策略写明每个 macOS 版本只支持一个 Xcode 大版本 [16]。`macos-15` 上的 Xcode 26 能保留多久未核实。

容量与 15 分钟取消：

- GitHub 文档说需求高时 macOS job 启动会变慢 [11]。各计划最多同时运行 5 个 macOS job，Enterprise 为 50 个，标准与大型 runner 共用此上限 [12]。
- 本仓库失败 job 的注释是 "The job was not acquired by Runner of type hosted even after multiple attempts"，同时有通知 "Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times." [18]
- 2026-11-01 以来的 115 次 CI 与 Release 运行中，6 次运行共 12 个 macOS job 未分配到 runner，全部发生在 11-12 和 11-13。每个都在约 15 分钟后被取消，`rust` 和 `build-release` 都受影响 [18]。同一次运行里，`build-release` 拿到了 `macos-26` runner，`rust` 却没有 [18]。分配是间歇性的。
- GitHub Community 讨论 #211885 从 2026-11-02 起记录同一现象：job 排队约 15 分钟后取消，`runner_id` 为 0，没有执行任何步骤。截至 11-13 没有 GitHub 员工回复，状态页显示正常 [17]。讨论中有人改用 `macos-15` 后立即拿到 runner，也有其他仓库在 `macos-15` 上遇到同样取消 [17]。通知文本指向整个 macOS arm64 池。
- 15 分钟取消不是我们的超时。`timeout-minutes` 默认 360 分钟，从 job 开始运行计时 [13]。讨论中一个设了 90 分钟超时的 job 同样在 15 分钟时取消 [17]。GitHub 文档没有写明托管 runner 的分配时限，"15 分钟"只来自观测，未核实。
- 超过本账户 5 个并发上限时，job 是排队还是同样被取消，未核实。合并一个 PR 时，`main` 的 CI 占 2 个 macOS job，`release` 分支的 CI 再占 2 个，发布构建占 1 个，可能刚好达到上限。

## 3. 其他项目的做法

| 项目 | runner | 凭据存放 | 何时公证 | 凭据预检 |
| --- | --- | --- | --- | --- |
| Zed | Namespace 付费 runner `namespace-profile-mac-large-xcode26` | 仓库 secret：p12、p12 密码、公证 API 密钥三项 | 打 tag 发布；nightly 每天 02:00 与 14:00 两次；PR 加 `run-bundling` 标签时 | 无。缺少 secret 时跳过签名 [19] |
| Ghostty | Namespace runner `namespace-profile-ghostty-macos-golden`，超时 90 分钟 | 仓库 secret，带 `PROD_` 前缀；Sparkle 私钥也在 secret 中 | 打 tag 发布；`main` 上测试通过后的每次 tip 构建 | 无 [20] |
| WezTerm | GitHub 托管 `macos-latest` | 仓库 secret：Apple ID 加 App 专用密码 | 打 tag 发布 | 无 [21] |
| tauri 示例 | GitHub 托管 `macos-latest` | secret：p12、密码、钥匙串密码；API 密钥或 Apple ID 二选一 | 由使用者决定 | 无 [22] |
| GoReleaser | 跨平台模式用内置 quill 库，任何系统都能签名和公证；原生模式需 macOS 和 GoReleaser Pro | 配置项引用 p12 与 API 密钥 | 发布时 | 无。`wait` 超时默认 10 分钟，上限 20 分钟 [24] |

GoReleaser 的跨平台模式只处理裸二进制。文档警告把已公证的二进制放进未公证的 `.app` 无效，`.app` 加 DMG 需要原生模式 [24]。我们发布的是 DMG，Linux 上签名不适用。

两个项目遇到过同一个 403。EnviousWispr 在 2026-10-08 合并了 PR #3532，把 `notarytool history` 移到发布 job 开头，从第 16 分钟提前到第 1 分钟失败 [26]。OpenMS 的 nightly 因协议未签连续数周失败才被发现 [26]。

Apple 的命令行公证文档在减少等待时间的建议中写有"每天公证不超过 75 次" [1]。每个 PR 都公证会消耗这个额度，还要占用稀缺的 macOS runner。参考项目也没有这样做。

## 4. 提前发现凭据与协议问题

- 协议：Apple 更新 Program License Agreement 时通知 Account Holder，并要求在开发者账户首页标注的日期前接受，否则失去 App Store Connect 和 App Store Connect API 等资源的完整访问 [6]。只有 Account Holder 能接受 [6]。最早的信号是这封邮件和 developer.apple.com/account 首页的提示。
- 我们的日志证明 `notarytool history` 会返回同一个 403 [18]。接受协议后，Apple 后端可能要几小时到一天才恢复。Apple DTS 工程师在论坛中推测是缓存原因 [7]。
- App Store Connect API 中没有查询协议状态的接口。这一否定结论未核实。
- Notary REST API 的 `GET https://appstoreconnect.apple.com/notary/v2/submissions` 返回最近 100 次提交，用 API 密钥签的 JWT 认证，不需要 macOS [4]。notarytool 也是调用这个服务。协议过期时该接口是否同样返回 403 未核实。下次出现 403 时用它验证。
- 证书有效期：Developer ID 证书过期后，已签名的软件仍可安装运行，但签不了新版本。吊销后已签软件无法安装或启动。只有 Account Holder 能创建 Developer ID 证书，每类最多 5 个 [5]。Apple 该页没有写有效期年限，未核实。应读取 p12 中证书的 `notAfter`，用 `openssl x509 -checkend` 判断剩余天数 [27]。
- 定时 workflow 只在默认分支运行，整点可能延迟。公开仓库连续 60 天无活动会被自动停用 [14]。

## 5. 差距与建议

| 项目 | 现状 | 常见做法 | 建议 | 成本 |
| --- | --- | --- | --- | --- |
| 凭据预检位置 | `release.sh` 里的 `notarytool history` 在 `Check` 步骤之后运行。11-13 的失败中 `Check` 用了 15.5 分钟，预检在第 16 分钟才失败 [18] | EnviousWispr 把预检放在开头 [26] | 在 `build` job 导入证书前加一步，单独运行 `notarytool history`。保留 `release.sh` 中的预检供本地使用 | 一个步骤。每次凭据失败节省约 15 分钟 macOS 时间 |
| 定期健康检查 | 无。403 只在发布时出现 | 参考项目都没有。OpenMS 靠 nightly 发现 [26] | 新增每周一次的 `ubuntu-latest` 定时 workflow：用 API 密钥签 JWT 调用 Notary API 提交历史接口，用 `openssl` 检查 p12 证书剩余 60 天以上，失败时开 issue。时间避开整点 [4][14][27] | 一个新 workflow。Linux 分钟，不占 macOS 容量。Notary API 对协议过期的响应需在下次出错时确认 |
| 协议通知 | 依赖发布失败 | Apple 邮件通知 Account Holder [6] | Account Holder 为 Apple 协议邮件设置提醒。接受后等待恢复再重跑 [7] | 无 |
| runner 分配失败 | 无重试。每次 CI 运行用 2 个 `macos-26` job 加 1 个 `macos-15` job | 社区做法是只对"未分配 runner"的 job 自动重跑，最多两次 [17] | 新增 `workflow_run` 触发的 workflow：当失败 job 的注释是 "not acquired" 时运行 `gh run rerun --failed`，最多两次 | 一个新 workflow。无法解决长时间缺容量 |
| macOS job 数量 | `ci.yml` 的 `rust` 与 `build-release` 是两个并行的 `macos-26` job | | 合并为一个 job：先 clippy 和测试，再 `cargo build --release`。每次运行少一次分配机会，也降低触及 5 个并发上限的概率 [12] | CI 总时长变长，两个阶段改为串行 |
| 超时 | 所有 job 都没有 `timeout-minutes`，默认 360 分钟 [13] | Zed 每个 job 60 分钟，Ghostty 发布 job 90 分钟 [19][20] | macOS job 设 `timeout-minutes`，按现有耗时留余量。这只限制真实卡死，不影响 15 分钟分配取消 | 每个 job 一行 |
| runner 标签 | 全部用 `macos-26` | 参考项目用付费 Namespace runner 或 `macos-latest` [19][20][21] | 保留 `macos-26`。换 `macos-15` 不能解决容量问题 [17]。容量问题持续数周再评估付费 runner。不要在开发者的 Mac 上跑 self-hosted runner：GitHub 警告公开仓库的 fork PR 可借此取得 secret [10]，且开发者的 Mac 上运行着用户自己的 Agent Screen | 无 |
| 签名密钥存放 | 6 个签名与发布 secret 都是仓库 secret，任何分支上的 workflow 都能读取 | 参考项目同样用仓库 secret [19][20][21]。GitHub 建议用 environment 保护 [9][10] | 建 `release` environment，只允许 `main`，把 Developer ID、App Store Connect、Sparkle 和 tap 部署密钥移进去，`build` 和 `publish` job 引用它。`pull_request` closed 事件的 `GITHUB_REF` 是 PR 的 merge ref，可能不符合 `main` 规则 [9]。需实测，必要时把触发改为 `main` 上的 push，用提交标题识别发布，与 `prepare-release.yml` 的做法一致 | 一次配置和一次实测。可能要改发布触发方式 |
| PR 公证 | PR 不签名不公证。`build-release` 只编译 | Zed 只在打标签的 PR 上公证，Ghostty 在 `main` 每次推送后公证 [19][20] | 不在 PR 上公证。仅当 `bundle.sh`、`release.sh`、`Info.plist` 或 entitlements 变化时，在 `main` 上自动运行一次 `release.yml` 的 dry run，提前发现签名回归 [1] | 偶发的 macOS 运行，占用少量每日公证额度 |
| Sparkle 私钥 | 仓库 secret | Sparkle 建议放在钥匙串，用 `-x` 导出迁移，丢失后可通过 Developer ID DMG 轮换 [25]。Ghostty 也存为 secret [20] | 随 environment 一起迁移，另存一份离线备份 | 无 |
| Homebrew cask | 脚本生成 `agent-screen.rb` 后直接推送 tap | Homebrew 要求提交前运行 `brew style --fix --cask` 和 `brew audit --new --cask` [25] | 推送前运行 `brew style --cask` 和 `brew audit --cask`。Linux 上的 Homebrew 能否审计 cask 未核实，不行就放到 macOS `build` job 末尾 | 一个步骤 |

## 未核实与待确认

- Apple 文档是否明确推荐 CI 用 API 密钥而非 App 专用密码；公证所需的最低 API 密钥角色。
- Developer ID 证书的有效期年限。
- 协议过期时 Notary REST API 是否返回 403；App Store Connect API 是否有协议状态接口。
- GitHub 托管 runner 的分配时限是否固定为 15 分钟；超出账户并发上限时的表现。
- `macos-15` 加 Xcode 26.3 能否构建本项目，以及该组合会保留多久。
- `pull_request` closed 触发的 job 能否通过限定 `main` 的 environment 规则。

## 来源

1. Apple, [Customizing the notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)。正文经 `https://developer.apple.com/tutorials/data/documentation/security/customizing-the-notarization-workflow.json` 读取。
2. Apple, [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)。macOS 10.15 起 Developer ID 分发的软件必须公证。
3. Apple, [Creating API keys for App Store Connect API](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api)。
4. Apple, [Notary API: Get Previous Submissions](https://developer.apple.com/documentation/notaryapi/get-previous-submissions)，[Submitting software for notarization over the web](https://developer.apple.com/documentation/notaryapi/submitting-software-for-notarization-over-the-web)。
5. Apple, [Create Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/)。
6. Apple, [Apple Developer Program roles](https://developer.apple.com/help/account/access/roles/)。
7. Apple Developer Forums, [thread 812222](https://developer.apple.com/forums/thread/812222)，2026 年 1 月，含 DTS 工程师回复。
8. GitHub Docs, [Installing an Apple certificate on macOS runners for Xcode development](https://docs.github.com/en/actions/how-tos/deploy/deploy-to-third-party-platforms/sign-xcode-applications)。
9. GitHub Docs, [Deployments and environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)。
10. GitHub Docs, [Secure use reference](https://docs.github.com/en/actions/reference/security/secure-use)。
11. GitHub Docs, [GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)。
12. GitHub Docs, [Actions limits](https://docs.github.com/en/actions/reference/limits)。
13. GitHub Docs, [Workflow syntax: `jobs.<job_id>.timeout-minutes`](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idtimeout-minutes)。
14. GitHub Docs, [Events that trigger workflows: schedule](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule)。
15. GitHub Changelog, [macos-26 is now generally available for GitHub-hosted runners](https://github.blog/changelog/2026-02-26-macos-26-is-now-generally-available-for-github-hosted-runners/)，2026-02-26。
16. actions/runner-images, [README](https://github.com/actions/runner-images/blob/main/README.md)，[macOS 15 arm64 镜像](https://github.com/actions/runner-images/blob/main/images/macos/macos-15-arm64-Readme.md) 20261104.0514.1，[macOS 26 arm64 镜像](https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md) 20261102.0473.1。
17. GitHub Community, [Discussion #211885: macos-latest ARM64 jobs cancelled with zero steps after hosted-runner acquisition failures](https://github.com/orgs/community/discussions/211885)，2026-11-02 至 11-13。
18. 本仓库运行记录，经 `gh` API 于 2026-11-14 读取：CI 运行 [43467237986](https://github.com/Fanzzzd/agent-screen/actions/runs/43467237986)、[43463974051](https://github.com/Fanzzzd/agent-screen/actions/runs/43463974051)；Release 运行 [43481950920](https://github.com/Fanzzzd/agent-screen/actions/runs/43481950920) 第 3 次尝试，`Check` 18:55:49 至 19:11:20，403 出现在 19:11:24。
19. Zed, [release.yml](https://github.com/zed-industries/zed/blob/main/.github/workflows/release.yml)，[run_bundling.yml](https://github.com/zed-industries/zed/blob/main/.github/workflows/run_bundling.yml)，[release_nightly.yml](https://github.com/zed-industries/zed/blob/main/.github/workflows/release_nightly.yml)，[script/bundle-mac](https://github.com/zed-industries/zed/blob/main/script/bundle-mac)。
20. Ghostty, [release-tag.yml](https://github.com/ghostty-org/ghostty/blob/main/.github/workflows/release-tag.yml)，[release-tip.yml](https://github.com/ghostty-org/ghostty/blob/main/.github/workflows/release-tip.yml)。
21. WezTerm, [gen_macos_tag.yml](https://github.com/wezterm/wezterm/blob/main/.github/workflows/gen_macos_tag.yml)，[ci/deploy.sh](https://github.com/wezterm/wezterm/blob/main/ci/deploy.sh)。
22. Tauri, [macOS code signing](https://v2.tauri.app/distribute/sign/macos/)。该页示例按 "Apple Development" 筛选证书，与其正文要求的 Developer ID 证书不一致。
23. 本机 `xcrun notarytool history --help`，notarytool 1.1.2 (41)。
24. GoReleaser, [Notarize macOS applications](https://goreleaser.com/customization/sign/notarize/)。
25. Sparkle, [Documentation: EdDSA signatures and key rotation](https://sparkle-project.org/documentation/)；Homebrew, [How to Create and Maintain a Tap](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)，[Adding Software to Homebrew](https://docs.brew.sh/Adding-Software-to-Homebrew)。
26. saurabhav88/EnviousWispr, [PR #3532](https://github.com/saurabhav88/EnviousWispr/pull/3532)，2026-10-08 合并；OpenMS, [Issue #9457](https://github.com/OpenMS/OpenMS/issues/9457)，2026-06-07。
27. OpenSSL, [openssl-x509 manual, `-checkend`](https://docs.openssl.org/3.0/man1/openssl-x509/)。未单独抓取页面，依据 OpenSSL 标准选项。
