# 运行时 API 与集成契约

`codewhale app-server` 是规范的本地运行时 API 和控制平面。
本地 SDK、移动/远程控制客户端和编辑器集成与其通信，而不是屏幕抓取终端输出。它提供完整的 HTTP/SSE 运行时 API（`/v1/*`）、基于 stdio 的 JSON-RPC 控制传输以及适合手机的移动页面。`codewhale doctor --json` 提供机器可读的健康检查，`codewhale serve --acp` 通过 stdio 为 Zed 等编辑器提供 Agent Client Protocol。

`codewhale serve --http` / `serve --mobile` 作为 `codewhale app-server --http` / `--mobile` 的**兼容性别名**保留；两者启动相同的服务器。新的集成应针对 `app-server`。

`codewhale exec` 是独立的一次性无头工作路径（stream-json、fleet worker 子进程、CI 原语）。它不属于此 API，但它共享相同的运行时、提供者/模型解析、权限配置文件和事件词汇。

本文档是嵌入 DeepSeek 引擎的原生工作台应用程序（及其他本地监督器）的稳定集成契约。

## 架构

```
本地监督器 / SDK / 自动化测试套件
        │
        ├─ codewhale app-server --http     → HTTP/SSE 运行时 API (/v1/*)        [规范]
        ├─ codewhale app-server --mobile   → 运行时 API + 移动控制页面
        ├─ codewhale app-server --stdio    → 基于 stdio 的 JSON-RPC 控制传输
        ├─ codewhale doctor --json         → 机器可读的健康检查与能力
        ├─ codewhale serve --acp           → 面向 Zed 等编辑器的 ACP stdio agent
        ├─ codewhale serve --mcp           → MCP stdio 服务器
        ├─ codewhale serve --http/--mobile → `app-server --http/--mobile` 的旧版别名
        └─ codewhale exec [args]           → 一次性无头 worker（stream-json）
```

引擎作为仅本地进程运行。默认情况下所有 API 绑定到 `localhost`。没有托管中继，没有提供者令牌托管，没有密钥泄露。

关于已完成的轮次的只读审计导出的提议，请参见 [`docs/RECEIPTS.md`](RECEIPTS.md)。该文档是协议说明；收据 CLI/API 表面尚未实现。

## 运行时 API 入口点

| 入口 | 传输 | 用途 |
|---|---|---|
| `codewhale app-server --http` | HTTP/SSE 在 `127.0.0.1:7878` | 完整的 `/v1/*` 运行时 API（规范） |
| `codewhale app-server --mobile` | HTTP/SSE 在 `0.0.0.0:7878` + `/mobile` | 运行时 API + 手机控制页面 |
| `codewhale app-server --stdio` | 基于 stdio 的 JSON-RPC 2.0 | 本地 SDK / 控制探针（无监听器） |
| `codewhale app-server` | HTTP 在 `127.0.0.1:8787` | 旧版进程内 app-server（`/healthz`、`/thread`、`/app`、`/prompt`、`/tool`、`/jobs`） |
| `codewhale serve --http` / `--mobile` | 与 `app-server --http`/`--mobile` 相同的服务器 | 兼容性别名 |

`app-server --http` 和 `--mobile` 启动与历史上通过 `serve --http` 访问的相同的成熟运行时 API 服务器——没有路由或行为变化，因此下面记录的每个端点在两个入口点之间完全相同。运行时 API 令牌从 `--auth-token` 读取，然后从 `CODEWHALE_RUNTIME_TOKEN`，然后从 `DEEPSEEK_RUNTIME_TOKEN`；仅在回环绑定时使用 `--insecure-no-auth`。`serve` 兼容性别名保留其 `--insecure` 标志。旧版进程内 `codewhale app-server` 也需要显式的 `--auth-token` 或 `DEEPSEEK_RUNTIME_TOKEN`；它没有无认证模式。

### 旧版 app-server 路由

当不带 `--http`、`--mobile` 或 `--stdio` 调用 `codewhale app-server` 时，它启动旧版进程内服务器，暴露以下路由：

