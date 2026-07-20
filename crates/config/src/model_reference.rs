//! 事实型模型参考数据库（#3205, #2300）。
//!
//! 一个可浏览的、只读的编译目录投影，用于每个产品
//! "事实卡片"：模型 ID 本身、服务提供商及其种类、
//! 上下文窗口、价格以及模态（文本 vs 多模态）。它存在的目的
//! 是回答"这个模型的声明属性是什么？"，仅此而已。
//!
//! 这一层**只有标签**。它不执行选择、路由、分层
//! 或排序——它从不决定使用哪个模型，并且不携带任何
//! `strong`/`balanced`/`fast` 或角色概念。它是 [`crate::catalog::CatalogOffering`]
//! 行的一个无超集视图。
//!
//! 诚实规则（与 #2608 / #3085 共享）：目录层未声明的属性
//! 报告为 **unknown**，从不猜测。一个没有目录事实的本地/自定义端点
//! 产生 `Unknown` 模态、`None` 上下文窗口和未知价格——
//! 其模型 ID 仍然按原样保留。这里没有任何东西是从模型 ID 前缀推断的。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::ProviderKind;
use crate::catalog::{CatalogOffering, CatalogSnapshot, CatalogSource, bundled_catalog_offerings};
use crate::models_dev::ModelsDevModalities;
use crate::pricing::{Currency, OfferingPricing};

/// 模型的粗略、事实性输入/输出模态标签。
///
/// `text` vs `multimodal` 是从声明的输入/输出模态的并集推导的。
/// 缺少模态元数据是 [`Modality::Unknown`]，与声明的纯文本模型不同——
/// "我们没有被告知"不同于"纯文本"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// 每个声明的模态都是文本。
    Text,
    /// 至少一个声明的模态是非文本的（图像/音频/视频/…）。
    Multimodal,
    /// 此行没有声明模态元数据。
    #[default]
    Unknown,
}

impl Modality {
    /// 从 Models.dev 形状的模态块分类模态。
    ///
    /// 缺少元数据或空白列表返回 [`Modality::Unknown`]，
    /// 当任何声明的输入/输出模态不是 `text` 时返回 [`Modality::Multimodal`]，
    /// 当唯一声明的模态都是文本时返回 [`Modality::Text`]。
    #[must_use]
    pub fn from_modalities(modalities: Option<&ModelsDevModalities>) -> Self {
        let Some(modalities) = modalities else {
            return Self::Unknown;
        };
        let mut saw_any = false;
        for modality in modalities.input.iter().chain(modalities.output.iter()) {
            let trimmed = modality.trim();
            if trimmed.is_empty() {
                continue;
            }
            saw_any = true;
            if !trimmed.eq_ignore_ascii_case("text") {
                return Self::Multimodal;
            }
        }
        if saw_any { Self::Text } else { Self::Unknown }
    }

    /// 稳定的小写标签。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Multimodal => "multimodal",
            Self::Unknown => "unknown",
        }
    }
}

/// 一个提供商产品的事实参考卡片。
///
/// 每个字段要么是声明的事实，要么是显式的未知。这是
/// 一个仅标签的投影：它不承载路由、层级或选择概念。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReferenceCard {
    /// 提供此产品的提供商 ID，与目录行声明完全一致。
    pub provider: String,
    /// 解析的内置提供商种类，当提供商 ID 映射到一个时。
    ///
    /// 对于无法识别/用户命名的自定义提供商为 `None`——
    /// 未知种类，而不是猜测。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<ProviderKind>,
    /// 提供商线路模型 ID，逐字保留。从不规范化或加前缀。
    pub model_id: String,
    /// 规范模型标识，仅当行携带显式连接时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model: Option<String>,
    /// 模型系列/代系，当已声明时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// 上下文窗口令牌数，当已声明时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// 最大输出令牌数，当已声明时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    /// 文本 vs 多模态，或未知。
    pub modality: Modality,
    /// 每令牌定价事实，当有定价时。`None` 是未知，绝不是免费。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<OfferingPricing>,
    /// 底层目录行的来源（捆绑/实时/覆盖）。
    pub source: CatalogSource,
}

impl ModelReferenceCard {
    /// 将目录产品投影到其事实参考卡片。
    #[must_use]
    pub fn from_offering(offering: &CatalogOffering) -> Self {
        Self {
            provider: offering.provider.clone(),
            provider_kind: ProviderKind::parse(&offering.provider),
            model_id: offering.wire_model_id.clone(),
            canonical_model: offering.canonical_model.clone(),
            family: offering.family.clone(),
            context_window: offering.limit.as_ref().and_then(|limit| limit.context),
            max_output: offering.limit.as_ref().and_then(|limit| limit.output),
            modality: Modality::from_modalities(offering.modalities.as_ref()),
            pricing: OfferingPricing::from_catalog_offering(offering),
            source: offering.source.clone(),
        }
    }

