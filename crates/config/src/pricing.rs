//! 带来源溯源的提供商/产品级定价投影 (#3085)。
//!
//! 无需网络。将 Models.dev 产品 `cost`（以及实时/用户覆盖行）映射到携带显式
//! **来源**、**货币**和**生效时间**元数据的定价行，再加上一个基于规范化令牌用量的
//! 纯成本估算器。UI 显示（`CostDisplay`）和提供商用量载荷解析位于此层之上，
//! 不在本范围内。
//!
//! 与路由层的边界：此为*定价*模型——产品拥有的每令牌单价。粗粒度的路由层计量形状
//! 已经作为 [`crate::route::PricingSku`] 存在
//! （`Token` / `SubscriptionQuota` / `AccountCredits` / `LocalOrNotApplicable` /
//! `UnknownOrStale`）；[`OfferingPricing::to_route_sku`] 和
//! [`route_pricing_sku`] 连接到它。
//!
//! 诚实规则 (#2608 / #3085)：永远不假定定价。没有来源价格的路由在此处返回 `None`，
//! 在路由层返回 `UnknownOrStale`——从不虚构令牌价格，也从不隐式地将
//! 本地/自定义/订阅路由视为"免费"。

use serde::{Deserialize, Serialize};

use crate::catalog::{CatalogOffering, CatalogSource};
use crate::models_dev::ModelsDevCost;
use crate::route::PricingSku;

/// 定价行的结算货币。Models.dev 发布每百万 USD 成本；
/// 其他货币通过提供商文档或用户覆盖提供。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    #[default]
    Usd,
    Cny,
    /// CodeWhale 不做特殊处理的 ISO-4217 风格代码。
    Other(String),
}

/// 定价行的来源。保留此信息以便 UI 可以显示来源，并且过期/未知价格不会静默地被当作权威数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PricingProvenance {
    /// 从捆绑的 Models.dev 目录快照中播种。
    ModelsDevBundled,
    /// 来自提供商实时 `/models`（或定价）刷新。
    ProviderLive,
    /// 来自提供商文档/手动来源的种子。仅由直接构造行的调用者设置；
    /// `from_catalog_offering` 从不产生此值
    /// （Models.dev 来源的行映射到 `ModelsDevBundled` / `ProviderLive`）。
    ProviderDocs,
    /// 用户提供的覆盖（自定义端点、企业条款、本地路由）。
    UserOverride,
    /// 无来源价格。
    Unknown,
}

/// 单轮次的规范化令牌用量，以规范的计费类别表示。
///
/// 从特定提供商的用量载荷（Chat Completions、Responses、Anthropic）生成此数据
/// 是一个独立的关注点；此层只消费它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// 非缓存输入（提示）令牌。
    pub input: u64,
    /// 输出（补全）令牌，包括推理输出。
    pub output: u64,
    /// 缓存读取（缓存命中）输入令牌，按缓存读取费率计费。
    pub cache_read: u64,
    /// 缓存写入（缓存创建）令牌，按缓存写入费率计费。
    pub cache_write: u64,
}

/// 一个提供商/产品级的定价行。
///
/// 价格以每百万令牌为单位，使用 [`Currency`]。任何字段都可能未知（`None`）；
/// [`OfferingPricing::estimate_cost`] 拒绝为价格未知的已使用类别虚构数字。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfferingPricing {
    /// 提供产品的提供商 ID。
    pub provider: String,
    /// 提供商拥有的线路 ID，价格适用于该线路。
    pub wire_model_id: String,
    /// 规范模型标识（当产品携带时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model: Option<String>,
    /// 结算货币。
    pub currency: Currency,
    /// 每百万令牌的输入价格。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_million: Option<f64>,
    /// 每百万令牌的输出价格。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_million: Option<f64>,
    /// 每百万令牌的缓存读取价格。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_per_million: Option<f64>,
    /// 每百万令牌的缓存写入价格。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_million: Option<f64>,
    /// 价格的来源。
    pub provenance: PricingProvenance,
    /// 价格获取/生效的 Unix 秒数（当已知时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_at: Option<u64>,
}

impl OfferingPricing {
    /// 从目录产品的 `cost` 派生定价行（当有定价时）。
    ///
    /// 当产品没有成本，或成本对象没有具体价格字段时返回 `None`——
    /// 这些路由是*未知*的，而非免费，调用方应相应地渲染它们（参见 [`route_pricing_sku`]）。
    ///
    /// Models.dev `cost` 值是以每百万令牌 USD 计价的，因此货币为 [`Currency::Usd`]；
    /// 来源 provenance 和 `effective_at` 跟随产品的 [`CatalogSource`]。
    #[must_use]
    pub fn from_catalog_offering(offering: &CatalogOffering) -> Option<Self> {
        let cost = offering.cost.as_ref()?;
        if cost.input.is_none()
            && cost.output.is_none()
            && cost.cache_read.is_none()
            && cost.cache_write.is_none()
        {
            return None;
        }
        Some(Self {
            provider: offering.provider.clone(),
            wire_model_id: offering.wire_model_id.clone(),
            canonical_model: offering.canonical_model.clone(),
            currency: Currency::Usd,
            input_per_million: cost.input,
            output_per_million: cost.output,
            cache_read_per_million: cost.cache_read,
            cache_write_per_million: cost.cache_write,
            provenance: provenance_from_source(&offering.source),
            effective_at: effective_at_from_source(&offering.source),
        })
    }

