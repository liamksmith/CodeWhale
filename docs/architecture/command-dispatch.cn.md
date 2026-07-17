# 命令调度架构

**目标分支：** `main`
**关联 EPIC：** [#2870](https://github.com/Hmbown/CodeWhale/issues/2870)
**关联 Issue：** [#2791](https://github.com/Hmbown/CodeWhale/issues/2791)
**EPIC-002（命令单一职责抽取）：** 第 4.x 层（FEAT-006 至 FEAT-008）

本文档记录了命令边界重放落地到 `main` 后的命令调度所有权模型，并已按 EPIC-002（命令单一职责抽取）更新。它反映了最终的分层所有权：顶层分组注册、分组归属的命令注册，以及命令级别的元数据和行为所有权。本文档是模块边界、调度优先级以及命令边界重构后保留的永久例外项的公开参考。

## 调度流程

`commands::execute()` 拥有斜杠命令调度入口。顺序是经过设计的：

| 步骤 | 来源 | 行为 |
|------|------|------|
| 0 | `$skill` 兼容 | `$name` 在斜杠解析之前被解析为 `/skill name`。 |
| 1 | 用户命令 | `user_registry::try_dispatch()` 首先检查工作空间和全局 markdown 命令，因此用户命令可以覆盖内置命令。 |
| 2 | 永久兼容别名 | `/jihua` 和 `/zidong` 通过配置模式调度路由；`/slop` 和 `/canzha` 直接调度到 `/debt`。所有这些都早于分组归属的注册表，并绕过内置 `CommandRegistry`。 |
| 3 | 内置注册表 | `CommandRegistry` 通过规范名称或别名解析分组归属的内置命令。 |
| 4 | 遗留迁移提示 | 已退役的命令（如 `/set` 和 `/deepseek`）返回针对性的替换指导。 |
| 5 | 技能回退 | 若无命令匹配，同名技能可能在显示"未知命令"建议之前运行。 |

## 模块边界

| 模块 | 职责 |
|------|------|
| `crates/tui/src/commands/mod.rs` | 中央调度入口、注册表初始化、公共命令查找辅助函数和未知命令建议。 |
| `crates/tui/src/commands/traits.rs` | 内置命令元数据、trait 支持的命令对象、命令分组和注册表查找。 |
| `crates/tui/src/commands/groups/` | 分组归属的内置命令区域。每个分组拥有其命令元数据和处理程序。 |
| `crates/tui/src/commands/user_registry.rs` | 用户命令注册表边界：markdown 元数据、别名、隐藏条目、验证错误、调度状态重置和覆盖行为。 |
| `crates/tui/src/commands/user_commands.rs` | 注册表使用的底层文件扫描、frontmatter 解析、allowed-tools 解析和模板替换。 |
| `crates/tui/src/tui/command_palette.rs` | 内置命令和可见用户命令的面板条目，用户命令覆盖内置命令。 |
| `crates/tui/src/tui/widgets/mod.rs` | 斜杠补全、用户命令元数据显示和别名覆盖行为。 |

## 内置命令分组

| 分组 | 范围 |
|------|------|
| `core` | 帮助、模型/provider 选择、队列、钩子、子智能体、链接、反馈、语音和核心导航。 |
| `config` | 配置、设置、状态界面、模式、主题、信任、登出及相关设置命令。 |
| `debug` | Token/成本自省、缓存、系统/上下文、diff/edit、撤销和重试。 |
| `memory` | 持久化记忆和笔记。 |
| `plugins` | 插件发现、列表和逐插件元数据详情显示。 |
| `project` | 项目初始化、分享、LSP 和 goal/hunt 命令。 |
| `session` | 重命名、保存、fork/new/load 会话、压缩、清理、中继和导出。 |
| `skills` | 技能列表、执行、审查和恢复。 |
| `utility` | 附件、任务/作业、MCP 和网络。 |

## 用户命令

用户命令是从以下位置按优先级顺序加载的 markdown 文件：

1. `<workspace>/.codewhale/commands/`
2. `<workspace>/.deepseek/commands/`
3. `<workspace>/.claude/commands/`
4. `<workspace>/.cursor/commands/`
5. `~/.codewhale/commands/`
6. `~/.deepseek/commands/`

支持的 frontmatter 字段：

| 字段 | 含义 |
|------|------|
| `description` | 工作目标和 UI 描述。 |
| `argument-hint` | 面板/补全中预期参数的提示。 |
| `allowed-tools` | 限制命令执行工具。显式空值阻止所有工具。 |
| `pausable` | 标记命令具有暂停/恢复能力。 |
| `alias` / `aliases` | 额外的用户命令名称，可以覆盖内置别名。 |
| `hidden` | 从面板/补全中隐藏命令，同时允许直接调度。 |

通过 `user_registry` 调度会在发送新命令体之前重置过期的命令状态：hunt 目标字段、token/时间计数器、继续计数、allowed tools、暂停状态、todos 和计划状态。

## 永久例外项

| 例外项 | 理由 |
|--------|------|
| `/jihua`、`/zidong`、`/slop`、`/canzha` | 早于分组归属注册表的向后兼容调度别名。`/jihua` 和 `/zidong` 通过配置模式调度路由；`/slop` 和 `/canzha` 直接调度到 `/debt`。 |
| `/set` 和 `/deepseek` 迁移提示 | 已退役命令，仅作为直接输入指导保留。已从注册表和自动补全中排除。 |
| 匹配分组模块中的 `#[allow(clippy::module_inception)]` | 分组目录有意包含同名子模块，如 `core/core.rs`。 |
| `user_commands.rs` 下层 | 注册表拥有运行时行为，而此模块保持为共享的文件系统和解析器层。 |
| `user_commands.rs` 中的 `#[cfg(test)]` 辅助函数 | 延迟测试迁移兼容性，同时注册表特定测试正在添加中。 |

## EPIC-002 完成状态（阶段 8 完成；准备提 PR）

EPIC-002（命令单一职责抽取）通过第 4.x 层子层为全部 9 个命令分组抽取了命令。第 4.2 层（FEAT-008）已完成，最终验证证据已记录。

| 层 | FEAT | 标题 | 状态 |
|---|---|---|---|
| 4 | FEAT-006 | Core、Config、Session 和 Debug 命令抽取 | 完成 |
| 4.1 | FEAT-007 | Project、Memory、Skills、Utility 和 Plugins 抽取 | 完成 |
| 4.2 | FEAT-008 | 注册表清理、文档和完整验证 | 完成 |

### 当前证据（草案——最终验证待定）

## 重放状态（EPIC-001）

FEAT-001 的分组归属内置命令方向在 `main` 上由更新的 trait 支持注册表和嵌套分组树表示。FEAT-002 重放为专用的用户命令注册表边界。FEAT-003 重放为公开架构和 PR/issue 证据文档，按当前 `main` 目标更新，而非旧的 `release/v0.8.60` 分支。
