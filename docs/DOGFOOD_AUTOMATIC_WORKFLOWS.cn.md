# 内部测试：自动 Workflow 场景 (#4131)

针对软自动 Workflow 产品路径的可复现检查。其他工程师应能仅凭本文档重新运行每个场景。

相关文档：

- [自动 Workflow](AUTOMATIC_WORKFLOWS.md) — 产品行为
- [Workflow 编写](WORKFLOW_AUTHORING.md) — 已检入的脚本/IR
- [Fleet + Workflow 教程](FLEET_WORKFLOW_TUTORIAL.md) — 手动 Fleet 路径
- 示例脚本：[`docs/examples/dogfood-automatic/`](examples/dogfood-automatic/)
- 面板单元覆盖率：`crates/tui/src/tui/widgets/workflow_panel.rs`（`dogfood_*` 测试）
- 运行时韧性：`crates/workflow-js/tests/vm_tests.rs`（`parallel_*`、取消丢弃）

## 前置条件

```bash
# 从 origin/main（或待测 PR）上的干净 worktree 执行
cargo build -p codewhale-tui --locked
# 以下场景使用的可选无头/运行时检查
cargo test -p codewhale-tui --locked dogfood_ -- --nocapture
cargo test -p codewhale-workflow-js --locked
```

隔离配置，使内部测试不会触及你真实的 home 目录：

```bash
export DOGFOOD_ROOT="$(mktemp -d)"
export CODEWHALE_HOME="$DOGFOOD_ROOT/codewhale-home"
export HOME="$DOGFOOD_ROOT/home"
mkdir -p "$HOME" "$CODEWHALE_HOME" "$DOGFOOD_ROOT/workspace"
cd "$DOGFOOD_ROOT/workspace"
# 指向待测的 CodeWhale 检出版本，用于读取/审计工作
export CODEWHALE_SRC=/path/to/CodeWhale
```

安全规则：

1. 在内部测试期间不要执行 `git push`。
2. 优先使用只读提示词；对写入/worktree 运行要经过谨慎审批。
3. 测试完成后使用 `rm -rf "$DOGFOOD_ROOT"` 清理。

主要交互界面：

```bash
codewhale-tui   # 或： cargo run -p codewhale-tui --locked
```

确认软自动已开启（`[workflow] automatic = true` 是默认值）。

---

## 场景矩阵

| ID | 场景 | 主要提示词/命令 | 预期 UI | 自动检查 |
|----|------|--------------------------|-------------|-----------------|
| WF-A1 | 只读仓库审计 | 自然语言审计提示词 | 软自动或 `/workflow`；面板阶段；纯读取计划无需写入审批 | `dogfood_read_only_repo_audit_panel` |
| WF-A2 | 分阶段 Bug 修复（worktree 实现者 + 验证者） | 分阶段实现+验证提示词 | 实现者行显示 `wt`，验证者在主分支或第二阶段；如有提权则需写入审批 | `dogfood_staged_worktree_implementer_verifier` |
| WF-A3 | 部分失败 + 综合 | 并行部分失败脚本/提示词 | 失败槽位显示 null/`fail` 计数；综合仍然产出操作者摘要 | `dogfood_partial_failure_and_synthesis` + workflow-js `parallel_fan_out_*` |
| WF-A4 | 运行中途取消 | 启动长时间运行 → 面板 `[c]` 或 `/workflow cancel` | 生命周期 `cancelled`；运行的子进程被取消；调用了 cancel_all | `dogfood_cancellation_mid_run` + workflow-js 丢弃/取消测试 |

每次交互测试后填写底部的通过/失败表。

---

## WF-A1 — 只读仓库审计

### 可复现的提示词

在 `codewhale-tui` 中，工作区 = CodeWhale 检出版本（或其副本）：

```text
Audit this repository for security and reliability risks. Stay read-only:
list crates, scan for unsafe blocks and unwrap in hot paths, and summarize
findings by severity. Do not edit files or run destructive commands.
```

如果软自动未触发，强制编排：

```text
/workflow
```

然后重申相同的审计目标，或运行已检入的示例：

```text
/workflow run docs/examples/dogfood-automatic/wf_a1_read_only_audit.workflow.js
```

### 预期 UI 行为