    /// 是否有任何每令牌价格已知。
    #[must_use]
    pub fn has_any_price(&self) -> bool {
        self.input_per_million.is_some()
            || self.output_per_million.is_some()
            || self.cache_read_per_million.is_some()
            || self.cache_write_per_million.is_some()
    }

    /// 此价格在 `now_unix` 时是否早于 `max_age_secs`。
    ///
    /// 没有 `effective_at` 的行（捆绑快照/用户覆盖）没有获取时钟，
    /// 在此处不被视为旧数据；实时行则会被视为旧数据。
    #[must_use]
    pub fn is_stale(&self, now_unix: u64, max_age_secs: u64) -> bool {
        match self.effective_at {
            Some(t) => now_unix.saturating_sub(t) >= max_age_secs,
            None => false,
        }
    }

    /// 估算此行 [`Currency`] 中 `usage` 的成本。
    ///
    /// 如果任何非零令牌数的用量类别有未知价格，则返回 `None`——
    /// 否则估算会静默地低估。如果所有用量为零，则成本为 `Some(0.0)`。
    #[must_use]
    pub fn estimate_cost(&self, usage: &TokenUsage) -> Option<f64> {
        let mut total = 0.0_f64;
        for (tokens, price) in [
            (usage.input, self.input_per_million),
            (usage.output, self.output_per_million),
            (usage.cache_read, self.cache_read_per_million),
            (usage.cache_write, self.cache_write_per_million),
        ] {
            if tokens > 0 {
                let price = price?;
                // 每轮令牌数远低于 2^53，因此此转换是精确的；
                // 如果 TokenUsage 将来跨会话聚合，请重新审视。
                total += (tokens as f64 / 1_000_000.0) * price;
            }
        }
        Some(total)
    }

    /// 投影到粗粒度的路由层计量形状。
    ///
    /// 仅在输入或输出费率已知时返回 [`PricingSku::Token`]。
    /// 路由层的 `Token` 形状仅携带输入/输出费率，因此*仅*在缓存类别上定价的行
    /// 会变成没有可见费率的 `Token`——在路由层会产生误导。此类行在此处降级为
    /// [`PricingSku::UnknownOrStale`]，而其缓存费率仍可通过
    /// [`OfferingPricing::estimate_cost`] 使用。
    #[must_use]
    pub fn to_route_sku(&self) -> PricingSku {
        if self.input_per_million.is_none() && self.output_per_million.is_none() {
            return PricingSku::UnknownOrStale;
        }
        PricingSku::Token {
            input_per_mtok: self.input_per_million,
            output_per_mtok: self.output_per_million,
        }
    }
}

/// 目录产品的诚实路由层定价计量器。
///
/// 具有可用输入/输出费率的产品变为 [`PricingSku::Token`]；
/// 其他所有情况——无成本、成本对象没有具体价格或仅缓存价格——变为
/// [`PricingSku::UnknownOrStale`]，而非虚构的零价格。
/// （`from_catalog_offering` 将未定价情况折叠为 `None`；
/// `to_route_sku` 将仅缓存情况折叠。）
#[must_use]
pub fn route_pricing_sku(offering: &CatalogOffering) -> PricingSku {
    OfferingPricing::from_catalog_offering(offering)
        .map_or(PricingSku::UnknownOrStale, |pricing| pricing.to_route_sku())
}

/// 原始 Models.dev `cost` 块的诚实路由层定价计量器。
///
/// 与 [`route_pricing_sku`] 相同的诚实规则，但适用于直接持有 [`ModelsDevCost`]
/// 的调用者（[`crate::models_dev`] 中的路由产品构建器）而非完整的 [`CatalogOffering`]。
/// 缺失或具体为空的成本，或仅缓存成本，产生 [`PricingSku::UnknownOrStale`]；
/// 仅当有可用输入/输出费率时产生 [`PricingSku::Token`]。
#[must_use]
pub(crate) fn route_pricing_sku_from_cost(cost: Option<&ModelsDevCost>) -> PricingSku {
    let Some(cost) = cost else {
        return PricingSku::UnknownOrStale;
    };
    if cost.input.is_none() && cost.output.is_none() {
        // 无输入/输出费率：仅缓存或空成本会在路由层呈现为无费率的 `Token`，
        // 因此诚实保持为未知。
        return PricingSku::UnknownOrStale;
    }
    PricingSku::Token {
        input_per_mtok: cost.input,
        output_per_mtok: cost.output,
    }
}

fn provenance_from_source(source: &CatalogSource) -> PricingProvenance {
    match source {
        CatalogSource::Bundled => PricingProvenance::ModelsDevBundled,
        CatalogSource::Live { .. } => PricingProvenance::ProviderLive,
        CatalogSource::UserOverride => PricingProvenance::UserOverride,
    }
}

fn effective_at_from_source(source: &CatalogSource) -> Option<u64> {
    match source {
        CatalogSource::Live { fetched_at, .. } => Some(*fetched_at),
        CatalogSource::Bundled | CatalogSource::UserOverride => None,
    }
}

#[cfg(test)]
mod tests;
