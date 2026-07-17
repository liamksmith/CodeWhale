# 工具界面

为什么是这些特定工具、按此分组，以及每个工具应如何优先于等效 Shell 命令使用。配套文档：`crates/tui/src/prompts/agent.txt`。

## 设计立场

- **专用工具优先于 `exec_shell`，只要专用工具返回结构化输出。** Bash 转义容易出错且平台行为各异（GNU vs BSD `grep`，`rg` 不一定安装）。结构化输出也让模型免于重新解析自由格式文本。
- **`exec_shell` 用于其他一切。** 构建、测试、格式化、lint、临时命令、任何平台特定的东西。我们不试图覆盖长尾场景。
- **移除不优于其 Shell 等价物的工具。** 同一底层操作的两个工具别名是模型陷阱——LLM 会在两者之间切换，缓存命中率受损。

## 当前界面 (v0.8.49)

### 文件操作

| 工具 | 定位 |
|---|---|
| `read_file` | 读取 UTF-8 文件。PDF 通过内置纯 Rust 提取器自动提取（无需安装 Poppler）；`pages: "1-5"` 可切片大型文档。 |
| `list_dir` | 结构化、gitignore 感知的目录列表。优先于 `exec_shell("ls")`。 |
| `write_file` | 创建或覆盖文件。 |
| `edit_file` | 单文件内的搜索替换。比完整重写更轻量。 |
| `apply_patch` | 应用 unified diff。多块编辑的正确工具。 |
| `retrieve_tool_result` | 读取先前大型工具输出（溢出到 `~/.codewhale/tool_outputs/`）的摘要或切片；使用 `summary`、`head`、`tail`、`lines` 或 `query` 模式，而非重放整个结果。 |
| `handle_read` | 从活跃工具环境持有的 `var_handle` 负载中读取有界投影。这是 RLM 会话、子代理转录和其他大型符号负载的基础。 |

### 搜索

| 工具 | 定位 |
|---|---|
| `grep_files` | 工作区内正则搜索文件内容；结构化匹配 + 上下文行。纯 Rust（`regex` crate），不通过 shell 调用 `rg`/`grep`。 |
| `file_search` | 模糊匹配文件名（非内容）。当大致知道名称时使用。 |
| `web_search` | 默认 DuckDuckGo 加 Bing 回退；Bing、Tavily、Bocha、Metaso、SearXNG、Baidu、Volcengine 和 Sofya 可在配置中选择。排名的片段 + `ref_id` 用于引用。 |
| `fetch_url` | 对已知 URL 直接 HTTP GET。当链接已知时比 `web_search` 更快。默认将 HTML 剥离为纯文本。 |

### Shell

Shell 工具仅当当前会话或配置文件启用了 shell 访问时，才出现在模型可见的工具目录中。交互式 TUI Agent 会话默认暴露 shell 并带审批提示，除非顶层 `allow_shell = false` 将其隐藏。无头、持久任务和其他非交互式配置文件保持保守的省略字段默认值，需要 `allow_shell = true`。YOLO 模式自动启用 shell 访问。Plan 模式保持关闭 shell 执行。

| 工具 | 定位 |
|---|---|
| `exec_shell` | 运行 shell 命令。前台运行可取消，但仅用于有界命令；超时会杀死进程并返回后台重运行提示。 |
| `exec_shell_wait` | 轮询后台任务的增量输出。取消当前回合停止等待而不杀死任务。 |
| `exec_shell_interact` | 向运行中的后台任务发送 stdin 并读取增量输出。 |
| `exec_shell_cancel` | 按 id 取消一个运行中的后台 shell 任务，或在显式请求时取消全部。 |

### Git 检查

| 工具 | 定位 |
|---|---|
| `git_status` | 工作区 `git status --porcelain=v1 -b`。 |
| `git_diff` | `git diff` 含上下文截断。支持 `--cached` 和子目录范围。 |
| `git_log` | `git log` 含作者/日期过滤器和最多条目数上限。 |
| `git_show` | 针对特定修订版的 `git show`；默认为 patch + stat。 |
| `git_blame` | 逐文件 `git blame`；默认最多 200 行，上限 2000 行。 |

### GitHub 上下文和受保护的写入

| 工具 | 定位 |
|---|---|
| `github_issue_context` | 通过 `gh issue view` 只读获取 issue 上下文；大型正文在可能时转为任务工件。 |
| `github_pr_context` | 通过 `gh pr view` 只读获取 PR 上下文；可选的 diff 捕获通过 `gh pr diff --patch`；大型正文/diff 在可能时转为任务工件。 |
| `github_comment` | 需审批的 issue/PR 评论，附带结构化证据。 |
| `github_close_issue` | 需审批的 issue 关闭。要求非空验收标准和证据；除非显式允许，拒绝脏工作树。切勿用于 PR。 |
| `github_close_pr` | 需审批的 PR 关闭。要求与 issue 关闭相同的结构化证据，并在工具输出/审计记录中保持 PR 措辞。 |

### PR 尝试

