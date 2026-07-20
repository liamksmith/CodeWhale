//! Models.dev 支持的目录缓存的冒烟测试（#3385）。
//!
//! 测试数据使用合成 ID 进行反硬编码守卫，加上 issue 明确要求测试的
//! GLM-5.2 和托管的 DeepSeek 行。此处不复制完整的托管提供商模型列表。

use super::*;

/// Zhipu 规范 + Zhipu/Z.AI 提供商提供物，以及由聚合器以带前缀的
/// 有线 ID 提供的托管 DeepSeek 行，并带有显式的规范 `base_model` 连接。
const FIXTURE: &str = r#"{
  "models": {
    "zhipuai/glm-5.2": {
      "id": "zhipuai/glm-5.2",
      "family": "glm",
      "reasoning": true,
      "modalities": { "input": ["text"], "output": ["text"] },
      "limit": { "context": 1000000, "output": 131072 }
    }
  },
  "providers": {
    "zhipuai": {
      "id": "zhipuai",
      "models": {
        "glm-5.2": {
          "id": "glm-5.2",
          "family": "glm",
          "default": true,
          "reasoning": true,
          "reasoning_options": [{ "type": "effort", "values": ["high", "max"] }],
          "modalities": { "input": ["text"], "output": ["text"] },
          "limit": { "context": 1000000, "output": 131072 },
          "cost": { "input": 1.4, "output": 4.4, "cache_read": 0.26 }
        },
        "glm-voice": {
          "id": "glm-voice",
          "modalities": { "input": ["text"], "output": ["audio"] }
        }
      }
    },
    "together": {
      "id": "together",
      "models": {
        "deepseek-ai/DeepSeek-V4-Pro": {
          "id": "deepseek-ai/DeepSeek-V4-Pro",
          "base_model": "deepseek-v4-pro",
          "family": "deepseek",
          "reasoning": false,
          "modalities": { "input": ["text"], "output": ["text"] },
          "cost": { "input": 0.9, "output": 0.9 }
        }
      }
    }
  }
}"#;

fn fixture() -> ModelsDevCatalog {
    ModelsDevCatalog::parse_json(FIXTURE).expect("fixture parses")
}

fn find<'a>(rows: &'a [CatalogOffering], provider: &str, wire: &str) -> &'a CatalogOffering {
    rows.iter()
        .find(|r| r.provider == provider && r.wire_model_id == wire)
        .unwrap_or_else(|| panic!("offering {provider}/{wire} not found"))
}

#[test]
fn hydrates_models_dev_offerings_preserving_offering_facts() {
    let rows = bundled_offerings_from_models_dev(&fixture());

    // glm-voice（音频输出）被排除；剩下两个对话提供物。
    assert_eq!(rows.len(), 2, "audio-only rows are not chat offerings");

    let glm = find(&rows, "zhipuai", "glm-5.2");
    assert!(glm.default_for_provider);
    assert_eq!(glm.family.as_deref(), Some("glm"));
    assert_eq!(glm.reasoning, Some(true));
    // 提供商作用域的推理选项被保留，而非折叠。
    assert_eq!(glm.reasoning_options.len(), 1);
    assert_eq!(glm.limit.as_ref().and_then(|l| l.context), Some(1_000_000));
    assert_eq!(glm.cost.as_ref().and_then(|c| c.cache_read), Some(0.26));
    // 提供商行未携带 base_model 链接 → 非推断的规范模型。
    assert_eq!(glm.canonical_model, None);
    assert_eq!(glm.source, CatalogSource::Bundled);
}

#[test]
fn hosted_offering_keeps_prefixed_wire_id_and_explicit_canonical_join() {
    let rows = bundled_offerings_from_models_dev(&fixture());
    let hosted = find(&rows, "together", "deepseek-ai/DeepSeek-V4-Pro");

    // 带前缀的有线 ID 在服务提供商下原样保留。
    assert_eq!(hosted.wire_model_id, "deepseek-ai/DeepSeek-V4-Pro");
    assert_eq!(hosted.provider, "together");
    // 规范链接仅来自显式的 base_model。
    assert_eq!(hosted.canonical_model.as_deref(), Some("deepseek-v4-pro"));
    assert_eq!(hosted.reasoning, Some(false));
}

