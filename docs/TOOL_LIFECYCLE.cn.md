# 工具表面生命周期策略（v0.8.53）

**状态：** 设计文档 / 策略。本次周期不落地目录代码——代码工作是**推迟的**。本文档是 GitHub **#2681** 的总体策略，其中 **#2682** 和 **#2683** 作为计划精简的具体实例。它描述了*将要做什么*以及任何未来精简 PR 必须遵守的不变量。

**相关开放工作的范围（不得与之矛盾）：**
- PR **#2684** — 子智能体角色词汇、生命周期信号、评估工效。此策略中的旧版子智能体名称清理 + 护栏测试以 #2684 为基础。
- PR **#2685** — git-history active + RLM/field 错误。

所有 file:line 引用均基于当前 CodeWhale 检出（v0.8.52/0.8.53）的已验证树。

---

## 1. 目的与弱模型问题

CodeWhale 提供了大量原生工具表面。该表面的第一轮次 *active* 分区是每个模型在运行任何 `tool_search_*` 调用之前所看到的内容。目前该 active 集合包含几个**近乎重复的工具**，它们以不同名称映射到*相同*的实现：

- `exec_wait` 和 `exec_shell_wait` 都是 `ShellWaitTool`（`crates/tui/src/tools/registry.rs:526,529`）。
- `exec_interact` 和 `exec_shell_interact` 都是 `ShellInteractTool`（`registry.rs:527,530`）。
- `tts` 和 `speech` 都是 `SpeechTool`（`registry.rs:787-792`，均为 deferred）。
- `work_update`、`checklist_*` 和 `todo_*` 是*相同*的 `TodoWriteTool` 表面，只有 `work_update` 对模型可见。

对于强模型，冗余名称是无害的噪音。对于**较弱/较小的模型**（Arcee Trinity 通道、`deepseek-v4-flash` 子执行器以及任何非思考执行器），可见集合中的每个额外近重复都意味着真实成本：

- 它用*没有任何独特之处*的选项扩大了选择空间，增加了错误工具选择和同义词间来回切换的几率。
- 它将稀缺的第一轮次目录预算（第 5 节）花费在零信息条目上。
- 它稀释了让小型模型能够推理工具表面的"一个名称 = 一件事"契约。

生命周期策略的存在是为了**缩小和规范模型可见的表面**，同时绝不破坏重放引用了已退役名称的旧转录的能力。

### v0.8.68 的规范工作跟踪表面

模型可见的进度表面是一个单一工具：`work_update`（#4132）。Agent 和 Fleet 工作者在活动运行时线程或持久化任务下使用它进行具体的待办/工作进度跟踪。

`task_*` 和 Fleet/Workflow 账本保持为持久化生命周期所有者。检查清单元数据是进度的模型可见投影：`task_updates.checklist` 携带当前条目、完成百分比和进行中条目。`update_plan` 是复杂计划的可选策略元数据/上下文/路线；它不得重复待办条目或成为并行的进度存储。

旧版 `checklist_*` 和更早的 `todo_*` 名称是隐藏兼容性别名。它们保持注册并可对相同的待办状态调度，以便旧转录无数据丢失地重放，但它们不向模型目录公告。

---

## 2. 五个生命周期状态

每个原生工具名称恰好占据一个生命周期状态。

| 状态 | 含义 | 第一轮次可见？ | 在 `tool_search_*` 中？ | 被调用时执行？ | 使用时机 |
|---|---|---|---|---|---|
| **active** | 规范的，在第一轮次目录头部 | **是** | 不适用（已 active） | 是 | 模型默认应使用的工具 |
| **deferred** | 已注册 + 可发现，按需加载 | 否 | **是** | 是 | 真实有用的工具，但不值得占用第一轮次位置 |
| **hidden-compatibility** | 已注册 + 可调度，但从 active **和** search 中移除 | 否 | **否** | **是 — 行为完全相同，静默** | 旧同义词，仅保留用于旧转录重放；不应有新模型发现它 |
| **deprecated** | 类似 hidden-compat，但执行时**向结果元数据追加替换通知** | 否 | **否** | **是 — 有效，外加"请改用 X"通知** | 淘汰的名称，我们主动引导调用者远离，重放仍安全 |
| **removed** | 完全未注册 | 否 | 否 | **否 — 硬错误** | 仅在 `planned_removal_version` 之后，正式放弃重放支持后 |

