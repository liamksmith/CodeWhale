# CodeWhale v0.8.67 — 计算机使用/终端代理内部测试提示词

**发布版本：** v0.8.67 (2026-07-06) — Fleet/Workflow 可用性 + 子代理可靠性通道  
**受众：** 具有终端/计算机使用能力、可以交互或半交互驱动 `codewhale-tui` 的模型。  
**涵盖的里程碑 issue：** #4050、#4051、#4052、#4053、#4054、#4056、#4057、#4058、#4059、#4062、#4063，以及发布候选后检查 #4071-#4076、#4078、#4081，部分 #4027，以及 Constitution / `/workflow` / `/fleet` 基础知识。#4077 保持开放/未覆盖。

---

## 测试代理的序言

### CodeWhale 是什么

CodeWhale 是一个**终端原生 TUI 编码代理**。它运行多轮会话，支持工具使用（读取/搜索/补丁/shell）、子代理（Fleet/委派）、可选的工作流编排、constitution 驱动的设置以及提供商无关的模型路由。发布的运行时二进制文件是 **`codewhale-tui`**；`codewhale` 是调度器 CLI。

### 安装和版本检查

```bash
codewhale-tui --version
# 预期：codewhale-tui 0.8.67（如果只有调度器在 PATH 上，则为 codewhale 0.8.67）
```

如果版本不对，在内部测试之前安装 v0.8.67：

```bash
curl -fsSL https://codewhale.net/install.sh | sh
# 或： npm i -g codewhale@0.8.67
# 或： cargo install codewhale-tui --version 0.8.67 --locked --force
```

在报告中记录确切的版本字符串。

### 工作区选择

使用**以下之一**（在开始时选择；在报告中注明）：

