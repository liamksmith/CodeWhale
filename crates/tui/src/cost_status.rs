//! 进程级成本累加侧信道 (#526)。
//!
//! 主回合完成路径之外的背景 LLM 调用
//!（压缩摘要、接缝重新压缩）
//! 过去会丢弃其令牌使用量——仪表板的
//! 会话成本只看到父回合的令牌，因此长时间
//! 触发压缩的会话会低估背景调用消耗的令牌成本。
//!
//! 镜像了 [`crate::retry_status`] 模式：背景调用者在每次
//! `client.create_message` 后调用 [`report`]，TUI
//! 渲染循环每帧调用 [`drain`]，任何排出的金额
//! 都会被纳入 `App::accrue_subagent_cost_estimate`。
//!
//! 为什么使用侧信道而不是管道回调：泄漏的调用者
//!（`compaction.rs`、`seam_manager.rs`）是
//! 引擎内部机制，没有直接操作 `App` 或
//! 引擎事件通道的句柄。侧信道使变更面
//! 极小——每个调用点只需新增一个 `report` 行——并且任何
//! 未来的背景调用者（摘要器、检索助手）都
//! 无需额外管道即可自动累加成本。

use std::sync::{Mutex, OnceLock};

use crate::models::Usage;
use crate::pricing::CostEstimate;

static PENDING: OnceLock<Mutex<CostEstimate>> = OnceLock::new();

fn cell() -> &'static Mutex<CostEstimate> {
    PENDING.get_or_init(|| Mutex::new(CostEstimate::default()))
}

/// 背景调用者在此报告其 LLM 使用量。通过
/// [`crate::pricing::calculate_turn_cost_estimate_from_usage`] 计算成本并
/// 添加到待处理池中。开销很小；持有一个短生命周期的锁后
/// 返回。对于定价表未知的模型不执行任何操作。
pub fn report(model: &str, usage: &Usage) {
    let Some(cost) = crate::pricing::calculate_turn_cost_estimate_from_usage(model, usage) else {
        return;
    };
    if !cost.is_positive() {
        return;
    }
    // 从中毒的锁中恢复——前一个持有者 panic 了，但
    // 累积的数据仍然有效。
    let mut pending = cell().lock().unwrap_or_else(|e| e.into_inner());
    pending.usd += cost.usd;
    pending.cny += cost.cny;
}

/// 排出待处理的成本。返回累积的金额并将
/// 池重置为零。由 TUI 渲染/事件循环每帧调用；
/// 任何非零结果都会被纳入 `accrue_subagent_cost_estimate`。
pub fn drain() -> CostEstimate {
    // 从中毒的锁中恢复——前一个持有者 panic 了，但
    // 累积的数据仍然有效。
    let mut pending = cell().lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *pending)
}

/// 将池重置为零而不消耗。测试专用的辅助函数，
/// 供那些共享静态变量且需要从已知状态开始的测试套件使用。
/// 生产代码应始终使用 [`drain`]。
#[cfg(test)]
pub fn reset_for_tests() {
    let mut pending = cell().lock().unwrap_or_else(|e| e.into_inner());
    *pending = CostEstimate::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_usage() -> Usage {
        Usage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Default::default()
        }
    }

    /// 测试并行运行并共享静态变量——通过此互斥锁序列化
    /// 那些访问池的测试，使并发的 `report`/`drain` 不会导致断言竞态。
    fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn report_adds_to_pool_and_drain_returns_then_resets() {
        let _g = serial_lock();
        reset_for_tests();
        report("deepseek-v4-flash", &small_usage());
        let first = drain();
        assert!(first.usd > 0.0, "expected positive USD cost, got {first:?}");
        assert!(first.cny > 0.0, "expected positive CNY cost, got {first:?}");
        let second = drain();
        assert_eq!(second, CostEstimate::default(), "drain must zero the pool");
    }

    #[test]
    fn report_skips_unknown_models() {
        let _g = serial_lock();
        reset_for_tests();
        // NIM 托管的模型有意没有 DeepSeek 定价。
        report("deepseek-ai/deepseek-v4-pro", &small_usage());
        assert_eq!(drain(), CostEstimate::default());
    }

    #[test]
    fn report_accumulates_across_multiple_calls() {
        let _g = serial_lock();
        reset_for_tests();
        report("deepseek-v4-flash", &small_usage());
        report("deepseek-v4-flash", &small_usage());
        let total = drain();
        // 两次相同的报告——总金额必须是单次报告的 2 倍。
        let single = crate::pricing::calculate_turn_cost_estimate_from_usage(
            "deepseek-v4-flash",
            &small_usage(),
        )
        .unwrap();
        assert!((total.usd - 2.0 * single.usd).abs() < 1e-12);
        assert!((total.cny - 2.0 * single.cny).abs() < 1e-12);
    }
}