    /// 已解析的提供商种类的标签，或 `"unknown"`。
    #[must_use]
    pub fn provider_kind_label(&self) -> &'static str {
        self.provider_kind.map_or("unknown", ProviderKind::as_str)
    }

    /// 人类可读的上下文窗口标签，例如 `"1M"`、`"131K"`、`"512"` 或
    /// `"unknown"`。确切的令牌数保留在 [`Self::context_window`] 上。
    #[must_use]
    pub fn context_window_label(&self) -> String {
        humanize_tokens(self.context_window)
    }

    /// 人类可读的最大输出标签，格式与 [`Self::context_window_label`] 相同。
    #[must_use]
    pub fn max_output_label(&self) -> String {
        humanize_tokens(self.max_output)
    }

    /// 简短的事实价格标签，例如 `"$0.30 / $1.20 per Mtok"`，或
    /// `"unknown"` 当没有每令牌输入/输出费率来源时。
    ///
    /// 某一位的 `?` 表示该单一费率未知而另一位已声明；
    /// 完全未知的价格收敛为 `"unknown"` 而不是虚构的零。
    #[must_use]
    pub fn price_label(&self) -> String {
        let Some(pricing) = self.pricing.as_ref() else {
            return "unknown".to_string();
        };
        if pricing.input_per_million.is_none() && pricing.output_per_million.is_none() {
            return "unknown".to_string();
        }
        let symbol = currency_symbol(&pricing.currency);
        let render = |value: Option<f64>| match value {
            Some(rate) => format!("{symbol}{rate:.2}"),
            None => "?".to_string(),
        };
        let suffix = currency_suffix(&pricing.currency);
        format!(
            "{} / {} per Mtok{suffix}",
            render(pricing.input_per_million),
            render(pricing.output_per_million),
        )
    }
}

/// 一个可浏览的、只读的模型产品事实参考数据库。
///
/// 卡片按 `(provider, model id)` 排序，并在该标识上
/// 去重，因此数据库无论输入顺序如何都是确定性的。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelReferenceDatabase {
    cards: Vec<ModelReferenceCard>,
}

impl ModelReferenceDatabase {
    /// 从原始目录产品构建。
    ///
    /// 行按 `(provider, model id)` 键化；具有相同标识的后一行
    /// 替换前一行，与目录合并语义一致。
    #[must_use]
    pub fn from_offerings(offerings: &[CatalogOffering]) -> Self {
        let mut by_identity: BTreeMap<(String, String), ModelReferenceCard> = BTreeMap::new();
        for offering in offerings {
            let card = ModelReferenceCard::from_offering(offering);
            by_identity.insert((card.provider.clone(), card.model_id.clone()), card);
        }
        Self {
            cards: by_identity.into_values().collect(),
        }
    }

    /// 从编译的目录快照构建（捆绑 < 实时 < 覆盖）。
    #[must_use]
    pub fn from_snapshot(snapshot: &CatalogSnapshot) -> Self {
        Self::from_offerings(&snapshot.offerings)
    }

    /// 从 CodeWhale 的离线/过时捆绑目录快照构建（#4188）。
    ///
    /// 在有可用时优先使用实时/编译的 [`CatalogSnapshot`]。捆绑集
    /// 无需凭据或网络连接，是每个安装都携带的离线回退。
    #[must_use]
    pub fn bundled() -> Self {
        Self::from_offerings(&bundled_catalog_offerings())
    }

    /// 所有卡片，按稳定的 `(provider, model id)` 顺序。
    #[must_use]
    pub fn cards(&self) -> &[ModelReferenceCard] {
        &self.cards
    }

    /// 卡片数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.cards.len()
    }

    /// 数据库是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// 不同的提供商 ID，排序后返回。
    #[must_use]
    pub fn providers(&self) -> Vec<&str> {
        self.cards
            .iter()
            .map(|card| card.provider.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 由一个提供商 ID 提供的所有卡片。
    #[must_use]
    pub fn for_provider(&self, provider: &str) -> Vec<&ModelReferenceCard> {
        self.cards
            .iter()
            .filter(|card| card.provider == provider)
            .collect()
    }

    /// 按 `(provider, model id)` 查找卡片。
    #[must_use]
    pub fn find(&self, provider: &str, model_id: &str) -> Option<&ModelReferenceCard> {
        self.cards
            .iter()
            .find(|card| card.provider == provider && card.model_id == model_id)
    }
}

/// 将令牌数四舍五入为简短的人类可读标签（`"1M"`、`"203K"`、`"512"`），或
/// 对于缺失的计数返回 `"unknown"`。仅用于显示；需要精确值的调用者
/// 直接读取 `Option<u64>` 字段。
fn humanize_tokens(tokens: Option<u64>) -> String {
    let Some(tokens) = tokens else {
        return "unknown".to_string();
    };
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        let rendered = format!("{millions:.2}");
        let trimmed = rendered.trim_end_matches('0').trim_end_matches('.');
        format!("{trimmed}M")
    } else if tokens >= 1_000 {
        format!("{}K", (tokens as f64 / 1_000.0).round() as u64)
    } else {
        tokens.to_string()
    }
}

