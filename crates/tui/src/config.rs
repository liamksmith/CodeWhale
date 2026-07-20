//! codewhale 的配置加载和默认值。

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use codewhale_execpolicy::ExecPolicyEngine;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::audit::log_sensitive_event;
use crate::features::{Feature, Features, FeaturesToml, is_known_feature_key};
use crate::hooks::HooksConfig;

// 子代理并发/超时限制常量及其限制解析器位于 `subagent_limits` 叶子模块中。
// 常量被重新导出（保持每个项目的可见性），因此 `crate::config::<CONST>` 路径的解析保持不变；
// 私有解析器被拉回而不扩大外部接口（#3311）。
mod subagent_limits;
pub use subagent_limits::*;
use subagent_limits::{resolve_subagent_api_timeout_secs, resolve_subagent_heartbeat_timeout_secs};

// 提供商模型名称和基础 URL 常量位于 `models` 叶子模块中
// 并在下方重新导出，以便每个 `crate::config::<CONST>` 路径保持不变（#3311）。
mod models;
pub use models::*;

const API_KEYRING_SENTINEL: &str = "__KEYRING__";
pub const DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY: usize = 3;
pub const MAX_PROVIDER_REQUEST_CONCURRENCY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    Deepseek,
    DeepseekCN,
    DeepseekAnthropic,
    NvidiaNim,
    Openai,
    Atlascloud,
    WanjieArk,
    Volcengine,
    Openrouter,
    XiaomiMimo,
    Novita,
    Fireworks,
    Siliconflow,
    SiliconflowCn,
    Arcee,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    Huggingface,
    Together,
    Qianfan,
    OpenaiCodex,
    Anthropic,
    Openmodel,
    Zai,
    Stepfun,
    Minimax,
    Deepinfra,
    Sakana,
    LongCat,
    Meta,
    Xai,
    /// 用户自定义的 OpenAI 兼容端点（#1519）。
    ///
    /// 当 `provider = "<name>"` 指定了一个 `[providers.<name>] kind="openai-compatible"` 表时选中。
    /// 一个单一动态标识，映射到 [`codewhale_config::ProviderKind::Custom`]
    /// 并通过 OpenAI Chat Completions 有线协议路由；具体的端点/模型/认证来自
    /// 命名的配置表，而非此变体。
    Custom,
}

impl ApiProvider {
    #[must_use]
    pub fn names_hint() -> String {
        let mut names = Vec::with_capacity(Self::all().len() + 1);
        names.push(Self::Deepseek.as_str());
        names.push(Self::DeepseekCN.as_str());
        names.extend(
            Self::all()
                .iter()
                .filter(|provider| !matches!(provider, Self::Deepseek))
                .map(|provider| provider.as_str()),
        );
        names.join(", ")
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        // ApiProvider 特定："deepseek-cn" 在此处是一个遗留变体，
        // 而 ProviderKind 将其视为 Deepseek 别名。
        if trimmed.eq_ignore_ascii_case("deepseek-cn")
            || trimmed.eq_ignore_ascii_case("deepseek_china")
            || trimmed.eq_ignore_ascii_case("deepseekcn")
            || trimmed.eq_ignore_ascii_case("deepseek-china")
        {
            return Some(Self::DeepseekCN);
        }
        codewhale_config::ProviderKind::parse(value).map(Self::from_kind)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self.kind() {
            Some(kind) => kind.as_str(),
            None => "deepseek-cn",
        }
    }

    /// 用于选择器 UI / 状态标签的人工友好标签。
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self.kind() {
            Some(kind) => kind.provider().display_name(),
            None => "DeepSeek (legacy alias)",
        }
    }

    /// 来自共享 config crate 的提供商元数据。
    ///
    /// 仅对 TUI 独有的遗留 `DeepseekCN` 变体返回 `None`，
    /// 该变体有意保留自己的配置表，同时共享 DeepSeek 认证环境变量。
    #[must_use]
    pub fn metadata(self) -> Option<&'static dyn codewhale_config::provider::Provider> {
        self.kind().map(|kind| kind.provider())
    }

    /// 此提供商 API 密钥的环境变量候选项。
    #[must_use]
    pub fn env_vars(self) -> &'static [&'static str] {
        self.metadata().map_or(
            codewhale_config::ProviderKind::Deepseek
                .provider()
                .env_vars(),
            |provider| provider.env_vars(),
        )
    }

    /// 为 UI 复制而格式化的环境变量候选项。
    #[must_use]
    pub fn env_vars_label(self) -> String {
        self.env_vars().join(" / ")
    }

    /// 为选择器/浏览界面排序的提供商列表。
    #[must_use]
    pub fn sorted_for_display() -> Vec<Self> {
        codewhale_config::provider::providers_sorted_for_display()
            .iter()
            .map(|provider| Self::from_kind(provider.kind()))
            .collect()
    }

    /// 此提供商的默认基础 URL。
    #[must_use]
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
            _ => self
                .metadata()
                .expect("ApiProvider variant missing ProviderKind metadata")
                .default_base_url(),
        }
    }

    /// 用于创建或查找凭据的官方提供商页面。
    #[must_use]
    pub fn credential_url(self) -> Option<&'static str> {
        Some(match self {
            Self::Deepseek | Self::DeepseekCN | Self::DeepseekAnthropic => {
                "https://platform.deepseek.com/api_keys"
            }
            Self::NvidiaNim => "https://build.nvidia.com/settings/api-keys",
            Self::Openai => "https://platform.openai.com/api-keys",
            Self::Atlascloud => "https://atlascloud.ai/docs/en/api-keys",
            Self::WanjieArk => "https://docs.wanjiedata.com/maas/maas-openapi-v1.html",
            Self::Volcengine => "https://console.volcengine.com/ark",
            Self::Openrouter => "https://openrouter.ai/settings/keys",
            Self::XiaomiMimo => "https://platform.xiaomimimo.com/token-plan",
            Self::Novita => "https://novita.ai/docs/guides/quickstart",
            Self::Fireworks => "https://fireworks.ai/account/api-keys",
            Self::Siliconflow | Self::SiliconflowCn => "https://cloud.siliconflow.com/account/ak",
            Self::Arcee => "https://docs.arcee.ai/other/create-your-first-api-key",
            Self::Moonshot => "https://platform.kimi.ai/",
            Self::Huggingface => "https://huggingface.co/settings/tokens",
            Self::Together => "https://api.together.ai/settings/api-keys",
            Self::Qianfan => "https://console.bce.baidu.com/iam/#/iam/accesslist",
            Self::Anthropic => "https://console.anthropic.com/settings/keys",
            Self::Openmodel => "https://docs.openmodel.ai/en/docs/guides/api-key",
            Self::Zai => "https://z.ai/model-api",
            Self::Stepfun => "https://platform.stepfun.ai/",
            Self::Minimax => "https://platform.minimax.io/docs/guides/quickstart-preparation",
            Self::Deepinfra => "https://deepinfra.com/dash/api_keys",
            Self::Sakana => "https://api.sakana.ai/",
            Self::LongCat => "https://longcat.chat/platform",
            Self::Meta => "https://developer.meta.com/ai/",
            Self::Xai => "https://console.x.ai/",
            Self::OpenaiCodex | Self::Sglang | Self::Vllm | Self::Ollama => return None,
            // 自定义端点没有规范的凭据页面；用户通过自己的 `api_key_env` 提供密钥。
            Self::Custom => return None,
        })
    }

    /// 所有提供商，按稳定的 `ProviderKind::ALL` 顺序。
    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::FROM_KIND_LOOKUP
    }

    /// `ApiProvider` 判别式 → `ProviderKind` 查找表。
    /// 索引 1 为遗留的 `DeepseekCN` 变体，值为 `None`。
    const KIND_LOOKUP: [Option<codewhale_config::ProviderKind>; 34] = [
        Some(codewhale_config::ProviderKind::Deepseek),
        None, // DeepseekCN
        Some(codewhale_config::ProviderKind::DeepseekAnthropic),
        Some(codewhale_config::ProviderKind::NvidiaNim),
        Some(codewhale_config::ProviderKind::Openai),
        Some(codewhale_config::ProviderKind::Atlascloud),
        Some(codewhale_config::ProviderKind::WanjieArk),
        Some(codewhale_config::ProviderKind::Volcengine),
        Some(codewhale_config::ProviderKind::Openrouter),
        Some(codewhale_config::ProviderKind::XiaomiMimo),
        Some(codewhale_config::ProviderKind::Novita),
        Some(codewhale_config::ProviderKind::Fireworks),
        Some(codewhale_config::ProviderKind::Siliconflow),
        Some(codewhale_config::ProviderKind::SiliconflowCN),
        Some(codewhale_config::ProviderKind::Arcee),
        Some(codewhale_config::ProviderKind::Moonshot),
        Some(codewhale_config::ProviderKind::Sglang),
        Some(codewhale_config::ProviderKind::Vllm),
        Some(codewhale_config::ProviderKind::Ollama),
        Some(codewhale_config::ProviderKind::Huggingface),
        Some(codewhale_config::ProviderKind::Together),
        Some(codewhale_config::ProviderKind::Qianfan),
        Some(codewhale_config::ProviderKind::OpenaiCodex),
        Some(codewhale_config::ProviderKind::Anthropic),
        Some(codewhale_config::ProviderKind::Openmodel),
        Some(codewhale_config::ProviderKind::Zai),
        Some(codewhale_config::ProviderKind::Stepfun),
        Some(codewhale_config::ProviderKind::Minimax),
        Some(codewhale_config::ProviderKind::Deepinfra),
        Some(codewhale_config::ProviderKind::Sakana),
        Some(codewhale_config::ProviderKind::LongCat),
        Some(codewhale_config::ProviderKind::Meta),
        Some(codewhale_config::ProviderKind::Xai),
        Some(codewhale_config::ProviderKind::Custom),
    ];

    /// `ProviderKind` 判别式 → `ApiProvider` 查找表。
    const FROM_KIND_LOOKUP: [Self; 33] = [
        Self::Deepseek,
        Self::DeepseekAnthropic,
        Self::NvidiaNim,
        Self::Openai,
        Self::Atlascloud,
        Self::WanjieArk,
        Self::Volcengine,
        Self::Openrouter,
        Self::XiaomiMimo,
        Self::Novita,
        Self::Fireworks,
        Self::Siliconflow,
        Self::Arcee,
        Self::SiliconflowCn,
        Self::Moonshot,
        Self::Sglang,
        Self::Vllm,
        Self::Ollama,
        Self::Huggingface,
        Self::Together,
        Self::Qianfan,
        Self::OpenaiCodex,
        Self::Anthropic,
        Self::Openmodel,
        Self::Zai,
        Self::Stepfun,
        Self::Minimax,
        Self::Deepinfra,
        Self::Sakana,
        Self::LongCat,
        Self::Meta,
        Self::Xai,
        Self::Custom,
    ];

    /// 映射到配置级别的 `ProviderKind`。
    /// 对于遗留的 `DeepseekCN` 变体返回 `None`。
    #[must_use]
    pub fn kind(self) -> Option<codewhale_config::ProviderKind> {
        Self::KIND_LOOKUP[self as usize]
    }

    /// 从配置级别的 `ProviderKind` 构造。
    #[must_use]
    pub fn from_kind(kind: codewhale_config::ProviderKind) -> Self {
        Self::FROM_KIND_LOOKUP[kind as usize]
    }

    /// 此提供商是否为自托管/本地运行时。
    ///
    /// 这些提供商无需托管认证，流量保持在用户自己的基础设施上，
    /// 因此具有本地/私有姿态。被回退链用于避免将本地/私有主提供商
    /// 静默路由到云提供商（#2574），以及被 `/provider` 仪表盘的自托管
    /// 提示使用（#3083）。添加运行时托管在用户自己基础设施上的提供商时，
    /// 请更新此列表。
    #[must_use]
    pub fn is_self_hosted(self) -> bool {
        matches!(self, Self::Sglang | Self::Vllm | Self::Ollama)
    }
}

fn normalize_subagent_provider_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            '-' | '_' | '.' | ' ' => '_',
            _ => ch,
        })
        .collect()
}

fn subagent_provider_key_matches(key: &str, provider: ApiProvider) -> bool {
    if ApiProvider::parse(key).is_some_and(|candidate| candidate == provider) {
        return true;
    }

    let normalized = normalize_subagent_provider_key(key);
    if normalized == normalize_subagent_provider_key(provider.as_str()) {
        return true;
    }

    match provider {
        ApiProvider::Deepseek => matches!(
            normalized.as_str(),
            "deepseek" | "deepseek_api" | "deepseek_official"
        ),
        ApiProvider::DeepseekCN => matches!(
            normalized.as_str(),
            "deepseek_cn" | "deepseek_china" | "deepseekcn"
        ),
        ApiProvider::DeepseekAnthropic => matches!(
            normalized.as_str(),
            "deepseek_anthropic" | "deepseek_claude" | "deepseek_anthropic_api"
        ),
        ApiProvider::Openrouter => matches!(normalized.as_str(), "openrouter" | "open_router"),
        ApiProvider::OpenaiCodex => matches!(
            normalized.as_str(),
            "openai_codex" | "codex" | "chatgpt" | "openai_chatgpt"
        ),
        ApiProvider::Anthropic => {
            matches!(
                normalized.as_str(),
                "anthropic" | "claude" | "anthropic_api"
            )
        }
        ApiProvider::Zai => matches!(
            normalized.as_str(),
            "zai"
                | "z_ai"
                | "glm"
                | "zai_glm"
                | "z_glm"
                | "zhipu"
                | "zhipuai"
                | "bigmodel"
                | "big_model"
                | "zhipu_glm"
        ),
        ApiProvider::LongCat => matches!(
            normalized.as_str(),
            "longcat" | "long_cat" | "meituan_longcat" | "meituan"
        ),
        ApiProvider::Meta => matches!(
            normalized.as_str(),
            "meta" | "meta_ai" | "meta_model_api" | "muse" | "muse_spark"
        ),
        ApiProvider::Xai => matches!(normalized.as_str(), "xai" | "x_ai" | "grok"),
        _ => false,
    }
}

// ============================================================================
// 提供商能力矩阵
// ============================================================================

/// 提供商 + 已解析模型组合的已知能力。
///
/// 由 [`provider_capability`] 返回，描述给定提供商对
/// 已解析模型字符串的支持情况。所有字段均来自静态知识
///（发布文档、API 指南），而非实时 API 探测。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ProviderCapability {
    /// 规范的提供商标识符。
    pub provider: ApiProvider,
    /// 将发送到 API 负载中的已解析模型标识符。
    pub resolved_model: String,
    /// 上下文窗口（token 数，模型能接受的最大输入）。
    pub context_window: u32,
    /// 此组合的官方最大输出 token 数。
    ///
    /// 这是用于诊断和 CI 策略的模型元数据。正常的轮次使用
    /// 引擎中单独的、更保守的请求上限。
    pub max_output: u32,
    /// 提供商+模型是否支持思考/推理模式。
    pub thinking_supported: bool,
    /// 提供商是否返回提示缓存遥测字段。
    pub cache_telemetry_supported: bool,
    /// 提供商使用哪种请求负载方言。
    pub request_payload_mode: RequestPayloadMode,
    /// 仍被接受的兼容性别名的弃用元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_deprecation: Option<ModelAliasDeprecation>,
}

pub const DEEPSEEK_ALIAS_RETIREMENT_DATE: &str = "2026-07-24";
pub const DEEPSEEK_ALIAS_RETIREMENT_UTC: &str = "2026-07-24T15:59:00Z";
pub const DEEPSEEK_ALIAS_REPLACEMENT: &str = "deepseek-v4-flash";

/// 仍保持兼容的模型别名的上游退役元数据。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelAliasDeprecation {
    pub alias: String,
    pub replacement: String,
    pub retirement_date: String,
    pub retirement_utc: String,
    pub notice: String,
}

/// 提供商使用哪种请求负载方言。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum RequestPayloadMode {
    /// 标准 OpenAI 兼容的 `/v1/chat/completions` 负载。
    ChatCompletions,
    /// OpenAI Responses API 负载。
    Responses,
    /// 原生 Anthropic Messages API `/v1/messages` 负载（#3014）。
    AnthropicMessages,
}

/// 解析给定 [`ApiProvider`] 和已解析模型字符串的提供商能力。
///
/// `resolved_model` 应是在 API 负载中出现的最终模型标识符
///（经过规范化/提供商特定映射之后）。
#[must_use]
pub fn provider_capability(provider: ApiProvider, resolved_model: &str) -> ProviderCapability {
    if matches!(provider, ApiProvider::Anthropic | ApiProvider::Openmodel) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            // 200K is the conservative Anthropic floor; 4.6+ models resolve
            // their 1M windows from models.rs rows (#3014).
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(200_000),
            max_output: crate::models::max_output_tokens_for_model(resolved_model)
                .unwrap_or(64_000),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: matches!(provider, ApiProvider::Anthropic),
            request_payload_mode: RequestPayloadMode::AnthropicMessages,
            alias_deprecation: None,
        };
    }

    if matches!(provider, ApiProvider::OpenaiCodex) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS,
            max_output: crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096),
            thinking_supported: true,
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::Responses,
            alias_deprecation: None,
        };
    }

    // #3023：删除 Openai/Atlascloud/Moonshot 的提前返回，以便这些
    // 提供商使用下方通用的基于模型的路径，该路径从 models.rs 查找中
    // 正确解析上下文窗口、输出限制和思考支持。Ollama 也会回退到
    // 基于模型的查找，使用 8192 作为最后的回退值，而非硬编码的下限。
    if matches!(provider, ApiProvider::XiaomiMimo) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
            max_output: crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    if matches!(provider, ApiProvider::Arcee) {
        return ProviderCapability {
            provider,
            resolved_model: resolved_model.to_string(),
            context_window: crate::models::context_window_for_model(resolved_model)
                .unwrap_or(crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
            max_output: crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096),
            thinking_supported: crate::models::model_supports_reasoning(resolved_model),
            cache_telemetry_supported: false,
            request_payload_mode: RequestPayloadMode::ChatCompletions,
            alias_deprecation: None,
        };
    }

    let model_lower = resolved_model.to_ascii_lowercase();
    let alias_deprecation = if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) {
        deepseek_alias_deprecation(&model_lower)
    } else {
        None
    };
    let is_v4_pro = model_lower.contains("v4-pro") || model_lower == "deepseek-v4pro";
    let is_v4_flash = model_lower.contains("v4-flash")
        || model_lower == "deepseek-v4flash"
        || model_lower == "deepseek-v4"
        || alias_deprecation.is_some();
    let is_reasoner = matches!(provider, ApiProvider::WanjieArk)
        && (model_lower.contains("reasoner") || model_lower.contains("r1"));

    // 上下文窗口：V4 类模型获得 1M，其他所有模型回退到
    // 模型自身的查找或默认值。Ollama 默认为 8192
    //（对于小型本地模型较为保守），而非 128K。
    let context_window = if is_v4_pro || is_v4_flash {
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    } else if let Some(window) = crate::models::context_window_for_model(resolved_model) {
        window
    } else if matches!(provider, ApiProvider::Ollama) {
        8192
    } else {
        crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
    };

    // 最大输出 token 数：官方 DeepSeek V4 API 元数据列出 384K；
    // 运行时请求上限保持独立且更为保守。
    let max_output = if is_v4_pro || is_v4_flash {
        384_000
    } else {
        crate::models::max_output_tokens_for_model(resolved_model).unwrap_or(4096)
    };

    // 思考支持：V4 模型在所有提供商上都支持思考，但
    // 仅当模型名称匹配 V4 系列时。
    let thinking_supported = is_v4_pro
        || is_v4_flash
        || is_reasoner
        || crate::models::model_supports_reasoning(resolved_model);

    // 缓存遥测：仅由 DeepSeek 原生和 NVIDIA NIM 端点返回。
    let cache_telemetry_supported = matches!(
        provider,
        ApiProvider::Deepseek
            | ApiProvider::DeepseekCN
            | ApiProvider::NvidiaNim
            | ApiProvider::Volcengine
    );

    let request_payload_mode = if matches!(
        provider,
        ApiProvider::DeepseekAnthropic | ApiProvider::Openmodel
    ) {
        RequestPayloadMode::AnthropicMessages
    } else {
        RequestPayloadMode::ChatCompletions
    };

    ProviderCapability {
        provider,
        resolved_model: resolved_model.to_string(),
        context_window,
        max_output,
        thinking_supported,
        cache_telemetry_supported,
        request_payload_mode,
        alias_deprecation,
    }
}

fn deepseek_alias_deprecation(model_lower: &str) -> Option<ModelAliasDeprecation> {
    match model_lower {
        "deepseek-chat" | "deepseek-reasoner" => Some(ModelAliasDeprecation {
            alias: model_lower.to_string(),
            replacement: DEEPSEEK_ALIAS_REPLACEMENT.to_string(),
            retirement_date: DEEPSEEK_ALIAS_RETIREMENT_DATE.to_string(),
            retirement_utc: DEEPSEEK_ALIAS_RETIREMENT_UTC.to_string(),
            notice: format!(
                "{model_lower} is a compatibility alias for {DEEPSEEK_ALIAS_REPLACEMENT} and is scheduled to retire on {DEEPSEEK_ALIAS_RETIREMENT_DATE}."
            ),
        }),
        _ => None,
    }
}

/// 将紧凑的 DeepSeek 模型别名规范化为稳定的 ID。
///
/// 已有效的模型 ID 保持原样。只有紧凑的
/// `v4pro`/`v4flash` 拼写会被重写为带连字符的形式。
#[must_use]
pub fn canonical_model_name(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "pro" | "deepseek-v4pro" => Some("deepseek-v4-pro"),
        "flash" | "deepseek-v4flash" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

/// 规范化已配置/运行时的模型名称。
///
/// 去除空白，对已有效的模型 ID 保留调用者提供的大小写，
/// 仅规范化像 `deepseek-v4pro` 这样的紧凑别名。
/// 非 DeepSeek 或格式错误的名称返回 `None`；DeepSeek 的 `/v1/models`
/// 端点是有效模型 ID 的权威来源。
#[must_use]
pub fn normalize_model_name(model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(canonical) = canonical_model_name(trimmed) {
        return Some(canonical.to_string());
    }

    let normalized = trimmed.to_ascii_lowercase();
    if !normalized.starts_with("deepseek") && !normalized.contains("/deepseek") {
        return None;
    }

    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
    {
        return Some(trimmed.to_string());
    }

    None
}

#[must_use]
pub(crate) fn normalize_custom_model_id(model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 根据活跃提供商验证用户请求的模型 ID（#3018）。
///
/// DeepSeek 提供商使用严格的 `normalize_model_name` 门控（官方
/// API 只接受 DeepSeek ID）。所有其他提供商允许任何非空、
/// 非控制字符的字符串通过——提供商 API 是权威来源。
#[must_use]
pub fn requested_model_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => {
            normalize_model_name(model)
        }
        _ => normalize_custom_model_id(model),
    }
}

/// 拒绝我们确信无效的提供商/模型组合，*在*到达网络之前（#3227）。
///
/// 路由隔离错误会将在一个提供商下选择的模型与另一个
/// 提供商的路由配对（模型芯片 `deepseek-v4-pro`，提供商徽章
/// `Z.ai`），导致上游返回 `400 Unknown Model`。此守卫
/// 在本地捕获该问题并命名不兼容的对。
///
/// 我们只拒绝*已知*错误的组合，因此合法的自定义
/// 路由（自托管端点、代理 DeepSeek 权重的 OpenAI 兼容聚合器等）
/// 保持正常工作：
///
/// 1. DeepSeek 原生提供商（`deepseek` / `deepseek-cn`）仅接受
///    DeepSeek 模型 ID 或 `auto`——与 [`normalize_model_name`] 相同的门控。
/// 2. 非 DeepSeek *原生*提供商（例如提供 GLM 的 Z.ai）不能
///    被赋予仅 DeepSeek 的模型 ID。这复用了模型解析器使用的
///    “对直接提供商来说外来”分类，因此 DeepSeek 聚合器
///    （NVIDIA NIM、OpenRouter、Fireworks 等）保持宽松。
///
/// 对我们无法确信拒绝的任何组合返回 `Ok(())`（提供商
/// API 仍然是对这些组合的最终权威）。
pub fn validate_route(provider: ApiProvider, model: &str) -> Result<(), String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "No model selected for provider '{}'.",
            provider.as_str()
        ));
    }
    if trimmed.eq_ignore_ascii_case("auto") {
        return Ok(());
    }

    // 模型 ID 按原样传递的提供商（OpenAI 兼容、Ollama 标签、
    // 自定义基础 URL 等）由上游服务验证。
    if provider_passes_model_through(provider) {
        return Ok(());
    }

    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        if normalize_model_name(trimmed).is_some() {
            return Ok(());
        }
        return Err(format!(
            "Model '{trimmed}' is not a DeepSeek model, but the active provider is '{}'. \
             Use a DeepSeek model id (for example {}) or switch providers together with the model.",
            provider.as_str(),
            COMMON_DEEPSEEK_MODELS.join(", ")
        ));
    }

    // 非 DeepSeek 原生提供商被赋予了仅 DeepSeek 的模型 ID：这正是
    // #3227 中的污染问题（Z.ai + deepseek-v4-pro）。
    if root_deepseek_model_is_foreign_to_direct_provider(provider, trimmed) {
        return Err(format!(
            "Model '{trimmed}' is a DeepSeek model and is not compatible with provider '{}'. \
             Switch the provider and model together, or pick a model this provider serves.",
            provider.as_str()
        ));
    }

    Ok(())
}

fn canonical_official_deepseek_model_id(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-v4-pro"
        | "deepseek-v4pro"
        | "deepseek-ai/deepseek-v4-pro"
        | "deepseek-ai/deepseek-v4pro"
        | "deepseek/deepseek-v4-pro"
        | "deepseek/deepseek-v4pro" => Some("deepseek-v4-pro"),
        "deepseek-v4-flash"
        | "deepseek-v4flash"
        | "deepseek-ai/deepseek-v4-flash"
        | "deepseek-ai/deepseek-v4flash"
        | "deepseek/deepseek-v4-flash"
        | "deepseek/deepseek-v4flash" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

fn canonical_openrouter_recent_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL
        | "trinity"
        | "trinity-large-thinking"
        | "arcee-trinity"
        | "arcee-trinity-large-thinking" => Some(OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL),
        OPENROUTER_GEMMA_4_31B_MODEL | "gemma-4-31b" | "gemma-4-31b-it" => {
            Some(OPENROUTER_GEMMA_4_31B_MODEL)
        }
        OPENROUTER_GEMMA_4_26B_A4B_MODEL | "gemma-4-26b-a4b" | "gemma-4-26b-a4b-it" => {
            Some(OPENROUTER_GEMMA_4_26B_A4B_MODEL)
        }
        OPENROUTER_GLM_5_1_MODEL | "glm-5.1" | "glm-5-1" | "zai-glm-5.1" | "zai-glm-5-1" => {
            Some(OPENROUTER_GLM_5_1_MODEL)
        }
        OPENROUTER_GLM_5_2_MODEL | "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => {
            Some(OPENROUTER_GLM_5_2_MODEL)
        }
        OPENROUTER_GLM_5_TURBO_MODEL | "glm-5-turbo" | "glm-5turbo" | "zai-glm-5-turbo" => {
            Some(OPENROUTER_GLM_5_TURBO_MODEL)
        }
        OPENROUTER_KIMI_K2_7_CODE_MODEL
        | "kimi"
        | "kimi-k2"
        | "kimi-k2.7"
        | "kimi-k2-7"
        | "kimi-k2.7-code"
        | "kimi-k2-7-code"
        | "kimi-code"
        | "moonshot-kimi-k2.7-code"
        | "openrouter-kimi-k2.7-code" => Some(OPENROUTER_KIMI_K2_7_CODE_MODEL),
        OPENROUTER_KIMI_K2_6_MODEL | "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6" => {
            Some(OPENROUTER_KIMI_K2_6_MODEL)
        }
        OPENROUTER_MINIMAX_M3_MODEL | "minimax-m3" | "minimax-m-3" => {
            Some(OPENROUTER_MINIMAX_M3_MODEL)
        }
        OPENROUTER_MINIMAX_M2_7_MODEL
        | "minimax-2.7"
        | "minimax-2-7"
        | "minimax-m2.7"
        | "minimax-m2-7"
        | "minimax-m-2.7"
        | "minimax-m-2-7" => Some(OPENROUTER_MINIMAX_M2_7_MODEL),
        OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL
        | "nemotron-3-nano-omni"
        | "nemotron-3-nano-omni-reasoning" => Some(OPENROUTER_NEMOTRON_3_NANO_OMNI_MODEL),
        OPENROUTER_NEMOTRON_3_ULTRA_MODEL
        | "nvidia/nemotron-3-ultra"
        | "nemotron-3-ultra"
        | "nemotron-3-ultra-550b-a55b"
        | "nvidia-nemotron-3-ultra"
        | "nvidia-nemotron-3-ultra-550b-a55b" => Some(OPENROUTER_NEMOTRON_3_ULTRA_MODEL),
        OPENROUTER_QWEN_3_6_35B_A3B_MODEL
        | "qwen3.6-35b-a3b"
        | "qwen-3.6-35b-a3b"
        | "qwen3-6-35b-a3b" => Some(OPENROUTER_QWEN_3_6_35B_A3B_MODEL),
        OPENROUTER_QWEN_3_6_FLASH_MODEL | "qwen3.6-flash" | "qwen-3.6-flash" => {
            Some(OPENROUTER_QWEN_3_6_FLASH_MODEL)
        }
        OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL
        | "qwen3.6-max-preview"
        | "qwen-3.6-max-preview"
        | "qwen-max-preview" => Some(OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL),
        OPENROUTER_QWEN_3_6_27B_MODEL | "qwen3.6-27b" | "qwen-3.6-27b" | "qwen3-6-27b" => {
            Some(OPENROUTER_QWEN_3_6_27B_MODEL)
        }
        OPENROUTER_QWEN_3_6_PLUS_MODEL | "qwen3.6-plus" | "qwen-3.6-plus" => {
            Some(OPENROUTER_QWEN_3_6_PLUS_MODEL)
        }
        OPENROUTER_QWEN_3_7_MAX_MODEL | "qwen3.7-max" | "qwen-3.7-max" => {
            Some(OPENROUTER_QWEN_3_7_MAX_MODEL)
        }
        OPENROUTER_TENCENT_HY3_PREVIEW_MODEL | "hy3-preview" | "tencent-hy3-preview" => {
            Some(OPENROUTER_TENCENT_HY3_PREVIEW_MODEL)
        }
        OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL
        | "mimo-v2.5-pro"
        | "mimo-v2-5-pro"
        | "xiaomi-mimo-v2.5-pro"
        | "xiaomi-mimo-v2-5-pro" => Some(OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL),
        OPENROUTER_XIAOMI_MIMO_V2_5_MODEL
        | "mimo-v2.5"
        | "mimo-v2-5"
        | "xiaomi-mimo-v2.5"
        | "xiaomi-mimo-v2-5" => Some(OPENROUTER_XIAOMI_MIMO_V2_5_MODEL),
        _ => None,
    }
}

