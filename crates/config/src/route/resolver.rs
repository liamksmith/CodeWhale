//! [`ReadyRouteCandidate`] 的唯一生产者 (#3384)。
//!
//! [`RouteResolver::resolve`] 是 [`ReadyRouteCandidate::new`] 的唯一调用方。
//! 它将 [`RouteRequest`] 解析为可执行的路由，使用以下方法：
//!
//! 1. 仅来自 `explicit_provider` 的提供商（无 base-URL / 前缀嗅探）；
//!    当未指定时，使用工作区默认提供商范围。提供商
//!    绝不会从模型前缀推断。
//! 2. 模型选择器，在该提供商范围内严格解释，
//!    匹配解析器提供的 offerings 加上提供商默认值。默认
//!    解析器使用 [`bundled_offerings`]，而测试或快照加载器可以
//!    注入 Models.dev 派生的行。带前缀的选择器被原样保留
//!    作为 [`WireModelId`]。
//! 3. `auto` => [`LogicalModelRef::is_auto`] 哨兵，绝不是字面量的
//!    模型。
//!
//! 它编码了自己的最小 direct/aggregator/local 分类，
//! 因为 tui 辅助函数（`provider_passes_model_through` /
//! `accepts_custom_model_ids`）从 `crates/config` 不可达。这里的
//! 分类故意比 tui 的 `validate_route` 更窄：
//! 它仅针对给定明确外来选择器的一小组严格直接提供商，
//! 拒绝 [`RouteError::ForeignModelForDirectProvider`]；
//! 聚合器、本地和自定义端点通过 `Ok`（`validation.ok == true`）。
//!
//! [`RouteRequest`] 上故意没有提示文本/自由格式字段，
//! 这在结构上阻止了提示内容路由。

use super::candidate::{
    PricingSku, ReadyRouteCandidate, ResolvedAuthSource, ResolvedEndpoint, ValidationReport,
};
use super::descriptor::ProviderDescriptor;
use super::errors::RouteError;
use super::ids::{LogicalModelRef, ModelId, ProviderId, WireModelId};
use super::offering::{ProviderModelOffering, RouteLimits, bundled_offerings};
use crate::ProviderKind;
use crate::catalog::{CatalogOffering, bundled_catalog_offerings};

/// 解析为可执行路由的请求。
///
/// 注意没有任何提示文本/自由格式字段：解析器看不到
/// 提示内容，因此它不能基于提示内容静默路由。
#[derive(Debug, Clone, Default)]
pub struct RouteRequest {
    /// 明确的提供商选择。提供商身份的唯一来源。
    pub explicit_provider: Option<ProviderKind>,
    /// 调用方选择的模型（可能是 `auto` 或带前缀的）。
    pub model_selector: Option<LogicalModelRef>,
    /// 之前保存的提供商 wire 模型 ID，用作范围回退。
    pub saved_provider_model: Option<WireModelId>,
    /// 端点的显式基础 URL 覆盖。
    pub base_url_override: Option<String>,
}

/// 将 [`RouteRequest`] 解析为 [`ReadyRouteCandidate`]。
#[derive(Debug, Clone)]
pub struct RouteResolver {
    offerings: Vec<ProviderModelOffering>,
}

impl Default for RouteResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteResolver {
    /// 使用 CodeWhale 打包的离线 offerings 构造解析器。
    ///
    /// 默认 offerings 是已提交的 Models.dev 形状的目录资产
    /// （`crate::catalog::bundled_catalog_offerings`，真实的上下文窗口和
    /// 诚实的每行 `cost`），与小型手工接缝（[`bundled_offerings`]）合并。
    /// 手工接缝被保留并在 `(provider, wire id)` 冲突时具有优先级：
    /// 它编码了路由不变量所依赖的策划的规范模型连接
    ///（例如 DeepSeek 原生行和将带前缀的 wire id 映射回
    /// `deepseek-v4-pro` 的聚合器行），这些生成的 Models.dev JSON 无法证明。
    /// 仅资产行（GLM、Kimi、MiniMax、Qwen 等）添加了选择器和候选者
    /// 之前缺少的真实提供商/模型事实。
    #[must_use]
    pub fn new() -> Self {
        Self::from_offerings(default_offerings())
    }

    /// 从提供商范围的 offering 目录构造解析器。
    ///
    /// 这是 Models.dev 快照的桥梁：调用方解析目录，
    /// 发出提供商 offerings，然后将这些行交给解析器，
    /// 而不改变路由解析语义。
    #[must_use]
    pub fn from_offerings(offerings: Vec<ProviderModelOffering>) -> Self {
        Self { offerings }
    }

