//! 活跃 Models.dev 目录获取 + 无密钥的磁盘缓存（#4187）。
//!
//! OpenCode 风格的生产者：
//! - 启动时读取过时/新鲜的磁盘缓存（从不阻塞模型选择），
//! - 在后台以有限超时和显式 User-Agent（无凭据）获取
//!   `https://models.dev/catalog.json`，
//! - 通过临时文件 + 重命名原子写入缓存，
//! - 将解析后的行编译为 [`CatalogOffering`] 并发布到
//!   [`crate::provider_lake`]，
//! - 在任何失败时回退到之前的缓存或捆绑的快照。
//!
//! 覆盖旋钮（测试/自测）：
//! - `CODEWHALE_MODELS_DEV_URL` — 基础 URL（附加 `/catalog.json`）或完整的
//!   `*.json` URL。
//! - `CODEWHALE_MODELS_DEV_PATH` — 本地文件路径；跳过网络。
//! - `CODEWHALE_DISABLE_MODELS_DEV_FETCH` — 当为真值时，从不访问网络。

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

use codewhale_config::catalog::{
    CatalogSnapshot, base_url_fingerprint, live_offerings_from_models_dev, now_unix,
};
use codewhale_config::models_dev::{MODELS_DEV_CATALOG_URL, ModelsDevCatalog};
use codewhale_config::persistence::atomic_write;
use serde::{Deserialize, Serialize};

/// 活跃 Models.dev 快照的默认 TTL（24 小时，#4187 / #4114）。
pub const DEFAULT_MODELS_DEV_TTL_SECS: u64 = 24 * 60 * 60;

/// Models.dev 获取的有限 HTTP 超时。
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// 显式 User-Agent；无凭据，无会话 Cookie。
pub const USER_AGENT: &str = concat!("CodeWhale/", env!("CARGO_PKG_VERSION"), " (+models-dev)");

/// CodeWhale `catalog` 状态目录下的文件名。
pub const CACHE_FILE: &str = "models-dev-catalog.json";

/// 环境变量：覆盖 Models.dev 基础 URL 或完整目录 URL。
pub const ENV_MODELS_DEV_URL: &str = "CODEWHALE_MODELS_DEV_URL";
/// 环境变量：从本地路径加载目录 JSON（跳过网络）。
pub const ENV_MODELS_DEV_PATH: &str = "CODEWHALE_MODELS_DEV_PATH";
/// 环境变量：完全禁用网络获取（`1`/`true`/`yes`/`on`）。
pub const ENV_DISABLE_FETCH: &str = "CODEWHALE_DISABLE_MODELS_DEV_FETCH";

const CACHE_SCHEMA_VERSION: u32 = 1;

/// Models.dev 活跃层的新鲜度/来源，用于 UI 芯片显示（#4187）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelsDevFreshness {
    /// 没有活跃/缓存层；选择器仅看到捆绑行。
    #[default]
    Bundled,
    /// TTL 内的活跃（或磁盘缓存）行。
    Live,
    /// 超过 TTL 的磁盘缓存/之前的活跃行；仍可见。
    Stale,
    /// 上次刷新失败；之前的/捆绑的行仍然可用。
    Failed,
}

/// 供 UI / `/model refresh` 反馈的安静状态快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelsDevStatus {
    pub freshness: ModelsDevFreshness,
    pub offering_count: usize,
    pub fetched_at: Option<u64>,
    pub source_label: String,
    pub last_error: Option<String>,
}

static STATUS: RwLock<ModelsDevStatus> = RwLock::new(ModelsDevStatus {
    freshness: ModelsDevFreshness::Bundled,
    offering_count: 0,
    fetched_at: None,
    source_label: String::new(),
    last_error: None,
});

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedModelsDevCache {
    schema_version: u32,
    /// 载荷获取时的 Unix 秒数（或从覆盖路径加载时）。
    fetched_at: u64,
    /// 用于 [`CatalogSource::Live`] 的源 URL/路径指纹。
    source_fingerprint: String,
    /// 人类可读的源标签（URL 或 `file:…`）；绝非密钥。
    source_label: String,
    /// 原始 Models.dev 目录 JSON 正文（构建时无密钥）。
    body: String,
}