| 方法 | 路由 | 描述 |
|---|---|---|
| GET | `/healthz` | 服务器就绪探针 |
| GET | `/thread` | 列出已知线程 |
| GET | `/app` | App 元数据 |
| POST | `/prompt` | 入队提示词并返回 stream-id |
| POST | `/tool` | 解决工具批准/交互 |
| GET | `/jobs` | 列出已知作业 |
| POST | `/jobs/:job_id/pause` | 暂停作业 |
| POST | `/jobs/:job_id/resume` | 恢复作业 |
| POST | `/jobs/:job_id/cancel` | 取消作业 |

### MCP stdio 服务器

```bash
codewhale serve --mcp [--mcp-strict]
```

将 DeepSeek 的 CodeWhale 工具目录作为 MCP 服务器暴露在 stdio 上。默认仅暴露工具；通过 `--mcp-with-prompts` 和 `--mcp-with-resources` 分别添加提示词和资源能力。

每个公开的工具都带有来自 CodeWhale 目录的描述和 JSON Schema，因此任何 MCP 客户端都接收相同的结构化接口。`--mcp-strict` 还会将工具结果作为 `MCPError` 而不是 MCP 元数据返回底层 CodeWhale 工具错误，因此客户端可以在不解析元数据的情况下分支。

### ACP stdio agent（Zed 集成）

```bash
codewhale serve --acp
```

通过 Agent Client Protocol 将 CodeWhale 作为 Zed 编辑器 agent 暴露在 stdio 上。当 Zed 的 agent 调用 `create_message` 时，ACP 适配器将用户提示和当前文件上下文转发到配置的 DeepSeek API，构建工具定义，并每流式块发出 `session/update` 事件。`session/prompt` 响应包括当前客户端和当前默认模型。响应以 `session/update` agent 消息块的形式发出，后跟带有 `stopReason: "end_turn"` 的 `session/prompt` 响应。

适配器故意保守：尚未通过 ACP 暴露 shell 工具、文件写入工具、检查点重放或会话加载。使用 `codewhale serve --http` 获取完整的本地运行时 API，使用 `codewhale serve --mcp` 当另一个客户端需要将 DeepSeek 的工具作为 MCP 工具时。

## 能力端点：`codewhale doctor --json`

返回描述当前安装就绪状态的 JSON 对象。适用于 macOS 工作台的健康检查轮询。

```bash
codewhale doctor --json
```

### 响应模式（关键字段）

| 字段 | 类型 | 描述 |
|---|---|---|
| `version` | string | 已安装版本（例如 `"0.8.9"`） |
| `config_path` | string | 解析出的配置文件路径 |
| `config_present` | bool | 配置文件是否存在 |
| `workspace` | string | 默认工作区目录 |
| `legacy_state.primary_root` | string | 为已知状态路径检查的主要 CodeWhale 状态根 |
| `legacy_state.legacy_root` | string | 为已知状态路径检查的旧版 `.deepseek` 状态根 |
| `legacy_state.needs_attention` | bool | 已知 `~/.deepseek` 状态路径是否未迁移或同时存在于 `~/.codewhale` 旁边 |
| `legacy_state.legacy_only_count` | number | 仅存在于旧版根下的已知状态路径计数 |
| `legacy_state.dual_present_count` | number | 同时存在于主要和旧版根下的已知状态路径计数 |
| `legacy_state.entries` | array | 每个路径的迁移状态：`{name, primary_present, legacy_present, status}` |
| `api_key.source` | string | `env`、`config` 或 `missing` |
| `base_url` | string | API 基础 URL |
| `default_text_model` | string | 默认模型 |
| `memory.enabled` | bool | 记忆功能是否开启 |
| `memory.path` | string | 记忆文件路径 |
| `memory.file_present` | bool | 记忆文件是否存在 |
| `mcp.config_path` | string | MCP 配置文件路径 |
| `mcp.present` | bool | MCP 配置是否存在 |
| `mcp.servers` | array | 每个服务器的健康状况：`{name, enabled, status, detail}` |
| `skills.selected` | string | 解析出的 skills 目录 |
| `skills.global.path` / `.present` / `.count` | — | CodeWhale 全局 skills 目录（`~/.codewhale/skills`，支持旧版 `~/.deepseek/skills`） |
| `skills.agents.path` / `.present` / `.count` | — | 工作区 `.agents/skills/` 目录 |
| `skills.agents_global.path` / `.present` / `.count` | — | agentskills.io 全局 skills 目录（`~/.agents/skills`） |
| `skills.local.path` / `.present` / `.count` | — | `skills/` 目录 |
| `skills.opencode.path` / `.present` / `.count` | — | `.opencode/skills/` 目录 |
| `skills.claude.path` / `.present` / `.count` | — | `.claude/skills/` 目录 |
| `tools.path` / `.present` / `.count` | — | 全局工具目录 |
| `plugins.path` / `.present` / `.count` | — | 全局插件目录 |
| `sandbox.available` | bool | 此操作系统是否支持沙箱 |
| `sandbox.kind` | string or null | 沙箱类型（例如 `"macos_seatbelt"`） |
| `storage.spillover.path` / `.present` / `.count` | — | 工具输出溢出目录 |
| `storage.stash.path` / `.present` / `.count` | — | Composer 暂存 |

