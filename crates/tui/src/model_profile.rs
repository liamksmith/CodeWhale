//! 类型化的模型和已解析路由能力描述符（#3365）。
//!
//! 此模块桥接了附加的 [`crate::model_registry`] 事实和
//! [`crate::config::provider_capability`] 中的提供商+模型能力矩阵。
//! 它有意将内在模型事实与已解析路由事实分开，
//! 以便未来的路由解析可以组合目录产品、用户
//! 覆盖、实时提示和认证就绪状态，而无需在提示词、工具和 Fleet 代码中
//! 散布提供商/模型字符串检查。
#![allow(dead_code)]

use crate::config::{ApiProvider, RequestPayloadMode, provider_capability};
use crate::model_registry::{self, ModelProvider};

/// 三态支持事实。Unknown 与 Unsupported 不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportState {
    Supported,
    Unsupported,
    Unknown,
}

impl SupportState {
    #[must_use]
    pub const fn from_bool(value: bool) -> Self {
        if value {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

/// 所选路线的粗略工具目录预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSurfaceBudget {
    /// 只保留最基本的首轮工具表面。
    Compact,
    /// 当前默认表面：核心工具有效，长尾工具延迟加载。
    Standard,
    /// 大窗口/全能力路线可以负担标准的完整头部。
    Full,
}

/// 用于诊断和路线解释的事实来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactProvenance {
    SeededModelRegistry,
    LegacyModelHeuristics,
    ConservativeUnknownFallback,
    ProviderCapabilityMatrix,
    UserOverride,
}

/// 模型身份拥有的与提供商无关的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntrinsicCapabilityProfile {
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub reasoning: SupportState,
    pub native_tool_calls: SupportState,
    pub parallel_tool_calls: SupportState,
    pub structured_output: SupportState,
    pub streaming: SupportState,
    pub prompt_caching: SupportState,
    pub tool_surface_budget: ToolSurfaceBudget,
}

/// 模型拥有的配置文件。这不意味着提供商路由已就绪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProfile {
    pub canonical_id: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub family: Option<ModelProvider>,
    pub capabilities: IntrinsicCapabilityProfile,
    pub provenance: FactProvenance,
}

/// 在提供商能力事实之后层叠的可选能力覆盖。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityOverride {
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub reasoning: Option<SupportState>,
    pub native_tool_calls: Option<SupportState>,
    pub parallel_tool_calls: Option<SupportState>,
    pub structured_output: Option<SupportState>,
    pub streaming: Option<SupportState>,
    pub prompt_caching: Option<SupportState>,
    pub tool_surface_budget: Option<ToolSurfaceBudget>,
}

/// 应用提供商路线事实和用户覆盖后的能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityProfile {
    pub provider: ApiProvider,
    pub canonical_model: Option<String>,
    pub wire_model_id: String,
    pub request_payload_mode: RequestPayloadMode,
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub reasoning: SupportState,
    pub native_tool_calls: SupportState,
    pub parallel_tool_calls: SupportState,
    pub structured_output: SupportState,
    pub streaming: SupportState,
    pub prompt_caching: SupportState,
    pub tool_surface_budget: ToolSurfaceBudget,
    pub provenance: Vec<FactProvenance>,
}

impl CapabilityProfile {
    #[must_use]
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning.is_supported()
    }

    #[must_use]
    pub fn has_large_context(&self) -> bool {
        self.context_window.is_some_and(|window| window >= 400_000)
    }

    #[must_use]
    pub fn prefers_full_tool_surface(&self) -> bool {
        matches!(self.tool_surface_budget, ToolSurfaceBudget::Full)
    }

    #[must_use]
    pub fn suitable_for_broad_fleet_worker(&self) -> bool {
        self.has_large_context()
            && !matches!(self.native_tool_calls, SupportState::Unsupported)
            && matches!(
                self.tool_surface_budget,
                ToolSurfaceBudget::Standard | ToolSurfaceBudget::Full
            )
    }
}

