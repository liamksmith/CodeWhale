# 子代理

子代理是面向用户的嵌套工作分配词汇：父进程通过 `agent` 启动一个聚焦角色（`explore`、`review`、`implementer`、`verifier` 等），并在 worker 运行时获得一个 `agent_id` 加上 transcript 句柄。

架构上，子代理不应成为第二个执行基础设施。持久化原语是 [`AGENT_RUNTIME.md`](AGENT_RUNTIME.md) 中描述的 fleet 支持的 worker 运行：重试、终端状态、收据、工件引用、检查和重启行为都属于那里。面向模型的启动器是单一的 `agent` 工具，分离的工作应收敛到与 Agent Fleet 相同的生命周期。

当前的 `agent` 实现在过渡完成期间委托给持久化子代理运行时。它对于短期的会话内委派仍然有用。瞬态 provider 头/流/超时故障会在子运行时内以退避方式重试，直到 worker 被标记为中断；如果重试预算耗尽，CodeWhale 会保留一个检查点并返回一个续接句柄，而不是让父进程去推断发生了什么。对于必须在进程重启、休眠或远程执行后存活的工作，请优先使用 Fleet 或 Workflow 支持的 fleet 运行。

子代理默认继承父进程的工具注册表，但子代理是叶子 worker：它们不会收到 `agent` 或嵌套的生命周期工具。`agent` 启动分离的后台工作：取消父进程的 turn 会停止父进程的等待路径，但不会杀死已经打开的子运行。

本文档涵盖角色分类和当前的兼容性控制。活跃的编排接口是 `agent`；参见
`crates/tui/src/prompts/constitution.md` 的"子代理策略"和行内工具描述。

## 角色分类

`agent` 上的 `type` 字段为子进程选择一个系统提示姿态（`agent_type` 被接受为兼容性别名）。每个角色都是对工作的独特立场——而不仅仅是一个不同的标签。

## 维护者姿态

子代理帮助 CodeWhale 更快地前进，但父代理仍然拥有维护者的决定权。使用子进程来收集证据、审查补丁和运行验证，同时保持 [`AGENT_ETHOS.md`](AGENT_ETHOS.md) 中的社区姿态：issue 是开放的入口，PR 门控是审查负载控制，harvest 的工作需要清晰的贡献者署名。

当子进程审查社区工作时，父进程仍应在合并、收获、关闭或推迟之前检查 PR diff、关联的 issue、测试和 CI。子代理的结果是一个工作集，而不是管理的替代品。

| 角色 | 立场 | 写入？ | Shell 姿态 | 典型用途 |
|---------------|----------------------------------------|---------|---------------|----------------------------------------------|
| `general` | 灵活；执行父进程的任何指令 | yes | yes | 默认；多步骤任务 |
| `explore` | 只读；快速映射相关代码 | no | 只读 | "找到 `Foo` 的每个调用点" |
| `plan` | 分析并产出策略 | minimal | minimal | "设计迁移方案；不执行" |
| `review` | 读取并评分，带有严重程度等级 | no | 只读 | "审计此 PR 的 bug" |
| `implementer` | 在范围约束内落地指定的更改 | yes | yes | "修复 #4567 中的竞态条件" |
| `verifier` | 对测试套件或验证运行并报告通过/失败 | no | 只读 | "验证此补丁修复了 #4567" |
| `custom` | 由调用者通过 `allowed_tools` 单独定义 | 取决于 allowlist | 取决于 allowlist | 旧版/内部约束接口 |

## 探索器输出模板

探索器、审查者和验证者全都输出结构化报告。探索器应遵循子代理输出格式（[SUMMARY, EVIDENCE, GAPS, NEXT]），并加上 STOP_CONDITION 检查。输出存储在 `crates/tui/src/prompts/subagent_output_format.md`。

## 子代理简报

父进程应使用紧凑的子代理简报来调用 `agent`，包含 QUESTION、SCOPE、ALREADY_KNOWN、EFFORT、STOP_CONDITION 和 OUTPUT 部分。

示例：