#[test]
fn to_offering_projects_routing_identity_and_limits() {
    let rows = bundled_offerings_from_models_dev(&fixture());
    let glm = find(&rows, "zhipuai", "glm-5.2").to_offering();

    assert_eq!(glm.provider.as_str(), "zhipuai");
    assert_eq!(glm.wire_model_id.as_str(), "glm-5.2");
    assert_eq!(glm.canonical_model, None);
    assert_eq!(glm.endpoint_key, "chat");
    assert_eq!(glm.limits.context_tokens, Some(1_000_000));
    assert_eq!(glm.limits.output_tokens, Some(131_072));
}

#[test]
fn compiler_merges_layers_with_override_precedence() {
    // 合成提供商"acme"的捆绑默认值。
    let bundled = vec![CatalogOffering {
        provider: "acme".into(),
        wire_model_id: "synth-chat-1".into(),
        endpoint_key: "chat".into(),
        default_for_provider: true,
        family: Some("synth".into()),
        source: CatalogSource::Bundled,
        ..Default::default()
    }];
    // 活跃刷新添加新行并重新声明捆绑行并带有成本。
    let live = vec![
        CatalogOffering {
            provider: "acme".into(),
            wire_model_id: "synth-chat-1".into(),
            endpoint_key: "chat".into(),
            cost: Some(ModelsDevCost {
                input: Some(2.0),
                ..Default::default()
            }),
            source: CatalogSource::Live {
                base_url_fingerprint: "fp".into(),
                fetched_at: 100,
            },
            ..Default::default()
        },
        CatalogOffering {
            provider: "acme".into(),
            wire_model_id: "synth-chat-2".into(),
            endpoint_key: "chat".into(),
            source: CatalogSource::Live {
                base_url_fingerprint: "fp".into(),
                fetched_at: 100,
            },
            ..Default::default()
        },
    ];
    // 用户覆盖在 synth-chat-1 上固定了自定义规范模型。
    let overrides = vec![CatalogOffering {
        provider: "acme".into(),
        wire_model_id: "synth-chat-1".into(),
        canonical_model: Some("acme-canonical".into()),
        endpoint_key: "chat".into(),
        source: CatalogSource::UserOverride,
        ..Default::default()
    }];

    let snapshot = CatalogCompiler::new()
        .with_bundled(bundled)
        .with_live(live)
        .with_overrides(overrides)
        .compile();

    // 两个不同的 (provider, wire) 标识在去重后存活。
    assert_eq!(snapshot.offerings.len(), 2);

    let one = find(&snapshot.offerings, "acme", "synth-chat-1");
    // 最高优先级层（覆盖）在标识冲突中胜出。
    assert_eq!(one.source, CatalogSource::UserOverride);
    assert_eq!(one.canonical_model.as_deref(), Some("acme-canonical"));

    let two = find(&snapshot.offerings, "acme", "synth-chat-2");
    assert!(matches!(two.source, CatalogSource::Live { .. }));
}

#[test]
fn cache_scopes_by_provider_and_base_url_fingerprint() {
    let fp_a = base_url_fingerprint("https://api.example.com/v1");
    let fp_b = base_url_fingerprint("https://other.example.com/v1");
    assert_ne!(fp_a, fp_b, "different hosts must not share a fingerprint");

    let mut cache = ProviderCatalogCache::new();
    let row = |id: &str| CatalogOffering {
        provider: "acme".into(),
        wire_model_id: id.into(),
        endpoint_key: "chat".into(),
        ..Default::default()
    };

    // 同一个提供商，两个不同的基 URL。
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp_a.clone(),
            fetched_at: 1_000,
            offerings: vec![row("from-a")],
        },
        3_600,
    );
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp_b.clone(),
            fetched_at: 1_000,
            offerings: vec![row("from-b")],
        },
        3_600,
    );
    // 不同的提供商，与 fp_a 相同的基 URL。
    cache.record_success(
        ProviderCatalogDelta {
            provider: "beta".into(),
            base_url_fingerprint: fp_a.clone(),
            fetched_at: 1_000,
            offerings: vec![row("from-beta")],
        },
        3_600,
    );

    let a = cache.fresh_offerings("acme", &fp_a, 1_100);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].wire_model_id, "from-a");
    // 同一提供商，不同基 URL 不得跨范围泄漏行。
    let b = cache.fresh_offerings("acme", &fp_b, 1_100);
    assert_eq!(b[0].wire_model_id, "from-b");
    // 同一基 URL 上的不同提供商也不得共享行。
    let beta = cache.fresh_offerings("beta", &fp_a, 1_100);
    assert_eq!(beta[0].wire_model_id, "from-beta");
    assert_eq!(cache.entries.len(), 3);
}

