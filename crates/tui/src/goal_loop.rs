//! 目标循环编排器——持久目标控制层（#3215，及其演进 #891 / #1976 / #2058 / #2029）。
//!
//! 这是**工作流目标层**：将一次性 `/goal` 转变为持久工作循环的决策核心。
//! 给定持久目标状态、累计用量（来自 `crates/state` `record_thread_goal_usage` 中
//! 按目标核算的统计）和预算，它决定是**继续**（重新分派另一个工作者轮次
//! 朝向目标）还是以终结状态**停止**。它是 Workflow≈ultracode 映射中的编排器——
//! 将工作分发给工作者（`worker_profile`）并在提交前进行验证的循环。
//!
//! 范围：**决策逻辑 + 类型**。引擎（`core/engine.rs`）在每个轮次后读取
//! `SharedGoalState` 快照并调用 `decide_continuation` 来决定是否重新分派。
//! **没有连续次数的上限**——目标运行直到模型自我报告完成/阻塞、用户暂停或清除、
//! 或可选的令牌/时间预算耗尽。这与持久目标的预期一致："直到完成"，而非"直到 N 轮"。

/// 持久目标的终态或活跃状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalRunStatus {
    /// 仍在朝着目标工作。
    Active,
    /// 目标已达成（模型自我报告完成，且理想情况下验证器已确认——参见 `GoalGate`）。
    Completed,
    /// 模型报告被阻塞，需要用户介入。
    #[allow(dead_code)]
    Blocked,
}

/// 循环停止的原因，用于终态决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Objective achieved.
    Completed,
    /// Model reported blocked.
    #[allow(dead_code)]
    Blocked,
    /// 令牌预算耗尽。
    TokenBudget,
    /// 挂钟时间预算耗尽。
    TimeBudget,
    /// 连续性断路器触发（太多连续轮次没有终态信号）。
    /// 为 API 完整性保留；当前循环没有连续次数上限，
    /// 因此 `decide_continuation` 不会构造此变体。
    #[allow(dead_code)]
    ContinuationLimit,
}

/// 目标运行的累计持久进度。镜像 `crates/state` `record_thread_goal_usage`
/// 连接的字段（tokens_used / time_used_seconds）加上循环维护的连续次数计数器。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoalProgress {
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuations: u32,
}

/// 目标运行的边界。`None` 字段表示无限制。**没有连续次数上限**——
/// 循环运行直到模型自我报告完成/阻塞、用户暂停/清除、或可选预算耗尽。
/// 这是有意为之：目标是"直到完成"，而非"直到 N 轮"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalBudget {
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<u64>,
}

impl GoalBudget {
    /// 完全无限制——没有令牌或时间上限。唯一停止条件是终态模型状态
    /// （完成/阻塞）或显式用户暂停/清除。
    #[allow(dead_code)]
    pub const fn unbounded() -> Self {
        Self {
            token_budget: None,
            time_budget_seconds: None,
        }
    }

    /// 仅令牌预算——循环运行直到模型完成或令牌预算耗尽。
    #[allow(dead_code)]
    pub const fn with_token_budget(token_budget: u64) -> Self {
        Self {
            token_budget: Some(token_budget),
            time_budget_seconds: None,
        }
    }
}

/// 循环在每个工作者轮次后做出的决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationDecision {
    /// 重新分派另一个朝向目标的轮次。
    Continue,
    /// 停止；目标运行已终态。
    Stop(StopReason),
}

/// 决定持久目标运行是否应在轮次后继续。
///
/// 优先级（从高到低）：
/// 1. 终态模型状态（Completed / Blocked）结束运行。
/// 2. 可选令牌或时间预算（如果耗尽）结束运行。
/// 3. 否则继续。
///
/// **没有连续次数上限**。目标运行直到模型报告完成/阻塞、用户暂停或清除、
/// 或可选预算耗尽。
#[must_use]
pub fn decide_continuation(
    status: GoalRunStatus,
    progress: GoalProgress,
    budget: GoalBudget,
) -> ContinuationDecision {
    // 1. 终态模型信号优先。
    match status {
        GoalRunStatus::Completed => return ContinuationDecision::Stop(StopReason::Completed),
        GoalRunStatus::Blocked => return ContinuationDecision::Stop(StopReason::Blocked),
        GoalRunStatus::Active => {}
    }

    // 2. 可选预算。无连续次数上限——"直到完成"。
    if let Some(tokens) = budget.token_budget
        && progress.tokens_used >= tokens
    {
        return ContinuationDecision::Stop(StopReason::TokenBudget);
    }
    if let Some(secs) = budget.time_budget_seconds
        && progress.time_used_seconds >= secs
    {
        return ContinuationDecision::Stop(StopReason::TimeBudget);
    }

    // 3. 继续运行。
    ContinuationDecision::Continue
}

/// 停止原因是代表成功（Completed）还是提前/强制退出。
/// 对于 UI/状态投影很有用（#2666 令牌/时间可见性）。
#[must_use]
#[allow(dead_code)]
pub fn is_success(reason: StopReason) -> bool {
    matches!(reason, StopReason::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_status_stops_with_success() {
        let d = decide_continuation(
            GoalRunStatus::Completed,
            GoalProgress::default(),
            GoalBudget::unbounded(),
        );
        assert_eq!(d, ContinuationDecision::Stop(StopReason::Completed));
        assert!(is_success(StopReason::Completed));
    }

    #[test]
    fn blocked_status_stops_without_success() {
        let d = decide_continuation(
            GoalRunStatus::Blocked,
            GoalProgress::default(),
            GoalBudget::unbounded(),
        );
        assert_eq!(d, ContinuationDecision::Stop(StopReason::Blocked));
        assert!(!is_success(StopReason::Blocked));
    }

    #[test]
    fn active_under_budget_continues() {
        let progress = GoalProgress {
            tokens_used: 10,
            time_used_seconds: 5,
            continuations: 2,
        };
        let budget = GoalBudget {
            token_budget: Some(1000),
            time_budget_seconds: Some(600),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn active_with_no_budget_continues_indefinitely() {
        // 无连续次数上限：高连续次数且没有令牌/时间预算时仍必须继续。
        // 循环是"直到完成"，而非"直到 N 次"。
        let progress = GoalProgress {
            continuations: 1_000_000,
            ..GoalProgress::default()
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, GoalBudget::unbounded()),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn token_budget_exhaustion_stops() {
        let progress = GoalProgress {
            tokens_used: 1000,
            continuations: 1,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::with_token_budget(1000);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::TokenBudget)
        );
    }

    #[test]
    fn time_budget_exhaustion_stops() {
        let progress = GoalProgress {
            time_used_seconds: 601,
            continuations: 1,
            ..GoalProgress::default()
        };
        let budget = GoalBudget {
            token_budget: None,
            time_budget_seconds: Some(600),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::TimeBudget)
        );
    }

    #[test]
    fn terminal_status_outranks_remaining_budget() {
        // 即使剩余大量预算，Completed 也优先。
        let progress = GoalProgress::default();
        let budget = GoalBudget {
            token_budget: Some(1_000_000),
            time_budget_seconds: Some(86_400),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Completed, progress, budget),
            ContinuationDecision::Stop(StopReason::Completed)
        );
    }
}