/// 为任何模型字符串构建内在配置文件。
#[must_use]
pub fn model_profile(model: &str) -> ModelProfile {
    let trimmed = model.trim();
    let display_name = display_name(trimmed);
    match model_registry::lookup(trimmed) {
        Some(meta) => {
            let canonical_id = if meta.id.is_empty() {
                trimmed.to_string()
            } else {
                meta.id.to_string()
            };
            let provenance = if meta.id.is_empty() {
                FactProvenance::LegacyModelHeuristics
            } else {
                FactProvenance::SeededModelRegistry
            };
            ModelProfile {
                canonical_id,
                display_name,
                aliases: Vec::new(),
                family: Some(meta.provider),
                capabilities: IntrinsicCapabilityProfile {
                    context_window: meta.context_window,
                    max_output: meta.max_output,
                    reasoning: SupportState::from_bool(meta.supports_reasoning),
                    native_tool_calls: SupportState::Unknown,
                    parallel_tool_calls: SupportState::Unknown,
                    structured_output: SupportState::Unknown,
                    streaming: SupportState::Supported,
                    prompt_caching: SupportState::Unknown,
                    tool_surface_budget: tool_surface_for_window(meta.context_window),
                },
                provenance,
            }
        }
        None => ModelProfile {
            canonical_id: trimmed.to_string(),
            display_name,
            aliases: Vec::new(),
            family: None,
            capabilities: IntrinsicCapabilityProfile {
                context_window: None,
                max_output: None,
                reasoning: SupportState::Unknown,
                native_tool_calls: SupportState::Unknown,
                parallel_tool_calls: SupportState::Unknown,
                structured_output: SupportState::Unknown,
                streaming: SupportState::Unknown,
                prompt_caching: SupportState::Unknown,
                tool_surface_budget: ToolSurfaceBudget::Compact,
            },
            provenance: FactProvenance::ConservativeUnknownFallback,
        },
    }
}

/// 从内在模型事实加上提供商事实解析路线能力。
#[must_use]
pub fn resolved_capability_profile(
    provider: ApiProvider,
    wire_model_id: &str,
) -> CapabilityProfile {
    resolved_capability_profile_with_overrides(
        provider,
        wire_model_id,
        CapabilityOverride::default(),
    )
}

/// 解析路线能力并最后应用显式用户/配置覆盖。
#[must_use]
pub fn resolved_capability_profile_with_overrides(
    provider: ApiProvider,
    wire_model_id: &str,
    overrides: CapabilityOverride,
) -> CapabilityProfile {
    let model = model_profile(wire_model_id);
    let provider_cap = provider_capability(provider, wire_model_id);
    let request_payload_mode = provider_cap.request_payload_mode;
    let context_window = Some(
        overrides
            .context_window
            .unwrap_or(provider_cap.context_window),
    );
    let max_output = Some(overrides.max_output.unwrap_or(provider_cap.max_output));
    let reasoning = overrides
        .reasoning
        .unwrap_or_else(|| SupportState::from_bool(provider_cap.thinking_supported));
    let prompt_caching = overrides
        .prompt_caching
        .unwrap_or_else(|| SupportState::from_bool(provider_cap.cache_telemetry_supported));
    let native_tool_calls = overrides
        .native_tool_calls
        .unwrap_or_else(|| native_tool_support_for_payload(request_payload_mode));
    let structured_output = overrides
        .structured_output
        .unwrap_or(model.capabilities.structured_output);
    let streaming = overrides.streaming.unwrap_or(SupportState::Supported);
    let parallel_tool_calls = overrides
        .parallel_tool_calls
        .unwrap_or(model.capabilities.parallel_tool_calls);
    let tool_surface_budget = overrides
        .tool_surface_budget
        .unwrap_or_else(|| tool_surface_for_window(context_window));

    let mut provenance = vec![model.provenance, FactProvenance::ProviderCapabilityMatrix];
    if overrides != CapabilityOverride::default() {
        provenance.push(FactProvenance::UserOverride);
    }

    CapabilityProfile {
        provider,
        canonical_model: Some(model.canonical_id),
        wire_model_id: wire_model_id.to_string(),
        request_payload_mode,
        context_window,
        max_output,
        reasoning,
        native_tool_calls,
        parallel_tool_calls,
        structured_output,
        streaming,
        prompt_caching,
        tool_surface_budget,
        provenance,
    }
}

