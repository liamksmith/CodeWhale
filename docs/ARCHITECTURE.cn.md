# codewhale 架构

本文档为开发者和贡献者提供 codewhale 架构的概述。

当前边界说明（v0.8.67）：
- `crates/tui` 仍然是 TUI、运行时 API、任务管理器和工具执行循环的活跃终端用户运行时。
- 其他工作区 crate 正在逐步拆分，但它们还不是唯一的运行时事实来源。
- LSP 子系统（`crates/tui/src/lsp/`）已完全接入引擎的后工具执行路径
  （`core/engine/lsp_hooks.rs`），在每次 edit_file/apply_patch/write_file 之后提供内联诊断。
- 集群代理系统已在 v0.8.5 中移除。活跃的子代理界面是单一的 `agent` 工具；持久 RLM 会话仍然通过 `rlm_open` / `rlm_eval` / `rlm_configure` / `rlm_close` 可用。
  活跃代码库中不存在模型可见的集群工具。

## 高层概述

```
┌─────────────────────────────────────────────────────────────────┐
│                         用户界面                                │
│  ┌─────────────────┐  ┌─────────────────┐  ┌────────────────┐  │
│  │   TUI (ratatui) │  │  一次性模式     │  │  配置/CLI      │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬───────┘  │
└───────────┼─────────────────────┼────────────────────┼──────────┘
            │                     │                    │
            ▼                     ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                        核心引擎                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                    代理循环（core/engine.rs）            │   │
│  │  ┌─────────┐  ┌─────────────┐  ┌──────────────────────┐ │   │
│  │  │ 会话    │  │ 轮次管理    │  │ 工具编排             │ │   │
│  │  └─────────┘  └─────────────┘  └──────────────────────┘ │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
            │                     │                    │
            ▼                     ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                     工具与扩展层                                 │
│  ┌──────────┐  ┌──────────┐  ┌─────────┐  ┌────────────────┐   │
│  │  工具    │  │  技能    │  │  钩子   │  │  MCP 服务器    │   │
│  │ (shell,  │  │ (插件)   │  │ (前/    │  │  (外部)        │   │
│  │  file)   │  │          │  │  后)    │  │                │   │
│  └──────────┘  └──────────┘  └─────────┘  └────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
            │                     │                    │
            ▼                     ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                  运行时 API + 任务管理                           │
│  ┌─────────────────────────────┐  ┌──────────────────────────┐  │
│  │ HTTP/SSE 运行时 API         │  │ 持久任务管理器           │  │
│  │ (runtime_api.rs)            │  │ (task_manager.rs)        │  │
│  └─────────────────────────────┘  └──────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
            │                     │
            ▼                     ▼
┌─────────────────────────────────────────────────────────────────┐
│                      持久化层                                    │
│  ┌─────────────────┐  ┌──────────────────┐  ┌──────────────┐   │
│  │ 会话/线程       │  │ 任务记录/时间线  │  │ 审计事件     │   │
│  │ (SQLite)        │  │ (SQLite)         │  │ (append-only) │   │
│  └─────────────────┘  └──────────────────┘  └──────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## 核心组件

### 代理引擎（`core/engine.rs`）

核心代理循环管理：
- 会话生命周期（创建、恢复、串联）
- 轮次执行（用户输入 → LLM → 工具调用 → 结果 → LLM → 最终响应）
- 工具编排和批准门控
- 上下文管理和压缩
- 代理驱动的上下文清除（外科手术式消息移除/重写）
- 子代理（`agent` 工具）、RLM 会话（`rlm_open` 等）
- 计划、待办事项/检查清单、任务持久化、自动化调度
- LSP 诊断钩子（编辑后执行）

### 工作区 Crate

- **`crates/config`** — 统一配置加载：TOML 文件 + 环境变量 + `.env`
- **`crates/console`** — 跨平台终端/控制台工具（颜色支持、TTY 检测、ANSI 处理）
- **`crates/execpolicy`** — 工具执行决策的批准/沙箱策略引擎。
- **`crates/hooks`** — 工具事件前后执行的生命周期钩子（stdout、jsonl、webhook）。
- **`crates/mcp`** — 用于模型上下文协议工具服务器的 MCP 客户端 + stdio 服务器。
- **`crates/protocol`** — 请求/响应帧和协议类型。
- **`crates/secrets`** — 用于 API 密钥存储的操作系统密钥环集成。
- **`crates/state`** — SQLite 线程/会话持久化层。

### LLM 集成

- **`client.rs`** — DeepSeek 的文档化 OpenAI 兼容 Chat Completions API 的 HTTP 客户端
- **`llm_client.rs`** — 带重试逻辑的抽象 LLM 客户端 trait
- **`models.rs`** — API 请求/响应的数据结构

#### DeepSeek API 端点

DeepSeek 暴露 OpenAI 兼容的端点。CLI 使用：
- `https://api.deepseek.com/beta/chat/completions` — 默认 v0.8.16 DeepSeek 模型轮次
- `https://api.deepseek.com/beta/models` — 默认 v0.8.16 实时模型发现和健康检查

