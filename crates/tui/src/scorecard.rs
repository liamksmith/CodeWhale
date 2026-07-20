//! Token / 缓存 / 成本计分卡（#3388）。
//!
//! 代理运行 Token 经济学的发布门控视图：每轮输入/输出/缓存读取 Token
//! 和成本、聚合总计 + 缓存命中率，以及针对已提交基线的回归检测。
//! 这是"Token、缓存和上下文纪律"EPIC 所要求的度量层——它使成本/Token
//! 回归可见，而不是静默地交付。
//!
//! 核心是纯离线的：它将已记录的每轮 [`Usage`]（每轮捕获，在 `TurnRecord`
//! 中持久化）转换为计分卡，重用现有定价层而不是重新发明成本计算。
//! `scorecard` 子命令是该模块的一个薄 I/O 包装。

use serde::{Deserialize, Serialize};

use crate::models::Usage;
use crate::pricing::{calculate_turn_cost_estimate_from_usage, token_usage_for_pricing};

/// 一轮的归一化 Token 经济学。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnScore {
    pub turn_id: String,
    pub model: String,
    /// 非缓存（可计费）输入 Token。
    pub input_tokens: u64,
    /// 输出 Token，包括推理输出。
    pub output_tokens: u64,
    /// 缓存读取（缓存命中）输入 Token。
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub cost_cny: f64,
    /// 当 `model` 没有定价行时为 true：成本报告为 0 但没有意义，
    /// 因此摘要可以标记它，而不是暗示"$0.00"。
    pub cost_unpriced: bool,
}

/// 一次运行的聚合指标。序列化/反序列化为基线文件。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScorecardMetrics {
    pub turns: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cost_usd: f64,
    pub total_cost_cny: f64,
    /// `cache_read / (input + cache_read)`；当没有输入 Token 时为 `0.0`。
    /// 越高越好（更多的提示词从缓存中提供）。
    pub cache_hit_ratio: f64,
}

/// 与基线相比增长超过允许阈值的指标。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
    /// 相对于基线的百分比增长。基线为 0 时为 `f64::INFINITY`。
    pub pct_increase: f64,
}

/// 完整计分卡：每轮明细加上聚合。
#[derive(Debug, Clone, Serialize)]
pub struct Scorecard {
    pub per_turn: Vec<TurnScore>,
    pub metrics: ScorecardMetrics,
}

/// 计分卡的一行输入：轮次 ID、服务的模型以及该轮记录的用量。
pub struct TurnInput<'a> {
    pub turn_id: String,
    pub model: String,
    pub usage: &'a Usage,
}

/// 从计分卡输入文件读取的记录轮次（JSON 数组）。
/// 匹配 `TurnEnd` hook 已经发出的每轮数据（`model` + `usage`），
/// 因此运行的轮次可以被捕获并进行离线评分。
#[derive(Debug, Clone, Deserialize)]
pub struct RecordedTurn {
    #[serde(default)]
    pub turn_id: String,
    pub model: String,
    pub usage: Usage,
}

