//! 子代理并发/超时限制及其钳位解析器。
//!
//! 纯数值/字符串限制常量以及两个仅对这些常量进行操作的私有钳位辅助函数。
//! 从 `config.rs` 逐字提取；常量通过 `pub use subagent_limits::*;` 重新导出（保留每个项的
//! `pub`/`pub(crate)` 可见性），解析器通过私有 `use` 拉回 `config.rs`，因此不会创建新的外部表面（#3311）。

/// 共享上下文切换使 agent 扇出变得廉价时的临时高吞吐量默认值。
/// 最终应由 API/背压预算而非内存驱动的计数节流来控制。
pub const DEFAULT_MAX_SUBAGENTS: usize = 64;
/// 用户可配置的并发子代理执行上限。保持此值高于默认值，以便操作员可以在完整的资源预算门控落地前，
/// 无需代码更改即可选择更大的 API 绑定扇出。
pub const MAX_SUBAGENTS: usize = 128;
/// 排队中 + 运行中的子代理准入上限。此值故意高于瞬时并发上限，以便 Workflow 风格的扇出
/// 可以选择大的有界种群，而无须无界队列增长。
pub const MAX_SUBAGENT_ADMISSION: usize = 1024;
/// 子代理请求的默认每步 DeepSeek API 超时时间（秒）。
/// 与旧版硬编码值匹配，以便当 `[subagents] api_timeout_secs` 未设置时，现有配置保持其旧行为（#1806, #1808）。
pub const DEFAULT_SUBAGENT_API_TIMEOUT_SECS: u64 = 120;
/// 最小接受的 `[subagents] api_timeout_secs`。任何更低的值（包括 `0`，否则会产生即时超时陷阱）
/// 在运行时看到之前都会钳位到此值。
pub const MIN_SUBAGENT_API_TIMEOUT_SECS: u64 = 1;
/// 最大接受的 `[subagents] api_timeout_secs`（30 分钟）。此上限防止配置错误的每步超时无限期掩盖真实的模型/网络挂起。
pub const MAX_SUBAGENT_API_TIMEOUT_SECS: u64 = 1800;
/// 在管理器可见的子代理无进展时，自动取消正在运行的子代理以释放其槽位的默认挂钟间隔（#2614）。
pub const DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS: u64 = 300;
/// 最小接受的 `[subagents] heartbeat_timeout_secs`。
pub const MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS: u64 = 30;
/// 最大接受的 `[subagents] heartbeat_timeout_secs`（1 小时）。
pub const MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS: u64 = 3600;
/// 默认每 SSE 块空闲超时时间（秒）。
pub const DEFAULT_STREAM_CHUNK_TIMEOUT_SECS: u64 = 900;
/// 最小接受的流块超时时间。
pub const MIN_STREAM_CHUNK_TIMEOUT_SECS: u64 = 1;
/// 最大接受的流块超时时间。
pub const MAX_STREAM_CHUNK_TIMEOUT_SECS: u64 = 3600;
pub(crate) const STREAM_CHUNK_TIMEOUT_ENV: &str = "DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS";

pub(crate) fn resolve_subagent_api_timeout_secs(raw: Option<u64>) -> u64 {
    let raw = raw.unwrap_or(DEFAULT_SUBAGENT_API_TIMEOUT_SECS);
    if raw == 0 {
        return DEFAULT_SUBAGENT_API_TIMEOUT_SECS;
    }
    raw.clamp(MIN_SUBAGENT_API_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS)
}

pub(crate) fn resolve_subagent_heartbeat_timeout_secs(
    raw: Option<u64>,
    api_timeout_secs: u64,
) -> u64 {
    let raw = raw.unwrap_or(DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS);
    let configured = if raw == 0 {
        DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS
    } else {
        raw.clamp(
            MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
            MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
        )
    };
    let min_for_api = api_timeout_secs.saturating_add(30).clamp(
        MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
        MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS,
    );
    configured.max(min_for_api)
}
