//! Models.dev 支持的提供者目录快照和无密钥的活跃缓存
//!（#3385，为 EPIC #2608 和 #3383 提供数据）。
//!
//! 此模块在构造上**不涉及网络**。调用方提供解析后的
//! [`crate::models_dev::ModelsDevCatalog`] JSON（捆绑快照或活跃刷新）
//! 和活跃的 [`ProviderCatalogDelta`]；HTTP `/models` 获取层
//! 位于此模块之上。此处没有任何执行 I/O 或读取凭据的操作。
//!
//! 层次结构（优先级从低到高；#4188）：
//!
//! ```text
//! 捆绑的 Models.dev 快照          （仅离线/过时回退——非竞争性真相）
//!   < 活跃的 Models.dev / 提供者 `/models` 缓存
//!   < 用户/自定义覆盖               （自定义端点、固定模型、显式事实）
//! ```
//!
//! 在 #4187 之后，活跃的 Models.dev 行在存在时优先。捆绑的
//! 资产仍然存在，以便离线启动和失败的刷新仍然能解析默认值。
//!
//! 从 #2608 / #3497 保留的不变量：
//! - 目录行**不是**可执行的路由。行仍然需要通过 `RouteResolver`
//!   编译为 `ReadyRouteCandidate` 后才能执行。
//! - `wire_model_id` 与 `canonical_model` 保持分离；提供者行可能
//!   不暴露规范的 `base_model` 连接，并且前缀从不证明规范所有权。
//! - 未知/自定义/本地行通过显式来源和 `None` 规范模型获得支持。
//!
//! 磁盘缓存格式有意使用普通的 `String` 身份字段，而不是内部的路由
//! newtype，因此持久化的形状与内部类型解耦，并且可以轻松审计为
//!"无密钥"（参见 [`ProviderCatalogCache`] 测试）。

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models_dev::{ModelsDevCatalog, ModelsDevCost, ModelsDevLimit, ModelsDevModalities};
use crate::route::{ModelId, ProviderId, ProviderModelOffering, RouteLimits, WireModelId};

/// 目录行的来源。驱动层次优先级和 UI 来源显示。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogSource {
    /// 离线/过时捆绑种子（Models.dev 形状的快照）。不是竞争性
    /// 真相——活跃的 Models.dev 行覆盖此层（#4188）。
    #[default]
    Bundled,
    /// 提供者活跃 `/models` 行，限定到基础 URL 指纹和
    /// 获取时的 Unix 时间戳。
    Live {
        base_url_fingerprint: String,
        fetched_at: u64,
    },
    /// 用户/自定义覆盖（自定义端点、固定模型、显式事实）。
    UserOverride,
}

/// 一个目录层提供物行。
///
/// 这携带路由身份（提供者 + 有线 ID + 可选规范模型 + 端点）
/// 加上 CodeWhale 想要保留的提供物自有的 Models.dev 事实
///（系列、限制、成本、推理支持/选项）。它是 [`ProviderModelOffering`]
/// 的超集；使用 [`CatalogOffering::to_offering`] 投影解析器
/// 消费的最小路由身份。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CatalogOffering {
    /// 提供此提供物的提供者 ID。
    pub provider: String,
    /// 提供者自有的有线 ID，在请求中逐字发送。
    pub wire_model_id: String,
    /// 规范模型身份，仅当存在显式连接时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model: Option<String>,
    /// 提供物服务的端点键（例如 `chat`）。
    pub endpoint_key: String,
    /// 这是否是提供者的默认提供物。
    #[serde(default)]
    pub default_for_provider: bool,
    /// 为此提供物暴露的模型系列（例如 `glm`、`deepseek`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// 此提供物的 token 限制，当已知时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<ModelsDevLimit>,
    /// 提供者范围的定价，当已知时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelsDevCost>,
    /// 此提供物的输入/输出模态，当已知时。保留为原始 Models.dev
    /// 形状，以便可以派生出事实性的 `text` 与 `multimodal` 标签
    /// 而无需猜测；`None` 表示该层未声明它（未知，而非"仅文本"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelsDevModalities>,
    /// 此提供物是否支持推理，当已知时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// 是否支持工具调用，当已知时（#4115）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    /// 提供者范围的推理控件/接受的努力元数据。保留为原始 JSON，
    /// 以便通过不同网关服务的同一模型系列可以暴露不同的努力
    /// 词汇表，而不会损失性地折叠。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_options: Vec<Value>,
    /// 此行的来源。
    pub source: CatalogSource,
}

