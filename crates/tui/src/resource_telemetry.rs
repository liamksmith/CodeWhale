#![allow(dead_code)]

//! 长时间运行 CodeWhale 任务的资源使用遥测。
//!
//! 此模块是一个纯粹的、无副作用的底层基础，用于展示任务消耗了多少令牌
//! 和多少挂钟时间，可选地相对于预算。它不执行 I/O 和渲染；
//! 消费者（状态行、成本面板、目标/预算工具）单独连接，
//! 因此格式化和压力逻辑可以隔离进行单元测试。
//!
//! 形状有意镜像目标工具已使用的预算词汇表
//!（`token_budget: Option<_>`），以便消费者可以在这两者之间适配，
//! 而无需发明新概念。我们保留本地类型而不是在此处重用
//! `tools::goal`，以避免将表示层辅助函数耦合到工具
//! 领域模型（其预算是 `u32` 并具有不相关的簿记）。

use std::{
    fmt::{self, Write as _},
    time::Duration,
};

/// 任务距离耗尽预算的粗略三级读数。
///
/// 级别源自所有有界维度（令牌和时间）中的*最高*压力，
/// 因此令牌舒适但时间即将耗尽的任务仍报告 [`PressureLevel::High`]。
/// 当没有任何有界时，压力根据定义为 [`PressureLevel::Low`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    /// 充足余量（每个有界预算的 ~75% 以下）。
    Low,
    /// 接近边界（某个预算达到/超过约 75% 但低于 100%）。
    Medium,
    /// 某个有界维度达到或超过预算。
    High,
}

impl PressureLevel {
    /// 维度被视为中等压力的分数阈值。
    const MEDIUM_THRESHOLD: f64 = 0.75;
    /// 维度被视为高压力的分数阈值。
    const HIGH_THRESHOLD: f64 = 1.0;

    /// 分类单个预算分数（例如使用 41% 时为 `0.41`）。
    ///
    /// 负数或非有限输入被视为 [`PressureLevel::Low`]；
    /// 遥测辅助函数从不产生此类值，但防御性分类使此函数
    /// 对任意调用者都可用。
    fn from_fraction(fraction: f64) -> Self {
        if !fraction.is_finite() || fraction < Self::MEDIUM_THRESHOLD {
            PressureLevel::Low
        } else if fraction < Self::HIGH_THRESHOLD {
            PressureLevel::Medium
        } else {
            PressureLevel::High
        }
    }

    /// 适合紧凑状态输出的简短小写标签。
    pub fn label(self) -> &'static str {
        match self {
            PressureLevel::Low => "low",
            PressureLevel::Medium => "medium",
            PressureLevel::High => "high",
        }
    }
}

/// 单个任务的令牌和时间使用快照，带有可选预算。
///
/// 所有字段都是普通计数器；此类型不拥有时钟，不读取环境。
/// 从调用者已跟踪的任何内容构建它，并使用下面的辅助函数
/// 来渲染或分类它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceTelemetry {
    /// 到目前为止消耗的总令牌数。
    pub tokens_used: u64,
    /// 到目前为止经过的总挂钟秒数。
    pub time_used_seconds: u64,
    /// 任务的令牌上限；`None` 表示无限制。
    pub token_budget: Option<u64>,
    /// 以秒为单位的时间上限；`None` 表示无限制。
    pub time_budget_seconds: Option<u64>,
}

impl ResourceTelemetry {
    /// 创建无预算（完全无限制）的遥测快照。
    pub fn new(tokens_used: u64, time_used_seconds: u64) -> Self {
        Self {
            tokens_used,
            time_used_seconds,
            token_budget: None,
            time_budget_seconds: None,
        }
    }

    /// 设置令牌预算，返回更新后的快照（构建器风格）。
    pub fn with_token_budget(mut self, budget: u64) -> Self {
        self.token_budget = Some(budget);
        self
    }

    /// 设置时间预算（秒），返回更新后的快照。
    pub fn with_time_budget_seconds(mut self, seconds: u64) -> Self {
        self.time_budget_seconds = Some(seconds);
        self
    }