#[must_use]
pub fn tool_surface_for_window(context_window: Option<u32>) -> ToolSurfaceBudget {
    match context_window {
        Some(window) if window >= 400_000 => ToolSurfaceBudget::Full,
        Some(window) if window >= 128_000 => ToolSurfaceBudget::Standard,
        _ => ToolSurfaceBudget::Compact,
    }
}

fn native_tool_support_for_payload(mode: RequestPayloadMode) -> SupportState {
    match mode {
        RequestPayloadMode::ChatCompletions
        | RequestPayloadMode::Responses
        | RequestPayloadMode::AnthropicMessages => SupportState::Supported,
    }
}

fn display_name(model: &str) -> String {
    model
        .rsplit(['/', ':'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(model)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_profile_known_lookup_uses_seeded_model_facts() {
        let profile = model_profile("deepseek-v4-pro");

        assert_eq!(profile.canonical_id, "deepseek-v4-pro");
        assert_eq!(profile.family, Some(ModelProvider::DeepSeek));
        assert_eq!(profile.capabilities.context_window, Some(1_000_000));
        assert_eq!(profile.capabilities.max_output, Some(384_000));
        assert_eq!(profile.capabilities.reasoning, SupportState::Supported);
        assert_eq!(
            profile.capabilities.tool_surface_budget,
            ToolSurfaceBudget::Full
        );
        assert_eq!(profile.provenance, FactProvenance::SeededModelRegistry);
    }

    #[test]
    fn model_profile_unknown_fallback_is_conservative() {
        let profile = model_profile("custom-local-model");

        assert_eq!(profile.canonical_id, "custom-local-model");
        assert_eq!(profile.family, None);
        assert_eq!(profile.capabilities.context_window, None);
        assert_eq!(profile.capabilities.reasoning, SupportState::Unknown);
        assert_eq!(
            profile.capabilities.native_tool_calls,
            SupportState::Unknown
        );
        assert_eq!(
            profile.capabilities.tool_surface_budget,
            ToolSurfaceBudget::Compact
        );
        assert_eq!(
            profile.provenance,
            FactProvenance::ConservativeUnknownFallback
        );
    }

    #[test]
    fn resolved_capability_profile_merges_provider_facts_and_overrides() {
        let profile = resolved_capability_profile_with_overrides(
            ApiProvider::OpenaiCodex,
            "gpt-5-codex",
            CapabilityOverride {
                context_window: Some(123_456),
                reasoning: Some(SupportState::Unsupported),
                tool_surface_budget: Some(ToolSurfaceBudget::Compact),
                ..CapabilityOverride::default()
            },
        );

        assert_eq!(profile.provider, ApiProvider::OpenaiCodex);
        assert_eq!(profile.request_payload_mode, RequestPayloadMode::Responses);
        assert_eq!(profile.context_window, Some(123_456));
        assert_eq!(profile.reasoning, SupportState::Unsupported);
        assert_eq!(profile.native_tool_calls, SupportState::Supported);
        assert_eq!(profile.tool_surface_budget, ToolSurfaceBudget::Compact);
        assert!(profile.provenance.contains(&FactProvenance::UserOverride));
    }

    #[test]
    fn capability_predicates_are_not_provider_string_checks() {
        let broad = resolved_capability_profile(ApiProvider::Deepseek, "deepseek-v4-pro");
        let compact = resolved_capability_profile_with_overrides(
            ApiProvider::Openrouter,
            "unknown-small-model",
            CapabilityOverride {
                context_window: Some(32_000),
                native_tool_calls: Some(SupportState::Unknown),
                tool_surface_budget: Some(ToolSurfaceBudget::Compact),
                ..CapabilityOverride::default()
            },
        );

        assert!(broad.has_large_context());
        assert!(broad.prefers_full_tool_surface());
        assert!(broad.suitable_for_broad_fleet_worker());
        assert!(!compact.has_large_context());
        assert!(!compact.prefers_full_tool_surface());
        assert!(!compact.suitable_for_broad_fleet_worker());
    }
}