impl CatalogOffering {
    /// 作为路由 newtype 的提供者 ID。
    #[must_use]
    pub fn provider_id(&self) -> ProviderId {
        ProviderId::from(self.provider.clone())
    }

    /// 作为路由 newtype 的有线模型 ID。
    #[must_use]
    pub fn wire_id(&self) -> WireModelId {
        WireModelId::from(self.wire_model_id.clone())
    }

    /// 投影解析器消费的最小路由身份。
    ///
    /// 目录故意携带比路由需要更丰富的事实；这丢弃了大部分，
    /// 以便 `RouteResolver::from_offerings` 保持单一接缝。
    /// 面向路由的定价表是例外：它在此处通过
    /// [`crate::pricing::route_pricing_sku`] 投影（其中提供物的
    /// 来源 `cost` 在作用域内），以便解析后的候选者可以携带
    /// 诚实的定价，而路由层从未看到原始成本（#3085）。
    #[must_use]
    pub fn to_offering(&self) -> ProviderModelOffering {
        ProviderModelOffering {
            provider: self.provider_id(),
            canonical_model: self.canonical_model.clone().map(ModelId::from),
            wire_model_id: self.wire_id(),
            endpoint_key: self.endpoint_key.clone(),
            default_for_provider: self.default_for_provider,
            limits: self
                .limit
                .as_ref()
                .map(RouteLimits::from)
                .unwrap_or_default(),
            pricing: crate::pricing::route_pricing_sku(self),
        }
    }

    /// 用于去重和层合并的稳定身份键。
    fn merge_key(&self) -> (String, String) {
        (self.provider.clone(), self.wire_model_id.clone())
    }
}

/// 提交的离线/过时 Models.dev 形状的目录快照（#3385 / #4188）。
///
/// 这**不是**竞争性的策划真相来源。首选元数据来自活跃的
/// Models.dev 目录（#4187）。捆绑的资产是经过验证的仓库内
/// 默认值的紧凑无网络种子（来自 `crates/tui/src/models.rs` 的
/// 上下文/输出，来自 `crates/tui/src/pricing.rs` 的 USD 定价），
/// 以便 [`crate::route::RouteResolver::new`] 和选择器在离线或
/// 刷新失败后仍然工作。参见资产的 `_meta.role` / `_meta.source`
/// 以及关于省略定价的诚实规则（`UnknownOrStale`，绝不是编造的零）。
pub const BUNDLED_MODELS_DEV_JSON: &str = include_str!("../assets/models_dev.bundled.json");

/// 解析提交的捆绑 Models.dev 快照。
///
/// # Panics
/// 仅当提交的资产不是有效的 Models.dev JSON 时 panic。
/// `tests::bundled_asset_parses` 守卫使其成为构建时失败，
/// 因此这在发布的构建中永远不会 panic。
#[must_use]
pub fn bundled_models_dev_catalog() -> ModelsDevCatalog {
    ModelsDevCatalog::parse_json(BUNDLED_MODELS_DEV_JSON)
        .expect("committed bundled Models.dev asset must be valid JSON")
}

/// 来自离线快照的捆绑层 [`CatalogOffering`] 行（#4188）。
///
/// 最低优先级的目录层：来自 [`BUNDLED_MODELS_DEV_JSON`] 的每个
/// 文本聊天行，标记为 [`CatalogSource::Bundled`]。活跃的 Models.dev
/// 行在可用时按 `(provider, wire_model_id)` 覆盖这些行。
#[must_use]
pub fn bundled_catalog_offerings() -> Vec<CatalogOffering> {
    bundled_offerings_from_models_dev(&bundled_models_dev_catalog())
}