    /// 令牌预算的消耗比例，无上限时返回 `None`。
    ///
    /// 零预算返回 `None`（零的百分比无意义）
    /// 而不是无穷大，确保每个下游消费者安全。
    pub fn token_fraction(&self) -> Option<f64> {
        fraction(self.tokens_used, self.token_budget)
    }

    /// 时间预算的消耗比例，无上限时返回 `None`。
    pub fn time_fraction(&self) -> Option<f64> {
        fraction(self.time_used_seconds, self.time_budget_seconds)
    }

    /// 令牌和时间中最大的有界预算比例。
    ///
    /// 仅在*两个*维度都无上限时返回 `None`。当至少有一个
    /// 预算存在时，最有压力的有界维度胜出。
    pub fn budget_fraction(&self) -> Option<f64> {
        match (self.token_fraction(), self.time_fraction()) {
            (Some(t), Some(s)) => Some(t.max(s)),
            (Some(t), None) => Some(t),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        }
    }

    /// 预算比例表示为整百分比（四舍五入），无上限时返回 `None`。
    /// 这是人类摘要中展示的值。
    pub fn budget_percent(&self) -> Option<u64> {
        self.budget_fraction().map(|f| (f * 100.0).round() as u64)
    }

    /// 从 [`Self::budget_fraction`] 派生的粗略压力级别。
    ///
    /// 无上限的任务始终为 [`PressureLevel::Low`]。
    pub fn pressure(&self) -> PressureLevel {
        match self.budget_fraction() {
            Some(fraction) => PressureLevel::from_fraction(fraction),
            None => PressureLevel::Low,
        }
    }

    /// 紧凑、人类可读的一行摘要，例如 `12.3k tok · 4m12s · 41% budget`。
    ///
    /// 令牌用 `k`/`M` 后缀缩写，时间渲染为
    /// `Hh Mm Ss`（删除前导零单位），任务无上限时
    /// 完全省略预算部分。
    pub fn human_summary(&self) -> String {
        let mut out = String::new();
        // 向 String 写入 `write!` 不会失败；忽略 Result。
        let _ = write!(
            out,
            "{} tok · {}",
            format_tokens(self.tokens_used),
            format_duration(self.time_used_seconds),
        );
        if let Some(percent) = self.budget_percent() {
            let _ = write!(out, " · {percent}% budget");
        }
        out
    }
}

impl fmt::Display for ResourceTelemetry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.human_summary())
    }
}

/// 进行中或已完成回合的输出令牌吞吐量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenThroughput {
    pub output_tokens: u64,
    pub elapsed_seconds: f64,
}

impl TokenThroughput {
    pub fn new(output_tokens: u64, elapsed: Duration) -> Option<Self> {
        let elapsed_seconds = elapsed.as_secs_f64();
        if output_tokens == 0 || !elapsed_seconds.is_finite() || elapsed_seconds <= 0.0 {
            return None;
        }
        Some(Self {
            output_tokens,
            elapsed_seconds,
        })
    }

    pub fn from_estimated_text(text: &str, elapsed: Duration) -> Option<Self> {
        Self::new(estimate_output_tokens_from_text(text), elapsed)
    }

    pub fn tokens_per_second(self) -> f64 {
        self.output_tokens as f64 / self.elapsed_seconds
    }

    pub fn compact_rate(self) -> String {
        let rate = self.tokens_per_second();
        if rate < 10.0 {
            format!("{rate:.1}")
        } else {
            format!("{rate:.0}")
        }
    }
}

/// 在提供商使用数据到达前从流式文本估计输出令牌。
///
/// 提供商报告的使用在回合完成时仍为规范数据。在实时流期间，
/// 这为页脚提供稳定的近似值，无需检查提供商特定的分词器内部。
pub fn estimate_output_tokens_from_text(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    if chars == 0 {
        0
    } else {
        chars.saturating_add(3) / 4
    }
}

/// 用 `used` 除以可选预算，防止缺失或零预算。
/// 当预算为 `None` 或 `0` 时返回 `None`。
fn fraction(used: u64, budget: Option<u64>) -> Option<f64> {
    match budget {
        Some(budget) if budget > 0 => Some(used as f64 / budget as f64),
        _ => None,
    }
}