/// Models.dev 刷新未发布新行的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsDevRefreshError {
    Disabled,
    Network(String),
    HttpStatus(u16),
    InvalidResponse(String),
    EmptyCatalog,
    Io(String),
}

impl std::fmt::Display for ModelsDevRefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "Models.dev fetch disabled"),
            Self::Network(msg) => write!(f, "network: {msg}"),
            Self::HttpStatus(code) => write!(f, "HTTP {code}"),
            Self::InvalidResponse(msg) => write!(f, "invalid response: {msg}"),
            Self::EmptyCatalog => write!(f, "empty catalog"),
            Self::Io(msg) => write!(f, "io: {msg}"),
        }
    }
}

/// 解析 CodeWhale `catalog` 状态目录下的磁盘缓存路径。
#[must_use]
pub fn cache_path() -> Option<PathBuf> {
    codewhale_config::resolve_state_dir("catalog")
        .ok()
        .map(|dir| dir.join(CACHE_FILE))
}

/// 当前的安静状态（供 UI / 斜杠命令反馈）。
#[must_use]
pub fn status() -> ModelsDevStatus {
    STATUS.read().map(|guard| guard.clone()).unwrap_or_default()
}

fn set_status(next: ModelsDevStatus) {
    if let Ok(mut guard) = STATUS.write() {
        *guard = next;
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// 从环境变量覆盖或 Models.dev 默认值解析目录 URL。
#[must_use]
pub fn resolve_catalog_url() -> String {
    match std::env::var(ENV_MODELS_DEV_URL) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                MODELS_DEV_CATALOG_URL.to_string()
            } else if trimmed.ends_with(".json") {
                trimmed.to_string()
            } else {
                format!("{}/catalog.json", trimmed.trim_end_matches('/'))
            }
        }
        Err(_) => MODELS_DEV_CATALOG_URL.to_string(),
    }
}

/// 在任何选择器读取之前从磁盘 Models.dev 缓存种子 ProviderLake。
///
/// 缺失/损坏/空的缓存是无操作的——捆绑行仍然可用。
/// 过时的缓存仍然会发布（新鲜度 = Stale），以便离线启动保留
/// 上次已知的活跃行。
pub fn maybe_load_persisted_cache() {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(cache) = load_cache_file(&path) {
        let age = now_unix().saturating_sub(cache.fetched_at);
        let freshness = if age > DEFAULT_MODELS_DEV_TTL_SECS {
            ModelsDevFreshness::Stale
        } else {
            ModelsDevFreshness::Live
        };
        if let Err(err) = publish_from_body(
            &cache.body,
            &cache.source_fingerprint,
            cache.fetched_at,
            &cache.source_label,
            freshness,
        ) {
            tracing::debug!(
                target: "models_dev_live",
                error = %err,
                "persisted Models.dev cache failed to publish; keeping bundled"
            );
        }
    }
}

/// 强制刷新：优先使用 `CODEWHALE_MODELS_DEV_PATH`，否则进行网络获取。
///
/// 成功时，更新磁盘缓存和 ProviderLake。失败时，保留任何
/// 之前的活跃/捆绑行，并记录安静的 Failed 状态。
pub async fn refresh(force_network: bool) -> Result<usize, ModelsDevRefreshError> {
    if let Ok(path) = std::env::var(ENV_MODELS_DEV_PATH) {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return refresh_from_path(Path::new(trimmed)).await;
        }
    }

    if env_truthy(ENV_DISABLE_FETCH) {
        mark_failed(ModelsDevRefreshError::Disabled);
        return Err(ModelsDevRefreshError::Disabled);
    }

    if !force_network {
        let current = status();
        if current.freshness == ModelsDevFreshness::Live
            && current
                .fetched_at
                .is_some_and(|ts| now_unix().saturating_sub(ts) < DEFAULT_MODELS_DEV_TTL_SECS)
        {
            return Ok(current.offering_count);
        }
    }

    let url = resolve_catalog_url();
    let body = match fetch_catalog_body(&url).await {
        Ok(body) => body,
        Err(err) => {
            mark_failed(err.clone());
            return Err(err);
        }
    };
    let fetched_at = now_unix();
    let fingerprint = base_url_fingerprint(&url);
    let count = publish_from_body(
        &body,
        &fingerprint,
        fetched_at,
        &url,
        ModelsDevFreshness::Live,
    )?;
    if let Some(path) = cache_path() {
        let _ = save_cache_file(
            &path,
            &PersistedModelsDevCache {
                schema_version: CACHE_SCHEMA_VERSION,
                fetched_at,
                source_fingerprint: fingerprint,
                source_label: url,
                body,
            },
        );
    }
    Ok(count)
}