### 示例

```json
{
  "version": "0.8.9",
  "config_path": "/Users/you/.codewhale/config.toml",
  "config_present": true,
  "workspace": "/Users/you/projects/codewhale-tui",
  "api_key": {
    "source": "env"
  },
  "base_url": "https://api.deepseek.com/beta",
  "default_text_model": "deepseek-v4-pro",
  "memory": {
    "enabled": false,
    "path": "/Users/you/.codewhale/memory.md",
    "file_present": true
  },
  "mcp": {
    "config_path": "/Users/you/.codewhale/mcp.json",
    "present": true,
    "servers": [
      {"name": "filesystem", "enabled": true, "status": "ok", "detail": "ready"}
    ]
  },
  "sandbox": {
    "available": true,
    "kind": "macos_seatbelt"
  }
}
```

## HTTP/SSE 运行时 API：`codewhale app-server --http`

```bash
codewhale app-server --http [--host 127.0.0.1] [--port 7878] [--workers 2] [--auth-token TOKEN] [--insecure-no-auth]
codewhale app-server --mobile [--host 0.0.0.0] [--port 7878] [--auth-token TOKEN]
codewhale app-server --mobile --host 127.0.0.1 [--port 7878] [--insecure-no-auth]

# 兼容性别名 — 相同的服务器，serve 标志名称：
codewhale serve --http   [...] [--insecure]
codewhale serve --mobile [...] [--insecure]
```

默认值：主机 `127.0.0.1`，端口 `7878`，2 个工作线程（限制在 1–8）。

服务器默认绑定到 `localhost`。配置通过 CLI 标志——没有 `[app_server]` 配置节。

`/v1/*` 路由需要 bearer 令牌，除非 `codewhale app-server` 在回环绑定（如 `127.0.0.1`）上以 `--insecure-no-auth` 启动。不要将无认证模式与 `--mobile` 默认主机 `0.0.0.0` 结合使用；对 LAN 移动访问使用令牌，或添加 `--host 127.0.0.1` 进行仅本地无认证测试。`codewhale serve` 兼容性别名使用 `--insecure` 作为相同的回环逃生舱。
在启动服务器之前传递 `--auth-token TOKEN` 或设置 `DEEPSEEK_RUNTIME_TOKEN=TOKEN`。如果两者都未设置，进程会在启动时生成一次性令牌并打印出来。`/health` 和 `/v1/runtime/info` 对本地监督和引导保持公开。当移动模式禁用时，`/mobile` 返回 404；当移动模式启用且认证开启时，除非请求提供运行时令牌，否则 `/mobile` 返回 401。

认证客户端可以将令牌提供为 `Authorization: Bearer TOKEN`、`X-DeepSeek-Runtime-Token: TOKEN` 或 `?token=TOKEN`（用于无法设置自定义头的 EventSource 风格客户端）。

### 移动控制页面

`codewhale serve --mobile` 启动相同的 HTTP/SSE 运行时 API，并在 `/mobile` 上提供适合手机的控制页面。当绑定主机保持默认值时，移动模式绑定到 `0.0.0.0`，打印警告，并打印本地/LAN URL。传递 `--host 127.0.0.1` 以保持移动页面仅回环。如果生成或提供了运行时令牌，打印的移动 URL 包含它作为查询参数；页面在本地存储它并从地址栏中移除它。静态 HTML 页面不包含密钥，但当认证启用时仍受令牌门控，因此未认证的 LAN 客户端无法指纹识别移动表面。

