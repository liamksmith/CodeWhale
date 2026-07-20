use codewhale_config::route::{
    LogicalModelRef, ReadyRouteCandidate, RouteLimits, RouteRequest, RouteResolver, WireModelId,
};

use crate::config::{ApiProvider, Config, DEFAULT_NVIDIA_NIM_BASE_URL};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRuntimeRoute {
    pub(crate) candidate: ReadyRouteCandidate,   // 路由解析的结果候选
    pub(crate) config: Config,   // 配置（可能是修改过的副本）
    pub(crate) model: String,    // 选定的模型名（字符串）
}

/// 这个函数解析路由候选。参数：
/// ## parameter
/// - `provider` 目标API供应商
/// - `model_selector` 用户指定的模型选择器（可选），比如"--model deepseek-v4-pro"
/// - `saved_provider_model` 配置文件中保存的供应商模型（可选）
/// - `base_url_override` 覆盖的base URL
/// - `context_window_override` 覆盖的上下文窗口大小
/// 返回 Result<ReadyRouteCandidate, String>——成功时返回路由候选，失败时返回错误字符串。
pub(crate) fn resolve_route_candidate(
    provider: ApiProvider,
    model_selector: Option<&str>,
    saved_provider_model: Option<&str>,
    base_url_override: Option<String>,
    context_window_override: Option<u32>,
) -> Result<ReadyRouteCandidate, String> {
    let route_request = RouteRequest {
        explicit_provider: provider.kind(),  //  获取供应商的kind（如 "deepseek", "openai" 等字符串标识）
        model_selector: model_selector.map(|model| LogicalModelRef::from(model.to_string())),
        saved_provider_model: saved_provider_model
            .map(|model| WireModelId::from(model.to_string())),
        base_url_override,
    };
    let mut candidate = RouteResolver::new()
        .resolve(&route_request)
        .map_err(|err| err.to_string())?;
    apply_context_window_override(&mut candidate.limits, context_window_override);
    Ok(candidate)
}

fn apply_context_window_override(limits: &mut RouteLimits, context_window: Option<u32>) {
    if let Some(context_window) = context_window.filter(|window| *window > 0) {
        limits.context_tokens = Some(u64::from(context_window));
    }
}

/// 这是主要的入口函数，解析运行时路由。参数：
/// ## parameters
/// - `config` 配置引用
/// - `provider` API供应商
/// - `model_selector` 可选的模型选择器
pub(crate) fn resolve_runtime_route(
    config: &Config,
    provider: ApiProvider,
    model_selector: Option<&str>,
) -> Result<ResolvedRuntimeRoute, String> {
    //  准备路由配置（克隆并调整后的配置副本）
    let mut route_config = prepared_route_config(config, provider, model_selector);
    // 从配置中读取该供应商的已保存模型名
    let saved_provider_model = route_config
        .provider_config_for(provider)
        .and_then(|provider| provider.model.as_deref());
    // 执行解析，传入供应商、模型选择器、保存的模型、base URL和上下文窗口
    let candidate = resolve_route_candidate(
        provider,
        model_selector,
        saved_provider_model,
        Some(route_config.deepseek_base_url()),
        route_config.context_window_for_provider_config(provider),
    )?;
    // 从候选结果中提取模型ID字符串
    let model = candidate.wire_model_id.as_str().to_string();
    // 将选定的模型写回配置副本
    route_config.provider_config_for_mut(provider).model = Some(model.clone());
    // 返回 ResolvedRuntimeRoute
    Ok(ResolvedRuntimeRoute {
        candidate,
        config: route_config,
        model,
    })
}

// 准备路由配置。
fn prepared_route_config(
    config: &Config,
    provider: ApiProvider,
    model_selector: Option<&str>,
) -> Config {
    // 克隆配置
    let mut route_config = config.clone();
    // 为什么 Custom 供应商要做特殊处理：对于内置供应商（如 Deepseek、OpenAI），需要把 provider 
    // 字段设为标准名称；但 Custom 供应商的用户自定义名称本身就是查找键，覆盖成 "custom" 字面量会破坏路由。
    // 对于内置供应商（非Custom），设置provider字段为供应商名称。但Custom类型保持不变，因为其名称本身就是查找键。
    if provider != ApiProvider::Custom {
        route_config.provider = Some(provider.as_str().to_string());
    }
    // NvidiaNim供应商特殊处理——如果base_url不包含"integrate.api.nvidia.com"，则设置为默认NIM URL
    if matches!(provider, ApiProvider::NvidiaNim)
        && route_config
            .base_url
            .as_deref()
            .map(|base| !base.contains("integrate.api.nvidia.com"))
            .unwrap_or(true)
    {
        route_config.base_url = Some(DEFAULT_NVIDIA_NIM_BASE_URL.to_string());
    }
    // Deepseek/DeepseekCN供应商特殊处理——如果base_url属于非Deepseek供应商，则清空base_url
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && route_config
            .base_url
            .as_deref()
            .map(root_base_url_belongs_to_non_deepseek_provider)
            .unwrap_or(false)
    {
        route_config.base_url = None;
    }
    // 如果有model_selector，将其写入配置中的对应供应商model字段
    if let Some(model) = model_selector {
        route_config.provider_config_for_mut(provider).model = Some(model.to_string());
    }
    route_config
}

