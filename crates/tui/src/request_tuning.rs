//! 通过 CodeWhale 请求路由携带的请求调优意图（#3024）。
//!
//! 此处的请求"调优"指调用方可附加到出站模型请求的可选旋钮，它们塑造模型*如何*响应，
//! 而不改变*询问什么*：推理努力等级和最大输出 token 数。此模块仅在路由层之间传递该意图。
//! 客户端代码仍负责将该意图转换为每个提供商的线路格式。
//!
//! ## 推理努力枚举复用
//!
//! [`RequestTuning::reasoning_effort`] 复用了规范的 [`crate::tui::app::ReasoningEffort`] 枚举，
//! 而非定义本地的 `Off/Low/Medium/High` 副本。该枚举是跨 DeepSeek 和 Codex 努力选择器的努力等级的唯一真相来源，
//! 已被同级顶层模块（`auto_reasoning`、`model_routing`）导入，并携带了未来请求调优消费者所需的提供商标准化逻辑
//!（`normalize_for_provider`、`api_value_for_provider`）。在此处定义平行的本地枚举会重复该接口并存在漂移风险，
//! 因此我们导入现有类型。
//!
use crate::tui::app::ReasoningEffort;

/// 调用方可附加到模型请求的可选请求调优旋钮。
///
/// 两个字段都是 `Option`：`None` 表示"不调优；使用提供商默认值"。
/// 这是描述意图的元数据 — 将其应用于线路请求是客户端的责任，而非此模块。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestTuning {
    /// 期望的推理努力等级，或 `None` 使用提供商默认值。
    ///
    /// 复用规范的 [`ReasoningEffort`] 枚举（参见模块文档）。
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 期望的最大输出 token 数，或 `None` 使用提供商默认值。
    pub max_output_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_tuning_default_has_no_knobs() {
        let tuning = RequestTuning::default();
        assert_eq!(tuning.reasoning_effort, None);
        assert_eq!(tuning.max_output_tokens, None);
    }

    #[test]
    fn request_tuning_reuses_reasoning_effort_enum() {
        let tuning = RequestTuning {
            reasoning_effort: Some(ReasoningEffort::High),
            max_output_tokens: Some(4096),
        };
        assert_eq!(tuning.reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(tuning.max_output_tokens, Some(4096));
    }
}