/// 从解析的 Models.dev 目录填充捆绑的 [`CatalogOffering`] 行。
///
/// 仅发出文本聊天提供物（TTS/纯音频行保留在解析的目录中，
/// 但从路由候选者中排除，匹配 `ModelsDevCatalog::provider_offerings`）。
/// 每行标记为 [`CatalogSource::Bundled`]。不从前缀推断规范模型；
/// 规范链接仅从显式的 `base_model` 设置。
///
/// 提供者 ID 从 Models.dev 载荷逐字保留（提交的捆绑资产已经
/// 使用 CodeWhale ID）。活跃刷新通过 [`live_offerings_from_models_dev`]
/// 规范化别名。
#[must_use]
pub fn bundled_offerings_from_models_dev(catalog: &ModelsDevCatalog) -> Vec<CatalogOffering> {
    offerings_from_models_dev(catalog, CatalogSource::Bundled, false)
}

/// 从获取的 Models.dev 目录填充活跃的 [`CatalogOffering`] 行（#4187）。
///
/// 与 [`bundled_offerings_from_models_dev`] 相同的文本聊天过滤器，
/// 但每行标记为 [`CatalogSource::Live`] 并带有 Models.dev URL 指纹
/// 和获取时间戳。提供者键在存在别名匹配时规范化为
/// CodeWhale [`crate::ProviderKind`] ID（`moonshotai` → `moonshot`，
/// `togetherai` → `together`，`zhipuai` → `zai`……）；未知的
/// Models.dev 提供者保留其上游 ID，以便它们保持可发现性而无需
/// 成为可执行的路由。
#[must_use]
pub fn live_offerings_from_models_dev(
    catalog: &ModelsDevCatalog,
    base_url_fingerprint: &str,
    fetched_at: u64,
) -> Vec<CatalogOffering> {
    offerings_from_models_dev(
        catalog,
        CatalogSource::Live {
            base_url_fingerprint: base_url_fingerprint.to_string(),
            fetched_at,
        },
        true,
    )
}

fn offerings_from_models_dev(
    catalog: &ModelsDevCatalog,
    source: CatalogSource,
    normalize_provider_ids: bool,
) -> Vec<CatalogOffering> {
    let mut out = Vec::new();
    for (provider_key, provider) in &catalog.providers {
        let raw_id = if provider.id.trim().is_empty() {
            provider_key.trim()
        } else {
            provider.id.trim()
        };
        if raw_id.is_empty() {
            continue;
        }
        let provider_id = if normalize_provider_ids {
            // 将 Models.dev 提供者 ID 规范化为 CodeWhale 类型（当已知时）
            //（#4186）。未知的上游 ID 逐字保留以供目录浏览。
            crate::ProviderKind::parse(raw_id)
                .map(|kind| kind.as_str().to_string())
                .unwrap_or_else(|| raw_id.to_string())
        } else {
            raw_id.to_string()
        };
        for model in provider.models.values() {
            if !model.supports_text_chat() {
                continue;
            }
            out.push(CatalogOffering {
                provider: provider_id.clone(),
                wire_model_id: model.id.clone(),
                canonical_model: model.base_model.clone(),
                endpoint_key: "chat".to_string(),
                default_for_provider: model.default_for_provider,
                family: model.family.clone(),
                limit: model.limit.clone(),
                cost: model.cost.clone(),
                modalities: model.modalities.clone(),
                reasoning: model.reasoning,
                tool_call: model.tool_call,
                reasoning_options: model.reasoning_options.clone(),
                source: source.clone(),
            });
        }
    }
    out
}

/// 提供者的活跃 `/models` 刷新结果，限定到基础 URL 指纹。
///
/// 以 delta 形式返回，而不是直接修改任何全局模型状态，
/// 符合 #3385 架构契约。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderCatalogDelta {
    /// 此 delta 所属的提供者。
    pub provider: String,
    /// 行获取自的基础 URL 指纹。
    pub base_url_fingerprint: String,
    /// 行获取时的 Unix 秒数。
    pub fetched_at: u64,
    /// 活跃提供物行。来源在摄入时规范化为 `Live`。
    pub offerings: Vec<CatalogOffering>,
}

/// 提供者活跃目录刷新未产生可用行的原因。
///
/// 每个变体必须使先前缓存/捆绑/配置的行保持可用；
/// 刷新失败对模型选择绝不致命。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRefreshError {
    /// 401 — 认证缺失或无效。
    Unauthorized,
    /// 403 — 认证存在但未授权。
    Forbidden,
    /// 404 — 提供者在此基础 URL 上不公开 `/models`。
    NotFound,
    /// 429 — 频率限制。
    RateLimited,
    /// 响应无法解析为模型列表。
    InvalidResponse,
    /// 提供者返回空的模型列表。
    EmptyList,
    /// 传输/网络失败。
    Network,
}

