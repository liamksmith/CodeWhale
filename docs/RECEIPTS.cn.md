# 运行时收据

本文档勾勒了一个未来用于已完成运行时轮次的只读收据导出功能。这是一份协议说明，而非已实现的端点。

目标是为本地监管者提供审计单个已完成轮次的能力，无需截取终端输出的屏幕。一份收据应摘要 CodeWhale 已有的持久化运行时记录：线程元数据、轮次状态、轮次条目、事件序列谱系、可用时的用量信息、审批决策以及副作用边界。

## 非目标

收据不是安全认证、提供商兼容性认证或托管证明。它不得调用提供商、执行工具、写入内存、写入项目文件、变更运行时状态或暴露 API 密钥。

默认情况下，收据不应导出原始的思维链或私有推理内容。当表示推理保管时，应使用稳定的条目 ID、计数、哈希值或显式的 `unavailable` 字段，而非原始隐藏内容。

## 候选表面

潜在的仅本地表面：

```text
codewhale receipt export --thread <thread_id> --turn <turn_id> --format json
GET /v1/threads/{thread_id}/turns/{turn_id}/receipt
```

两个表面应共享现有的运行时 API 认证边界。它们只能读取持久化的运行时记录和仅追加事件。

## 审查收据

`codewhale review --write-receipt` 将审查过的 diff 的本地 JSON 收据写入 CodeWhale 状态目录（`review-receipts/`），除非提供了 `--receipt-path <path>`。这是一份推送前的交接产物：它记录了审查了什么 diff 以及审查报告了什么，不会推送、打标签、创建 PR 或声称替代维护者审查。

当前收据包括：

- `diff_fingerprint`：审查的 diff 的 SHA-256 值。
- `provider` 和 `model`：路由的审查提供者/模型。
- `checks_run`：可用时附加到收据的本地检查。为空表示未附加任何检查；附加的检查必须报告通过状态。
- `findings`：当审查输出为结构化时，结构化的问题/建议计数和问题位置。
- `unresolved_risk`：从未解决的发现中推导出的保守摘要。
- `review_content_sha256`：审查文本的 SHA-256 值。

收据故意不包含原始 diff 体。更改 diff 后重新运行 `codewhale review --write-receipt`；审查者在 PR 交接中重用收据前应比较 `diff_fingerprint`。

`codewhale review --check-receipt` 是本地推送前关卡。它不调用模型；它将当前 diff 指纹与提供的收据（`--receipt-path <path>`）或最新的匹配本地收据进行比较。当 diff 不再匹配、收据 schema 不受支持、收据存在未解决风险或附加的检查未通过时，检查以非零退出。

## 当前数据源

当前运行时存储已持久化了收据构建器所需的核心输入：

- `ThreadRecord`：模型、工作区、模式、shell/trust/auto-approve 标志、标题、任务关联和最新轮次元数据。
- `TurnRecord`：轮次状态、输入摘要、时间戳、持续时间、用量、错误、引导计数和条目 ID。
- `TurnItemRecord`：条目类型、生命周期状态、摘要、可选详情、元数据、产物引用和条目时间戳。
- `RuntimeEventRecord`：线程 ID、轮次 ID、条目 ID、事件名称、JSON 负载、时间戳以及每个运行时存储的单调 `seq` 值。

并非每个收据字段都能从这些记录中填充。如果提供商或存储未持久化某个值，收据应注明 `available: false` 或 `unavailable`，而不是从 UI 文本中推断。

## 草稿 Schema 形态

```json
{
  "schema_id": "codewhale.conformance-receipt/v0",
  "thread": {
    "id": "thr_...",
    "model": "deepseek-v4-pro",
    "mode": "agent",
    "auto_approve": false,
    "trust_mode": false,
    "allow_shell": false
  },
  "turn": {
    "id": "turn_...",
    "status": "completed",
    "started_at": "2026-06-02T01:00:00Z",
    "ended_at": "2026-06-02T01:00:12Z",
    "duration_ms": 12000
  },
  "reasoning_custody": {
    "raw_reasoning_exported": false,
    "available": false,
    "reason": "推理块未作为收据就绪记录持久化"
  },
  "tool_lineage": {
    "tool_call_count": 1,
    "tool_result_count": 1,
    "unmatched_tool_call_ids": [],
    "unmatched_tool_result_ids": []
  },
  "usage_evidence": {
    "available": true,
    "usage": {
      "prompt_tokens": 123,
      "completion_tokens": 45
    },
    "provider_cache_breakdown_available": false
  },
  "source_event_lineage": {
    "first_seq": 10,
    "last_seq": 42,
    "event_count": 33,
    "missing_event_ranges": []
  },
  "side_effect_boundary": {
    "approval_required_count": 1,
    "approval_allowed_count": 0,
    "approval_denied_count": 1,
    "command_execution_count": 0,
    "file_change_count": 0,
    "sandbox_denied_count": 0
  },
  "claim_ceiling": [
    "local_receipt_only",
    "not_safety_certification",
    "not_provider_compatibility_certification"
  ]
}
```

## 构建器规则

收据构建器应是确定性和保守的：

1. 按 ID 加载线程和轮次，然后拒绝不匹配的 `thread_id` 值。
2. 仅加载轮次引用的条目 ID。
3. 读取线程的事件记录并按 `turn_id` 过滤。
4. 使用 `first_seq`、`last_seq` 和任何检测到的间隙来保留事件序列边界。
5. 仅从类型化记录或已知事件名称统计审批、命令、文件、沙箱和工具事件。
6. 显式标记不可用的证据，而非从自由格式摘要中推导。
7. 除现有的条目摘要外，不输出任何原始工具输出，除非后续 schema 添加了单独的脱敏策略。

## 增量实现路径

最安全的实现路径是：

1. 落地本协议说明并确定字段名称/非目标。
2. 为已完成、失败和审批被拒的轮次添加协议结构体和 JSON 快照 fixture。
3. 在 `ThreadRecord`、`TurnRecord`、`TurnItemRecord` 和 `RuntimeEventRecord` 之上添加一个纯构建器。
4. 暴露本地运行时 API 端点。
5. 添加 CLI 导出命令和可选的验证模式。