| 选项 | 使用时机 | 设置 |
| --- | --- | --- |
| **A — 隔离的临时仓库** | 最安全的默认选项；避免污染真实安装 | 参见下方[隔离环境](#隔离环境) |
| **B — CodeWhale 仓库** | 针对真实代码库的 Fleet/workflow/子代理测试 | `cd /path/to/codewhale` 并配置好提供商密钥 |
| **C — Harness 父目录 + 嵌套克隆** | #4052 worktree 发现所需 | 父目录下一级有嵌套 git 检出（例如 `Harness/CW/CodeWhale/`） |

### 隔离环境

隔离配置，使内部测试不会触及 `~/.codewhale`：

```bash
export DOGFOOD_ROOT="$(mktemp -d)"
export CODEWHALE_HOME="$DOGFOOD_ROOT/codewhale-home"
export HOME="$DOGFOOD_ROOT/home"
export USERPROFILE="$DOGFOOD_ROOT/home"
export DEEPSEEK_CONFIG_PATH="$CODEWHALE_HOME/config.toml"
mkdir -p "$CODEWHALE_HOME" "$HOME"

# 可选：临时 git 工作区
export WORKSPACE="$DOGFOOD_ROOT/workspace"
mkdir -p "$WORKSPACE" && cd "$WORKSPACE"
git init -q && git commit --allow-empty -q -m "init"
```

对于需要真实 API 路由的测试，在启动前设置**一个**提供商密钥到环境变量中（例如 `DEEPSEEK_API_KEY`、`OPENROUTER_API_KEY`）。不要提交密钥。

### 安全规则

1. 在内部测试期间**不要执行 `git push`** 到任何远程仓库。
2. **不要使用 YOLO 模式**，除非测试明确要求（YOLO 启用 shell + trust + auto-approve）。
3. 对于只读 UI 检查，优先使用 **Plan 模式**；仅在测试需要工具执行时使用 **Agent 模式**。
4. 仅在你有意测试过 onboarding（#4062）之后使用 **`--skip-onboarding`**；否则至少运行一次全新的 onboarding。
5. 完成后清理临时目录：`rm -rf "$DOGFOOD_ROOT"`。

### 如何启动

```bash
# 交互式 TUI（主要内部测试界面）
codewhale-tui --workspace "$WORKSPACE"
```

### 测试执行注意事项

- 如果你无法执行某个步骤（键盘/终端限制、无 API 路由等），记录为 SKIP 并附上原因。
- 如果预期行为与实际不符，记录为 FAIL 并附上复现步骤。
- 每个测试 ID 映射到一个 issue；不要批量合并通过/失败。
- 提交报告时附上测试日期和版本信息。

---

## 优先级分类的测试矩阵

### P0 — 安全/发布阻塞项

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| P0 | 无 YOLO 静默提权 | 启动 Agent 模式。`/mode yolo`。确认转换需要确认或触发显式权限提升 UI。 | 没有隐式 YOLO 入口；YOLO 以不同颜色/标签显示。 | 无静默模式切换 |

### P1 — 功能正确性

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| P1 | Doctor + provider 就绪 | `codewhale doctor --json` | 有效 JSON，`.setup.first_run_ready` 存在 | `operate_ready` 在配置后为 true；无崩溃 |

### P2 — UI/UX 质量

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| P2 | 无回归的空白面板 | 浏览 `/fleet roster`、`/workflow status`、`/setup` | 面板渲染标题和内容，无"未知子进程"行 | 无空白/损坏的面板 |

---

### #4050 — 子代理子进程完成生命周期

**Issue：** 子代理必须完整报告完成状态（`completed`/`failed`/`cancelled`），并在父会话中发出收据。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4050-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked subagent -- --nocapture` | 通过 | 测试绿色 |
| 4050-B | 实时子代理 | 在 Agent 模式下的 TUI 中：`Open a read-only explorer to list the top-level crates.` | 子代理卡片出现并带有角色/标签；在完成后收到紧凑收据。 | 子代理卡片在 `completed` 状态下最终化 |
| 4050-C | 子代理失败 | 启动一个注定失败的子代理（例如无模型路由） | 生命周期变为 `failed`；错误原因显示在卡片上 | `failed` + 错误原因，无静默丢弃 |

---

### #4051 — 委派行排序和去重

**Issue：** 委派的子代理活动必须在工作/代理侧边栏中以可预测的顺序出现，不重复。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4051-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked agent_activity -- --nocapture` | 通过 | 测试绿色 |
| 4051-B | 行排序 | 启动 3 个并行子代理 | 代理侧边栏以一致的顺序显示行（不跳动）；每个子代理一行 | 启动期间无重复行 |
| 4051-C | 完成后去重 | 等待所有 3 个子代理完成 | 每个子代理的已完成收据恰好一行；无重复的完成事件 | 代理侧边栏在完成后无垃圾信息 |

---

### #4052 — Worktree 发现和隔离

**Issue：** 委派到隔离 worktree 的子代理必须将父工作区与子 worktree 分开；父工作区中的发现必须知道嵌套的 git 目录。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4052-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked worktree_discovery -- --nocapture` | 通过 | 测试绿色 |
| 4052-B | Worktree 子代理 | 使用实现者角色和 worktree 隔离启动子代理修复 | 子行显示 `wt`；父工作区在子代理运行期间保持不变 | 无交叉污染 |
| 4052-C | 嵌套发现 | 从包含嵌套 git 检出的目录启动 CodeWhale | 工作区发现报告正确的根目录；不将嵌套仓库误认为主工作区 | 正确的根目录展示 |

---

### #4053 — 子代理预算和递归限制

**Issue：** 子代理必须遵守 token 预算、递归深度限制和步骤限制。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4053-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked budget -- --nocapture` | 通过 | 测试绿色 |
| 4053-B | 预算执行 | 用极小的 token 预算启动子代理 | 子代理在预算耗尽时停止，而不是静默挂起 | 预算耗尽的子代理产生 `budget_exceeded` 或等效状态 |
| 4053-C | 递归限制 | 尝试嵌套子代理超过最大深度 | 最深的子代理被拒绝；拒绝原因传播到父级 | 无无限递归 |

---

### #4054 — 目标完成门控和验证

**Issue：** 会话目标关闭必须接受验证或声明 `not_applicable`；已接受的完成不得停止继续循环。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4054-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked update_goal_accepts_not_applicable -- --nocapture` | 通过 | 测试绿色 |
| 4054-B | 实时目标关闭 | `/goal Summarize what CodeWhale is in two sentences for a README intro` → 让代理工作 → 确保它以 `not_applicable` 验证调用目标完成 | 目标侧边栏显示 **complete**；经过的计时器冻结 | 目标变为非活跃 |
| 4054-C | 无继续循环 | 在 4054-B 之后，等待一个空闲周期（不要发送新消息） | 不自动重新注入继续轮次 | 接受的完成后会话保持空闲 |

---

### #4056 — 稳定功能不标记为实验性

**Issue：** 会话配置不得将已发布工具（`mcp`、`web_search`、`apply_patch`、`exec_policy`、`subagents`）标记为实验性；`vision_model` 是 **beta**。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4056-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked config_view_experimental -- --nocapture` | 通过 | 测试绿色 |
| 4056-B | 配置 UI | 在 TUI 中：`/config` → 滚动到 **Experimental** 部分 | 仅列出 **beta/实验性** 功能（`vision_model` = beta）；稳定工具不在 Experimental 中出现 | Experimental 下无 `mcp`/`subagents`/等 |
| 4056-C | 目标/workflow 文案 | 过滤 `goal` 和 `workflow` 的配置 | 文案描述实时命令；无"预览占位符"措辞 | 专业、准确的描述 |
| 4056-D | CLI 功能表 | `codewhale-tui features list`（或 `codew features list`） | `shell_tool`、`subagents`、`mcp` 等显示阶段为 **stable** | 与发布现实匹配 |

---

### #4062 — 提供商 onboarding（非仅 DeepSeek）

**Issue：** 首次运行 onboarding 必须让用户选择提供商并通过 `save_api_key_for(provider, …)` 路由密钥。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4062-A | 全新 onboarding 流程 | 新的 `CODEWHALE_HOME`；**不带** `--skip-onboarding` 启动 | 欢迎 → 语言 → **提供商选择器**（按键 1–8：DeepSeek、OpenAI、Anthropic、OpenRouter、Z.ai、Moonshot、SiliconFlow、Ollama） | 提供商步骤存在；文案不限于 DeepSeek |
| 4062-B | 非 DeepSeek 密钥路由 | 选择提供商 `4`（OpenRouter）或 `2`（OpenAI）；粘贴测试密钥；完成 onboarding | 密钥存储在配置/secrets 中的正确提供商槽位下 | `doctor --json` 显示所选提供商就绪；密钥不在仅 `deepseek` 槽位中 |
| 4062-C | 提供商中立文案 | 阅读 API 密钥步骤标题/正文 | 无"连接你的 DeepSeek API 密钥"作为唯一路径 | 中立或提供商特定的文案与选择匹配 |

**Onboarding 按键：** `Enter` 前进 · `Esc` 后退 · `1`–`7` 语言 · 提供商步骤：`1`–`8` 选择提供商 · 信任：`y`/`n`（纯 `Enter` **不得**静默授予信任）

---

### #4063 — 设置向导滚动（PageDown）

**Issue：** 长设置步骤正文必须支持 PageUp/PageDown 滚动；滚动在步骤更改时重置。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4063-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked setup_wizard_body_scroll -- --nocapture` | 通过 | 测试绿色 |
| 4063-B | 打开向导 | `/setup` 或在 onboarding 后完成设置检查点 | 设置向导模态框打开 | 向导可见 |
| 4063-C | 滚动长步骤 | 选择 **Constitution** 或 **Runtime Posture** 步骤；反复按 `PageDown`（终端 ≥ 80×24） | 正文内容滚动；较早的行移出视野 | 折叠以下的内容变得可读 |
| 4063-D | 步骤更改时重置 | 向下滚动，然后按 `Down`/`Right`/`n` 到下一步 | 滚动位置重置到顶部 | 新步骤从偏移量 0 开始 |
| 4063-E | PageUp | 在 PageDown 之后，按 `PageUp` | 滚动向上移动 | 双向滚动有效 |

**设置向导按键：** `Esc`/`q` 关闭 · `Left`/`b` 上一步 · `Right`/`n` 下一步 · `Up`/`Down` 也可更改步骤 · `PageUp`/`PageDown` 正文滚动 · `s` 跳过步骤 · Constitution：`1`–`6` 循环答案，`g` 引导保存，`a` 模型草稿，`u` 捆绑/默认

---

### #4057 — 语言包

**Issue：** 已发布的 UI 语言包（en、ja、zh-Hans、es-419、pt-BR、vi）必须相对于 `en.json` 完整；**zh-Hant** 是有意不完整的，会回退到英语。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4057-A | JSON 有效性 | `jq empty crates/tui/locales/*.json`（从仓库执行） | 所有文件都是有效 JSON | 退出码 0 |
| 4057-B | 奇偶校验测试 | `for f in localization missing_message; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | 通过 | 完整语言包无缺失键 |
| 4057-C | 非英语设置 | Onboarding 语言：选择 `3`（zh-Hans）或 `2`（ja）；打开 `/setup` | 设置向导标题/步骤以所选语言显示 | 完整语言包的 UI 不是英语 |
| 4057-D | zh-Hant 部分 | 选择 zh-Hant（繁体中文）；打开 `/setup` | 混合 zh-Hant + 英语回退可接受；不宣传为完全本地化 | 记录是否出现回退字符串；无崩溃 |
| 4057-E | Workflow 措辞 | 在 ja 或 zh-Hans 中，运行 `/workflow` 帮助或斜杠菜单描述 | 使用 workflow 术语；无过时的"swarm"措辞 | 术语一致 |

**完整语言包（v0.8.67）：** en、ja、zh-Hans、es-419、pt-BR、vi。**部分：** zh-Hant（`Locale::is_partial_pack()`）。

---

### #4058 — 模型定价提示（`glm-5.2`、`kimi-k2.7-code`）

**Issue：** 模型选择器和注册表在已知的情况下暴露当前模型及其定价元数据。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4058-A | 单元回归 | `for f in pricing model_catalog; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | 通过 | glm-5.2 和 kimi-k2.7-code 定价测试绿色 |
| 4058-B | 模型选择器提示 | `/model` → 搜索或滚动到 `glm-5.2` 和 `kimi-k2.7-code`（或提供商限定的 id，如 `z-ai/glm-5.2`、`moonshotai/kimi-k2.7-code`） | 行提示在目录有数据时包含 **`priced`**（不是 `price unknown`） | 两个模型都显示 priced 提示 |
| 4058-C | 捆绑目录 | `jq '.entries["glm-5.2"], .entries["kimi-k2.7-code"]' crates/tui/assets/model_catalog.bundled.json` | 条目存在且带有元数据 | 非空目录条目 |
| 4058-D | LongCat 标签 | 打开提供商选择器 `/provider`；找到 LongCat | 标记为 Meituan LongCat（或等效的专业标签） | 提供商信息不过时 |

---

### #4059 — 运行中工具上的旋转动画（手动）

**Issue：** 单个运行中的工具应显示可见的状态动画（鲸鱼喷水/盲文旋转），而非冻结的行。

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| 4059-A | 单元回归 | `cargo test -p codewhale-tui --bin codewhale-tui --locked status_animation -- --nocapture` | 通过 | 测试绿色 |
| 4059-B | 实时旋转 | 在 Agent 模式下启动 `Run a slow test command` | 运行中的行显示动画（盲文旋转或鲸鱼喷水图案） | 可见动画，非冻结 |
| 4059-C | 完成后停止 | 等待命令完成 | 旋转停止；行显示最终退出状态 | 完成后无残留动画 |

---

### Constitution 测试（设置向导和 `/constitution`）

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| CON-A | Constitution 管理器打开 | `/constitution` | 管理器视图打开并显示当前或默认 constitution | 无崩溃，可读文本 |
| CON-B | 引导保存 | `/setup` → Constitution 步骤 → `g` | 预览出现；按 `g` 再次保存 | Constitution 写入并重新加载 |
| CON-C | 模型草稿 | Constitution 步骤 → `a` | 草稿出现；不自动保存 | 草稿可审查；按 `g` 批准或按 `Esc` 放弃 |
| CON-D | 捆绑默认 | `u` 恢复捆绑默认 | 默认文本重新加载 | 默认可见 |

---

### Workflow 基础（斜杠命令和面板）

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| WF-A | Workflow 状态面板 | `/workflow status`（无活动运行时） | 面板显示"无活动运行"或等效信息 | 无错误，面板渲染 |
| WF-B | Workflow 帮助 | `/workflow help` | 列出子命令 | 帮助文本可读 |
| WF-C | 运行内联 Workflow | `/workflow run` 带最小内联规范 | 启动、显示阶段行、完成 | 运行完成 |
| WF-D | Workflow 取消 | 在运行期间：`/workflow cancel <run_id>` | 运行状态变为 cancelled | 无挂起的 worker |
| WF-E | 软自动门控 | 在 Plan 模式下，提出多步骤审计 | 软自动可能建议编排但不静默启动 | 无自动写入或未经请求的 worker 启动 |

---

### Fleet 基础（斜杠命令和设置）

| ID | 目标 | 步骤 | 预期 | 通过标准 |
| --- | --- | --- | --- | --- |
| FL-A | Fleet roster 打开 | `/fleet` 或 `/fleet roster` | 显示已配置的角色，以及一个用于添加配置文件的操作行 | 无崩溃，可读列表 |
| FL-B | Fleet 设置向导 | `/fleet setup` | 角色选择 → 模型选择 → 思考等级 → 审查 → 批准 | 在批准前不保存任何内容 |
| FL-C | 模型草稿 | 在 Fleet 设置审查步骤按 `m` | 草稿出现；权限保持在 fleet 下限（无 shell、无 trust、需要审批） | 审查步骤上内联显示草案 TOML |
| FL-D | 批准保存 | 批准 Fleet 配置文件 | 配置文件写入 `.codewhale/agents/<role>` | 文件存在，内容有效 |
| FL-E | Fleet 状态 | `/fleet status` | Worker 状态视图，无崩溃 | 面板渲染 |

---

## 命令参考

在测试期间保持此矩阵可见：

| 命令 | 用途 |
| --- | --- |
| `/setup` | Constitution 优先的设置向导 |
| `/constitution` | Constitution 管理器 |
| `/config` | 会话配置浏览器 |
| `/model` | 模型选择器 + 定价提示 |
| `/provider` | 提供商选择器 |
| `/goal` | 会话目标跟踪 |
| `/workflow` | Workflow 编排选择加入 |
| `/fleet` | Fleet roster / 设置 / 状态 |
| `/mode plan` | 只读调查模式 |

### 模态焦点

当模态框打开时（设置向导、审批、选择器），**全局热栏和编辑器快捷键可能被阻止**。在测试全局按键之前用 `Esc` 关闭。

### 非交互式测试的限制

某些行为**无法**在没有实时 TUI + 模型路由的情况下完全验证：

| 区域 | 交互式 TUI | 无头替代方案 |
| --- | --- | --- |
| 旋转动画 (#4059) | 视觉确认需要 | `cargo test … status_animation` |
| 委派行排序 (#4051) | 在实时 fan-out 期间需要 | `cargo test … agent_activity history` |
| Onboarding 提供商 UX (#4062) | 需要一次 | 检查 `ONBOARDING_PROVIDER_OPTIONS` + `save_api_key_for` 测试 |
| 设置滚动 (#4063) | 溢出 UX 需要 | `setup_wizard_body_scroll_resets_on_step_change` 测试 |
| 语言环境视觉效果 (#4057) | 文案 QA 需要 | `localization`/`missing_message` 测试 + `jq empty locales/*.json` |
| Workflow/Fleet 端到端 | 需要提供商 + Agent 模式 | `codewhale exec --auto` 带 `--output-format stream-json` 用于部分跟踪；命令路由的单元测试 |
| 子代理子进程完成 (#4050–#4053) | 需要 Agent + API | `cargo test -p codewhale-tui --bin codewhale-tui --locked -- subagent` |

**CLI 自动化路径：**

```bash
codewhale exec --auto --output-format stream-json "your prompt here"
```

纯 `codewhale exec` 是仅文本的单次执行（无工具）。使用 `--auto` 进行带工具的非交互式运行。

**推荐的回归测试包（从仓库根目录执行）：**

```bash
cargo test -p codewhale-tui --bin codewhale-tui --locked
cargo test -p codewhale-tui --bin codewhale-tui --locked tools::workflow::tests -- --nocapture
for f in subagent setup constitution localization experimental_config pricing model_catalog status_animation fleet_roster fleet_setup; do
  cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture
done
scripts/v0867-setup-qa.sh
```

---

## 报告模板

将本节复制到你的内部测试报告中并填写。

### 运行元数据

| 字段 | 值 |
| --- | --- |
| 日期 | |
| 测试者（代理/人类） | |
| `codewhale-tui --version` | |
| 平台（OS/架构/终端） | |
| 工作区选项 (A/B/C) | |
| 使用的提供商（如有） | |
| `CODEWHALE_HOME` 已隔离？（是/否） | |

### 结果摘要

| ID | 结果（通过/失败/跳过） | 备注 |
| --- | --- | --- |
| P0 | | |
| P1 | | |
| P2 | | |
| 4050-A/B/C | | |
| 4051-A/B/C | | |
| 4052-A/B/C | | |
| 4053-A/B/C | | |
| 4054-A/B/C | | |
| 4056-A/B/C/D | | |
| 4062-A/B/C | | |
| 4063-A/B/C/D/E | | |
| 4057-A/B/C/D/E | | |
| 4058-A/B/C/D | | |
| 4059-A/B/C | | |
| CON-A/B/C/D | | |
| WF-A/B/C/D/E | | |
| FL-A/B/C/D/E | | |

**总计：** ___ 通过 / ___ 失败 / ___ 跳过

### 阻塞项

| 严重性 | ID | 摘要 | 复现步骤 | 证据 |
| --- | --- | --- | --- | --- |
| P0 / P1 / P2 | | | | |

### 观察结果（非阻塞）

- 
- 

### 无头测试日志（可选）

```
在此粘贴 cargo test / scripts/v0867-setup-qa.sh 输出摘要
```

---

## 参考资料

- [CHANGELOG.md](../CHANGELOG.md) — v0.8.67 发布说明 (2026-07-06)
- [docs/evidence/v0867-constitution-setup-qa-matrix.md](evidence/v0867-constitution-setup-qa-matrix.md)
- [docs/KEYBINDINGS.md](KEYBINDINGS.md)
- [docs/MODES.md](MODES.md)
- [docs/FLEET.md](FLEET.md)
- GitHub 里程碑 issue：#4050–#4054、#4056–#4059、#4062–#4063
