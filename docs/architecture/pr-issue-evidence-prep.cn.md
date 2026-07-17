# EPIC 证据准备

## EPIC-002 关闭证据（最终——阶段 8 完成；准备提 PR）

**Epic：** EPIC-002 —— 命令单一职责抽取
**关联 EPIC：** [#2870](https://github.com/Hmbown/CodeWhale/issues/2870)
**关联 Issue：** [#2791](https://github.com/Hmbown/CodeWhale/issues/2791)、
[#2851](https://github.com/Hmbown/CodeWhale/pull/2851)、
[#2887](https://github.com/Hmbown/CodeWhale/pull/2887)

本节记录在阶段 8（最终检查点）验证的 EPIC-002 最终关闭证据。以下所有证据均通过在当前工作树上运行文档化命令收集。

### PR 引用

- 第 4 层（FEAT-006）：Core、config、session 和 debug 命令抽取
- 第 4.1 层（FEAT-007）：Project、memory、skills、utility 和 plugins 抽取
- 第 4.2 层（FEAT-008）：注册表清理、文档和完整验证

### 验收证据

| AT ID | 检查项 | 结果 |
|-------|--------|------|
| AT-001 | `cargo test -p codewhale-tui acceptance`（epic_acceptance_harness + eval_harness） | ✅ 2 通过（0 失败） |
| AT-002 | `every_registered_command_dispatches_to_a_handler` | ✅ 通过（489 个命令测试的一部分） |
| AT-003 | `every_command_alias_dispatches_to_a_handler` | ✅ 通过（489 个命令测试的一部分） |
| AT-004 | 帮助/面板/补全界面测试（包含在 489 个命令测试中） | ✅ 通过 |
| AT-005 | `dispatch_prefers_user_command_over_builtin_with_same_name` | ✅ 通过 |
| AT-006 | `hidden_user_commands_still_dispatch_directly` | ✅ 通过 |
| AT-007 | `unknown_command_suggests_nearest_match` | ✅ 通过 |
| AT-008 | `command_registry_has_unique_names_and_aliases` | ✅ 通过（0 个重复名称/别名） |
| AT-009 | `command_ownership_contract_is_enforced` | ✅ 通过（9 个分组，分层所有权） |
| AT-010 | 清理清单——无未记录的迁移路径 | ✅ 已验证（所有项均为永久例外或不存在） |
| AT-011 | 最终关闭矩阵（本文档） | ✅ 完成 |

### 永久例外项

| 例外项 | 理由 |
|--------|------|
| 配置分组本地元数据 | Config `mod.rs` 保留 11 个 `CommandInfo` 静态变量和调度——永久结构，不在清理范围 |
| Debug 分组本地元数据 | Debug `mod.rs` 保留 11 个 `CommandInfo` 静态变量和调度——永久结构，不在清理范围 |
| `/jihua`、`/zidong` | `/mode` 的中文向后兼容别名——早于分组归属注册表 |
| `/slop`、`/canzha` | `/debt` 的仅输入别名——早于分组归属注册表 |
| `/set`、`/deepseek` 迁移提示 | 已退役命令，仅直接输入指导，已从注册表/补全中排除 |
| `$skill` 前缀 | 非斜杠兼容语法，早于 EPIC-002 |
| 技能名称回退 | 斜杠命令在内置命令和用户命令之后回退到技能调度 |
| `command_runs_directly()` 面板列表 | UI 策略决策，非注册表元数据 |
| 公开 re-export 桥接路径 | 长期公开 API 兼容性 |
| 用户命令兼容加载器 | `.deepseek`、`.claude`、`.cursor` 目录——用户命令范围，非内置清理 |
| `#[allow(clippy::module_inception)]` | 同名分组和子模块的有意结构 |

### 验证

- `cargo fmt --all -- --check` —— 干净
- `cargo check -p codewhale-tui` —— 干净（无错误，无警告）
- `cargo test -p codewhale-tui commands::` —— 489 通过（0 失败）
- `cargo test -p codewhale-tui acceptance` —— 2 通过（epic_acceptance_harness：1 场景，3 步骤；eval_harness：1 测试）
- `cargo test --workspace` —— 5344 通过，1 失败（已知不稳定：`run_verifiers_background_starts_shell_jobs_and_returns_task_ids`；隔离运行时通过——预先存在的问题，非 FEAT-008 回归），2 忽略
- `git diff --check` —— 干净（两个仓库）
- 孤立文件检查 —— 无孤立 `.rs` 文件

## FEAT-008 PR 摘要草稿

**标题：** 第 4.2 层：注册表清理、文档和完整验证（FEAT-008）

```markdown
Refs #2870。

## 摘要

FEAT-008 通过移除仅过渡期的命令脚手架、验证命令和别名唯一性、更新经源码验证的命令架构文档以及准备可审计的 EPIC 关闭证据，完成了 EPIC-002（命令单一职责抽取）。这是第 4.2 层（最终清理和验证层）。

## 变更

- 不再保留临时适配器、重复命令列表或仅迁移用的调度路径——所有 §3.2 清单项在第 3 阶段源码验证后确认为永久例外或不存在。
- 命令注册所有权遵循最终的分层模型：顶层分组注册 → 分组归属的命令模块 → 命令级元数据和行为。
- 架构文档（`docs/architecture/command-dispatch.md`）已更新以反映最终调度流程和永久例外项。
- PR/issue 证据文档（`docs/architecture/pr-issue-evidence-prep.md`）已为 EPIC-002 关闭做好准备。

## Gherkin / 验收覆盖

- `tests/epic_acceptance_harness.rs` —— 1 场景，3 步骤（AT-001）
- `tests/core_session_command_extraction.rs` —— 1 场景，4 步骤（AT-002/003）
- `tests/eval_smoke_acceptance.rs` —— 1 场景，4 步骤（非 AT-004 证据）
- `tests/plugin_e2e_acceptance.rs` —— 4 测试（AT-002/003/004 覆盖）
- AT-008：`command_registry_has_unique_names_and_aliases` —— 由测试强制执行
- AT-009：`command_ownership_contract_is_enforced` —— 由测试强制执行
- AT-010：清理清单已验证——无未记录的迁移路径

## 验证

| 检查项 | 结果 |
|--------|------|
| `cargo fmt --all -- --check` | 干净 |
| `cargo check -p codewhale-tui` | 干净（0 错误，0 警告） |
| `cargo test -p codewhale-tui commands::` | 489 通过，0 失败 |
| `cargo test -p codewhale-tui acceptance` | 2 通过（epic_acceptance_harness：1，eval_harness：1） |
| `cargo test --workspace` | 5344 通过，1 已知不稳定（verifier 并行竞争；隔离运行通过），2 忽略 |
| `git diff --check` | 干净（两个仓库） |
| 孤立文件检查 | 无孤立 `.rs` 文件 |
| `git status --porcelain` | 干净（CodeWhale 仓库） |

Paulo Aboim Pinto
```

---

## EPIC-001 Hunter 重放证据

**目标分支：** `hunter/0.8.62-glm-subagents`
**重放分支：** `feat/replay-epic-001-on-hunter`
**关联 EPIC：** [#2870](https://github.com/Hmbown/CodeWhale/issues/2870)
**关联 Issue：** [#2791](https://github.com/Hmbown/CodeWhale/issues/2791)

本节记录将 EPIC-001 FEAT-001、FEAT-002 和 FEAT-003 重放到 Hunter 分支的工作 PR/issue 证据检查清单。

## 重放范围

| 功能 | Hunter 重放决策 |
|------|-----------------|
| FEAT-001 | 不进行原始 cherry-pick。Hunter 已包含更新的分组归属命令树和 trait 支持的注册表。 |
| FEAT-002 | 语义重放为 `user_registry.rs`，接入调度、面板和斜杠补全。适配以保留更新的 Hunter 命令状态重置行为。 |
| FEAT-003 | 重放为针对 Hunter 目标的公开架构和 PR/issue 证据文档。旧的 release 分支验证声明未被复制。 |

## PR 摘要草稿

```markdown
## 摘要

将已完成的 EPIC-001 命令边界工作重放到 `hunter/0.8.62-glm-subagents`。

## 变更

- 保留 Hunter 现有的 trait 支持内置命令注册表和嵌套的分组归属命令树作为 FEAT-001 的结果。
- 为 markdown 用户命令添加专用的 `UserCommandRegistry` 边界。
- 将用户命令调度、命令面板条目和斜杠补全通过注册表路由。
- 在用户命令启动时保留 Hunter 更新的命令状态重置行为，包括 todos 和计划状态。
- 保留空 `allowed-tools` 语义：显式空值阻止所有工具。
- 为 Hunter 目标添加公开架构和 PR/issue 证据文档。

## 验证

- `cargo fmt --all -- --check`
- `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo check -p codewhale-tui`
- `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo test -p codewhale-tui commands::`
- `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo test -p codewhale-tui command_palette`
- `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo test -p codewhale-tui slash_completion`
- `git diff --check`
```

## Issue #2870 评论文本

```markdown
EPIC-001 已通过语义重放而非原始 cherry-pick 方式重放到 Hunter 目标。

- FEAT-001：由 Hunter 当前的 trait 支持注册表和分组归属命令树表示。
- FEAT-002：重放为用户命令注册表边界，适配以保留当前 Hunter 行为。
- FEAT-003：重放为针对 Hunter 目标的公开架构和证据文档。

验证证据包含在 PR 正文中。

Paulo Aboim Pinto
```

## 验证结果

在打开或更新 PR 之前在此处记录实时结果。

| 检查项 | 结果 |
|--------|------|
| `cargo fmt --all -- --check` | 通过 |
| `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo check -p codewhale-tui` | 通过 |
| `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo test -p codewhale-tui commands::` | 通过：456 个命令测试 |
| `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo test -p codewhale-tui command_palette` | 通过：18 个测试 |
| `CARGO_TARGET_DIR=/tmp/codewhale-hunter-target cargo test -p codewhale-tui slash_completion` | 通过：17 个测试 |
| `git diff --check` | 通过 |