移动页面可以列出/创建线程、发送提示、关注实时 SSE 事件、操控或中断活跃轮次，并通过 `POST /v1/approvals/{approval_id}` 解决正常的工具批准。它仍然是本地/LAN 便利表面：不要在没有 TLS 和可信前置层的情况下直接暴露到公共互联网。

### 端点

**健康检查**
- `GET /health`

**会话**（旧版会话管理器）
- `GET /v1/sessions?limit=50&search=<substring>`
- `GET /v1/sessions/{id}`
- `DELETE /v1/sessions/{id}`
- `POST /v1/sessions/{id}/resume-thread`

**线程**（持久运行时数据模型）
- `GET /v1/threads?limit=50&include_archived=false&archived_only=false`
- `GET /v1/threads/summary?limit=50&search=<optional>&include_archived=false&archived_only=false`
- `POST /v1/threads`
- `GET /v1/threads/{id}`
- `PATCH /v1/threads/{id}`（见下面的请求体格式）
- `POST /v1/threads/{id}/resume`
- `POST /v1/threads/{id}/fork`

`GET /v1/threads/summary` 是 VS Code Agent View 使用的只读摘要表面。每个项目包括 `id`、`title`、`preview`、`model`、`mode`、`archived`、`updated_at`、`latest_turn_id`、`latest_turn_status`，以及工作区元数据：

```json
{
  "id": "thread_...",
  "title": "Implement MCP status count",
  "preview": "The TUI footer should count project MCP servers...",
  "model": "deepseek-v4-pro",
  "mode": "agent",
  "branch": "feature/runtime-api",
  "head": "abc1234",
  "dirty": false,
  "workspace": "/Users/you/projects/codewhale",
  "archived": false,
  "updated_at": "2026-06-06T05:43:00Z",
  "latest_turn_id": "turn_...",
  "latest_turn_status": "completed"
}
```

`branch` 在请求时从线程工作区解析，当工作区不是 Git 仓库或无法读取分支时可能为 `null`。`head` 是该工作区可用的当前短 Git 提交。`dirty` 为 true 时表示工作区有暂存、未暂存或未跟踪的更改。包含 `workspace` 以便编辑器客户端可以显示 agent 通道何时在当前 VS Code 文件夹之外工作。

线程派生（fork）是兄弟运行时线程，而不是原地树投影。`thread.forked` 事件包含 `source_thread_id`；内部回溯感知派生也可能包含 `backtrack_depth_from_tail` 和 `dropped_turn_id`。线程列表和摘要响应在 v0.8.40 中保持扁平，因此需要图的客户端应从事件中重建它，而不是假设列表顺序是完整的树。

`archived_only=true` 仅返回已归档线程（与 `include_archived` 互斥覆盖）。默认行为不变：`include_archived=false` 和 `archived_only=false` 返回活跃线程。在 v0.8.10（#563）中添加。

`PATCH /v1/threads/{id}` 请求体 — 每个字段都是可选的，缺失意味着"不变"。至少需要一个字段。`title` 和 `system_prompt` 接受空字符串以清除先前设置的值。在 v0.8.10（#562）中添加：

```json
{
  "archived": true,
  "allow_shell": false,
  "trust_mode": false,
  "auto_approve": false,
  "model": "deepseek-v4-pro",
  "mode": "agent",
  "title": "User-set thread title",
  "system_prompt": "You are a useful assistant."
}
```

**轮次**（在线程内）
- `POST /v1/threads/{id}/turns`
- `POST /v1/threads/{id}/turns/{turn_id}/steer`
- `POST /v1/threads/{id}/turns/{turn_id}/interrupt`
- `POST /v1/threads/{id}/compact`（手动压缩）

**批准**
- `POST /v1/approvals/{approval_id}`，请求体为 `{ "decision": "allow" | "deny", "remember": false }`

**事件**（SSE 重放 + 实时流）
- `GET /v1/threads/{id}/events?since_seq=<u64>`