`https://api.deepseek.com/v1` 被接受用于 OpenAI SDK 兼容性，并且
仍然可以显式配置以选择退出仅限 beta 的功能，例如
严格工具模式、聊天前缀补全和 FIM 补全。公开的
DeepSeek 文档未为此工作流文档化 Responses API 路径；引擎
通过 Chat Completions 驱动轮次。

### 工具系统

- **`tools/`** — 内置工具实现
  - `mod.rs` — 工具注册表和通用类型
  - `shell.rs` — Shell 命令执行
  - `file.rs` — 文件读/写操作
  - `todo.rs` — 检查清单工具以及旧版 todo 别名
  - `tasks.rs` — 模型可见的持久任务、门控、后台 shell 和 PR 尝试工具
  - `github.rs` — 由 `gh` 支持的只读 GitHub 上下文和受保护的评论/关闭工具
  - `automation.rs` — `AutomationManager` 上的模型可见调度工具
  - `plan.rs` — 规划工具
  - `subagent.rs` — 持久子代理会话
  - `spec.rs` — 工具规范
  - `rlm.rs` — 持久递归语言模型（RLM）会话——沙箱化的 Python REPL，带语义助手调用和 `var_handle` 输出支持

### 扩展系统

- **`mcp.rs`** — 用于外部工具服务器的模型上下文协议客户端
- **`skills.rs`** — 插件/技能加载和执行
- **`hooks.rs`** — 带条件的前/后执行钩子

### 用户界面

- **`tui/`** — 终端 UI 组件（基于 ratatui）
  - `app.rs` — 应用程序状态和消息处理
  - `ui.rs` — 事件处理、流式状态和渲染逻辑
  - `approval.rs` — 工具批准对话框
  - `clipboard.rs` — 剪贴板处理
  - `streaming.rs` — 流式文本收集器

- **`ui.rs`** — 旧版/简单 UI 工具

### LSP 集成

- **`lsp/`** — 编辑后诊断注入（#136）
  - `mod.rs` — `LspManager` — 惰性按语言传输池 + 配置
  - `client.rs` — `StdioLspTransport` — 基于 stdio 的 JSON-RPC，带 `didOpen`/`didChange`/`publishDiagnostics`
  - `diagnostics.rs` — 诊断类型、严重程度和 HTML 块渲染器
  - `registry.rs` — 语言检测和默认服务器映射（rust-analyzer、pyright、gopls、clangd、typescript-language-server、jdtls、vue-language-server）
  - 通过 `core/engine/lsp_hooks.rs` 接入引擎——在每次成功编辑后调用

### 安全

- **`sandbox/`** — 平台沙箱策略准备和拒绝报告
  - `mod.rs` — 沙箱类型定义
  - `policy.rs` — 沙箱策略配置
  - `seatbelt.rs` — macOS Seatbelt 配置文件生成
  - `landlock.rs` — Linux Landlock 检测和未来的助手契约
  - `windows.rs` — Windows 助手契约；在 Job Object 进程容器助手存在之前不做广告

### 工具集

- **`utils.rs`** — 通用工具
- **`logging.rs`** — 日志基础设施
- **`compaction.rs`** — 长对话的上下文压缩
- **`purge.rs`** — 代理驱动的上下文清除（外科手术式消息移除/重写）
- **`pricing.rs`** — 成本估算
- **`prompts.rs`** — 系统提示模板
- **`project_doc.rs`** — 项目文档处理
- **`session.rs`** — 会话序列化
- **`runtime_api.rs`** — HTTP/SSE 运行时 API（`codewhale serve --http`）
- **`runtime_threads.rs`** — 持久线程/轮次/条目存储 + 可重放事件时间线
- **`task_manager.rs`** — 持久队列、工作池、任务时间线和工件

## 数据流

### 交互式会话

1. 用户在 TUI 中输入
2. 输入由 `core/engine.rs` 处理
3. 消息通过 `llm_client.rs` 发送到 LLM
4. 响应流式返回，在 `client.rs` 中解析
5. 工具调用被提取并通过 `tools/` 执行
6. 钩子在工具执行前后触发
7. 结果聚合并发送回 LLM
8. 最终响应在 TUI 中渲染

### 崩溃恢复 + 离线队列

1. 在发送用户输入之前，TUI 将检查点快照写入 `~/.codewhale/sessions/checkpoints/latest.json`
2. 启动默认是全新的；之前的会话通过 `--resume`/`--continue`（或 TUI 中的 `Ctrl+R`）显式恢复
3. 在降级/离线时，新提示在内存中排队并镜像到 `~/.codewhale/sessions/checkpoints/offline_queue.json`
4. 队列编辑（`/queue ...`）持续持久化，以便草稿和排队的提示在重启后仍然存在
5. 成功的轮次完成会清除活跃检查点并写入持久会话快照
6. Agent/Yolo 轮次还会在 `~/.codewhale/snapshots/<project_hash>/<worktree_hash>/.git` 下进行轮次前后 side-git 工作区快照；`/restore N` 和 `revert_turn` 恢复文件状态而不更改对话历史或用户的 `.git`

### 工具执行

