# 贡献者

CodeWhale 在开放中构建，拥有不断壮大的贡献者社区。每个 issue 报告和 pull request 都是真实的项目工作 — 欢迎任何经验水平的参与。这是按**时间顺序**排列的完整每 PR 贡献者记录（最新在前），折叠为时间段以便浏览。展开任何时间段查看所有人。

如需实时列表，请参见
[GitHub 贡献者页面](https://github.com/Hmbown/CodeWhale/graphs/contributors)、
[`AUTHOR_MAP`](https://github.com/Hmbown/CodeWhale/blob/main/.github/AUTHOR_MAP)
和 [CHANGELOG.md](../CHANGELOG.md)。

## 组织感谢

- **[DeepSeek](https://github.com/deepseek-ai)** — 为项目提供启动的模型和支持。感谢 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — 感谢支持并欢迎我们加入鲸鱼兄弟大家庭。感谢 DataWhale 的支持。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — 感谢优先支持 codewhale 并协作改善终端 agent 体验。
- **[Open Design](https://github.com/nexu-io/open-design)** — 感谢围绕面向设计的 agent 工作流的支持和协作。

维护者规则：报告和 PR 是真实的项目工作，即使最终补丁必须缩小范围、延迟或收录到维护者分支。收录的 PR 在提交/PR 正文、changelog 或发布说明以及相关 issue/PR 评论中保留可见的署名。

---

## 按时间排列的贡献者

<details open>
<summary><strong>v0.8.68 — 维护清扫、运行时持久化和发布证据</strong></summary>

v0.8.68 维护 lane 以 `main` 上的发布后清理开始：运行时线程持久化、终端选择、UTF-8 编辑处理、README 发现和死代码移除，以及 v0.8.67 的 Cursor dogfood 证据一并落地。

- **[Jeffrey Luna / Mr-Moon121](https://github.com/Mr-Moon121)** — 子 agent 等待的反轮询 constitution（从 PR #4098 收录到 #4097 / PR #4229）

- **[MXAntian](https://github.com/MXAntian)** — 将压缩摘要持久化到线程记录中，使 `/v1` 引擎重载保留压缩上下文（#4091）
- **[nightt5879](https://github.com/nightt5879)** — 在禁用鼠标捕获时保持原生终端选择可用，并在 UTF-8 字符边界上推进模糊编辑匹配（#4088、#4045）
- **[gaord](https://github.com/gaord)** — 将社区维护的 CodeWhale for VS Code GUI 前端添加到英文和中文 README 中（#4035）
- **[Darrell Thomas](https://github.com/DarrellThomas)** — 移除未使用的 whale route 分类模块及其死测试（#4041）
- **[Taixin Guo](https://github.com/taixinguo)** — CJK 模糊编辑 panic 报告和修复方向，在 UTF-8 边界补丁中被署名（#3971、#4045）

</details>

<details>
<summary><strong>v0.8.66 — 发布就绪、provider 接入和 UI 加固</strong></summary>

v0.8.66 发布准备了 0.8.66 打包 lane，加固了 provider/model 路由和模态界面，推进了 Hotbar/子 agent UI 可靠性，并将多个社区 provider 和桥接贡献纳入发布致谢。

- **[lerugray](https://github.com/lerugray)** — Sakana AI Fugu provider 支持，涵盖配置、CLI、TUI provider 选择器、文档和模型补全（#3748，已收录）
- **[greyfreedom](https://github.com/greyfreedom)** — agent 运行时基板、provider 路由、TUI 子 agent 启动器、fleet roster 基础、model-strength 路由（#3593 等，已收录）
- **[nightt5879](https://github.com/nightt5879)** — `/statusline` 前缀稳定芯片和 `codewhale-tui` 发布二进制搜索路径使 `codewhale update` 发现 TUI 更新（#3619、#3630）
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — HarmonyOS / OpenHarmony 移植工作延续（#3669，已收录至 fleet）
- **[cyq1017](https://github.com/cyq1017)** — 可配置的 completion sound 和 MCP gateway 发现实现（#3627、#3551，已收录）
- **[gaord](https://github.com/gaord)** — 运行时 API 会话保存和 GUI 前端方向（#3639、#3631，已收录至运行时 API）
- **[idling11](https://github.com/idling11)** — Azure MCP token 支持和 agent 运行时轮询方向（#3720、#3733，已收录至 fleet）
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — 会话中 provider/base_url 切换和 token-plan 端点配置（#3616、#3665，已收录）
- **[h3c-hexin](https://github.com/h3c-hexin)** — 多 provider HTTP 头部和上下文窗口覆盖（#3587、#3625，已收录）
- **[hongchen1993](https://github.com/hongchen1993)** — Volcengine provider 在 TUI 调度器中的支持（#3533，已收录）
- **[mvanhorn](https://github.com/mvanhorn)** — `codewhale doctor` provider 选择和 `/model` provider 切换说明（#3539、#3569）

</details>

<details>
<summary><strong>v0.8.65 — ACP 注册表、provider 扩展和平台修复</strong></summary>

v0.8.65 发布推送了 ACP 注册表提交、provider 扩展和 Windows/macOS 平台修复，以及 MCP 和 Composer 修复，跨越多个维护迭代。

- **[nightt5879](https://github.com/nightt5879)** — `stream-json` 事件序列化、子 agent 转录回执、双二进制更新程序指令和 Windows 调度器安装程序（#3408、#3412、#3446、#3540）
- **[gaord](https://github.com/gaord)** — 交互式 runtime-API session 和 VS Code 基础（#3355、#3128 等，已收录至 CodeWhale for VS Code）
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — Xiaomi MiMo provider 发现、token-plan 端点发现、provider `api_key` 编码（#3268、#3339、#3463）
- **[h3c-hexin](https://github.com/h3c-hexin)** — provider 路由优先于模型名称推断和 MiMo/DeepSeek `base_url` 引导（#3270、#3357、#3366）
- **[idling11](https://github.com/idling11)** — PlanArtifact 线框持久化和暂存 provider 注册表方向（#3312，已收录至 fleet 和 Workflow）
- **[LeoAlex0](https://github.com/LeoAlex0)** — 模式安全的 `allow_shell` 启用以及 MCP/skills 集群的 provider 选择器引导（#3296、#3360）
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — HarmonyOS / OpenHarmony 移植工作（#3238，已收录至 fleet）
- **[greyfreedom](https://github.com/greyfreedom)** — provider 就绪检查、MCP 工具超时合并、agent 运行时工具注册表方向（#3362、#3396 等，已收录至 fleet）
- **[reidliu41](https://github.com/reidliu41)** — CodeWhale 专属技能发现门禁（`[skills].scan_codewhale_only`）忽略跨工具目录（#3296）以及 app-server 无认证环回文档
- **[reidliu41](https://github.com/reidliu41)** — 将斜杠命令暴露为 hotbar 操作（#3269）
- **[wavezhang](https://github.com/wavezhang)** — 静态 Linux x64（musl）发布二进制文件
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — 每工具快照门禁遵循 `[snapshots].enabled`（#3292）以及在 `.codewhale` 下写入的 composer 历史
- **[gaord](https://github.com/gaord)** — `workspace_follow_symlinks` 设置用于支持符号链接的工具操作，具有加固的路径处理
- **[greyfreedom](https://github.com/greyfreedom)** — 在运行时遵循 ask-permission 规则（#3295）
- **[aboimpinto](https://github.com/aboimpinto)** — EPIC-001 命令边界重放和用户注册表审查反馈
- **[h3c-hexin](https://github.com/h3c-hexin)** — 将易变的工作区路径移出静态系统前缀（前缀缓存卫生）
- **[hongchen1993](https://github.com/hongchen1993)** — 当 flash 路由器不可用时仅使用启发式自动路由
- **[lucaszhu-hue](https://github.com/lucaszhu-hue)** — Atlas Cloud provider 设置文档
- 追溯协调（早期发布，现在署名）：
  **[manaskarra](https://github.com/manaskarra)** / **[xfy6238](https://github.com/xfy6238)**（#1157）、
  **[djairjr](https://github.com/djairjr)**（#1309 与 reidliu41 一起）、
  **[Geallier](https://github.com/Geallier)**（#1470）、
  **[quentin-lian](https://github.com/quentin-lian)** / **[k0tran](https://github.com/k0tran)**（#1531 / #1992）、
  **[F1LT3R](https://github.com/F1LT3R)**（#1656）、
  **[cmyyy](https://github.com/cmyyy)**（#1842）、
  **[Final527](https://github.com/Final527)**（#3058）

</details>

<details>
<summary><strong>v0.8.61 — 运行时控制平面和社区收尾</strong></summary>

v0.8.61 发布是社区收尾：运行时控制平面、provider 补丁和 TUI 修复与新人和回归贡献者的工作一同落地。

- **[idling11](https://github.com/idling11)** — DeepInfra provider 支持，包含 OpenAI 兼容路由和模型注册表条目（#3235，关闭 #3231）
- **[greyfreedom](https://github.com/greyfreedom)** — 原子 ask-only 权限规则持久化，使执行策略规则在触发提示的写入中存活（#3233）
- **[VincentCorleone](https://github.com/VincentCorleone)** — 微信桥接（`integrations/weixin-bridge`）利用 Feishu + Tencent OpenClaw（#3206）
- **[nightt5879](https://github.com/nightt5879)** — whale-accent 重命名（#3197）和 `/skill` 激活的 `$skillname` 别名（#3241）
- **[mvanhorn](https://github.com/mvanhorn)** — 非 DeepSeek 模型定价覆盖（#3201）
- **[cyq1017](https://github.com/cyq1017)** — Telegram 轮询传输（#3195）和 VS Code 只读 API 文档（#3013）
- **[RobertEmprechtinger](https://github.com/RobertEmprechtinger)** — 移动端事件历史（#3220）
- **[gaord](https://github.com/gaord)** — runtime-API 会话保存（#3199）
- **[hongchen1993](https://github.com/hongchen1993)** — `exec` 中遵循 `DEEPSEEK_BASE_URL` / `MODEL`（#3221）

</details>

<details>
<summary><strong>前瞻轨道 — 近期 v0.9 工作（最新）</strong></summary>

v0.9 前瞻轨道始于 2026 年初，聚焦于 agent 运行时基板、Fleet 编排器和多 provider 路由，横跨多个维护迭代。多个贡献者提交了已收录到 fleet、Workflow 和 TUI 轨道中的 PR。

- **[xyuai](https://github.com/xyuai)** — 规范 CodeWhale settings 路径、provider 持久化、provider 选择器、登出范围和 MiMo 认证清理工作（#2730、#2714、#2715、#2717、#2718）
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — HarmonyOS / OpenHarmony 移植工作和 MatePad Edge 验证跟踪（#2634）
- **[ousamabenyounes](https://github.com/ousamabenyounes)** — Windows 键盘布局的 AZERTY/AltGr composer 快捷键修复（#2863、#2867）
- **[reidliu41](https://github.com/reidliu41)** — hotbar 操作注册表基础和 Ollama 模型补全清理用于前瞻轨道（#2866、#2742）
- **[ljm3790865](https://github.com/ljm3790865)** — 多标签核心/持久化基础和更广泛的标签协作方向（#2864、#2753）
- **[sximelon](https://github.com/sximelon)** — 保存的会话恢复底部提示工作以及 provider 特征元数据注册表方向，已审查并收录到前瞻轨道（#2758、#2760、#2479）
- **[aboimpinto](https://github.com/aboimpinto)** — 侧边栏命令打磨和可暂停自定义命令生命周期方向，已收录到前瞻轨道，以及直接合并的命令支持边界清理和更广泛的命令层设计方向（#2788、#2732、#2871、#2851、#2791）
- **[AdityaVG13](https://github.com/AdityaVG13)** — Workflow 编排和成本跟踪草稿，塑造了维护的 Workflow IR 和 TraceStore 基础（#2482、#2486）
- **[lbcheng888](https://github.com/lbcheng888)**、**[AiurArtanis](https://github.com/AiurArtanis)** 和 **[nasus9527](https://github.com/nasus9527)** — VS Code 扩展脚手架方向、Agent View 请求和 IDE 插件请求，塑造了官方 Phase 0 扩展（#1022、#1584、#2580）
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — `web_run` 缓存状态锁拆分、对话元数据前缀缓存稳定性和项目上下文缓存工作（#2502、#2517、#2636）
- **[idling11](https://github.com/idling11)** — PlanArtifact 连续性、密集工具调用转录折叠、侧边栏详情弹出窗口和 HarnessPosture provider/model 策略方向（#2733、#2738、#2734、#2741、#2692、#2694、#2693）
- **[h3c-hexin](https://github.com/h3c-hexin)** — 子 agent 模型继承、配置的 `skills_dir` 发现、提示环境稳定性和静态提示 composer 方向（#2736、#2737、#2786）
- **[gaord](https://github.com/gaord)** — 运行时线程工作区更新和已完成线程保存会话 API 工作（#2640、#2639）
- **[cyq1017](https://github.com/cyq1017)** — 受信任工作区 MCP 配置、provider 认证回滚、自定义搜索端点、自定义 completion sound、恢复列表和待处理输入传送模式标签工作（#2751、#2755、#2510、#2512、#2513、#2532、#2054）
- **[yusufgurdogan](https://github.com/yusufgurdogan)** — Sofya 搜索 provider 实现，已收录为非默认搜索后端（#2790）
- **[LeoAlex0](https://github.com/LeoAlex0)** — 运行时提示元数据缓存方向，已收录到维护的提示/缓存路径（#2687）；`allow_shell` 前缀缓存解耦和 `visibility="internal"` 解释用于模式切换稳定性（#2949、#2951）
- **[hongchen1993](https://github.com/hongchen1993)** — Volcengine provider 在 TUI 调度器中的支持和调度器 API-key 偏好（#2923、#2928）
- **[NASLXTO](https://github.com/NASLXTO)** 和 **[wuxixin](https://github.com/wuxixin)** — 离线安装打包方向，已收录到 CNB 和安装轨道中（#2679、#2759、#2847）

</details>

<details>
<summary><strong>v0.8.63 — provider 扩展、TUI 打磨和社区修复</strong></summary>

v0.8.63 发布横跨多个维护迭代，在社区 PR 和 provider 配置之上扩展了 provider 支持、TUI 功能和安全修复。

- **[idling11](https://github.com/idling11)** — DeepSeek `beta` prefix-cache 端点的专用 token 使用跟踪，以及收费 token 使用事件的工具审计事件（#2935、#2950、#2960）
- **[xyuai](https://github.com/xyuai)** — 在代码库中清理遗留的 `deepseek` provider 名称引用，改为规范 CodeWhale 名称（#2768）
- **[gaord](https://github.com/gaord)** — VS Code 工作线程扩展工作，已收录到 CodeWhale for VS Code 文档和仓库中（#2833）
- **[Tinghui-Zhou](https://github.com/Tinghui-Zhou)** — macOS 上基于 tmux 的 lane 持久化方向，已收录到 fleet runtime 轨道中（#2601、#2649）
- **[h3c-hexin](https://github.com/h3c-hexin)** — GLM-5.2 系列 provider 支持，包含模型注册表和工作配置（#3101、#3102）
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — OpenHarmony / 鸿蒙移植工作（#3152、#3153）
- **[lalala-233](https://github.com/lalala-233)** — 空 provider 列表 doctor 处理，在 #3211 中用结构化 `doctor.providers` 部分进行了审查和扩展
- **[greyfreedom](https://github.com/greyfreedom)** — 带 provider 路由的 agent 运行时、`max_depth` 钳制和 fleet/mcp/sandbox 预览门禁（#3187 等，已收录到 fleet）
- **[nightt5879](https://github.com/nightt5879)** — 对话获取/跳转、composer 即时模式和 CLI `doctor` 超时（#3004、#3002、#3032）
- **[cyq1017](https://github.com/cyq1017)** — 应用服务器完成提示音、用户取消、Cascadia Code Nerd Font 文档和受信任工作区 MCP 配置（#3021、#3045、#3030、#3106）
- **[rockyzhang](https://github.com/rockyzhang)** — 自定义 web_search 提供者基础，已收录到维护的搜索路由路径中（#2965）

</details>

<details>
<summary><strong>v0.8.62 — provider 路由、TUI 交互和 CI 加固</strong></summary>

v0.8.62 发布覆盖了 provider 路由和模型选择、TUI 交互和 CI 加固，横跨多个维护迭代和 community PR。

- **[idling11](https://github.com/idling11)** — 前端 UI 逻辑已收录到 PlanArtifact 面板和 Workflow dogfood 路径中（#2835、#2856、#2874、#2877）
- **[Hong Huang](https://github.com/Hong-Huang)** — `/ide` 命令在系统提示中暴露 VS Code 工作区状态（#2803、#2804）
- **[gaord](https://github.com/gaord)** — 已保存运行时会话的 HTTP 服务器和 SSE 流以及 VS Code 扩展工作（#2780、#2781、#2802、#2822）
- **[nightt5879](https://github.com/nightt5879)** — 对话获取、composer 即时模式、命令行扩展点和 CLI `codewhale doctor`（#2806、#2832、#2846、#2876）
- **[h3c-hexin](https://github.com/h3c-hexin)** — provider 路由和模型选择器注册为可供 `/model` 选择（#2792、#2793、#2829、#2845）
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — 多 provider API 就绪和 Glo 搜索后端新增（#2821、#2827、#2837）
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — Windows 保留冒号路径处理、设置路径迁移（`~/.codewhale` 优先于 `~/.deepseek`）和 OpenHarmony 移植工作（#2849、#2852）
- **[Jeffrey Luna / Mr-Moon121](https://github.com/Mr-Moon121)** — 子 agent 元数据在正确的深度发出（#2814）
- **[hongchen1993](https://github.com/hongchen1993)** — MiniMax provider 名册已注册到模型选择器轮换中（#2807）
- **[reidliu41](https://github.com/reidliu41)** — 可过滤的斜杠命令菜单和热重载（#2798、#2801）
- **[mvanhorn](https://github.com/mvanhorn)** — 模型选择器分辨率，当 provider 只提供自动代理模型时（#2800）
- **[NASLXTO](https://github.com/NASLXTO)** — 社区离线安装程序的 `INSTALL.md` 文档（#2816、#2836）

</details>

<details>
<summary><strong>v0.8.60 — 发布工程、provider 集成、技能系统和 TUI 基础</strong></summary>

广泛的多迭代发布，涵盖 Windows 安装程序、provider 和模型选择器集成、子 agent、skills、hooks、MCP、沙箱、TUI 打磨和社区贡献者工作。

- **[Sskift](https://github.com/Sskift)** — CI 跳过、IME composer 路由和 eager shell companion 工具（#2154–#2168、#2302、#2329、#2330、#2331）
- **[encyc](https://github.com/encyc)** — 底部和 `/status` 中的会话 token 分解（#2152）
- **[saieswar237](https://github.com/saieswar237)** — 审查流水线文档（#2178）
- **[sximelon](https://github.com/sximelon)** — 粘贴 Enter 抑制、按键处理提取（#2174、#2042）
- **[nanookclaw](https://github.com/nanookclaw)** — doctor 输出中的搜索 provider（#2135）
- **[Sskift](https://github.com/Sskift)** — CLI 默认环境变量覆盖预防和状态栏底部清除（#2119、#2248）
- **[xin1104](https://github.com/xin1104)** — Homebrew codewhale 二进制安装（#2105）
- **[mrluanma](https://github.com/mrluanma)** — Metaso 搜索 provider（#2059）
- **[Lellansin](https://github.com/Lellansin)** — 在主目录跳过配置合并（#2055）
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — 更新发布渠道和遗留 MCP SSE 修复（#2145、#2301）
- **[cy2311](https://github.com/cy2311)** — CodeWhale 的 Windows `.bat` 启动器（#1861）
- **[LING71671](https://github.com/LING71671)** — 有效成本货币上下文、自定义 provider 文档和核心工具分类提示块（#1902、#2287、#2292）
- **[dzyuan](https://github.com/dzyuan)** — 带 DeepSeek V4 Pro/Flash 模型的 Volcengine provider 支持（#1993）
- **[mvanhorn](https://github.com/mvanhorn)** — 实时请求形态测试工厂和全局 `~/.agents/AGENTS.md` 回退（#2107、#2236）
- **[malsony](https://github.com/malsony)** — Matrix 风格主题和主题选择器改进（#2129）
- **[gaord](https://github.com/gaord)** — 外部 GUI 运行时事件桥接、会话详情序列化和技能 API 发现对齐（#2133、#2265、#2285）
- **[yuanchenglu](https://github.com/yuanchenglu)** — Feishu 每聊天模型切换（#2149）
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — Xiaomi 余额/状态工作、停滞对话恢复、批准意图摘要、移动端 smoke/QR 支持、Claude 主题以及广泛的文档/测试/CI 覆盖（#2257、#2267、#2283、#2384、#2385、#2389、#2403、#2440–#2458、#2460）
- **[h3c-hexin](https://github.com/h3c-hexin)** — web-search URL 解码、提示/指令覆盖钩子、子 agent 指导、SSRF 虚假 IP 信任配置和 prefix-cache 友好的环境放置（#2245、#2311、#2313、#2314、#2354、#2355、#2356）
- **[tdccccc](https://github.com/tdccccc)** — 批准提示关键详情和 shell 预览工作，已收录到维护的批准路径（#1991、#2269）
- **[AresNing](https://github.com/AresNing)** — 首次运行指南、消息提交钩子转换设计，以及对话结束观察者钩子工作，已收录到维护的 hooks 路径（#2278、#2318、#2434、#2578）
- **[Implementist](https://github.com/Implementist)** — Volcengine Ark 搜索 provider 和可靠性加固（#2426、#2429、#2439）
- **[lihuan215](https://github.com/lihuan215)** — Unix socket 钩子接收设计，已收录到可选钩子事件路径（#2333、#2430）
- **[AdityaVG13](https://github.com/AdityaVG13)** — Xiaomi MiMo provider 支持（#2246）
- **[New2Niu](https://github.com/New2Niu)** — macOS 显示通知（#2260）
- **[AiurArtanis](https://github.com/AiurArtanis)** — Solarized Light 主题（#2270）
- **[Lee-take](https://github.com/Lee-take)** — 任务迁移和会话环境隔离修复（#2272）
- **[LeoAlex0](https://github.com/LeoAlex0)** — 消息计数和工具输出缓存保留的会话持久化修复（#2388、#2395）
- **[jimmyzhuu](https://github.com/jimmyzhuu)** — 用于 `web_search` 的 Baidu AI Search 后端（#2371）
- **[rockyzhang](https://github.com/rockyzhang)** — 自定义搜索端点基础，已收录到维护的搜索路由路径

</details>

---

*更多贡献者记录持续更新中。查看 [GitHub 贡献者页面](https://github.com/Hmbown/CodeWhale/graphs/contributors) 获取完整列表。*
