//! 进程级重试状态面板 (#499)。
//!
//! `client::send_with_retry` 中的 HTTP 重试路径已经计时等待并知道错误类别。
//! 此模块为 TUI 提供观察该状态的方式——`start`、`succeeded` 和 `failed`
//! 切换一个全局的 `RetryState`，页脚/状态面板每帧读取。
//!
//! 为什么是进程级全局变量：面向用户的 TUI 每个进程运行一个引擎，
//! 我们想要展示的唯一重试状态是用户正在关注的那个。
//! 后台任务中的子代理重试有意**不**点亮前台横幅——它们本应是不可见的。
//! 如果未来某个功能需要按引擎区分重试面板，将其替换为 `EngineHandle`
//! 上携带的 `Arc<RwLock<...>>`；公开 API 保持不变。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 一次进行中的重试尝试。`deadline` 是下一次请求将发起的挂钟时间——
/// UI 从中减去 `Instant::now()` 来渲染实时倒计时。
#[derive(Debug, Clone)]
pub struct RetryBanner {
    /// 从 1 开始的重试尝试编号（第一次重试为 1）。
    pub attempt: u32,
    /// 下一次请求将发送的时间。
    pub deadline: Instant,
    /// 简短的可读原因（"频率受限"、"服务器错误"等）。
    pub reason: String,
}

/// 供 UI 渲染的重试状态快照。
#[derive(Debug, Clone, Default)]
pub enum RetryState {
    /// 无进行中的重试。横幅隐藏。
    #[default]
    Idle,
    /// 请求在重试前等待。显示倒计时横幅。
    Active(RetryBanner),
    /// 所有重试已耗尽；显示失败行直到下一个轮次开始。
    /// `since` 记录行被设置的时间，以便未来的优化可以自动将其老化；
    /// 目前引擎在 `TurnStarted` 时清除它。
    Failed {
        reason: String,
        #[allow(dead_code)]
        since: Instant,
    },
}

impl RetryState {
    /// 活动横幅上的挂钟秒数剩余，如果非活动则返回 `None`。
    /// 饱和到零——渲染器应将任何负剩余视为"正在发起"。
    #[must_use]
    pub fn seconds_remaining(&self) -> Option<u64> {
        match self {
            Self::Active(banner) => Some(
                banner
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs(),
            ),
            _ => None,
        }
    }

    /// 失败行是否仍应显示。镜像 issue 规范中的"直到下一个轮次"规则；
    /// 引擎通过 `TurnStarted` 上的 [`clear`] 显式清除它。
    #[cfg(test)]
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// 在首次读取时懒初始化 cell，这样调用者不必在启动时初始化进程级状态。
fn cell() -> &'static Mutex<RetryState> {
    static STATE: OnceLock<Mutex<RetryState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RetryState::Idle))
}

fn rate_limit_cell() -> &'static Mutex<Option<Instant>> {
    static STATE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// 供渲染器使用的公开只读快照。
#[must_use]
pub fn snapshot() -> RetryState {
    cell().lock().map(|s| s.clone()).unwrap_or(RetryState::Idle)
}

/// 扩展供应商级频率限制暂停窗口。这与页脚横幅分开，
/// 以便一个成功的并发请求不会清除另一个请求的活动 `Retry-After` 窗口。
pub fn note_rate_limit(delay: Duration) {
    let deadline = Instant::now() + delay;
    if let Ok(mut current) = rate_limit_cell().lock()
        && current.is_none_or(|existing| existing < deadline)
    {
        *current = Some(deadline);
    }
}

/// 供应商级频率限制暂停的剩余时间（如果有）。
#[must_use]
pub fn rate_limit_remaining() -> Option<Duration> {
    let now = Instant::now();
    let mut current = rate_limit_cell().lock().ok()?;
    match *current {
        Some(deadline) if deadline > now => Some(deadline.duration_since(now)),
        Some(_) => {
            *current = None;
            None
        }
        None => None,
    }
}

/// 标记进行中的重试。`attempt` 是*即将到来*的重试编号（第一次为 1）；
/// `delay` 是客户端在发起前将休眠的时间。
pub fn start(attempt: u32, delay: Duration, reason: impl Into<String>) {
    let banner = RetryBanner {
        attempt,
        deadline: Instant::now() + delay,
        reason: reason.into(),
    };
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Active(banner);
    }
}

/// 标记重试链已成功。隐藏横幅。
pub fn succeeded() {
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Idle;
    }
}

/// 标记重试链已耗尽重试次数。渲染器保留失败行直到 [`clear`]（通常在 `TurnStarted` 时调用）。
pub fn failed(reason: impl Into<String>) {
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Failed {
            reason: reason.into(),
            since: Instant::now(),
        };
    }
}

/// 重置为空闲。在 `TurnStarted` 时调用，这样上一个轮次的失败行不会渗入下一个轮次。
pub fn clear() {
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Idle;
    }
}

#[cfg(test)]
pub fn clear_rate_limit() {
    if let Ok(mut current) = rate_limit_cell().lock() {
        *current = None;
    }
}

/// 测试辅助函数：序列化触及全局状态的测试，这样 cargo 的并行运行器
/// 无法观察到撕裂的读取。该守卫被导出，以便*其他*模块中的测试
/// （例如页脚渲染测试）可以持有与 `retry_status::tests` 中相同的锁。
#[cfg(test)]
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: Mutex<()> = Mutex::new(());
    GUARD.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 从 [`super::test_guard`] 获取跨模块测试守卫，并在让出给测试体之前将状态重置为 `Idle`。
    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let g = test_guard();
        clear();
        clear_rate_limit();
        g
    }

    #[test]
    fn idle_by_default_after_clear() {
        let _g = setup();
        assert!(matches!(snapshot(), RetryState::Idle));
        assert_eq!(snapshot().seconds_remaining(), None);
    }

    #[test]
    fn start_then_succeeded_returns_to_idle() {
        let _g = setup();
        start(1, Duration::from_secs(5), "rate limited");
        let s = snapshot();
        assert!(matches!(s, RetryState::Active(_)));
        let remaining = s.seconds_remaining().unwrap();
        assert!(remaining <= 5, "{remaining}");
        succeeded();
        assert!(matches!(snapshot(), RetryState::Idle));
    }

    #[test]
    fn failed_persists_until_clear() {
        let _g = setup();
        failed("upstream 500");
        let s = snapshot();
        assert!(s.is_failed());
        if let RetryState::Failed { reason, .. } = s {
            assert_eq!(reason, "upstream 500");
        } else {
            panic!("expected Failed");
        }
        clear();
        assert!(matches!(snapshot(), RetryState::Idle));
    }

    #[test]
    fn deadline_in_past_yields_zero_remaining() {
        let _g = setup();
        // 绕过 `start` 以便我们设置一个已经过去的截止时间。
        if let Ok(mut s) = cell().lock() {
            *s = RetryState::Active(RetryBanner {
                attempt: 2,
                deadline: Instant::now() - Duration::from_secs(1),
                reason: "test".into(),
            });
        }
        assert_eq!(snapshot().seconds_remaining(), Some(0));
        clear();
    }

    #[test]
    fn rate_limit_deadline_survives_banner_clear() {
        let _g = setup();
        note_rate_limit(Duration::from_secs(5));
        start(1, Duration::from_secs(5), "rate limited");
        succeeded();
        assert!(
            rate_limit_remaining().is_some(),
            "供应商级频率限制暂停不得被无关的成功操作清除"
        );
        clear_rate_limit();
    }
}
