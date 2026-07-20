//! 沙箱化 VM 与子代理引擎之间的驱动层接缝。
//!
//! QuickJS VM 运行在专用线程上，其 `'js` 值永远不能跨越 `.await` 到另一个线程，
//! 因此离开 VM 的所有内容都是纯 `Send` 数据：[`TaskRequest`] 发出，
//! [`TaskCompletion`] 通过 oneshot 返回。[`WorkflowDriver`] trait 是主机端合约，
//! tui 布线通过 `SubAgentManager` 实现（在那里 spawn 即 fire-and-forget；
//! 驱动器的完成泵从邮箱中解析由 `agent_id` 键控的 `Completed` 信号，
//! 然后通过 `get_result` 读取完整的未截断文本）。测试使用
//! [`crate::testing::FakeDriver`] 实现。
//!
//! 预算所有权：令牌记账和 §5.3 保留语义完全在驱动器端（管理器的预算范围）。
//! VM 只读取 [`BudgetSnapshot`]——它在生成前执行快速失败的 `spent >= total` 检查，
//! 并将数字以 `budget.*` 暴露给 JS，但从不自行保留或扣减令牌。
//! 批准生成的驱动器是权威；其拒绝表现为对应 `task()` 调用上的 JS throw。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::DriverError;

/// 一次 `task()` 调用，在 VM 端完全解析和验证。
///
/// 字段语义镜像 `agent` 工具的 spawn 选项。
///
/// 步骤标识为 fleet `role`（首选）和/或 `profile`（#4177）。两个令牌都使用与
/// `crates/workflow` 叶子 profile 相同的规则规范化为（修剪 + 小写）。
/// 名单成员资格由驱动器（tui）在 spawn 时解析——此 crate 从未看到保存的 Fleet 名单。
/// Provider/model 仍然是可选覆盖，不是必需的身份字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRequest {
    /// 子提示（JS `prompt`，回退到 `description`；必需）。
    pub description: String,
    /// 子代理类型（JS `subagentType` 或 `type`）；`None` 让驱动器
    /// 应用其默认值（`general`）。
    pub subagent_type: Option<String>,
    /// Fleet 角色名称（JS `role`），例如 `scout` / `implementer`（#4177）。
    pub role: Option<String>,
    /// Fleet profile 令牌，已规范化（修剪、小写）并已验证。
    /// 在 spawn 时显式 profile 优先于角色映射。
    pub profile: Option<String>,
    /// 显式模型覆盖；始终优先于 `model_strength`。
    pub model: Option<String>,
    /// 相对模型强度（`same`/`faster`，以及驱动器端别名）。
    pub model_strength: Option<String>,
    /// 推理努力度（`inherit`/`off`/`low`/`medium`/`high`/`max`）。
    pub thinking: Option<String>,
    /// 在新的 git worktree 中运行子任务以进行并行编辑。
    pub worktree: bool,
    /// 显式工具允许列表；`custom` 角色需要驱动器提供。
    pub allowed_tools: Option<Vec<String>>,
    /// 每次调用的 spawn 深度覆盖（驱动器限制在其上限内）。
    pub max_depth: Option<u32>,
    /// 显式令牌预算：在驱动器端分叉一个隔离池。
    /// 省略则使子任务继承（并扣减）共享运行池。
    pub token_budget: Option<u64>,
    /// 回复必须满足的 JSON schema；在驱动器返回原始文本后在 VM 中验证
    /// （解码规则参见 [`crate`] 文档）。
    pub response_schema: Option<serde_json::Value>,
    /// 用于进度展示的简短人类可读标签。
    pub label: Option<String>,
    /// 此任务所属的阶段名称，用于进度分组。
    pub phase: Option<String>,
}

/// 一个已生成任务的最终结果，通过完成 oneshot 传递。
/// 除 `Completed` 之外的所有内容都变为等待 `task()` 调用上的 JS throw。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCompletion {
    /// 子任务已完成；`text` 是完整的未截断结果。
    Completed { text: String },
    /// 子任务失败（错误结果、超时等）。
    Failed { message: String },
    /// 子任务已被取消（级联或显式）。
    Cancelled,
    /// 子任务的预算范围在运行中耗尽。
    BudgetExhausted { message: String },
}

/// 一个成功受理的 spawn：驱动器分配的任务 ID（引擎的 `agent_id`）
/// 加上驱动器在完成时解析的 oneshot。
///
/// 丢弃接收器不得阻塞驱动器；驱动器应将已关闭的完成通道视为
/// "没有人在监听"并继续处理。
#[derive(Debug)]
pub struct SpawnedTask {
    /// 驱动器分配的 ID，在运行内唯一（引擎 `agent_id`）。
    pub task_id: String,
    /// 仅解析一次，带有最终的 [`TaskCompletion`]。
    pub completion: oneshot::Receiver<TaskCompletion>,
}

