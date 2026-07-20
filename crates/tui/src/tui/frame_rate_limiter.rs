//! TUI 渲染循环的 120 FPS 绘制速率上限。
//!
//! 改编自
//! [`codex-rs/tui/src/tui/frame_rate_limiter.rs`](https://github.com/openai/codex)
//! — 意图相同，但由于我们的渲染循环基于轮询而非调度器，因此实现更简单。
//! 我们只需要限制 `terminal.draw` 调用的最小间隔；现有的 `needs_redraw`
//! 标志已经会在多次轮询之间发生多次事件时，将多次状态变更合并为一次绘制。
//!
//! ## 为什么需要这个
//!
//! 当模型流式输出较长的助手响应时，每个 SSE 数据块都会触发
//! `App.needs_redraw = true`。如果没有上限，主循环会在每个数据块
//! 到达时愉快地重绘整个屏幕——有时在几百毫秒的流式输出期间超过 300 帧/秒。
//! 用户无法感知超过 ~60-120 FPS 的帧率，而 ratatui 的差异刷新是有实际
//! 开销的（换行、样式、crossterm `queue!`），因此这完全是浪费。
//!
//! ## 行为
//!
//! - 默认状态：从不限制。
//! - 调用 `mark_emitted(t)` 之后，后续的 `clamp_deadline(t')`
//!   会返回 `max(t', t + MIN_FRAME_INTERVAL)`。
//! - 渲染循环调用 `clamp_deadline(now)` 并：
//!   - 如果结果 == `now`，则可以立即绘制。
//!   - 如果结果 > `now`，则循环应休眠/缩短轮询超时，以便在
//!     该确切时刻唤醒。
//!
//! 集成点参见 `crates/tui/src/tui/ui.rs`（`run_app`）。

use std::time::Duration;
use std::time::Instant;

/// 120 FPS 最小帧间隔（≈8.33ms）。
pub const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(8_333_334);

/// 30 FPS 最小帧间隔（≈33.33ms），在低动态模式下使用。
pub const LOW_MOTION_MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(33_333_333);

/// 记录最近一次发出的绘制时间，允许将截止时间向前限制，
/// 使下一次绘制不会在上一次绘制后 `MIN_FRAME_INTERVAL`
/// 之内发生。
#[derive(Debug, Default)]
pub struct FrameRateLimiter {
    last_emitted_at: Option<Instant>,
    /// 为 true 时使用 30 FPS 上限替代 120 FPS。
    low_motion: bool,
}

impl FrameRateLimiter {
    /// 返回 `requested`，如果超出最大帧率则向前限制。
    #[must_use]
    pub fn clamp_deadline(&self, requested: Instant) -> Instant {
        let Some(last_emitted_at) = self.last_emitted_at else {
            return requested;
        };
        let min_allowed = last_emitted_at
            .checked_add(self.interval())
            .unwrap_or(last_emitted_at);
        requested.max(min_allowed)
    }

    /// 记录在 `emitted_at` 时刻已发出一次绘制。
    pub fn mark_emitted(&mut self, emitted_at: Instant) {
        self.last_emitted_at = Some(emitted_at);
    }

    /// 如果下一次绘制必须从 `now` 等待 `d` 则返回 `Some(d)`。
    /// 如果允许立即绘制则返回 `None`。
    /// 渲染循环使用该值缩短轮询超时，以便在允许绘制时恰好唤醒。
    #[must_use]
    pub fn time_until_next_draw(&self, now: Instant) -> Option<Duration> {
        let clamped = self.clamp_deadline(now);
        if clamped <= now {
            None
        } else {
            Some(clamped - now)
        }
    }

    /// 设置低动态模式：将帧率上限设为 30 FPS 而非 120 FPS。
    pub fn set_low_motion(&mut self, low_motion: bool) {
        self.low_motion = low_motion;
    }

    fn interval(&self) -> Duration {
        if self.low_motion {
            LOW_MOTION_MIN_FRAME_INTERVAL
        } else {
            MIN_FRAME_INTERVAL
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_does_not_clamp() {
        let t0 = Instant::now();
        let limiter = FrameRateLimiter::default();
        assert_eq!(limiter.clamp_deadline(t0), t0);
        assert!(limiter.time_until_next_draw(t0).is_none());
    }

    #[test]
    fn clamps_to_min_interval_since_last_emit() {
        let t0 = Instant::now();
        let mut limiter = FrameRateLimiter::default();

        assert_eq!(limiter.clamp_deadline(t0), t0);
        limiter.mark_emitted(t0);

        let too_soon = t0 + Duration::from_millis(1);
        assert_eq!(limiter.clamp_deadline(too_soon), t0 + MIN_FRAME_INTERVAL);
    }

    #[test]
    fn time_until_next_draw_reports_remaining_window() {
        let t0 = Instant::now();
        let mut limiter = FrameRateLimiter::default();
        limiter.mark_emitted(t0);

        let after_4ms = t0 + Duration::from_millis(4);
        let remaining = limiter.time_until_next_draw(after_4ms).unwrap();
        // ≈ 4.33ms 剩余（8.33 - 4）
        assert!(
            remaining > Duration::from_micros(4_000) && remaining < Duration::from_millis(5),
            "expected ~4.33ms, got {remaining:?}"
        );
    }

    #[test]
    fn time_until_next_draw_none_after_interval_elapsed() {
        let t0 = Instant::now();
        let mut limiter = FrameRateLimiter::default();
        limiter.mark_emitted(t0);

        let well_past = t0 + Duration::from_millis(50);
        assert!(limiter.time_until_next_draw(well_past).is_none());
    }

    #[test]
    fn low_motion_clamps_to_30fps_interval() {
        let t0 = Instant::now();
        let mut limiter = FrameRateLimiter::default();
        limiter.set_low_motion(true);
        limiter.mark_emitted(t0);

        let too_soon = t0 + Duration::from_millis(5);
        // 在 30 FPS（~33.33 ms）下，上次绘制后 5 ms 的绘制请求会被限制。
        assert_eq!(
            limiter.clamp_deadline(too_soon),
            t0 + LOW_MOTION_MIN_FRAME_INTERVAL
        );

        // 34 ms 后允许绘制。
        let after_34 = t0 + Duration::from_millis(34);
        assert!(limiter.time_until_next_draw(after_34).is_none());
    }

    #[test]
    fn low_motion_switching_respects_current_mode() {
        let t0 = Instant::now();
        let mut limiter = FrameRateLimiter::default();

        // 默认（120 FPS）：在 t0 标记，10 ms 后被限制到 ~8.33ms
        limiter.mark_emitted(t0);
        let t10 = t0 + Duration::from_millis(10);
        assert!(limiter.time_until_next_draw(t10).is_none()); // 10ms > 8.33ms

        // 切换到 low_motion；再次标记
        limiter.set_low_motion(true);
        limiter.mark_emitted(t10);
        let t20 = t10 + Duration::from_millis(10);
        let remaining = limiter.time_until_next_draw(t20).unwrap();
        // 30 FPS = 33.33 ms 间隔；已过 10ms → 约 23.33 剩余
        assert!(
            remaining > Duration::from_millis(20) && remaining < Duration::from_millis(25),
            "expected ~23.33ms remaining, got {remaining:?}"
        );
    }
}