#[test]
fn fingerprint_folds_cosmetic_base_url_differences() {
    let canonical = base_url_fingerprint("https://API.Example.com/v1");
    assert_eq!(
        canonical,
        base_url_fingerprint("https://api.example.com/v1/"),
        "trailing slash + host case must not change the cache scope"
    );
    assert_eq!(
        canonical,
        base_url_fingerprint("  https://api.example.com:443/v1  "),
        "default https port + surrounding whitespace must fold away"
    );
    // 路径大小写很重要（提供商可能对路径大小写敏感）。
    assert_ne!(
        canonical,
        base_url_fingerprint("https://api.example.com/V1")
    );

    // 端口剥离是协议感知的：:80 是 http 的默认端口（折叠），但
    // http 上的 :443 是非默认端口，必须与裸 http 保持不同。
    assert_eq!(
        base_url_fingerprint("http://h.example.com:80/v1"),
        base_url_fingerprint("http://h.example.com/v1"),
        "http default port :80 must fold away"
    );
    assert_ne!(
        base_url_fingerprint("http://h.example.com:443/v1"),
        base_url_fingerprint("http://h.example.com/v1"),
        ":443 is not http's default port and must not fold"
    );
}

#[test]
fn ttl_marks_entries_stale_and_excludes_them_from_fresh() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "synth-chat-1".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        100, // ttl
    );

    // TTL 内：新鲜。
    assert_eq!(cache.status("acme", &fp, 1_050), CatalogStatus::Fresh);
    assert_eq!(cache.fresh_offerings("acme", &fp, 1_050).len(), 1);

    // 超过 TTL：过时，并从新鲜提供物中排除。
    match cache.status("acme", &fp, 1_200) {
        CatalogStatus::Stale { age_secs } => assert_eq!(age_secs, 200),
        other => panic!("expected stale, got {other:?}"),
    }
    assert!(cache.fresh_offerings("acme", &fp, 1_200).is_empty());
    // 但行仍然存在于缓存中，用于显式回退显示。
    assert_eq!(cache.get("acme", &fp).unwrap().offerings.len(), 1);
}

#[test]
fn ttl_zero_is_always_stale() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![],
        },
        0,
    );
    assert!(cache.get("acme", &fp).unwrap().is_stale(1_000));
}

#[test]
fn unknown_scope_reports_unknown_status() {
    let cache = ProviderCatalogCache::new();
    let fp = base_url_fingerprint("https://api.example.com");
    assert_eq!(cache.status("acme", &fp, 1_000), CatalogStatus::Unknown);
    assert!(cache.fresh_offerings("acme", &fp, 1_000).is_empty());
}

#[test]
fn refresh_failure_preserves_prior_rows_and_marks_failed() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "synth-chat-1".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        3_600,
    );

    for reason in [
        CatalogRefreshError::Unauthorized,
        CatalogRefreshError::Forbidden,
        CatalogRefreshError::NotFound,
        CatalogRefreshError::RateLimited,
        CatalogRefreshError::InvalidResponse,
        CatalogRefreshError::EmptyList,
        CatalogRefreshError::Network,
    ] {
        cache.record_failure("acme", &fp, reason);
        let entry = cache.get("acme", &fp).expect("entry survives failure");
        // 先前的成功行在失败刷新后仍然可用。
        assert_eq!(entry.offerings.len(), 1, "{reason:?} dropped prior rows");
        assert_eq!(entry.status, CatalogStatus::Failed { reason });
        // fetched_at 不会被失败更新。
        assert_eq!(entry.fetched_at, 1_000);
        // ...但失败的条目不得贡献到新鲜提供物，即使仍在 TTL 窗口内
        //（now=1_100, ttl=3_600）。这些行仅可通过 get() 用于显式回退显示。
        assert!(
            cache.fresh_offerings("acme", &fp, 1_100).is_empty(),
            "{reason:?}: failed entry served fresh offerings within TTL"
        );
        assert!(cache.all_fresh_offerings(1_100).is_empty());
        assert_eq!(
            cache.status("acme", &fp, 1_100),
            CatalogStatus::Failed { reason }
        );
    }
}

#[test]
fn failure_without_prior_creates_observable_empty_entry() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_failure("acme", &fp, CatalogRefreshError::Unauthorized);

    let entry = cache.get("acme", &fp).expect("failure is observable");
    assert!(entry.offerings.is_empty());
    assert_eq!(
        entry.status,
        CatalogStatus::Failed {
            reason: CatalogRefreshError::Unauthorized
        }
    );
}