fn canonical_xiaomi_mimo_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "mimo"
        | DEFAULT_XIAOMI_MIMO_MODEL
        | "mimo-v2-5-pro"
        | "xiaomi-mimo-v2.5-pro"
        | "xiaomi-mimo-v2-5-pro" => Some(DEFAULT_XIAOMI_MIMO_MODEL),
        XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL
        | "mimo-v2-5-pro-ultraspeed"
        | "xiaomi-mimo-v2.5-pro-ultraspeed"
        | "xiaomi-mimo-v2-5-pro-ultraspeed"
        | "ultraspeed"
        | "pro-ultraspeed" => Some(XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL),
        "omni"
        | "mimo-omni"
        | "v2.5-omni"
        | "v25-omni"
        | "mimo-v2.5"
        | "mimo-v25"
        | "mimo-v2-5"
        | "mimo-v2.5-omni"
        | "mimo-v25-omni"
        | "mimo-v2-5-omni"
        | "xiaomi-mimo-v2.5"
        | "xiaomi-mimo-v2-5"
        | "xiaomi-mimo-v2.5-omni"
        | "xiaomi-mimo-v2-5-omni" => Some(XIAOMI_MIMO_V2_5_OMNI_MODEL),
        "asr" | "mimo-asr" | "mimo-v2.5-asr" | "speech-to-text" | "transcribe" => {
            Some(XIAOMI_MIMO_ASR_MODEL)
        }
        "mimo-tts" | "mimo-v25-tts" | "mimo-v2.5-tts" | "tts" | "speech" => {
            Some(XIAOMI_MIMO_TTS_MODEL)
        }
        "mimo-tts-voicedesign"
        | "mimo-voice-design"
        | "mimo-v25-tts-voicedesign"
        | "mimo-v2.5-tts-voicedesign"
        | "voicedesign"
        | "voice-design" => Some(XIAOMI_MIMO_TTS_VOICE_DESIGN_MODEL),
        "mimo-tts-voiceclone"
        | "mimo-voice-clone"
        | "mimo-v25-tts-voiceclone"
        | "mimo-v2.5-tts-voiceclone"
        | "voiceclone"
        | "voice-clone" => Some(XIAOMI_MIMO_TTS_VOICE_CLONE_MODEL),
        "mimo-v2-tts" => Some(XIAOMI_MIMO_V2_TTS_MODEL),
        _ => None,
    }
}

fn canonical_arcee_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "trinity" | "arcee-trinity" | "trinity-large-thinking" | "arcee-trinity-large-thinking" => {
            Some(DEFAULT_ARCEE_MODEL)
        }
        "arcee-trinity-mini" | ARCEE_TRINITY_MINI_MODEL => Some(ARCEE_TRINITY_MINI_MODEL),
        "arcee-trinity-large-preview" | ARCEE_TRINITY_LARGE_PREVIEW_MODEL => {
            Some(ARCEE_TRINITY_LARGE_PREVIEW_MODEL)
        }
        _ => None,
    }
}

fn canonical_moonshot_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "kimi"
        | "kimi-k2"
        | "kimi-k2.7"
        | "kimi-k2-7"
        | "kimi-k2.7-code"
        | "kimi-k2-7-code"
        | "kimi-code"
        | "moonshot-kimi-k2.7-code" => Some(DEFAULT_MOONSHOT_MODEL),
        "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6" => Some(MOONSHOT_KIMI_K2_6_MODEL),
        _ => None,
    }
}

fn canonical_zai_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "glm-5.1" | "glm-5-1" | "zai-glm-5.1" | "zai-glm-5-1" => Some(ZAI_GLM_5_1_MODEL),
        "glm-5.2" | "glm-5-2" | "zai-glm-5.2" | "zai-glm-5-2" => Some(DEFAULT_ZAI_MODEL),
        "glm-5-turbo" | "glm-5turbo" | "zai-glm-5-turbo" => Some(ZAI_GLM_5_TURBO_MODEL),
        _ => None,
    }
}

fn canonical_minimax_model_id(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    let normalized = normalized.replace(['_', ' '], "-");
    match normalized.as_str() {
        "minimax" | "minimax-m3" | "minimax-m-3" | "minimax-m-3-thinking" => {
            Some(DEFAULT_MINIMAX_MODEL)
        }
        "minimax-m2.7" | "minimax-m2-7" | "minimax-m-2.7" | "minimax-m-2-7" => {
            Some(MINIMAX_M2_7_MODEL)
        }
        "minimax-m2.7-highspeed"
        | "minimax-m2-7-highspeed"
        | "minimax-m-2.7-highspeed"
        | "minimax-m-2-7-highspeed" => Some(MINIMAX_M2_7_HIGHSPEED_MODEL),
        "minimax-m2.5" | "minimax-m2-5" | "minimax-m-2.5" | "minimax-m-2-5" => {
            Some(MINIMAX_M2_5_MODEL)
        }
        "minimax-m2.5-highspeed"
        | "minimax-m2-5-highspeed"
        | "minimax-m-2.5-highspeed"
        | "minimax-m-2-5-highspeed" => Some(MINIMAX_M2_5_HIGHSPEED_MODEL),
        "minimax-m2.1" | "minimax-m2-1" | "minimax-m-2.1" | "minimax-m-2-1" => {
            Some(MINIMAX_M2_1_MODEL)
        }
        "minimax-m2.1-highspeed"
        | "minimax-m2-1-highspeed"
        | "minimax-m-2.1-highspeed"
        | "minimax-m-2-1-highspeed" => Some(MINIMAX_M2_1_HIGHSPEED_MODEL),
        "minimax-m2" | "minimax-m-2" => Some(MINIMAX_M2_MODEL),
        _ => None,
    }
}

/// 将用户输入的模型 ID 解析为提供商理解的规范系列 ID，
/// 不进行任何有线 ID 转换。
///
/// 模型系列被平等对待：每个提供商拥有的系列（GLM 通过
/// Z.ai/Zhipu、Kimi、Xiaomi MiMo、MiniMax、Arcee、OpenRouter 别名等）
/// 都通过相同的“应用系列的规范映射，否则直接传递输入”路径解析。
/// 没有任何东西仅因为不是 DeepSeek ID 而被拒绝——上游 API 仍然是
/// 最终权威，反映了 models.dev 目录（路由解析器的真相来源）
/// 如何为每个产品携带一个权威 ID，无论供应商如何。
///
/// 这是 [`normalize_model_name_for_provider`] 曾经融合在一起的
/// 规范化的那一半。有线 ID 转换（例如 `deepseek-v4-pro` →
/// 聚合器的 `accounts/…/deepseek-v4-pro` 别名）属于请求时的路由解析器，
/// 而不是输入到 `/provider` 的名称，因此被刻意排除在此之外。
///
/// 仅对空输入或控制字符输入返回 `None`；所有其他 ID
/// 都通过，因此自定义/自托管端点永远不会被错误拒绝。
#[must_use]
pub fn canonical_model_id_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }

    // 提供商拥有的模型系列通过它们自己的规范映射解析，
    // 该映射定义了权威的大小写（`glm-5.1` → `GLM-5.1`，
    // `minimax-m2.7` → `MiniMax-M2.7`）。每个映射只识别*自己的*
    // 别名，因此未知 ID 会通过传递——没有系列充当
    // 针对其他系列的门控。
    let family_canonical: Option<&'static str> = match provider {
        ApiProvider::Openrouter => canonical_openrouter_recent_model_id(trimmed),
        ApiProvider::XiaomiMimo => canonical_xiaomi_mimo_model_id(trimmed),
        ApiProvider::Arcee => canonical_arcee_model_id(trimmed),
        ApiProvider::Moonshot => canonical_moonshot_model_id(trimmed),
        ApiProvider::Zai => canonical_zai_model_id(trimmed),
        ApiProvider::Minimax => canonical_minimax_model_id(trimmed),
        _ => None,
    };
    if let Some(canonical) = family_canonical {
        return Some(canonical.to_string());
    }

    // 官方 DeepSeek API 是唯一合法的按系列门控：它只服务
    // 自己的 ID（对其他任何内容返回 400），因此拒绝它不
    // 识别的 ID。紧凑别名被重写（deepseek-v4pro → deepseek-v4-pro），
    // 对已有效的 ID 保留调用者的大小写（`DeepSeek-V4-Flash`
    // 保持原样）。自定义/自托管 DeepSeek 端点走
    // 接受自定义模型 ID 的路径，因此它们永远不会到达此门控。
    if matches!(
        provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
    ) {
        let normalized = normalize_model_name(trimmed)?;
        if let Some(canonical) = canonical_official_deepseek_model_id(&normalized) {
            if canonical.eq_ignore_ascii_case(&normalized)
                || normalized.to_ascii_lowercase() == canonical
            {
                return Some(normalized);
            }
            return Some(canonical.to_string());
        }
        return Some(normalized);
    }

    // 托管 DeepSeek 的聚合器（NIM、Novita、Fireworks、SiliconFlow、SGLang、
    // vLLM、DeepInfra、Wanjie Ark、Volcengine）将识别的 DeepSeek ID 规范化，
    // 但让其他所有内容通过——它们服务的不仅仅是 DeepSeek，因此
    // 上游 API 仍然是权威。名称在此处永远不会被拒绝。
    if matches!(
        provider,
        ApiProvider::NvidiaNim
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Deepinfra
            | ApiProvider::WanjieArk
            | ApiProvider::Volcengine
    ) && let Some(canonical) = canonical_official_deepseek_model_id(
        &normalize_model_name(trimmed).unwrap_or_else(|| trimmed.to_string()),
    ) {
        return Some(canonical.to_string());
    }

    // 其他所有内容（HuggingFace、OpenAI 兼容、Qianfan、StepFun、Codex、
    // Anthropic）没有规范映射——用户输入的 ID 是权威的。
    Some(trimmed.to_string())
}

/// 规范化为活跃提供商通过 TUI 选择的模型，在规范系列 ID 之上
/// 应用提供商的有线别名转换。
///
/// 这是拆分的有关线 ID 的一半（规范化位于
/// [`canonical_model_id_for_provider`]）。用于配置文件规范化，
/// 其中供应商前缀的 ID（例如 SiliconFlow 上的 `deepseek-ai/DeepSeek-V4-Pro`）
/// 是存储形式。`/provider` 刻意使用规范的那一半。
#[must_use]
pub fn normalize_model_name_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    let canonical = canonical_model_id_for_provider(provider, model)?;
    // 当提供商的 API 使用供应商前缀的 ID（Together、Siliconflow、NIM 等）时，
    // 将规范系列 ID 转换为提供商的有线别名。
    // 对于没有有线别名映射的提供商，`model_for_provider` 是无操作的，
    // 因此这是在平等对待的规范解析器之上的一个统一层。
    Some(model_for_provider(provider, canonical))
}

#[must_use]
pub fn wire_model_for_provider(provider: ApiProvider, model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    if matches!(provider, ApiProvider::XiaomiMimo) {
        return normalize_model_name_for_provider(provider, trimmed)
            .unwrap_or_else(|| trimmed.to_string());
    }
    if provider_passes_model_through(provider) {
        return trimmed.to_string();
    }
    normalize_model_name_for_provider(provider, trimmed).unwrap_or_else(|| trimmed.to_string())
}

/// 硬编码的按提供商模型 ID 列表，**仅作为兼容性回退**使用（#4188）。
///
/// 首选来源是实时 Models.dev 目录和通过 [`crate::provider_lake`] 的
/// 离线捆绑快照。仅对 Models.dev 不代表的仅 CodeWhale/本地提供商，
/// 或在测试中探测回退表时，直接调用此函数。
/// 选择器、库存和子代理界面必须通过 provider lake。
#[must_use]
pub fn model_completion_names_for_provider(provider: ApiProvider) -> Vec<&'static str> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => {
            OFFICIAL_DEEPSEEK_MODELS.to_vec()
        }
        ApiProvider::NvidiaNim => vec![DEFAULT_NVIDIA_NIM_MODEL, DEFAULT_NVIDIA_NIM_FLASH_MODEL],
        ApiProvider::Openrouter => {
            let mut models = vec![DEFAULT_OPENROUTER_MODEL, DEFAULT_OPENROUTER_FLASH_MODEL];
            models.extend_from_slice(RECENT_OPENROUTER_LARGE_MODELS);
            models
        }
        ApiProvider::XiaomiMimo => vec![
            DEFAULT_XIAOMI_MIMO_MODEL,
            XIAOMI_MIMO_V2_5_PRO_ULTRASPEED_MODEL,
            XIAOMI_MIMO_V2_5_OMNI_MODEL,
        ],
        ApiProvider::Novita => vec![DEFAULT_NOVITA_MODEL, DEFAULT_NOVITA_FLASH_MODEL],
        ApiProvider::Fireworks => vec![DEFAULT_FIREWORKS_MODEL],
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => {
            vec![DEFAULT_SILICONFLOW_MODEL, DEFAULT_SILICONFLOW_FLASH_MODEL]
        }
        ApiProvider::Arcee => vec![DEFAULT_ARCEE_MODEL, ARCEE_TRINITY_LARGE_PREVIEW_MODEL],
        ApiProvider::Moonshot => vec![DEFAULT_MOONSHOT_MODEL],
        ApiProvider::Huggingface => {
            vec![DEFAULT_HUGGINGFACE_MODEL, DEFAULT_HUGGINGFACE_FLASH_MODEL]
        }
        ApiProvider::Deepinfra => vec![DEFAULT_DEEPINFRA_MODEL, DEFAULT_DEEPINFRA_FLASH_MODEL],
        ApiProvider::WanjieArk => {
            vec![
                DEFAULT_WANJIE_ARK_MODEL,
                "deepseek-v4-pro",
                "deepseek-v4-flash",
            ]
        }
        ApiProvider::Sglang => vec![DEFAULT_SGLANG_MODEL, DEFAULT_SGLANG_FLASH_MODEL],
        ApiProvider::Vllm => vec![DEFAULT_VLLM_MODEL, DEFAULT_VLLM_FLASH_MODEL],
        ApiProvider::Volcengine => vec![DEFAULT_VOLCENGINE_MODEL, DEFAULT_VOLCENGINE_FLASH_MODEL],
        ApiProvider::Ollama => Vec::new(),
        ApiProvider::Openai | ApiProvider::Atlascloud => OFFICIAL_DEEPSEEK_MODELS.to_vec(),
        ApiProvider::Together => vec![DEFAULT_TOGETHER_MODEL, DEFAULT_TOGETHER_FLASH_MODEL],
        ApiProvider::Qianfan => vec![DEFAULT_QIANFAN_MODEL],
        ApiProvider::OpenaiCodex => vec![DEFAULT_OPENAI_CODEX_MODEL],
        ApiProvider::Openmodel => vec![DEFAULT_OPENMODEL_MODEL],
        ApiProvider::Zai => vec![DEFAULT_ZAI_MODEL, ZAI_GLM_5_1_MODEL, ZAI_GLM_5_TURBO_MODEL],
        ApiProvider::Stepfun => vec![DEFAULT_STEPFUN_MODEL],
        ApiProvider::Anthropic => vec![
            ANTHROPIC_OPUS_MODEL,
            DEFAULT_ANTHROPIC_MODEL,
            ANTHROPIC_HAIKU_MODEL,
        ],
        ApiProvider::Minimax => vec![
            DEFAULT_MINIMAX_MODEL,
            MINIMAX_M2_7_MODEL,
            MINIMAX_M2_7_HIGHSPEED_MODEL,
            MINIMAX_M2_5_MODEL,
            MINIMAX_M2_5_HIGHSPEED_MODEL,
            MINIMAX_M2_1_MODEL,
            MINIMAX_M2_1_HIGHSPEED_MODEL,
            MINIMAX_M2_MODEL,
        ],
        ApiProvider::Sakana => vec![DEFAULT_SAKANA_MODEL, SAKANA_FUGU_ULTRA_MODEL],
        ApiProvider::LongCat => vec![DEFAULT_LONGCAT_MODEL],
        ApiProvider::Meta => vec![DEFAULT_META_MODEL],
        ApiProvider::Xai => vec![
            DEFAULT_XAI_MODEL,
            XAI_GROK_4_3_MODEL,
            XAI_GROK_BUILD_MODEL,
            XAI_GROK_COMPOSER_2_5_FAST_MODEL,
            XAI_GROK_4_20_0309_REASONING_MODEL,
            XAI_GROK_4_20_0309_NON_REASONING_MODEL,
        ],
        // 自定义端点不暴露内置的完成名称；用户
        // 提供自己的模型 ID（#1519）。
        ApiProvider::Custom => Vec::new(),
    }
}

// === 类型 ===

/// 从配置文件加载的原始重试配置。
#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    pub enabled: Option<bool>,
    pub max_retries: Option<u32>,
    pub initial_delay: Option<f64>,
    pub max_delay: Option<f64>,
    pub exponential_base: Option<f64>,
}

/// 宽容地反序列化 `status_items`：跳过此构建不认识的键，
/// 而不是以"未知变体"报错。这让开发构建写入
/// `"balance"`（或任何未来项目），而稳定构建仍然能成功解析配置文件。
fn deser_status_items<'de, D>(deserializer: D) -> Result<Option<Vec<StatusItem>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Vec<String>> = Option::deserialize(deserializer)?;
    Ok(raw.map(|strings| {
        strings
            .into_iter()
            .filter_map(|s| {
                StatusItem::from_key(&s).or_else(|| {
                    tracing::warn!("ignoring unknown status item {s:?} in config");
                    None
                })
            })
            .collect()
    }))
}

/// 从配置文件加载的 UI 配置。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TuiConfig {
    pub alternate_screen: Option<String>,
    pub mouse_capture: Option<bool>,
    /// 启动终端模式/探测调用的超时时间（毫秒）。
    /// 省略时默认为 500ms。
    pub terminal_probe_timeout_ms: Option<u64>,
    /// 每个 SSE 块的空闲超时时间（秒）。省略时默认为 900 秒。
    /// `0` 映射为默认值；值限制在 `1..=3600`。
    pub stream_chunk_timeout_secs: Option<u64>,
    /// Ordered list of footer items the user wants visible. `None` (the field
    /// missing from `config.toml`) means "use the built-in default order"; an
    /// empty `Some(vec![])` means "show nothing in the footer".
    ///
    /// Edited interactively via `/statusline`; persisted to `tui.status_items`
    /// in `~/.deepseek/config.toml`.
    #[serde(default, deserialize_with = "deser_status_items")]
    pub status_items: Option<Vec<StatusItem>>,
    /// 在记录中的 URL 周围发出 OSC 8 超链接转义序列，以便
    /// 支持的终端（iTerm2、Terminal.app 13+、Ghostty、Kitty、
    /// WezTerm、Alacritty、最新的 gnome-terminal/konsole）使它们可以
    /// Cmd+点击打开。不支持 OSC 8 的终端渲染纯文本
    /// 标签并忽略转义。macOS/Linux 默认为开启，Windows 旧版控制台
    /// 默认为关闭；设为 `false` 可在所有位置禁用（例如对于
    /// 错误渲染该序列的终端）。OSC 8 转义是带外发出的，
    /// 因此不用担心缓冲区列损坏。
    pub osc8_links: Option<bool>,
    /// 高级通知触发条件。设置后，覆盖较低级别的
    /// `[notifications]` 块中的 `[notifications].threshold_secs` 门控：
    ///
    /// - `Always` — 每次成功的轮次都触发轮次完成通知，
    ///   无论持续时间如何。仍尊重已配置的 `[notifications].method`
    ///   和 `include_summary` 标志。
    /// - `Never` — 抑制所有轮次完成通知。
    /// - 未设置（默认）— 回退到 `[notifications]` 默认值。
    pub notification_condition: Option<NotificationCondition>,
    /// 当为 `true` 时，空编辑器上的 Up/Down 滚动记录
    /// 而非调出输入历史。对于将鼠标滚轮手势映射到
    /// 方向键的终端很有用。默认：仅当鼠标捕获关闭时为 `true`；
    /// 否则为 `false`。
    #[serde(default)]
    pub composer_arrows_scroll: Option<bool>,
}

/// 高级通知触发覆盖。参见
/// [`TuiConfig::notification_condition`]。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCondition {
    /// 每次成功的轮次都通知（无持续时间阈值）。
    Always,
    /// 完全抑制通知。
    Never,
}

/// 通知传递方法（镜像 `tui::notifications::Method`）。
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationMethod {
    /// 自动检测：为当前终端选择最佳协议
    ///（OSC 9、Kitty OSC 99、Ghostty OSC 777 或 Bel）。
    #[default]
    Auto,
    /// OSC 9 转义。
    Osc9,
    /// 纯 BEL 字符。
    Bel,
    /// Kitty 通知协议（OSC 99）。
    Kitty,
    /// Ghostty 通知协议（OSC 777）。
    Ghostty,
    /// 禁用通知。
    Off,
}

fn default_threshold_secs() -> u64 {
    30
}

/// 完成声音选项。
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionSound {
    /// 轮次完成时无声音。
    Off,
    /// 系统通知蜂鸣声（默认）。在 Windows 上使用 `MessageBeep`。
    #[default]
    Beep,
    /// 终端 BEL 字符（`\x07`）。
    Bell,
    /// 播放已配置的 WAV 声音文件。
    File,
}

/// 控制在 fleet/工作流运行期间每个子代理完成通知的触发时机。
/// 轮次完成通知不受影响。
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentCompletionNotification {
    /// 每个子代理完成时都通知。
    Always,
    /// 仅当批次中最后一个子代理完成时通知——没有其他子代理
    /// 在运行且没有工作流在进行中。默认：运行中保持安静，
    /// 在 fleet 耗尽时触发一次。
    #[default]
    FinalOnly,
    /// 从不触发子代理完成通知。
    Off,
}

/// 桌面通知配置（轮次完成时的 OSC 9 / BEL）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NotificationsConfig {
    /// 传递方法：`auto` | `osc9` | `bel` | `off`。默认：`auto`。
    /// `auto` 为 iTerm.app / Ghostty / WezTerm / Cmux 解析为 OSC 9
    ///（通过 `$TERM_PROGRAM` 然后是 `$LC_TERMINAL` 检测）；否则
    /// 回退到 BEL。在 Windows 上，BEL 路径通过 `MessageBeep(MB_OK)` 路由。
    /// 当您的终端支持 OSC-9 但未设置这两个环境变量时，
    /// 显式使用 `method = "osc9"`（例如没有 `LC_TERMINAL` 的 Cmux）。
    #[serde(default)]
    pub method: NotificationMethod,
    /// 仅当轮次至少花了这么多秒时才通知。默认：30。
    #[serde(default = "default_threshold_secs")]
    pub threshold_secs: u64,
    /// 在通知正文中包含简短摘要（经过时间 + 成本）。
    /// 默认：`false`。
    #[serde(default)]
    pub include_summary: bool,

    /// 在 fleet/工作流运行期间何时触发每个子代理完成通知：
    /// `always` | `final-only` | `off`。默认：`final-only`
    ///（运行中安静，批次耗尽时一个通知）。设为 `off` 可完全
    /// 静音子代理通知。
    #[serde(default)]
    pub subagent_completion: SubagentCompletionNotification,

    /// 完成声音：`"off"` | `"beep"` | `"bell"` | `"file"`。默认：`"beep"`。
    /// 每次轮次完成时播放声音（与 ✅ 标记一起）。
    #[serde(default)]
    pub completion_sound: CompletionSound,

    /// `completion_sound = "file"` 时使用的 WAV 声音文件路径。
    #[serde(default)]
    pub sound_file: Option<PathBuf>,
}

fn default_snapshots_enabled() -> bool {
    true
}

fn default_snapshot_max_age_days() -> u64 {
    crate::snapshot::DEFAULT_MAX_AGE.as_secs() / (24 * 60 * 60)
}

fn default_snapshot_max_workspace_gb() -> u64 {
    crate::snapshot::DEFAULT_MAX_WORKSPACE_BYTES_FOR_SNAPSHOT / (1024 * 1024 * 1024)
}

/// 工作区侧 git 快照配置（#137）。
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotsConfig {
    /// 在每个交互式代理轮次前后对工作区进行快照。
    #[serde(default = "default_snapshots_enabled")]
    pub enabled: bool,
    /// 在会话启动时清理早于此天数的侧 git 快照。
    #[serde(default = "default_snapshot_max_age_days")]
    pub max_age_days: u64,
    /// 快照功能在首次使用前自动禁用的最大非排除工作区大小（GB）。
    /// 设为 `0` 以禁用上限并不限大小进行快照（v0.8.31 行为）。
    /// 遍历遵循 `.gitignore` 和快照模块的内置排除项
    ///（`node_modules/`、`target/` 等），因此测量的大小反映
    /// 实际会进入快照提交的内容。
    #[serde(default = "default_snapshot_max_workspace_gb")]
    pub max_workspace_gb: u64,
}

impl Default for SnapshotsConfig {
    fn default() -> Self {
        Self {
            enabled: default_snapshots_enabled(),
            max_age_days: default_snapshot_max_age_days(),
            max_workspace_gb: default_snapshot_max_workspace_gb(),
        }
    }
}

/// 用户级记忆配置（#489）。
///
/// 默认是选择加入：当此表不存在或 `enabled = false` 时，
/// 记忆文件既不被读取也不被写入，编辑器中的 `# foo` 快速添加
/// 回退到正常的轮次提交路径。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemoryConfig {
    /// 当为 `true` 时，将 `Config::memory_path()` 处的用户记忆文件
    /// 作为 `<user_memory>` 块加载到系统提示中，并拦截
    /// 编辑器中输入的 `# foo` 以追加到该文件。默认 `false`。
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 当为 `true` 时，弃用仓库内 `memory.rs` 的推送/注入路径
    ///（`<user_memory>` 块 + `remember` 工具 + `# foo` 快速添加），
    /// 转而使用 Moraine 通过其 MCP 工具的拉取/召回。即使
    /// `enabled = true`，旧路径也会被跳过。默认 `false`。
    #[serde(default)]
    pub moraine_fallback: Option<bool>,
}

/// 小米 MiMo 语音/TTS 输出配置。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpeechConfig {
    /// 未提供明确输出路径时生成的语音/TTS 文件的默认目录。
    #[serde(default)]
    pub output_dir: Option<String>,
}

impl SnapshotsConfig {
    #[must_use]
    pub fn max_age(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.max_age_days.saturating_mul(24 * 60 * 60))
    }
}

// 网络搜索 `[search]` 表类型位于 `search` 叶子模块中，并在下方
// 重新导出，以便 `crate::config::SearchProvider`（及其同类）
// 解析保持不变（#3311）。
mod search;
pub use search::*;

/// 模型可见的工具目录控制（config.toml 中的 `[tools]` 表）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolsConfig {
    /// 即使在小默认核心目录之外也保持加载的原生工具名称。
    /// 未知名称无害，只是永远不会匹配。
    #[serde(default)]
    pub always_load: Vec<String>,

    /// 可选目录，用于扫描插件工具脚本。带有前置元数据头部
    ///（`# name:`、`# description:`、`# schema:`）的脚本会被
    /// 自动发现并注册为工具。
    ///
    /// 当为 `None` 时，默认为 `~/.codewhale/tools/`。
    #[serde(default)]
    pub plugin_dir: Option<String>,

    /// 以内置工具名称为键的按工具覆盖。
    /// 每个覆盖替换或禁用命名的工具。
    #[serde(default)]
    pub overrides: Option<HashMap<String, ToolOverride>>,
}

/// 一个可配置的页脚项目。
///
/// 用户在 `Vec<StatusItem>` 中的顺序被保留：左侧集群
///（`Mode`、`Model`、`Cost`、`Status`）按给定顺序渲染；
/// 右侧集群标签（`Agents`、`ReasoningReplay`、`PrefixStability`、
/// `Cache`、`ContextPercent`、`GitBranch`、`LastToolElapsed`、`RateLimit`）
/// 同样遵循其集群内的顺序。左右分割是刻意的——左侧持有稳定的
/// 标识（模式/模型/成本），右侧持有瞬态信号——因此我们将每个变体
/// 路由到正确的一侧，而不是让用户在间隔符之间重新排序。
///
/// 没有当前数据源的变体（`RateLimit`、`LastToolElapsed`）
/// 今天被有意暴露，以便选择器向前兼容；它们在支持字段落地之前
/// 渲染为空。空的跨度不占用页脚宽度，因此用户看不到视觉伪影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StatusItem {
    /// "agent" / "yolo" / "plan" 标签。
    Mode,
    /// 模型标识符（例如 `deepseek-v4-pro`）。
    Model,
    /// 会话成本，以配置的显示货币计。
    Cost,
    /// 活动标签："idle" / "busy" / "draft" / "working"。
    Status,
    /// 子代理计数标签（"3 agents"）。
    Agents,
    /// 推理回放 token 数（"rsn 12.3k"）。
    ReasoningReplay,
    /// 前缀稳定性（"cache prefix 100%"）。
    PrefixStability,
    /// 缓存命中率（"cache 73%"）。
    Cache,
    /// 上下文窗口使用百分比（"48%"）。
    ContextPercent,
    /// 当前 git 分支名称。
    GitBranch,
    /// 最近一次工具调用的经过时间（占位符，直到接入）。
    LastToolElapsed,
    /// 剩余速率限制预算（占位符，直到接入）。
    RateLimit,
    /// 会话 token 用量：输入 / 缓存命中 / 输出。
    Tokens,
    /// DeepSeek 账户余额，每次轮次完成时刷新。
    Balance,
}

