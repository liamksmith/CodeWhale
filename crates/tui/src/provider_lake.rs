//! 已配置的提供商/模型湖外观（#3830, Wave 5b / #4188）。
//!
//! 覆盖 Models.dev 目录层和与 `/provider` 共享的已配置提供商
//! 谓词的单一切面。优先级为**实时 Models.dev > 捆绑离线快照 >
//! 遗留硬编码回退**。选择器、热栏路由槽、[`crate::model_inventory::ModelInventory`]、
//! 斜杠补全和子代理验证应从此处读取模型列表。
//!
//! [`crate::config::model_completion_names_for_provider`] 仅作为兼容性回退保留，
//! 用于 Models.dev 不表示的仅 CodeWhale/本地提供商（以及在实时目录覆盖
//! 它们之前的未捆绑网关）。

use std::sync::RwLock;

use codewhale_config::catalog::{CatalogOffering, CatalogSnapshot, bundled_catalog_offerings};

use crate::config::{
    ApiProvider, Config, model_completion_names_for_provider, provider_is_configured_for_active,
};

static BUNDLED_SNAPSHOT: std::sync::OnceLock<CatalogSnapshot> = std::sync::OnceLock::new();

/// 可选的实时 Models.dev 快照（#4187）。当为 `None` 时，仅捆绑的
/// 离线/过时回退行可见。
static LIVE_SNAPSHOT: RwLock<Option<CatalogSnapshot>> = RwLock::new(None);

fn bundled_snapshot() -> &'static CatalogSnapshot {
    BUNDLED_SNAPSHOT.get_or_init(|| CatalogSnapshot {
        offerings: bundled_catalog_offerings(),
    })
}

/// 设置实时目录快照。在后台刷新成功后调用此函数；
/// 湖在下一次读取时将实时行合并到捆绑行之上。
/// 过时或空的快照无害——`None` 仅表示"仅限捆绑"。
pub fn set_live_snapshot(snapshot: CatalogSnapshot) {
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        *guard = Some(snapshot);
    }
}

/// 清除实时快照（例如在缓存逐出或关闭时）。
pub fn clear_live_snapshot() {
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        *guard = None;
    }
}

/// 合并的目录快照：实时行在 `(provider, wire_model_id)` 标识上
/// 覆盖捆绑行（#4188）。当没有实时快照时，
/// 这只是离线捆绑快照。
fn merged_snapshot() -> CatalogSnapshot {
    let live = LIVE_SNAPSHOT.read().ok().and_then(|guard| guard.clone());
    match live {
        None => bundled_snapshot().clone(),
        Some(live) => {
            use std::collections::BTreeMap;
            let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
            for row in &bundled_snapshot().offerings {
                merged.insert(
                    (row.provider.clone(), row.wire_model_id.clone()),
                    row.clone(),
                );
            }
            for row in &live.offerings {
                merged.insert(
                    (row.provider.clone(), row.wire_model_id.clone()),
                    row.clone(),
                );
            }
            CatalogSnapshot {
                offerings: merged.into_values().collect(),
            }
        }
    }
}

/// 将 [`ApiProvider`] 映射到其捆绑目录的提供商 ID。
fn catalog_provider_id(provider: ApiProvider) -> &'static str {
    match provider {
        ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => "deepseek",
        ApiProvider::SiliconflowCn => "siliconflow",
        _ => provider.as_str(),
    }
}

fn push_unique_model(models: &mut Vec<String>, model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    if !models
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(model))
    {
        models.push(model.to_string());
    }
}

fn catalog_models_from_offerings<'a>(
    offerings: impl IntoIterator<Item = &'a CatalogOffering>,
) -> Vec<String> {
    let mut rows: Vec<_> = offerings.into_iter().collect();
    rows.sort_by(|left, right| {
        right
            .default_for_provider
            .cmp(&left.default_for_provider)
            .then_with(|| left.wire_model_id.cmp(&right.wire_model_id))
    });
    let mut models = Vec::new();
    for row in rows {
        push_unique_model(&mut models, &row.wire_model_id);
    }
    models
}

/// 一个提供商的目录支持的模型 ID（#4188）。
///
/// 优先级：实时 Models.dev 行（当已发布时）在 `(provider, wire_model_id)` 上
/// 覆盖捆绑离线行；如果合并目录仍然没有该提供商的行，
/// 回退到 [`crate::config::model_completion_names_for_provider`]，以便
/// 仅 CodeWhale/本地提供商（以及尚未在离线种子中的网关）保持默认值。
#[must_use]
pub fn all_catalog_models_for_provider(provider: ApiProvider) -> Vec<String> {
    let catalog_id = catalog_provider_id(provider);
    let merged = merged_snapshot();
    let mut models = catalog_models_from_offerings(merged.offerings_for_provider(catalog_id));
    if models.is_empty() {
        for model in model_completion_names_for_provider(provider) {
            push_unique_model(&mut models, model);
        }
    }
    models
}