**快照**（只读 side-git 恢复点列表）
- `GET /v1/snapshots?limit=20`

`/v1/snapshots` 列出运行时工作区的最近 side-git 恢复点。它是只读的，不恢复文件。`limit` 默认为 `20`，必须在 `1` 到 `100` 之间。

```json
[
  {
    "id": "snap_...",
    "label": "post-turn:1",
    "timestamp": 1780730580
  }
]
```

运行时 API 的恢复/重试/撤销/编辑器应用变更端点是有意推迟的。GUI 客户端应将线程摘要和快照视为检查表面，直到原子文件系统 + 对话状态变更语义被指定和测试。

**收据**（未来只读审计导出）
- 仅提议：`GET /v1/threads/{thread_id}/turns/{turn_id}/receipt`

**兼容性流**（一次性，向后兼容）
- `POST /v1/stream`

**任务**（持久后台工作）
- `GET /v1/tasks`
- `POST /v1/tasks`
- `GET /v1/tasks/{id}`
- `POST /v1/tasks/{id}/cancel`

**自动化**（定期重复工作）
- `GET /v1/automations`
- `POST /v1/automations`
- `GET /v1/automations/{id}`
- `PATCH /v1/automations/{id}`
- `DELETE /v1/automations/{id}`
- `POST /v1/automations/{id}/run`
- `POST /v1/automations/{id}/pause`
- `POST /v1/automations/{id}/resume`
- `GET /v1/automations/{id}/runs?limit=20`

**内省**
- `GET /v1/workspace/status`
- `GET /v1/skills`
- `GET /v1/apps/mcp/servers`
- `GET /v1/apps/mcp/tools?server=<optional>`

**用量**（跨线程的令牌/成本聚合）
- `GET /v1/usage?since=<rfc3339>&until=<rfc3339>&group_by=<day|model|provider|thread>`

`since` / `until` 是包含的 RFC 3339 时间戳，可以省略（无边界）。`group_by` 默认为 `day`。桶按键升序排序。空时间范围产生空的 `buckets`（永不 404）。成本通过模型→定价映射计算；模型没有定价条目的轮次贡献令牌但成本为 `0.0`。在 v0.8.10（#564）中添加。

```json
{
  "since": "2026-04-01T00:00:00Z",
  "until": "2026-04-30T23:59:59Z",
  "group_by": "day",
  "totals": {
    "input_tokens": 12345,
    "output_tokens": 6789,
    "cached_tokens": 0,
    "reasoning_tokens": 0,
    "cost_usd": 0.012,
    "turns": 42
  },
  "buckets": [
    {
      "key": "2026-04-30",
      "input_tokens": 1234,
      "output_tokens": 678,
      "cached_tokens": 0,
      "reasoning_tokens": 0,
      "cost_usd": 0.001,
      "turns": 3
    }
  ]
}
```

## 运行时数据模型

运行时使用持久化的 Thread/Turn/Item 生命周期。

- **ThreadRecord** — `id`、`created_at`、`updated_at`、`model`、`workspace`、`mode`、`task_id`、`system_prompt`、`latest_turn_id`、`latest_response_bookmark`、`archived`
- **TurnRecord** — `id`、`thread_id`、`status`（`queued|in_progress|completed|failed|interrupted|canceled`）、时间戳、持续时间、用量、错误摘要
- **TurnItemRecord** — `id`、`turn_id`、`kind`（`user_message|agent_message|tool_call|file_change|command_execution|context_compaction|status|error`）、生命周期 `status`、`metadata`

事件是仅追加的，具有全局单调的 `seq` 用于重放/恢复。

### 重启语义

- 如果在轮次或项目处于 `queued` 或 `in_progress` 状态时进程重启，恢复的记录将被标记为 `interrupted`，并带有 `"Interrupted by process restart"` 错误。
- 任务执行在同一持久化的线程/轮次存储之上执行自己的恢复。

### 批准模型

- `auto_approve` 标志适用于运行时批准桥和引擎工具上下文。当为线程/轮次/任务启用时，需要批准的工具在非交互式运行时路径中自动批准，shell 安全检查在自动批准模式下运行，生成的子 agent 继承该设置。
- 当省略时，`auto_approve` 默认为 `false`。