### hidden-compatibility vs deprecated — 要精确

两种状态都是**不可见的**（不在 active 中，不在工具搜索中），且都保持**可调度**（调用它们仍然有效）。*唯一*区别是面向调用者的信号：

- **hidden-compatibility：** 完全静默。工具的行为与其规范对应物逐字节相同。当没有*行为或命名教训需要传达*时使用——名称曾是纯别名，我们只是不想模型重新学习它。（例如：`exec_wait` 字面上就是 `exec_shell_wait`。）
- **deprecated：** 行为相同*且成功*，但工具结果的**元数据**携带追加通知，如 `"deprecated: use <replacement> instead"`。该通知**仅**出现在该调用的**结果元数据**中——绝不在缓存的工具目录前缀中（见第 8 节）。当存在我们希望调用者（以及任何阅读转录的人）被引导到的规范替换时使用。

两种状态都*不会*改变调用的*行为*。重放始终有效。

---

## 3. 代码中的表示

生命周期表示为 **const 名称集加上别名/清单表**，位于 `crates/tui/src/core/engine/tool_catalog.rs` 中，与现有的 `DEFAULT_ACTIVE_NATIVE_TOOLS`（`tool_catalog.rs:37-64`）和 `ARCEE_FIRST_TURN_NATIVE_TOOLS`（`tool_catalog.rs:106-115`）并列。

### 3a. 名称集和清单（草图）

```rust
// crates/tui/src/core/engine/tool_catalog.rs  （计划中）

/// 从 active 集合和 tool-search 中移除，但仍注册并可调度，行为逐字节相同。静默。
pub(super) const HIDDEN_COMPATIBILITY_TOOLS: &[&str] = &[
    "exec_wait",          // == exec_shell_wait  (ShellWaitTool)
    "exec_interact",      // == exec_shell_interact (ShellInteractTool)
    "tts",                // == speech (SpeechTool)
    "checklist_write",    // == work_update (TodoWriteTool)
    "checklist_add",      // == work_update 单条目添加
    "checklist_update",   // == work_update 单条目更新
    "checklist_list",     // == work_update 列表
    "todo_write",         // == work_update
    "todo_add",           // == work_update 单条目添加
    "todo_update",        // == work_update 单条目更新
    "todo_list",          // == work_update 列表
];

/// 弃用别名：不可见 + 可调度，仅在**结果元数据**中追加替换通知（绝不在缓存前缀中）。
pub(super) struct DeprecatedAlias {
    pub name: &'static str,
    pub replacement: &'static str,
    pub note: &'static str,
}

pub(super) const DEPRECATED_ALIASES: &[DeprecatedAlias] = &[
    // #4132 工作表面切换时为空：checklist_* 和 todo_* 是 work_update 的
    // 静默 hidden-compatibility 别名，用于转录重放。
];

#[inline]
pub(super) fn is_hidden_or_deprecated(name: &str) -> bool {
    HIDDEN_COMPATIBILITY_TOOLS.contains(&name)
        || DEPRECATED_ALIASES.iter().any(|d| d.name == name)
}
```

### 3b. 两个过滤点

1. **目录 / tool-search 排除（tool_catalog.rs）。** 延迟由 `should_default_defer_tool`（`tool_catalog.rs:66-82`）决定，active 集合是由 `build_model_tool_catalog`（`tool_catalog.rs:178-196`）构建的头部。Hidden-compat 和 deprecated 工具必须被强制*排除在 active 头部之外*和*排除在 tool-search 可发现池之外*。具体来说，延迟谓词获得短路使得这些名称永远不在 active 中，tool-search 索引构建器跳过任何 `is_hidden_or_deprecated(name)` 为 true 的名称。Arcee 的缩窄第一轮次路径（`apply_provider_tool_policy`，`tool_catalog.rs:134-149`）已按构造排除它们，因为它们不在 `ARCEE_FIRST_TURN_NATIVE_TOOLS` 中。