```text
QUESTION：`rlm_open` 加载源时是否有任何文件系统或 shell 逃逸？
SCOPE：`crates/tui/src/tools/rlm*`、`crates/tui/src/tools/sandbox*`、`crates/tui/src/tools/exec*`。
ALREADY_KNOWN：RLM 上下文在工作区边界内运行。检查文件路径是否解析到工作区之外。
EFFORT：medium
STOP_CONDITION：找到至少一个 MAJOR issue 后停止，或收集足够证据确认没有 MAJOR+ issue。
OUTPUT：VERDICT、EVIDENCE（带 file:line 引用或 PR 引用）、GAPS、NEXT。
```

```text
QUESTION：子代理提示在哪里组装？
SCOPE：crates/tui/src/prompts*、crates/tui/src/tools/subagent/*。
ALREADY_KNOWN：面向模型的启动器只有 `agent`；不要查找已移除的生命周期工具。
EFFORT：quick
STOP_CONDITION：在识别提示源文件和包装分配文本的函数后停止。
OUTPUT：VERDICT、EVIDENCE、GAPS、NEXT。
```

```text
QUESTION：聚焦的 prompt/subagent 测试过滤器是否有效，如果无效会失败什么？
SCOPE：cargo test -p codewhale-tui --bin codewhale-tui --locked prompt；如果需要再加 subagent 过滤器。
ALREADY_KNOWN：不要修复失败；捕获确切的命令、退出码和第一个相关的断言。
EFFORT：medium
STOP_CONDITION：在一个干净的 PASS 或一个可复现的失败断言（带命令证据）后停止。
OUTPUT：VERDICT、EVIDENCE、GAPS、NEXT。
```

### 何时选择哪个角色

- **`general`** — 当任务是"完成这整件事"，而不是"去看看"、"设计"或"验证"。这是正确的默认值；只有在姿态很重要时才选择更具体的角色。
- **`explore`** — 当父进程在决定下一步之前需要证据。探索器便宜且快速；对独立区域并行打开 2-3 个。
  它们应首先定向：确认项目根目录，在不熟悉的树中阅读相关的
  `AGENTS.md`/`README.md` 指导，仅搜索可能的范围，并返回 `path:line-range` 证据而不是叙述性导览。要使用的角色名称是 `explore` 或 `explorer`。
- **`plan`** — 当父进程有目标但没有可执行的分解时。规划器写入工件（`update_plan` 行、
  `checklist_write` 条目）但不执行它们。
- **`review`** — 当已经有更改并且父进程想要对其进行评分时。审查者不修补——他们在发现中描述修复方案，以便父进程在裁决为"修复它"时派遣 Implementer。
- **`implementer`** — 当更改已经被指定且只需要落地时。Implementer 保持严格的范围：最小化编辑，不做顺手重构，在交回之前运行快速验证。
- **`verifier`** — 当父进程需要对测试套件或其他验证进行权威的通过/失败判断时。验证者不修复失败；他们捕获失败的断言 + 堆栈并将修复候选放在 RISKS 下。
- **`custom`** — 仅当父进程需要显式约束工具集时。通过旧版/内部子代理记录上的 `allowed_tools` 字段传递 allowlist；面向模型的 `agent` 工具保持公共模式有意精简。

### 别名

模型可以通过多种方式拼写每个角色：

| 规范名称 | 别名 |
|---------------|------------------------------------------------------------------|
| `general` | `worker`、`default`、`general-purpose` |
| `explore` | `explorer`、`exploration` |
| `plan` | `planning`、`planner`、`awaiter` |
| `review` | `reviewer`、`code-review`、`code_review` |
| `implementer` | `implement`、`implementation`、`builder` |
| `verifier` | `verify`、`verification`、`validator`、`tester` |
| `custom` | (无；需要显式的 `allowed_tools` 数组) |

所有匹配不区分大小写。未知值会产生一个列出可接受集合的类型化错误，以便模型可以在下一个 turn 自我纠正。

## 并发上限