### SSE 事件流

`/v1/threads/{id}/events` 的 SSE 事件负载格式：

```json
{
  "schema_version": 1,
  "seq": 42,
  "event": "item.delta",
  "kind": "item.delta",
  "thread_id": "thr_1234abcd",
  "turn_id": "turn_5678efgh",
  "item_id": "item_90ab12cd",
  "timestamp": "2026-02-11T20:18:49.123Z",
  "created_at": "2026-02-11T20:18:49.123Z",
  "payload": {
    "delta": "partial output",
    "kind": "agent_message"
  }
}
```

兼容性说明：

- `schema_version` 是 HTTP/SSE 信封模式版本。它独立于用于持久化线程/轮次/事件记录的运行时存储模式。
- `event` 在现有客户端中仍然是 SSE 事件名称；它保持原样。
- `kind` 在稳定信封中镜像 `event`，供类型化客户端使用。
- `thread.started`、`turn.started` 和 `turn.completed` 像以前一样作为 SSE 事件名称发出。
- `timestamp` 在模式版本 1 中仍然是规范事件时间。`created_at` 是在其他地方使用 `created_at` 命名的客户端的等效别名；不要求两个字段都存在。

常见事件名称：`thread.started`、`thread.forked`、`turn.started`、`turn.lifecycle`、`turn.steered`、`turn.interrupt_requested`、`turn.completed`、`item.started`、`item.delta`、`item.completed`、`item.failed`、`item.interrupted`、`approval.required`、`approval.decided`、`approval.timeout`、`sandbox.denied`。

当执行策略规则导致提示时，`approval.required` 事件可能包含 `matched_rule` 字符串。此字段是客户端的解释性元数据，不授予或持久化权限。

## 安全边界

- **默认 localhost**。服务器默认绑定到 `127.0.0.1`。`--mobile` 在未提供主机时绑定到 `0.0.0.0`，以便同一 LAN 上的手机可以访问，CLI 会为该重新绑定打印警告。传递 `--host 127.0.0.1` 仅限回环移动页面。仅在信任网络路径或有反向代理/VPN 进行认证时设置非回环主机。运行时不提供用户隔离或 TLS。
- **可选令牌守卫**。`--auth-token` 或 `DEEPSEEK_RUNTIME_TOKEN` 要求 `/v1/*` 路由匹配 bearer 令牌。这是本地便利守卫，不是公共网络上 TLS、VPN 或可信反向代理的替代品。
- **不托管提供者令牌**。服务器永不返回 API 密钥。`api_key.source` 能力字段报告 `env`、`config` 或 `missing`——绝不返回密钥本身。
- **无托管中继**。app-server 是用户控制下的本地进程。没有云组件。
- **能力响应**永不泄露密钥、文件内容或会话消息体。它们报告*元数据*：存在性、计数、状态标志。

### CORS 允许列表

运行时 API 附带内置的开发源允许列表：`http://localhost:3000`、`http://127.0.0.1:3000`、`http://localhost:1420`、`http://127.0.0.1:1420`、`tauri://localhost`。要添加其他源（例如在 Vite 默认的 `:5173` 上开发 UI 时），使用以下任一方式：

- CLI 标志（可重复）：`codewhale serve --http --cors-origin http://localhost:5173`
- 环境变量（逗号分隔）：`DEEPSEEK_CORS_ORIGINS="http://localhost:5173,http://localhost:8080"`
- 配置（`~/.codewhale/config.toml`）：
  ```toml
  [runtime_api]
  cors_origins = ["http://localhost:5173"]
  ```

用户提供的源**叠加在**内置默认值之上；它们不会替换内置默认值。不支持通配符源——保留显式允许列表模型。在 v0.8.10（#561）中添加。

## 运行时 SDK Fleet 辅助函数

v0.8.60 运行时 SDK 夹具位于 `npm/runtime-sdk` 中，并作为 `@codewhale/runtime-sdk` 工作区包暴露。它故意保持薄层：每个辅助函数调用本地 Rust 运行时 API，因此不能绕过 CodeWhale 的沙箱、批准提示、提供者配置或 fleet 账本权限。