#[test]
fn record_success_stamps_live_provenance_on_rows() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    // 行到达时被错误标记为 Bundled；摄入必须规范化来源。
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 4_242,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "synth-chat-1".into(),
                endpoint_key: "chat".into(),
                source: CatalogSource::Bundled,
                ..Default::default()
            }],
        },
        3_600,
    );
    let entry = cache.get("acme", &fp).unwrap();
    assert_eq!(
        entry.offerings[0].source,
        CatalogSource::Live {
            base_url_fingerprint: fp,
            fetched_at: 4_242,
        }
    );
}

#[test]
fn cache_serialization_round_trips_and_contains_no_secrets() {
    let fp = base_url_fingerprint("https://api.example.com/v1");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "zhipuai".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_700,
            offerings: bundled_offerings_from_models_dev(&fixture()),
        },
        3_600,
    );

    let json = serde_json::to_string_pretty(&cache).expect("cache serializes");
    let round: ProviderCatalogCache = serde_json::from_str(&json).expect("cache round-trips");
    assert_eq!(round, cache);

    // 持久化的形状携带模型事实，但没有可能持有凭据的字段。
    // 防止未来字段重新引入凭据。
    let lower = json.to_lowercase();
    for needle in [
        "api_key",
        "apikey",
        "api-key",
        "authorization",
        "secret",
        "password",
        "bearer",
        "access_token",
    ] {
        assert!(
            !lower.contains(needle),
            "cache JSON unexpectedly contains `{needle}`"
        );
    }
    // 合理性检查：它确实序列化了有意义的提供商/模型事实。
    assert!(json.contains("glm-5.2"));
    assert!(json.contains("base_url_fingerprint"));
}

#[test]
fn all_fresh_offerings_spans_providers_and_skips_stale() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "fresh-row".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        3_600,
    );
    cache.record_success(
        ProviderCatalogDelta {
            provider: "beta".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 0,
            offerings: vec![CatalogOffering {
                provider: "beta".into(),
                wire_model_id: "stale-row".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        10, // tiny ttl → stale at now=1_100
    );

    let fresh = cache.all_fresh_offerings(1_100);
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].wire_model_id, "fresh-row");

    // #4139：选择器仍然看到过时的行；只有新鲜辅助函数会丢弃它们。
    let visible = cache.all_visible_offerings(1_100);
    assert_eq!(visible.len(), 2);
    assert!(visible.iter().any(|row| row.wire_model_id == "fresh-row"));
    assert!(visible.iter().any(|row| row.wire_model_id == "stale-row"));
}

#[test]
fn snapshot_feeds_route_resolver_offerings() {
    // 编译后的快照投影到 RouteResolver 消费的确切类型中，
    // 证明目录行仅通过提供物接缝到达路由层。
    let snapshot = CatalogCompiler::new().with_models_dev(&fixture()).compile();
    let offerings = snapshot.to_offerings();

    let glm = offerings
        .iter()
        .find(|o| o.provider.as_str() == "zhipuai" && o.wire_model_id.as_str() == "glm-5.2")
        .expect("GLM offering reaches the route resolver seam");
    assert_eq!(glm.limits.context_tokens, Some(1_000_000));
    assert_eq!(glm.limits.output_tokens, Some(131_072));
    // 纯音频行永远不会成为路由提供物。
    assert!(
        !offerings
            .iter()
            .any(|o| o.wire_model_id.as_str() == "glm-voice")
    );
}

// ---------------------------------------------------------------------------
// #3385 / #4188：提交的离线/过时捆绑 Models.dev 资产。
// ---------------------------------------------------------------------------

#[test]
fn bundled_asset_parses() {
    // 提交的资产必须通过 `include_str!` 加载并反序列化为
    // 解析器的 `ModelsDevCatalog` 结构。这是构建时守卫，
    // 确保 `bundled_models_dev_catalog()` 在发布的构建中不会 panic。
    let catalog = ModelsDevCatalog::parse_json(BUNDLED_MODELS_DEV_JSON)
        .expect("committed bundled asset must be valid Models.dev JSON");
    assert!(
        !catalog.providers.is_empty(),
        "bundled asset must carry provider rows"
    );
    // 辅助函数返回相同解析后的目录。
    assert_eq!(bundled_models_dev_catalog(), catalog);
}