impl Scorecard {
    /// 从记录的每轮用量构建计分卡。纯离线；成本通过共享定价层计算
    ///（`None` 定价 → 无定价，0 成本）。
    #[must_use]
    pub fn from_turns(turns: &[TurnInput<'_>]) -> Self {
        let mut per_turn = Vec::with_capacity(turns.len());
        let mut metrics = ScorecardMetrics::default();

        for turn in turns {
            // 一次性将提供商用量归一化为规范的可计费类别。
            let classes = token_usage_for_pricing(turn.usage);
            let cost = calculate_turn_cost_estimate_from_usage(&turn.model, turn.usage);
            let (cost_usd, cost_cny, cost_unpriced) = match cost {
                Some(c) => (c.usd, c.cny, false),
                None => (0.0, 0.0, true),
            };

            metrics.turns += 1;
            metrics.total_input_tokens += classes.input;
            metrics.total_output_tokens += classes.output;
            metrics.total_cache_read_tokens += classes.cache_read;
            metrics.total_cost_usd += cost_usd;
            metrics.total_cost_cny += cost_cny;

            per_turn.push(TurnScore {
                turn_id: turn.turn_id.clone(),
                model: turn.model.clone(),
                input_tokens: classes.input,
                output_tokens: classes.output,
                cache_read_tokens: classes.cache_read,
                cost_usd,
                cost_cny,
                cost_unpriced,
            });
        }

        let cacheable = metrics.total_input_tokens + metrics.total_cache_read_tokens;
        metrics.cache_hit_ratio = if cacheable > 0 {
            metrics.total_cache_read_tokens as f64 / cacheable as f64
        } else {
            0.0
        };

        Self { per_turn, metrics }
    }

    /// 渲染一个紧凑的人类可读摘要（用于非 JSON 输出）。
    #[must_use]
    pub fn to_summary(&self) -> String {
        let m = &self.metrics;
        let unpriced = self.per_turn.iter().filter(|t| t.cost_unpriced).count();
        let mut out = String::new();
        out.push_str("Token / cache / cost scorecard\n");
        out.push_str(&format!("turns: {}\n", m.turns));
        out.push_str(&format!(
            "input_tokens: {}  output_tokens: {}  cache_read_tokens: {}\n",
            m.total_input_tokens, m.total_output_tokens, m.total_cache_read_tokens
        ));
        out.push_str(&format!(
            "cache_hit_ratio: {:.1}%\n",
            m.cache_hit_ratio * 100.0
        ));
        out.push_str(&format!(
            "cost_usd: ${:.4}  cost_cny: ¥{:.4}\n",
            m.total_cost_usd, m.total_cost_cny
        ));
        if unpriced > 0 {
            out.push_str(&format!(
                "note: {unpriced} turn(s) had no pricing row; their cost is excluded.\n"
            ));
        }
        out
    }
}

impl ScorecardMetrics {
    /// 标记比 `baseline` 增长超过 `threshold_pct` 的指标。成本
    /// 和 Token 计数是"越低越好"，因此只有*增长*才是回归。
    ///（缓存命中率相反，单独报告。）
    #[must_use]
    pub fn regressions_against(
        &self,
        baseline: &ScorecardMetrics,
        threshold_pct: f64,
    ) -> Vec<Regression> {
        let mut out = Vec::new();
        push_regression(
            &mut out,
            "total_cost_usd",
            baseline.total_cost_usd,
            self.total_cost_usd,
            threshold_pct,
        );
        push_regression(
            &mut out,
            "total_input_tokens",
            baseline.total_input_tokens as f64,
            self.total_input_tokens as f64,
            threshold_pct,
        );
        push_regression(
            &mut out,
            "total_output_tokens",
            baseline.total_output_tokens as f64,
            self.total_output_tokens as f64,
            threshold_pct,
        );
        // 缓存命中率在*下降*时发生回归；将下降表示为正百分比，
        // 使其与其他指标读起来一致。
        if baseline.cache_hit_ratio > 0.0 {
            let drop_pct = (baseline.cache_hit_ratio - self.cache_hit_ratio)
                / baseline.cache_hit_ratio
                * 100.0;
            if drop_pct > threshold_pct {
                out.push(Regression {
                    metric: "cache_hit_ratio_drop".to_string(),
                    baseline: baseline.cache_hit_ratio,
                    current: self.cache_hit_ratio,
                    pct_increase: drop_pct,
                });
            }
        }
        out
    }
}

fn push_regression(
    out: &mut Vec<Regression>,
    metric: &str,
    base: f64,
    cur: f64,
    threshold_pct: f64,
) {
    if base > 0.0 {
        let pct = (cur - base) / base * 100.0;
        if pct > threshold_pct {
            out.push(Regression {
                metric: metric.to_string(),
                baseline: base,
                current: cur,
                pct_increase: pct,
            });
        }
    } else if cur > 0.0 {
        out.push(Regression {
            metric: metric.to_string(),
            baseline: base,
            current: cur,
            pct_increase: f64::INFINITY,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, cache_hit: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            prompt_cache_hit_tokens: Some(cache_hit),
            ..Default::default()
        }
    }

    #[test]
    fn aggregates_tokens_and_cache_hit_ratio_independent_of_pricing() {
        // input_tokens 包含缓存命中；token_usage_for_pricing 将它们拆分：
        // 非缓存输入 = 1000-200 = 800，cache_read = 200。
        let u1 = usage(1000, 500, 200);
        let u2 = usage(2000, 100, 800); // 非缓存 = 1200，cache_read = 800
        let turns = [
            TurnInput {
                turn_id: "t1".into(),
                model: "unpriced-x".into(),
                usage: &u1,
            },
            TurnInput {
                turn_id: "t2".into(),
                model: "unpriced-x".into(),
                usage: &u2,
            },
        ];
        let card = Scorecard::from_turns(&turns);

        assert_eq!(card.metrics.turns, 2);
        assert_eq!(card.metrics.total_input_tokens, 800 + 1200);
        assert_eq!(card.metrics.total_output_tokens, 600); // 500 + 100
        assert_eq!(card.metrics.total_cache_read_tokens, 1000); // 200 + 800
        // cache_read / (input + cache_read) = 1000 / (2000 + 1000)
        let expected = 1000.0 / 3000.0;
        assert!((card.metrics.cache_hit_ratio - expected).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_is_marked_unpriced_with_zero_cost() {
        let u = usage(1000, 500, 0);
        let turns = [TurnInput {
            turn_id: "t1".into(),
            model: "definitely-not-a-real-model".into(),
            usage: &u,
        }];
        let card = Scorecard::from_turns(&turns);
        assert!(card.per_turn[0].cost_unpriced);
        assert_eq!(card.per_turn[0].cost_usd, 0.0);
        assert_eq!(card.metrics.total_cost_usd, 0.0);
        assert!(card.to_summary().contains("no pricing row"));
    }

    #[test]
    fn regression_flags_cost_and_token_increases_over_threshold() {
        let baseline = ScorecardMetrics {
            turns: 1,
            total_input_tokens: 1000,
            total_output_tokens: 1000,
            total_cache_read_tokens: 0,
            total_cost_usd: 0.10,
            total_cost_cny: 0.7,
            cache_hit_ratio: 0.5,
        };
        let current = ScorecardMetrics {
            total_cost_usd: 0.20,      // +100% → 回归
            total_input_tokens: 1010,  // +1% → 低于 5% 阈值，无回归
            total_output_tokens: 2000, // +100% → 回归
            cache_hit_ratio: 0.5,      // 未变化
            ..baseline.clone()
        };
        let regs = current.regressions_against(&baseline, 5.0);
        let names: Vec<&str> = regs.iter().map(|r| r.metric.as_str()).collect();
        assert!(names.contains(&"total_cost_usd"));
        assert!(names.contains(&"total_output_tokens"));
        assert!(!names.contains(&"total_input_tokens")); // 低于阈值
    }

    #[test]
    fn regression_flags_cache_hit_ratio_drop() {
        let baseline = ScorecardMetrics {
            cache_hit_ratio: 0.80,
            ..Default::default()
        };
        let current = ScorecardMetrics {
            cache_hit_ratio: 0.40,
            ..Default::default()
        };
        let regs = current.regressions_against(&baseline, 10.0);
        assert!(regs.iter().any(|r| r.metric == "cache_hit_ratio_drop"));
    }

    #[test]
    fn no_regressions_when_within_threshold() {
        let baseline = ScorecardMetrics {
            total_cost_usd: 1.0,
            total_input_tokens: 1000,
            total_output_tokens: 1000,
            cache_hit_ratio: 0.5,
            ..Default::default()
        };
        let current = baseline.clone();
        assert!(current.regressions_against(&baseline, 5.0).is_empty());
    }
}
