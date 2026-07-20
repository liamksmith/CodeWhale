//! 提供商模型产品（#3084）。
//!
//! [`ProviderModelOffering`] 将提供商绑定到规范模型、提供商拥有的有线 ID 以及端点键。
//! 这是证明 #2608 不变量的接缝：同一个规范模型可以由多个提供商以不同的有线 ID（部分带有聚合器前缀）
//! 提供服务，并且前缀绝不意味着提供商所有权。
//!
//! 手动筛选的种子表已移除（#4139 / #3830 P1）：来自 [`crate::catalog::bundled_catalog_offerings`] 的
//! 目录派生产品是唯一的捆绑真相来源。[`bundled_offerings`] 保留为空接缝，以便解析器可以在以后
//! 预置筛选的覆盖项，而无需重新引入并行的种子列表。

use serde::{Deserialize, Serialize};

use super::candidate::PricingSku;
use super::ids::{ModelId, ProviderId, WireModelId};

/// 一个已解析路由/产品的 token 限制。
///
/// 这些是可选的，因为托管目录、本地运行时和自定义端点可以合法地省略部分或全部限制信息。
/// 调用者应将 `None` 视为未知，而非零。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteLimits {
    /// 总上下文窗口（输入 + 输出），以 token 计。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// 输入 token 限制，当提供商单独报告时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// 路由/产品的输出 token 上限（已知时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

impl RouteLimits {
    /// 是否至少已知一个限制信息。
    #[must_use]
    pub const fn has_known_limit(self) -> bool {
        self.context_tokens.is_some() || self.input_tokens.is_some() || self.output_tokens.is_some()
    }
}

/// 一个提供商提供（可能是规范）模型的方式。
///
/// 有意未派生 `Eq`：[`PricingSku::Token`] 携带 `f64` 费率，因此该产品仅为 `PartialEq`。
/// 没有调用者以产品为键来设置集合/映射。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderModelOffering {
    /// 提供此产品的提供商。
    pub provider: ProviderId,
    /// 规范模型标识，如果此产品映射到一个。
    pub canonical_model: Option<ModelId>,
    /// 提供商拥有的有线 ID，按原样发送到请求中。
    pub wire_model_id: WireModelId,
    /// 提供此产品的端点键。
    pub endpoint_key: String,
    /// 是否为该提供商的默认产品。
    pub default_for_provider: bool,
    /// 提供商/产品范围的 token 限制（已知时）。
    pub limits: RouteLimits,
    /// 此产品的粗粒度面向路由的定价计量器（#3085）。
    ///
    /// 从拥有该产品的层（`CatalogOffering::to_offering` → [`crate::pricing::route_pricing_sku`]）的来源成本投影而来。
    /// 解析器将其按原样携带到候选者上；当未获取到价格时，它为 [`PricingSku::UnknownOrStale`]——绝不是一个虚构的零值（#2608 / #3085 诚实规则）。
    pub pricing: PricingSku,
}

/// 返回捆绑的产品接缝作为拥有的 [`ProviderModelOffering`] 行。
///
/// 设计为空：每个以前的手动种子行都被捆绑的 Models.dev 目录（[`crate::catalog::bundled_catalog_offerings`]）覆盖，
/// 该目录通过 `base_model` 携带相同的规范模型连接，加上旧种子所缺少的真实限制和定价（#4139 / #3830 P1 OFFERING_SEEDS 去重）。
#[must_use]
pub fn bundled_offerings() -> Vec<ProviderModelOffering> {
    Vec::new()
}