impl StatusItem {
    /// 始终在线的状态行的默认页脚组合。当 `config.toml` 中缺少
    /// `tui.status_items` 时使用，以便升级者默认看到简洁的页脚；
    /// 诊断标签仍然可通过 `/statusline` 使用，而不会拥挤主 UI。
    #[must_use]
    pub fn default_footer() -> Vec<StatusItem> {
        vec![
            StatusItem::Mode,
            StatusItem::Model,
            StatusItem::Cost,
            StatusItem::Status,
            StatusItem::Agents,
            StatusItem::ReasoningReplay,
            StatusItem::Cache,
            StatusItem::GitBranch,
            StatusItem::Tokens,
        ]
    }

    /// 在 TOML 和选择器标签中使用的稳定规范名称。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            StatusItem::Mode => "mode",
            StatusItem::Model => "model",
            StatusItem::Cost => "cost",
            StatusItem::Status => "status",
            StatusItem::Agents => "agents",
            StatusItem::ReasoningReplay => "reasoning_replay",
            StatusItem::PrefixStability => "prefix_stability",
            StatusItem::Cache => "cache",
            StatusItem::ContextPercent => "context_percent",
            StatusItem::GitBranch => "git_branch",
            StatusItem::LastToolElapsed => "last_tool_elapsed",
            StatusItem::RateLimit => "rate_limit",
            StatusItem::Tokens => "tokens",
            StatusItem::Balance => "balance",
        }
    }

    /// [`key`](Self::key) 的逆操作：将配置字符串解析回变体。
    /// 对未知键返回 `None`，以便配置解析器可以静默跳过
    /// 新版本添加的项目，而不是以"未知变体"崩溃。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "mode" => Some(Self::Mode),
            "model" => Some(Self::Model),
            "cost" => Some(Self::Cost),
            "status" => Some(Self::Status),
            "agents" => Some(Self::Agents),
            "reasoning_replay" => Some(Self::ReasoningReplay),
            "prefix_stability" => Some(Self::PrefixStability),
            "cache" => Some(Self::Cache),
            "context_percent" => Some(Self::ContextPercent),
            "git_branch" => Some(Self::GitBranch),
            "last_tool_elapsed" => Some(Self::LastToolElapsed),
            "rate_limit" => Some(Self::RateLimit),
            "tokens" => Some(Self::Tokens),
            "balance" => Some(Self::Balance),
            _ => None,
        }
    }

    /// 选择器的人类可读标签。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StatusItem::Mode => "Mode",
            StatusItem::Model => "Model",
            StatusItem::Cost => "Session cost",
            StatusItem::Status => "Activity (idle/busy/draft/working)",
            StatusItem::Agents => "Sub-agents in flight",
            StatusItem::ReasoningReplay => "Reasoning replay tokens",
            StatusItem::PrefixStability => "Prefix stability",
            StatusItem::Cache => "Prompt cache hit rate",
            StatusItem::ContextPercent => "Context window %",
            StatusItem::GitBranch => "Git branch",
            StatusItem::LastToolElapsed => "Last tool elapsed",
            StatusItem::RateLimit => "Rate-limit remaining",
            StatusItem::Tokens => "Session tokens",
            StatusItem::Balance => "Account balance",
        }
    }

    /// 标签旁显示的单行提示，以便用户了解每个项目显示什么内容，
    /// 而无需先打开它。
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            StatusItem::Mode => "agent · yolo · plan",
            StatusItem::Model => "the model id you'll send to",
            StatusItem::Cost => "running total for this session",
            StatusItem::Status => "what the agent is doing right now",
            StatusItem::Agents => "agents or RLM work in progress",
            StatusItem::ReasoningReplay => "thinking tokens replayed each turn",
            StatusItem::PrefixStability => "whether system/tools stayed cacheable",
            StatusItem::Cache => "% of prompt served from cache",
            StatusItem::ContextPercent => "tokens used / model context window",
            StatusItem::GitBranch => "current workspace branch",
            StatusItem::LastToolElapsed => "ms of the most recent tool call (reserved)",
            StatusItem::RateLimit => "remaining requests in the budget (reserved)",
            StatusItem::Tokens => "input / cache-hit / output token totals",
            StatusItem::Balance => "topped-up + granted balance from DeepSeek",
        }
    }

    /// 按显示顺序排列的每个变体——选择器用于枚举行。
    #[must_use]
    pub fn all() -> &'static [StatusItem] {
        &[
            StatusItem::Mode,
            StatusItem::Model,
            StatusItem::Cost,
            StatusItem::Balance,
            StatusItem::Status,
            StatusItem::Agents,
            StatusItem::ReasoningReplay,
            StatusItem::PrefixStability,
            StatusItem::Cache,
            StatusItem::ContextPercent,
            StatusItem::GitBranch,
            StatusItem::LastToolElapsed,
            StatusItem::RateLimit,
            StatusItem::Tokens,
        ]
    }

    /// 属于页脚左侧集群（稳定标识）的项目。
    #[must_use]
    pub fn is_left_cluster(self) -> bool {
        matches!(
            self,
            StatusItem::Mode
                | StatusItem::Model
                | StatusItem::Cost
                | StatusItem::Status
                | StatusItem::Balance
        )
    }

    /// 此项目是否对 `provider` 相关。提供商特定的项目对不支持
    /// 的提供商返回 `false`，以便选择器不提供永远无法显示有用数据的开关。
    #[must_use]
    pub fn is_available_for(self, provider: ApiProvider) -> bool {
        match self {
            StatusItem::Balance => {
                matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            }
            _ => true,
        }
    }
}

/// 已解析的重试策略，应用了默认值。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub enabled: bool,
    pub max_retries: u32,
    pub initial_delay: f64,
    pub max_delay: f64,
    pub exponential_base: f64,
}

impl RetryPolicy {
    /// 计算重试尝试的回退延迟。
    #[must_use]
    #[allow(dead_code)] // used by runtime_api; will be wired into client retry loop
    pub fn delay_for_attempt(&self, attempt: u32) -> std::time::Duration {
        let exponent = i32::try_from(attempt).unwrap_or(i32::MAX);
        let delay = self.initial_delay * self.exponential_base.powi(exponent);
        let delay = delay.min(self.max_delay);
        // 限制在合理范围内，防止错误配置值导致的 NaN/负数
        let delay = delay.clamp(0.0, 300.0);
        std::time::Duration::from_secs_f64(delay)
    }
}

/// 上下文管理配置（仅追加的分层上下文，带有 Flash 接缝）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ContextConfig {
    /// 分层上下文管理的主开关。默认：在 v0.7.5 审计 V4 前缀缓存行为期间为 false。
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 在稳定提示前缀中包含确定性的项目上下文包。默认：true；
    /// 设置 `[context] project_pack = false` 以禁用。
    #[serde(default)]
    pub project_pack: Option<bool>,
    /// 逐字窗口：最后 N 轮永远不会被总结。默认：16。
    #[serde(default)]
    pub verbatim_window_turns: Option<usize>,
    /// 基于活跃请求输入估计的软接缝阈值。
    #[serde(default)]
    pub l1_threshold: Option<usize>,
    #[serde(default)]
    pub l2_threshold: Option<usize>,
    #[serde(default)]
    pub l3_threshold: Option<usize>,
    /// 用于接缝/简报工作的模型。默认："deepseek-v4-flash"。
    #[serde(default)]
    pub seam_model: Option<String>,
}

/// 子代理模型覆盖。`models` 中的键可以是角色名称（`worker`、
/// `explorer`、`awaiter`）或类型名称（`general`、`explore`、`plan`、
/// `review`、`custom`）。每次调用显式选择的模型仍然优先。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SubagentsConfig {
    /// 面向模型的 `agent` 工具的顶级开关。`None` 保留
    /// 特性标志默认值；`false` 隐藏/拒绝子代理生成，
    /// 而不更改数值队列/深度旋钮。
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub worker_model: Option<String>,
    #[serde(default)]
    pub explorer_model: Option<String>,
    #[serde(default)]
    pub awaiter_model: Option<String>,
    #[serde(default)]
    pub review_model: Option<String>,
    #[serde(default)]
    pub custom_model: Option<String>,
    #[serde(default)]
    pub models: Option<HashMap<String, String>>,
    /// 最大并发子代理数。覆盖顶级 max_subagents 设置。
    /// 限制在 [1, MAX_SUBAGENTS]。
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    /// 交互式 `agent` 工具可以生成的嵌套子代理的层数。
    /// `0` 在此运行时深度阻止面向模型的 `agent` 工具；
    /// 使用 `[subagents] enabled = false` 作为更清晰的持久关闭开关。
    /// `1` 允许一层，`2` 两层，以此类推。未设置时，默认为
    /// [`codewhale_config::DEFAULT_SPAWN_DEPTH`]；任何值都被限制在
    /// [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`]。Fleet 工作者
    /// 由 `[fleet.exec] max_spawn_depth` 分别管理；两者共享相同的
    /// 默认值和上限，因此限制不会漂移。
    #[serde(default)]
    pub max_depth: Option<u32>,
    /// 可同时执行的直接（深度 1）子代理数量，超过此数量后
    /// 进一步的启动进入队列等待启动槽（#3095）。未设置时，
    /// 默认为完整解析的 `max_subagents()`（无人工节流）；
    /// 显式值限制在 [1, max_subagents]。
    #[serde(default)]
    pub launch_concurrency: Option<usize>,
    /// 一个会话中允许的最大排队 + 运行中的子代理数。
    /// 默认为一个有界的大队列，同时 `launch_concurrency` 保持
    /// 即时执行有界。
    #[serde(default, alias = "max_total", alias = "admission_limit")]
    pub max_admitted: Option<usize>,
    /// 可选的聚合 token 预算，由根 `agent` 运行及其后代共享。
    /// 未设置或为 0 时，子代理保持遗留的无限消费行为，
    /// 除非单个 `agent` 调用提供每次运行的覆盖。
    #[serde(default)]
    pub token_budget: Option<u64>,
    /// 已弃用的 pre-v0.8.61 `launch_concurrency` 别名。
    /// 仅在 `launch_concurrency` 未设置时生效，因此新键始终优先。
    #[serde(default, rename = "interactive_max_launch")]
    pub interactive_max_launch_legacy: Option<usize>,
    /// 子代理请求的每步 DeepSeek API 超时时间（秒）。
    /// 该超时包裹 `client.create_message`，因此卡住的单步不能
    /// 无限期占用父级的父完成唤醒通道。
    /// 默认为 `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (120)，并限制在
    /// `MIN_SUBAGENT_API_TIMEOUT_SECS..=MAX_SUBAGENT_API_TIMEOUT_SECS`
    /// (1..=1800)。零或未设置使用遗留的 120 秒默认值（#1806, #1808）。
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,
    /// 运行中的子代理停止做出管理器可见进展的挂钟超时。
    /// 默认为 5 分钟，并保持高于每步 API 超时，
    /// 以便缓慢但合法的模型调用不会在其请求超时触发之前被取消（#2614）。
    #[serde(default)]
    pub heartbeat_timeout_secs: Option<u64>,
    /// 子代理扇出和预算旋钮的按提供商覆盖。
    /// 键是提供商名称，如 `deepseek`、`zai`、`openrouter` 或 `anthropic`。
    #[serde(default)]
    pub providers: Option<HashMap<String, SubagentProviderConfig>>,
}

/// 提供商特定的子代理限制覆盖。
///
/// 每个字段在未设置时从 `[subagents]` 继承，
/// 因此提供商配置文件可以仅收紧对该 API 速率限制重要的旋钮。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SubagentProviderConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    #[serde(default)]
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub launch_concurrency: Option<usize>,
    #[serde(default, alias = "max_total", alias = "admission_limit")]
    pub max_admitted: Option<usize>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,
    #[serde(default)]
    pub heartbeat_timeout_secs: Option<u64>,
}

/// `[auto]` 表——`--model auto` / `/model auto` 路由器的旋钮。
///
/// `cost_saving`（#1207）：当为 `true` 时，自动模式路由器对模糊请求
/// 偏好 `deepseek-v4-flash`，仅在任务明显受益于更深推理时才升级到
/// `deepseek-v4-pro`。默认为 `false`（平衡——匹配现有路由风格）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AutoConfig {
    #[serde(default)]
    pub cost_saving: Option<bool>,
}

fn default_update_check_for_updates() -> bool {
    true
}

/// 启动更新检查配置（config.toml 中的 `[update]` 表）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UpdateConfig {
    /// 当为 false 时，完全跳过 TUI 启动后台更新检查。
    #[serde(default = "default_update_check_for_updates")]
    pub check_for_updates: bool,
    /// 可选的 GitHub 兼容的最新版本 JSON 端点。
    #[serde(default)]
    pub update_uri: Option<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_for_updates: true,
            update_uri: None,
        }
    }
}

impl UpdateConfig {
    #[must_use]
    pub fn update_uri(&self) -> Option<&str> {
        self.update_uri
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

/// 已解析的 CLI 配置，包括默认值和环境覆盖。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub provider: Option<String>,
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(alias = "baseUrl")]
    pub base_url: Option<String>,
    /// 发送到模型 API 请求的可选额外 HTTP 头。
    #[serde(alias = "httpHeaders")]
    pub http_headers: Option<HashMap<String, String>>,
    #[serde(alias = "defaultTextModel")]
    pub default_text_model: Option<String>,
    #[serde(alias = "authMode")]
    pub auth_mode: Option<String>,
    /// DeepSeek 推理努力级别：`"off" | "low" | "medium" | "high" | "max"`。
    /// 运行时未设置时默认为 `"max"`。
    pub reasoning_effort: Option<String>,
    /// 原生工具目录控制。此表控制内置工具加载策略。
    #[serde(default)]
    pub tools: Option<ToolsConfig>,
    pub skills_dir: Option<String>,
    pub mcp_config_path: Option<String>,
    pub mcp_oauth_callback_port: Option<u16>,
    pub mcp_oauth_callback_url: Option<String>,
    pub notes_path: Option<String>,
    pub memory_path: Option<String>,
    /// 当为 true 时，设置 `tool_choice: "required"` 并将兼容的函数
    /// 模式加入 DeepSeek beta 严格模式。带有根替代的模式保持非严格，
    /// 以避免更改可选/one-of 工具语义。
    pub strict_tool_mode: Option<bool>,
    /// 额外的用户拥有的系统提示源，按声明顺序连接（#454）。
    /// 路径通过 `expand_path` 展开，因此 `~` 和环境变量有效。
    /// 项目范围配置不允许设置此字段；TUI 项目覆盖忽略 `instructions`，
    /// 因此克隆的仓库不能选择任意本地文件放入提示中。
    /// 每个配置的文件被加载，上限为 100 KiB，在读取错误时跳过
    ///（带警告），因此缺失的可选文件不会导致启动失败。
    pub instructions: Option<Vec<String>>,
    pub allow_shell: Option<bool>,
    /// 每次完成的轮次后的选择加入幽灵文本后续提示建议。
    /// 默认：false——用户必须显式将其设为 true 才能启用。
    pub prompt_suggestion: Option<bool>,
    #[serde(alias = "approvalPolicy")]
    pub approval_policy: Option<String>,
    #[serde(alias = "sandboxMode")]
    pub sandbox_mode: Option<String>,
    #[serde(default, alias = "fallbackProviders")]
    pub fallback_providers: Vec<codewhale_config::ProviderKind>,
    pub yolo: Option<bool>,
    pub verbosity: Option<String>,
    /// 外部沙箱后端：`"none"` 或 `"opensandbox"`。
    /// 设置后，exec_shell 通过后端的 HTTP API 路由命令，
    /// 而不是生成本地进程。
    #[serde(alias = "sandboxBackend")]
    pub sandbox_backend: Option<String>,
    /// 外部沙箱后端的基础 URL（默认：`"http://localhost:8080"`）。
    #[serde(alias = "sandboxUrl")]
    pub sandbox_url: Option<String>,
    /// 外部沙箱后端（作为 Bearer token 发送）的可选 API 密钥。
    #[serde(alias = "sandboxApiKey")]
    pub sandbox_api_key: Option<String>,
    /// 当为 true 且 Linux 上存在 `/usr/bin/bwrap` 时，通过 bubblewrap
    /// 路由 exec_shell，而不是仅依赖 Landlock（#2184）。
    /// 默认为 false。需要单独安装 `bubblewrap` 包——
    /// 我们不内嵌 bwrap。
    #[serde(alias = "preferBwrap")]
    pub prefer_bwrap: Option<bool>,
    #[serde(alias = "managedConfigPath")]
    pub managed_config_path: Option<String>,
    #[serde(alias = "requirementsPath")]
    pub requirements_path: Option<String>,
    #[serde(alias = "maxSubagents")]
    pub max_subagents: Option<usize>,
    pub retry: Option<RetryConfig>,
    pub features: Option<FeaturesToml>,

    /// 工具调用的确定性用户级自动审查策略。引擎在内置安全底线之后
    /// 应用这些规则，因此配置不能绕过发布/破坏性后台保留。
    #[serde(default)]
    pub auto_review: Option<AutoReviewConfig>,

    /// TUI 配置（备用屏幕等）
    pub tui: Option<TuiConfig>,

    /// 生命周期钩子配置
    #[serde(default)]
    pub hooks: Option<HooksConfig>,

    /// 提供商特定的凭据和与 `codewhale` 外观共享的默认值。
    #[serde(default)]
    pub providers: Option<ProvidersConfig>,

    /// 桌面通知设置（长轮次完成时的 OSC 9 / BEL）。
    #[serde(default)]
    pub notifications: Option<NotificationsConfig>,

    /// 按域网络策略（#135）。不存在时，网络工具回退到
    /// 反映 pre-v0.7.0 行为的宽松默认值。
    #[serde(default)]
    pub network: Option<NetworkPolicyToml>,

    /// 验证器预览行为（#2093）。不存在时，自动验证器预览保持关闭，
    /// 验证器裁决使用 hunt 策略。
    #[serde(default)]
    pub verifier: Option<codewhale_config::VerifierConfigToml>,

    /// 社区技能安装器设置（#140）。不存在时，安装器命令
    /// 回退到捆绑的默认值
    ///（[`crate::skills::install::DEFAULT_REGISTRY_URL`] +
    /// [`crate::skills::install::DEFAULT_MAX_SIZE_BYTES`]）。
    #[serde(default)]
    pub skills: Option<SkillsConfig>,

    /// 工作区侧 git 快照（#137）。表不存在时默认为启用，保留 7 天。
    #[serde(default)]
    pub snapshots: Option<SnapshotsConfig>,

    /// 网络搜索提供商配置。不存在时，默认为 DuckDuckGo。
    /// 将 `provider` 设置为另一个支持的后端，如 `bing`、`tavily`、
    /// `bocha`、`metaso`、`searxng`、`baidu`、`volcengine` 或 `sofya`。
    /// API 支持的服务需要提供商特定的凭据；SearXNG 需要一个受信任的 `base_url`。
    #[serde(default)]
    pub search: Option<SearchConfig>,

    /// 用户级记忆文件（#489）。默认行为是**选择加入**：
    /// 仅当 `[memory] enabled = true` 或设置了 `DEEPSEEK_MEMORY=on` 时
    /// 才进行加载和注入。
    ///
    /// v0.8.66 弃用了此功能，转而支持 Moraine MCP 召回。设置
    /// `[memory] moraine_fallback = true` 以跳过遗留的推送/注入路径，
    /// 同时保留 Moraine 的拉取/召回工具。
    #[serde(default)]
    pub memory: Option<MemoryConfig>,

    /// 小米 MiMo 语音/TTS 默认值。
    #[serde(default)]
    pub speech: Option<SpeechConfig>,

    /// `--model auto`（#1207）的可调参数。不存在时，自动路由器
    /// 保持其现有的平衡行为。
    #[serde(default)]
    pub auto: Option<AutoConfig>,

    /// 可选的 1-8 快捷键栏位绑定（#2064）。不存在时，快捷键栏 UI
    /// 和分发层使用 `codewhale_config` 中的内置默认值。
    #[serde(default)]
    pub hotbar: Option<Vec<codewhale_config::HotbarBindingToml>>,

    /// 启动更新检查行为。不存在时，TUI 保持默认的
    /// 即发即弃的最新版本检查。
    #[serde(default)]
    pub update: Option<UpdateConfig>,

    /// 编辑后 LSP 诊断注入（#136）。不存在时，引擎应用
    /// [`LspConfigToml`] 中记录的默认值。
    #[serde(default)]
    pub lsp: Option<LspConfigToml>,

    /// 仅追加的分层上下文管理，带有 Flash 接缝管理器（#159）。
    #[serde(default)]
    pub context: ContextConfig,

    /// Agent Fleet 信任/安全/角色/执行配置。
    #[serde(default)]
    pub fleet: Option<codewhale_config::FleetConfigToml>,

    /// 工作流自动启动、审批、隔离和活动持久化旋钮（#4128）。
    /// 不存在时，消费者通过 [`Self::workflow_config`] 使用
    /// [`codewhale_config::WorkflowConfigToml::default`]。
    #[serde(default)]
    pub workflow: Option<codewhale_config::WorkflowConfigToml>,

    /// 子代理模型覆盖。
    #[serde(default)]
    pub subagents: Option<SubagentsConfig>,

    /// 运行时 API 服务器调优（`codewhale serve --http`）。目前仅
    /// 承载 CORS 允许列表扩展（whalescale#255 / #561）。当表不存在时，
    /// 守护进程以 localhost:3000 / localhost:1420 / tauri://localhost
    /// 作为唯一允许的开发来源。
    #[serde(default)]
    pub runtime_api: Option<RuntimeApiConfig>,

    /// Workshop / 大型工具输出路由（#548）。不存在时，
    /// 全局默认阈值 4096 token 生效，路由处于活动状态。
    #[serde(default)]
    pub workshop: Option<crate::tools::large_output_router::WorkshopConfig>,

    /// `image_analyze` 工具的视觉模型配置。
    #[serde(default)]
    pub vision_model: Option<VisionModelConfig>,

    /// 用于运行时检查的兄弟 `permissions.toml` 询问规则。
    ///
    /// 这刻意不是 `config.toml` 的一部分；它在配置文件/环境/托管配置
    /// 解析后从配套的权限文件加载。
    #[serde(skip)]
    pub exec_policy_engine: ExecPolicyEngine,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutoReviewConfig {
    #[serde(default, alias = "guidance", alias = "naturalLanguageGuidance")]
    pub natural_language_guidance: Option<String>,
    #[serde(default)]
    pub allow: Vec<AutoReviewRuleConfig>,
    #[serde(default)]
    pub block: Vec<AutoReviewRuleConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutoReviewRuleConfig {
    pub id: Option<String>,
    #[serde(default, alias = "toolName", alias = "tool_name")]
    pub tool: Option<String>,
    #[serde(default, alias = "actionKind", alias = "action_kind")]
    pub action_kind: Option<String>,
    #[serde(default, alias = "textContains", alias = "text_contains")]
    pub text_contains: Option<String>,
    pub reason: Option<String>,
}

impl AutoReviewConfig {
    fn to_runtime_policy(&self) -> crate::tui::auto_review::AutoReviewPolicy {
        crate::tui::auto_review::AutoReviewPolicy {
            allow_rules: self
                .allow
                .iter()
                .enumerate()
                .map(|(index, rule)| {
                    rule.to_runtime_rule(index, crate::tui::auto_review::AutoReviewAction::Allow)
                })
                .collect(),
            block_rules: self
                .block
                .iter()
                .enumerate()
                .map(|(index, rule)| {
                    rule.to_runtime_rule(index, crate::tui::auto_review::AutoReviewAction::Block)
                })
                .collect(),
            natural_language_guidance: self
                .natural_language_guidance
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }

    fn validate(&self) -> Result<()> {
        validate_auto_review_rules("allow", &self.allow)?;
        validate_auto_review_rules("block", &self.block)?;
        Ok(())
    }
}

impl AutoReviewRuleConfig {
    fn to_runtime_rule(
        &self,
        index: usize,
        action: crate::tui::auto_review::AutoReviewAction,
    ) -> crate::tui::auto_review::AutoReviewRule {
        let id_prefix = match action {
            crate::tui::auto_review::AutoReviewAction::Allow => "allow",
            crate::tui::auto_review::AutoReviewAction::Block => "block",
            crate::tui::auto_review::AutoReviewAction::AskUser => "ask",
            crate::tui::auto_review::AutoReviewAction::HoldForReview => "hold",
        };
        let id = self
            .id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("config-{id_prefix}-{index}"));
        let reason = self
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("configured auto-review {id_prefix} rule"));
        let mut rule = match action {
            crate::tui::auto_review::AutoReviewAction::Allow => {
                crate::tui::auto_review::AutoReviewRule::allow(id, reason)
            }
            crate::tui::auto_review::AutoReviewAction::Block => {
                crate::tui::auto_review::AutoReviewRule::block(id, reason)
            }
            crate::tui::auto_review::AutoReviewAction::AskUser
            | crate::tui::auto_review::AutoReviewAction::HoldForReview => {
                crate::tui::auto_review::AutoReviewRule::block(id, reason)
            }
        };

        if let Some(tool) = self
            .tool
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            rule = rule.tool_name(tool.to_string());
        }
        if let Some(action_kind) = self
            .action_kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(parse_auto_review_action_kind)
        {
            rule = rule.action_kind(action_kind);
        }
        if let Some(text) = self
            .text_contains
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            rule = rule.text_contains(text.to_string());
        }

        rule
    }

    fn has_matcher(&self) -> bool {
        self.tool
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .action_kind
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .text_contains
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

fn validate_auto_review_rules(kind: &str, rules: &[AutoReviewRuleConfig]) -> Result<()> {
    for (index, rule) in rules.iter().enumerate() {
        if !rule.has_matcher() {
            anyhow::bail!(
                "Invalid auto_review.{kind}[{index}]: set at least one of tool, action_kind, or text_contains."
            );
        }
        if let Some(action_kind) = rule.action_kind.as_deref()
            && parse_auto_review_action_kind(action_kind.trim()).is_none()
        {
            anyhow::bail!(
                "Invalid auto_review.{kind}[{index}].action_kind '{action_kind}': expected read, write, shell, network, git, mcp_read, mcp_action, browser, secret, publish, destructive, or unknown."
            );
        }
    }
    Ok(())
}

fn parse_auto_review_action_kind(raw: &str) -> Option<crate::tui::auto_review::ToolActionKind> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "read" => Some(crate::tui::auto_review::ToolActionKind::Read),
        "write" => Some(crate::tui::auto_review::ToolActionKind::Write),
        "shell" => Some(crate::tui::auto_review::ToolActionKind::Shell),
        "network" => Some(crate::tui::auto_review::ToolActionKind::Network),
        "git" => Some(crate::tui::auto_review::ToolActionKind::Git),
        "mcp_read" => Some(crate::tui::auto_review::ToolActionKind::McpRead),
        "mcp_action" => Some(crate::tui::auto_review::ToolActionKind::McpAction),
        "browser" => Some(crate::tui::auto_review::ToolActionKind::Browser),
        "secret" => Some(crate::tui::auto_review::ToolActionKind::Secret),
        "publish" => Some(crate::tui::auto_review::ToolActionKind::Publish),
        "destructive" => Some(crate::tui::auto_review::ToolActionKind::Destructive),
        "unknown" => Some(crate::tui::auto_review::ToolActionKind::Unknown),
        _ => None,
    }
}

/// 用户如何替换或禁用内置工具。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOverride {
    /// 运行本地脚本文件。脚本从标准输入接收工具的 JSON 输入，
    /// 并且必须在标准输出上返回 JSON `ToolResult`。
    Script {
        /// 脚本路径（绝对路径，或相对于 `~/.codewhale/tools/` 的路径）。
        path: String,
        /// 在工具的 JSON 输入之前预置的可选静态参数。
        #[serde(default)]
        args: Option<Vec<String>>,
    },
    /// 运行外部命令。命令从标准输入接收工具的 JSON 输入，
    /// 并且必须在标准输出上返回 JSON `ToolResult`。
    Command {
        /// 要运行的命令（二进制名称或绝对路径）。
        command: String,
        /// 在工具的 JSON 输入之前预置的可选静态参数。
        #[serde(default)]
        args: Option<Vec<String>>,
    },
    /// 完全禁用内置工具。该工具不会出现在模型可见的目录中，
    /// 也无法被调用。
    Disabled,
}

/// `image_analyze` 工具的视觉模型配置。
/// 使用 OpenAI 兼容的视觉模型 API。
#[derive(Debug, Clone, Deserialize)]
pub struct VisionModelConfig {
    /// 模型标识符（例如 "gemini-3.1-flash-lite-preview"）。
    pub model: String,
    /// 视觉模型的 API 密钥。如果未指定，则从主配置继承。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 视觉模型 API 的基础 URL。默认为 OpenAI。
    #[serde(default)]
    pub base_url: Option<String>,
}