1. LLM 通过 `tool_use` 内容块请求工具
2. 工具注册表查找处理程序
3. 执行前钩子运行
4. 如果需要则请求批准（非 yolo 模式）
5. 工具执行（在 macOS 上可能沙箱化）
6. 执行后钩子运行
7. 结果元数据保留在运行时条目记录上
8. **LSP 编辑后钩子**：如果工具是 `edit_file`/`apply_patch`/`write_file` 且 LSP 已启用，引擎运行 `run_post_edit_lsp_hook()` 收集诊断
9. **诊断刷新**：在下一次 API 请求之前，`flush_pending_lsp_diagnostics()` 将收集到的任何错误作为合成用户消息注入
10. 结果返回给代理循环

### 后台任务

1. 客户端排队任务（`/task add ...` 或 `POST /v1/tasks`）
2. `task_manager.rs` 在 `~/.codewhale/tasks` 下持久化任务 + 队列条目
3. 工作线程拾取排队任务（有界池），转换为 `running`
4. 任务创建/使用运行时线程并启动运行时轮次
5. `runtime_threads.rs` 持久化线程/轮次/条目记录 + 单调事件序列
6. 时间线/工具摘要/工件引用逐步持久化
7. 检查清单状态、验证器门控、PR 尝试和受保护的 GitHub 事件从工具元数据应用到活跃任务
8. 最终状态（`completed|failed|canceled`）是持久的，可通过 TUI/API 查询

模型可见的持久任务工具是同一管理器之上的界面。它们
不引入并行工作系统：`task_create` 排队普通任务，
`checklist_*` 更新任务本地进度，`task_gate_run` 和已完成的
`task_shell_wait` 附加验证证据，自动化运行排队
普通的持久任务。

### 运行时线程/轮次时间线

1. API/TUI 创建或恢复线程（`/v1/threads*`）
2. 在线程上启动轮次（`/v1/threads/{id}/turns`）
3. 引擎事件映射到条目生命周期事件（`item.started|item.delta|item.completed`）
4. 中断/引导操作仅适用于活跃轮次
5. 压缩（自动/手动）作为 `context_compaction` 条目生命周期发出
6. 清除（代理驱动）作为 `context_purge` 条目生命周期发出
7. 客户端通过 `/v1/threads/{id}/events?since_seq=<n>` 重放历史并恢复

### 持久模式门控

- `session_manager.rs`、`runtime_threads.rs` 和 `task_manager.rs` 在持久记录上嵌入 `schema_version`。
- 加载时，较新的模式版本会被显式错误拒绝，而不是静默截断/覆盖数据。
- 这允许安全的前向迁移，并在二进制文件和存储状态不同步时防止损坏。

## 扩展点

### 添加新工具

1. 在 `tools/` 中创建处理程序
2. 在 `tools/registry.rs` 中注册
3. 添加工具规范（名称、描述、输入模式）

### 添加 MCP 服务器

1. 在 `~/.codewhale/mcp.json` 中配置
2. 服务器在启动时自动发现
3. 工具自动暴露给 LLM

### 创建技能

1. 创建带有 `SKILL.md` 的技能目录
2. 定义技能提示和可选脚本
3. 放置在 `~/.codewhale/skills/` 中

### 添加钩子

在 `~/.codewhale/config.toml` 中配置：

```toml
[[hooks]]
event = "tool_call_before"
command = "echo 'Running tool: $TOOL_NAME'"
```

## 关键设计决策

1. **流式优先**：所有 LLM 响应流式传输以实现响应性
2. **工具安全**：非 YOLO 模式需要对破坏性操作（包括有副作用的 MCP 工具）进行批准
3. **可扩展性**：MCP、技能和钩子允许在不更改代码的情况下进行自定义
4. **跨平台**：核心在 Linux/macOS/Windows 上运行。沙箱保证
   是平台特定的：macOS Seatbelt 是活跃的策略路径；Linux 和
   Windows 在被视为完整操作系统沙箱之前需要助手强制执行。
5. **最小依赖**：精心选择依赖以加快构建速度
6. **本地优先运行时 API**：HTTP/SSE 端点用于受信任的本地主机访问，目前由 `crates/tui` 运行时提供服务

## 配置文件

- `~/.codewhale/config.toml` — 主配置（`~/.deepseek/config.toml` 仍作为旧版回退读取）
- `/etc/deepseek/managed_config.toml` — 可选托管默认层（Unix）
- `/etc/deepseek/requirements.toml` — 可选允许的策略约束（Unix）
- `~/.codewhale/mcp.json` — MCP 服务器配置
- `~/.codewhale/skills/` — 用户技能目录
- `~/.codewhale/sessions/` — 会话历史
- `~/.codewhale/sessions/checkpoints/` — 崩溃检查点 + 离线队列持久化
- `~/.codewhale/snapshots/` — 用于 `/restore` 和 `revert_turn` 的轮次前后 side-git 工作区快照
- `~/.codewhale/tasks/` — 后台任务记录、队列、时间线、工件
- `~/.codewhale/audit.log` — 凭据 + 批准/提权操作的仅追加审计事件