/// 运行共享令牌池的实时视图，由驱动器拥有。
///
/// `total == None` 表示未配置上限；JS 随后看到
/// `budget.total === null` 且 `budget.remaining() === Infinity`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetSnapshot {
    /// 池上限（以令牌为单位），如果有配置的话。
    pub total: Option<u64>,
    /// 池中已花费的令牌（加上驱动器端预留）。
    pub spent: u64,
}

impl BudgetSnapshot {
    /// 到达上限前剩余的令牌数；池无限制时返回 `None`。
    pub fn remaining(&self) -> Option<u64> {
        self.total.map(|total| total.saturating_sub(self.spent))
    }

    /// 当池有上限且已完全消耗时返回 true。
    pub fn exhausted(&self) -> bool {
        matches!(self.total, Some(total) if self.spent >= total)
    }
}

/// 脚本发出的进度事件（`log(..)` / `phase(..)`），
/// 同步且按脚本顺序传递给驱动器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// `log(msg)` — UI 的叙述行。
    Log {
        /// 字符串化后的消息。
        message: String,
    },
    /// `phase(title)` — 脚本进入了一个命名的阶段。
    Phase {
        /// 阶段标题。
        title: String,
    },
    /// 一个已完成的子任务返回了未能通过调用方 `responseSchema` 的文本。
    /// VM 在将验证错误抛回脚本之前发出此事件，以便主机端回执可以将该叶子标记为
    /// 失败，而不是报告一个成功的子任务旁边有一个 `null` 结果。
    TaskSchemaValidationFailed {
        /// 驱动器分配的任务 ID（引擎 `agent_id`）。
        task_id: String,
        /// 已传递给 JS 的验证错误。
        message: String,
    },
}

/// Workflow 运行的主机端执行器。
///
/// 实现必须能够从 VM 线程廉价调用：`spawn_task` 受理任务并立即返回
/// （fire-and-forget spawn——永远不要内联等待子任务），而 `budget`、
/// `progress` 和 `cancel_all` 是同步的。`cancel_all` 必须是幂等的；
/// 它在脚本出错时、运行 future 被丢弃时被调用，且多调用一次也无妨。
#[async_trait]
pub trait WorkflowDriver: Send + Sync {
    /// 受理并启动一个任务。错误表现为对应 `task()` 调用上的 JS throw。
    async fn spawn_task(&self, request: TaskRequest) -> Result<SpawnedTask, DriverError>;

    /// 取消属于此运行的所有进行中的任务。幂等。
    fn cancel_all(&self);

    /// 运行共享令牌池的当前快照。
    fn budget(&self) -> BudgetSnapshot;

    /// 接收脚本进度事件（有序，同步）。
    fn progress(&self, event: ProgressEvent);
}

/// 规范化并验证 Fleet profile 令牌：修剪、小写，然后应用与
/// `crates/workflow` 的 `validate_leaf_profile` 相同的令牌规则——
/// 非空、无空白字符，且不含 `"`、`'`、`` ` ``、`=`。
pub fn normalize_profile(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().to_lowercase();
    let invalid = normalized.is_empty()
        || normalized
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '='));
    if invalid {
        return Err(format!(
            "invalid profile token {raw:?}: profiles must be non-empty and contain no whitespace, quotes, backticks, or '='"
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_profile_trims_and_lowercases() {
        assert_eq!(normalize_profile("  ALpha-1  ").unwrap(), "alpha-1");
    }

    #[test]
    fn normalize_profile_rejects_bad_tokens() {
        for bad in ["", "   ", "two words", "a=b", "a\"b", "a'b", "a`b"] {
            assert!(
                normalize_profile(bad).is_err(),
                "expected rejection: {bad:?}"
            );
        }
    }

    #[test]
    fn budget_snapshot_math() {
        let unbounded = BudgetSnapshot {
            total: None,
            spent: 10,
        };
        assert_eq!(unbounded.remaining(), None);
        assert!(!unbounded.exhausted());

        let pool = BudgetSnapshot {
            total: Some(100),
            spent: 40,
        };
        assert_eq!(pool.remaining(), Some(60));
        assert!(!pool.exhausted());

        let drained = BudgetSnapshot {
            total: Some(100),
            spent: 120,
        };
        assert_eq!(drained.remaining(), Some(0));
        assert!(drained.exhausted());
    }
}