默认情况下最多 **64** 个子代理并发运行（`DEFAULT_MAX_SUBAGENTS`），可通过 `~/.codewhale/config.toml` 中的 `[subagents].max_concurrent` 配置，上限为 **128**（`MAX_SUBAGENTS`）。会话默认允许最多 **200** 个运行中加排队的子代理的有界队列，这样一个 turn 可以请求广泛的扇出，让管理器消耗它而不会创建无界数量的工作。

默认情况下，每个被允许的子进程可以立即启动——没有人为的节流。如果你想要更温和的扇出，降低 `[subagents].launch_concurrency`（一次启动多少直接子进程）；超过该限制的子进程**排队**等待启动槽位而不是突发启动。`launch_concurrency` 默认为解析后的 `max_subagents` 上限。（v0.8.61 之前的 `interactive_max_launch` 键仍然被接受为已弃用的别名；当两者都设置时，新键优先。）

高扇出 Workflow 可以通过 `[subagents] max_admitted`（别名：`max_total`、`admission_limit`）调整该有界数量。该总上限同时计算**运行中**和**排队中**的代理，而 `launch_concurrency` 保持即时执行有界。已完成/失败/已取消的记录保留以供检查，但不占用准入槽位。丢失了 `task_handle` 的代理（例如跨进程重启）也不计入上限。

Provider 配置文件允许一个配置对直接 API 路由保持激进，同时保持订阅或聚合路由温和。`[subagents.providers.<provider>]` 下的每个键在省略时继承自 `[subagents]`。Provider 键接受规范名称如 `deepseek`、`zai`、`openrouter`，以及别名如 `glm`（Z.ai）：

```toml
[subagents]
# 没有配置文件的 provider 的全局回退。
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200
max_depth = 6
token_budget = 100000

[subagents.providers.deepseek]
# 直接 API key，有扇出空间。
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200

[subagents.providers.glm]
# Z.ai / GLM 订阅式路由：保持压力紧凑。
max_concurrent = 4
launch_concurrency = 3
max_admitted = 12
max_depth = 2
api_timeout_secs = 180
heartbeat_timeout_secs = 240

[subagents.providers.openrouter]
max_concurrent = 5
launch_concurrency = 3
max_admitted = 20

[subagents.providers.anthropic]
max_concurrent = 3
launch_concurrency = 2
max_admitted = 12
```

使用 `/config subagents status` 查看全局值和活跃 provider 的已解析扇出、深度和超时配置文件。

## Token 预算管理器

设置 `[subagents].token_budget` 为每个根 `agent` 运行提供一个聚合 token 上限，由该子进程及其所有后代共享。子进程也可以通过面向模型的 `agent` 工具的 `token_budget` 字段启动一个新的作用域预算（`max_tokens` 别名被接受用于 Workflow 形状的调用者）。当没有配置或提供预算时，行为不变。

每个子模型调用完成时，provider 报告的输入和输出 token 会被折叠到 worker 记录中。持久化的 `usage` 对象显示 worker 自己的总数加上共享作用域的聚合 `budget_spent_tokens` 和 `budget_remaining_tokens`。一旦共享作用域耗尽，进一步的后代生成将被拒绝，并附上可操作的消息，而不是在已耗尽的池中打开更多代理。

## 每个角色的模型 (#3018)

子进程可以在与父进程不同的模型上运行。两个配置接口提供相同的覆盖映射（冲突时 `[subagents.models]` 键优先，键不区分大小写）：

```toml
[subagents]
default_model  = "deepseek-v4-flash"   # 每个角色的回退
worker_model   = "deepseek-v4-pro"     # worker / general
explorer_model = "deepseek-v4-flash"   # explorer / explore
awaiter_model  = "deepseek-v4-flash"   # awaiter / plan
review_model   = "deepseek-v4-pro"     # review
custom_model   = "deepseek-v4-pro"     # custom

[subagents.models]
# 自由形式的 role → model 映射；agent 接受的任何角色别名都有效。
implementation = "deepseek-v4-pro"
```

模型 ID 可以是**活跃 provider 接受的任何模型**——验证是 provider 感知的，并在生成时发生，而不是加载时。在官方 DeepSeek API 上，只有 DeepSeek ID 被接受；其他每个 provider 将 ID 透传给 provider API，后者是权威。一个非 DeepSeek 示例：