/// 在超过每个阈值时用 `k`/`M` 后缀格式化令牌计数。
///
/// 低于 1_000 的值按原样打印。千位使用一个小数位
///（`12.3k`），修剪尾随的 `.0` 以便圆整值干净地读取（`5k`）。
/// 百万遵循相同规则（`1.5M`、`2M`）。
fn format_tokens(tokens: u64) -> String {
    const K: u64 = 1_000;
    const M: u64 = 1_000_000;
    if tokens >= M {
        format_scaled(tokens, M, 'M')
    } else if tokens >= K {
        format_scaled(tokens, K, 'k')
    } else {
        tokens.to_string()
    }
}

/// 用 `suffix` 将 `value / divisor` 渲染到一个小数位，去掉尾随的 `.0`。
/// 除数始终是上述常量之一（非零）。
fn format_scaled(value: u64, divisor: u64, suffix: char) -> String {
    let scaled = value as f64 / divisor as f64;
    // 在决定小数部分是否为 ".0" 前四舍五入到一位小数，因此
    // 像 1_999_999 这样的值读作 "2M" 而不是 "1.9...M"。
    let rounded = (scaled * 10.0).round() / 10.0;
    if (rounded.fract()).abs() < f64::EPSILON {
        format!("{}{}", rounded as u64, suffix)
    } else {
        format!("{rounded:.1}{suffix}")
    }
}