2. **结果通知追加（tool_routing.rs）。** 调度已在 `crates/tui/src/tui/tool_routing.rs` 中按工具名称路由（例如 wait/interact 统一在 `tool_routing.rs:1139-1140`）。成功调度后，如果调用的名称在 `DEPRECATED_ALIASES` 中，路由器将匹配的 `note` 追加到**结果元数据**中。Hidden-compat 名称不追加任何内容。

### 3c. 为什么使用名称集而不是每个 `ToolSpec` 的枚举字段

每个 `ToolSpec` 的 `lifecycle: Lifecycle` 字段被拒绝，有三个原因：

- **前缀缓存安全。** 工具目录数组是 DeepSeek 不可变 KV 前缀的一部分（`tool_catalog.rs:169-177`）。每个 spec 的字段会诱使将生命周期状态序列化到*每个*工具的 schema 中，这正是那种会强制完全重新预填充的头部变更。名称集完全存在于目录构建逻辑中，从不触及发出的工具 JSON。
- **单一事实来源 + 可 diff 性。** 一个版本的精简是对一个文件中两三个 const 数组的小型、可审查编辑，而不是分散在多个工具模块中的字段翻转。
- **注册保持正交。** 工具保持完全按今天的方式注册（例如 `with_shell_tools`，`registry.rs:523-531`）。生命周期是叠加在注册之上的*目录策略*，而不是嵌入到工具中的属性。

---

## 4. 弃用清单（#2681 验收标准表）

这是权威清单。各列是 #2681 AC 列。0.8.53 中没有条目是"removed"；重放支持列出的所有内容。

