use codewhale_config::route::RouteLimits;

use crate::config::{ApiProvider, provider_capability};
use crate::context_budget::ContextBudget;
use crate::models::{
    DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS, DEFAULT_COMPACTION_TOKEN_THRESHOLD,
    compaction_threshold_for_model_at_percent,
};

/// 仅保留来自具体产品的路由限制。
#[must_use]
pub(crate) fn known_route_limits(limits: RouteLimits) -> Option<RouteLimits> {
    limits.has_known_limit().then_some(limits)
}

/// 已解析运行时路由的上下文窗口。
///
/// 已知的路由/产品事实优先；否则回退到现有的提供商+模型能力矩阵，以便启动和自定义/本地路由保持其之前的保守行为。
#[must_use]
pub(crate) fn route_context_window_tokens(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
) -> u32 {
    route_limits
        .and_then(|limits| limits.context_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
        .unwrap_or_else(|| provider_capability(provider, model).context_window)
}

/// 提供商/产品输出上限，当已解析的路由报告时。
#[must_use]
pub(crate) fn route_output_limit_tokens(route_limits: Option<RouteLimits>) -> Option<u32> {
    route_limits
        .and_then(|limits| limits.output_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

#[must_use]
pub(crate) fn route_context_budget(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    input_tokens: usize,
    configured_output_cap: u32,
) -> Option<ContextBudget> {
    let window = route_context_window_tokens(provider, model, route_limits);
    Some(ContextBudget::new(
        u64::from(window),
        u64::try_from(input_tokens).ok()?,
        u64::from(configured_output_cap),
    ))
}

#[must_use]
pub(crate) fn compaction_threshold_for_route_at_percent(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    percent: f64,
) -> usize {
    if route_limits
        .and_then(|limits| limits.context_tokens)
        .is_some()
    {
        let window = route_context_window_tokens(provider, model, route_limits);
        let percent = percent.clamp(10.0, 100.0);
        let threshold = (f64::from(window) * percent / 100.0).round();
        let threshold = if threshold.is_finite() && threshold > 0.0 {
            threshold as u64
        } else {
            return DEFAULT_COMPACTION_TOKEN_THRESHOLD;
        };
        return usize::try_from(threshold).unwrap_or(DEFAULT_COMPACTION_TOKEN_THRESHOLD);
    }

    compaction_threshold_for_model_at_percent(model, percent)
}

#[must_use]
pub(crate) fn auto_compact_default_for_route(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
) -> bool {
    if route_limits
        .and_then(|limits| limits.context_tokens)
        .is_some()
    {
        return route_context_window_tokens(provider, model, route_limits)
            <= DEFAULT_AUTO_COMPACT_MAX_CONTEXT_WINDOW_TOKENS;
    }

    crate::models::auto_compact_default_for_model(model)
}