/// `[runtime_api]` 表——本地 HTTP/SSE 守护进程的旋钮。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RuntimeApiConfig {
    /// 在内置默认值之上允许的额外 CORS 来源
    ///（`http://localhost:{3000,1420}`、`http://127.0.0.1:{3000,1420}`、
    /// `tauri://localhost`）。在针对非默认开发服务器端口
    ///（例如 Vite 的默认 `:5173`）开发 UI 时很有用。
    ///
    /// 解析顺序（最高优先级优先）：`--cors-origin` CLI 标志、
    /// `DEEPSEEK_CORS_ORIGINS` 环境变量（逗号分隔）、此字段。Whalescale#255 / #561。
    #[serde(default)]
    pub cors_origins: Option<Vec<String>>,
}

/// `[skills]` 表——社区技能安装器的旋钮。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SkillsConfig {
    /// 精选注册索引。`/skill install <name>` 在此查找规格。
    /// 默认为 [`crate::skills::install::DEFAULT_REGISTRY_URL`]。
    #[serde(default)]
    pub registry_url: Option<String>,
    /// 每个技能的*未压缩*最大大小（字节）。超过此限制的
    /// tarball 在验证期间被拒绝。默认为 5 MiB。
    #[serde(default)]
    pub max_install_size_bytes: Option<u64>,
    /// 当为 true 时，技能发现仅扫描 CodeWhale 拥有的技能根目录
    ///（加上任何显式的 `skills_dir`），而不是从其他 AI 工具
    ///（如 Claude、OpenCode 或 Cursor）导入兼容的目录。
    #[serde(default, alias = "scanCodewhaleOnly")]
    pub scan_codewhale_only: Option<bool>,
}

impl SkillsConfig {
    /// 使用捆绑的默认值解析注册表 URL。
    #[must_use]
    pub fn registry_url(&self) -> String {
        self.registry_url
            .clone()
            .unwrap_or_else(|| crate::skills::install::DEFAULT_REGISTRY_URL.to_string())
    }

    /// 使用捆绑的默认值解析最大安装大小。
    #[must_use]
    pub fn max_install_size_bytes(&self) -> u64 {
        self.max_install_size_bytes
            .unwrap_or(crate::skills::install::DEFAULT_MAX_SIZE_BYTES)
    }

    /// 解析会话时发现是否应忽略跨工具技能目录。
    /// 默认为保持兼容性的广泛扫描。
    #[must_use]
    pub fn scan_codewhale_only(&self) -> bool {
        self.scan_codewhale_only.unwrap_or(false)
    }
}

/// `[network]` 表——镜像 `codewhale_config::NetworkPolicyToml`，
/// 以便实时 TUI 运行时可以构造 [`crate::network_policy::NetworkPolicy`]，
/// 而无需深入 workspace config crate。文档请参见 `config.example.toml`。
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkPolicyToml {
    /// 不在 `allow` 或 `deny` 中的主机的决策。
    /// `"allow" | "deny" | "prompt"` 之一。默认为 `"prompt"`。
    #[serde(default = "default_network_decision")]
    pub default: String,
    /// 始终允许的主机。子域规则：前导点（`.example.com`）
    /// 匹配子域但不匹配顶域。
    #[serde(default)]
    pub allow: Vec<String>,
    /// 始终拒绝的主机。拒绝条目优先于允许条目。
    #[serde(default)]
    pub deny: Vec<String>,
    /// 其 DNS 可能在显式受信任的代理设置中解析为假 IP/私有代理范围的
    /// 主机名。文字 IP URL 仍然被阻止。
    #[serde(default)]
    pub proxy: Vec<String>,
    /// 是否每个出站网络调用记录一条审计日志。
    #[serde(default = "default_network_audit")]
    pub audit: bool,
}

fn default_network_decision() -> String {
    "prompt".to_string()
}

fn default_network_audit() -> bool {
    true
}

impl Default for NetworkPolicyToml {
    fn default() -> Self {
        Self {
            default: default_network_decision(),
            allow: Vec::new(),
            deny: Vec::new(),
            proxy: Vec::new(),
            audit: default_network_audit(),
        }
    }
}

impl NetworkPolicyToml {
    /// 从磁盘上的模式构建运行时 [`crate::network_policy::NetworkPolicy`]。
    #[must_use]
    pub fn into_runtime(self) -> crate::network_policy::NetworkPolicy {
        crate::network_policy::NetworkPolicy {
            default: crate::network_policy::Decision::parse(&self.default).into(),
            allow: self.allow,
            deny: self.deny,
            proxy: self.proxy,
            audit: self.audit,
        }
    }
}

/// `[lsp]` 表——镜像 [`crate::lsp::LspConfig`]。在 `config.example.toml`
/// 中有文档说明。省略时，应用 `LspConfig::default()` 的默认值
///（启用、5 秒轮询、20 条诊断/文件、仅错误、无覆盖）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LspConfigToml {
    /// 主开关。默认为 `true`。
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 在 `didOpen`/`didChange` 后等待 LSP 服务器发布诊断的时间。
    /// 默认为 5000 ms。
    #[serde(default)]
    pub poll_after_edit_ms: Option<u64>,
    /// 每个文件暴露的诊断上限。默认为 20。
    #[serde(default)]
    pub max_diagnostics_per_file: Option<usize>,
    /// 是否在错误之外还暴露警告。默认为 `false`。
    #[serde(default)]
    pub include_warnings: Option<bool>,
    /// `Language -> [cmd, ...args]` 表的可选覆盖。
    /// 键是语言标识（`"rust"`、`"go"` 等）。
    #[serde(default)]
    pub servers: Option<HashMap<String, Vec<String>>>,
    /// 为不在内置注册表中的文件扩展名定义的用户自定义 LSP 服务器。
    /// 按键是扩展名（例如 `"php"`、`"rb"`）。
    #[serde(default)]
    pub custom: Option<HashMap<String, crate::lsp::CustomLspDef>>,
}

impl LspConfigToml {
    /// 从磁盘上的模式构建运行时 [`crate::lsp::LspConfig`]，
    /// 对任何未设置的字段回退到默认值。
    #[must_use]
    pub fn into_runtime(self) -> crate::lsp::LspConfig {
        let defaults = crate::lsp::LspConfig::default();
        crate::lsp::LspConfig {
            enabled: self.enabled.unwrap_or(defaults.enabled),
            poll_after_edit_ms: self
                .poll_after_edit_ms
                .unwrap_or(defaults.poll_after_edit_ms),
            max_diagnostics_per_file: self
                .max_diagnostics_per_file
                .unwrap_or(defaults.max_diagnostics_per_file),
            include_warnings: self.include_warnings.unwrap_or(defaults.include_warnings),
            servers: self.servers.unwrap_or_default(),
            custom: self.custom.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(alias = "baseUrl")]
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(
        default,
        alias = "contextWindow",
        alias = "context_window_tokens",
        alias = "contextWindowTokens",
        alias = "context_length",
        alias = "contextLength"
    )]
    pub context_window: Option<u32>,
    pub mode: Option<String>,
    #[serde(alias = "authMode")]
    pub auth_mode: Option<String>,
    #[serde(alias = "insecureSkipTlsVerify")]
    pub insecure_skip_tls_verify: Option<bool>,
    #[serde(alias = "httpHeaders")]
    pub http_headers: Option<HashMap<String, String>>,
    #[serde(alias = "pathSuffix")]
    pub path_suffix: Option<String>,
    #[serde(alias = "reasoningStyle", alias = "reasoningStreamStyle")]
    pub reasoning_stream_style: Option<String>,
    #[serde(
        default,
        alias = "max-concurrency",
        alias = "maxConcurrency",
        alias = "concurrency"
    )]
    pub max_concurrency: Option<usize>,
    pub auth: Option<codewhale_config::ProviderAuthSourceToml>,
    /// 自定义 `[providers.<name>]` 条目的有线协议选择器（#1519）。
    ///
    /// 目前只接受 `"openai-compatible"`；任何其他值在
    /// 选择时被拒绝，因此不支持的线格式大声失败，
    /// 而非静默地作为 OpenAI 路由。内置提供商保留此字段未设置。
    #[serde(default)]
    pub kind: Option<String>,
    /// 保存此自定义提供商的 API 密钥的环境变量名称（#1519），
    /// 例如 `api_key_env = "EXAMPLE_API_KEY"`。密钥值本身
    /// 永远不会存储在配置中；只有环境变量名称被存储。
    #[serde(default, alias = "apiKeyEnv")]
    pub api_key_env: Option<String>,
}

impl ProviderConfig {
    /// 当此条目选择 OpenAI 兼容的自定义有线协议时为 true。
    ///
    /// `kind` 不区分大小写地与 `openai-compatible`（以及
    /// `openai_compatible` 下划线拼写）匹配。当 `kind` 未设置
    ///（内置提供商）或命名任何其他值时返回 `false`。
    #[must_use]
    pub fn is_openai_compatible_custom(&self) -> bool {
        self.kind.as_deref().is_some_and(|kind| {
            let normalized = kind.trim().to_ascii_lowercase().replace('_', "-");
            normalized == "openai-compatible"
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub deepseek: ProviderConfig,
    #[serde(default, alias = "deepseekCn")]
    pub deepseek_cn: ProviderConfig,
    #[serde(
        default,
        alias = "deepseek-anthropic",
        alias = "deepseekAnthropic",
        alias = "deepseek-claude",
        alias = "deepseek_claude"
    )]
    pub deepseek_anthropic: ProviderConfig,
    #[serde(default, alias = "nvidiaNim")]
    pub nvidia_nim: ProviderConfig,
    #[serde(default)]
    pub openai: ProviderConfig,
    #[serde(default)]
    pub atlascloud: ProviderConfig,
    #[serde(default, alias = "wanjieArk")]
    pub wanjie_ark: ProviderConfig,
    #[serde(default)]
    pub volcengine: ProviderConfig,
    #[serde(default)]
    pub openrouter: ProviderConfig,
    #[serde(
        default,
        alias = "xiaomi",
        alias = "mimo",
        alias = "xiaomimimo",
        alias = "xiaomiMimo"
    )]
    pub xiaomi_mimo: ProviderConfig,
    #[serde(default)]
    pub novita: ProviderConfig,
    #[serde(default)]
    pub fireworks: ProviderConfig,
    #[serde(default)]
    pub siliconflow: ProviderConfig,
    #[serde(
        default,
        alias = "siliconflow-CN",
        alias = "siliconflow-cn",
        alias = "siliconflowCn"
    )]
    pub siliconflow_cn: ProviderConfig,
    #[serde(default)]
    pub arcee: ProviderConfig,
    #[serde(default)]
    pub moonshot: ProviderConfig,
    #[serde(default)]
    pub sglang: ProviderConfig,
    #[serde(default)]
    pub vllm: ProviderConfig,
    #[serde(default)]
    pub ollama: ProviderConfig,
    #[serde(default, alias = "hugging-face", alias = "hf")]
    pub huggingface: ProviderConfig,
    #[serde(default, alias = "deep-infra", alias = "deep_infra")]
    pub deepinfra: ProviderConfig,
    #[serde(default, alias = "together-ai")]
    pub together: ProviderConfig,
    #[serde(
        default,
        alias = "baidu-qianfan",
        alias = "baidu_qianfan",
        alias = "baidu"
    )]
    pub qianfan: ProviderConfig,
    #[serde(
        default,
        alias = "openai-codex",
        alias = "openaiCodex",
        alias = "codex",
        alias = "chatgpt"
    )]
    pub openai_codex: ProviderConfig,
    #[serde(default, alias = "claude")]
    pub anthropic: ProviderConfig,
    #[serde(default, alias = "open-model", alias = "open_model")]
    pub openmodel: ProviderConfig,
    #[serde(
        default,
        alias = "zhipu",
        alias = "zhipuai",
        alias = "bigmodel",
        alias = "big-model"
    )]
    pub zai: ProviderConfig,
    #[serde(default)]
    pub stepfun: ProviderConfig,
    #[serde(default)]
    pub minimax: ProviderConfig,
    #[serde(default, alias = "sakana-ai", alias = "sakana_ai", alias = "fugu")]
    pub sakana: ProviderConfig,
    #[serde(
        default,
        alias = "long-cat",
        alias = "meituan-longcat",
        alias = "meituan"
    )]
    pub longcat: ProviderConfig,
    #[serde(
        default,
        alias = "meta-ai",
        alias = "meta_ai",
        alias = "meta-model-api",
        alias = "meta_model_api",
        alias = "muse",
        alias = "muse-spark"
    )]
    pub meta: ProviderConfig,
    #[serde(default, alias = "x-ai", alias = "x_ai", alias = "grok")]
    pub xai: ProviderConfig,
    /// 任意用户命名的自定义提供商（#1519）。
    ///
    /// 捕获每个键不是上述内置提供商之一的 `[providers.<name>]` 表。
    /// 每个条目是通过 `provider = "<name>"` 选择的 OpenAI 兼容自定义
    /// 端点；路由通过 [`ApiProvider::Custom`] 读取其 `base_url` / `model` / `api_key_env`。
    #[serde(flatten, default)]
    pub custom: HashMap<String, ProviderConfig>,
}

impl ProvidersConfig {
    /// 通过 `[providers.<name>]` 键查找用户定义的自定义提供商表（#1519）。
    /// 当没有该确切名称的条目时返回 `None`。
    #[must_use]
    pub fn custom_provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        self.custom.get(name)
    }

    fn validate(&self) -> Result<()> {
        let builtins = [
            ("providers.deepseek", &self.deepseek),
            ("providers.deepseek_cn", &self.deepseek_cn),
            ("providers.deepseek_anthropic", &self.deepseek_anthropic),
            ("providers.nvidia_nim", &self.nvidia_nim),
            ("providers.openai", &self.openai),
            ("providers.atlascloud", &self.atlascloud),
            ("providers.wanjie_ark", &self.wanjie_ark),
            ("providers.volcengine", &self.volcengine),
            ("providers.openrouter", &self.openrouter),
            ("providers.xiaomi_mimo", &self.xiaomi_mimo),
            ("providers.novita", &self.novita),
            ("providers.fireworks", &self.fireworks),
            ("providers.siliconflow", &self.siliconflow),
            ("providers.siliconflow_cn", &self.siliconflow_cn),
            ("providers.arcee", &self.arcee),
            ("providers.moonshot", &self.moonshot),
            ("providers.sglang", &self.sglang),
            ("providers.vllm", &self.vllm),
            ("providers.ollama", &self.ollama),
            ("providers.huggingface", &self.huggingface),
            ("providers.deepinfra", &self.deepinfra),
            ("providers.together", &self.together),
            ("providers.qianfan", &self.qianfan),
            ("providers.openai_codex", &self.openai_codex),
            ("providers.anthropic", &self.anthropic),
            ("providers.openmodel", &self.openmodel),
            ("providers.zai", &self.zai),
            ("providers.stepfun", &self.stepfun),
            ("providers.minimax", &self.minimax),
            ("providers.sakana", &self.sakana),
            ("providers.meta", &self.meta),
            ("providers.xai", &self.xai),
        ];
        for (name, config) in builtins {
            validate_provider_context_window(name, config.context_window)?;
        }
        for (name, config) in &self.custom {
            validate_provider_context_window(&format!("providers.{name}"), config.context_window)?;
        }
        Ok(())
    }
}