/// 查找 `(provider, wire_model_id)` 的合并目录产品（#4115）。
///
/// 当存在时返回实时覆盖捆绑的行，以便选择器元数据（上下文、
/// 定价、工具、推理、新鲜度）无需第二次目录遍历即可投影。
/// 对没有 Models.dev 行的仅 CodeWhale/遗留回退 ID 返回 `None`。
#[must_use]
pub fn catalog_offering_for_model(
    provider: ApiProvider,
    wire_model_id: &str,
) -> Option<CatalogOffering> {
    let catalog_id = catalog_provider_id(provider);
    let needle = wire_model_id.trim();
    if needle.is_empty() {
        return None;
    }
    merged_snapshot()
        .offerings_for_provider(catalog_id)
        .into_iter()
        .find(|row| row.wire_model_id.eq_ignore_ascii_case(needle))
        .cloned()
}

/// 一个提供商的合并目录模型计数（目录视图/仪表板）。
#[must_use]
pub fn catalog_model_count_for_provider(provider: ApiProvider) -> usize {
    all_catalog_models_for_provider(provider).len()
}

/// 用户已设置的提供商——活跃提供商、有效凭据/OAuth、
/// 或显式的 `[providers.<name>]` 条目（#3830）。
#[must_use]
pub fn configured_providers(config: &Config, active: ApiProvider) -> Vec<ApiProvider> {
    ApiProvider::sorted_for_display()
        .into_iter()
        .filter(|provider| provider_is_configured_for_active(config, *provider, active))
        .collect()
}

/// 符合 `active` 已配置条件的提供商的目录模型。
#[must_use]
pub fn models_for_provider(
    config: &Config,
    active: ApiProvider,
    provider: ApiProvider,
) -> Vec<String> {
    if provider_is_configured_for_active(config, provider, active) {
        all_catalog_models_for_provider(provider)
    } else {
        Vec::new()
    }
}