/// 提供者缓存的活跃目录的新鲜度/健康状况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogStatus {
    /// 缓存行在它们的 TTL 内。
    Fresh,
    /// 缓存行存在但已超过它们的 TTL。
    Stale { age_secs: u64 },
    /// 上次刷新失败；存在的任何行来自较早的成功。
    Failed { reason: CatalogRefreshError },
    /// 未尝试过刷新此提供者 + 基础 URL。
    Unknown,
}

/// 一个提供者 + 基础 URL 指纹的无密钥缓存提供者目录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedProviderCatalog {
    /// 提供者 ID。
    pub provider: String,
    /// 行获取自的基础 URL 指纹。
    pub base_url_fingerprint: String,
    /// 上次成功获取的 Unix 秒数（失败时不变）。
    pub fetched_at: u64,
    /// 存活时间（秒），超过此行被视为过时。
    pub ttl_secs: u64,
    /// 缓存的活跃提供物行（失败且没有先前行时可能为空）。
    pub offerings: Vec<CatalogOffering>,
    /// 此条目的最后已知状态。
    pub status: CatalogStatus,
}

impl CachedProviderCatalog {
    /// 相对于 `now_unix` 的年龄（秒），在时钟偏差时饱和到零。
    #[must_use]
    pub fn age_secs(&self, now_unix: u64) -> u64 {
        now_unix.saturating_sub(self.fetched_at)
    }

    /// 缓存行在 `now_unix` 时是否已超过其 TTL。
    ///
    /// 零 `ttl_secs` 意味着"始终过时"（从不作为新鲜提供）。
    #[must_use]
    pub fn is_stale(&self, now_unix: u64) -> bool {
        self.age_secs(now_unix) >= self.ttl_secs
    }

    /// 此条目在 `now_unix` 时是否可能贡献活跃提供物。
    ///
    /// 条目仅在 TTL 内**且**上次记录的刷新成功时才为新鲜。
    /// 失败的条目即使在其 TTL 窗口内也永不为新鲜——其行在失败的
    /// 刷新后存活下来，用于通过 [`ProviderCatalogCache::get`] 的
    /// 显式回退显示，但不会作为当前的活跃数据提供。
    #[must_use]
    pub fn is_fresh(&self, now_unix: u64) -> bool {
        !self.is_stale(now_unix) && !matches!(self.status, CatalogStatus::Failed { .. })
    }
}

/// 缓存提供者目录的无密钥存储，按键为提供者 + 基础 URL 指纹。
///
/// 作用域规则（#3385）：不同基础 URL 上的**相同**提供者必须不
/// 共享行，且相同基础 URL 上的**不同**提供者必须不共享行。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderCatalogCache {
    /// 按 [`ProviderCatalogCache::cache_key`] 索引的条目。
    #[serde(default)]
    pub entries: BTreeMap<String, CachedProviderCatalog>,
}

impl ProviderCatalogCache {
    /// 构造空缓存。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 为提供者 + 基础 URL 指纹计算复合缓存键。
    #[must_use]
    pub fn cache_key(provider: &str, base_url_fingerprint: &str) -> String {
        // 单元分隔符避免提供者和指纹之间的歧义。
        format!("{}\u{1f}{}", provider.trim(), base_url_fingerprint.trim())
    }

    /// 按提供者 + 基础 URL 指纹查找缓存的条目。
    #[must_use]
    pub fn get(
        &self,
        provider: &str,
        base_url_fingerprint: &str,
    ) -> Option<&CachedProviderCatalog> {
        self.entries
            .get(&Self::cache_key(provider, base_url_fingerprint))
    }

    /// 记录成功的刷新，替换此作用域的任何先前条目。
    ///
    /// 提供物来源规范化为 [`CatalogSource::Live`] 并带有 delta 的
    /// 指纹和 `fetched_at`，因此缓存的行总是携带诚实的来源，
    /// 无论 delta 是如何组装的。
    pub fn record_success(&mut self, delta: ProviderCatalogDelta, ttl_secs: u64) {
        let ProviderCatalogDelta {
            provider,
            base_url_fingerprint,
            fetched_at,
            offerings,
        } = delta;
        let offerings = offerings
            .into_iter()
            .map(|mut row| {
                row.source = CatalogSource::Live {
                    base_url_fingerprint: base_url_fingerprint.clone(),
                    fetched_at,
                };
                row
            })
            .collect();
        let key = Self::cache_key(&provider, &base_url_fingerprint);
        self.entries.insert(
            key,
            CachedProviderCatalog {
                provider,
                base_url_fingerprint,
                fetched_at,
                ttl_secs,
                offerings,
                status: CatalogStatus::Fresh,
            },
        );
    }