| 工具 | 定位 |
|---|---|
| `pr_attempt_record` | 将当前 git diff 捕获为持久任务上的尝试元数据和 patch 工件。 |
| `pr_attempt_list` | 列出任务上记录的尝试。 |
| `pr_attempt_read` | 检查一条已记录尝试及其工件引用。 |
| `pr_attempt_preflight` | 对尝试的 patch 运行 `git apply --check`。不改变工作树。 |

### 自动化

| 工具 | 定位 |
|---|---|
| `automation_create` | 创建计划自动化。需审批。 |
| `automation_list` / `automation_read` | 检查持久自动化及其近期运行。 |
| `automation_update` | 更新提示词、计划、工作目录或状态。需审批。 |
| `automation_pause` / `automation_resume` / `automation_delete` | 生命周期控制。需审批。 |
| `automation_run` | 立即运行自动化；运行将普通持久任务入队。需审批。 |

### 子代理

v0.8.33 开始将大型工具输出转向符号句柄：工具返回小型 `var_handle` 对象，`handle_read` 从后端环境中检索有界切片、计数或 JSON 投影。这使父转录保持精简，同时保留到完整负载的恢复路径。

活跃的模型面子代理界面有意保持精简：

| 工具 | 定位 |
|---|---|
| `agent` | 启动一个聚焦的子运行。返回 agent id、紧凑回执和转录句柄，父代理可继续协调。 |

参见 `agent.txt` 了解委托协议，参见 [`SUBAGENTS.md`](SUBAGENTS.md) 了解角色分类（`general` / `explore` / `plan` / `review` / `implementer` / `verifier` / `custom`）。

`agent` 默认使用全新的子对话。传递 `fork_context: true` 用于延续式工作或多视角审查（应继承父上下文）。在 fork 模式下，运行时尽可能保持父 prefill/prompt 前缀字节一致，以便重用 DeepSeek 的前缀缓存，然后附加子角色指令和任务。

### 递归 LM 会话

RLM 现在也是持久的：

| 工具 | 定位 |
|---|---|
| `rlm_session_objects` | 列出活跃提示词、会话元数据、转录、最新用户消息和每条消息引用的紧凑卡片。 |
| `rlm_open` | 基于文件、内联内容或 URL 打开命名 Python REPL。 |
| `rlm_eval` | 对该会话运行有界 Python，使用确定性代码和 REPL 内语义辅助函数如 `sub_query_batch`。 |
| `rlm_configure` | 调整输出反馈、子查询超时/深度和会话共享设置。 |
| `rlm_close` | 关闭 Python 运行时并返回最终会话统计。 |

`rlm_open` 也接受 `session_object`，这是 `rlm_session_objects` 返回的稳定引用，例如 `session://active/system_prompt`、`session://active/transcript` 或 `session://active/messages/0`。这将所选对象加载到 RLM REPL 中，仅向父转录返回元数据。转录对象将思考块和大型工具结果保留为紧凑元数据；通过返回的 `var_handle` 值和 `handle_read` 检查大型负载，而非让父转录粘贴原始文本。

大型 RLM 输出应以 `var_handle` 形式返回。使用 `handle_read` 获取有界文本切片、行范围、计数或 JSONPath 投影，而不是将完整值重放到父转录中。

在 `rlm_eval` 内部，加载的源内容作为 `_context` 可用；`_ctx` 和 `content` 也作为兼容性别名绑定，因为代理在 Python 分析中自然会使用它们。较短的 `context` 和 `ctx` 名称有意不绑定，以便用户变量可以无冲突地使用它们。

子调用超时是会话策略：在运行大规模扇出之前使用 `rlm_configure` 的 `sub_query_timeout_secs`。辅助函数 `sub_query`、`sub_query_batch`、`sub_query_map` 和 `sub_rlm` 接受 `timeout_secs` 关键字以兼容常见的代理猜测，但有效超时仍以 RLM 会话级别配置为准。

`finalize(value, confidence=...)` 保留 JSON 可序列化的值。字符串变为文本句柄；dict、列表、数字、布尔值和 null 变为 JSON 句柄，`handle_read` 可用 JSONPath 投影。

### 会话接力

`/relay [focus]` 要求当前代理将 `.deepseek/handoff.md` 写为紧凑的 `# Session relay` 工件，供下一个线程使用。文件名保留以兼容现有的提示词加载和旧会话；可见的心理模型是 relay / 接力。

别名：`/batonpass`、`/接力`。

在长时间休息、压缩或将工作转移到新会话之前使用它。接力应保留目标、当前 Work 检查清单项、变更的文件、决策、验证状态和一个具体的下一步行动。
将其视为自动压缩的有意对应物：两者都旨在为下一个会话或子代理保留连续性，但 `/relay` 让当前代理检查当前证据并显式选择持久的交接事实。当 `update_plan` 具有丰富的 PlanArtifact 时，`/relay` 包含该策略元数据，以便手动接力、fork 状态和压缩连续性不会漂移为独立的故事。

### 平行扇出：成本等级上限

两个工具提供平行扇出，具有不同的并发限制，反映截然不同的成本等级：