/// 将秒数格式化为紧凑的 `Hh Mm Ss` 字符串。
///
/// 前导零单位被删除，因此 252 秒渲染为 `4m12s`，90 秒渲染为
/// `1m30s`。亚分钟持续时间渲染为裸秒（`0s`、`45s`）。只有当
/// 更大的单位在它们之前时，分钟和秒才补零，匹配传统的
/// 时钟风格读数（`1h05m`、`2h00m03s`）。
fn format_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;

    let mut out = String::new();
    if hours > 0 {
        let _ = write!(out, "{hours}h");
    }
    if hours > 0 || minutes > 0 {
        if hours > 0 {
            let _ = write!(out, "{minutes:02}m");
        } else {
            let _ = write!(out, "{minutes}m");
        }
    }
    // 始终包括秒，除非我们有小时+分钟且秒为零
    // 仍然会有信息量；我们为了精度保留秒，当分钟或小时在前时补零。
    if hours > 0 || minutes > 0 {
        let _ = write!(out, "{seconds:02}s");
    } else {
        let _ = write!(out, "{seconds}s");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 令牌格式化 -------------------------------------------------

    #[test]
    fn format_tokens_under_a_thousand_is_verbatim() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(1), "1");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_uses_k_suffix_with_trimmed_decimal() {
        assert_eq!(format_tokens(1_000), "1k");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(12_345), "12.3k");
        // 恰好整千时修剪 ".0"。
        assert_eq!(format_tokens(5_000), "5k");
        // 刚好在百万边界以下保持在 k 范围。
        assert_eq!(format_tokens(999_400), "999.4k");
    }

    #[test]
    fn format_tokens_uses_m_suffix_for_millions() {
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(1_500_000), "1.5M");
        assert_eq!(format_tokens(2_340_000), "2.3M");
    }

    #[test]
    fn format_tokens_rounds_up_across_a_unit_boundary() {
        // 1_999_999 四舍五入为 2.0M -> "2M"，不是 "1.9M" 或 "2.0M"。
        assert_eq!(format_tokens(1_999_999), "2M");
        // 999_950 四舍五入为 1000.0k；仍在 k 分支并修剪 ".0"。
        assert_eq!(format_tokens(999_950), "1000k");
    }

    #[test]
    fn format_tokens_handles_very_large_values() {
        assert_eq!(format_tokens(u64::MAX), "18446744073709.6M");
    }

    // ---- 持续时间格式化 ---------------------------------------------

    #[test]
    fn format_duration_zero_and_sub_minute() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(1), "1s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn format_duration_minutes_and_seconds() {
        assert_eq!(format_duration(60), "1m00s");
        assert_eq!(format_duration(90), "1m30s");
        assert_eq!(format_duration(252), "4m12s");
        assert_eq!(format_duration(599), "9m59s");
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(3_600), "1h00m00s");
        assert_eq!(format_duration(3_661), "1h01m01s");
        // 2h00m03s 测试小时和秒之间的零填充分钟。
        assert_eq!(format_duration(7_203), "2h00m03s");
    }

    #[test]
    fn format_duration_large() {
        // 100 小时，1 分钟，1 秒。
        assert_eq!(format_duration(360_061), "100h01m01s");
    }

    // ---- 吞吐量 -------------------------------------------------------

    #[test]
    fn token_throughput_formats_compact_rates() {
        let throughput = TokenThroughput::new(120, Duration::from_secs(6)).expect("throughput");
        assert_eq!(throughput.tokens_per_second(), 20.0);
        assert_eq!(throughput.compact_rate(), "20");

        let slow = TokenThroughput::new(15, Duration::from_secs(4)).expect("throughput");
        assert_eq!(slow.compact_rate(), "3.8");
    }

    #[test]
    fn token_throughput_rejects_empty_or_zero_elapsed_samples() {
        assert!(TokenThroughput::new(0, Duration::from_secs(5)).is_none());
        assert!(TokenThroughput::new(5, Duration::ZERO).is_none());
    }

    #[test]
    fn estimated_streaming_tokens_round_up_from_text_chars() {
        assert_eq!(estimate_output_tokens_from_text(""), 0);
        assert_eq!(estimate_output_tokens_from_text("abc"), 1);
        assert_eq!(estimate_output_tokens_from_text("abcd"), 1);
        assert_eq!(estimate_output_tokens_from_text("abcde"), 2);

        let throughput =
            TokenThroughput::from_estimated_text(&"x".repeat(400), Duration::from_secs(10))
                .expect("estimated throughput");
        assert_eq!(throughput.output_tokens, 100);
        assert_eq!(throughput.compact_rate(), "10");
    }

    // ---- 分数 / 百分比 ----------------------------------------------

    #[test]
    fn fractions_are_none_when_unbounded() {
        let t = ResourceTelemetry::new(5_000, 120);
        assert_eq!(t.token_fraction(), None);
        assert_eq!(t.time_fraction(), None);
        assert_eq!(t.budget_fraction(), None);
        assert_eq!(t.budget_percent(), None);
    }

    #[test]
    fn zero_budget_yields_none_not_infinity() {
        let t = ResourceTelemetry {
            tokens_used: 100,
            time_used_seconds: 0,
            token_budget: Some(0),
            time_budget_seconds: Some(0),
        };
        assert_eq!(t.token_fraction(), None);
        assert_eq!(t.time_fraction(), None);
        assert_eq!(t.budget_fraction(), None);
        assert_eq!(t.pressure(), PressureLevel::Low);
    }

    #[test]
    fn token_fraction_is_computed_when_bounded() {
        let t = ResourceTelemetry::new(4_100, 0).with_token_budget(10_000);
        let frac = t.token_fraction().expect("bounded");
        assert!((frac - 0.41).abs() < 1e-9, "got {frac}");
        assert_eq!(t.budget_percent(), Some(41));
    }

    #[test]
    fn budget_fraction_takes_the_max_across_dimensions() {
        // 令牌 10%，时间 80% -> 时间压力占主导。
        let t = ResourceTelemetry {
            tokens_used: 1_000,
            time_used_seconds: 80,
            token_budget: Some(10_000),
            time_budget_seconds: Some(100),
        };
        let frac = t.budget_fraction().expect("bounded");
        assert!((frac - 0.80).abs() < 1e-9, "got {frac}");
        assert_eq!(t.budget_percent(), Some(80));
    }

    #[test]
    fn budget_fraction_present_when_only_one_dimension_bounded() {
        let only_time = ResourceTelemetry::new(9_999, 50).with_time_budget_seconds(200);
        assert_eq!(only_time.budget_percent(), Some(25));

        let only_tokens = ResourceTelemetry::new(2_500, 9_999).with_token_budget(10_000);
        assert_eq!(only_tokens.budget_percent(), Some(25));
    }

    #[test]
    fn budget_percent_rounds_to_nearest_whole() {
        // 333 / 1000 = 33.3% -> 33
        let down = ResourceTelemetry::new(333, 0).with_token_budget(1_000);
        assert_eq!(down.budget_percent(), Some(33));
        // 336 / 1000 = 33.6% -> 34
        let up = ResourceTelemetry::new(336, 0).with_token_budget(1_000);
        assert_eq!(up.budget_percent(), Some(34));
    }

    // ---- 压力级别 --------------------------------------------------

    #[test]
    fn pressure_low_when_unbounded_regardless_of_usage() {
        let t = ResourceTelemetry::new(u64::MAX, u64::MAX);
        assert_eq!(t.pressure(), PressureLevel::Low);
    }

    #[test]
    fn pressure_thresholds_just_under_and_over() {
        // 74% -> Low（刚好在中等阈值以下）。
        let low = ResourceTelemetry::new(7_400, 0).with_token_budget(10_000);
        assert_eq!(low.pressure(), PressureLevel::Low);

        // 恰好 75% -> Medium（包含下限）。
        let medium_edge = ResourceTelemetry::new(7_500, 0).with_token_budget(10_000);
        assert_eq!(medium_edge.pressure(), PressureLevel::Medium);

        // 99% -> Medium（刚好在高阈值以下）。
        let medium = ResourceTelemetry::new(9_900, 0).with_token_budget(10_000);
        assert_eq!(medium.pressure(), PressureLevel::Medium);

        // 恰好 100% -> High（达到预算）。
        let high_edge = ResourceTelemetry::new(10_000, 0).with_token_budget(10_000);
        assert_eq!(high_edge.pressure(), PressureLevel::High);

        // 超过预算 -> High。
        let over = ResourceTelemetry::new(12_500, 0).with_token_budget(10_000);
        assert_eq!(over.pressure(), PressureLevel::High);
    }

    #[test]
    fn pressure_level_labels_and_ordering() {
        assert_eq!(PressureLevel::Low.label(), "low");
        assert_eq!(PressureLevel::Medium.label(), "medium");
        assert_eq!(PressureLevel::High.label(), "high");
        // Ord 派生：Low < Medium < High。
        assert!(PressureLevel::Low < PressureLevel::Medium);
        assert!(PressureLevel::Medium < PressureLevel::High);
    }

    #[test]
    fn pressure_from_fraction_ignores_non_finite() {
        assert_eq!(PressureLevel::from_fraction(f64::NAN), PressureLevel::Low);
        assert_eq!(
            PressureLevel::from_fraction(f64::INFINITY),
            PressureLevel::Low
        );
        assert_eq!(PressureLevel::from_fraction(-0.5), PressureLevel::Low);
    }

    // ---- 人类摘要 ----------------------------------------------------

    #[test]
    fn human_summary_bounded_matches_example_shape() {
        let t = ResourceTelemetry::new(12_345, 252).with_token_budget(30_000);
        // 12_345 -> "12.3k", 252s -> "4m12s", 12345/30000 = 41.15% -> 41%。
        assert_eq!(t.human_summary(), "12.3k tok · 4m12s · 41% budget");
    }

    #[test]
    fn human_summary_unbounded_omits_budget_segment() {
        let t = ResourceTelemetry::new(500, 5);
        assert_eq!(t.human_summary(), "500 tok · 5s");
        // Display 委托给 human_summary。
        assert_eq!(t.to_string(), "500 tok · 5s");
    }

    #[test]
    fn human_summary_zero_everything() {
        let t = ResourceTelemetry::default();
        assert_eq!(t.human_summary(), "0 tok · 0s");
    }

    #[test]
    fn human_summary_over_budget_can_exceed_one_hundred_percent() {
        let t = ResourceTelemetry::new(15_000, 7_320).with_token_budget(10_000);
        // 15000/10000 = 150%, 2h02m00s。
        assert_eq!(t.human_summary(), "15k tok · 2h02m00s · 150% budget");
        assert_eq!(t.pressure(), PressureLevel::High);
    }

    #[test]
    fn human_summary_with_only_time_budget() {
        let t = ResourceTelemetry::new(2_000_000, 300).with_time_budget_seconds(600);
        // 2M 令牌，5m00s，300/600 = 50% 预算。
        assert_eq!(t.human_summary(), "2M tok · 5m00s · 50% budget");
        assert_eq!(t.pressure(), PressureLevel::Low);
    }
}