    /// 将请求解析为可执行的路由候选。
    ///
    /// # 错误
    /// 当模型为空、提供商无效或请求了严格直接提供商的外来模型时，
    /// 返回 [`RouteError`]。
    pub fn resolve(&self, req: &RouteRequest) -> Result<ReadyRouteCandidate, RouteError> {
        // 1. 提供商范围仅来自显式选择；否则使用默认值。
        //    提供商绝不会从模型前缀推断。
        let provider_kind = req.explicit_provider.unwrap_or_default();
        let descriptor = ProviderDescriptor::for_kind(provider_kind);
        let provider_id = descriptor.id();
        let default_offering = self.default_offering(&provider_id);

        // 2. 从显式选择确定逻辑选择器，然后是
        //    已保存模型的回退，然后是提供商默认值。
        let logical_model = match &req.model_selector {
            Some(selector) => selector.clone(),
            None => {
                // 无选择器：回退到已保存的 wire 模型，然后是提供商
                // 默认值。两者都保持在已解析提供商的范围内。
                let raw = req
                    .saved_provider_model
                    .as_ref()
                    .map(|w| w.as_str().to_string())
                    .unwrap_or_else(|| {
                        default_offering.map_or_else(
                            || descriptor.default_wire_model().as_str().to_string(),
                            |offering| offering.wire_model_id.as_str().to_string(),
                        )
                    });
                LogicalModelRef::from(raw)
            }
        };

        // 拒绝来自任何来源（显式、已保存或退化默认值）的空选择器，
        // 而不仅仅是空的显式选择器。
        if logical_model.raw().is_empty() {
            return Err(RouteError::EmptyModel);
        }

        // 3. `auto` 是主动选择的哨兵：解析为提供商默认的 wire id，
        //    而不将 "auto" 视为字面量模型名称。
        let is_auto = logical_model.is_auto();

        // 4. 将选择器映射到提供商范围内的 wire id。
        //    带前缀的选择器被原封不动地保留为 wire id。
        let class = if request_uses_custom_endpoint(&descriptor, req.base_url_override.as_deref()) {
            ProviderClass::LocalOrCustom
        } else {
            classify(provider_kind)
        };
        let (wire_model_id, canonical_model, endpoint_key, limits, pricing) = if is_auto {
            default_offering.map_or_else(
                || {
                    (
                        descriptor.default_wire_model(),
                        None,
                        "chat".to_string(),
                        RouteLimits::default(),
                        // 默认分支手上没有 offering：定价是
                        // 诚实地未知的 (#3085)，绝不是编造的零。
                        PricingSku::UnknownOrStale,
                    )
                },
                |offering| {
                    (
                        offering.wire_model_id.clone(),
                        offering.canonical_model.clone(),
                        offering.endpoint_key.clone(),
                        offering.limits,
                        // 匹配的 offering：携带其来源的定价计量器。
                        offering.pricing.clone(),
                    )
                },
            )
        } else {
            self.scope_selector(provider_kind, &provider_id, &logical_model, class)?
        };

        let endpoint = ResolvedEndpoint {
            base_url: req
                .base_url_override
                .clone()
                .unwrap_or_else(|| descriptor.default_base_url().to_string()),
            endpoint_key,
            protocol: descriptor.protocol(),
        };

        // 建议性验证 (#1519)：非回环的 `http://` 端点会以明文发送
        // 凭据。这是建议性的，不是硬性失败，因此
        // `ok` 保持 true，本地 `http://localhost` 运行时（Ollama / vLLM /
        // SGLang 默认值）保持干净。
        let mut messages = Vec::new();
        if endpoint_uses_insecure_http(&endpoint.base_url) {
            messages
                .push("endpoint uses insecure http:// (credentials sent in plaintext)".to_string());
        }
        let validation = ValidationReport { ok: true, messages };

        Ok(ReadyRouteCandidate::new(
            provider_id,
            provider_kind,
            logical_model,
            canonical_model,
            wire_model_id,
            endpoint,
            ResolvedAuthSource::Missing,
            descriptor.protocol(),
            limits,
            // #3085：从匹配的 offering 投影的诚实定价（
            // 目录层将来源成本映射到 SKU）；当没有匹配到 offering
            // 或 offering 没有价格时，为 `UnknownOrStale`。
            Some(pricing),
            validation,
        ))
    }

