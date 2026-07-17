# RFC：持久化 SQLite 迁移

**Issue：** #2189
**状态：** 草案
**日期：** 2026-05-27

## 1. 当前持久化后端

### 1.1 `crates/state` — 部分 SQLite（rusqlite）

**后端**：通过 `rusqlite` 的 SQLite（非 sqlx）。
**路径**：`~/.deepseek/state.db`
**表**：`threads`、`thread_dynamic_tools`、`messages`、`checkpoints`、`jobs`
**此外**：`session_index.jsonl` — 追加式 JSONL，用于线程名称查找。
**模式版本控制**：无 — 表形态由二进制文件隐式版本化。

### 1.2 `crates/tui/src/session_manager.rs` — JSON 会话

**后端**：单独的 JSON 文件 + 通过 `write_atomic` 的原子写入。
**路径**：
- `~/.codewhale/sessions/{id}.json`（首选，v0.8.44+）或 `~/.deepseek/sessions/{id}.json`（回退）
- `~/.deepseek/sessions/checkpoints/latest.json` — 崩溃恢复检查点
- `~/.deepseek/sessions/checkpoints/offline_queue.json` — 离线/降级模式队列

**模式常量**：
- `CURRENT_SESSION_SCHEMA_VERSION: u32 = 1`（`SavedSession`）
- `CURRENT_QUEUE_SCHEMA_VERSION: u32 = 1`（`OfflineQueueState`）

**策略**：拒绝更新版本 — 旧二进制文件将拒绝加载由更新版本写入的数据。

### 1.3 `crates/tui/src/runtime_threads.rs` — JSON 运行时存储

**后端**：每条记录的 JSON 文件 + 追加式 JSONL 用于事件。
**路径**（在 `~/.deepseek/tasks/runtime/` 或 `DEEPSEEK_RUNTIME_DIR` 下）：
- `threads/{id}.json`
- `turns/{id}.json`
- `items/{id}.json`
- `events/{thread_id}.jsonl` — 追加式 JSONL 事件时间线
- `state.json` — 全局单调序列计数器

**模式常量**：
- `CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 2`

**策略**：拒绝更新版本。

### 1.4 `crates/tui/src/task_manager.rs` — JSON 任务存储

**后端**：每条记录的 JSON 文件 + 原子写入。
**路径**（在 `~/.deepseek/tasks/` 或 `DEEPSEEK_TASKS_DIR` 下）：
- `{id}.json` — 每条任务记录
- `queue.json` — 队列状态

**模式常量**：
- `CURRENT_TASK_SCHEMA_VERSION: u32 = 2`

**策略**：拒绝更新版本。

### 1.5 `crates/tui/src/automation_manager.rs` — JSON 自动化存储

**后端**：每条记录的 JSON 文件。
**路径**（在 `~/.deepseek/automations/` 或 `DEEPSEEK_AUTOMATIONS_DIR` 下）：
- `{id}.json`

**模式常量**：
- `CURRENT_AUTOMATION_SCHEMA_VERSION: u32 = 1`

### 1.6 `crates/tui/src/audit.rs` — JSONL 审计日志

**后端**：追加式 JSONL，每个事件后 fsync。
**路径**：`~/.deepseek/audit.log`
**模式**：无版本字段 — 每行是一个 `{"ts", "event", "details"}` blob。

### 1.7 问题摘要

| 区域 | 后端 | 模式版本 | 写入策略 | 可查询性 |
|------|---------|---------------|----------------|-------------|
| state（线程/消息/任务） | SQLite | 隐式 | 直接 SQL | SQL |
| 会话 | JSON 文件 | v1 | 原子重命名 | 文件扫描 |
| 运行时线程/轮次/项 | JSON 文件 | v2 | 原子重命名 | 文件扫描 |
| 运行时事件 | JSONL | v2 | 追加+fsync | 线性扫描 |
| 任务 | JSON 文件 | v2 | 原子重命名 | 文件扫描 |
| 自动化 | JSON 文件 | v1 | 原子重命名 | 文件扫描 |
| 审计 | JSONL | 无 | 追加+fsync | 线性扫描 |

**关键痛点**：
1. **列出**线程/会话/任务需要扫描目录并反序列化每个文件。
2. **过滤**（例如"过去 7 天内所有失败的任务"）需要全量扫描。
3. **无事务一致性** — 保存轮次与其项之间的崩溃可能留下孤数据。
4. **事件时间线增长** — JSONL 追加对于重放是 O(n)；无索引。
5. **六个不同的模式版本常量**跨四个模块，每个都采用相同的拒绝更新版本策略。

## 2. 提案：统一 SQLite 后端（`codewhale-persistence`）

### 2.1 目标

将所有持久化状态合并到 `~/.codewhale/codewhale.db` 下的单个 SQLite 数据库中：

| 当前存储 | SQLite 表 |
|---|---|
| state（线程/消息/任务） | 合并到统一数据库中 |
| 会话 | `sessions` 表 |
| 运行时线程/轮次/项 | `runtime_threads`、`runtime_turns`、`runtime_items` 表 |
| 运行时事件 | `runtime_events` 表（带索引） |
| 任务 | `tasks` 表 |
| 自动化 | `automations` 表 |
| 审计 | `audit_log` 表 |

### 2.2 收益

- **列出/过滤**变成 SQL 查询，消除 O(n) 文件系统扫描。
- **事务一致性** — 轮次与其项在单个事务中原子保存。
- **索引事件时间线** — 按线程、时间戳、事件类型查询，无需全量扫描。
- **单一模式版本** — 一个 `schema_version` 编译指示，对于所有持久化数据。

### 2.3 迁移策略

1. 在新 crate `codewhale-persistence` 中创建统一数据库模式。
2. 编写从 JSON/JSONL 文件读取并写入 SQLite 的迁移代码。
3. 在启动时，检测旧数据，迁移它，在成功时重命名旧文件（`.migrated` 后缀）。
4. 至少保留一个发布版本的迁移路径。

## 3. 非目标

- 不更改运行时 API 或面向用户的接口。
- 不将 SQLite 暴露为公共 API（内部实现细节）。
- 不迁移尚未存在的状态（未来功能将直接使用 SQLite）。
- 不删除对 `rusqlite` 的依赖 — 它已经是 `crates/state` 中的依赖项。