fn validate_provider_context_window(name: &str, value: Option<u32>) -> Result<()> {
    if value == Some(0) {
        anyhow::bail!("{name}.context_window must be greater than 0");
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ConfigFile {
    #[serde(flatten)]
    base: Config,
    profiles: Option<HashMap<String, Config>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RequirementsFile {
    #[serde(default)]
    allowed_approval_policies: Vec<String>,
    #[serde(default)]
    allowed_sandbox_modes: Vec<String>,
}

// === 配置加载 ===

impl Config {
    #[must_use]
    pub fn search_provider_resolution(&self) -> SearchProviderResolution {
        if let Ok(raw) = std::env::var("DEEPSEEK_SEARCH_PROVIDER")
            && let Some(provider) = SearchProvider::parse(&raw)
        {
            return SearchProviderResolution {
                provider,
                source: SearchProviderSource::EnvOverride,
            };
        }

        if let Some(provider) = self.search.as_ref().and_then(|search| search.provider) {
            return SearchProviderResolution {
                provider,
                source: SearchProviderSource::Config,
            };
        }

        SearchProviderResolution {
            provider: SearchProvider::default(),
            source: SearchProviderSource::Default,
        }
    }

    #[must_use]
    pub fn search_provider(&self) -> SearchProvider {
        self.search_provider_resolution().provider
    }

    /// 如果设置了 `[auto] cost_saving = true` 选择加入则返回 `true`（#1207）。
    /// 当为 true 时，自动模式路由器对模糊请求偏向
    /// `deepseek-v4-flash`，而不是升级到 `deepseek-v4-pro`。
    /// 默认：`false`（平衡行为）。
    #[must_use]
    pub fn auto_cost_saving(&self) -> bool {
        self.auto
            .as_ref()
            .and_then(|a| a.cost_saving)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn tools_always_load(&self) -> std::collections::HashSet<String> {
        self.tools
            .as_ref()
            .map(|tools| {
                tools
                    .always_load
                    .iter()
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn auto_review_policy(&self) -> crate::tui::auto_review::AutoReviewPolicy {
        self.auto_review
            .as_ref()
            .map(AutoReviewConfig::to_runtime_policy)
            .unwrap_or_default()
    }

    /// 从磁盘加载配置并与环境覆盖合并。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// # use crate::config::Config;
    /// let config = Config::load(None, None)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn load(path: Option<PathBuf>, profile: Option<&str>) -> Result<Self> {
        let path = resolve_load_config_path(path);
        let mut config = if let Some(path) = path.as_ref() {
            if path.exists() {
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read config file: {}", path.display()))?;
                let parsed: ConfigFile = toml::from_str(&contents)
                    .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
                if let Some(msg) = warn_on_misplaced_top_level_keys(&contents) {
                    tracing::warn!("{msg}");
                }
                apply_profile(parsed, profile)?
            } else {
                Config::default()
            }
        } else {
            Config::default()
        };

        apply_env_overrides(&mut config);
        apply_managed_overrides(&mut config)?;
        apply_requirements(&mut config)?;
        normalize_model_config(&mut config);
        config.exec_policy_engine = load_sibling_exec_policy_engine(path.as_deref())?;
        config.validate()?;
        config.warn_on_misplaced_root_base_url();
        Ok(config)
    }

    /// Surface a one-line warning when the user has set the legacy root
    /// `base_url` field but their active provider is not DeepSeek (the only
    /// provider that actually reads that field, plus an NvidiaNim back-compat
    /// sniff). Common confusion: users add `base_url = "..."` at the top of
    /// `~/.deepseek/config.toml` for ollama / vllm / openai-compat servers
    /// and wonder why it's silently ignored (#1308).
    fn warn_on_misplaced_root_base_url(&self) {
        let Some(root_base) = self.base_url.as_deref().map(str::trim) else {
            return;
        };
        if root_base.is_empty() {
            return;
        }
        let provider = self.api_provider();
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
            return;
        }
        if matches!(provider, ApiProvider::NvidiaNim)
            && root_base.contains("integrate.api.nvidia.com")
        {
            return;
        }
        // 仅当按提供商的表没有显式的 `base_url` 时才警告，
        // 因为如果有，按提供商的值优先，根字段只是死配置——
        // 没有行为上的意外。
        let has_provider_base = self
            .provider_config_for(provider)
            .and_then(|p| p.base_url.as_deref().map(str::trim))
            .is_some_and(|s| !s.is_empty());
        if has_provider_base {
            return;
        }
        let Ok(table) = provider_config_table_name(provider) else {
            return;
        };
        tracing::warn!(
            "Top-level `base_url = \"{root_base}\"` is ignored for the {provider:?} provider. \
             Move it under `[{table}]` (e.g. `[{table}]\\nbase_url = \"...\"`) \
             or set the corresponding `*_BASE_URL` env var. (#1308)"
        );
    }

    /// 验证关键配置字段是否存在。
    pub fn validate(&self) -> Result<()> {
        if let Some(provider) = self.provider.as_deref()
            && ApiProvider::parse(provider).is_none()
            && self
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(provider))
                .is_none()
        {
            anyhow::bail!(
                "Invalid provider '{provider}': expected {}.",
                ApiProvider::names_hint()
            );
        }
        if let Some(ref key) = self.api_key
            && key.trim().is_empty()
        {
            anyhow::bail!("api_key cannot be empty string");
        }
        if let Some(features) = &self.features {
            for key in features.entries.keys() {
                if !is_known_feature_key(key) {
                    anyhow::bail!("Unknown feature flag: {key}");
                }
            }
        }
        if let Some(model) = self.default_text_model.as_deref()
            && !model.trim().eq_ignore_ascii_case("auto")
            && !provider_passes_model_through(self.api_provider())
            && !self.active_provider_preserves_custom_base_url_model()
            && normalize_model_name(model).is_none()
        {
            anyhow::bail!(
                "Invalid default_text_model '{model}': expected auto or a DeepSeek model ID (for example: deepseek-v4-pro, deepseek-v4-flash, deepseek-ai/deepseek-v4-pro)."
            );
        }
        if let Some(policy) = self.approval_policy.as_deref() {
            let normalized = policy.trim().to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "on-request" | "untrusted" | "never" | "auto" | "suggest"
            ) {
                anyhow::bail!(
                    "Invalid approval_policy '{policy}': expected on-request, untrusted, never, auto, or suggest."
                );
            }
        }
        if let Some(v) = self.verbosity.as_deref() {
            let normalized = v.trim().to_ascii_lowercase();
            if !matches!(normalized.as_str(), "normal" | "concise") {
                anyhow::bail!("Invalid verbosity '{v}': expected normal or concise.");
            }
        }
        if let Some(mode) = self.sandbox_mode.as_deref() {
            let normalized = mode.trim().to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "read-only" | "workspace-write" | "danger-full-access" | "external-sandbox"
            ) {
                anyhow::bail!(
                    "Invalid sandbox_mode '{mode}': expected read-only, workspace-write, danger-full-access, or external-sandbox."
                );
            }
        }
        if let Some(tui) = &self.tui
            && let Some(mode) = tui.alternate_screen.as_deref()
        {
            let mode = mode.to_ascii_lowercase();
            if !matches!(mode.as_str(), "auto" | "always" | "never") {
                anyhow::bail!(
                    "Invalid tui.alternate_screen '{mode}': expected auto, always, or never."
                );
            }
        }
        if let Some(auto_review) = &self.auto_review {
            auto_review.validate()?;
        }
        if let Some(providers) = &self.providers {
            providers.validate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn api_provider(&self) -> ApiProvider {
        if let Some(provider) = self.provider.as_deref().and_then(ApiProvider::parse) {
            return provider;
        }
        // #1519 安全修复：当 `provider = "<name>"` 不是内置提供商
        // 但命名了一个 `[providers.<name>]` 自定义表时，作为动态
        // 自定义标识路由。这必须在下面的 DeepSeek 回退之前，
        // 以便任意自定义名称永远不会静默错误路由到 DeepSeek。
        if let Some(name) = self.provider.as_deref()
            && self
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(name))
                .is_some()
        {
            return ApiProvider::Custom;
        }
        self.base_url
            .as_deref()
            .filter(|base| base.contains("integrate.api.nvidia.com"))
            .map(|_| ApiProvider::NvidiaNim)
            .or_else(|| {
                self.base_url
                    .as_deref()
                    .filter(|base| base.contains("api.deepseeki.com"))
                    .map(|_| ApiProvider::DeepseekCN)
            })
            .unwrap_or(ApiProvider::Deepseek)
    }

    pub(crate) fn provider_config_for(&self, provider: ApiProvider) -> Option<&ProviderConfig> {
        let providers = self.providers.as_ref()?;
        // 自定义提供商的配置存在于扁平映射中，键是选定的
        // `provider = "<name>"` 值，而不是在固定字段中（#1519）。
        // 按名称解析它，以便每个现有读取器（auth、headers、base_url）
        // 透明地看到命名的表。
        if provider == ApiProvider::Custom {
            return self
                .provider
                .as_deref()
                .and_then(|name| providers.custom_provider_config(name));
        }
        Some(match provider {
            ApiProvider::Deepseek => &providers.deepseek,
            ApiProvider::DeepseekCN => &providers.deepseek_cn,
            ApiProvider::DeepseekAnthropic => &providers.deepseek_anthropic,
            ApiProvider::NvidiaNim => &providers.nvidia_nim,
            ApiProvider::Openai => &providers.openai,
            ApiProvider::Atlascloud => &providers.atlascloud,
            ApiProvider::WanjieArk => &providers.wanjie_ark,
            ApiProvider::Openrouter => &providers.openrouter,
            ApiProvider::XiaomiMimo => &providers.xiaomi_mimo,
            ApiProvider::Novita => &providers.novita,
            ApiProvider::Fireworks => &providers.fireworks,
            ApiProvider::Siliconflow => &providers.siliconflow,
            ApiProvider::SiliconflowCn => &providers.siliconflow_cn,
            ApiProvider::Arcee => &providers.arcee,
            ApiProvider::Moonshot => &providers.moonshot,
            ApiProvider::Sglang => &providers.sglang,
            ApiProvider::Vllm => &providers.vllm,
            ApiProvider::Ollama => &providers.ollama,
            ApiProvider::Volcengine => &providers.volcengine,
            ApiProvider::Huggingface => &providers.huggingface,
            ApiProvider::Deepinfra => &providers.deepinfra,
            ApiProvider::Together => &providers.together,
            ApiProvider::Qianfan => &providers.qianfan,
            ApiProvider::OpenaiCodex => &providers.openai_codex,
            ApiProvider::Anthropic => &providers.anthropic,
            ApiProvider::Openmodel => &providers.openmodel,
            ApiProvider::Zai => &providers.zai,
            ApiProvider::Stepfun => &providers.stepfun,
            ApiProvider::Minimax => &providers.minimax,
            ApiProvider::Sakana => &providers.sakana,
            ApiProvider::LongCat => &providers.longcat,
            ApiProvider::Meta => &providers.meta,
            ApiProvider::Xai => &providers.xai,
            // 由上面按名称键的提前返回处理（#1519）。
            ApiProvider::Custom => unreachable!("custom provider resolved by name above"),
        })
    }

    pub(crate) fn subagent_provider_config(
        &self,
        provider: ApiProvider,
    ) -> Option<&SubagentProviderConfig> {
        let providers = self.subagents.as_ref()?.providers.as_ref()?;
        providers.iter().find_map(|(key, config)| {
            subagent_provider_key_matches(key, provider).then_some(config)
        })
    }

    pub(crate) fn provider_config_for_mut(&mut self, provider: ApiProvider) -> &mut ProviderConfig {
        // 自定义提供商的可变槽位键是扁平映射中选定的
        // `provider = "<name>"` 值（#1519）。在可变借用 `providers` 之前
        // 捕获名称；回退到私有哨兵键，以便在未设置名称时访问器保持完整。
        let custom_key = (provider == ApiProvider::Custom).then(|| {
            self.provider
                .clone()
                .unwrap_or_else(|| "__custom__".to_string())
        });
        let providers = self.providers.get_or_insert_with(ProvidersConfig::default);
        if let Some(key) = custom_key {
            return providers.custom.entry(key).or_default();
        }
        match provider {
            ApiProvider::Deepseek => &mut providers.deepseek,
            ApiProvider::DeepseekCN => &mut providers.deepseek_cn,
            ApiProvider::DeepseekAnthropic => &mut providers.deepseek_anthropic,
            ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
            ApiProvider::Openai => &mut providers.openai,
            ApiProvider::Atlascloud => &mut providers.atlascloud,
            ApiProvider::WanjieArk => &mut providers.wanjie_ark,
            ApiProvider::Openrouter => &mut providers.openrouter,
            ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
            ApiProvider::Novita => &mut providers.novita,
            ApiProvider::Fireworks => &mut providers.fireworks,
            ApiProvider::Siliconflow => &mut providers.siliconflow,
            ApiProvider::SiliconflowCn => &mut providers.siliconflow_cn,
            ApiProvider::Arcee => &mut providers.arcee,
            ApiProvider::Moonshot => &mut providers.moonshot,
            ApiProvider::Sglang => &mut providers.sglang,
            ApiProvider::Vllm => &mut providers.vllm,
            ApiProvider::Ollama => &mut providers.ollama,
            ApiProvider::Volcengine => &mut providers.volcengine,
            ApiProvider::Huggingface => &mut providers.huggingface,
            ApiProvider::Deepinfra => &mut providers.deepinfra,
            ApiProvider::Together => &mut providers.together,
            ApiProvider::Qianfan => &mut providers.qianfan,
            ApiProvider::OpenaiCodex => &mut providers.openai_codex,
            ApiProvider::Anthropic => &mut providers.anthropic,
            ApiProvider::Openmodel => &mut providers.openmodel,
            ApiProvider::Zai => &mut providers.zai,
            ApiProvider::Stepfun => &mut providers.stepfun,
            ApiProvider::Minimax => &mut providers.minimax,
            ApiProvider::Sakana => &mut providers.sakana,
            ApiProvider::LongCat => &mut providers.longcat,
            ApiProvider::Meta => &mut providers.meta,
            ApiProvider::Xai => &mut providers.xai,
            // 由上面按名称键的提前返回处理（#1519）。
            ApiProvider::Custom => unreachable!("custom provider resolved by name above"),
        }
    }

    /// 返回已配置的提供商请求并发上限。
    ///
    /// `None` 意味着客户端不应用额外的进行中请求信号量。
    /// Z.ai/GLM 获得保守的默认值，因为其 SSE 端点在持续并行流打开时
    /// 超时，远低于宣传的服务并发数（#3496）。操作员可以通过
    /// `[providers.zai] max_concurrency = N` 提高它；`0` 显式禁用
    /// 该提供商的客户端上限。
    #[must_use]
    pub fn provider_max_concurrency(&self, provider: ApiProvider) -> Option<usize> {
        let configured = self
            .provider_config_for(provider)
            .and_then(|entry| entry.max_concurrency);
        match configured {
            Some(0) => None,
            Some(limit) => Some(limit.clamp(1, MAX_PROVIDER_REQUEST_CONCURRENCY)),
            None if provider == ApiProvider::Zai => Some(DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY),
            None => None,
        }
    }

    pub(crate) fn provider_config(&self) -> Option<&ProviderConfig> {
        self.provider_config_for(self.api_provider())
    }

    fn provider_config_string_with_runtime_fallback<F>(
        &self,
        provider: ApiProvider,
        get: F,
    ) -> Option<String>
    where
        F: Fn(&ProviderConfig) -> Option<String>,
    {
        if let Some(value) = self.provider_config_for(provider).and_then(&get) {
            return Some(value);
        }
        if provider == ApiProvider::SiliconflowCn {
            return self
                .provider_config_for(ApiProvider::Siliconflow)
                .and_then(get);
        }
        None
    }

    #[must_use]
    pub fn insecure_skip_tls_verify(&self) -> bool {
        self.provider_config()
            .and_then(|provider| provider.insecure_skip_tls_verify)
            .unwrap_or(false)
    }

    #[must_use]
    pub(crate) fn context_window_for_provider_config(&self, provider: ApiProvider) -> Option<u32> {
        if let Some(window) = self
            .provider_config_for(provider)
            .and_then(|entry| entry.context_window)
            .filter(|window| *window > 0)
        {
            return Some(window);
        }
        if provider == ApiProvider::SiliconflowCn {
            return self
                .provider_config_for(ApiProvider::Siliconflow)
                .and_then(|entry| entry.context_window)
                .filter(|window| *window > 0);
        }
        None
    }

    #[must_use]
    pub fn http_headers(&self) -> HashMap<String, String> {
        let mut headers = self.http_headers.clone().unwrap_or_default();
        if let Some(provider_headers) = self
            .provider_config()
            .and_then(|provider| provider.http_headers.as_ref())
        {
            headers.extend(provider_headers.clone());
        }
        headers.retain(|name, value| !name.trim().is_empty() && !value.trim().is_empty());
        headers
    }

    #[must_use]
    pub fn default_model(&self) -> String {
        let provider = self.api_provider();
        if let Some(model) =
            self.provider_config_string_with_runtime_fallback(provider, |entry| entry.model.clone())
        {
            let model = model.trim();
            if provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model()
            {
                return model.to_string();
            }
            if let Some(normalized) = normalize_model_for_provider(provider, model) {
                return normalized;
            }
            // 一个显式的提供商范围的模型，不是已识别的 DeepSeek 别名，
            // 是针对非 DeepSeek 提供商（例如 OpenAI 兼容端点上的
            // `MiniMax-M2.7`）的刻意自定义选择。
            // 它必须按原样传递，而不是回退到 DeepSeek/提供商默认值（问题 #1714）。
            if !matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
                && !model.is_empty()
            {
                return model.to_string();
            }
        }
        // Codex Responses 后端仅服务自己的模型系列，全局的
        // `default_text_model` 被验证限制为 DeepSeek ID 或 "auto"——
        // 因此它永远不能命名 Codex 兼容的模型。在此回退到 Codex 默认值，
        // 而不是让 DeepSeek 默认值泄露并被后端拒绝。显式的
        // `[providers.openai_codex] model` 由上面的块处理。
        if provider == ApiProvider::OpenaiCodex {
            return DEFAULT_OPENAI_CODEX_MODEL.to_string();
        }

        let moonshot_config = (provider == ApiProvider::Moonshot)
            .then(|| self.provider_config())
            .flatten();
        let moonshot_uses_kimi_code = moonshot_config.is_some_and(|config| {
            provider_config_uses_kimi_oauth(config)
                || config
                    .base_url
                    .as_deref()
                    .is_some_and(moonshot_base_url_uses_kimi_code)
        });
        if moonshot_uses_kimi_code {
            return DEFAULT_KIMI_CODE_MODEL.to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && model.trim().eq_ignore_ascii_case("auto")
        {
            return "auto".to_string();
        }
        if provider == ApiProvider::XiaomiMimo
            && let Some(model) = self.default_text_model.as_deref()
            && let Some(canonical) = canonical_xiaomi_mimo_model_id(model)
        {
            return canonical.to_string();
        }
        if provider == ApiProvider::XiaomiMimo {
            return DEFAULT_XIAOMI_MIMO_MODEL.to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && (provider_passes_model_through(provider)
                || self.active_provider_preserves_custom_base_url_model())
        {
            return model.trim().to_string();
        }
        if let Some(model) = self.default_text_model.as_deref()
            && !root_deepseek_model_is_foreign_to_direct_provider(provider, model)
            && let Some(normalized) = normalize_model_name_for_provider(provider, model)
        {
            return normalized;
        }

        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => DEFAULT_TEXT_MODEL,
            ApiProvider::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_MODEL,
            ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_MODEL,
            ApiProvider::Openai => DEFAULT_OPENAI_MODEL,
            ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_MODEL,
            ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_MODEL,
            ApiProvider::Openrouter => DEFAULT_OPENROUTER_MODEL,
            ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_MODEL,
            ApiProvider::Novita => DEFAULT_NOVITA_MODEL,
            ApiProvider::Fireworks => DEFAULT_FIREWORKS_MODEL,
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => DEFAULT_SILICONFLOW_MODEL,
            ApiProvider::Arcee => DEFAULT_ARCEE_MODEL,
            ApiProvider::Moonshot => DEFAULT_MOONSHOT_MODEL,
            ApiProvider::Sglang => DEFAULT_SGLANG_MODEL,
            ApiProvider::Vllm => DEFAULT_VLLM_MODEL,
            ApiProvider::Ollama => DEFAULT_OLLAMA_MODEL,
            ApiProvider::Volcengine => DEFAULT_VOLCENGINE_MODEL,
            ApiProvider::Huggingface => DEFAULT_HUGGINGFACE_MODEL,
            ApiProvider::Deepinfra => DEFAULT_DEEPINFRA_MODEL,
            ApiProvider::Together => DEFAULT_TOGETHER_MODEL,
            ApiProvider::Qianfan => DEFAULT_QIANFAN_MODEL,
            ApiProvider::OpenaiCodex => DEFAULT_OPENAI_CODEX_MODEL,
            ApiProvider::Openmodel => DEFAULT_OPENMODEL_MODEL,
            ApiProvider::Zai => DEFAULT_ZAI_MODEL,
            ApiProvider::Stepfun => DEFAULT_STEPFUN_MODEL,
            ApiProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
            ApiProvider::Minimax => DEFAULT_MINIMAX_MODEL,
            ApiProvider::Sakana => DEFAULT_SAKANA_MODEL,
            ApiProvider::LongCat => DEFAULT_LONGCAT_MODEL,
            ApiProvider::Meta => DEFAULT_META_MODEL,
            ApiProvider::Xai => DEFAULT_XAI_MODEL,
            // Custom endpoints have no built-in default model; pass through the
            // descriptor placeholder when nothing is configured (#1519).
            ApiProvider::Custom => codewhale_config::ProviderKind::Custom
                .provider()
                .default_model(),
        }
        .to_string()
    }

    /// 返回已配置的 API 基础 URL（已规范化）。
    #[must_use]
    pub fn deepseek_base_url(&self) -> String {
        let provider = self.api_provider();
        let provider_base = self
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.base_url.clone());
        // 根 `base_url` 是遗留的 DeepSeek 字段；只有 NvidiaNim 有一个
        // 向后兼容检测（integrate.api.nvidia.com）。OpenRouter / Novita
        // 在 v0.6.7 中添加，需要显式的 `[providers.<name>]` 条目或
        // 相应的 `*_BASE_URL` 环境变量。
        let root_base = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => self.base_url.clone(),
            ApiProvider::DeepseekAnthropic => None,
            ApiProvider::NvidiaNim => self
                .base_url
                .as_ref()
                .filter(|base| base.contains("integrate.api.nvidia.com"))
                .cloned(),
            ApiProvider::Openai
            | ApiProvider::Anthropic
            | ApiProvider::Openmodel
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Openrouter
            | ApiProvider::XiaomiMimo
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Arcee
            | ApiProvider::Moonshot
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Ollama
            | ApiProvider::Volcengine
            | ApiProvider::Huggingface
            | ApiProvider::Deepinfra
            | ApiProvider::Together
            | ApiProvider::Qianfan
            | ApiProvider::OpenaiCodex
            | ApiProvider::Zai
            | ApiProvider::Stepfun
            | ApiProvider::Minimax
            | ApiProvider::Sakana
            | ApiProvider::LongCat
            | ApiProvider::Meta
            | ApiProvider::Xai
            // Custom 从命名的 `[providers.<name>]` 表（通过 provider_base）
            // 读取其 base_url，绝不从遗留的根字段读取。
            | ApiProvider::Custom => None,
        };
        let configured_base_url = provider_base.or(root_base);
        let base = if provider == ApiProvider::XiaomiMimo {
            let config_api_key = self
                .provider_config_for(provider)
                .and_then(|provider| provider.api_key.as_deref());
            let mode = self
                .provider_config_for(provider)
                .and_then(|provider| provider.mode.as_deref());
            let env_api_key =
                xiaomi_mimo_env_api_key_for_runtime(mode, configured_base_url.as_deref());
            let api_key = config_api_key.or(env_api_key.as_deref());
            resolve_xiaomi_mimo_base_url(configured_base_url, api_key, mode)
        } else {
            configured_base_url
                .or_else(env_base_url_override)
                .unwrap_or_else(|| {
                    match provider {
                        ApiProvider::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
                        ApiProvider::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
                        ApiProvider::DeepseekAnthropic => DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL,
                        ApiProvider::NvidiaNim => DEFAULT_NVIDIA_NIM_BASE_URL,
                        ApiProvider::Openai => DEFAULT_OPENAI_BASE_URL,
                        ApiProvider::Atlascloud => DEFAULT_ATLASCLOUD_BASE_URL,
                        ApiProvider::WanjieArk => DEFAULT_WANJIE_ARK_BASE_URL,
                        ApiProvider::Openrouter => DEFAULT_OPENROUTER_BASE_URL,
                        ApiProvider::XiaomiMimo => DEFAULT_XIAOMI_MIMO_BASE_URL,
                        ApiProvider::Novita => DEFAULT_NOVITA_BASE_URL,
                        ApiProvider::Fireworks => DEFAULT_FIREWORKS_BASE_URL,
                        ApiProvider::Siliconflow => DEFAULT_SILICONFLOW_BASE_URL,
                        ApiProvider::SiliconflowCn => DEFAULT_SILICONFLOW_CN_BASE_URL,
                        ApiProvider::Arcee => DEFAULT_ARCEE_BASE_URL,
                        ApiProvider::Moonshot => {
                            if self
                                .provider_config()
                                .is_some_and(provider_config_uses_kimi_oauth)
                            {
                                DEFAULT_KIMI_CODE_BASE_URL
                            } else {
                                DEFAULT_MOONSHOT_BASE_URL
                            }
                        }
                        ApiProvider::Sglang => DEFAULT_SGLANG_BASE_URL,
                        ApiProvider::Vllm => DEFAULT_VLLM_BASE_URL,
                        ApiProvider::Ollama => DEFAULT_OLLAMA_BASE_URL,
                        ApiProvider::Volcengine => DEFAULT_VOLCENGINE_BASE_URL,
                        ApiProvider::Huggingface => DEFAULT_HUGGINGFACE_BASE_URL,
                        ApiProvider::Deepinfra => DEFAULT_DEEPINFRA_BASE_URL,
                        ApiProvider::Together => DEFAULT_TOGETHER_BASE_URL,
                        ApiProvider::Qianfan => DEFAULT_QIANFAN_BASE_URL,
                        ApiProvider::OpenaiCodex => DEFAULT_OPENAI_CODEX_BASE_URL,
                        ApiProvider::Openmodel => DEFAULT_OPENMODEL_BASE_URL,
                        ApiProvider::Zai => DEFAULT_ZAI_BASE_URL,
                        ApiProvider::Stepfun => DEFAULT_STEPFUN_BASE_URL,
                        ApiProvider::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
                        ApiProvider::Minimax => DEFAULT_MINIMAX_BASE_URL,
                        ApiProvider::Sakana => DEFAULT_SAKANA_BASE_URL,
                        ApiProvider::LongCat => DEFAULT_LONGCAT_BASE_URL,
                        ApiProvider::Meta => DEFAULT_META_BASE_URL,
                        ApiProvider::Xai => DEFAULT_XAI_BASE_URL,
                        // No built-in endpoint; descriptor placeholder keeps the
                        // fallback total. A real custom route configures
                        // `[providers.<name>] base_url` which wins above (#1519).
                        ApiProvider::Custom => codewhale_config::ProviderKind::Custom
                            .provider()
                            .default_base_url(),
                    }
                    .to_string()
                })
        };
        normalize_base_url(&base)
    }

    fn active_provider_preserves_custom_base_url_model(&self) -> bool {
        let provider = self.api_provider();
        provider_preserves_custom_base_url_model(provider, &self.deepseek_base_url())
    }

    pub(crate) fn model_ids_pass_through(&self) -> bool {
        let provider = self.api_provider();
        provider_passes_model_through(provider)
            || self.active_provider_preserves_custom_base_url_model()
    }

    /// 读取 API 密钥。
    ///
    /// 优先级：**显式内存覆盖 → 提供商/根配置 → 环境**。
    ///
    /// 仅当用户显式设置了该字段时，内存中的 `self.api_key` 覆盖才被
    /// 遵守（不是遗留的 `API_KEYRING_SENTINEL` 占位符，不是空白）。
    pub fn deepseek_api_key(&self) -> Result<String> {
        let provider = self.api_provider();

        // 0. DeepSeek 兼容性槽位。遗留的顶级 `api_key`
        // 仅属于 DeepSeek；下面的提供商特定密钥必须对 NIM/OpenRouter 等
        // 优先，以便过时的 DeepSeek 密钥不会被发送到其他地方。
        //
        // 然而，当 CLI 分发器通过 `DEEPSEEK_API_KEY` 和分发器源标记
        // 转发显式的 `--api-key` 时，该有意的覆盖必须优先于保存的根密钥。
        // 这对于 DeepSeek 兼容的订阅端点至关重要，用户运行类似：
        //   codewhale --provider deepseek --api-key ark-... --base-url ... --model auto
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            && std::env::var("DEEPSEEK_API_KEY_SOURCE").as_deref() == Ok("cli")
            && let Some(env_key) = provider_env_api_key(provider)
            && !env_key.trim().is_empty()
        {
            return Ok(env_key);
        }
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
            && let Some(configured) = self.api_key.as_ref()
            && !configured.trim().is_empty()
            && configured != API_KEYRING_SENTINEL
        {
            return Ok(configured.clone());
        }

        if provider == ApiProvider::Moonshot
            && self
                .provider_config_for(provider)
                .is_some_and(provider_config_uses_kimi_oauth)
        {
            return kimi_cli_oauth_access_token();
        }

        // xAI / Grok OAuth 复用 ~/.grok/auth.json（Grok CLI）或
        // 以相同格式编写的设备码登录。由
        // [providers.xai] auth_mode = "oauth" 激活（#4257 残留）。
        if provider == ApiProvider::Xai
            && self
                .provider_config_for(provider)
                .is_some_and(provider_config_uses_xai_oauth)
        {
            return crate::xai_oauth::get_access_token();
        }

        // OpenAI Codex (ChatGPT) 复用现有的 Codex CLI OAuth 登录。
        // 访问令牌存在于 ~/.codex/auth.json 中（按需刷新），
        // 而不是存储的 API 密钥，因此在配置文件和环境槽位之前
        // 解析它。显式环境覆盖在 `get_credentials` 内部处理。
        if provider == ApiProvider::OpenaiCodex {
            return Ok(crate::oauth::get_credentials()?.access_token);
        }

        // 1. 配置文件（提供商范围的槽位）。这有意优先于
        // 环境变量，以便 `codewhale auth set` 修复过时的 shell 导出。
        if let Some(configured) = self
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
            && !configured.trim().is_empty()
        {
            return Ok(configured);
        }

        // 1b. 自定义提供商（#1519）通过 `[providers.<name>] api_key_env = "..."`
        // 为每个条目命名其认证环境变量。在通用环境步骤之前解析它，
        // 因为自定义标识声明了没有内置环境变量。
        // 环境变量名称从配置中读取；密钥值从进程环境中读取，永不持久化。
        if provider == ApiProvider::Custom
            && let Some(env_name) = self
                .provider_config_for(provider)
                .and_then(|entry| entry.api_key_env.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
            && let Ok(value) = std::env::var(env_name)
            && !value.trim().is_empty()
        {
            return Ok(value);
        }

        // 2. 环境变量。不要在此查询平台凭据存储；
        // 常规启动和医生检查必须保持无提示。
        if provider == ApiProvider::XiaomiMimo {
            let mode = self
                .provider_config_for(provider)
                .and_then(|provider| provider.mode.as_deref());
            if let Some(value) =
                xiaomi_mimo_env_api_key_for_runtime(mode, Some(&self.deepseek_base_url()))
                && !value.trim().is_empty()
            {
                return Ok(value);
            }
        }
        if let Some(value) = provider_env_api_key(provider) {
            return Ok(value);
        }

        if base_url_uses_local_host(&self.deepseek_base_url()) {
            return Ok(String::new());
        }

        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => anyhow::bail!(
                "DeepSeek API key not found.\n\
                 \n\
                 1. Get a key:  https://platform.deepseek.com/api_keys\n\
                 2. Save it (works in every folder, no OS prompts):\n\
                        codewhale auth set --provider deepseek\n\
                 \n\
                 Alternatives:\n\
                   • export DEEPSEEK_API_KEY=<your-key>      (current shell only;\n\
                     also note: zsh users — exports in ~/.zshrc only reach interactive\n\
                     shells, prefer ~/.zshenv for everything)\n\
                   • api_key = \"<your-key>\"  in ~/.codewhale/config.toml"
            ),
            ApiProvider::SiliconflowCn => anyhow::bail!(
                "SiliconFlow China API key not found. Get a key: {}. Run 'codewhale auth set --provider siliconflow-CN', \
                 set {}, or add [{}] api_key in ~/.codewhale/config.toml. \
                 [providers.siliconflow] remains a fallback when the CN table omits api_key.",
                provider
                    .credential_url()
                    .unwrap_or("https://cloud.siliconflow.com/account/ak"),
                provider.env_vars_label(),
                provider_config_table_name(provider)?
            ),
            ApiProvider::Moonshot => anyhow::bail!(
                "Moonshot/Kimi API key not found. Get a key: {}. Run 'codewhale auth set --provider moonshot', \
                 set {}, or add [{}] api_key. \
                 For a Kimi Code plan key, set [providers.moonshot] base_url = \
                 \"https://api.kimi.com/coding/v1\" and model = \"kimi-for-coding\".",
                provider
                    .credential_url()
                    .unwrap_or("https://platform.kimi.ai/"),
                provider.env_vars_label(),
                provider_config_table_name(provider)?
            ),
            ApiProvider::Anthropic | ApiProvider::Openmodel => {
                anyhow::bail!("{}", missing_provider_api_key_message(provider)?)
            }
            ApiProvider::OpenaiCodex => anyhow::bail!("{}", crate::oauth::missing_auth_message()),
            ApiProvider::Xai => {
                // Prefer OAuth guidance when auth_mode requests it or Grok CLI
                // tokens already exist; otherwise show both API-key and OAuth.
                if self
                    .provider_config_for(provider)
                    .is_some_and(provider_config_uses_xai_oauth)
                    || crate::xai_oauth::credentials_present()
                {
                    anyhow::bail!("{}", crate::xai_oauth::missing_auth_message());
                }
                anyhow::bail!(
                    "xAI API key not found. Get a key: https://console.x.ai/\n\
                     Run 'codewhale auth set --provider xai', set XAI_API_KEY, or add \
                     [providers.xai] api_key.\n\
                     OAuth alternative: run `grok login` (or device-code login) and set \
                     [providers.xai] auth_mode = \"oauth\"."
                );
            }
            // Self-hosted deployments commonly run without auth on localhost.
            // Return an empty key and let the client omit the Authorization header.
            ApiProvider::Sglang | ApiProvider::Vllm | ApiProvider::Ollama => Ok(String::new()),
            // 自定义 OpenAI 兼容端点（#1519）：密钥来自
            // `[providers.<name>] api_key_env` 命名的环境变量。
            // 如果我们到达这里，它未设置/为空（且端点不是回环）。
            ApiProvider::Custom => {
                let provider_name = self.provider.as_deref().unwrap_or("<name>");
                match self
                    .provider_config_for(provider)
                    .and_then(|entry| entry.api_key_env.as_deref())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    Some(env_name) => anyhow::bail!(
                        "Custom provider '{provider_name}' API key not found.\n\
                         Set the environment variable {env_name} to your key, \
                         or add api_key to [providers.{provider_name}]."
                    ),
                    None => anyhow::bail!(
                        "Custom provider '{provider_name}' has no auth configured.\n\
                         Add api_key_env = \"YOUR_ENV_VAR\" (or api_key) to \
                         [providers.{provider_name}] in ~/.codewhale/config.toml."
                    ),
                }
            }
            _ => anyhow::bail!("{}", missing_provider_api_key_message(provider)?),
        }
    }

    /// 解析技能目录路径。
    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.skills_dir
            .as_deref()
            .map(expand_path)
            .or_else(default_skills_dir)
            .unwrap_or_else(|| PathBuf::from("./skills"))
    }

    /// 解析 MCP 配置路径。
    #[must_use]
    pub fn mcp_config_path(&self) -> PathBuf {
        self.mcp_config_path
            .as_deref()
            .map(expand_path)
            .or_else(default_mcp_config_path)
            .unwrap_or_else(|| PathBuf::from("./mcp.json"))
    }

    /// 解析笔记文件路径。
    #[must_use]
    pub fn notes_path(&self) -> PathBuf {
        self.notes_path
            .as_deref()
            .map(expand_path)
            .or_else(default_notes_path)
            .unwrap_or_else(|| PathBuf::from("./notes.txt"))
    }

    /// 解析记忆文件路径。
    #[must_use]
    pub fn memory_path(&self) -> PathBuf {
        self.memory_path
            .as_deref()
            .map(expand_path)
            .or_else(default_memory_path)
            .unwrap_or_else(|| PathBuf::from("./memory.md"))
    }

    /// 解析默认语音/TTS 输出目录（如果已配置）。
    #[must_use]
    pub fn speech_output_dir(&self) -> Option<PathBuf> {
        std::env::var("XIAOMI_MIMO_SPEECH_OUTPUT_DIR")
            .or_else(|_| std::env::var("MIMO_SPEECH_OUTPUT_DIR"))
            .or_else(|_| std::env::var("XIAOMIMIMO_SPEECH_OUTPUT_DIR"))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| expand_path(&value))
            .or_else(|| {
                self.speech
                    .as_ref()
                    .and_then(|speech| speech.output_dir.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(expand_path)
            })
    }

    /// 解析已配置的 `instructions = [...]` 数组（#454）
    /// 到绝对路径，按声明顺序。当未设置或所有条目在修剪后
    /// 为空时为空。每个条目通过 `expand_path` 运行，
    /// 因此 `~` 和环境变量被遵守。
    #[must_use]
    pub fn instructions_paths(&self) -> Vec<PathBuf> {
        self.instructions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(expand_path)
            .collect()
    }

    /// 用户记忆功能是否启用。默认是**关闭**，
    /// 以保持未选择加入用户的零开销行为。
    /// 当 `config.toml` 中的 `[memory] enabled = true` 或
    /// 环境中设置了 `DEEPSEEK_MEMORY=on` 时翻转为 `true`。
    #[must_use]
    pub fn memory_enabled(&self) -> bool {
        self.memory
            .as_ref()
            .and_then(|m| m.enabled)
            .unwrap_or(false)
    }

    /// 遗留的 `memory.rs` 推送/注入路径是否已被弃用，转而支持
    /// Moraine MCP 召回。当为 `true` 时，`<user_memory>` 块被跳过，
    /// `remember` 工具不被注册，`# foo` 快速添加回退到正常轮次提交，
    /// 即使 `memory_enabled()` 返回 `true`。默认 `false`。
    #[must_use]
    pub fn moraine_fallback(&self) -> bool {
        self.memory
            .as_ref()
            .and_then(|m| m.moraine_fallback)
            .unwrap_or(false)
    }

    /// 返回已配置的视觉模型配置，从主配置继承 api_key。
    #[must_use]
    pub fn vision_model_config(&self) -> Option<VisionModelConfig> {
        let mut config = self.vision_model.clone()?;
        if config.api_key.is_none() {
            config.api_key = self.api_key.clone();
        }
        Some(config)
    }

    #[must_use]
    pub fn project_context_pack_enabled(&self) -> bool {
        self.context.project_pack.unwrap_or(true)
    }

    /// 返回是否允许非交互式和持久任务配置文件的 shell 执行。
    /// 默认为 `false`：在无头、应用服务器和后台任务上下文中没有人类
    /// 来批准命令，因此 shell 访问必须显式选择加入（GHSA-72w5-pf8h-xfp4）。
    #[must_use]
    pub fn allow_shell(&self) -> bool {
        self.allow_shell.unwrap_or(false)
    }

    /// 返回是否允许*交互式* TUI Agent 会话的 shell 执行。
    /// 默认为 `true`：交互式编辑器始终在每个 shell 命令后设置审批提示，
    /// 因此目录可以默认暴露 shell，同时仍保留同意（GHSA-72w5-pf8h-xfp4）。
    /// 显式的 `allow_shell = false` 仍然隐藏 shell 工具。
    /// 这是交互式默认值的唯一真相来源；启动（`run_interactive`）和
    /// 持久 Agent 权限基线都读取它，因此默认值不会在它们之间漂移。
    #[must_use]
    pub fn interactive_allow_shell(&self) -> bool {
        self.allow_shell.unwrap_or(true)
    }

    /// 是否启用幽灵文本提示建议（选择加入，默认关闭）。
    pub fn prompt_suggestion_enabled(&self) -> bool {
        self.prompt_suggestion.unwrap_or(false)
    }

    /// 返回最大并发子代理数。
    /// 先检查 `[subagents] max_concurrent`，然后是顶级 `max_subagents`，
    /// 然后回退到 `DEFAULT_MAX_SUBAGENTS`。
    #[must_use]
    pub fn max_subagents(&self) -> usize {
        // 先检查 [subagents] max_concurrent
        if let Some(subagents_cfg) = self.subagents.as_ref()
            && let Some(max) = subagents_cfg.max_concurrent
        {
            return max.clamp(1, MAX_SUBAGENTS);
        }
        // 回退到顶级 max_subagents
        self.max_subagents
            .unwrap_or(DEFAULT_MAX_SUBAGENTS)
            .clamp(1, MAX_SUBAGENTS)
    }

    /// 返回提供商特定的最大并发子代理数。
    /// `[subagents.providers.<provider>] max_concurrent` 在未设置时
    /// 从全局 `[subagents]` 值继承。
    #[must_use]
    pub fn max_subagents_for_provider(&self, provider: ApiProvider) -> usize {
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.max_concurrent)
            .map(|max| max.clamp(1, MAX_SUBAGENTS))
            .unwrap_or_else(|| self.max_subagents())
    }

    /// 在应用特性标志、显式的 `[subagents] enabled` 开关和遗留的
    /// 零值选择退出后，面向模型的 `agent` 工具是否可用。
    #[must_use]
    pub fn subagents_enabled(&self) -> bool {
        self.subagents_disabled_reason().is_none()
    }

    /// 在应用全局和提供商特定的子代理控制后，面向模型的 `agent` 工具
    /// 对此提供商是否可用。
    #[must_use]
    pub fn subagents_enabled_for_provider(&self, provider: ApiProvider) -> bool {
        if !self.subagents_enabled() {
            return false;
        }
        let Some(provider_cfg) = self.subagent_provider_config(provider) else {
            return true;
        };
        provider_cfg.enabled != Some(false)
            && provider_cfg.max_concurrent != Some(0)
            && provider_cfg.max_depth != Some(0)
    }

    /// 子代理被禁用的机器可读原因，按优先级顺序。
    #[must_use]
    pub fn subagents_disabled_reason(&self) -> Option<&'static str> {
        if !self.features().enabled(Feature::Subagents) {
            return Some("features.subagents=false");
        }
        let subagents_cfg = self.subagents.as_ref()?;
        if subagents_cfg.enabled == Some(false) {
            return Some("subagents.enabled=false");
        }
        if subagents_cfg.max_concurrent == Some(0) {
            return Some("subagents.max_concurrent=0");
        }
        if subagents_cfg.max_depth == Some(0) {
            return Some("subagents.max_depth=0");
        }
        None
    }

    /// 交互式 `agent` 工具可以生成的嵌套子代理层数。
    /// 读取 `[subagents] max_depth`；未设置时默认为
    /// [`codewhale_config::DEFAULT_SPAWN_DEPTH`]。`0` 是一个有效值，
    /// 在此运行时深度阻止 `agent` 工具。任何值都被限制在
    /// [`codewhale_config::MAX_SPAWN_DEPTH_CEILING`]，
    /// 因此操作员的选择永远不会超过硬递归上限。
    #[must_use]
    pub fn subagent_max_spawn_depth(&self) -> u32 {
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.max_depth)
            .unwrap_or(codewhale_config::DEFAULT_SPAWN_DEPTH)
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
    }

    /// 返回提供商特定的最大子代理递归深度。
    #[must_use]
    pub fn subagent_max_spawn_depth_for_provider(&self, provider: ApiProvider) -> u32 {
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.max_depth)
            .unwrap_or_else(|| self.subagent_max_spawn_depth())
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING)
    }

    /// 在进一步启动排队等待启动槽之前可同时执行的直接（深度-1）
    /// 子代理数（#3095）。读取 `[subagents] launch_concurrency`
    ///（或已弃用的 `interactive_max_launch` 别名）；未设置时默认为
    /// 完整解析的 `max_subagents()`（无人工节流），任何显式值
    /// 被限制在 `[1, max_subagents]`。
    #[must_use]
    pub fn launch_concurrency(&self) -> usize {
        let max = self.max_subagents();
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.launch_concurrency.or(cfg.interactive_max_launch_legacy))
            .unwrap_or(max)
            .clamp(1, max)
    }

    /// 返回提供商特定的直接启动节流。超过此限制的子项
    /// 排队等待启动槽，而不是立即启动。
    #[must_use]
    pub fn launch_concurrency_for_provider(&self, provider: ApiProvider) -> usize {
        let max = self.max_subagents_for_provider(provider);
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.launch_concurrency)
            .or_else(|| {
                self.subagents
                    .as_ref()
                    .and_then(|cfg| cfg.launch_concurrency.or(cfg.interactive_max_launch_legacy))
            })
            .unwrap_or(max)
            .clamp(1, max)
    }

    /// 会话允许的最大排队 + 运行中的子代理数。
    ///
    /// 默认为 [`MAX_SUBAGENT_ADMISSION`]，以便不同的 `agent` 调用可以
    /// 通过 `launch_concurrency` 排队和耗尽，而不是在瞬时并发上限处被拒绝。
    /// 显式值被限制在 `[max_subagents, MAX_SUBAGENT_ADMISSION]`。
    #[must_use]
    pub fn max_admitted_subagents(&self) -> usize {
        let max_concurrent = self.max_subagents();
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.max_admitted)
            .unwrap_or(MAX_SUBAGENT_ADMISSION)
            .clamp(max_concurrent, MAX_SUBAGENT_ADMISSION)
    }

    /// 返回提供商特定的排队 + 运行允许上限。
    #[must_use]
    pub fn max_admitted_subagents_for_provider(&self, provider: ApiProvider) -> usize {
        let max_concurrent = self.max_subagents_for_provider(provider);
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.max_admitted)
            .or_else(|| self.subagents.as_ref().and_then(|cfg| cfg.max_admitted))
            .unwrap_or(MAX_SUBAGENT_ADMISSION)
            .clamp(max_concurrent, MAX_SUBAGENT_ADMISSION)
    }

    /// 每个根 `agent` 运行的可选聚合 token 预算。
    ///
    /// 读取 `[subagents] token_budget`。`None` 和 `0` 都表示无限制，
    /// 保持遗留行为，直到显式配置了预算。
    #[must_use]
    pub fn subagent_token_budget(&self) -> Option<u64> {
        self.subagents
            .as_ref()
            .and_then(|cfg| cfg.token_budget)
            .filter(|budget| *budget > 0)
    }

    /// 返回每个根 `agent` 运行的提供商特定聚合 token 预算。
    #[must_use]
    pub fn subagent_token_budget_for_provider(&self, provider: ApiProvider) -> Option<u64> {
        self.subagent_provider_config(provider)
            .and_then(|cfg| cfg.token_budget)
            .or_else(|| self.subagents.as_ref().and_then(|cfg| cfg.token_budget))
            .filter(|budget| *budget > 0)
    }

    /// 已解析的子代理每步 DeepSeek API 超时时间（秒）。
    ///
    /// 读取 `[subagents] api_timeout_secs` 并限制在
    /// `[MIN_SUBAGENT_API_TIMEOUT_SECS, MAX_SUBAGENT_API_TIMEOUT_SECS]`
    /// (1..=1800)。`None` 或 `0` 解析为遗留的
    /// `DEFAULT_SUBAGENT_API_TIMEOUT_SECS` (120)，以便现有配置保持
    /// 其旧行为；显式的 `1` 被遵守，仅在快速失败测试中有用，
    /// 不用于生产环境（#1806, #1808）。
    #[must_use]
    pub fn subagent_api_timeout_secs(&self) -> u64 {
        resolve_subagent_api_timeout_secs(
            self.subagents.as_ref().and_then(|cfg| cfg.api_timeout_secs),
        )
    }

    /// 返回提供商特定的子代理每步 API 超时时间。
    #[must_use]
    pub fn subagent_api_timeout_secs_for_provider(&self, provider: ApiProvider) -> u64 {
        resolve_subagent_api_timeout_secs(
            self.subagent_provider_config(provider)
                .and_then(|cfg| cfg.api_timeout_secs)
                .or_else(|| self.subagents.as_ref().and_then(|cfg| cfg.api_timeout_secs)),
        )
    }

    /// 已解析的运行中子代理的无进展心跳超时时间。
    ///
    /// 读取 `[subagents] heartbeat_timeout_secs` 并限制在
    /// `[MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS, MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS]`。
    /// `None` 或 `0` 解析为默认 300 秒。最终值也保持在
    /// `subagent_api_timeout_secs()` 之上至少 30 秒，
    /// 以便已配置的长模型请求不会被心跳清理抢占。
    #[must_use]
    pub fn subagent_heartbeat_timeout_secs(&self) -> u64 {
        resolve_subagent_heartbeat_timeout_secs(
            self.subagents
                .as_ref()
                .and_then(|cfg| cfg.heartbeat_timeout_secs),
            self.subagent_api_timeout_secs(),
        )
    }

    /// 返回提供商特定的无进展心跳超时时间。
    #[must_use]
    pub fn subagent_heartbeat_timeout_secs_for_provider(&self, provider: ApiProvider) -> u64 {
        let api_timeout = self.subagent_api_timeout_secs_for_provider(provider);
        resolve_subagent_heartbeat_timeout_secs(
            self.subagent_provider_config(provider)
                .and_then(|cfg| cfg.heartbeat_timeout_secs)
                .or_else(|| {
                    self.subagents
                        .as_ref()
                        .and_then(|cfg| cfg.heartbeat_timeout_secs)
                }),
            api_timeout,
        )
    }

    /// 已解析的每个 SSE 块空闲超时时间（秒）。
    ///
    /// 读取 `[tui].stream_chunk_timeout_secs`，当配置键省略时回退到
    /// 遗留的 `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` 环境变量。
    /// `None` 或 `0` 解析为默认 900 秒；显式值限制在 `1..=3600`。
    #[must_use]
    pub fn stream_chunk_timeout_secs(&self) -> u64 {
        let raw = self
            .tui
            .as_ref()
            .and_then(|cfg| cfg.stream_chunk_timeout_secs)
            .or_else(|| {
                std::env::var(STREAM_CHUNK_TIMEOUT_ENV)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(DEFAULT_STREAM_CHUNK_TIMEOUT_SECS);
        if raw == 0 {
            return DEFAULT_STREAM_CHUNK_TIMEOUT_SECS;
        }
        raw.clamp(MIN_STREAM_CHUNK_TIMEOUT_SECS, MAX_STREAM_CHUNK_TIMEOUT_SECS)
    }

    /// 原始子代理模型覆盖映射。值在生成时验证，
    /// 因此无效的角色/类型模型在任何部分代理生成之前就失败。
    #[must_use]
    pub fn subagent_model_overrides(&self) -> HashMap<String, String> {
        let mut overrides = HashMap::new();
        let Some(cfg) = self.subagents.as_ref() else {
            return overrides;
        };

        let mut insert = |key: &str, value: &Option<String>| {
            if let Some(model) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                overrides.insert(key.to_string(), model.to_string());
            }
        };
        insert("default", &cfg.default_model);
        insert("worker", &cfg.worker_model);
        insert("general", &cfg.worker_model);
        insert("explorer", &cfg.explorer_model);
        insert("explore", &cfg.explorer_model);
        insert("awaiter", &cfg.awaiter_model);
        insert("plan", &cfg.awaiter_model);
        insert("review", &cfg.review_model);
        insert("custom", &cfg.custom_model);

        if let Some(models) = cfg.models.as_ref() {
            for (key, model) in models {
                let key = key.trim();
                let model = model.trim();
                if !key.is_empty() && !model.is_empty() {
                    overrides.insert(key.to_ascii_lowercase(), model.to_string());
                }
            }
        }

        overrides
    }

    /// 已解析的 `[fleet]` 表，或表不存在时的默认值
    ///（#fleet-roster cutover (v0.8.67)）。
    #[must_use]
    pub fn fleet_config(&self) -> codewhale_config::FleetConfigToml {
        self.fleet.clone().unwrap_or_default()
    }

    /// 已解析的 `[workflow]` 表，或表不存在时的产品默认值
    ///（#4128 / Section 2.11）。自动启动、审批、隔离和活动持久化
    /// 消费者应通过此访问器读取，以便省略的键共享一个模型。
    #[must_use]
    pub fn workflow_config(&self) -> codewhale_config::WorkflowConfigToml {
        self.workflow.clone().unwrap_or_default()
    }

    /// 返回已配置的 DeepSeek 推理努力级别（如果有）。
    #[must_use]
    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    /// 获取钩子配置，如果未配置则返回默认值。
    pub fn hooks_config(&self) -> HooksConfig {
        self.hooks.clone().unwrap_or_default()
    }

    /// 解析应用了默认值的通知配置。
    #[must_use]
    pub fn notifications_config(&self) -> NotificationsConfig {
        self.notifications.clone().unwrap_or_default()
    }

    /// 解析应用了默认值的工作区侧 git 快照设置。
    #[must_use]
    pub fn snapshots_config(&self) -> SnapshotsConfig {
        self.snapshots.clone().unwrap_or_default()
    }

    /// 解析应用了默认值的社区技能设置。
    #[must_use]
    pub fn skills_config(&self) -> SkillsConfig {
        self.skills.clone().unwrap_or_default()
    }

    /// 解析应用了默认值的启动更新检查设置。
    #[must_use]
    pub fn update_config(&self) -> UpdateConfig {
        self.update.clone().unwrap_or_default()
    }

    /// 解析渲染/分发层的持久快捷键栏绑定。
    #[must_use]
    pub fn resolve_hotbar_bindings(
        &self,
        known_action_ids: &[&str],
    ) -> codewhale_config::HotbarConfigResolution {
        codewhale_config::resolve_hotbar_bindings(self.hotbar.as_deref(), known_action_ids)
    }

    /// 从默认值和配置条目解析已启用的特性。
    #[must_use]
    pub fn features(&self) -> Features {
        let mut features = Features::with_defaults();
        if let Some(table) = &self.features {
            features.apply_map(&table.entries);
        }
        features
    }

    /// 在内存中覆盖特性标志（由 CLI 覆盖使用）。
    pub fn set_feature(&mut self, key: &str, enabled: bool) -> Result<()> {
        if !is_known_feature_key(key) {
            anyhow::bail!("Unknown feature flag: {key}");
        }
        let table = self.features.get_or_insert_with(FeaturesToml::default);
        table.entries.insert(key.to_string(), enabled);
        Ok(())
    }

    /// 解析应用了默认值的有效重试策略。
    #[must_use]
    pub fn retry_policy(&self) -> RetryPolicy {
        let defaults = RetryPolicy {
            enabled: true,
            max_retries: 3,
            initial_delay: 1.0,
            max_delay: 60.0,
            exponential_base: 2.0,
        };

        let Some(cfg) = &self.retry else {
            return defaults;
        };

        RetryPolicy {
            enabled: cfg.enabled.unwrap_or(defaults.enabled),
            max_retries: cfg.max_retries.unwrap_or(defaults.max_retries),
            initial_delay: cfg.initial_delay.unwrap_or(defaults.initial_delay),
            max_delay: cfg.max_delay.unwrap_or(defaults.max_delay),
            exponential_base: cfg.exponential_base.unwrap_or(defaults.exponential_base),
        }
    }
}