// 检查给定的base_url是否属于非Deepseek供应商。将URL转为小写，然后检查是否包含已知的
// 非Deepseek供应商域名。如果匹配任一，返回true。
fn root_base_url_belongs_to_non_deepseek_provider(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    [
        "integrate.api.nvidia.com",
        "api.openai.com",
        "api.atlascloud.ai",
        "maas-openapi.wanjiedata.com",
        "volces.com",
        "openrouter.ai",
        "xiaomimimo.com",
        "novita.ai",
        "fireworks.ai",
        "siliconflow",
        "arcee.ai",
        "moonshot.ai",
        "api.kimi.com",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TEXT_MODEL, DEFAULT_ZAI_MODEL, ProviderConfig, ProvidersConfig};

    /// 当没有指定 model_selector 时，使用目标供应商的默认模型。
    #[test]
    fn runtime_route_without_model_uses_target_provider_default() {
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    model: Some("deepseek/deepseek-v4-pro".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let route = resolve_runtime_route(&config, ApiProvider::Zai, None)
            .expect("target provider default should resolve");

        assert_eq!(route.model, DEFAULT_ZAI_MODEL);
        assert_eq!(route.config.provider.as_deref(), Some("zai"));
        assert_eq!(
            route
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.zai.model.as_deref()),
            Some(DEFAULT_ZAI_MODEL)
        );
        assert_eq!(
            route
                .config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.model.as_deref()),
            Some("deepseek/deepseek-v4-pro")
        );
    }

    // 验证：当配置的供应商是 deepseek，但用户试图用 --model deepseek-v4-pro 直接指定一个非 
    // Zai 供应商的模型时，应该被拒绝。
    #[test]
    fn runtime_route_rejects_foreign_direct_model_before_config_snapshot() {
        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(ProvidersConfig {
                deepseek: ProviderConfig {
                    model: Some(DEFAULT_TEXT_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let err = resolve_runtime_route(&config, ApiProvider::Zai, Some("deepseek-v4-pro"))
            .expect_err("foreign direct-provider model should reject");

        assert!(err.contains("not served by direct provider zai"));
        assert_eq!(config.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.zai.model.as_deref()),
            None
        );
    }

    fn custom_config(base_url: &str, model: &str) -> Config {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key_env: Some("EXAMPLE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        Config {
            provider: Some("my_thing".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn custom_provider_resolves_to_custom_endpoint_and_verbatim_model() {
        use codewhale_config::route::RequestProtocol;

        let config = custom_config("https://api.example.com/v1", "vendor/custom-model-v1");
        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom provider should resolve");

        // Endpoint + model come from the named table; the prefixed model id is
        // preserved verbatim as the wire id (no provider-prefix sniffing).
        assert_eq!(
            route.candidate.endpoint.base_url,
            "https://api.example.com/v1"
        );
        assert_eq!(
            route.candidate.wire_model_id.as_str(),
            "vendor/custom-model-v1"
        );
        assert_eq!(route.model, "vendor/custom-model-v1");
        assert_eq!(route.candidate.protocol, RequestProtocol::ChatCompletions);
        // HTTPS endpoint: route is valid with no insecure-http advisory.
        assert!(route.candidate.validation.ok);
        assert!(route.candidate.validation.messages.is_empty());
        // The selected provider name is preserved (not overwritten with "custom").
        assert_eq!(route.config.provider.as_deref(), Some("my_thing"));
    }

    #[test]
    fn custom_provider_context_window_overrides_unknown_route_limit() {
        let mut custom = std::collections::HashMap::new();
        custom.insert(
            "dashscope".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://dashscope.example.com/compatible-mode/v1".to_string()),
                model: Some("qwen3.7".to_string()),
                context_window: Some(1_000_000),
                api_key_env: Some("DASHSCOPE_API_KEY".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("dashscope".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom route should resolve");

        assert_eq!(route.model, "qwen3.7");
        assert_eq!(route.candidate.limits.context_tokens, Some(1_000_000));
    }

    #[test]
    fn custom_provider_http_non_loopback_fires_insecure_advisory() {
        let config = custom_config("http://gpu.internal.example:8000/v1", "custom-model-v1");
        let route = resolve_runtime_route(&config, ApiProvider::Custom, None)
            .expect("custom http provider should resolve");

        // Advisory only: the route still validates (ok == true) but warns that
        // credentials would be sent in plaintext over a non-loopback http URL.
        assert!(route.candidate.validation.ok);
        assert!(
            route
                .candidate
                .validation
                .messages
                .iter()
                .any(|message| message.contains("insecure http")),
            "expected insecure-http advisory, got {:?}",
            route.candidate.validation.messages
        );
        assert_eq!(
            route.candidate.endpoint.base_url,
            "http://gpu.internal.example:8000/v1"
        );
    }
}