    /// 记录刷新失败。
    ///
    /// 此作用域先前缓存的行被保留（以便 UI 仍然可以以可见的
    /// "过时/失败"状态提供它们）；仅更新状态。当没有先前条目
    /// 存在时，创建空的 `Failed` 条目，以便失败是可观察的。
    pub fn record_failure(
        &mut self,
        provider: &str,
        base_url_fingerprint: &str,
        reason: CatalogRefreshError,
    ) {
        let key = Self::cache_key(provider, base_url_fingerprint);
        match self.entries.get_mut(&key) {
            Some(entry) => entry.status = CatalogStatus::Failed { reason },
            None => {
                self.entries.insert(
                    key,
                    CachedProviderCatalog {
                        provider: provider.trim().to_string(),
                        base_url_fingerprint: base_url_fingerprint.trim().to_string(),
                        fetched_at: 0,
                        ttl_secs: 0,
                        offerings: Vec::new(),
                        status: CatalogStatus::Failed { reason },
                    },
                );
            }
        }
    }

    /// 条目在 `now_unix` 时的已解析状态。
    ///
    /// 记录为 `Fresh` 的条目如果已老化超过其 TTL，则报告
    /// `Stale`；`Failed`/`Unknown` 按存储原样返回。
    #[must_use]
    pub fn status(
        &self,
        provider: &str,
        base_url_fingerprint: &str,
        now_unix: u64,
    ) -> CatalogStatus {
        match self.get(provider, base_url_fingerprint) {
            None => CatalogStatus::Unknown,
            Some(entry) => match &entry.status {
                CatalogStatus::Failed { reason } => CatalogStatus::Failed { reason: *reason },
                CatalogStatus::Unknown => CatalogStatus::Unknown,
                CatalogStatus::Fresh | CatalogStatus::Stale { .. } => {
                    if entry.is_stale(now_unix) {
                        CatalogStatus::Stale {
                            age_secs: entry.age_secs(now_unix),
                        }
                    } else {
                        CatalogStatus::Fresh
                    }
                }
            },
        }
    }

    /// 一个提供者 + 基础 URL 在 `now_unix` 时的新鲜（TTL 内）活跃提供物。
    /// 过时或失败的条目在此不贡献任何内容；调用方回退到捆绑/配置的行
    /// 并单独显示状态。
    #[must_use]
    pub fn fresh_offerings(
        &self,
        provider: &str,
        base_url_fingerprint: &str,
        now_unix: u64,
    ) -> Vec<CatalogOffering> {
        match self.get(provider, base_url_fingerprint) {
            Some(entry) if entry.is_fresh(now_unix) => entry.offerings.clone(),
            _ => Vec::new(),
        }
    }

    /// 跨每个缓存的提供者 + 基础 URL 的所有新鲜活跃提供物。
    #[must_use]
    pub fn all_fresh_offerings(&self, now_unix: u64) -> Vec<CatalogOffering> {
        self.entries
            .values()
            .filter(|entry| entry.is_fresh(now_unix))
            .flat_map(|entry| entry.offerings.clone())
            .collect()
    }

    /// 选择器可能仍然显示的活跃提供物：新鲜行加上在失败的
    /// 刷新后存活的过时/先前行（#4139）。
    ///
    /// 与 [`Self::all_fresh_offerings`] 不同，这保留了超过 TTL
    /// 和 `Failed` 状态且仍然持有提供物行的条目。空条目不贡献
    /// 任何内容；调用方回退到捆绑快照。`now_unix` 被接受以保持
    /// 与新鲜辅助函数的 API 对称性（年龄芯片位于此层之上）。
    #[must_use]
    pub fn all_visible_offerings(&self, _now_unix: u64) -> Vec<CatalogOffering> {
        self.entries
            .values()
            .filter(|entry| !entry.offerings.is_empty())
            .flat_map(|entry| entry.offerings.clone())
            .collect()
    }
}