/// 尽力而为的后台刷新：永不 panic，永不阻塞调用方。
pub fn spawn_background_refresh() {
    if env_truthy(ENV_DISABLE_FETCH) && std::env::var(ENV_MODELS_DEV_PATH).is_err() {
        return;
    }
    tokio::spawn(async {
        match refresh(false).await {
            Ok(count) => {
                tracing::debug!(
                    target: "models_dev_live",
                    offering_count = count,
                    "Models.dev live catalog refreshed"
                );
            }
            Err(err) => {
                tracing::debug!(
                    target: "models_dev_live",
                    error = %err,
                    "Models.dev live catalog refresh skipped"
                );
            }
        }
    });
}

async fn refresh_from_path(path: &Path) -> Result<usize, ModelsDevRefreshError> {
    let body = match tokio::fs::read_to_string(path).await {
        Ok(body) => body,
        Err(err) => {
            let mapped = ModelsDevRefreshError::Io(err.to_string());
            mark_failed(mapped.clone());
            return Err(mapped);
        }
    };
    let fetched_at = now_unix();
    let label = format!("file:{}", path.display());
    let fingerprint = base_url_fingerprint(&label);
    let count = publish_from_body(
        &body,
        &fingerprint,
        fetched_at,
        &label,
        ModelsDevFreshness::Live,
    )?;
    if let Some(cache) = cache_path() {
        let _ = save_cache_file(
            &cache,
            &PersistedModelsDevCache {
                schema_version: CACHE_SCHEMA_VERSION,
                fetched_at,
                source_fingerprint: fingerprint,
                source_label: label,
                body,
            },
        );
    }
    Ok(count)
}

async fn fetch_catalog_body(url: &str) -> Result<String, ModelsDevRefreshError> {
    let client = crate::tls::reqwest_client_builder()
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| ModelsDevRefreshError::Network(err.to_string()))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| ModelsDevRefreshError::Network(err.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ModelsDevRefreshError::HttpStatus(status.as_u16()));
    }

    response
        .text()
        .await
        .map_err(|err| ModelsDevRefreshError::Network(err.to_string()))
}

fn publish_from_body(
    body: &str,
    fingerprint: &str,
    fetched_at: u64,
    source_label: &str,
    freshness: ModelsDevFreshness,
) -> Result<usize, ModelsDevRefreshError> {
    let catalog = ModelsDevCatalog::parse_json(body).map_err(|err| {
        let mapped = ModelsDevRefreshError::InvalidResponse(err.to_string());
        mark_failed(mapped.clone());
        mapped
    })?;
    let offerings = live_offerings_from_models_dev(&catalog, fingerprint, fetched_at);
    if offerings.is_empty() {
        let err = ModelsDevRefreshError::EmptyCatalog;
        mark_failed(err.clone());
        return Err(err);
    }
    let count = offerings.len();
    crate::provider_lake::set_live_snapshot(CatalogSnapshot { offerings });
    set_status(ModelsDevStatus {
        freshness,
        offering_count: count,
        fetched_at: Some(fetched_at),
        source_label: source_label.to_string(),
        last_error: None,
    });
    Ok(count)
}

fn mark_failed(err: ModelsDevRefreshError) {
    let mut next = status();
    // 保留之前的 offering_count / fetched_at，以便 UI 仍然可以显示最后
    // 的行，但将上次刷新结果与 TTL 过时状态明确区分。
    next.freshness = ModelsDevFreshness::Failed;
    next.last_error = Some(err.to_string());
    set_status(next);
}

fn load_cache_file(path: &Path) -> Option<PersistedModelsDevCache> {
    let bytes = std::fs::read(path).ok()?;
    let cache: PersistedModelsDevCache = serde_json::from_slice(&bytes).ok()?;
    if cache.schema_version != CACHE_SCHEMA_VERSION {
        return None;
    }
    if cache.body.trim().is_empty() {
        return None;
    }
    Some(cache)
}

