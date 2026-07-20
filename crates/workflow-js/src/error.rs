//! 动态 Workflow 运行时的错误类型。

use thiserror::Error;

/// [`crate::WorkflowVm::run_script`] 暴露的错误。
///
/// 脚本可见的失败（抛出的 JS 异常、拒绝的 promise、脚本内部未捕获的主机函数错误）
/// 全部归为 [`WorkflowJsError::Script`]，包含异常消息和堆栈。
/// 其余变体描述了从未到达脚本的运行时级失败。
#[derive(Debug, Error)]
pub enum WorkflowJsError {
    /// 无法创建 QuickJS 运行时或上下文。
    #[error("failed to initialize the Workflow JS VM: {0}")]
    VmInit(String),
    /// 脚本抛出异常（或 promise 被拒绝）且未被捕获。
    /// 携带异常消息和堆栈（如果有）。
    #[error("script error: {0}")]
    Script(String),
    /// 运行被取消 — 调用者丢弃了 run future，或合作取消信号在脚本执行过程中触发。
    #[error("workflow run cancelled")]
    Cancelled,
    /// 脚本完成但其返回值无法编码为 JSON（例如返回了函数或循环对象）。
    #[error("script result is not JSON-encodable: {0}")]
    ResultEncoding(String),
    /// 调用参数无法注入到 VM 中。
    #[error("invalid workflow arguments: {0}")]
    InvalidArgs(String),
    /// 专用 VM 线程退出而未报告结果（panic 或 spawn 失败）。观察到此时会取消未完成的 driver 任务。
    #[error("Workflow VM thread terminated unexpectedly: {0}")]
    VmTerminated(String),
}

/// [`crate::WorkflowDriver`] 从 `spawn_task` 可能返回的错误。
///
/// 两种变体在脚本内部都表现为对应 `task()` 调用上抛出的异常，
/// 因此脚本可以 `try`/`catch` 单个拒绝（准入、深度、预算）而不会导致整个运行失败。
#[derive(Debug, Clone, Error)]
pub enum DriverError {
    /// driver 拒绝生成此任务（准入上限、深度上限、预算预留失败、无效的子代理类型等）。
    #[error("spawn rejected: {0}")]
    Rejected(String),
    /// driver 已消失或其通道已关闭；后续的 spawn 将无法工作。
    #[error("driver unavailable: {0}")]
    Unavailable(String),
}