- 软自动可能在启动前声明形状（"先侦察 crate 再综合"）。
- 当 `auto_start_read_only = true` 时，只读计划可能无需写入审批卡片即可启动。
- Workflow 面板显示 ≥1 个阶段和带有角色/标签的子行（不是"未知子进程"）。
- 紧凑历史卡片保持简洁；展开可查看阶段/子进程详情。
- 纯读取侦察不需要 worktree 相关行（工作区 = main）。

### 通过/失败记录

| 检查项 | 通过？ | 备注 |
|-------|-------|-------|
| 编排已启动（软自动或 `/workflow`） | | |
| 面板显示阶段 + 带标签子进程 | | |
| 纯读取计划无需写入审批 | | |
| 无文件编辑 / 无推送 | | |
| 综合摘要操作者可读 | | |

自动化：

```bash
cargo test -p codewhale-tui --locked dogfood_read_only_repo_audit_panel
```

---

## WF-A2 — 分阶段 Bug 修复（worktree 实现者 + 验证者）

### 可复现的提示词

```text
Staged fix for a small bug in the docs only:
1) implementer: add a one-line clarification to docs/AUTOMATIC_WORKFLOWS.md
   in an isolated worktree (do not touch main workspace).
2) verifier: re-read the file and confirm the change is correct; do not
   implement further edits.
Keep the change minimal and reversible.
```

或运行：

```text
/workflow run docs/examples/dogfood-automatic/wf_a2_staged_bugfix.workflow.js
```

### 预期 UI 行为

- 提权计划（写入 / worktree）在 `require_approval_for_writes = true` 时显示审批卡片（#4126）。
- 面板阶段类似：实现 → 验证（或等效标签）。
- 实现者子行显示 `wt`（worktree）隔离。
- 验证者子进程完成并附带简短确认摘要。
- 验证者评估实现者返回的 worktree 交接结果；它不期望未合并的编辑出现在父工作区中。
- 每个委派单元一个工件/卡片（无重复委派垃圾信息）。

### 通过/失败记录

| 检查项 | 通过？ | 备注 |
|-------|-------|-------|
| 写入/worktree 计划显示审批卡片 | | |
| 实现者行标记为 worktree | | |
| 验证者在实现者之后运行 | | |
| 主工作区在合并/应用前未被修改 | | |
| 验证者验证隔离的交接结果而非父文件 | | |
| 紧凑历史卡片总结阶段 | | |

自动化：

```bash
cargo test -p codewhale-tui --locked dogfood_staged_worktree_implementer_verifier
```

---

## WF-A3 — 部分失败和综合

### 可复现的命令/脚本

无头运行时（始终可运行）：

```bash
cargo test -p codewhale-workflow-js --locked \
  parallel_fan_out_maps_one_failure_to_null_slot \
  parallel_logs_a_breadcrumb_when_a_slot_is_dropped_to_null
```

交互式/工具路径：

```text
/workflow run docs/examples/dogfood-automatic/wf_a3_partial_failure_synthesis.workflow.js
```

自然语言等效表达：

```text
Run three parallel scouts; deliberately allow one to fail. Synthesize a single
operator-facing summary from the successful slots and call out the failed branch.
```

### 预期 UI 行为

- 失败的并行槽位显示为失败/已取消的行或带有日志面包屑的 null 槽位（不是静默丢弃）。
- 当子进程失败时，面板标题显示非零 `fail` 计数。
- 运行仍能以存活槽位的综合摘要完成（`parallel()` 部分成功语义）。
- 展开的历史卡片列出失败的子进程 + 总体结果摘要。

### 通过/失败记录

| 检查项 | 通过？ | 备注 |
|-------|-------|-------|
| 失败槽位可见（行和/或面包屑） | | |
| 成功槽位仍对摘要有贡献 | | |
| 标题失败计数 ≥ 1 | | |
| 单个子进程失败不会导致整个运行 panic | | |

自动化：

```bash
cargo test -p codewhale-tui --locked dogfood_partial_failure_and_synthesis
cargo test -p codewhale-workflow-js --locked parallel_fan_out_maps_one_failure_to_null_slot
```

---

## WF-A4 — 运行中途取消

### 可复现的步骤

1. 启动一个长时间运行的多子进程 workflow，不阻塞父轮次：