```toml
provider = "moonshot"
model = "kimi-k2.7-code"

[subagents]
worker_model = "kimi-k2.6"
```

模型 ID 在应用于子路由时以相同方式验证；官方 DeepSeek API 上的无效 ID 会使生成失败并返回可接受 ID 列表，而不是不透明的 provider 400。

使用 `/model auto` 时，子代理路由也是 provider 感知的：具有已知大/便宜配对的 provider（DeepSeek，以及 NVIDIA NIM、OpenRouter、Novita、SiliconFlow、SGLang、vLLM 上的托管 DeepSeek 路由）在该配对之间路由；没有已知便宜等级的 provider（例如 Ollama、Moonshot）跳过网络路由器并保持子进程在会话模型上。

## 每个配置文件的 Provider 路由 (#3965)

`[subagents.models]` 在活跃 provider 内更改子模型。要将子进程固定到不同的 provider，使用 Fleet/AgentProfile 并通过 `profile` 传递给面向模型的 `agent` 工具。配置文件的显式 `provider` + `model` 字段优先于父会话路由；省略 `provider` 会保留现有的继承行为。

示例：保持父会话在 DeepSeek 上，但在本地 LM Studio OpenAI 兼容端点上运行格式化子进程：

```toml
# ~/.codewhale/config.toml 或工作区配置
provider = "deepseek"

[providers.deepseek]
api_key = "YOUR_DEEPSEEK_KEY"

[providers.lm-studio]
kind = "openai-compatible"
base_url = "http://127.0.0.1:1234/v1"
api_key = "lm-studio"
model = "qwen-2.5-7b"
```

```toml
# .codewhale/agents/local-formatter.toml
id = "local-formatter"
role_hint = "formatter"
provider = "lm-studio"
model = "qwen-2.5-7b"
reasoning_effort = "off"

[instructions]
text = "使用小型、本地化编辑。保持格式更改机械化。"
```

然后调用 `agent(profile: "local-formatter", prompt: "...")`。进程内子进程为 `lm-studio` 构建客户端；Fleet worker 将 `--provider lm-studio` 转发给 `codewhale exec`，后者解析相同的 `[providers.lm-studio]` 表。未知或未配置的 provider ID 会使生成失败，而不是静默回退到父 provider。

## 每步 API 超时 (#1806, #1808)

每个子代理步骤将其 DeepSeek `create_message` 调用包装在每步超时中，以便单个卡住的请求不会无限期地固定父进程的完成唤醒通道。默认值为 `120` 秒，与旧版硬编码值匹配。合法超过该时间的长思考子进程，例如 `agent` 背后的重度计划或审查工作，可以在 `~/.codewhale/config.toml` 中延长超时：

```toml
[subagents]
api_timeout_secs = 900  # 15 分钟；限制在 1..=1800
```

值被限制在 `1..=1800`。`0` 和 `unset` 保持旧版 `120` 秒默认值，因此现有安装不会看到行为变化。

## 过期代理心跳 (#2614)

运行中的代理还会跟踪管理器可见的进度。如果子进程停止发出进度超过心跳窗口，管理器会自动取消它，释放其子代理槽位，并通过返回的 transcript 句柄和持久化的 worker 记录保持已取消记录可检查。默认值为 5 分钟：

```toml
[subagents]
heartbeat_timeout_secs = 300  # 限制在 30..=3600
```

有效心跳保持在 `api_timeout_secs` 之上至少 30 秒，因此配置的长模型请求在其自己的请求超时触发之前不会被取消。

## 生命周期

每个打开的会话产生一个记录，经历以下阶段：

```
Pending → Running → (Completed | Failed(reason) | Cancelled | Interrupted(reason))
```

`Interrupted` 在管理器检测到一个 `Running` 代理的 task 句柄消失时触发——通常是在进程重启后，从 `.codewhale/state/subagents.v1.json` 加载工作区的持久化状态时。父进程可以使用相同的分配打开替代会话，或将其视为终端状态。

### 会话边界 (#405)