    /// 严格在提供商范围内解释具体（非 auto）选择器。
    fn scope_selector(
        &self,
        provider_kind: ProviderKind,
        provider_id: &ProviderId,
        logical_model: &LogicalModelRef,
        class: ProviderClass,
    ) -> Result<
        (
            WireModelId,
            Option<ModelId>,
            String,
            RouteLimits,
            PricingSku,
        ),
        RouteError,
    > {
        let raw = logical_model.raw();

        // 尝试匹配由此提供商拥有的目录 offering，通过
        // 规范模型 ID 或精确的 wire ID。这将解释保持在
        // 提供商范围内；来自其他提供商的 offerings 被忽略。
        for offering in &self.offerings {
            if offering.provider != *provider_id {
                continue;
            }
            let matches_canonical = offering
                .canonical_model
                .as_ref()
                .is_some_and(|m| m.as_str() == raw);
            let matches_wire = offering.wire_model_id.as_str() == raw;
            if matches_canonical || matches_wire {
                return Ok((
                    offering.wire_model_id.clone(),
                    offering.canonical_model.clone(),
                    offering.endpoint_key.clone(),
                    offering.limits,
                    // 匹配的 offering：携带其来源的定价计量器 (#3085)。
                    offering.pricing.clone(),
                ));
            }
        }

        // 无目录匹配。应用类特定的透传规则。
        match class {
            ProviderClass::StrictDirect => {
                if self.selector_matches_other_provider_offering(provider_id, raw) {
                    return Err(RouteError::ForeignModelForDirectProvider {
                        provider: provider_id.clone(),
                        model: raw.to_string(),
                    });
                }
                // 严格直接提供商的外来选择器被拒绝。
                // "外来" = 它带有聚合器/组织命名空间前缀，
                // 直接提供商从不期望这种前缀。
                if logical_model.namespace_hint().is_some() {
                    return Err(RouteError::ForeignModelForDirectProvider {
                        provider: provider_id.clone(),
                        model: raw.to_string(),
                    });
                }
                // 严格直接提供商上的裸未知模型被原样透传
                //（提供商在服务端验证它）。没有匹配的 offering，
                // 因此定价诚实地未知 (#3085)。
                Ok((
                    WireModelId::from(raw),
                    None,
                    "chat".to_string(),
                    RouteLimits::default(),
                    PricingSku::UnknownOrStale,
                ))
            }
            // 聚合器、本地运行时和自定义 OpenAI 兼容端点
            // 合法地接受任意/带前缀的 ID 原样。
            ProviderClass::Aggregator | ProviderClass::LocalOrCustom => {
                let _ = provider_kind;
                // 没有匹配的 offering：定价诚实地未知 (#3085)。
                Ok((
                    WireModelId::from(raw),
                    None,
                    "chat".to_string(),
                    RouteLimits::default(),
                    PricingSku::UnknownOrStale,
                ))
            }
        }
    }

    fn default_offering(&self, provider_id: &ProviderId) -> Option<&ProviderModelOffering> {
        self.offerings
            .iter()
            .find(|offering| offering.provider == *provider_id && offering.default_for_provider)
    }

    /// 当 `raw` 命名了一个位于*不同*提供商的 offering 时为 true。
    ///
    /// `wire_model_id` 分支捕获常见情况（另一个提供商服务的裸 ID）。
    /// `canonical_model` 分支覆盖了规范 ID 没有斜杠的目录行：
    /// Models.dev 规范 ID 通常包含命名空间（`zhipuai/glm-5.2`）
    /// 并在调用点已被 `namespace_hint()` 守卫捕获，
    /// 但裸规范 ID（或手工编写的 offering）会绕过仅 wire-id 匹配。
    /// 它被故意保留，以便裸规范选择器不能在错误的提供商上
    /// 伪装成透传模型。
    fn selector_matches_other_provider_offering(
        &self,
        provider_id: &ProviderId,
        raw: &str,
    ) -> bool {
        self.offerings.iter().any(|offering| {
            offering.provider != *provider_id
                && (offering.wire_model_id.as_str() == raw
                    || offering
                        .canonical_model
                        .as_ref()
                        .is_some_and(|model| model.as_str() == raw))
        })
    }
}

/// 从打包的 Models.dev 资产构建默认的解析器 offerings。
///
/// [`bundled_offerings`] 是一个空的覆盖接缝 (#4139)：当它后来再次获得
/// 策划的行时，这些行在 `(provider, wire id)` 冲突中胜过资产。
/// 目前资产是唯一的打包事实来源。
fn default_offerings() -> Vec<ProviderModelOffering> {
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut out = Vec::new();
    let asset_rows = bundled_catalog_offerings()
        .iter()
        .map(CatalogOffering::to_offering)
        .collect::<Vec<_>>();
    // 接缝优先，因此它在身份冲突中获胜，然后仅资产行跟随。
    for offering in bundled_offerings().into_iter().chain(asset_rows) {
        let key = (
            offering.provider.as_str().to_string(),
            offering.wire_model_id.as_str().to_string(),
        );
        if seen.insert(key) {
            out.push(offering);
        }
    }
    out
}