```js
import { createRuntimeClient } from "@codewhale/runtime-sdk";

const client = createRuntimeClient({
  baseUrl: "http://127.0.0.1:7878",
  token: process.env.CODEWHALE_RUNTIME_TOKEN,
});

const { runs } = await client.listFleetRuns();
const workers = await client.listFleetWorkers(runs[0].id);
await client.restartWorker(workers.workers[0].worker_id);
```

Fleet 辅助函数覆盖 v0.8.60 HTTP 表面：

| 辅助函数 | 运行时 API 路由 |
|---|---|
| `listFleetRuns()` | `GET /v1/fleet/runs` |
| `getFleetRun(runId)` | `GET /v1/fleet/runs/{run_id}` |
| `listFleetWorkers(runId)` | `GET /v1/fleet/runs/{run_id}/workers` |
| `getFleetWorker(workerId)` | `GET /v1/fleet/workers/{worker_id}` |
| `interruptWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/interrupt` |
| `restartWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/restart` |
| `stopFleetRun(runId)` | `POST /v1/fleet/runs/{run_id}/stop` |

`createFleetRun(spec)` 和 `fleetEvents(runId)` 在当前 Rust 路由之前进行了类型化，以便编辑器/web 客户端可以针对预期的 SDK 契约进行编码。在运行时 API 暴露 `POST /v1/fleet/runs` 和 fleet 事件流之前，SDK 会引发 `RuntimeCapabilityError`，带有稳定的能力字符串（`fleet_run_create`、`fleet_event_stream`），而不是将这些缺口暴露为通用 fetch 失败。

验证：

```bash
npm test --workspace @codewhale/runtime-sdk
```

## Agent 运行收据

子 agent 通道在 `.codewhale/state/subagents.v1.json` 中持久化紧凑的运行收据。运行时 API 将这些收据暴露为只读检查表面：

| 操作 | 端点 |
|---|---|
| 列出持久化的 agent 运行 | `GET /v1/agent-runs` |
| 检查一次运行 | `GET /v1/agent-runs/{run_id}` |

响应与 `agent` 收据暴露的相同 worker 记录格式：`spec.run_id`、`actor_kind`、生命周期 `status`、有界的 `events`、`follow_up`、`takeover`、`artifacts`、`usage` 和 `verification`。`run_id` 对旧记录回退到 worker id，且 `{run_id}` 可以是运行 id 或 worker id。

这些端点不启动、取消或操控子 agent。API 表面存在是为了 app/编辑器/无头客户端可以检查 TUI 和父模型看到的相同交接收据。

## 会话生命周期（原生 UI 监督）

| 操作 | 端点 |
|---|---|
| 列出会话 | `GET /v1/sessions` |
| 获取会话 | `GET /v1/sessions/{id}` |
| 删除会话 | `DELETE /v1/sessions/{id}` |
| 恢复到线程 | `POST /v1/sessions/{id}/resume-thread` |
| 创建线程 | `POST /v1/threads` |
| 列出线程 | `GET /v1/threads` |
| 附加到事件 | `GET /v1/threads/{id}/events?since_seq=0` |
| 发送消息 | `POST /v1/threads/{id}/turns` |
| 操控 | `POST /v1/threads/{id}/turns/{turn_id}/steer` |
| 中断 | `POST /v1/threads/{id}/turns/{turn_id}/interrupt` |
| 压缩 | `POST /v1/threads/{id}/compact` |

## 兼容性测试

契约快照位于 `crates/protocol/tests/`。运行：

```bash
cargo test -p codewhale-protocol --test parity_protocol --locked
```

这验证 app-server 的事件模式没有偏离文档化的契约。CI 在每次推送到 `main` 和发布标签时运行此测试。

app-server stdio 控制表面有自己的漂移守卫——广告的 `capabilities` 方法集固定在 `crates/app-server/src/lib.rs` 中：

```bash
cargo test -p codewhale-app-server capabilities
```

在发布之前，运行无头冒烟测试（stdio 探针 + 可选的提供者矩阵，无密钥泄露）：

```bash
scripts/release/app-server-smoke.sh --matrix        # 试运行计划
bash scripts/release/app-server-smoke.test.sh       # 解析器自测试（假二进制文件）
```