fn root_deepseek_model_is_foreign_to_direct_provider(provider: ApiProvider, model: &str) -> bool {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        || provider_passes_model_through(provider)
    {
        return false;
    }
    if matches!(
        provider,
        ApiProvider::NvidiaNim
            | ApiProvider::Openrouter
            | ApiProvider::Novita
            | ApiProvider::Fireworks
            | ApiProvider::Siliconflow
            | ApiProvider::SiliconflowCn
            | ApiProvider::Deepinfra
            | ApiProvider::Together
            | ApiProvider::Sglang
            | ApiProvider::Vllm
            | ApiProvider::Volcengine
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
    ) {
        return false;
    }
    normalize_model_name(model).is_some()
}

// === 默认值 ===

// 纯文件系统路径辅助函数位于 `paths` 叶子模块中。两个
// `pub(crate)` 入口点被重新导出，以便外部 `crate::config::` 调用者
// 解析不变；其余辅助函数被私有导入，用于保留在此文件中的
// 工作区信任/配置加载逻辑（#3311）。
mod paths;
use paths::{
    canonicalize_or_keep, codewhale_home_dir, default_config_path, default_managed_config_path,
    default_mcp_config_path, default_memory_path, default_notes_path, default_requirements_path,
    default_skills_dir, env_config_path, expand_pathbuf, home_config_path, workspace_config_key,
};
pub(crate) use paths::{effective_home_dir, expand_path};

pub(crate) fn workspace_trust_config_candidate_paths() -> Vec<PathBuf> {
    if let Some(path) = env_config_path() {
        return vec![path];
    }

    if let Some(codewhale_home) = codewhale_home_dir() {
        return vec![codewhale_home.join("config.toml")];
    }

    let Some(home) = effective_home_dir() else {
        return Vec::new();
    };
    vec![
        home.join(".codewhale").join("config.toml"),
        home.join(".deepseek").join("config.toml"),
    ]
}

#[must_use]
pub(crate) fn is_workspace_trusted(workspace: &Path) -> bool {
    let Some(config_path) = default_config_path() else {
        return false;
    };
    let Ok(raw) = fs::read_to_string(config_path) else {
        return false;
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&raw) else {
        return false;
    };
    workspace_trust_level_from_doc(&doc, workspace).is_some_and(is_trusted_level)
}

pub(crate) fn save_workspace_trust(workspace: &Path) -> Result<PathBuf> {
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;
    ensure_parent_dir(&config_path)?;

    let project_key = workspace_config_key(workspace);
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["projects", project_key.as_str(), "trust_level"],
            "trusted",
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

fn workspace_trust_level_from_doc<'a>(doc: &'a toml::Value, workspace: &Path) -> Option<&'a str> {
    let workspace = canonicalize_or_keep(workspace);
    let projects = doc.get("projects")?.as_table()?;
    for (raw_path, project) in projects {
        let project_path = canonicalize_or_keep(&expand_path(raw_path));
        if project_path == workspace {
            return project.get("trust_level").and_then(toml::Value::as_str);
        }
    }
    None
}

fn is_trusted_level(level: &str) -> bool {
    level.trim().eq_ignore_ascii_case("trusted")
}

pub(crate) fn resolve_load_config_path(path: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = path {
        return Some(expand_pathbuf(path));
    }

    if let Some(path) = env_config_path() {
        if path.exists() {
            return Some(path);
        }

        if let Some(home_path) = home_config_path()
            && home_path.exists()
        {
            return Some(home_path);
        }

        return Some(path);
    }

    home_config_path()
}

/// 在首次交互式启动时创建可检查的配置文件。
///
/// 该文件有意省略 `api_key`；入职流程或 `codewhale auth set`
/// 在用户提供密钥后写入该字段。
pub fn ensure_config_file_exists(path: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let config_path = path
        .map(expand_pathbuf)
        .or_else(default_config_path)
        .context("Failed to resolve config path: home directory not found.")?;
    if config_path.exists() {
        return Ok(None);
    }

    ensure_parent_dir(&config_path)?;
    let content = format!(
        r#"# codewhale Configuration
# Get your API key from https://platform.deepseek.com
# Save it with: codewhale auth set --provider deepseek

# Base URL (default: https://api.deepseek.com/beta)
# Set https://api.deepseek.com to opt out of beta features.
# base_url = "https://api.deepseek.com/beta"

# Default model
default_text_model = "{DEFAULT_TEXT_MODEL}"

# Thinking mode (DeepSeek V4 reasoning effort):
# "auto" | "off" | "low" | "medium" | "high" | "max"
# Shift+Tab in the TUI cycles between off / high / max.
reasoning_effort = "auto"

# Startup update check
[update]
check_for_updates = true
# update_uri = "https://internal.mirror.example/codewhale/releases/latest"
"#
    );
    write_config_file_secure(&config_path, &content)
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(Some(config_path))
}

// === 环境覆盖 ===

/// 读取 CLI 分发器从 `--base-url` 转发的 `DEEPSEEK_BASE_URL` /
/// `CODEWHALE_BASE_URL` 环境变量。当变量不存在或为空时返回 `None`，
/// 以便提供商特定的默认值仍然适用。
fn env_base_url_override() -> Option<String> {
    codewhale_env_var("CODEWHALE_BASE_URL", "DEEPSEEK_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// 解析环境变量，优先使用 `CODEWHALE_*` 形式而不是遗留的
/// `DEEPSEEK_*` 形式。忽略空值，以便空白的 shell 导出
/// 不会擦除已配置的提供商设置。
fn codewhale_env_var(
    codewhale_name: &str,
    legacy_name: &str,
) -> Result<String, std::env::VarError> {
    std::env::var(codewhale_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var(legacy_name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or(std::env::VarError::NotPresent)
}

fn apply_env_overrides(config: &mut Config) {
    if let Ok(value) = codewhale_env_var("CODEWHALE_PROVIDER", "DEEPSEEK_PROVIDER") {
        config.provider = Some(value);
    }
    if let Ok(value) = codewhale_env_var("CODEWHALE_BASE_URL", "DEEPSEEK_BASE_URL") {
        match config.api_provider() {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                config.base_url = Some(value);
            }
            ApiProvider::DeepseekAnthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .deepseek_anthropic
                    .base_url = Some(value);
            }
            ApiProvider::NvidiaNim => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .nvidia_nim
                    .base_url = Some(value);
            }
            ApiProvider::Openai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openai
                    .base_url = Some(value);
            }
            ApiProvider::Anthropic => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .anthropic
                    .base_url = Some(value);
            }
            ApiProvider::Openmodel => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openmodel
                    .base_url = Some(value);
            }
            ApiProvider::Openrouter => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openrouter
                    .base_url = Some(value);
            }
            ApiProvider::XiaomiMimo => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .xiaomi_mimo
                    .base_url = Some(value);
            }
            ApiProvider::WanjieArk => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .wanjie_ark
                    .base_url = Some(value);
            }
            ApiProvider::Novita => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .novita
                    .base_url = Some(value);
            }
            ApiProvider::Fireworks => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .fireworks
                    .base_url = Some(value);
            }
            ApiProvider::Siliconflow => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .siliconflow
                    .base_url = Some(value);
            }
            ApiProvider::SiliconflowCn => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .siliconflow_cn
                    .base_url = Some(value);
            }
            ApiProvider::Arcee => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .arcee
                    .base_url = Some(value);
            }
            ApiProvider::Moonshot => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .moonshot
                    .base_url = Some(value);
            }
            ApiProvider::Sglang => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .sglang
                    .base_url = Some(value);
            }
            ApiProvider::Vllm => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .vllm
                    .base_url = Some(value);
            }
            ApiProvider::Ollama => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .ollama
                    .base_url = Some(value);
            }
            ApiProvider::Volcengine => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .volcengine
                    .base_url = Some(value);
            }
            ApiProvider::Atlascloud => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .atlascloud
                    .base_url = Some(value);
            }
            ApiProvider::Huggingface => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .huggingface
                    .base_url = Some(value);
            }
            ApiProvider::Deepinfra => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .deepinfra
                    .base_url = Some(value);
            }
            ApiProvider::Together => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .together
                    .base_url = Some(value);
            }
            ApiProvider::Qianfan => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .qianfan
                    .base_url = Some(value);
            }
            ApiProvider::OpenaiCodex => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .openai_codex
                    .base_url = Some(value);
            }
            ApiProvider::Zai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .zai
                    .base_url = Some(value);
            }
            ApiProvider::Stepfun => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .stepfun
                    .base_url = Some(value);
            }
            ApiProvider::Minimax => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .minimax
                    .base_url = Some(value);
            }
            ApiProvider::Sakana => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .sakana
                    .base_url = Some(value);
            }
            ApiProvider::LongCat => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .longcat
                    .base_url = Some(value);
            }
            ApiProvider::Meta => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .meta
                    .base_url = Some(value);
            }
            ApiProvider::Xai => {
                config
                    .providers
                    .get_or_insert_with(ProvidersConfig::default)
                    .xai
                    .base_url = Some(value);
            }
            // Custom 解析到命名的 `[providers.<name>]` 表；通过名称键的
            // 可变访问器路由覆盖（#1519）。
            ApiProvider::Custom => {
                config.provider_config_for_mut(ApiProvider::Custom).base_url = Some(value);
            }
        }
    }
    if matches!(config.api_provider(), ApiProvider::NvidiaNim)
        && let Ok(value) = std::env::var("NVIDIA_NIM_BASE_URL")
            .or_else(|_| std::env::var("NIM_BASE_URL"))
            .or_else(|_| std::env::var("NVIDIA_BASE_URL"))
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .nvidia_nim
            .base_url = Some(value);
    }
    // OpenAI-compatible and non-DeepSeek hosted providers are scoped only on
    // their own provider entry — the legacy root `base_url` keeps DeepSeek-only
    // semantics.
    if matches!(config.api_provider(), ApiProvider::Openai)
        && let Ok(value) = std::env::var("OPENAI_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openai
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Atlascloud)
        && let Ok(value) = std::env::var("ATLASCLOUD_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .atlascloud
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Openrouter)
        && let Ok(value) = std::env::var("OPENROUTER_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openrouter
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::XiaomiMimo)
        && let Ok(value) =
            std::env::var("XIAOMI_MIMO_BASE_URL").or_else(|_| std::env::var("MIMO_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xiaomi_mimo
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::XiaomiMimo)
        && let Ok(value) = std::env::var("XIAOMI_MIMO_MODE").or_else(|_| std::env::var("MIMO_MODE"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xiaomi_mimo
            .mode = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::WanjieArk)
        && let Ok(value) = std::env::var("WANJIE_ARK_BASE_URL")
            .or_else(|_| std::env::var("WANJIE_BASE_URL"))
            .or_else(|_| std::env::var("WANJIE_MAAS_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .wanjie_ark
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Volcengine)
        && let Ok(value) = std::env::var("VOLCENGINE_BASE_URL")
            .or_else(|_| std::env::var("VOLCENGINE_ARK_BASE_URL"))
            .or_else(|_| std::env::var("ARK_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .volcengine
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Novita)
        && let Ok(value) = std::env::var("NOVITA_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .novita
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Fireworks)
        && let Ok(value) = std::env::var("FIREWORKS_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .fireworks
            .base_url = Some(value);
    }
    let active_provider = config.api_provider();
    if matches!(
        active_provider,
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn
    ) && let Ok(value) = std::env::var("SILICONFLOW_BASE_URL")
        && !value.trim().is_empty()
    {
        config.provider_config_for_mut(active_provider).base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Arcee)
        && let Ok(value) = std::env::var("ARCEE_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .arcee
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Huggingface)
        && let Ok(value) =
            std::env::var("HUGGINGFACE_BASE_URL").or_else(|_| std::env::var("HF_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .huggingface
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Moonshot)
        && let Ok(value) =
            std::env::var("MOONSHOT_BASE_URL").or_else(|_| std::env::var("KIMI_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .moonshot
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Sglang)
        && let Ok(value) = std::env::var("SGLANG_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .sglang
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Vllm)
        && let Ok(value) = std::env::var("VLLM_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .vllm
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Meta)
        && let Ok(value) = std::env::var("META_MODEL_API_BASE_URL")
            .or_else(|_| std::env::var("MODEL_API_BASE_URL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .meta
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Xai)
        && let Ok(value) = std::env::var("XAI_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xai
            .base_url = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_HTTP_HEADERS")
        && let Ok(headers) = parse_http_headers(&value)
        && !headers.is_empty()
    {
        let mut root_headers = config.http_headers.clone().unwrap_or_default();
        root_headers.extend(headers.clone());
        config.http_headers = Some(root_headers);

        let provider = config.api_provider();
        // 在下面可变借用 `providers` 之前捕获自定义条目键
        //（选定的提供商名称）（#1519）。
        let custom_key = (provider == ApiProvider::Custom).then(|| {
            config
                .provider
                .clone()
                .unwrap_or_else(|| "__custom__".to_string())
        });
        let providers = config
            .providers
            .get_or_insert_with(ProvidersConfig::default);
        let entry = match provider {
            ApiProvider::Deepseek => &mut providers.deepseek,
            ApiProvider::DeepseekCN => &mut providers.deepseek_cn,
            ApiProvider::DeepseekAnthropic => &mut providers.deepseek_anthropic,
            ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
            ApiProvider::Openai => &mut providers.openai,
            ApiProvider::Atlascloud => &mut providers.atlascloud,
            ApiProvider::WanjieArk => &mut providers.wanjie_ark,
            ApiProvider::Openrouter => &mut providers.openrouter,
            ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
            ApiProvider::Novita => &mut providers.novita,
            ApiProvider::Fireworks => &mut providers.fireworks,
            ApiProvider::Siliconflow => &mut providers.siliconflow,
            ApiProvider::SiliconflowCn => &mut providers.siliconflow_cn,
            ApiProvider::Arcee => &mut providers.arcee,
            ApiProvider::Moonshot => &mut providers.moonshot,
            ApiProvider::Sglang => &mut providers.sglang,
            ApiProvider::Vllm => &mut providers.vllm,
            ApiProvider::Ollama => &mut providers.ollama,
            ApiProvider::Volcengine => &mut providers.volcengine,
            ApiProvider::Huggingface => &mut providers.huggingface,
            ApiProvider::Deepinfra => &mut providers.deepinfra,
            ApiProvider::Together => &mut providers.together,
            ApiProvider::Qianfan => &mut providers.qianfan,
            ApiProvider::OpenaiCodex => &mut providers.openai_codex,
            ApiProvider::Anthropic => &mut providers.anthropic,
            ApiProvider::Openmodel => &mut providers.openmodel,
            ApiProvider::Zai => &mut providers.zai,
            ApiProvider::Stepfun => &mut providers.stepfun,
            ApiProvider::Minimax => &mut providers.minimax,
            ApiProvider::Sakana => &mut providers.sakana,
            ApiProvider::LongCat => &mut providers.longcat,
            ApiProvider::Meta => &mut providers.meta,
            ApiProvider::Xai => &mut providers.xai,
            ApiProvider::Custom => providers
                .custom
                .entry(custom_key.expect("custom key captured for custom provider"))
                .or_default(),
        };
        let mut provider_headers = entry.http_headers.clone().unwrap_or_default();
        provider_headers.extend(headers);
        entry.http_headers = Some(provider_headers);
    }
    if matches!(config.api_provider(), ApiProvider::Ollama)
        && let Ok(value) = std::env::var("OLLAMA_BASE_URL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .ollama
            .base_url = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Sglang)
        && let Ok(value) = std::env::var("SGLANG_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Vllm)
        && let Ok(value) = std::env::var("VLLM_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Ollama)
        && let Ok(value) = std::env::var("OLLAMA_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Openai)
        && let Ok(value) = std::env::var("OPENAI_MODEL")
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openai
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::XiaomiMimo)
        && let Ok(value) =
            std::env::var("XIAOMI_MIMO_MODEL").or_else(|_| std::env::var("MIMO_MODEL"))
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xiaomi_mimo
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Atlascloud)
        && let Ok(value) = std::env::var("ATLASCLOUD_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::WanjieArk)
        && let Ok(value) = std::env::var("WANJIE_ARK_MODEL")
            .or_else(|_| std::env::var("WANJIE_MODEL"))
            .or_else(|_| std::env::var("WANJIE_MAAS_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .wanjie_ark
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Openrouter)
        && let Ok(value) = std::env::var("OPENROUTER_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .openrouter
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Volcengine)
        && let Ok(value) =
            std::env::var("VOLCENGINE_MODEL").or_else(|_| std::env::var("VOLCENGINE_ARK_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .volcengine
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Novita)
        && let Ok(value) = std::env::var("NOVITA_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .novita
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Fireworks)
        && let Ok(value) = std::env::var("FIREWORKS_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .fireworks
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Moonshot)
        && let Ok(value) = std::env::var("MOONSHOT_MODEL")
            .or_else(|_| std::env::var("KIMI_MODEL_NAME"))
            .or_else(|_| std::env::var("KIMI_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .moonshot
            .model = Some(value);
    }
    let active_provider = config.api_provider();
    if matches!(
        active_provider,
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn
    ) && let Ok(value) = std::env::var("SILICONFLOW_MODEL")
        && !value.trim().is_empty()
    {
        config.provider_config_for_mut(active_provider).model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Arcee)
        && let Ok(value) = std::env::var("ARCEE_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .arcee
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Huggingface)
        && let Ok(value) = std::env::var("HUGGINGFACE_MODEL").or_else(|_| std::env::var("HF_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .huggingface
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Meta)
        && let Ok(value) =
            std::env::var("META_MODEL_API_MODEL").or_else(|_| std::env::var("MODEL_API_MODEL"))
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .meta
            .model = Some(value);
    }
    if matches!(config.api_provider(), ApiProvider::Xai)
        && let Ok(value) = std::env::var("XAI_MODEL")
        && !value.trim().is_empty()
    {
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .xai
            .model = Some(value);
    }
    if let Some(value) = codewhale_env_var("CODEWHALE_MODEL", "DEEPSEEK_MODEL")
        .ok()
        .or_else(|| {
            std::env::var("DEEPSEEK_DEFAULT_TEXT_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
    {
        // The CLI `--model` handoff always sets DEEPSEEK_MODEL, never the
        // provider-specific *_MODEL var. The legacy root `default_text_model`
        // is a DeepSeek-only slot (the validator rejects non-DeepSeek IDs
        // there). For a non-DeepSeek provider the explicit model must land in
        // the provider-scoped slot instead so the verbatim-passthrough path
        // honors it rather than falling back to a DeepSeek/provider default
        // (issue #1714). Mirror the OPENAI_MODEL branch above for every
        // non-DeepSeek provider.
        let provider = config.api_provider();
        // 在下面的可变借用之前捕获自定义条目键（#1519）。
        let custom_key = (provider == ApiProvider::Custom).then(|| {
            config
                .provider
                .clone()
                .unwrap_or_else(|| "__custom__".to_string())
        });
        if matches!(
            provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
        ) {
            config.default_text_model = Some(value);
        } else {
            let providers = config
                .providers
                .get_or_insert_with(ProvidersConfig::default);
            let entry = match provider {
                ApiProvider::Deepseek
                | ApiProvider::DeepseekCN
                | ApiProvider::DeepseekAnthropic => unreachable!(
                    "DeepSeek providers are handled in the if branch above (issue #1714)"
                ),
                ApiProvider::Custom => providers
                    .custom
                    .entry(custom_key.expect("custom key captured for custom provider"))
                    .or_default(),
                ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
                ApiProvider::Openai => &mut providers.openai,
                ApiProvider::Atlascloud => &mut providers.atlascloud,
                ApiProvider::WanjieArk => &mut providers.wanjie_ark,
                ApiProvider::Openrouter => &mut providers.openrouter,
                ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
                ApiProvider::Novita => &mut providers.novita,
                ApiProvider::Fireworks => &mut providers.fireworks,
                ApiProvider::Siliconflow => &mut providers.siliconflow,
                ApiProvider::SiliconflowCn => &mut providers.siliconflow_cn,
                ApiProvider::Arcee => &mut providers.arcee,
                ApiProvider::Moonshot => &mut providers.moonshot,
                ApiProvider::Sglang => &mut providers.sglang,
                ApiProvider::Vllm => &mut providers.vllm,
                ApiProvider::Ollama => &mut providers.ollama,
                ApiProvider::Volcengine => &mut providers.volcengine,
                ApiProvider::Huggingface => &mut providers.huggingface,
                ApiProvider::Deepinfra => &mut providers.deepinfra,
                ApiProvider::Together => &mut providers.together,
                ApiProvider::Qianfan => &mut providers.qianfan,
                ApiProvider::OpenaiCodex => &mut providers.openai_codex,
                ApiProvider::Anthropic => &mut providers.anthropic,
                ApiProvider::Openmodel => &mut providers.openmodel,
                ApiProvider::Zai => &mut providers.zai,
                ApiProvider::Stepfun => &mut providers.stepfun,
                ApiProvider::Minimax => &mut providers.minimax,
                ApiProvider::Sakana => &mut providers.sakana,
                ApiProvider::LongCat => &mut providers.longcat,
                ApiProvider::Meta => &mut providers.meta,
                ApiProvider::Xai => &mut providers.xai,
            };
            entry.model = Some(value);
        }
    }
    if matches!(config.api_provider(), ApiProvider::NvidiaNim)
        && let Ok(value) = std::env::var("NVIDIA_NIM_MODEL")
    {
        config.default_text_model = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SKILLS_DIR") {
        config.skills_dir = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MCP_CONFIG") {
        config.mcp_config_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_NOTES_PATH") {
        config.notes_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MEMORY_PATH") {
        config.memory_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MEMORY") {
        let on = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes" | "y" | "enabled"
        );
        config
            .memory
            .get_or_insert_with(MemoryConfig::default)
            .enabled = Some(on);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_ALLOW_SHELL") {
        config.allow_shell = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) = std::env::var("DEEPSEEK_APPROVAL_POLICY") {
        config.approval_policy = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_MODE") {
        config.sandbox_mode = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_YOLO") {
        config.yolo = Some(value == "1" || value.eq_ignore_ascii_case("true"));
    }
    if let Ok(value) =
        std::env::var("CODEWHALE_VERBOSITY").or_else(|_| std::env::var("DEEPSEEK_VERBOSITY"))
    {
        config.verbosity = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_BACKEND") {
        config.sandbox_backend = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_URL") {
        config.sandbox_url = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SANDBOX_API_KEY") {
        config.sandbox_api_key = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MANAGED_CONFIG_PATH") {
        config.managed_config_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_SEARCH_API_KEY")
        && !value.trim().is_empty()
    {
        config
            .search
            .get_or_insert_with(SearchConfig::default)
            .api_key = Some(value);
    }
    if let Ok(value) = codewhale_env_var("CODEWHALE_SEARCH_BASE_URL", "DEEPSEEK_SEARCH_BASE_URL") {
        config
            .search
            .get_or_insert_with(SearchConfig::default)
            .base_url = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_REQUIREMENTS_PATH") {
        config.requirements_path = Some(value);
    }
    if let Ok(value) = std::env::var("DEEPSEEK_MAX_SUBAGENTS")
        && let Ok(parsed) = value.parse::<usize>()
    {
        config.max_subagents = Some(parsed.clamp(1, MAX_SUBAGENTS));
    }
}

fn normalize_model_config(config: &mut Config) {
    if let Some(model) = config.default_text_model.as_deref()
        && !provider_passes_model_through(config.api_provider())
        && !config.active_provider_preserves_custom_base_url_model()
        && let Some(normalized) = normalize_model_for_provider(config.api_provider(), model)
    {
        config.default_text_model = Some(normalized);
    }

    if let Some(providers) = config.providers.as_mut() {
        if let Some(model) = providers.deepseek.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Deepseek, &providers.deepseek)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Deepseek, model)
        {
            providers.deepseek.model = Some(normalized);
        }
        if let Some(model) = providers.deepseek_cn.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::DeepseekCN, &providers.deepseek_cn)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::DeepseekCN, model)
        {
            providers.deepseek_cn.model = Some(normalized);
        }
        if let Some(model) = providers.nvidia_nim.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::NvidiaNim, &providers.nvidia_nim)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::NvidiaNim, model)
        {
            providers.nvidia_nim.model = Some(normalized);
        }
        if let Some(model) = providers.openrouter.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Openrouter, &providers.openrouter)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Openrouter, model)
        {
            providers.openrouter.model = Some(normalized);
        }
        if let Some(model) = providers.novita.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Novita, &providers.novita)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Novita, model)
        {
            providers.novita.model = Some(normalized);
        }
        if let Some(model) = providers.fireworks.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Fireworks, &providers.fireworks)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Fireworks, model)
        {
            providers.fireworks.model = Some(normalized);
        }
        if let Some(model) = providers.siliconflow.model.as_deref()
            && !provider_entry_uses_custom_base_url(
                ApiProvider::Siliconflow,
                &providers.siliconflow,
            )
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Siliconflow, model)
        {
            providers.siliconflow.model = Some(normalized);
        }
        if let Some(model) = providers.siliconflow_cn.model.as_deref()
            && !provider_entry_uses_custom_base_url(
                ApiProvider::SiliconflowCn,
                &providers.siliconflow_cn,
            )
            && let Some(normalized) =
                normalize_model_for_provider(ApiProvider::SiliconflowCn, model)
        {
            providers.siliconflow_cn.model = Some(normalized);
        }
        if let Some(model) = providers.moonshot.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Moonshot, &providers.moonshot)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Moonshot, model)
        {
            providers.moonshot.model = Some(normalized);
        }
        if let Some(model) = providers.sglang.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Sglang, &providers.sglang)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Sglang, model)
        {
            providers.sglang.model = Some(normalized);
        }
        if let Some(model) = providers.vllm.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Vllm, &providers.vllm)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Vllm, model)
        {
            providers.vllm.model = Some(normalized);
        }
        if let Some(model) = providers.deepinfra.model.as_deref()
            && !provider_entry_uses_custom_base_url(ApiProvider::Deepinfra, &providers.deepinfra)
            && let Some(normalized) = normalize_model_for_provider(ApiProvider::Deepinfra, model)
        {
            providers.deepinfra.model = Some(normalized);
        }
    }
}