/// 解析器的最小路由分类。
///
/// 故意比 tui 的 `validate_route` 更窄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderClass {
    /// 严格直接提供商：拒绝明确外来（带前缀）的选择器。
    StrictDirect,
    /// 聚合器：在带前缀的 wire ID 下服务多个目录。
    Aggregator,
    /// 本地运行时或自定义 OpenAI 兼容端点：透传。
    LocalOrCustom,
}

/// 为解析器透传规则分类提供商类型。
///
/// 只有一小部分提供商是严格直接的。其他一切透传，
/// 因此解析器默认保持宽松。
fn classify(kind: ProviderKind) -> ProviderClass {
    match kind {
        // 严格的第一方直接提供商。
        ProviderKind::Deepseek | ProviderKind::Zai => ProviderClass::StrictDirect,
        // 本地运行时 / 自定义 OpenAI 兼容端点。
        ProviderKind::Ollama | ProviderKind::Vllm | ProviderKind::Sglang | ProviderKind::Openai => {
            ProviderClass::LocalOrCustom
        }
        // 其他一切都被视为聚合器风格的透传。
        _ => ProviderClass::Aggregator,
    }
}

fn request_uses_custom_endpoint(
    descriptor: &ProviderDescriptor,
    base_url_override: Option<&str>,
) -> bool {
    base_url_override.is_some_and(|base_url| {
        normalize_route_base_url(base_url)
            != normalize_route_base_url(descriptor.default_base_url())
    })
}

fn normalize_route_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let deepseek_domains = ["api.deepseek.com", "api.deepseeki.com"];
    if deepseek_domains
        .iter()
        .any(|domain| trimmed.to_ascii_lowercase().contains(domain))
    {
        return trimmed.trim_end_matches("/v1").to_string();
    }
    if let Some(idx) = trimmed.find("://") {
        let (scheme, rest) = trimmed.split_at(idx);
        let scheme = scheme.to_ascii_lowercase();
        let rest = &rest[3..];
        let (authority, path) = match rest.find('/') {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, ""),
        };
        return format!("{scheme}://{}{path}", authority.to_ascii_lowercase());
    }
    trimmed.to_ascii_lowercase()
}

/// 当 `base_url` 是 `http://` 端点且其主机不是回环地址时为 true
/// (#1519)。此类端点通过网络以明文发送凭据；
/// 回环地址（`localhost` / `127.0.0.1` / `::1`）豁免，因为本地
/// 运行时（Ollama / vLLM / SGLang）默认使用纯 `http://localhost`。
fn endpoint_uses_insecure_http(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    // 协议匹配不区分大小写，但必须是 `http`，而不是 `https`。
    let Some(rest) = strip_http_scheme(trimmed) else {
        return false;
    };
    !is_loopback_host(host_of_authority(rest))
}

/// 去除开头的不区分大小写的 `http://` 协议，返回剩余部分。
/// 对于任何其他协议（包括 `https://`）或无协议时返回 `None`。
fn strip_http_scheme(base_url: &str) -> Option<&str> {
    let idx = base_url.find("://")?;
    let (scheme, rest) = base_url.split_at(idx);
    if scheme.eq_ignore_ascii_case("http") {
        Some(&rest[3..])
    } else {
        None
    }
}

/// 从 authority+path 字符串中提取裸主机：取 authority 直到
/// 第一个 `/`，去掉任何 `user@` 用户信息和 `:port` 后缀，并解开
/// `[..]` IPv6 括号。
fn host_of_authority(rest: &str) -> &str {
    let authority = rest.split('/').next().unwrap_or(rest);
    // 如果存在，去掉用户信息（`user:pass@host`）。
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(inner) = authority.strip_prefix('[') {
        // 带括号的 IPv6 字面量：主机是直到右括号的所有内容。
        return inner.split(']').next().unwrap_or(inner);
    }
    // 否则去掉尾部的 `:port`。
    authority.split(':').next().unwrap_or(authority)
}

/// `host` 是否为 IPv4/IPv6/名称回环地址。
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_matches(|c| c == '[' || c == ']');
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        // 任何 127.0.0.0/8 地址都是回环地址。
        || host
            .strip_prefix("127.")
            .is_some_and(|_| host.split('.').count() == 4)
}