#[test]
fn bundled_asset_meta_describes_offline_fallback_not_competing_truth() {
    // #4188：资产必须将自己描述为离线/过时回退，而不是
    // 与活跃 Models.dev 并存的竞争性策划真相来源。
    let raw: serde_json::Value =
        serde_json::from_str(BUNDLED_MODELS_DEV_JSON).expect("bundled JSON");
    let meta = raw
        .get("_meta")
        .and_then(|m| m.as_object())
        .expect("_meta object");
    let role = meta
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        role.to_ascii_lowercase().contains("not a competing"),
        "_meta.role must demote the bundled asset: {role}"
    );
    assert!(
        role.to_ascii_lowercase().contains("live"),
        "_meta.role must point at live Models.dev preference: {role}"
    );
}

#[test]
fn bundled_asset_yields_real_chat_offerings_for_key_models() {
    let rows = bundled_catalog_offerings();
    assert!(
        rows.len() >= 20,
        "expected dozens of bundled chat offerings, got {}",
        rows.len()
    );

    // GLM 和 Kimi 行携带其真实（非默认）上下文窗口，
    // 证明真实事实通过，而不是 `RouteLimits::default()`（未知）。
    let glm = find(&rows, "zai", "GLM-5.2");
    assert_eq!(glm.limit.as_ref().and_then(|l| l.context), Some(1_000_000));
    assert!(glm.default_for_provider);

    let kimi = find(&rows, "moonshot", "kimi-k2.7-code");
    assert_eq!(kimi.limit.as_ref().and_then(|l| l.context), Some(262_144));

    // 音频/TTS 行缺失（资产只提供对话模型，但无论如何断言过滤器契约）。
    assert!(
        rows.iter().all(|r| !r.wire_model_id.contains("tts")),
        "no TTS rows should reach the offering layer"
    );
}

#[test]
fn bundled_asset_pricing_is_honest() {
    let rows = bundled_catalog_offerings();

    // DeepSeek 原生行在此处故意未定价（通过别处的有时效性的
    // DeepSeek 表定价）；如果定价也会破坏路由层的
    // `unpriced_offering_stays_unknown` 不变量。
    let deepseek = find(&rows, "deepseek", "deepseek-v4-pro");
    assert!(
        deepseek.cost.is_none(),
        "DeepSeek-native rows must stay unpriced in the bundled asset"
    );

    // 任何*确实*携带成本的行必须暴露可用的输入/输出费率
    //（诚实规则：没有仅缓存/空的成本对象，后者会在路由层渲染为
    // 没有费率的 Token）。
    for row in &rows {
        if let Some(cost) = row.cost.as_ref() {
            assert!(
                cost.input.is_some() || cost.output.is_some(),
                "{}/{}: priced row must have an input or output rate",
                row.provider,
                row.wire_model_id
            );
        }
    }

    // 抽样定价行与仓库内 USD 表（crates/tui pricing）匹配：
    // GLM-5.1 按 2026-07-09 Z.ai 发布的费率。
    let glm51 = find(&rows, "zai", "glm-5.1");
    let cost = glm51.cost.as_ref().expect("glm-5.1 is priced");
    assert_eq!(cost.input, Some(1.40));
    assert_eq!(cost.output, Some(4.40));
    assert_eq!(cost.cache_read, Some(0.26));
}

#[test]
fn live_offerings_normalize_models_dev_provider_aliases() {
    // 必须映射到 CodeWhale 类型的活跃 Models.dev ID（#4186/#4187）。
    let raw = r#"{
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
        "togetherai": {
          "id": "togetherai",
          "models": {
            "deepseek-ai/DeepSeek-V4-Pro": {
              "id": "deepseek-ai/DeepSeek-V4-Pro",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        },
        "zhipuai": {
          "id": "zhipuai",
          "models": {
            "glm-5.2": {
              "id": "glm-5.2",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        },
        "brand-new-gateway": {
          "id": "brand-new-gateway",
          "models": {
            "x-1": {
              "id": "x-1",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        }
      }
    }"#;
    let catalog = ModelsDevCatalog::parse_json(raw).expect("fixture parses");
    let rows = live_offerings_from_models_dev(&catalog, "fp-models-dev", 1_700);

    assert_eq!(
        find(&rows, "moonshot", "kimi-k2.5").source,
        CatalogSource::Live {
            base_url_fingerprint: "fp-models-dev".into(),
            fetched_at: 1_700,
        }
    );
    find(&rows, "together", "deepseek-ai/DeepSeek-V4-Pro");
    find(&rows, "zai", "glm-5.2");
    // 未知的上游提供商保留其 Models.dev ID。
    find(&rows, "brand-new-gateway", "x-1");
    assert!(rows.iter().all(|r| r.provider != "moonshotai"));
    assert!(rows.iter().all(|r| r.provider != "togetherai"));
    assert!(rows.iter().all(|r| r.provider != "zhipuai"));
}