每个 `SubAgentManager` 实例在构造时为自己分配一个新的 `session_boot_id`。每个新会话用该 ID 标记代理；工作区状态文件记录它用于重启恢复。

侧边栏/状态投影默认关注当前会话的代理。不再运行的前会话代理被视为归档记录，因此模型不会将过期工作误认为活跃工作。

从 #405 之前持久化状态文件加载的记录（没有 `session_boot_id` 字段）被归类为前会话，因为管理器无法将它们匹配到当前启动。

## 运行收据、跟进和接管

每个兼容性子代理在 `.codewhale/state/subagents.v1.json` 中有一个持久化的 worker 记录。该记录是子代理通道的当前运行分类账切片，直到这些通道直接由 fleet 分类账支持：它存储 `run_id`、目标、角色/模型、工作区/分支、生命周期事件、工件引用、跟进目标、接管目标、使用来源和验证来源。

`agent` 在顶层和 `worker_record` 内返回带有这些字段的会话投影。正常的父进程契约不是轮询：继续工作，在子进程完成时消费完成事件。如果需要审计详细信息，使用 `handle_read` 检查返回的 `transcript_handle`。

旧版跟进递送仅为旧 transcript 和内部恢复保留。如果消息已递送，worker 记录存储一个有界预览和时间戳。新的面向模型流程应在子进程的分配不再适用时打开替代 `agent`。

工件是符号引用。使用 `handle_read` 对返回的 `transcript_handle` 获取 transcript 详细信息，并将 `result_summary` 视为子进程自我报告，除非 `verification.status` 指向单独的验证门或收据。`usage.status` 为 `unknown`，直到 provider 使用量被报告；然后切换到 `reported`，或当配置的共享 token 预算没有剩余 token 时切换到 `budget_exhausted`。

## 输出契约

每个子代理生成一个最终结果字符串，包含五个部分，按顺序排列：

```
SUMMARY：    一个段落；你做了什么以及发生了什么
CHANGES：    修改的文件，附一行描述；如果只读则为 "None."
EVIDENCE：   path:line-range 引用和关键发现；每项一个要点
RISKS：      可能出错的地方 / 父进程应仔细检查的内容
BLOCKERS：   阻止你完成的原因；如果干净完成则为 "None."
```

确切格式在 `crates/tui/src/prompts/subagent_output_format.md` 中。父进程将 `EVIDENCE` 作为下一个 turn 的工作集来读取，因此探索器和审查者在此处应保持精确。

## 记忆与 `remember` 工具 (#489)

子代理在启用记忆时继承父进程的记忆文件
（`[memory] enabled = true` 或 `DEEPSEEK_MEMORY=on`）。它们可以通过 `remember` 工具追加持久化笔记——这对于发现值得跨会话保留的项目约定的探索器，或了解到"此测试不稳定"的验证者来说很方便。

记忆写入范围限定在用户自己的 `memory.md` 文件；它们不经过标准的写入批准流程。

## 实现说明

- 源文件：`crates/tui/src/tools/subagent/mod.rs`。
- 持久化状态：`<workspace>/.codewhale/state/subagents.v1.json`。Schema 版本 `1`（向前兼容——新的可选字段使用 `#[serde(default)]`）。
- Worker 记录按时间修剪：已完成/失败/已取消/中断的记录在与已完成代理相同的保留窗口后逐出（默认 1 小时，`COMPLETED_AGENT_RETENTION`）。运行中/启动中/等待中的记录被保留。256 条记录的硬上限作为安全边界保留（#4217）。
- `SubAgentRuntime::background_runtime()` 从 `child_runtime()` 启动，但用新的取消 token 替换 turn 范围的子 token，因此父 turn 取消不会停止分离的后台会话。
- `is_running` 检查忽略 `task_handle` 为 `None` 的代理；这避免将持久化但已分离的记录计入并发上限（#509）。
- `SharedSubAgentManager` 是 `Arc<RwLock<...>>`——读取路径使用读锁，因此 `/agents` 和侧边栏投影在多代理扇出期间不会阻塞主循环（#510）。