| 别名 | 替换（规范） | 生命周期状态 | first_deprecated_version | planned_removal_version | replay_supported |
|---|---|---|---|---|---|
| `exec_wait` | `exec_shell_wait` | hidden-compatibility | 0.8.53 | TBD (≥ 0.9.x) | Yes |
| `exec_interact` | `exec_shell_interact` | hidden-compatibility | 0.8.53 | TBD (≥ 0.9.x) | Yes |
| `tts` | `speech` | hidden-compatibility | 0.8.53 | TBD (≥ 0.9.x) | Yes |
| `checklist_write` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `checklist_add` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `checklist_update` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `checklist_list` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_write` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_add` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_update` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |
| `todo_list` | `work_update` | hidden-compatibility | 0.8.68 | TBD (≥ 0.9.x) | Yes |

**旧版子智能体名称 — 已移除，不需要清单条目。** 模型可见的子智能体表面仅为 `agent`。旧的生命周期名称和实验性 tool-agent 通道已被移除，而非保留为隐藏兼容性工具。

`planned_removal_version` 有意设为 `TBD`：一个名称仅在正式放弃对足够旧的转录的重放支持后才移至 **removed**，这是每个名称单独的、慎重的决策。

---

## 5. Active 目录预算（按模式、按提供商）

Active 集合是第一轮次成本。不要在此重复确切的 `DEFAULT_ACTIVE_NATIVE_TOOLS` 计数：v0.8.53 批次中的相邻 PR 可能会添加或移除 active 工具，事实来源始终是 `tool_catalog.rs`。本文档定义精简策略和不变量，而非第二个目录快照。

### 按提供商

| 提供商 | 第一轮次 active 来源 | 预算策略 |
|---|---|---|
| 默认（DeepSeek 等） | `DEFAULT_ACTIVE_NATIVE_TOOLS` | 当规范对应工具保持 active 时，从 active 头部移除重复别名；任何净增长需要明确的预算决策。 |
| Arcee（Trinity） | `ARCEE_FIRST_TURN_NATIVE_TOOLS` | 提供商特定的只读 WAF 变通方案；除非显式审查，否则不受默认精简影响。 |

默认精简从 active 头部移除 `exec_wait` 和 `exec_interact`（它们变为 hidden-compat；其规范对应工具 `exec_shell_wait` / `exec_shell_interact` 保留）。`tts` 和 `todo_*` *已经不在* active 集合中，因此在此次精简中不改变 active 预算。此特定精简的净效果是从当前 v0.8.53 PR 批次后的任何默认 active 头部中移除两个重复的 active 别名。

### 按模式（Plan / Agent / YOLO）

原生 active 头部按设计在模式之间是**相同集合**——模式不会从 `DEFAULT_ACTIVE_NATIVE_TOOLS` 中添加或移除原生工具（`should_default_defer_tool` 对原生工具忽略 `_mode`，`tool_catalog.rs:66-68`）。模式影响的是 **MCP** 延迟：`apply_mcp_tool_deferral` 保持 MCP 工具延迟，除非 `mode == Yolo`（`tool_catalog.rs:162-167`）。

| 模式 | 原生 active 预算 | MCP 工具 active？ |
|---|---|---|
| Plan | 相同原生头部 | 否（deferred） |
| Agent | 相同原生头部 | 否（deferred） |
| YOLO | 相同原生头部 | 是（已知的、有意的放宽） |

**预算规则：** 原生 active 头部必须在 Plan ↔ Agent ↔ YOLO 之间保持逐字节相同（第 8 节）。任何头部的增长需要退役其他内容或本文档中的显式预算提升。

---

## 6. 规范表面规则

> **每个模型可见的（active 或 deferred-discoverable）工具必须有一个清晰的定位。如果工具被取代，它获得一个命名替换并移至 hidden-compatibility 或 deprecated — 它不得保持可见。**

### 混淆集群的规范 vs 兼容性总结

| 集群 | 规范（保持可见） | 兼容性 / 退役 | 备注 |
|---|---|---|---|
| **Shell wait** | `exec_shell_wait` | `exec_wait` → hidden-compat | 相同 `ShellWaitTool`（`registry.rs:526,529`）；路由器已统一（`tool_routing.rs:1139`） |
| **Shell interact** | `exec_shell_interact` | `exec_interact` → hidden-compat | 相同 `ShellInteractTool`（`registry.rs:527,530`） |
| **工作进度 / checklist / todo** | `work_update` | `checklist_write/add/update/list`、`todo_write/add/update/list` → hidden-compat | 相同 `TodoWriteTool`；兼容性名称仅用于重放旧转录 |
| **Speech / tts** | `speech` | `tts` → hidden-compat | 相同 `SpeechTool`（`registry.rs:787-792`） |
| **子智能体生命周期** | `agent` | 旧生命周期名称和 tool-agent 通道已移除 | 单一异步启动器；子智能体是叶子工作者 |
| **编辑家族** | `apply_patch`、`edit_file`、`write_file`、`fim_edit` | 无 — **各有不同的定位** | 不触及（按 #2681 非目标）；仅文档规范指导 |
| **搜索家族** | `grep_files`（内容）、`file_search`（文件名）、`project_map`（结构） | 无 — **各有不同的定位** | 不触及；目前不存在 FTS5/BM25/语义索引 |

**非目标（按 #2681 明确不在本次精简周期内）：**
`apply_patch` / `edit_file` / `write_file` / `fim_edit`；
`grep_files` / `file_search` / `project_map`；
`fetch_url` / `web.run` / `web_search`；
`task_shell_*`；`handle_read` / `retrieve_tool_result`。这些具有不同定位，仅接收**规范指导** — 无生命周期变更。

RLM 表面（`rlm_open` / `rlm_eval` / `rlm_configure` / `rlm_close` / `rlm_session_objects`，`crates/tui/src/tools/rlm.rs`）同样不在范围内；`handle_read` 检索 var handle，而 `finalize` / `FINAL` 是内核内 Python 函数，**不是工具** — 因此那里没有需要退役的内容。

---

## 7. 子智能体切换决策：一个可见启动器

旧的生命周期三元组和 tool-agent 通道已被移除，不是隐藏兼容性工具。

**决策：仅暴露 `agent`。**

- `agent` 启动一个聚焦的后台子任务并返回 agent id 加转录句柄。
- 子任务结果以完成事件到达。父任务应继续工作，而不是轮询生命周期工具。
- 子任务工具目录排除子智能体生命周期工具，因此子任务是叶子工作者，不能递归启动更多智能体。
- 详细检查通过返回的转录句柄上的 `handle_read` 进行。

这是生命周期简化，而非提供商关卡。

---

## 8. 前缀缓存安全 + 重放保证

### 每个精简 PR 必须遵守的前缀缓存规则

工具数组是 DeepSeek 不可变 KV 前缀的一部分。目录头部字节稳定性不变量（`tool_catalog.rs:169-196`）具有约束力：

1. **绝不非确定性地变更 active 头部。** 第一轮次 active 块必须在运行间逐字节相同，且在 Plan ↔ Agent ↔ YOLO 之间相同。
2. **精简是一次性确定性编辑。** 从 `DEFAULT_ACTIVE_NATIVE_TOOLS` 中移除名称恰好一次性地改变头部；之后必须保持稳定。将此类编辑作为其自己的聚焦变更落地。
3. **通知存在于结果元数据中，绝不在前缀中。** 弃用替换注释在 `tool_routing.rs` 中的调度时追加到*调用结果*中。**任何**关于 hidden/deprecated 状态的内容都不得序列化到工具 schema、描述或目录数组中。
4. **保持排序和分区。** `build_model_tool_catalog` 按名称对每个分区排序，并将内置工具作为 MCP 工具前的连续前缀（`tool_catalog.rs:186-194`）。精简编辑不得破坏此规则。
5. **Hidden/deprecated 工具在头部构建*之前*被排除**，因此它们的移除是唯一的头部变更 — 它们完全不出现在前缀中。

### 旧转录重放保证

> 对于弃用清单中 `replay_supported = Yes` 的每个名称，工具保持**注册并可调度，行为相同**。重放调用 `exec_wait`、`exec_interact`、`tts` 或任何 `todo_*` 的旧转录产生与其一直以来的相同结果。弃用名称额外附加结果元数据通知；hidden-compat 名称是静默的。名称仅在经过慎重的、每个名称单独的决策在 `planned_removal_version` 放弃重放支持后，才变为不可调度（**removed**）。

---

## 9. 必需的测试

任何精简 PR（以及总括 #2681 工作）必须添加/保持：

1. **重复 active 别名防护。** 一个测试断言 `HIDDEN_COMPATIBILITY_TOOLS` 或 `DEPRECATED_ALIASES` 中没有任何名称出现在 `DEFAULT_ACTIVE_NATIVE_TOOLS` 或 `ARCEE_FIRST_TURN_NATIVE_TOOLS` 中，且没有两个 active 条目解析为相同的基础工具实现。

2. **工具搜索排除测试。** 断言 hidden-compat 和 deprecated 名称不在 tool-search 可发现池中，同时仍存在于注册表中（可调度）。

3. **重放 / 调度测试。** 对于清单中的每个名称，调用它仍然执行并返回与其规范对应工具相同的结果。弃用名称额外断言替换通知存在于**结果元数据**中且不在目录/前缀中。Hidden-compat 名称断言**无**添加通知。

4. **黄金 active 块字节测试。** 一个快照测试固定第一轮次 active 工具块的字节序列化，断言其在 Plan / Agent / YOLO 之间相同（原生头部）且运行间稳定 — 强制执行 `tool_catalog.rs:169-196` 不变量。黄金值**仅**在精简落地时作为已审查的、慎重的一次性编辑更新。

5. **子智能体护栏测试。** 断言仅 `agent` 注册为模型可见的子智能体工具，且来自 `subagent/mod.rs` 的 hidden/legacy 名称不被公告。

6. **叶子工作者测试。** 断言子智能体工具目录排除 `agent` 和已退役的旧版生命周期名称。