fn save_cache_file(
    path: &Path,
    cache: &PersistedModelsDevCache,
) -> Result<(), ModelsDevRefreshError> {
    let bytes =
        serde_json::to_vec(cache).map_err(|err| ModelsDevRefreshError::Io(err.to_string()))?;
    atomic_write(path, &bytes).map_err(|err| ModelsDevRefreshError::Io(err.to_string()))
}

/// 为单元测试公开的编译辅助函数：正文 → 带有规范化提供者 ID 的活跃提供物。
#[cfg(test)]
pub(crate) fn offerings_from_json_for_test(
    body: &str,
) -> Result<Vec<codewhale_config::catalog::CatalogOffering>, String> {
    let catalog = ModelsDevCatalog::parse_json(body).map_err(|e| e.to_string())?;
    Ok(live_offerings_from_models_dev(
        &catalog,
        "test-fp",
        1_700_000_000,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiProvider;
    use crate::provider_lake::{all_catalog_models_for_provider, clear_live_snapshot};
    use crate::test_support::{EnvVarGuard, lock_test_env};
    use codewhale_config::catalog::CatalogSource;

    const FIXTURE: &str = r#"{
      "models": {},
      "providers": {
        "togetherai": {
          "id": "togetherai",
          "models": {
            "deepseek-ai/DeepSeek-V4-Pro": {
              "id": "deepseek-ai/DeepSeek-V4-Pro",
              "name": "DeepSeek V4 Pro",
              "modalities": { "input": ["text"], "output": ["text"] },
              "limit": { "context": 128000, "output": 8192 }
            }
          }
        },
        "moonshotai": {
          "id": "moonshotai",
          "models": {
            "kimi-k2.5": {
              "id": "kimi-k2.5",
              "name": "Kimi K2.5",
              "modalities": { "input": ["text"], "output": ["text"] },
              "limit": { "context": 256000, "output": 8192 }
            }
          }
        },
        "unknown-gateway": {
          "id": "unknown-gateway",
          "models": {
            "mystery-1": {
              "id": "mystery-1",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        }
      }
    }"#;

    #[test]
    fn resolve_catalog_url_defaults_and_overrides() {
        let _lock = lock_test_env();
        let _url = EnvVarGuard::remove(ENV_MODELS_DEV_URL);
        assert_eq!(resolve_catalog_url(), MODELS_DEV_CATALOG_URL);

        let _url = EnvVarGuard::set(ENV_MODELS_DEV_URL, "https://example.test");
        assert_eq!(resolve_catalog_url(), "https://example.test/catalog.json");

        let _url = EnvVarGuard::set(ENV_MODELS_DEV_URL, "https://example.test/api.json");
        assert_eq!(resolve_catalog_url(), "https://example.test/api.json");
    }

    #[test]
    fn live_offerings_normalize_models_dev_provider_ids() {
        let rows = offerings_from_json_for_test(FIXTURE).expect("fixture");
        let providers: Vec<_> = rows.iter().map(|r| r.provider.as_str()).collect();
        assert!(providers.contains(&"together"));
        assert!(providers.contains(&"moonshot"));
        assert!(providers.contains(&"unknown-gateway"));
        assert!(!providers.contains(&"togetherai"));
        assert!(!providers.contains(&"moonshotai"));
        assert!(
            rows.iter()
                .all(|r| matches!(r.source, CatalogSource::Live { .. }))
        );
    }

    #[test]
    fn publish_from_path_updates_provider_lake() {
        let _lock = lock_test_env();
        clear_live_snapshot();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, FIXTURE).expect("write fixture");

        let _home = EnvVarGuard::set("CODEWHALE_HOME", dir.path().join("home"));
        let _disable = EnvVarGuard::set(ENV_DISABLE_FETCH, "1");
        let _path = EnvVarGuard::set(ENV_MODELS_DEV_PATH, &path);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let count = rt.block_on(refresh(true)).expect("refresh from path");
        assert!(count >= 2);

        let together = all_catalog_models_for_provider(ApiProvider::Together);
        assert!(
            together.iter().any(|m| m == "deepseek-ai/DeepSeek-V4-Pro"),
            "Together lake missing live Models.dev row: {together:?}"
        );
        let moonshot = all_catalog_models_for_provider(ApiProvider::Moonshot);
        assert!(
            moonshot.iter().any(|m| m == "kimi-k2.5"),
            "Moonshot lake missing live Models.dev row: {moonshot:?}"
        );

        let st = status();
        assert_eq!(st.freshness, ModelsDevFreshness::Live);
        assert!(st.last_error.is_none());
        assert!(st.offering_count >= 2);

        // 缓存文件应存在且不含密钥。
        let cache = cache_path().expect("cache path");
        assert!(cache.exists());
        let on_disk = std::fs::read_to_string(&cache).expect("read cache");
        let lowered = on_disk.to_lowercase();
        for needle in ["api_key", "authorization", "bearer", "password"] {
            assert!(
                !lowered.contains(&format!("\"{needle}\"")),
                "cache must not persist `{needle}`"
            );
        }

        clear_live_snapshot();
    }

    #[test]
    fn invalid_json_keeps_bundled_and_marks_failed() {
        let _lock = lock_test_env();
        clear_live_snapshot();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{not-json").expect("write");

        let _home = EnvVarGuard::set("CODEWHALE_HOME", dir.path().join("home"));
        let _path = EnvVarGuard::set(ENV_MODELS_DEV_PATH, &path);

        let before = all_catalog_models_for_provider(ApiProvider::Together);
        assert!(!before.is_empty(), "bundled Together rows required");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let err = rt.block_on(refresh(true)).expect_err("bad json");
        assert!(matches!(err, ModelsDevRefreshError::InvalidResponse(_)));

        let after = all_catalog_models_for_provider(ApiProvider::Together);
        assert_eq!(after, before, "bundled rows must survive parse failure");
        let st = status();
        assert_eq!(st.freshness, ModelsDevFreshness::Failed);
        assert!(st.last_error.is_some());
        clear_live_snapshot();
    }

    #[test]
    fn stale_disk_cache_still_publishes() {
        let _lock = lock_test_env();
        clear_live_snapshot();
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", &home);

        let cache_dir = home.join("catalog");
        std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cache = cache_dir.join(CACHE_FILE);
        let stale = PersistedModelsDevCache {
            schema_version: CACHE_SCHEMA_VERSION,
            fetched_at: 1, // 很远的过去 → 过时
            source_fingerprint: "stale-fp".into(),
            source_label: "https://models.dev/catalog.json".into(),
            body: FIXTURE.into(),
        };
        save_cache_file(&cache, &stale).expect("save");

        maybe_load_persisted_cache();
        let st = status();
        assert_eq!(st.freshness, ModelsDevFreshness::Stale);
        assert!(st.offering_count >= 2);
        let together = all_catalog_models_for_provider(ApiProvider::Together);
        assert!(together.iter().any(|m| m == "deepseek-ai/DeepSeek-V4-Pro"));
        clear_live_snapshot();
    }

    #[test]
    fn network_failure_keeps_prior_rows_and_marks_failed() {
        let _lock = lock_test_env();
        clear_live_snapshot();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("catalog.json");
        std::fs::write(&path, FIXTURE).expect("write");

        let _home = EnvVarGuard::set("CODEWHALE_HOME", dir.path().join("home"));
        let _path = EnvVarGuard::set(ENV_MODELS_DEV_PATH, &path);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let count = rt.block_on(refresh(true)).expect("seed from path");
        assert!(count >= 2);

        // 指向一个不可达的 URL 并强制网络（清除路径覆盖）。
        let _path = EnvVarGuard::remove(ENV_MODELS_DEV_PATH);
        let _disable = EnvVarGuard::remove(ENV_DISABLE_FETCH);
        let _url = EnvVarGuard::set(ENV_MODELS_DEV_URL, "http://127.0.0.1:1");

        let err = rt.block_on(refresh(true)).expect_err("dead URL");
        assert!(matches!(err, ModelsDevRefreshError::Network(_)));

        let together = all_catalog_models_for_provider(ApiProvider::Together);
        assert!(
            together.iter().any(|m| m == "deepseek-ai/DeepSeek-V4-Pro"),
            "prior live rows must survive network failure"
        );
        let st = status();
        assert_eq!(st.freshness, ModelsDevFreshness::Failed);
        assert!(st.last_error.is_some());
        assert!(
            st.offering_count >= 2,
            "status should retain prior live row count after failure"
        );
        clear_live_snapshot();
    }
}