/// 编译后的、层合并的目录快照。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    /// 合并后的提供物，按 (provider, wire id) 去重，以稳定顺序排列。
    pub offerings: Vec<CatalogOffering>,
}

impl CatalogSnapshot {
    /// 投影路由提供物，用于 `RouteResolver::from_offerings`。
    #[must_use]
    pub fn to_offerings(&self) -> Vec<ProviderModelOffering> {
        self.offerings
            .iter()
            .map(CatalogOffering::to_offering)
            .collect()
    }

    /// 一个提供者 ID 的所有提供物。
    #[must_use]
    pub fn offerings_for_provider(&self, provider: &str) -> Vec<&CatalogOffering> {
        self.offerings
            .iter()
            .filter(|row| row.provider == provider)
            .collect()
    }
}

/// 通过按优先级顺序合并层来构建 [`CatalogSnapshot`]：
/// bundled < live < user overrides。后面的层覆盖共享 (provider, wire id)
/// 身份的较早行。
#[derive(Debug, Clone, Default)]
pub struct CatalogCompiler {
    bundled: Vec<CatalogOffering>,
    live: Vec<CatalogOffering>,
    overrides: Vec<CatalogOffering>,
}

impl CatalogCompiler {
    /// 启动空编译器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加捆绑行（最低优先级）。
    #[must_use]
    pub fn with_bundled(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.bundled.extend(rows);
        self
    }

    /// 从解析的 Models.dev 目录种子捆绑行。
    #[must_use]
    pub fn with_models_dev(mut self, catalog: &ModelsDevCatalog) -> Self {
        self.bundled
            .extend(bundled_offerings_from_models_dev(catalog));
        self
    }

    /// 添加活跃行（中间优先级）。
    #[must_use]
    pub fn with_live(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.live.extend(rows);
        self
    }

    /// 添加用户/自定义覆盖行（最高优先级）。
    #[must_use]
    pub fn with_overrides(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.overrides.extend(rows);
        self
    }

    /// 将所有层合并为确定性快照。
    #[must_use]
    pub fn compile(self) -> CatalogSnapshot {
        let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
        for row in self
            .bundled
            .into_iter()
            .chain(self.live)
            .chain(self.overrides)
        {
            merged.insert(row.merge_key(), row);
        }
        CatalogSnapshot {
            offerings: merged.into_values().collect(),
        }
    }
}

/// 规范化基础 URL 并为其生成指纹，用于缓存作用域。
///
/// 规范化折叠 scheme/host 的大小写，修剪尾部斜杠，并
/// 删除默认端口后缀，因此同一个端点在表面上不同的拼写共享
/// 一个缓存作用域，而真正不同的端点则不共享。指纹是无依赖的
/// FNV-1a 十六进制摘要；它在运行内和跨运行都是确定性的，但
/// 不是加密哈希（它标识缓存桶，与安全无关）。
#[must_use]
pub fn base_url_fingerprint(base_url: &str) -> String {
    let normalized = normalize_base_url(base_url);
    fnv1a_hex(normalized.as_bytes())
}

fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    // 仅小写 scheme://host 部分；保留路径大小写。
    if let Some(idx) = trimmed.find("://") {
        let (scheme, rest) = trimmed.split_at(idx);
        let scheme = scheme.to_ascii_lowercase();
        let rest = &rest[3..];
        let (authority, path) = match rest.find('/') {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, ""),
        };
        let authority = authority.to_ascii_lowercase();
        // 仅剥离 scheme 自身的默认端口，因此像 `http://host:443`
        // 这样的非默认配对保持与 `http://host` 不同。
        let default_port = match scheme.as_str() {
            "https" => Some(":443"),
            "http" => Some(":80"),
            _ => None,
        };
        let authority = default_port
            .and_then(|port| authority.strip_suffix(port))
            .unwrap_or(&authority);
        format!("{scheme}://{authority}{path}")
    } else {
        trimmed.to_ascii_lowercase()
    }
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// 当前 Unix 时间（秒），供组装 delta / 缓存条目的调用方使用。
///
/// 纯缓存逻辑显式接受 `now_unix` 以在测试中保持确定性；
/// 此辅助函数是读取挂钟的唯一点。
#[must_use]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