fn currency_symbol(currency: &Currency) -> &'static str {
    match currency {
        Currency::Usd => "$",
        Currency::Cny => "¥",
        Currency::Other(_) => "",
    }
}

fn currency_suffix(currency: &Currency) -> String {
    match currency {
        Currency::Usd | Currency::Cny => String::new(),
        Currency::Other(code) => format!(" {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models_dev::{ModelsDevCost, ModelsDevLimit};

    fn offering(provider: &str, wire: &str) -> CatalogOffering {
        CatalogOffering {
            provider: provider.to_string(),
            wire_model_id: wire.to_string(),
            endpoint_key: "chat".to_string(),
            source: CatalogSource::Bundled,
            ..Default::default()
        }
    }

    #[test]
    fn modality_text_multimodal_and_unknown() {
        assert_eq!(Modality::from_modalities(None), Modality::Unknown);
        assert_eq!(
            Modality::from_modalities(Some(&ModelsDevModalities::default())),
            Modality::Unknown,
            "空的模态块是未知，而非纯文本"
        );
        assert_eq!(
            Modality::from_modalities(Some(&ModelsDevModalities {
                input: vec!["text".to_string()],
                output: vec!["text".to_string()],
            })),
            Modality::Text
        );
        assert_eq!(
            Modality::from_modalities(Some(&ModelsDevModalities {
                input: vec!["text".to_string(), "image".to_string()],
                output: vec!["text".to_string()],
            })),
            Modality::Multimodal
        );
        // 不区分大小写，容忍仅在输出中存在非文本模态。
        assert_eq!(
            Modality::from_modalities(Some(&ModelsDevModalities {
                input: vec!["TEXT".to_string()],
                output: vec!["Audio".to_string()],
            })),
            Modality::Multimodal
        );
    }

    #[test]
    fn card_projects_stated_facts() {
        let row = CatalogOffering {
            family: Some("deepseek".to_string()),
            limit: Some(ModelsDevLimit {
                context: Some(1_000_000),
                input: None,
                output: Some(384_000),
            }),
            cost: Some(ModelsDevCost {
                input: Some(0.3),
                output: Some(1.2),
                cache_read: Some(0.06),
                cache_write: None,
            }),
            modalities: Some(ModelsDevModalities {
                input: vec!["text".to_string()],
                output: vec!["text".to_string()],
            }),
            ..offering("deepseek", "deepseek-v4-pro")
        };
        let card = ModelReferenceCard::from_offering(&row);

        assert_eq!(card.provider, "deepseek");
        assert_eq!(card.provider_kind, Some(ProviderKind::Deepseek));
        assert_eq!(card.provider_kind_label(), "deepseek");
        assert_eq!(card.model_id, "deepseek-v4-pro");
        assert_eq!(card.family.as_deref(), Some("deepseek"));
        assert_eq!(card.context_window, Some(1_000_000));
        assert_eq!(card.context_window_label(), "1M");
        assert_eq!(card.max_output, Some(384_000));
        assert_eq!(card.max_output_label(), "384K");
        assert_eq!(card.modality, Modality::Text);
        assert_eq!(card.price_label(), "$0.30 / $1.20 per Mtok");
    }

    #[test]
    fn custom_local_row_is_all_unknown_but_keeps_model_id_verbatim() {
        // 用户命名的自定义端点，无目录事实：提供商种类、
        // 上下文窗口、模态和价格都是未知的——从不猜测——
        // 模型 ID 精确保留。
        let row = CatalogOffering {
            source: CatalogSource::UserOverride,
            ..offering("my-local-llm", "Vendor/Custom-Model_v1")
        };
        let card = ModelReferenceCard::from_offering(&row);

        assert_eq!(card.provider_kind, None);
        assert_eq!(card.provider_kind_label(), "unknown");
        assert_eq!(card.model_id, "Vendor/Custom-Model_v1");
        assert_eq!(card.context_window, None);
        assert_eq!(card.context_window_label(), "unknown");
        assert_eq!(card.max_output_label(), "unknown");
        assert_eq!(card.modality, Modality::Unknown);
        assert_eq!(card.price_label(), "unknown");
    }

    #[test]
    fn unpriced_and_cache_only_rows_report_unknown_price_never_zero() {
        // 完全没有成本块。
        let unpriced = ModelReferenceCard::from_offering(&offering("deepseek", "deepseek-v4-pro"));
        assert_eq!(unpriced.price_label(), "unknown");
        assert!(unpriced.pricing.is_none());

        // 一个仅在缓存类上有定价的成本对象，在标题输入/输出费率标签上
        // 仍然是未知的。
        let cache_only = CatalogOffering {
            cost: Some(ModelsDevCost {
                input: None,
                output: None,
                cache_read: Some(0.05),
                cache_write: None,
            }),
            ..offering("acme", "house-model")
        };
        assert_eq!(
            ModelReferenceCard::from_offering(&cache_only).price_label(),
            "unknown"
        );
    }

    #[test]
    fn partial_price_renders_known_rate_and_marks_the_other_unknown() {
        let row = CatalogOffering {
            cost: Some(ModelsDevCost {
                input: Some(5.0),
                output: None,
                cache_read: None,
                cache_write: None,
            }),
            ..offering("openai", "gpt-5.5")
        };
        assert_eq!(
            ModelReferenceCard::from_offering(&row).price_label(),
            "$5.00 / ? per Mtok"
        );
    }

    #[test]
    fn database_is_sorted_deduped_and_queryable() {
        let rows = vec![
            CatalogOffering {
                limit: Some(ModelsDevLimit {
                    context: Some(1),
                    input: None,
                    output: None,
                }),
                ..offering("zai", "GLM-5.2")
            },
            offering("deepseek", "deepseek-v4-pro"),
            // 具有更高上下文相同标识的重复行获胜（后写优先）。
            CatalogOffering {
                limit: Some(ModelsDevLimit {
                    context: Some(1_000_000),
                    input: None,
                    output: None,
                }),
                ..offering("zai", "GLM-5.2")
            },
        ];
        let db = ModelReferenceDatabase::from_offerings(&rows);

        assert_eq!(db.len(), 2, "重复的 (provider, model) 收敛为一个");
        // 按 (provider, model id) 排序：deepseek 在 zai 之前。
        assert_eq!(db.cards()[0].provider, "deepseek");
        assert_eq!(db.cards()[1].provider, "zai");
        assert_eq!(db.providers(), vec!["deepseek", "zai"]);
        assert_eq!(db.for_provider("zai").len(), 1);
        assert_eq!(
            db.find("zai", "GLM-5.2")
                .and_then(|card| card.context_window),
            Some(1_000_000),
            "后写优先保留了更丰富的行"
        );
        assert!(db.find("zai", "missing").is_none());
    }

    #[test]
    fn bundled_database_is_nonempty_and_honest() {
        let db = ModelReferenceDatabase::bundled();
        assert!(!db.is_empty());
        assert!(
            db.len() >= 20,
            "捆绑的离线快照应携带种子产品，得到 {}",
            db.len()
        );

        // 每张卡片保留非空模型 ID，并为捆绑的（一级）提供商
        // 解析已知种类。
        for card in db.cards() {
            assert!(!card.model_id.is_empty());
            assert!(
                card.provider_kind.is_some(),
                "捆绑提供商 {} 应映射到已知种类",
                card.provider
            );
        }

        // 一个 DeepSeek 原生的行：上下文窗口已知，价格诚实未知
        //（捆绑快照省略了 DeepSeek 原生的每令牌定价）。
        let deepseek = db
            .find("deepseek", "deepseek-v4-pro")
            .expect("捆绑的 deepseek 行");
        assert_eq!(deepseek.context_window, Some(1_000_000));
        assert_eq!(deepseek.modality, Modality::Text);
        assert_eq!(deepseek.price_label(), "unknown");

        // 一个定价行展示其声明的每令牌费率。
        let minimax = db
            .find("minimax", "MiniMax-M3")
            .expect("捆绑的 minimax 行");
        assert_eq!(minimax.price_label(), "$0.30 / $1.20 per Mtok");
    }

    #[test]
    fn humanize_tokens_shapes() {
        assert_eq!(humanize_tokens(None), "unknown");
        assert_eq!(humanize_tokens(Some(512)), "512");
        assert_eq!(humanize_tokens(Some(131_072)), "131K");
        assert_eq!(humanize_tokens(Some(1_000_000)), "1M");
        assert_eq!(humanize_tokens(Some(1_050_000)), "1.05M");
    }
}
