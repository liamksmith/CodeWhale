//! 轻量级启动里程碑追踪（#3757）。
//!
//! 记录命名的里程碑相对于单个进程启动时刻的时间，并在 TUI 进入事件循环时向运行时日志输出一行摘要。
//! 里程碑在内存中缓冲，因为大多数里程碑发生在运行时日志初始化之前；摘要是产物，而非事件本身。

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static MILESTONES: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

/// 固定进程启动时刻。首次调用有效；后续调用为空操作，以便测试和替代入口点不会扭曲时间线。
pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

/// 记录当前自进程启动以来的耗时，并关联到指定的 `label`。
/// 如果从未调用过 [`mark_process_start`]，则为空操作（例如非交互式子命令）。
pub fn mark(label: &'static str) {
    let Some(start) = PROCESS_START.get() else {
        return;
    };
    let elapsed_ms = start.elapsed().as_millis() as u64;
    if let Ok(mut milestones) = MILESTONES.lock() {
        milestones.push((label, elapsed_ms));
    }
}

/// 将缓冲的里程碑作为一行摘要输出并清空缓冲区。
/// 在运行时日志就绪后调用（恰好在事件循环启动之前）。
pub fn log_summary() {
    let Some(start) = PROCESS_START.get() else {
        return;
    };
    let total_ms = start.elapsed().as_millis() as u64;
    let Ok(mut milestones) = MILESTONES.lock() else {
        return;
    };
    let line = milestones
        .iter()
        .map(|(label, ms)| format!("{label}={ms}ms"))
        .collect::<Vec<_>>()
        .join(" ");
    milestones.clear();
    tracing::info!(target: "startup", "startup {line} event_loop={total_ms}ms");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestones_accumulate_and_summary_drains() {
        mark_process_start();
        mark("alpha");
        mark("beta");
        {
            let milestones = MILESTONES.lock().unwrap();
            let labels: Vec<&str> = milestones.iter().map(|(l, _)| *l).collect();
            assert!(labels.contains(&"alpha"));
            assert!(labels.contains(&"beta"));
        }
        log_summary();
        assert!(MILESTONES.lock().unwrap().is_empty());
    }
}