/// 每个携带至少一个合并目录行的内置提供商。
#[must_use]
#[allow(dead_code)]
pub fn all_catalog_providers() -> Vec<ApiProvider> {
    let mut seen = Vec::new();
    for offering in &merged_snapshot().offerings {
        if let Some(provider) = ApiProvider::parse(&offering.provider)
            && !seen.contains(&provider)
        {
            seen.push(provider);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TOGETHER_FLASH_MODEL, DEFAULT_TOGETHER_MODEL};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// 序列化修改进程级实时快照的测试。
    fn lock_live_snapshot() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn together_catalog_includes_flash_from_bundled_asset() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let models = all_catalog_models_for_provider(ApiProvider::Together);
        assert!(
            models.contains(&DEFAULT_TOGETHER_MODEL.to_string()),
            "缺少 Together pro: {models:?}"
        );
        assert!(
            models.contains(&DEFAULT_TOGETHER_FLASH_MODEL.to_string()),
            "缺少 Together flash: {models:?}"
        );
    }

    #[test]
    fn configured_providers_matches_provider_predicate() {
        let config = Config::default();
        let active = ApiProvider::Deepseek;
        let expected: Vec<_> = ApiProvider::sorted_for_display()
            .into_iter()
            .filter(|provider| {
                crate::config::provider_is_configured_for_active(&config, *provider, active)
            })
            .collect();
        assert_eq!(configured_providers(&config, active), expected);
    }

    #[test]
    fn models_for_provider_filters_unconfigured_gateways() {
        let _env_lock = crate::test_support::lock_test_env();
        let _together = crate::test_support::EnvVarGuard::remove("TOGETHER_API_KEY");
        let config = Config::default();
        assert!(
            models_for_provider(&config, ApiProvider::Deepseek, ApiProvider::Together).is_empty()
        );
        assert!(
            !models_for_provider(&config, ApiProvider::Deepseek, ApiProvider::Deepseek).is_empty()
        );
    }

    /// #4116 关键（已迁移消费者的不缩小保证）：目录支持的外观
    /// 必须为每个具有非空遗留 `model_completion_names_for_provider` 表的提供商
    /// 返回非空枚举。`all_catalog_models_for_provider` 在合并目录没有该提供商行时
    /// 回退到该遗留表，因此按构造成立——并且它证明从子代理
    /// `operator_model_for_subagent` 消费者中移除的原始遗留尾部（仅在
    /// 外观为空时运行）在遗留非空时是不可到达的。因此已迁移的消费者
    /// 是行为保持的：它总是有一个目录来源的模型可供选择，
    /// 并且从不缩小到比遗留路径提供的更少选择。
    ///
    /// 注意：外观有意*以目录为权威*（实时 > 捆绑 > 遗留回退，#4188），
    /// 因此对于一些目录取代遗留占位符表中过时条目的提供商
    ///（例如 OpenRouter/MiniMax 修订版），外观不是每个遗留 ID 的严格超集。
    /// 这种差异不影响子代理模型的*接受*，后者由 `validate_route` /
    /// `requested_model_for_provider` 门控，而非此列表。
    #[test]
    fn catalog_facade_covers_every_provider_with_a_legacy_table() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        for &provider in ApiProvider::all() {
            let legacy_len = model_completion_names_for_provider(provider).len();
            if legacy_len == 0 {
                continue;
            }
            assert!(
                !all_catalog_models_for_provider(provider).is_empty(),
                "目录外观为 {provider:?} 返回空模型，尽管遗留表非空（{legacy_len} 个条目）：\
                 操作者路由消费者将无可枚举内容"
            );
        }
    }

    /// #4188：仅 CodeWhale/本地提供商在 Models.dev（实时或捆绑）没有其行时
    /// 通过遗留回退保持默认值。
    #[test]
    fn codewhale_only_providers_keep_legacy_defaults() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let openai_codex = all_catalog_models_for_provider(ApiProvider::OpenaiCodex);
        assert!(
            !openai_codex.is_empty(),
            "openai-codex 必须在离线状态下保持默认模型: {openai_codex:?}"
        );
        assert_eq!(
            openai_codex,
            model_completion_names_for_provider(ApiProvider::OpenaiCodex)
                .iter()
                .map(|m| (*m).to_string())
                .collect::<Vec<_>>(),
            "openai-codex 应来自兼容性回退表"
        );

        // Ollama 有意具有空的遗留表（用户提供的 ID）；
        // 湖仍然必须返回空而不是发明行。
        assert!(all_catalog_models_for_provider(ApiProvider::Ollama).is_empty());
        assert!(model_completion_names_for_provider(ApiProvider::Ollama).is_empty());
    }

    /// #4116 / #4188（验收条件）：没有捆绑/实时目录覆盖的提供商必须
    /// 按原样回退到遗留表，以便仅 CodeWhale 路由保持可用。
    /// 我们为每个当前未捆绑但仍具有非空遗留列表的提供商断言这一点，
    /// 并要求至少存在一个这样的提供商，以便实际执行回退路径。
    #[test]
    fn unbundled_provider_falls_back_to_legacy_table() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let merged = merged_snapshot();
        let mut exercised = 0usize;
        for &provider in ApiProvider::all() {
            let catalog_id = catalog_provider_id(provider);
            let has_catalog_rows = !merged.offerings_for_provider(catalog_id).is_empty();
            let legacy = model_completion_names_for_provider(provider);
            if has_catalog_rows || legacy.is_empty() {
                continue;
            }
            // 未捆绑 + 非空遗留：外观必须回显遗留列表。
            let facade = all_catalog_models_for_provider(provider);
            let expected: Vec<String> = legacy.iter().map(|m| m.to_string()).collect();
            assert_eq!(
                facade, expected,
                "未捆绑提供商 {provider:?} 未回退到遗留表"
            );
            exercised += 1;
        }
        assert!(
            exercised > 0,
            "预期至少有一个未捆绑提供商执行遗留回退路径"
        );
    }

    /// #4188：实时 Models.dev 行在标识上胜于捆绑行，清除
    /// 实时快照恢复离线捆绑快照（离线启动仍然有效）。
    #[test]
    fn live_snapshot_merges_over_bundled() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        // 没有实时快照时，获得捆绑模型。
        let bundled = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert!(!bundled.is_empty());

        // 设置一个添加合成模型的实时快照。
        let live = CatalogSnapshot {
            offerings: vec![CatalogOffering {
                provider: "deepseek".to_string(),
                wire_model_id: "deepseek-v4-synthetic".to_string(),
                endpoint_key: "chat".to_string(),
                ..Default::default()
            }],
        };
        set_live_snapshot(live);
        let merged = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert!(merged.contains(&"deepseek-v4-synthetic".to_string()));
        // 捆绑模型仍然存在。
        assert!(merged.iter().any(|m| bundled.contains(m)));

        clear_live_snapshot();
        let after_clear = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert_eq!(after_clear, bundled);
    }

    /// #4188：实时 > 捆绑 > 遗留回退优先级，包括实时
    /// 覆盖捆绑线路 ID 以及在别名规范化后没有重复行
    ///（`moonshotai` → `moonshot`）。
    #[test]
    fn live_over_bundled_over_legacy_precedence_and_alias_dedupe() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        let bundled_moonshot = all_catalog_models_for_provider(ApiProvider::Moonshot);
        assert!(
            !bundled_moonshot.is_empty(),
            "离线捆绑 Moonshot 种子必需: {bundled_moonshot:?}"
        );

        // 实时行使用 Models.dev 别名 ID；湖合并必须规范化为
        // CodeWhale `moonshot` 并且不留并行的 `moonshotai` 桶。
        let live = CatalogSnapshot {
            offerings: vec![
                CatalogOffering {
                    provider: "moonshot".to_string(),
                    wire_model_id: "kimi-k2.5-live".to_string(),
                    endpoint_key: "chat".to_string(),
                    default_for_provider: true,
                    ..Default::default()
                },
                // 与典型捆绑 Moonshot 默认值相同标识——实时胜出。
                CatalogOffering {
                    provider: "moonshot".to_string(),
                    wire_model_id: bundled_moonshot[0].clone(),
                    endpoint_key: "chat".to_string(),
                    family: Some("live-override".to_string()),
                    ..Default::default()
                },
            ],
        };
        set_live_snapshot(live);

        let merged = merged_snapshot();
        let moonshot_rows = merged.offerings_for_provider("moonshot");
        assert!(
            moonshot_rows
                .iter()
                .any(|r| r.wire_model_id == "kimi-k2.5-live"),
            "仅实时的 Moonshot 行缺失: {moonshot_rows:?}"
        );
        let overridden = moonshot_rows
            .iter()
            .find(|r| r.wire_model_id == bundled_moonshot[0])
            .expect("捆绑 Moonshot ID 在实时合并后仍应存在");
        assert_eq!(
            overridden.family.as_deref(),
            Some("live-override"),
            "实时行必须用相同的线路 ID 替换捆绑事实"
        );
        assert!(
            merged.offerings_for_provider("moonshotai").is_empty(),
            "别名规范化的提供商不得留下重复的 moonshotai 桶"
        );

        let models = all_catalog_models_for_provider(ApiProvider::Moonshot);
        let mut seen = std::collections::BTreeSet::new();
        for model in &models {
            assert!(
                seen.insert(model.to_ascii_lowercase()),
                "别名合并后重复的 Moonshot 模型行: {model}"
            );
        }
        assert!(models.contains(&"kimi-k2.5-live".to_string()));

        // 当目录行存在时跳过遗留回退（即使遗留列表包含额外 ID）——
        // 目录一旦非空即为权威。
        assert!(
            !model_completion_names_for_provider(ApiProvider::Moonshot).is_empty(),
            "遗留 Moonshot 表仍应作为回退文档存在"
        );

        clear_live_snapshot();
        assert_eq!(
            all_catalog_models_for_provider(ApiProvider::Moonshot),
            bundled_moonshot,
            "清除实时必须恢复离线捆绑 Moonshot 行"
        );
    }

    /// #4188：当实时 Models.dev 为同一提供商同时发出别名 ID 和 CodeWhale ID
    /// 时，通过 `live_offerings_from_models_dev` 编译然后合并到湖中
    /// 不得产生重复的模型行。
    #[test]
    fn alias_normalized_live_rows_do_not_duplicate_in_lake() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let body = r#"{
          "models": {},
          "providers": {
            "moonshotai": {
              "id": "moonshotai",
              "models": {
                "kimi-k2.5": {
                  "id": "kimi-k2.5",
                  "modalities": { "input": ["text"], "output": ["text"] }
                }
              }
            },
            "moonshot": {
              "id": "moonshot",
              "models": {
                "kimi-k2.5": {
                  "id": "kimi-k2.5",
                  "modalities": { "input": ["text"], "output": ["text"] },
                  "limit": { "context": 262144, "output": 8192 }
                },
                "kimi-k2.7-code": {
                  "id": "kimi-k2.7-code",
                  "modalities": { "input": ["text"], "output": ["text"] }
                }
              }
            }
          }
        }"#;
        let catalog =
            codewhale_config::models_dev::ModelsDevCatalog::parse_json(body).expect("parse");
        let live_rows = codewhale_config::catalog::live_offerings_from_models_dev(
            &catalog,
            "alias-fp",
            1_700_000_000,
        );
        assert!(
            live_rows.iter().all(|r| r.provider == "moonshot"),
            "moonshotai 和 moonshot 都必须规范化为 moonshot: {:?}",
            live_rows
                .iter()
                .map(|r| r.provider.as_str())
                .collect::<Vec<_>>()
        );
        set_live_snapshot(CatalogSnapshot {
            offerings: live_rows,
        });

        let models = all_catalog_models_for_provider(ApiProvider::Moonshot);
        let kimi_count = models.iter().filter(|m| m.as_str() == "kimi-k2.5").count();
        assert_eq!(
            kimi_count, 1,
            "别名规范化的提供商不得重复 kimi-k2.5: {models:?}"
        );
        assert!(
            merged_snapshot()
                .offerings_for_provider("moonshotai")
                .is_empty()
        );
        clear_live_snapshot();
    }
}