| 工具 | 每个子任务做什么 | 墙钟时间 | Token 成本 | 上限 |
|---|---|---|---|---|
| `agent` | 完整子代理循环（规划、工具调用、多轮流式传输） | 分钟级 | 数千 token | 默认 20 个运行中（`[subagents].max_concurrent`，硬上限 20），默认最多 200 个运行中 + 排队接纳 |
| `rlm_eval` 辅助函数 `sub_query_batch` | 在活跃 RLM 会话中固定到 `deepseek-v4-flash` 的一次性非流式 Chat Completions 调用 | 秒级 | 约数百 token | 每次调用 16 个 |

上限出现在每个工具的描述和错误消息中，以便模型（和用户）可以为工作选择合适的工具。如果一个子代理足够但需要对同一加载上下文进行并行语义查找，优先使用 `rlm_eval` 搭配 `sub_query_batch`；如果每个任务需要自己的携带工具的代理循环，使用 `agent` 并在需要时通过返回的转录句柄检查。

## 已移除的旧版别名和界面

旧的模型面子代理扇出界面已从活跃提示词和工具目录中移除。不要在新的活跃指导中使用已退役的子代理生命周期名称。

旧的一次性 `rlm` 模型面工具也被持久的 `rlm_open` / `rlm_eval` / `rlm_configure` / `rlm_close` 会话替代。

v0.9.0 添加以下隐藏兼容别名 (#2682, #2683)：

| 隐藏别名 | 规范替代 | 状态 |
|---|---|---|
| `checklist_write` | `work_update` | 隐藏，可调用用于重放 (#4132) |
| `checklist_add` / `checklist_update` / `checklist_list` | `work_update` | 隐藏，可调用用于重放 |
| `todo_write` / `todo_add` / `todo_update` / `todo_list` | `work_update` | 隐藏，可调用用于重放 |
| `exec_wait` | `exec_shell_wait` | 隐藏，可调用用于重放 |
| `exec_interact` | `exec_shell_interact` | 隐藏，可调用用于重放 |

所有隐藏别名保持注册和可调用，以便保存的转录可以重放，而无需向新会话教授已弃用的拼写。

## 发布冒烟测试：验证当前名称

验证发布时，直接验证模型可见的注册表名称。不要 grep 随机的处理函数名称；处理函数名称允许漂移，而注册表契约保持稳定。

版本冒烟测试：

```bash
codewhale --version
codewhale-tui --version
```

工具界面冒烟测试：

```bash
rg -n '"handle_read"|"rlm_open"|"rlm_eval"|"rlm_configure"|"rlm_close"|"agent"' crates/tui/src
rg -n 'handle_read|rlm_open|rlm_eval|rlm_configure|rlm_close|agent' docs crates/tui/src/prompts crates/tui/src/tools
```

规范的当前名称：

- `handle_read`
- `rlm_open`、`rlm_eval`、`rlm_configure`、`rlm_close`
- `agent`

注册表不应在历史 changelog 条目之外主动宣传已退役的子代理生命周期名称或旧的前台 `rlm` 工具。

## 额外已注册工具 (v0.8.49)

上述分类表涵盖了最常用的工具。完整注册表还包括以下模型可见工具：

| 工具 | 定位 |
|---|---|
| `web.run` | 基于浏览器的 web 交互（JavaScript 渲染页面、表单填写） |
| `multi_tool_use.parallel` | 在单轮中执行多个独立工具 |
| `request_user_input` | 在回合中提示用户输入 |
| `git_show` / `git_log` / `git_blame` | 检查提交详情、历史和行归属 |
| `load_skill` | 从已安装技能集中按 id 加载技能 |
| `revert_turn` | 将工作区回滚到回合前快照 |
| `pandoc_convert` | 通过 pandoc 在文档格式间转换（受二进制文件存在限制） |
| `validate_data` | 根据模式验证 JSON 或 TOML |
| `code_execution` | 在隔离沙箱中执行 Python 代码 |
| `review` | 带结构化反馈的代码审查 |
| `project_map` | 生成项目工作区的结构映射 |
| `remember` | 将持久事实存储到用户记忆中（受 `memory_enabled` 限制） |
| `image_analyze` | 视觉模型的图像理解（受 `[vision_model]` 配置限制） |
| `image_ocr` | 通过本地 OCR 从图像中提取文本 |
| `finance` | 获取市场数据和股票报价 |

MCP 工具、插件提供的工具和受功能开关限制的工具也可能根据运行时配置可见。使用 `codewhale tools list` 或 TUI `/tools` 面板检查活跃目录。

## 为什么我们不提供单一 `bash` 工具

单一 `bash` 代理（Claude Code 的设计）功能强大，但将 shell 脚本的所有陷阱交给模型：引号问题、平台差异、误读工作目录的副作用、`cd` 在调用间不持久等。我们的文件工具在转录中渲染也显著更轻量（结构化 JSON 形状的输出比 `ls -la` 文本墙折叠得更好）。

模型随时可以回退到 `exec_shell` 当某些功能缺失时。专用工具只是将常见的 80% 从 shell 的逃生舱口中剥离出来。