```text
Use the workflow tool with exactly
{"action":"start","source_path":"docs/examples/dogfood-automatic/wf_a4_cancel_mid_run.workflow.js"}.
```

或一个带有多个侦察者的自然语言长审计。

2. 当状态为 `running` 时，通过以下方式之一取消：

```text
# 面板聚焦 + 按键
[c]   # 或 X — Workflow 面板取消

# 斜杠命令
/workflow status
/workflow cancel <run_id>
```

### 预期 UI 行为

- 面板显示 `cancelling…`，然后生命周期变为 `cancelled`。
- 仍在运行的子进程最终化为已取消；已成功的行保持成功状态。
- 主机取消路径是幂等的（第二次取消是空操作）。
- 已完成的面板保持可见，直到下一次运行开始。

### 通过/失败记录

| 检查项 | 通过？ | 备注 |
|-------|-------|-------|
| 运行中可接受取消 | | |
| 生命周期变为 cancelled | | |
| 运行中的子进程被取消；已完成的子进程被保留 | | |
| 第二次取消是空操作 | | |
| 取消后无挂起的 agent | | |

自动化：

```bash
cargo test -p codewhale-tui --locked dogfood_cancellation_mid_run
cargo test -p codewhale-workflow-js --locked dropping_the_run_future_cancels_outstanding_tasks
```

---

## 发现的 Bug

在此处记录内部测试期间发现或关联的 bug（不要静默吞掉它们）：

| 日期 | 场景 | 症状 | Issue / PR |
|------|----------|---------|------------|
| 2026-07-09 | WF-A1 | 文档中 `export default async function` 的夹具被运行时脱糖路径拒绝。 | 在 #4325 中修复，并附带回归覆盖。 |
| 2026-07-09 | WF-A1 | 包含 `description` 和 `prompt` 两个字段的夹具选项因重复字段导致 serde 解码失败。 | 在 #4325 中修复，附带 prompt 优先级覆盖。 |
| 2026-07-09 | WF-A3 | 拒绝是一个成功的子进程完成，因此它无法确定性地测试一个可空的失败槽位。 | 夹具现在使用一个 token 的子进程预算；#4325。 |
| 2026-07-09 | WF-A4 | 运行取消被降级为一个可空的并行槽位，允许脚本进入其不可达的下一阶段。 | 在 #4325 中通过外部取消回归修复。 |
| 2026-07-09 | WF-A4 | 一个竞态的 `run_completed: failed` 事件使实时面板保持失败状态并带有运行中的行，尽管收据已被取消。 | 在 #4325 中通过终端行最终化 + 权威的 `run_cancelled` 流修复。 |

---

## 交互式结果日志（每次测试复制一份）

树/二进制：`codex/v0868-workflow-export-default`，调试 `codewhale-tui`
操作者：发布内部测试会话
日期：2026-07-09

- WF-A1：通过 — `workflow_dd5de6d0`；3 个带标签的只读侦察者，然后一个综合者，4/4 完成，跨越两个阶段。检入的 `source_path` 因执行前能力未知而保守地请求了审批；无工作区文件被更改。
- WF-A2：通过 — `workflow_97ae14dc`；隔离的实现者 worktree，随后是主工作区验证者，2/2 在 1 分 31 秒内完成 Implement/Verify 阶段。验证者确认了预期的交接结果，并且未合并的编辑在父工作区中保持缺失，将隔离视为成功而非期望跨 worktree 变更。
- WF-A3：通过 — `workflow_2f590eec`；一个子进程明显耗尽了其单 token 预算，标题显示一次失败，`parallel()` 提供了一个 null 槽位，综合者从两个存活者完成（`surviving_count=2`）。
- WF-A4：通过 — `workflow_45629ac6`；非阻塞启动后显式取消，产生生命周期 `cancelled`，保留了一个已完成的子进程，将两个运行中的子进程最终化为已取消，未进入不可达阶段，且未返回结果。重复取消通过工具回归测试覆盖为空操作。

新提交的 bug：无；上述所有可复现缺陷已在 #4325 中修复。
后续工作：在合并后运行来自 #4178/#4179 的 Fleet 支持的 stopship 通道。

### CI / PR 关卡（非交互式）

```bash
cargo fmt --all -- --check
cargo test -p codewhale-tui --locked dogfood_
cargo test -p codewhale-workflow-js --locked
```