fn normalize_model_for_provider(provider: ApiProvider, model: &str) -> Option<String> {
    if matches!(provider, ApiProvider::XiaomiMimo)
        && let Some(canonical) = canonical_xiaomi_mimo_model_id(model)
    {
        return Some(canonical.to_string());
    }
    if provider_passes_model_through(provider) {
        return None;
    }
    normalize_model_name_for_provider(provider, model)
}

pub(crate) fn provider_passes_model_through(provider: ApiProvider) -> bool {
    matches!(
        provider,
        ApiProvider::Openai
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Volcengine
            | ApiProvider::XiaomiMimo
            | ApiProvider::Moonshot
            | ApiProvider::Qianfan
            | ApiProvider::Openmodel
            | ApiProvider::Ollama
            | ApiProvider::Huggingface
            | ApiProvider::Meta
            | ApiProvider::Xai
            // Custom OpenAI-compatible endpoints preserve user-supplied model
            // ids verbatim (#1519); never normalize/rewrite them.
            | ApiProvider::Custom
    )
}

fn provider_entry_uses_custom_base_url(provider: ApiProvider, entry: &ProviderConfig) -> bool {
    entry
        .base_url
        .as_deref()
        .is_some_and(|base_url| provider_preserves_custom_base_url_model(provider, base_url))
}

fn default_base_url_for_provider(provider: ApiProvider) -> &'static str {
    provider.default_base_url()
}

fn xiaomi_mimo_base_url_for_mode(mode: &str) -> Option<&'static str> {
    let normalized = mode.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    if normalized.is_empty() || xiaomi_mimo_mode_uses_standard_endpoint(&normalized) {
        return None;
    }
    Some(match normalized.as_str() {
        "token-plan" | "tokenplan" | "subscription" | "subscribed" | "plan" => {
            DEFAULT_XIAOMI_MIMO_BASE_URL
        }
        "token-plan-cn"
        | "token-plan-china"
        | "token-plan-mainland"
        | "token-plan-mainland-china"
        | "cn"
        | "china" => XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL,
        "token-plan-sgp"
        | "token-plan-sg"
        | "token-plan-singapore"
        | "sgp"
        | "sg"
        | "singapore" => XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL,
        "token-plan-ams"
        | "token-plan-eu"
        | "token-plan-europe"
        | "token-plan-amsterdam"
        | "ams"
        | "eu"
        | "europe"
        | "amsterdam" => XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL,
        _ => DEFAULT_XIAOMI_MIMO_BASE_URL,
    })
}

fn xiaomi_mimo_mode_uses_standard_endpoint(normalized_mode: &str) -> bool {
    matches!(
        normalized_mode,
        "standard" | "default" | "payg" | "paygo" | "pay-as-you-go" | "pay-as-go"
    )
}

fn xiaomi_mimo_base_url_uses_token_plan(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url).to_ascii_lowercase();
    normalized == XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL
        || normalized == XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL
        || normalized == XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL
}

fn xiaomi_mimo_env_var(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn xiaomi_mimo_env_api_key_for_runtime(
    mode: Option<&str>,
    base_url: Option<&str>,
) -> Option<String> {
    const TOKEN_PLAN_ENV_VARS: &[&str] =
        &["XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "MIMO_TOKEN_PLAN_API_KEY"];
    const STANDARD_ENV_VARS: &[&str] = &["XIAOMI_MIMO_API_KEY", "XIAOMI_API_KEY", "MIMO_API_KEY"];

    let normalized_mode =
        mode.map(|value| value.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let standard_selected = normalized_mode
        .as_deref()
        .is_some_and(xiaomi_mimo_mode_uses_standard_endpoint)
        || base_url.is_some_and(xiaomi_mimo_base_url_is_pay_as_you_go);
    if standard_selected {
        return xiaomi_mimo_env_var(STANDARD_ENV_VARS);
    }

    let token_plan_selected = normalized_mode
        .as_deref()
        .and_then(xiaomi_mimo_base_url_for_mode)
        .is_some()
        || base_url.is_some_and(xiaomi_mimo_base_url_uses_token_plan);
    if token_plan_selected {
        return xiaomi_mimo_env_var(TOKEN_PLAN_ENV_VARS);
    }

    xiaomi_mimo_env_var(TOKEN_PLAN_ENV_VARS).or_else(|| xiaomi_mimo_env_var(STANDARD_ENV_VARS))
}

fn resolve_xiaomi_mimo_base_url(
    configured: Option<String>,
    api_key: Option<&str>,
    mode: Option<&str>,
) -> String {
    let normalized_mode =
        mode.map(|value| value.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let uses_standard_mode = normalized_mode
        .as_deref()
        .is_some_and(xiaomi_mimo_mode_uses_standard_endpoint);
    let mode_base_url = normalized_mode
        .as_deref()
        .and_then(xiaomi_mimo_base_url_for_mode);
    let uses_token_plan = xiaomi_mimo_api_key_uses_token_plan(api_key);
    match configured {
        Some(base_url) if uses_standard_mode => base_url,
        Some(base_url) if uses_token_plan && xiaomi_mimo_base_url_is_pay_as_you_go(&base_url) => {
            mode_base_url
                .unwrap_or(DEFAULT_XIAOMI_MIMO_BASE_URL)
                .to_string()
        }
        Some(base_url) => base_url,
        None => {
            if let Some(base_url) = mode_base_url {
                base_url.to_string()
            } else if uses_standard_mode {
                XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string()
            } else if uses_token_plan || api_key.is_none() {
                DEFAULT_XIAOMI_MIMO_BASE_URL.to_string()
            } else {
                XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL.to_string()
            }
        }
    }
}

fn xiaomi_mimo_api_key_uses_token_plan(api_key: Option<&str>) -> bool {
    api_key.is_some_and(|key| key.trim_start().starts_with("tp-"))
}

fn xiaomi_mimo_base_url_is_pay_as_you_go(base_url: &str) -> bool {
    matches!(
        normalize_base_url(base_url).to_ascii_lowercase().as_str(),
        "https://api.xiaomimimo.com" | "https://api.xiaomimimo.com/v1"
    )
}

fn base_url_is_custom_for_provider(provider: ApiProvider, base_url: &str) -> bool {
    if (provider == ApiProvider::Siliconflow || provider == ApiProvider::SiliconflowCn)
        && siliconflow_base_url_is_official(base_url)
    {
        return false;
    }
    if provider == ApiProvider::XiaomiMimo
        && (xiaomi_mimo_base_url_uses_token_plan(base_url)
            || xiaomi_mimo_base_url_is_pay_as_you_go(base_url))
    {
        return false;
    }
    normalize_base_url(base_url) != normalize_base_url(default_base_url_for_provider(provider))
}

fn provider_preserves_custom_base_url_model(provider: ApiProvider, base_url: &str) -> bool {
    base_url_is_custom_for_provider(provider, base_url)
}

fn siliconflow_base_url_is_official(base_url: &str) -> bool {
    matches!(
        normalize_base_url(base_url).to_ascii_lowercase().as_str(),
        "https://api.siliconflow.com/v1" | "https://api.siliconflow.cn/v1"
    )
}

fn moonshot_base_url_uses_kimi_code(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url).to_ascii_lowercase();
    normalized == DEFAULT_KIMI_CODE_BASE_URL
        || normalized == "https://api.kimi.com/coding"
        || normalized.starts_with("https://api.kimi.com/coding/")
}

fn provider_config_uses_kimi_oauth(config: &ProviderConfig) -> bool {
    config
        .auth_mode
        .as_deref()
        .is_some_and(auth_mode_uses_kimi_oauth)
}

fn auth_mode_uses_kimi_oauth(mode: &str) -> bool {
    matches!(
        normalize_auth_mode(mode).as_str(),
        "kimi" | "kimi_oauth" | "kimi_cli" | "oauth"
    )
}

fn provider_config_uses_xai_oauth(config: &ProviderConfig) -> bool {
    config
        .auth_mode
        .as_deref()
        .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth)
}

fn normalize_auth_mode(mode: &str) -> String {
    mode.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

/// Whether a base URL points at a loopback/unspecified host, i.e. a local
/// runtime rather than a hosted endpoint. Shared by the active-provider
/// local-base-url check above and the `/provider` picker's custom-provider
/// auth-optionality heuristic (#3830).
pub(crate) fn base_url_uses_local_host(base_url: &str) -> bool {
    let Some(host) = base_url_host(base_url) else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "0.0.0.0") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|addr| addr.is_loopback() || addr.is_unspecified())
}

fn base_url_host(base_url: &str) -> Option<&str> {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = without_scheme.split('/').next()?.rsplit('@').next()?;
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    authority.split(':').next().filter(|host| !host.is_empty())
}

fn model_for_provider(provider: ApiProvider, normalized: String) -> String {
    let lowered = normalized.to_ascii_lowercase();
    match (provider, lowered.as_str()) {
        (ApiProvider::NvidiaNim, "deepseek-v4-pro") => DEFAULT_NVIDIA_NIM_MODEL.to_string(),
        (ApiProvider::NvidiaNim, "deepseek-v4-flash") => DEFAULT_NVIDIA_NIM_FLASH_MODEL.to_string(),
        (ApiProvider::Openrouter, "deepseek-v4-pro") => DEFAULT_OPENROUTER_MODEL.to_string(),
        (ApiProvider::Openrouter, "deepseek-v4-flash") => {
            DEFAULT_OPENROUTER_FLASH_MODEL.to_string()
        }
        (ApiProvider::Novita, "deepseek-v4-pro") => DEFAULT_NOVITA_MODEL.to_string(),
        (ApiProvider::Novita, "deepseek-v4-flash") => DEFAULT_NOVITA_FLASH_MODEL.to_string(),
        (ApiProvider::Fireworks, "deepseek-v4-pro") => DEFAULT_FIREWORKS_MODEL.to_string(),
        (
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn,
            "deepseek-v4-pro" | "deepseek-reasoner" | "deepseek-r1",
        ) => DEFAULT_SILICONFLOW_MODEL.to_string(),
        (
            ApiProvider::Siliconflow | ApiProvider::SiliconflowCn,
            "deepseek-v4-flash" | "deepseek-chat" | "deepseek-v3",
        ) => DEFAULT_SILICONFLOW_FLASH_MODEL.to_string(),
        (ApiProvider::Sglang, "deepseek-v4-pro") => DEFAULT_SGLANG_MODEL.to_string(),
        (ApiProvider::Sglang, "deepseek-v4-flash") => DEFAULT_SGLANG_FLASH_MODEL.to_string(),
        (ApiProvider::Vllm, "deepseek-v4-pro") => DEFAULT_VLLM_MODEL.to_string(),
        (ApiProvider::Vllm, "deepseek-v4-flash") => DEFAULT_VLLM_FLASH_MODEL.to_string(),
        (ApiProvider::Deepinfra, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_DEEPINFRA_MODEL.to_string()
        }
        (ApiProvider::Deepinfra, "deepseek-v4-flash" | "deepseek-chat" | "deepseek-reasoner") => {
            DEFAULT_DEEPINFRA_FLASH_MODEL.to_string()
        }
        (ApiProvider::Together, "deepseek-v4-pro" | "deepseek-v4pro") => {
            DEFAULT_TOGETHER_MODEL.to_string()
        }
        (
            ApiProvider::Together,
            "deepseek-v4-flash" | "deepseek-v4flash" | "deepseek-chat" | "deepseek-reasoner",
        ) => DEFAULT_TOGETHER_FLASH_MODEL.to_string(),
        (
            ApiProvider::Moonshot,
            "kimi"
            | "kimi-k2"
            | "kimi-k2.7"
            | "kimi-k2-7"
            | "kimi-k2.7-code"
            | "kimi-k2-7-code"
            | "kimi-code"
            | "moonshot-kimi-k2.7-code",
        ) => DEFAULT_MOONSHOT_MODEL.to_string(),
        (ApiProvider::Moonshot, "kimi-k2.6" | "kimi-k2-6" | "moonshot-kimi-k2.6") => {
            MOONSHOT_KIMI_K2_6_MODEL.to_string()
        }
        _ => normalized,
    }
}

fn normalize_base_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    let deepseek_domains = ["api.deepseek.com", "api.deepseeki.com"];
    if deepseek_domains
        .iter()
        .any(|domain| trimmed.contains(domain))
    {
        return trimmed.trim_end_matches("/v1").to_string();
    }
    trimmed.to_string()
}

fn parse_http_headers(raw: &str) -> Result<HashMap<String, String>> {
    let mut headers = HashMap::new();
    for pair in raw.trim().split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((name, value)) = pair.split_once('=') else {
            anyhow::bail!("invalid header pair '{pair}', expected name=value");
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            anyhow::bail!("header name cannot be empty");
        }
        if value.is_empty() {
            continue;
        }
        headers.insert(name.to_string(), value.to_string());
    }
    Ok(headers)
}

fn apply_profile(config: ConfigFile, profile: Option<&str>) -> Result<Config> {
    if let Some(profile_name) = profile {
        let profiles = config.profiles.as_ref();
        match profiles.and_then(|profiles| profiles.get(profile_name)) {
            Some(override_cfg) => Ok(merge_config(config.base, override_cfg.clone())),
            None => {
                let available = profiles
                    .map(|profiles| {
                        let mut keys = profiles.keys().cloned().collect::<Vec<_>>();
                        keys.sort();
                        if keys.is_empty() {
                            "none".to_string()
                        } else {
                            keys.join(", ")
                        }
                    })
                    .unwrap_or_else(|| "none".to_string());
                anyhow::bail!("Profile '{profile_name}' not found. Available profiles: {available}")
            }
        }
    } else {
        Ok(config.base)
    }
}

fn merge_config(base: Config, override_cfg: Config) -> Config {
    Config {
        provider: override_cfg.provider.or(base.provider),
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        default_text_model: override_cfg.default_text_model.or(base.default_text_model),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        reasoning_effort: override_cfg.reasoning_effort.or(base.reasoning_effort),
        tools: override_cfg.tools.or(base.tools),
        skills_dir: override_cfg.skills_dir.or(base.skills_dir),
        mcp_config_path: override_cfg.mcp_config_path.or(base.mcp_config_path),
        mcp_oauth_callback_port: override_cfg
            .mcp_oauth_callback_port
            .or(base.mcp_oauth_callback_port),
        mcp_oauth_callback_url: override_cfg
            .mcp_oauth_callback_url
            .or(base.mcp_oauth_callback_url),
        notes_path: override_cfg.notes_path.or(base.notes_path),
        memory_path: override_cfg.memory_path.or(base.memory_path),
        vision_model: override_cfg.vision_model.or(base.vision_model),
        // #454: user-owned overlays such as profiles and managed config may
        // replace the instruction array. Project-scope config is filtered in
        // main.rs and cannot set instruction paths.
        instructions: override_cfg.instructions.or(base.instructions),
        allow_shell: override_cfg.allow_shell.or(base.allow_shell),
        prompt_suggestion: override_cfg.prompt_suggestion.or(base.prompt_suggestion),
        yolo: override_cfg.yolo.or(base.yolo),
        verbosity: override_cfg.verbosity.or(base.verbosity),
        approval_policy: override_cfg.approval_policy.or(base.approval_policy),
        sandbox_mode: override_cfg.sandbox_mode.or(base.sandbox_mode),
        fallback_providers: if override_cfg.fallback_providers.is_empty() {
            base.fallback_providers
        } else {
            override_cfg.fallback_providers
        },
        sandbox_backend: override_cfg.sandbox_backend.or(base.sandbox_backend),
        sandbox_url: override_cfg.sandbox_url.or(base.sandbox_url),
        sandbox_api_key: override_cfg.sandbox_api_key.or(base.sandbox_api_key),
        prefer_bwrap: override_cfg.prefer_bwrap.or(base.prefer_bwrap),
        managed_config_path: override_cfg
            .managed_config_path
            .or(base.managed_config_path),
        requirements_path: override_cfg.requirements_path.or(base.requirements_path),
        max_subagents: override_cfg.max_subagents.or(base.max_subagents),
        retry: override_cfg.retry.or(base.retry),
        auto_review: override_cfg.auto_review.or(base.auto_review),
        tui: override_cfg.tui.or(base.tui),
        hooks: override_cfg.hooks.or(base.hooks),
        providers: merge_providers(base.providers, override_cfg.providers),
        features: merge_features(base.features, override_cfg.features),
        notifications: override_cfg.notifications.or(base.notifications),
        network: override_cfg.network.or(base.network),
        verifier: override_cfg.verifier.or(base.verifier),
        skills: merge_skills_config(base.skills, override_cfg.skills),
        snapshots: override_cfg.snapshots.or(base.snapshots),
        search: override_cfg.search.or(base.search),
        memory: override_cfg.memory.or(base.memory),
        speech: override_cfg.speech.or(base.speech),
        auto: override_cfg.auto.or(base.auto),
        hotbar: override_cfg.hotbar.or(base.hotbar),
        update: override_cfg.update.or(base.update),
        lsp: override_cfg.lsp.or(base.lsp),
        context: ContextConfig {
            enabled: override_cfg.context.enabled.or(base.context.enabled),
            project_pack: override_cfg
                .context
                .project_pack
                .or(base.context.project_pack),
            verbatim_window_turns: override_cfg
                .context
                .verbatim_window_turns
                .or(base.context.verbatim_window_turns),
            l1_threshold: override_cfg
                .context
                .l1_threshold
                .or(base.context.l1_threshold),
            l2_threshold: override_cfg
                .context
                .l2_threshold
                .or(base.context.l2_threshold),
            l3_threshold: override_cfg
                .context
                .l3_threshold
                .or(base.context.l3_threshold),
            seam_model: override_cfg.context.seam_model.or(base.context.seam_model),
        },
        fleet: override_cfg.fleet.or(base.fleet),
        workflow: override_cfg.workflow.or(base.workflow),
        subagents: override_cfg.subagents.or(base.subagents),
        strict_tool_mode: override_cfg.strict_tool_mode.or(base.strict_tool_mode),
        runtime_api: override_cfg.runtime_api.or(base.runtime_api),
        workshop: override_cfg.workshop.or(base.workshop),
        exec_policy_engine: override_cfg.exec_policy_engine,
    }
}

fn load_sibling_exec_policy_engine(config_path: Option<&Path>) -> Result<ExecPolicyEngine> {
    let Some(config_path) = config_path else {
        return Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()));
    };
    let permissions_path = codewhale_config::permissions_path_for_config_path(config_path);
    if !permissions_path.exists() {
        return Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()));
    }

    let raw = fs::read_to_string(&permissions_path).with_context(|| {
        format!(
            "Failed to read permissions file: {}",
            permissions_path.display()
        )
    })?;
    let permissions: codewhale_config::PermissionsToml =
        toml::from_str(&raw).with_context(|| {
            format!(
                "Failed to parse permissions file: {}",
                permissions_path.display()
            )
        })?;
    if permissions.is_empty() {
        Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()))
    } else {
        Ok(ExecPolicyEngine::with_rulesets(vec![permissions.ruleset()]))
    }
}

fn merge_skills_config(
    base: Option<SkillsConfig>,
    override_cfg: Option<SkillsConfig>,
) -> Option<SkillsConfig> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
        (Some(base), Some(override_cfg)) => Some(SkillsConfig {
            registry_url: override_cfg.registry_url.or(base.registry_url),
            max_install_size_bytes: override_cfg
                .max_install_size_bytes
                .or(base.max_install_size_bytes),
            scan_codewhale_only: override_cfg
                .scan_codewhale_only
                .or(base.scan_codewhale_only),
        }),
    }
}

fn merge_provider_config(base: ProviderConfig, override_cfg: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        model: override_cfg.model.or(base.model),
        context_window: override_cfg.context_window.or(base.context_window),
        mode: override_cfg.mode.or(base.mode),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        insecure_skip_tls_verify: override_cfg
            .insecure_skip_tls_verify
            .or(base.insecure_skip_tls_verify),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        path_suffix: override_cfg.path_suffix.or(base.path_suffix),
        reasoning_stream_style: override_cfg
            .reasoning_stream_style
            .or(base.reasoning_stream_style),
        max_concurrency: override_cfg.max_concurrency.or(base.max_concurrency),
        auth: override_cfg.auth.or(base.auth),
        kind: override_cfg.kind.or(base.kind),
        api_key_env: override_cfg.api_key_env.or(base.api_key_env),
    }
}

/// Merge the per-name custom provider maps (#1519): the union of both key sets,
/// with each shared key deep-merged via [`merge_provider_config`] (override
/// wins field-by-field). Keys present in only one map are carried through as-is.
fn merge_custom_providers(
    mut base: HashMap<String, ProviderConfig>,
    override_cfg: HashMap<String, ProviderConfig>,
) -> HashMap<String, ProviderConfig> {
    for (name, entry) in override_cfg {
        let merged = match base.remove(&name) {
            Some(base_entry) => merge_provider_config(base_entry, entry),
            None => entry,
        };
        base.insert(name, merged);
    }
    base
}

fn merge_providers(
    base: Option<ProvidersConfig>,
    override_cfg: Option<ProvidersConfig>,
) -> Option<ProvidersConfig> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
        (Some(base), Some(override_cfg)) => Some(ProvidersConfig {
            deepseek: merge_provider_config(base.deepseek, override_cfg.deepseek),
            deepseek_cn: merge_provider_config(base.deepseek_cn, override_cfg.deepseek_cn),
            deepseek_anthropic: merge_provider_config(
                base.deepseek_anthropic,
                override_cfg.deepseek_anthropic,
            ),
            nvidia_nim: merge_provider_config(base.nvidia_nim, override_cfg.nvidia_nim),
            openai: merge_provider_config(base.openai, override_cfg.openai),
            anthropic: merge_provider_config(base.anthropic, override_cfg.anthropic),
            openmodel: merge_provider_config(base.openmodel, override_cfg.openmodel),
            atlascloud: merge_provider_config(base.atlascloud, override_cfg.atlascloud),
            wanjie_ark: merge_provider_config(base.wanjie_ark, override_cfg.wanjie_ark),
            openrouter: merge_provider_config(base.openrouter, override_cfg.openrouter),
            xiaomi_mimo: merge_provider_config(base.xiaomi_mimo, override_cfg.xiaomi_mimo),
            novita: merge_provider_config(base.novita, override_cfg.novita),
            fireworks: merge_provider_config(base.fireworks, override_cfg.fireworks),
            siliconflow: merge_provider_config(base.siliconflow, override_cfg.siliconflow),
            siliconflow_cn: merge_provider_config(base.siliconflow_cn, override_cfg.siliconflow_cn),
            arcee: merge_provider_config(base.arcee, override_cfg.arcee),
            moonshot: merge_provider_config(base.moonshot, override_cfg.moonshot),
            sglang: merge_provider_config(base.sglang, override_cfg.sglang),
            vllm: merge_provider_config(base.vllm, override_cfg.vllm),
            ollama: merge_provider_config(base.ollama, override_cfg.ollama),
            volcengine: merge_provider_config(base.volcengine, override_cfg.volcengine),
            huggingface: merge_provider_config(base.huggingface, override_cfg.huggingface),
            deepinfra: merge_provider_config(base.deepinfra, override_cfg.deepinfra),
            together: merge_provider_config(base.together, override_cfg.together),
            qianfan: merge_provider_config(base.qianfan, override_cfg.qianfan),
            openai_codex: merge_provider_config(base.openai_codex, override_cfg.openai_codex),
            zai: merge_provider_config(base.zai, override_cfg.zai),
            stepfun: merge_provider_config(base.stepfun, override_cfg.stepfun),
            minimax: merge_provider_config(base.minimax, override_cfg.minimax),
            sakana: merge_provider_config(base.sakana, override_cfg.sakana),
            longcat: merge_provider_config(base.longcat, override_cfg.longcat),
            meta: merge_provider_config(base.meta, override_cfg.meta),
            xai: merge_provider_config(base.xai, override_cfg.xai),
            custom: merge_custom_providers(base.custom, override_cfg.custom),
        }),
    }
}

fn load_single_config_file(path: &Path) -> Result<Config> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let parsed: ConfigFile = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
    Ok(parsed.base)
}

/// Build a one-line warning when top-level-only keys are nested under a section
/// CodeWhale does not define (`[general]` / `[sandbox]`). TOML silently drops
/// those keys, so e.g. `[general]\nallow_shell = true` never takes effect and
/// the shell tools (`exec_shell`, `task_shell_start`, …) are absent from the
/// catalog with no explanation. Returns `None` when nothing is misplaced.
///
/// This is the exact confusion behind #2589: `allow_shell` and `sandbox_mode`
/// belong at the top of the file, above any `[section]` header.
fn warn_on_misplaced_top_level_keys(raw: &str) -> Option<String> {
    let doc = toml::from_str::<toml::Value>(raw).ok()?;
    // Sections CodeWhale does not recognize but users nest settings under.
    const UNKNOWN_SECTIONS: &[&str] = &["general", "sandbox"];
    // Keys that are only ever read from the top level of the config.
    const TOP_LEVEL_KEYS: &[&str] = &[
        "allow_shell",
        "sandbox_mode",
        "approval_policy",
        "verbosity",
    ];

    let mut hits: Vec<String> = Vec::new();
    for section in UNKNOWN_SECTIONS {
        let Some(table) = doc.get(*section).and_then(toml::Value::as_table) else {
            continue;
        };
        for key in TOP_LEVEL_KEYS {
            if table.contains_key(*key) {
                hits.push(format!("`{section}.{key}`"));
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "Ignoring {} — CodeWhale has no `[general]` or `[sandbox]` section, so these \
         keys are silently dropped. Move them to the TOP of the config file (above any \
         `[section]` header), e.g. `allow_shell = true`. Until then, shell tools stay \
         disabled. (#2589)",
        hits.join(", ")
    ))
}

fn apply_managed_overrides(config: &mut Config) -> Result<()> {
    let path = config
        .managed_config_path
        .as_deref()
        .map(expand_path)
        .or_else(default_managed_config_path);
    let Some(path) = path else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let managed = load_single_config_file(&path)?;
    *config = merge_config(config.clone(), managed);
    Ok(())
}

fn apply_requirements(config: &mut Config) -> Result<()> {
    let path = config
        .requirements_path
        .as_deref()
        .map(expand_path)
        .or_else(default_requirements_path);
    let Some(path) = path else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read requirements file: {}", path.display()))?;
    let requirements: RequirementsFile = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse requirements file: {}", path.display()))?;

    if !requirements.allowed_approval_policies.is_empty()
        && let Some(policy) = config.approval_policy.as_ref()
    {
        let policy = policy.to_ascii_lowercase();
        if !requirements
            .allowed_approval_policies
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&policy))
        {
            anyhow::bail!(
                "approval_policy '{policy}' is not allowed by requirements ({})",
                requirements.allowed_approval_policies.join(", ")
            );
        }
    }
    if !requirements.allowed_sandbox_modes.is_empty()
        && let Some(mode) = config.sandbox_mode.as_ref()
    {
        let mode = mode.to_ascii_lowercase();
        if !requirements
            .allowed_sandbox_modes
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&mode))
        {
            anyhow::bail!(
                "sandbox_mode '{mode}' is not allowed by requirements ({})",
                requirements.allowed_sandbox_modes.join(", ")
            );
        }
    }

    Ok(())
}

fn merge_features(
    base: Option<FeaturesToml>,
    override_cfg: Option<FeaturesToml>,
) -> Option<FeaturesToml> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(mut base), Some(override_cfg)) => {
            for (key, value) in override_cfg.entries {
                base.entries.insert(key, value);
            }
            Some(base)
        }
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
    }
}

pub fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        #[cfg(unix)]
        {
            // Tighten group/other bits on the parent dir as a hardening pass.
            // The dir lives under the user's home, so the chmod is best-effort:
            // filesystems that don't accept Unix permission bits (Docker
            // bind-mounts of NTFS, network shares, FAT, certain CI volumes —
            // see #897) return EPERM/ENOTSUP. The dir already exists by the
            // time we get here, so failing the whole save just because we
            // couldn't tighten perms strands the user mid-onboarding. Warn
            // loudly so a security-sensitive operator can still notice via
            // `RUST_LOG=warn`, then continue.
            if let Ok(meta) = fs::metadata(parent) {
                let mode = meta.permissions().mode();
                if mode & 0o077 != 0 {
                    let mut perms = meta.permissions();
                    perms.set_mode(mode & !0o077);
                    if let Err(err) = fs::set_permissions(parent, perms) {
                        tracing::warn!(
                            target: "codewhale::config",
                            path = %parent.display(),
                            error = %err,
                            "could not tighten parent dir permissions; \
                             filesystem may not support Unix chmod \
                             (Docker bind-mount, NTFS, network share). \
                             Continuing — the file will still be written."
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// 以限制性权限（仅所有者读/写）将内容写入配置文件。
/// 在 Unix 上，写入前设置模式 0o600。
fn write_config_file_secure(path: &Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        // The file was already opened with mode 0o600; the explicit
        // set_permissions re-asserts that on filesystems where mode-at-open
        // didn't take effect (or where the file already existed with broader
        // bits). Filesystems that don't accept Unix chmod at all (Docker
        // bind-mounts of NTFS, network shares — #897) return EPERM. Treat
        // that as a warning rather than failing the whole save: the file
        // contents are written, and on Windows/macOS hosts the parent file
        // system's native ACL model is doing the access control.
        if let Err(err) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                target: "codewhale::config",
                path = %path.display(),
                error = %err,
                "could not enforce 0o600 on config file; filesystem may \
                 not support Unix chmod. File contents written; rely on \
                 host ACLs for access control."
            );
        }
    }
    #[cfg(not(unix))]
    {
        fs::write(path, content)?;
    }
    Ok(())
}

/// 保存的凭据的去向。由 [`save_api_key`] 返回，
/// 以便调用者可以显示确认消息而不泄露密钥。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedCredential {
    /// 存储在**OS 密钥环**和 codewhale 配置文件中。
    /// 这是具有可用密钥环后端的平台上的默认结果：
    /// 写入两层都打破了 `keyring → env → config-file` 的解析顺序阴影，
    /// 否则来自先前安装的过时 OS 密钥环条目会隐藏新输入的密钥（#593）。
    /// `backend` 标签是写入时的 [`codewhale_secrets::Secrets::backend_name`] 值，
    /// 以便提示文本可以命名实际的后端
    ///（`"system keyring"`、`"file-based (~/.codewhale/secrets/)"`）。
    KeyringAndConfigFile {
        /// 写入时的 `Secrets::backend_name()`。
        backend: String,
        /// 也被更新的配置文件的绝对路径。
        path: PathBuf,
    },
    /// 仅存储在 codewhale 配置文件中。当没有密钥环后端可用时，
    /// 或在 `cfg(test)` 下以便单元测试不污染主机密钥环时的回退。
    ConfigFile(PathBuf),
}

impl SavedCredential {
    /// 状态/日志输出的人类可读描述。决不包含密钥值。
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::KeyringAndConfigFile { backend, path } => {
                format!("OS keyring ({backend}) and {}", path.display())
            }
            Self::ConfigFile(path) => path.display().to_string(),
        }
    }
}

/// 保存活跃提供商的 API 密钥。
///
/// **双写策略（#593）：** 写入 `~/.codewhale/config.toml`（总是）
/// 并通过 [`codewhale_secrets::Secrets`] 写入 OS 密钥环（当后端可用时）。
/// 运行时按照 `keyring → env → config-file` 的顺序解析凭据；
/// 仅写入配置文件（如 v0.8.8 到 v0.8.10 所做的）会让先前安装中的
/// 过时密钥环条目静默遮盖用户在 TUI 入职期间刚刚输入的新值，
/// 产生 #593 中报告的"无响应"症状。
///
/// 配置文件仍然是可检查的持久记录（在 npm 安装、IDE 终端和无头机器上
/// 都能工作），而密钥环作为分层覆盖，在解析路径上击败过时阴影。
/// 当密钥环写入失败时（无后端、OS 权限拒绝等），配置文件写入仍然有效，
/// 函数报告 [`SavedCredential::ConfigFile`] 结果——调用者不应将其视为失败。
///
/// 在 `cfg(test)` 下跳过，以便测试套件从不接触主机密钥环。
/// `secrets` crate 有自己的密钥环设置/获取的测试覆盖。
pub fn save_api_key(api_key: &str) -> Result<SavedCredential> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Refusing to save an empty API key.");
    }

    // Always write the inspectable copy first. The config file is the
    // durable record everyone — including macOS Keychain-prompted
    // first-run, headless CI, and IDE terminals — can rely on.
    let path = save_api_key_to_config_file(trimmed)?;

    // Then mirror to the OS keyring when one is reachable. This
    // overwrites any stale entry from a prior install so
    // `Secrets::resolve` (keyring → env → config-file) no longer
    // shadows the fresh key. Skipped under `cfg(test)` so unit tests
    // can't pollute the host keyring (macOS Always-Allow prompts,
    // cross-test contamination).
    #[cfg(not(test))]
    {
        let secrets = codewhale_secrets::Secrets::auto_detect();
        match secrets.set("deepseek", trimmed) {
            Ok(()) => {
                let backend = secrets.backend_name().to_string();
                log_sensitive_event(
                    "credential.save",
                    json!({
                        "backend": backend.clone(),
                        "config_path": path.display().to_string(),
                        "dual_write": true,
                    }),
                );
                return Ok(SavedCredential::KeyringAndConfigFile { backend, path });
            }
            Err(err) => {
                tracing::warn!("OS keyring write failed; key saved to config.toml only: {err}");
                // Fall through to the ConfigFile-only outcome below.
            }
        }
    }

    Ok(SavedCredential::ConfigFile(path))
}

/// Write the `api_key` slot directly to `config.toml`.
fn save_api_key_to_config_file(api_key: &str) -> Result<PathBuf> {
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;

    ensure_parent_dir(&config_path)?;

    if config_path.exists() {
        // TOML-aware upsert. The old line scan keyed off
        // `existing.contains("api_key")`, so a comment that merely mentioned
        // api_key made it skip the insert entirely; editing the document
        // replaces or inserts the real key and keeps user comments.
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::set_document_value(doc, &["api_key"], api_key)
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    } else {
        // Create new minimal config
        let content = format!(
            r#"# codewhale Configuration
# Set provider credentials in this file or via environment variables.
# See /links in the TUI for provider-specific credential pages.

api_key = "{api_key}"

# Base URL (default: https://api.deepseek.com/beta)
# Set https://api.deepseek.com to opt out of beta features.
# base_url = "https://api.deepseek.com/beta"

# Default model
default_text_model = "{DEFAULT_TEXT_MODEL}"

# Thinking mode (DeepSeek V4 reasoning effort):
# "off" | "low" | "medium" | "high" | "max"
# Shift+Tab in the TUI cycles between off / high / max.
reasoning_effort = "max"
"#
        );
        crate::config_persistence::write_config_toml_atomic(&config_path, &content)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    }

    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "config_path": config_path.display().to_string(),
        }),
    );

    Ok(config_path)
}

/// Check if the active provider has any API key configured anywhere the
/// runtime can resolve it.
///
/// Platform credential stores are intentionally not queried here.
/// Startup/onboarding checks must be cheap and prompt-free, so v0.8.8
/// keeps the default auth path to environment variables and
/// `~/.codewhale/config.toml`.
///
/// Used by [`crate::tui::app::App::new`] to decide whether to gate
/// the user behind the in-TUI api-key onboarding screen — getting
/// this wrong made users get prompted for credentials in situations
/// where normal env/config auth was already available.
pub fn has_api_key(config: &Config) -> bool {
    has_api_key_for(config, config.api_provider())
}

#[must_use]
pub fn active_provider_has_config_api_key(config: &Config) -> bool {
    let provider = config.api_provider();

    if provider == ApiProvider::Moonshot
        && config
            .provider_config_for(provider)
            .is_some_and(provider_config_uses_kimi_oauth)
    {
        return kimi_cli_credentials_present();
    }
    if provider == ApiProvider::OpenaiCodex {
        // The persistent Codex login is the OAuth credential file, analogous to
        // a stored config key. Token env overrides are scored separately by
        // active_provider_has_env_api_key.
        return crate::oauth::auth_file_path().exists();
    }
    if matches!(provider, ApiProvider::Huggingface)
        && std::env::var("HUGGINGFACE_API_KEY")
            .or_else(|_| std::env::var("HF_TOKEN"))
            .is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }

    if config
        .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
        .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }
    if config
        .provider_config_for(provider)
        .and_then(|entry| entry.auth.as_ref())
        .is_some_and(|auth| auth.validate().is_ok())
    {
        return true;
    }

    matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
}

#[must_use]
pub fn active_provider_has_env_api_key(config: &Config) -> bool {
    provider_env_api_key(config.api_provider()).is_some()
}

#[must_use]
pub fn active_provider_uses_env_only_api_key(config: &Config) -> bool {
    active_provider_has_env_api_key(config) && !active_provider_has_config_api_key(config)
}

/// Check whether the given provider has any usable API key — via env var,
/// provider/root config. Used by the `/provider` picker to decide whether to
/// prompt for a key inline.
#[must_use]
pub fn has_api_key_for(config: &Config, provider: ApiProvider) -> bool {
    if provider
        .env_vars()
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|k| !k.trim().is_empty()))
    {
        return true;
    }

    if provider == ApiProvider::Moonshot
        && config
            .provider_config_for(provider)
            .is_some_and(provider_config_uses_kimi_oauth)
    {
        return kimi_cli_credentials_present();
    }
    if provider == ApiProvider::OpenaiCodex {
        // Token env overrides are checked above; also honor the Codex CLI OAuth
        // login on disk.
        return crate::oauth::auth_file_path().exists();
    }
    if provider == ApiProvider::Xai && crate::xai_oauth::credentials_present() {
        // xAI supports both API keys and OAuth. A Grok-compatible token file is
        // sufficient, but its absence must fall through to the ordinary API-key
        // checks below instead of masking a configured key.
        return true;
    }

    // Self-hosted providers typically run without authentication.
    if provider.is_self_hosted() {
        return true;
    }

    if provider == config.api_provider() && base_url_uses_local_host(&config.deepseek_base_url()) {
        return true;
    }

    if config
        .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
        .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }
    if config
        .provider_config_for(provider)
        .and_then(|entry| entry.auth.as_ref())
        .is_some_and(|auth| auth.validate().is_ok())
    {
        return true;
    }

    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty() && k != API_KEYRING_SENTINEL)
    {
        return true;
    }

    false
}

/// Whether a provider counts as "configured" for the default `/provider`
/// and `/model` manager views (#3830). Shared by both pickers so "what shows
/// up without browsing the full catalog" stays a single definition.
/// Self-hosted providers (Ollama/Sglang/Vllm) report `has_key = true`
/// unconditionally in [`has_api_key_for`] since they don't require auth to
/// route to — that's correct for routing, but wrong for "did the user set
/// this up," so a self-hosted provider only qualifies via an explicit
/// `[providers.<name>]` entry or being active, never via `has_key` alone
/// (otherwise every self-hosted provider type would always show up).
#[must_use]
pub(crate) fn provider_is_configured(
    provider: ApiProvider,
    is_active: bool,
    has_key: bool,
    configured: Option<&ProviderConfig>,
    is_named_custom_entry: bool,
) -> bool {
    // A *named* custom provider entry (one the user actually added) always
    // counts. The unconfigured `Custom` placeholder row that fills the slot
    // when no custom provider exists yet is not itself "configured" — it's
    // the catalog's invitation to add one.
    if is_active || is_named_custom_entry {
        return true;
    }
    if configured.is_some_and(provider_config_is_explicit) {
        return true;
    }
    if provider.is_self_hosted() {
        return false;
    }
    has_key
}

/// Convenience wrapper around [`provider_is_configured`] for callers that
/// just want "is this provider configured given the active one," without
/// the provider picker's multi-row named-custom-provider bookkeeping
/// (`is_named_custom_entry`) — e.g. the `/model` picker (#3830), which only
/// ever resolves the single, currently-selected `Custom` slot via
/// [`Config::provider_config_for`], the same way model/route resolution
/// does everywhere else.
#[must_use]
pub(crate) fn provider_is_configured_for_active(
    config: &Config,
    provider: ApiProvider,
    active: ApiProvider,
) -> bool {
    provider_is_configured(
        provider,
        provider == active,
        has_api_key_for(config, provider),
        config.provider_config_for(provider),
        false,
    )
}

/// True when a `[providers.<name>]` table entry has any field the user would
/// have had to set explicitly — base URL, model, auth, etc. Used by
/// [`provider_is_configured`]: merely existing in the
/// (always-`Some`-once-any-provider-is-configured) `ProvidersConfig` struct
/// isn't enough, since untouched providers still resolve to a
/// `ProviderConfig::default()` there.
fn provider_config_is_explicit(entry: &ProviderConfig) -> bool {
    entry.api_key.is_some()
        || entry.base_url.is_some()
        || entry.model.is_some()
        || entry.auth_mode.is_some()
        || entry.auth.is_some()
        || entry.context_window.is_some()
        || entry.mode.is_some()
        || entry.max_concurrency.is_some()
        || entry.http_headers.is_some()
        || entry.path_suffix.is_some()
        || entry.reasoning_stream_style.is_some()
        || entry.insecure_skip_tls_verify.is_some()
}

/// Save an API key to the appropriate place for the given provider.
/// DeepSeek goes through [`save_api_key`]. Other providers write
/// `[providers.<name>] api_key = "..."` to `~/.codewhale/config.toml`.
/// Returns the config file path.
pub fn save_api_key_for(provider: ApiProvider, api_key: &str) -> Result<PathBuf> {
    if provider == ApiProvider::OpenaiCodex {
        anyhow::bail!(
            "OpenAI Codex uses OAuth. Run `codex login` or set OPENAI_CODEX_ACCESS_TOKEN; CodeWhale does not store an API key for this provider."
        );
    }
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        return match save_api_key(api_key)? {
            SavedCredential::KeyringAndConfigFile { path, .. }
            | SavedCredential::ConfigFile(path) => Ok(path),
        };
    }

    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;
    ensure_parent_dir(&config_path)?;

    let key_inside = provider_config_key(provider).context("provider api key table")?;
    // Edit the `[providers.<name>]` table in place so unrelated sections,
    // comments, and formatting survive the write.
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "api_key"],
            api_key,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "provider": provider.as_str(),
            "config_path": config_path.display().to_string(),
        }),
    );

    Ok(config_path)
}

/// Persist a default model for `provider` via the comment-preserving config
/// path used by guided provider setup (#3875). DeepSeek writes root
/// `default_text_model`; other hosted providers write `[providers.<name>] model`.
pub fn save_provider_model_for(provider: ApiProvider, model: &str) -> Result<PathBuf> {
    let model = model.trim();
    anyhow::ensure!(!model.is_empty(), "model cannot be empty");

    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;
    ensure_parent_dir(&config_path)?;

    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::set_document_value(doc, &["default_text_model"], model)
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        return Ok(config_path);
    }

    let key_inside = provider_config_key(provider).context("provider model table")?;
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "model"],
            model,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

pub fn save_provider_auth_mode_for_at(
    provider: ApiProvider,
    auth_mode: &str,
    config_path: Option<&Path>,
) -> Result<PathBuf> {
    let config_path = match config_path {
        Some(path) => path.to_path_buf(),
        None => default_config_path()
            .context("Failed to resolve config path: home directory not found.")?,
    };
    ensure_parent_dir(&config_path)?;
    let key_inside = provider_config_key(provider).context("provider auth mode key")?;
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "auth_mode"],
            auth_mode,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.auth_mode.set",
        json!({
            "backend": "config_file",
            "provider": provider.as_str(),
            "auth_mode": auth_mode,
            "config_path": config_path.display().to_string(),
        }),
    );
    Ok(config_path)
}

fn provider_config_key(provider: ApiProvider) -> Result<&'static str> {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        anyhow::bail!("DeepSeek stores auth at the root config level");
    }
    provider
        .metadata()
        .map(|metadata| metadata.provider_config_key())
        .context("provider config key")
}

fn provider_config_table_name(provider: ApiProvider) -> Result<String> {
    Ok(format!("providers.{}", provider_config_key(provider)?))
}

fn provider_env_api_key(provider: ApiProvider) -> Option<String> {
    if provider == ApiProvider::Huggingface {
        return std::env::var("HUGGINGFACE_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("HF_TOKEN")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            });
    }

    provider.env_vars().iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn missing_provider_api_key_message(provider: ApiProvider) -> Result<String> {
    let credential_hint = provider
        .credential_url()
        .map(|url| format!(" Get a key: {url}."))
        .unwrap_or_default();
    Ok(format!(
        "{} API key not found.{} Run 'codewhale auth set --provider {}', set {}, or add [{}] api_key in ~/.codewhale/config.toml.",
        provider.display_name(),
        credential_hint,
        provider.as_str(),
        provider.env_vars_label(),
        provider_config_table_name(provider)?
    ))
}

const KIMI_CODE_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const KIMI_CODE_CREDENTIAL_FILE: &str = "kimi-code.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct KimiOAuthCredential {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<f64>,
    expires_in: Option<f64>,
    scope: Option<String>,
    token_type: Option<String>,
}

fn kimi_cli_oauth_access_token() -> Result<String> {
    let path = kimi_cli_oauth_credentials_path()?;
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "Kimi OAuth credentials not found at {}. Run `kimi login`, then set \
             [providers.moonshot] auth_mode = \"kimi_oauth\".",
            path.display()
        )
    })?;
    let mut credential: KimiOAuthCredential =
        serde_json::from_str(&raw).context("Failed to parse Kimi OAuth credentials")?;

    if kimi_oauth_access_token_is_fresh(&credential) {
        return credential
            .access_token
            .filter(|token| !token.trim().is_empty())
            .context("Kimi OAuth access token is empty");
    }

    let refresh_token = credential
        .refresh_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .context("Kimi OAuth refresh token is empty. Run `kimi login` again.")?;
    credential = refresh_kimi_oauth_token(refresh_token)?;
    write_kimi_oauth_credential(&path, &credential)?;
    credential
        .access_token
        .filter(|token| !token.trim().is_empty())
        .context("Kimi OAuth refresh returned an empty access token")
}

fn kimi_oauth_access_token_is_fresh(credential: &KimiOAuthCredential) -> bool {
    let Some(now) = now_unix_secs() else {
        return false;
    };

    credential
        .access_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
        && credential
            .expires_at
            .is_some_and(|expires_at| expires_at - now > 60.0)
}

fn refresh_kimi_oauth_token(refresh_token: &str) -> Result<KimiOAuthCredential> {
    let oauth_host = std::env::var("KIMI_CODE_OAUTH_HOST")
        .or_else(|_| std::env::var("KIMI_OAUTH_HOST"))
        .unwrap_or_else(|_| "https://auth.kimi.com".to_string());
    let url = format!("{}/api/oauth/token", oauth_host.trim_end_matches('/'));
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("Failed to build Kimi OAuth refresh client")?;
    let params = [
        ("client_id", KIMI_CODE_CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    let response = client
        .post(url)
        .header("X-Msh-Platform", "kimi_cli")
        .header("X-Msh-Version", env!("CARGO_PKG_VERSION"))
        .form(&params)
        .send()
        .context("Kimi OAuth refresh request failed")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Kimi OAuth refresh failed with HTTP {status}. Run `kimi login` again.");
    }

    let mut refreshed: KimiOAuthCredential = response
        .json()
        .context("Failed to parse Kimi OAuth refresh response")?;
    if let Some(expires_in) = refreshed.expires_in
        && let Some(now) = now_unix_secs()
    {
        refreshed.expires_at = Some(now + expires_in);
    }
    Ok(refreshed)
}

fn kimi_cli_oauth_credentials_path() -> Result<PathBuf> {
    if let Some(kimi_code_home) = kimi_code_home_override() {
        return Ok(kimi_oauth_credential_path(kimi_code_home));
    }

    let modern_path = effective_home_dir()
        .map(|home| kimi_oauth_credential_path(home.join(".kimi-code")))
        .context("Failed to resolve Kimi Code home directory")?;
    if modern_path.exists() {
        return Ok(modern_path);
    }

    if let Some(legacy_share_dir) = kimi_legacy_share_dir_override() {
        return Ok(kimi_oauth_credential_path(legacy_share_dir));
    }

    if let Some(legacy_path) = effective_home_dir()
        .map(|home| kimi_oauth_credential_path(home.join(".kimi")))
        .filter(|path| path.exists())
    {
        return Ok(legacy_path);
    }

    Ok(modern_path)
}

fn kimi_code_home_override() -> Option<PathBuf> {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn kimi_legacy_share_dir_override() -> Option<PathBuf> {
    std::env::var_os("KIMI_SHARE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn kimi_oauth_credential_path(home: PathBuf) -> PathBuf {
    home.join("credentials").join(KIMI_CODE_CREDENTIAL_FILE)
}

fn write_kimi_oauth_credential(path: &Path, credential: &KimiOAuthCredential) -> Result<()> {
    let serialized = serde_json::to_vec_pretty(credential)
        .context("Failed to serialize Kimi OAuth credentials")?;
    crate::utils::write_atomic(path, &serialized).with_context(|| {
        format!(
            "Failed to write Kimi OAuth credentials to {}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            target: "codewhale::config",
            path = %path.display(),
            error = %err,
            "could not enforce 0o600 on Kimi OAuth credentials; relying on host ACLs"
        );
    }
    Ok(())
}

fn now_unix_secs() -> Option<f64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .ok()
}

#[must_use]
pub fn kimi_cli_credentials_present() -> bool {
    kimi_cli_oauth_credentials_path().is_ok_and(|path| path.exists())
}

/// Clear the API key from config-file storage.
///
/// `/logout` calls this to wipe credentials so the next request can't
/// silently use a stale config key (#343). The function removes the legacy
/// root `api_key` entry *and* every `api_key` entry nested in a
/// `[providers.<name>]` table, leaving keys like `api_key_env`, comments,
/// and formatting untouched.
///
/// Environment variables (`DEEPSEEK_API_KEY`, etc.) are intentionally
/// **not** unset — they are managed by the user's shell and outside the
/// CLI's purview. `Config::deepseek_api_key`'s explicit-override path
/// (Path 0) ensures a freshly-entered key still wins over a stale env
/// var that lingers from a previous session.
pub fn clear_api_key() -> Result<()> {
    // Strip api_key entries from config.toml, including provider-scoped
    // nested entries. Clearing a config file must not trigger platform
    // credential prompts.
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;

    if !config_path.exists() {
        return Ok(());
    }

    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::remove_document_key_recursive(doc.as_table_mut(), "api_key");
        Ok(())
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.clear",
        json!({
            "backend": "config_file",
            "config_path": config_path.display().to_string(),
            "scope": "root_and_provider_keys",
        }),
    );

    Ok(())
}

/// Clear only the active provider's API key from the config file.
/// Unlike `clear_api_key()` which strips ALL api_key entries, this
/// removes only the key for the specified provider section (plus the
/// legacy root `api_key` when the provider is DeepSeek).
pub fn clear_active_provider_api_key(provider: &str) -> Result<()> {
    let config_path = default_config_path()
        .context("Failed to resolve config path: home directory not found.")?;

    if !config_path.exists() {
        return Ok(());
    }

    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        // The root-level api_key is the legacy DeepSeek slot.
        if provider == "deepseek" {
            crate::config_persistence::unset_document_value(doc, &["api_key"])?;
        }
        crate::config_persistence::unset_document_value(doc, &["providers", provider, "api_key"])?;
        Ok(())
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.clear",
        json!({
            "backend": "config_file",
            "config_path": config_path.display().to_string(),
            "scope": provider,
        }),
    );

    Ok(())
}

#[cfg(test)]
mod tests;
