//! 由多个提供商支持的网页搜索工具：Bing HTML 抓取、DuckDuckGo
//!（Bing 后备的 HTML 抓取）、Tavily API、Bocha（博查）API、
//! Metaso API（<https://metaso.cn>）、SearXNG JSON API、百度 AI 搜索、
//! 火山引擎 Ark 和 Sofya（<https://sofya.co>）。
//!
//! 这是代理的主要网页搜索接口。对于浏览工作流
//!（打开页面、点击、截图），请改用直接 URL 方式。
//!
//! 在 config.toml 中设置 `[search]` 以切换提供商：
//!   provider = "duckduckgo"  # 或 tavily/bocha/metaso/searxng/baidu/volcengine/sofya
//!   base_url = "https://search.example/"  # DDG 兼容 URL 或 SearXNG 实例
//!   api_key = "tvly-..."

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_u64,
};
use crate::config::SearchProvider;
use crate::network_policy::{Decision, NetworkPolicyDecider};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::Duration;

const DUCKDUCKGO_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const BING_HOST: &str = "www.bing.com";
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";
const BOCHA_ENDPOINT: &str = "https://api.bochaai.com/v1/web-search";
const METASO_ENDPOINT: &str = "https://metaso.cn/api/v1";
const BAIDU_ENDPOINT: &str = "https://qianfan.baidubce.com/v2/ai_search/web_search";
const VOLCENGINE_RESPONSES_ENDPOINT: &str = "https://ark.cn-beijing.volces.com/api/v3/responses";
const SOFYA_ENDPOINT: &str = "https://sofya.co/v1/search";
/// Metaso 为开源/社区使用提供的公开默认密钥。
/// 在配置和环境变量之后的最後手段。限速约 100 次搜索/天。
const METASO_DEFAULT_API_KEY: &str = "mk-E384C1DD5E8501BB7EFE27C949AFDE5B";
const ERROR_BODY_PREVIEW_BYTES: usize = 512;

/// 如果策略允许调用则返回 `Ok(())`，否则返回 `ToolError`。
/// 当未附加策略时静默通过（向后兼容）。
fn check_policy(decider: Option<&NetworkPolicyDecider>, host: &str) -> Result<(), ToolError> {
    let Some(decider) = decider else {
        return Ok(());
    };
    match decider.evaluate(host, "web_search") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(format!(
            "web search to '{host}' blocked by network policy"
        ))),
        Decision::Prompt => Err(ToolError::permission_denied(format!(
            "web search to '{host}' requires approval; \
             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
        ))),
    }
}

// 用于 HTML 解析的缓存正则表达式模式
static TITLE_RE: OnceLock<Regex> = OnceLock::new();
static SNIPPET_RE: OnceLock<Regex> = OnceLock::new();
static TAG_RE: OnceLock<Regex> = OnceLock::new();
static BING_RESULT_RE: OnceLock<Regex> = OnceLock::new();
static BING_TITLE_RE: OnceLock<Regex> = OnceLock::new();
static BING_SNIPPET_RE: OnceLock<Regex> = OnceLock::new();
static BEARER_TOKEN_RE: OnceLock<Regex> = OnceLock::new();

fn get_title_re() -> &'static Regex {
    TITLE_RE.get_or_init(|| {
        Regex::new(r#"<a[^>]*class=\"result__a\"[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("title regex pattern is valid")
    })
}

fn get_snippet_re() -> &'static Regex {
    SNIPPET_RE.get_or_init(|| {
        Regex::new(
            r#"<a[^>]*class=\"result__snippet\"[^>]*>(.*?)</a>|<div[^>]*class=\"result__snippet\"[^>]*>(.*?)</div>"#,
        )
        .expect("snippet regex pattern is valid")
    })
}

fn get_tag_re() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("tag regex pattern is valid"))
}

fn get_bing_result_re() -> &'static Regex {
    BING_RESULT_RE.get_or_init(|| {
        Regex::new(r#"(?is)<li[^>]*class=\"[^\"]*\bb_algo\b[^\"]*\"[^>]*>(.*?)</li>"#)
            .expect("bing result regex pattern is valid")
    })
}

fn get_bing_title_re() -> &'static Regex {
    BING_TITLE_RE.get_or_init(|| {
        Regex::new(r#"(?is)<h2[^>]*>.*?<a[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("bing title regex pattern is valid")
    })
}

fn get_bing_snippet_re() -> &'static Regex {
    BING_SNIPPET_RE.get_or_init(|| {
        Regex::new(r#"(?is)<div[^>]*class=\"[^\"]*\bb_caption\b[^\"]*\"[^>]*>.*?<p[^>]*>(.*?)</p>"#)
            .expect("bing snippet regex pattern is valid")
    })
}

fn get_bearer_token_re() -> &'static Regex {
    BEARER_TOKEN_RE.get_or_init(|| {
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+")
            .expect("bearer token regex pattern is valid")
    })
}

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS: usize = 10;
const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

#[derive(Debug, Clone, Serialize)]
struct WebSearchEntry {
    title: String,
    url: String,
    snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WebSearchResponse {
    query: String,
    source: String,
    count: usize,
    message: String,
    results: Vec<WebSearchEntry>,
}

pub struct WebSearchTool;

#[async_trait]
impl ToolSpec for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web and return ranked results with URLs and snippets. Default backend is DuckDuckGo with Bing fallback; set `[search] provider = \"bing\" | \"tavily\" | \"bocha\" | \"metaso\" | \"searxng\" | \"baidu\" | \"volcengine\" | \"sofya\"` in config.toml to switch backends, or `[search] base_url` for a DuckDuckGo-compatible endpoint or trusted SearXNG instance. Use this instead of scraping search engines with `curl` in `exec_shell`. For a known canonical URL, prefer `fetch_url` directly."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Compatibility aliases: q, or search_query[0].q."
                },
                "q": {
                    "type": "string",
                    "description": "Search query."
                },
                "search_query": {
                    "type": "array",
                    "description": "Array form for advanced queries: [{\"q\":\"...\", \"max_results\": 5}]",
                    "items": {
                        "type": "object",
                        "properties": {
                            "q": { "type": "string" },
                            "query": { "type": "string" },
                            "max_results": { "type": "integer" }
                        }
                    }
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5, max: 10)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 15000, max: 60000)"
                }
            }
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = extract_search_query(&input)?;
        if query.is_empty() {
            return Err(ToolError::invalid_input("Query cannot be empty"));
        }
        let max_results =
            usize::try_from(optional_search_max_results(&input)).unwrap_or(DEFAULT_MAX_RESULTS);
        let max_results = max_results.clamp(1, MAX_RESULTS);
        let timeout_ms = optional_u64(&input, "timeout_ms", DEFAULT_TIMEOUT_MS).min(60_000);

        if configured_search_base_url(context.search_base_url.as_deref()).is_some()
            && !matches!(
                context.search_provider,
                SearchProvider::DuckDuckGo | SearchProvider::Searxng
            )
        {
            return Err(ToolError::invalid_input(format!(
                "[search].base_url is only supported with provider = \"duckduckgo\" or \"searxng\"; current provider is \"{}\"",
                context.search_provider.as_str()
            )));
        }

        // 在构建 Bing/DuckDuckGo 使用的 HTML 抓取客户端之前，
        // 分发到已配置的基于 API 的搜索提供商。
        match context.search_provider {
            SearchProvider::Tavily => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "api.tavily.com")?;
                return self
                    .run_tavily_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Bocha => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "api.bochaai.com")?;
                return self
                    .run_bocha_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Metaso => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "metaso.cn")?;
                return self
                    .run_metaso_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Searxng => {
                return self
                    .run_searxng_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Baidu => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "qianfan.baidubce.com")?;
                return self
                    .run_baidu_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Volcengine => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "ark.cn-beijing.volces.com")?;
                return self
                    .run_volcengine_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Sofya => {
                let decider = context.network_policy.as_ref();
                check_policy(decider, "sofya.co")?;
                return self
                    .run_sofya_search(&query, max_results, timeout_ms, context)
                    .await;
            }
            SearchProvider::Bing | SearchProvider::DuckDuckGo => {}
        }

        let decider = context.network_policy.as_ref();
        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        // 记录 Bing 是否已尝试并返回零结果，以便在结果消息中显示后备信息（#2130）。
        let mut bing_was_empty = false;

        if matches!(context.search_provider, SearchProvider::Bing) {
            check_policy(decider, BING_HOST)?;
            let results = run_bing_search(&client, &query, max_results).await?;
            if !results.is_empty() {
                return search_tool_result(query, "bing", results, None);
            }
            // Bing 返回了零结果——回退到 DuckDuckGo。
            bing_was_empty = true;
        }

        // 按域名的网络策略门控（#135）。网页搜索的"host"是
        // 上游搜索引擎域名——优先 DuckDuckGo 兼容端点，
        // Bing 作为后备。我们在此门控已配置的端点；Bing
        // 在后备路径中单独门控，这样对一个引擎的拒绝
        // 不会静默允许另一个。
        let (url, duckduckgo_host) =
            duckduckgo_search_url(context.search_base_url.as_deref(), &query)?;
        let allow_bing_fallback =
            duckduckgo_allows_bing_fallback(context.search_base_url.as_deref());
        check_policy(decider, &duckduckgo_host)?;

        let resp = client
            .get(&url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Web search request failed: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            return Err(ToolError::execution_failed(format!(
                "Web search failed: HTTP {}",
                status.as_u16()
            )));
        }

        let mut results = parse_duckduckgo_results(&body, max_results);
        let mut source = if allow_bing_fallback {
            "duckduckgo".to_string()
        } else {
            duckduckgo_host.clone()
        };
        let mut message_suffix: Option<&str> = None;

        // 当 Bing 返回零结果且我们回退到 DuckDuckGo 时，
        // 在结果消息中显示后备信息（#2130）。
        if bing_was_empty && !results.is_empty() {
            message_suffix = Some("Bing returned no results; used DuckDuckGo fallback");
        }

        let duckduckgo_blocked = is_duckduckgo_challenge(&body);
        if results.is_empty() && duckduckgo_blocked && !allow_bing_fallback {
            return Err(ToolError::execution_failed(format!(
                "DuckDuckGo-compatible search endpoint at {duckduckgo_host} returned a bot challenge; check the private search service, credentials, or network policy"
            )));
        }

        if results.is_empty() && allow_bing_fallback {
            // Bing 是独立的主机——单独门控，这样 DuckDuckGo 的拒绝
            // 不会静默放行 Bing（反之亦然）。
            check_policy(decider, BING_HOST)?;
            match run_bing_search(&client, &query, max_results).await {
                Ok(fallback_results) if !fallback_results.is_empty() => {
                    results = fallback_results;
                    source = "bing".to_string();
                    message_suffix = Some(if duckduckgo_blocked {
                        "DuckDuckGo returned a bot challenge; used Bing fallback"
                    } else {
                        "DuckDuckGo returned no parseable results; used Bing fallback"
                    });
                }
                Ok(_) if duckduckgo_blocked => {
                    return Err(ToolError::execution_failed(
                        "DuckDuckGo returned a bot challenge and Bing fallback returned no results",
                    ));
                }
                Err(err) if duckduckgo_blocked => {
                    return Err(ToolError::execution_failed(format!(
                        "DuckDuckGo returned a bot challenge and Bing fallback failed: {err}"
                    )));
                }
                Ok(_) | Err(_) => {}
            }
        }

        search_tool_result(query, source, results, message_suffix)
    }
}

fn search_tool_result(
    query: String,
    source: impl Into<String>,
    results: Vec<WebSearchEntry>,
    message_suffix: Option<&str>,
) -> Result<ToolResult, ToolError> {
    let message = if results.is_empty() {
        if let Some(suffix) = message_suffix {
            format!("No results found. {suffix}")
        } else {
            "No results found".to_string()
        }
    } else if let Some(suffix) = message_suffix {
        format!("Found {} result(s). {suffix}", results.len())
    } else {
        format!("Found {} result(s)", results.len())
    };

    let response = WebSearchResponse {
        query,
        source: source.into(),
        count: results.len(),
        message,
        results,
    };

    ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
}

impl WebSearchTool {
    /// 通过配置的 SearXNG JSON API 搜索。
    ///
    /// SearXNG 暴露 `/search?q=...&format=json`，但公共实例通常
    /// 禁用 JSON 输出或对自动化进行限速。因此 CodeWhale 仅使用
    /// 在 `[search] base_url` 中配置的可信实例。
    async fn run_searxng_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let (url, host) = searxng_search_url(context.search_base_url.as_deref(), query)?;
        check_policy(context.network_policy.as_ref(), &host)?;

        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("SearXNG search request to {host} failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read SearXNG response from {host}: {e}"))
        })?;

        if !status.is_success() {
            let truncated = truncate_error_body(&body);
            let msg = match status.as_u16() {
                403 => format!(
                    "SearXNG search failed: HTTP 403 from {host}. Check that JSON output is enabled and this instance permits API access. {truncated}"
                ),
                429 => format!(
                    "SearXNG search failed: HTTP 429 from {host}. The configured instance is rate-limiting requests; use a trusted/self-hosted instance or retry later. {truncated}"
                ),
                code => format!("SearXNG search failed: HTTP {code} from {host}. {truncated}"),
            };
            return Err(ToolError::execution_failed(msg));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to parse SearXNG JSON response from {host}: {e}. Ensure the instance supports format=json and JSON output is enabled."
            ))
        })?;

        let results = parse_searxng_results(&parsed, max_results);
        let suffix = format!("Backend: searxng at {host}");
        search_tool_result(query.to_string(), "searxng", results, Some(&suffix))
    }

    /// 通过 Tavily AI 搜索 API（<https://tavily.com>）搜索。
    async fn run_tavily_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let api_key = context
            .search_api_key
            .as_deref()
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Tavily search requires an API key. Set `[search] api_key = \"tvly-...\"` in config.toml.",
                )
            })?;

        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = json!({
            "api_key": api_key, // noqa: body 中的 API 密钥
            "query": query,
            "search_depth": "basic",
            "max_results": max_results,
        });

        let resp = client
            .post(TAVILY_ENDPOINT)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Tavily search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Tavily response: {e}"))
        })?;

        if !status.is_success() {
            let truncated = truncate_error_body(&body);
            return Err(ToolError::execution_failed(format!(
                "Tavily search failed: HTTP {} — {truncated}",
                status.as_u16()
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Tavily response: {e}"))
        })?;

        let results: Vec<WebSearchEntry> = parsed
            .get("results")
            .and_then(|v| v.as_array())
            .into_iter()
            .flat_map(|arr| arr.iter())
            .filter_map(|item| {
                let title = item.get("title")?.as_str()?.to_string();
                let url = item.get("url")?.as_str()?.to_string();
                let snippet = item
                    .get("content")
                    .or_else(|| item.get("snippet"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                Some(WebSearchEntry {
                    title,
                    url,
                    snippet,
                })
            })
            .take(max_results)
            .collect();

        let message = if results.is_empty() {
            "No results found".to_string()
        } else {
            format!("Found {} result(s)", results.len())
        };

        let response = WebSearchResponse {
            query: query.to_string(),
            source: "tavily".to_string(),
            count: results.len(),
            message,
            results,
        };

        ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    /// 通过 Sofya 网页搜索 API（<https://sofya.co>）搜索。
    ///
    /// Sofya 返回完整的提取页面内容而非摘要。API 密钥（`ay_live_...`）
    /// 来自 `[search] api_key`，回退到 `SOFYA_API_KEY` 环境变量，
    /// 并以 `Bearer` 令牌形式发送。
    async fn run_sofya_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let env_key = std::env::var("SOFYA_API_KEY").ok();
        let api_key = context
            .search_api_key
            .as_deref()
            .or(env_key.as_deref())
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Sofya search requires an API key. Set `[search] api_key = \"ay_live_...\"` in config.toml or the SOFYA_API_KEY env var.",
                )
            })?;

        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = json!({
            "query": query,
            "max_results": max_results,
        });

        let resp = client
            .post(SOFYA_ENDPOINT)
            .header("Content-Type", "application/json")
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Sofya search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Sofya response: {e}"))
        })?;

        if !status.is_success() {
            let truncated = truncate_error_body(&body);
            return Err(ToolError::execution_failed(format!(
                "Sofya search failed: HTTP {} — {truncated}",
                status.as_u16()
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Sofya response: {e}"))
        })?;

        let results = parse_sofya_results(&parsed, max_results);

        let message = if results.is_empty() {
            "No results found".to_string()
        } else {
            format!("Found {} result(s)", results.len())
        };

        let response = WebSearchResponse {
            query: query.to_string(),
            source: "sofya".to_string(),
            count: results.len(),
            message,
            results,
        };

        ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    /// 通过博查 AI 搜索 API（<https://bochaai.com>）搜索。
    async fn run_bocha_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let api_key = context
            .search_api_key
            .as_deref()
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Bocha search requires an API key. Set `[search] api_key = \"sk-...\"` in config.toml.",
                )
            })?;

        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = json!({
            "query": query,
            "freshness": "noLimit",
            "count": max_results,
        });

        let resp = client
            .post(BOCHA_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Bocha search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Bocha response: {e}"))
        })?;

        if !status.is_success() {
            let truncated = truncate_error_body(&body);
            return Err(ToolError::execution_failed(format!(
                "Bocha search failed: HTTP {} — {truncated}",
                status.as_u16()
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Bocha response: {e}"))
        })?;

        if let Some(error) = bocha_error_message(&parsed) {
            return Err(ToolError::execution_failed(error));
        }

        let results = parse_bocha_results(&parsed, max_results);

        let message = if results.is_empty() {
            "No results found".to_string()
        } else {
            format!("Found {} result(s)", results.len())
        };

        let response = WebSearchResponse {
            query: query.to_string(),
            source: "bocha".to_string(),
            count: results.len(),
            message,
            results,
        };

        ToolResult::json(&response).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    /// 通过 Metaso AI 搜索 API（<https://metaso.cn>）搜索。如果没有设置配置密钥，
    /// 则回退到 `METASO_API_KEY` 环境变量，然后是内置默认密钥。
    async fn run_metaso_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let env_key = std::env::var("METASO_API_KEY").ok();
        let api_key = context
            .search_api_key
            .as_deref()
            .or(env_key.as_deref())
            .unwrap_or(METASO_DEFAULT_API_KEY);

        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let size = max_results.clamp(1, 100);
        let payload = json!({
            "q": query,
            "scope": "webpage",
            "size": size,
        });

        let resp = client
            .post(format!("{METASO_ENDPOINT}/search"))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Metaso search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Metaso response: {e}"))
        })?;

        if !status.is_success() {
            let msg = match status.as_u16() {
                401 | 403 => "Metaso API key rejected — check METASO_API_KEY or set `[search] api_key` in config.toml, or get one at https://metaso.cn/search-api/playground".to_string(),
                429 => "Metaso rate-limited — wait and retry, or get your own API key at https://metaso.cn/search-api/playground".to_string(),
                _ => {
                    let truncated = truncate_error_body(&body);
                    format!("Metaso server error (HTTP {status}) — {truncated}")
                }
            };
            return Err(ToolError::execution_failed(msg));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Metaso response: {e}"))
        })?;

        // 检查响应体中的业务逻辑错误码。
        if let Some(code) = parsed.get("code").and_then(|v| v.as_i64())
            && code != 0
        {
            let msg = parsed
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(ToolError::execution_failed(match code {
                3003 => "Metaso: daily search limit reached — set METASO_API_KEY or get one at https://metaso.cn/search-api/playground".to_string(),
                2005 => "Metaso API key rejected — check METASO_API_KEY or set `[search] api_key` in config.toml".to_string(),
                _ => format!("Metaso API error (code {code}: {msg})"),
            }));
        }

        let results: Vec<WebSearchEntry> = parsed
            .get("webpages")
            .and_then(|v| v.as_array())
            .into_iter()
            .flat_map(|arr| arr.iter())
            .filter_map(|item| {
                let title = item.get("title")?.as_str()?.to_string();
                let url = item.get("link")?.as_str()?.to_string();
                let snippet = item
                    .get("snippet")
                    .or_else(|| item.get("summary"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
                Some(WebSearchEntry {
                    title,
                    url,
                    snippet,
                })
            })
            .take(size)
            .collect();

        search_tool_result(query.to_string(), "metaso", results, None)
    }

    /// 通过百度 AI 搜索 API（<https://qianfan.baidubce.com>）搜索。
    async fn run_baidu_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let env_key = std::env::var("BAIDU_SEARCH_API_KEY").ok();
        let api_key = context
            .search_api_key
            .as_deref()
            .or(env_key.as_deref())
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Baidu search requires an API key. Set `BAIDU_SEARCH_API_KEY` or `[search] api_key` in config.toml.",
                )
            })?;

        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = baidu_search_payload(query, max_results);

        let resp = client
            .post(BAIDU_ENDPOINT)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ToolError::execution_failed(format!("Baidu search request failed: {e}"))
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("Failed to read Baidu response: {e}"))
        })?;

        if !status.is_success() {
            let msg = match status.as_u16() {
                401 | 403 => "Baidu search API key rejected — check BAIDU_SEARCH_API_KEY or `[search] api_key` in config.toml".to_string(),
                429 => "Baidu search rate-limited — wait and retry, or check your Baidu AI Search quota".to_string(),
                _ => {
                    let truncated = truncate_error_body(&body);
                    format!("Baidu search failed: HTTP {} — {truncated}", status.as_u16())
                }
            };
            return Err(ToolError::execution_failed(msg));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse Baidu response: {e}"))
        })?;

        if let Some(error) = baidu_error_message(&parsed) {
            return Err(ToolError::execution_failed(error));
        }

        let results = parse_baidu_results(&parsed, max_results);
        search_tool_result(query.to_string(), "baidu", results, None)
    }

    /// 通过火山引擎 Ark Responses API 的 web_search 工具搜索。
    /// 使用严格的 JSON 提示约束从模型的搜索增强响应中提取结构化结果。
    ///
    /// 将用户提供的超时覆盖为至少 90 秒，因为
    /// Responses API 管道（网页搜索→模型推理→JSON 生成）
    /// 本质上比简单的搜索 API 往返更慢。单独的 15 秒
    /// `connect_timeout` 让 DNS/TLS 故障快速暴露。
    /// 瞬态传输错误将使用指数退避重试两次。
    async fn run_volcengine_search(
        &self,
        query: &str,
        max_results: usize,
        timeout_ms: u64,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let volc_key = std::env::var("VOLCENGINE_API_KEY").ok();
        let volc_ark_key = std::env::var("VOLCENGINE_ARK_API_KEY").ok();
        let ark_key = std::env::var("ARK_API_KEY").ok();
        let api_key = context
            .search_api_key
            .as_deref()
            .or(volc_key.as_deref())
            .or(volc_ark_key.as_deref())
            .or(ark_key.as_deref())
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "Volcengine search requires an API key. Set `[search] api_key`, \
                     or VOLCENGINE_API_KEY / VOLCENGINE_ARK_API_KEY / ARK_API_KEY env var.",
                )
            })?;

        // 火山引擎 Responses API 管道（搜索+模型推理）速度较慢，
        // 因此强制至少 90 秒。仅在调用方值超过 90_000 ms 时才使用调用方的值。
        let effective_timeout = timeout_ms.max(90_000);

        let client = crate::tls::reqwest_client_builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_millis(effective_timeout))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Some(Duration::from_secs(15)))
            .http2_keep_alive_timeout(Duration::from_secs(20))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| {
                ToolError::execution_failed(format!("Failed to build HTTP client: {e}"))
            })?;

        let payload = volcengine_search_payload(query, max_results);

        // 重试瞬态传输错误（DNS、连接重置、超时）
        // 最多 2 次，使用指数退避：1 秒、2 秒。
        let mut last_err: Option<ToolError> = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1000 * (1 << (attempt - 1)))).await;
            }

            match client
                .post(VOLCENGINE_RESPONSES_ENDPOINT)
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.map_err(|e| {
                        ToolError::execution_failed(format!(
                            "Failed to read Volcengine response: {e}"
                        ))
                    })?;

                    if !status.is_success() {
                        let msg = match status.as_u16() {
                            401 | 403 => "Volcengine API key rejected — check `[search] api_key` in config.toml or VOLCENGINE_API_KEY / VOLCENGINE_ARK_API_KEY / ARK_API_KEY".to_string(),
                            429 => "Volcengine API rate-limited — wait and retry, or check your quota".to_string(),
                            _ => {
                                let truncated = truncate_error_body(&body);
                                format!("Volcengine search failed: HTTP {} — {truncated}", status.as_u16())
                            }
                        };
                        return Err(ToolError::execution_failed(msg));
                    }

                    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                        ToolError::execution_failed(format!(
                            "Failed to parse Volcengine response: {e}"
                        ))
                    })?;

                    if let Some(error) = volcengine_error_message(&parsed) {
                        return Err(ToolError::execution_failed(error));
                    }

                    let response_text = volcengine_extract_text(&parsed).ok_or_else(|| {
                        ToolError::execution_failed("Volcengine response contains no output text")
                    })?;

                    let results = parse_volcengine_results(&response_text, max_results);
                    return search_tool_result(query.to_string(), "volcengine", results, None);
                }
                Err(e) => {
                    let is_transient = e.is_timeout() || e.is_connect();
                    if !is_transient || attempt == 2 {
                        return Err(ToolError::execution_failed(format!(
                            "Volcengine search request failed: {e}"
                        )));
                    }
                    last_err = Some(ToolError::execution_failed(format!(
                        "Volcengine search request failed (attempt {}/3): {e}",
                        attempt + 1
                    )));
                }
            }
        }

        // 不可达——最后一次迭代总是会从上方返回。
        Err(last_err.unwrap_or_else(|| {
            ToolError::execution_failed("Volcengine search: unexpected retry exit")
        }))
    }
}

fn truncate_error_body(body: &str) -> String {
    let stripped = sanitize_error_body(body);
    if stripped.len() <= ERROR_BODY_PREVIEW_BYTES {
        stripped
    } else {
        let mut end = ERROR_BODY_PREVIEW_BYTES;
        while !stripped.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &stripped[..end])
    }
}

fn sanitize_error_body(body: &str) -> String {
    let stripped = strip_html_tags(body);
    let visible: String = stripped
        .chars()
        .filter(|c| !c.is_control() || c.is_ascii_whitespace())
        .collect();
    get_bearer_token_re()
        .replace_all(&visible, "Bearer [REDACTED]")
        .to_string()
}

fn parse_bocha_results(parsed: &Value, max_results: usize) -> Vec<WebSearchEntry> {
    parsed
        .get("data")
        .and_then(|d| {
            d.get("webPages")
                .and_then(|w| w.get("value"))
                .or_else(|| d.get("pages"))
        })
        .or_else(|| parsed.get("pages"))
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|arr| arr.iter())
        .filter_map(|item| {
            let title = item
                .get("name")
                .or_else(|| item.get("title"))
                .and_then(|s| s.as_str())?
                .trim();
            let url = item
                .get("url")
                .or_else(|| item.get("link"))
                .and_then(|s| s.as_str())?
                .trim();
            if title.is_empty() || url.is_empty() {
                return None;
            }
            let snippet = item
                .get("summary")
                .or_else(|| item.get("snippet"))
                .or_else(|| item.get("description"))
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            Some(WebSearchEntry {
                title: title.to_string(),
                url: url.to_string(),
                snippet,
            })
        })
        .take(max_results)
        .collect()
}

fn bocha_error_message(parsed: &Value) -> Option<String> {
    let code = parsed.get("code").and_then(|v| v.as_i64())?;
    if code == 0 || code == 200 {
        return None;
    }
    let message = parsed
        .get("msg")
        .or_else(|| parsed.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    Some(format!("Bocha search API error (code {code}: {message})"))
}

fn parse_baidu_results(parsed: &Value, max_results: usize) -> Vec<WebSearchEntry> {
    parsed
        .get("references")
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|arr| arr.iter())
        .filter_map(|item| {
            let title = item
                .get("title")
                .or_else(|| item.get("name"))
                .and_then(|s| s.as_str())?
                .trim();
            let url = item
                .get("url")
                .or_else(|| item.get("link"))
                .and_then(|s| s.as_str())?
                .trim();
            if title.is_empty() || url.is_empty() {
                return None;
            }
            let snippet = item
                .get("content")
                .or_else(|| item.get("snippet"))
                .or_else(|| item.get("summary"))
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            Some(WebSearchEntry {
                title: title.to_string(),
                url: url.to_string(),
                snippet,
            })
        })
        .take(max_results)
        .collect()
}

fn parse_searxng_results(parsed: &Value, max_results: usize) -> Vec<WebSearchEntry> {
    parsed
        .get("results")
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|arr| arr.iter())
        .filter_map(|item| {
            let title = item.get("title").and_then(Value::as_str)?.trim();
            let url = item.get("url").and_then(Value::as_str)?.trim();
            if title.is_empty() || url.is_empty() {
                return None;
            }
            let snippet = first_non_empty_string(item, &["content", "snippet"]);
            Some(WebSearchEntry {
                title: title.to_string(),
                url: url.to_string(),
                snippet,
            })
        })
        .take(max_results)
        .collect()
}

fn baidu_error_message(parsed: &Value) -> Option<String> {
    let code = parsed
        .get("error_code")
        .or_else(|| parsed.get("code"))
        .and_then(|v| v.as_i64())?;
    if code == 0 {
        return None;
    }
    let message = parsed
        .get("error_msg")
        .or_else(|| parsed.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error");
    Some(format!("Baidu search API error (code {code}: {message})"))
}

fn parse_sofya_results(parsed: &Value, max_results: usize) -> Vec<WebSearchEntry> {
    parsed
        .get("results")
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|arr| arr.iter())
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let url = item.get("url")?.as_str()?.to_string();
            let snippet = first_non_empty_string(item, &["content", "description"]);
            Some(WebSearchEntry {
                title,
                url,
                snippet,
            })
        })
        .take(max_results)
        .collect()
}

fn first_non_empty_string(item: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        item.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn baidu_search_payload(query: &str, max_results: usize) -> Value {
    json!({
        "messages": [
            {
                "role": "user",
                "content": query,
            }
        ],
        "search_source": "baidu_search_v2",
        "resource_type_filter": [
            {
                "type": "web",
                "top_k": max_results,
            }
        ],
    })
}

fn volcengine_search_payload(query: &str, max_results: usize) -> Value {
    json!({
        "model": "doubao-seed-2-0-lite-260428",
        "stream": false,
        "tools": [{"type": "web_search"}],
        "input": [{
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": format!(
                    "Search the web for: {query}\n\n\
                     CRITICAL: Respond ONLY with a valid JSON object. No markdown, no explanation.\n\
                     Schema: {{\"results\":[{{\"title\":\"...\",\"url\":\"https://...\",\"snippet\":\"...\"}}]}}\n\
                     - results: 1-{max_results} most relevant pages\n\
                     - title: page title (required)\n\
                     - url: full URL starting with https:// (required)\n\
                     - snippet: 1-2 sentence factual summary (required)\n\
                     - If zero results: {{\"results\":[]}}\n\
                     - Your entire response must be valid, parseable JSON."
                )
            }]
        }]
    })
}

/// 从火山引擎 Responses API 输出中提取模型的文本响应。
fn volcengine_extract_text(parsed: &Value) -> Option<String> {
    parsed
        .get("output")
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|arr| arr.iter().rev())
        .find(|item| item.get("type").and_then(|t| t.as_str()) == Some("message"))
        .and_then(|msg| msg.get("content").and_then(|c| c.as_array()))
        .and_then(|content| {
            content
                .iter()
                .find(|c| c.get("text").and_then(|t| t.as_str()).is_some())
        })
        .and_then(|c| c.get("text").and_then(|t| t.as_str()))
        .map(|s| s.to_string())
}

/// 检查火山引擎 Responses API 响应中的业务逻辑错误。
fn volcengine_error_message(parsed: &Value) -> Option<String> {
    let error = parsed.get("error")?;
    let code = error
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let message = error
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("no details");
    Some(format!("Volcengine API error (code {code}: {message})"))
}

/// 将火山引擎模型生成的 JSON 结果解析为 `WebSearchEntry` 条目。
fn parse_volcengine_results(response_text: &str, max_results: usize) -> Vec<WebSearchEntry> {
    let json_text = extract_json_block(response_text).unwrap_or(response_text);

    let parsed: Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    parsed
        .get("results")
        .and_then(|v| v.as_array())
        .into_iter()
        .flat_map(|arr| arr.iter())
        .filter_map(|item| {
            let title = item.get("title").and_then(|s| s.as_str())?.trim();
            let url = item.get("url").and_then(|s| s.as_str())?.trim();
            if title.is_empty() || url.is_empty() {
                return None;
            }
            let snippet = item
                .get("snippet")
                .and_then(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            Some(WebSearchEntry {
                title: title.to_string(),
                url: url.to_string(),
                snippet,
            })
        })
        .take(max_results)
        .collect()
}

/// 尝试从可能被 markdown 围栏（```json ... ```）包裹或包含周围说明文字的文本中提取 JSON 块。
fn extract_json_block(text: &str) -> Option<&str> {
    if let Some(start) = text.find("```json") {
        let inner = &text[start + 7..];
        if let Some(end) = inner.find("```") {
            return Some(inner[..end].trim());
        }
    }
    if let Some(start) = text.find('{')
        && let Some(end) = text.rfind('}')
    {
        return Some(&text[start..=end]);
    }
    None
}

fn extract_search_query(input: &Value) -> Result<String, ToolError> {
    for key in ["query", "q"] {
        if let Some(value) = input.get(key) {
            let Some(query) = value.as_str() else {
                return Err(ToolError::invalid_input(format!(
                    "Field '{key}' must be a string"
                )));
            };
            let query = query.trim();
            if !query.is_empty() {
                return Ok(query.to_string());
            }
        }
    }

    for item in search_query_items(input) {
        for key in ["q", "query"] {
            if let Some(value) = item.get(key) {
                let Some(query) = value.as_str() else {
                    return Err(ToolError::invalid_input(format!(
                        "Field 'search_query[].{key}' must be a string"
                    )));
                };
                let query = query.trim();
                if !query.is_empty() {
                    return Ok(query.to_string());
                }
            }
        }
    }

    Err(ToolError::missing_field("query"))
}

fn optional_search_max_results(input: &Value) -> u64 {
    if let Some(value) = input.get("max_results").and_then(Value::as_u64) {
        return value;
    }
    search_query_items(input)
        .filter_map(|item| item.get("max_results").and_then(Value::as_u64))
        .next()
        .unwrap_or(DEFAULT_MAX_RESULTS as u64)
}

fn search_query_items(input: &Value) -> impl Iterator<Item = &Value> {
    input
        .get("search_query")
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|items| items.iter())
}

async fn run_bing_search(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<WebSearchEntry>, ToolError> {
    let encoded = url_encode(query);
    let url = format!("https://www.bing.com/search?q={encoded}");
    let resp = client
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Bing search request failed: {e}")))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
        ToolError::execution_failed(format!("Failed to read Bing search response: {e}"))
    })?;

    if !status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "Bing search failed: HTTP {}",
            status.as_u16()
        )));
    }

    Ok(parse_bing_results(&body, max_results))
}

fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<WebSearchEntry> {
    let title_re = get_title_re();
    let snippet_re = get_snippet_re();
    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .filter_map(|cap| cap.get(1).or_else(|| cap.get(2)))
        .map(|m| normalize_text(m.as_str()))
        .collect();

    let mut results = Vec::new();
    for (idx, cap) in title_re.captures_iter(html).enumerate() {
        if results.len() >= max_results {
            break;
        }
        let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_text(title_raw);
        if title.is_empty() {
            continue;
        }
        let url = normalize_url(href);
        let snippet = snippets
            .get(idx)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        results.push(WebSearchEntry {
            title,
            url,
            snippet,
        });
    }

    if is_likely_spam_results(&results) {
        // 与 Bing 路径相同的防御（#964）：当上游降级时，DDG 后备页面
        // 也可能提供单域名填充的结果集。丢弃而非误导模型。
        return Vec::new();
    }
    results
}

fn is_duckduckgo_challenge(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("Unfortunately, bots use DuckDuckGo too")
}

fn parse_bing_results(html: &str, max_results: usize) -> Vec<WebSearchEntry> {
    let mut results = Vec::new();
    for cap in get_bing_result_re().captures_iter(html) {
        if results.len() >= max_results {
            break;
        }
        let Some(block) = cap.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(title_cap) = get_bing_title_re().captures(block) else {
            continue;
        };
        let href = title_cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = title_cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_text(title_raw);
        if title.is_empty() {
            continue;
        }
        let snippet = get_bing_snippet_re()
            .captures(block)
            .and_then(|snippet_cap| snippet_cap.get(1))
            .map(|m| normalize_text(m.as_str()))
            .filter(|s| !s.is_empty());

        results.push(WebSearchEntry {
            title,
            url: normalize_bing_url(href),
            snippet,
        });
    }

    if is_likely_spam_results(&results) {
        // Bing 的抓取端点偶尔会提供一个填充页面，
        // 其中同一个低质量域名占据大部分 b_algo 条目——
        // #964 报告了来自 `astralia.forumgratuit.org` 的连续八个无关查询结果。
        // 将该批次视为"无结果"，以便调用方显示干净的失败消息，
        // 而不是将模型引向垃圾信息。
        return Vec::new();
    }
    results
}

/// 针对抓取的 SERP HTML 的启发式垃圾检测器（#964）。
///
/// 当一个根域名拥有至少 60% 的结果集且至少有三个结果时返回 `true`。
/// 来自 Google/Bing/DDG 的真实前五页面会混合多个域名；
/// 由一个主机主导的结果页面几乎总是 SEO 垃圾信息或机器人检测填充的替代页面。
fn is_likely_spam_results(results: &[WebSearchEntry]) -> bool {
    if results.len() < 3 {
        return false;
    }
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in results {
        if let Some(host) = root_domain(&r.url) {
            *counts.entry(host).or_insert(0) += 1;
        }
    }
    let Some(&max) = counts.values().max() else {
        return false;
    };
    // 60% 阈值：3/5、4/6、5/8 均触发；2/5 不触发。
    max * 5 >= results.len() * 3
}

/// 从 URL 中提取可注册的根域名（eTLD+1 近似值），
/// 以便垃圾检测将 `astralia.forumgratuit.org` 与
/// `russia.forumgratuit.org` 分组。返回小写主机减去最左侧标签，
/// 或当只有两个标签时返回裸主机。
fn root_domain(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = after_scheme.split(['/', '?', '#']).next()?;
    let host = host.split('@').next_back()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').filter(|s| !s.is_empty()).collect();
    if labels.len() <= 2 {
        return Some(host);
    }
    Some(labels[labels.len().saturating_sub(2)..].join("."))
}

fn normalize_url(href: &str) -> String {
    if let Some(uddg) = extract_query_param(href, "uddg") {
        let decoded = percent_decode(&uddg);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with('/') {
        return format!("https://duckduckgo.com{href}");
    }
    href.to_string()
}

fn normalize_bing_url(href: &str) -> String {
    // Bing 将每个 SERP 结果 URL 包裹在 `/ck/a?...&u=<base64>` 点击跟踪
    // 重定向中，在原始 HTML 中，分隔符是 `&amp;` 实体。如果不先解码实体，
    // `extract_query_param` 会查找 `u`，但实际键是 `amp;u`，
    // 因此真实 URL 永远无法恢复：每个结果都崩溃为 `bing.com` 根域名，
    // 然后垃圾启发式会拒绝它——导致默认 Bing 后端返回零结果。
    // 在解析之前解码实体。
    let href = decode_html_entities(href);
    let href = href.as_str();
    if let Some(encoded) = extract_query_param(href, "u") {
        let decoded = percent_decode(&encoded);
        let token = decoded.strip_prefix("a1").unwrap_or(&decoded);
        let mut padded = token.replace('-', "+").replace('_', "/");
        while !padded.len().is_multiple_of(4) {
            padded.push('=');
        }
        if let Ok(bytes) = general_purpose::STANDARD.decode(padded)
            && let Ok(url) = String::from_utf8(bytes)
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            return url;
        }
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with('/') {
        return format!("https://www.bing.com{href}");
    }
    href.to_string()
}

fn duckduckgo_search_url(
    base_url: Option<&str>,
    query: &str,
) -> Result<(String, String), ToolError> {
    let raw = configured_search_base_url(base_url).unwrap_or(DUCKDUCKGO_ENDPOINT);
    let mut url = reqwest::Url::parse(raw).map_err(|err| {
        ToolError::invalid_input(format!(
            "Invalid DuckDuckGo-compatible search base_url: {err}"
        ))
    })?;
    url.query_pairs_mut().append_pair("q", query);
    let host = url.host_str().ok_or_else(|| {
        ToolError::invalid_input("DuckDuckGo-compatible search base_url must include a host")
    })?;
    Ok((url.to_string(), host.to_string()))
}

fn searxng_search_url(base_url: Option<&str>, query: &str) -> Result<(String, String), ToolError> {
    let raw = configured_search_base_url(base_url).ok_or_else(|| {
        ToolError::invalid_input(
            "SearXNG search requires [search] base_url = \"https://your-searxng.example\"; no public instance is used by default.",
        )
    })?;
    let mut url = reqwest::Url::parse(raw).map_err(|err| {
        ToolError::invalid_input(format!("Invalid SearXNG search base_url: {err}"))
    })?;
    let host = url
        .host_str()
        .ok_or_else(|| ToolError::invalid_input("SearXNG search base_url must include a host"))?
        .to_string();

    let path = url.path().trim_end_matches('/');
    if path.is_empty() {
        url.set_path("search");
    } else if path != "/search" && !path.ends_with("/search") {
        url.set_path(&format!("{path}/search"));
    }
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json");

    Ok((url.to_string(), host))
}

fn configured_search_base_url(base_url: Option<&str>) -> Option<&str> {
    base_url.map(str::trim).filter(|value| !value.is_empty())
}

fn duckduckgo_allows_bing_fallback(base_url: Option<&str>) -> bool {
    configured_search_base_url(base_url).is_none()
}

fn normalize_text(text: &str) -> String {
    let stripped = strip_html_tags(text);
    let decoded = decode_html_entities(&stripped);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_html_tags(text: &str) -> String {
    get_tag_re().replace_all(text, "").to_string()
}

fn decode_html_entities(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static ENTITY_RE: OnceLock<Regex> = OnceLock::new();
    let re = ENTITY_RE.get_or_init(|| {
        Regex::new(r"&(?:#(\d+)|#x([0-9A-Fa-f]+)|([a-zA-Z]+));").expect("HTML entity regex")
    });

    re.replace_all(text, |caps: &regex::Captures| {
        if let Some(dec) = caps.get(1) {
            return dec
                .as_str()
                .parse::<u32>()
                .ok()
                .and_then(std::char::from_u32)
                .unwrap_or('\u{FFFD}')
                .to_string();
        }
        if let Some(hex) = caps.get(2) {
            return u32::from_str_radix(hex.as_str(), 16)
                .ok()
                .and_then(std::char::from_u32)
                .unwrap_or('\u{FFFD}')
                .to_string();
        }
        let named = caps.get(3).map(|m| m.as_str());
        match named {
            Some("amp") => "&",
            Some("lt") => "<",
            Some("gt") => ">",
            Some("quot") => "\"",
            Some("apos") => "'",
            Some("nbsp") => " ",
            Some("copy") => "\u{00A9}",
            Some("reg") => "\u{00AE}",
            Some("mdash") => "\u{2014}",
            Some("ndash") => "\u{2013}",
            Some("lsquo") => "\u{2018}",
            Some("rsquo") => "\u{2019}",
            Some("ldquo") => "\u{201C}",
            Some("rdquo") => "\u{201D}",
            Some("hellip") => "\u{2026}",
            _ => return caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string(),
        }
        .to_string()
    })
    .to_string()
}

fn url_encode(input: &str) -> String {
    crate::utils::url_encode(input)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
            }
            b'+' => out.push(b' '),
            _ => out.push(bytes[i]),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for part in query.split('&') {
        let mut iter = part.splitn(2, '=');
        let name = iter.next().unwrap_or("");
        if name == key {
            return iter.next().map(str::to_string);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        ERROR_BODY_PREVIEW_BYTES, WebSearchEntry, WebSearchTool, baidu_search_payload,
        bocha_error_message, decode_html_entities, duckduckgo_search_url, extract_search_query,
        is_likely_spam_results, normalize_bing_url, optional_search_max_results,
        parse_baidu_results, parse_bocha_results, parse_searxng_results, parse_sofya_results,
        root_domain, sanitize_error_body, searxng_search_url, truncate_error_body,
        volcengine_extract_text,
    };
    use serde_json::json;

    // 回归防护：Bing /ck/a 重定向 href 使用 HTML 实体编码（`&amp;`）。
    // normalize_bing_url 必须在提取 `u=` base64 负载之前解码实体，
    // 否则真实 URL 永远无法恢复，结果的根域名崩溃为 bing.com
    //（然后作为垃圾信息丢弃 → 默认 Bing 后端得到 0 个结果）。
    #[test]
    fn bing_ckurl_with_html_entities_decodes_real_url() {
        let href = "https://www.bing.com/ck/a?!&amp;&amp;p=abc&amp;u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&amp;ntb=1";
        assert_eq!(normalize_bing_url(href), "https://rust-lang.org/");
    }

    fn entry(url: &str) -> WebSearchEntry {
        WebSearchEntry {
            title: "x".into(),
            url: url.into(),
            snippet: None,
        }
    }

    #[test]
    fn root_domain_strips_subdomain_keeps_two_labels() {
        assert_eq!(
            root_domain("https://astralia.forumgratuit.org/path/page").as_deref(),
            Some("forumgratuit.org"),
        );
        assert_eq!(
            root_domain("http://www.example.com/").as_deref(),
            Some("example.com"),
        );
        assert_eq!(
            root_domain("https://example.com").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn root_domain_handles_port_and_userinfo() {
        assert_eq!(
            root_domain("http://user:pass@blog.example.com:8080/x").as_deref(),
            Some("example.com"),
        );
    }

    #[test]
    fn root_domain_returns_none_for_garbage() {
        assert!(
            root_domain("not-a-url").as_deref().is_some(),
            "bare token is treated as host"
        );
        assert!(root_domain("https:///path").is_none());
    }

    #[test]
    fn spam_detector_flags_single_domain_dominance() {
        // #964 重现：5/5 的结果来自同一个低质量主机。
        let r = vec![
            entry("https://astralia.forumgratuit.org/page1"),
            entry("https://russia.forumgratuit.org/page2"),
            entry("https://other.forumgratuit.org/page3"),
            entry("https://hello.forumgratuit.org/page4"),
            entry("https://world.forumgratuit.org/page5"),
        ];
        assert!(is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_passes_diverse_serp() {
        // 正常的 SERP 混合了多个域名；没有标记任何内容。
        let r = vec![
            entry("https://example.com/a"),
            entry("https://wikipedia.org/b"),
            entry("https://stackoverflow.com/c"),
            entry("https://reddit.com/d"),
            entry("https://example.com/e"),
        ];
        assert!(!is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_passes_short_result_set() {
        // 来自同一域名的两个结果不足以构成信号——对合法的双链接答案（文档+首页）
        // 产生误报的伤害比放行它们更大。
        let r = vec![
            entry("https://example.com/a"),
            entry("https://example.com/b"),
        ];
        assert!(!is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_threshold_is_sixty_percent() {
        // 3/5 同域名触发 60% 阈值。
        let r3of5 = vec![
            entry("https://spam.example.com/a"),
            entry("https://spam.example.com/b"),
            entry("https://spam.example.com/c"),
            entry("https://other.com/d"),
            entry("https://third.com/e"),
        ];
        assert!(is_likely_spam_results(&r3of5));
        // 2/5 不触发阈值。
        let r2of5 = vec![
            entry("https://spam.example.com/a"),
            entry("https://spam.example.com/b"),
            entry("https://other.com/c"),
            entry("https://third.com/d"),
            entry("https://fourth.com/e"),
        ];
        assert!(!is_likely_spam_results(&r2of5));
    }

    #[test]
    fn decode_html_entities_handles_named_entities() {
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&lt;"), "<");
        assert_eq!(decode_html_entities("&gt;"), ">");
        assert_eq!(decode_html_entities("&quot;"), "\"");
        assert_eq!(decode_html_entities("&apos;"), "'");
        assert_eq!(decode_html_entities("&nbsp;"), " ");
        assert_eq!(decode_html_entities("&copy;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&mdash;"), "\u{2014}");
    }

    #[test]
    fn decode_html_entities_handles_decimal_numeric_references() {
        assert_eq!(decode_html_entities("&#65;"), "A");
        assert_eq!(decode_html_entities("&#60;"), "<");
        assert_eq!(decode_html_entities("&#8211;"), "\u{2013}");
    }

    #[test]
    fn decode_html_entities_handles_hex_numeric_references() {
        assert_eq!(decode_html_entities("&#x41;"), "A");
        assert_eq!(decode_html_entities("&#x3C;"), "<");
        assert_eq!(decode_html_entities("&#x2014;"), "\u{2014}");
    }

    #[test]
    fn decode_html_entities_passthrough_unknown() {
        assert_eq!(decode_html_entities("&unknown;"), "&unknown;");
    }

    #[test]
    fn decode_html_entities_mixed_content() {
        let input = "Hello &amp; welcome to &quot;Rust&apos;s world&quot; &mdash; enjoy!";
        let expected = "Hello & welcome to \"Rust's world\" \u{2014} enjoy!";
        assert_eq!(decode_html_entities(input), expected);
    }

    #[test]
    fn extract_search_query_accepts_legacy_query() {
        let query =
            extract_search_query(&json!({"query": " deepseek v4 "})).expect("query should parse");
        assert_eq!(query, "deepseek v4");
    }

    #[test]
    fn extract_search_query_accepts_q_alias() {
        let query =
            extract_search_query(&json!({"q": "deepseek v4 pro"})).expect("q alias should parse");
        assert_eq!(query, "deepseek v4 pro");
    }

    #[test]
    fn extract_search_query_accepts_array_form() {
        let input = json!({"search_query": [{"q": "deepseek api", "max_results": 3}]});
        let query = extract_search_query(&input).expect("array form should parse");
        assert_eq!(query, "deepseek api");
        assert_eq!(optional_search_max_results(&input), 3);
    }

    #[test]
    fn extract_search_query_rejects_missing_query() {
        let err = extract_search_query(&json!({"max_results": 2}))
            .expect_err("missing query should fail");
        assert!(format!("{err}").contains("missing required field 'query'"));
    }

    #[test]
    fn optional_max_results_prefers_top_level_value() {
        // 顶级 `max_results` 优先于数组形式的兄弟字段，
        // 因为使用数组形式的调用方通常会整体复制粘贴它，
        // 然后随后调整外层的 max_results。
        assert_eq!(
            optional_search_max_results(
                &json!({"query": "x", "max_results": 8, "search_query": [{"q": "y", "max_results": 2}]})
            ),
            8,
        );
    }

    #[test]
    fn optional_max_results_falls_back_to_array_form() {
        // 当只有数组形式设置了 max_results 时，该值应该是
        // 到达调用方的值。这是 V4 在发出结构化 `search_query: [{…}]` 形状时使用的路径。
        assert_eq!(
            optional_search_max_results(&json!({"search_query": [{"q": "y", "max_results": 3}]})),
            3,
        );
    }

    #[test]
    fn optional_max_results_uses_default_when_neither_set() {
        // 没有任何显式限制 → 应用默认值（当前为 5），
        // 这样模型就不能仅仅通过省略该字段来意外拉取 MAX_RESULTS 的带宽。
        assert_eq!(optional_search_max_results(&json!({"query": "x"})), 5);
        assert_eq!(
            optional_search_max_results(&json!({"search_query": [{"q": "y"}]})),
            5,
        );
    }

    #[test]
    fn optional_max_results_only_reads_first_array_entry() {
        // 子搜索支持是未来的功能；目前忽略第一个之后的数组条目。
        // 固定此行为，以便未来的多查询实现必须有意更新此测试，
        // 而不是静默开始分发。
        assert_eq!(
            optional_search_max_results(
                &json!({"search_query": [{"q": "first", "max_results": 1}, {"q": "second", "max_results": 9}]})
            ),
            1,
        );
    }

    #[test]
    fn extract_search_query_trims_whitespace_from_array_form_q_alias() {
        // "修剪"约定是辅助函数不变式的一部分——
        // 模型有时会使用 heredoc 的换行符填充 `q`。
        let q = extract_search_query(&json!({"search_query": [{"q": "  deepseek tui  "}]}))
            .expect("array form should parse with trim");
        assert_eq!(q, "deepseek tui");
    }

    #[test]
    fn extract_search_query_rejects_empty_query() {
        // 空字符串查询进入 extract_search_query → 作为 missing_field 传播，
        // 而不是在下面几层出现令人困惑的引擎错误。锁定此失败模式。
        for body in [json!({"query": ""}), json!({"q": "   "}), json!({})] {
            let err = extract_search_query(&body).expect_err("empty query must reject");
            let msg = format!("{err}");
            assert!(
                msg.contains("missing required field 'query'") || msg.contains("Query"),
                "expected query-missing error, got `{msg}`"
            );
        }
    }

    #[test]
    fn truncate_error_body_truncates_long_body() {
        let body = "a".repeat(ERROR_BODY_PREVIEW_BYTES + 100);
        let truncated = truncate_error_body(&body);
        assert!(truncated.len() <= ERROR_BODY_PREVIEW_BYTES + 3);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn truncate_error_body_keeps_short_body_intact() {
        let body = "short error";
        assert_eq!(truncate_error_body(body), body);
    }

    #[test]
    fn sanitize_error_body_strips_html_and_control_chars() {
        let body = "<p>error</p>\x00\x01\x02";
        let sanitized = sanitize_error_body(body);
        assert_eq!(sanitized, "error");
    }

    #[test]
    fn sanitize_error_body_redacts_bearer_tokens() {
        let body = r#"{"error":"bad token","authorization":"Bearer test-token/with+chars="}"#;

        let sanitized = sanitize_error_body(body);

        assert!(!sanitized.contains("test-token/with+chars="));
        assert!(sanitized.contains("Bearer [REDACTED]"));
    }

    #[test]
    fn parse_bocha_web_pages_value_extracts_ranked_results() {
        let body = json!({
            "code": 200,
            "msg": null,
            "data": {
                "webPages": {
                    "value": [
                        {
                            "name": "广州天气",
                            "url": "https://bocha.cn/share/weather",
                            "snippet": "广州今日雷阵雨转晴。"
                        },
                        {
                            "name": "中央气象台",
                            "url": "https://www.weather.com.cn/",
                            "summary": "天气实况。"
                        }
                    ]
                }
            }
        });

        let results = parse_bocha_results(&body, 10);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "广州天气");
        assert_eq!(results[0].url, "https://bocha.cn/share/weather");
        assert_eq!(results[0].snippet.as_deref(), Some("广州今日雷阵雨转晴。"));
        assert_eq!(results[1].title, "中央气象台");
    }

    #[test]
    fn parse_bocha_keeps_legacy_pages_shape() {
        let body = json!({
            "code": 200,
            "data": {
                "pages": [
                    {
                        "title": "Legacy title",
                        "link": "https://example.com/legacy",
                        "description": "Legacy description"
                    }
                ]
            }
        });

        let results = parse_bocha_results(&body, 5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Legacy title");
        assert_eq!(results[0].url, "https://example.com/legacy");
        assert_eq!(results[0].snippet.as_deref(), Some("Legacy description"));
    }

    #[test]
    fn bocha_error_message_flags_non_success_business_code() {
        let body = json!({"code": 401, "msg": "invalid api key"});

        let error = bocha_error_message(&body).expect("non-success code should error");

        assert!(error.contains("Bocha"));
        assert!(error.contains("401"));
        assert!(error.contains("invalid api key"));
    }

    #[test]
    fn parse_baidu_references_extracts_ranked_results() {
        let body = json!({
            "references": [
                {
                    "title": "Rust 官方文档",
                    "url": "https://www.rust-lang.org/",
                    "content": "Rust 是一门注重性能和可靠性的语言。"
                },
                {
                    "title": "Cargo Book",
                    "url": "https://doc.rust-lang.org/cargo/",
                    "snippet": "Cargo is Rust's package manager."
                }
            ]
        });

        let results = parse_baidu_results(&body, 10);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust 官方文档");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(
            results[0].snippet.as_deref(),
            Some("Rust 是一门注重性能和可靠性的语言。")
        );
        assert_eq!(results[1].title, "Cargo Book");
        assert_eq!(results[1].url, "https://doc.rust-lang.org/cargo/");
        assert_eq!(
            results[1].snippet.as_deref(),
            Some("Cargo is Rust's package manager.")
        );
    }

    #[test]
    fn parse_baidu_references_skips_incomplete_entries() {
        let body = json!({
            "references": [
                {"title": "No URL", "content": "missing url"},
                {"url": "https://example.com/no-title", "content": "missing title"},
                {"title": "Valid", "url": "https://example.com/valid"}
            ]
        });

        let results = parse_baidu_results(&body, 10);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Valid");
        assert_eq!(results[0].url, "https://example.com/valid");
        assert_eq!(results[0].snippet, None);
    }

    #[test]
    fn baidu_search_payload_uses_official_search_source() {
        let payload = baidu_search_payload("Rust cargo workspace", 3);

        assert_eq!(
            payload.get("search_source").and_then(|v| v.as_str()),
            Some("baidu_search_v2")
        );
        assert_eq!(
            payload
                .get("messages")
                .and_then(|v| v.as_array())
                .and_then(|messages| messages.first())
                .and_then(|message| message.get("content"))
                .and_then(|v| v.as_str()),
            Some("Rust cargo workspace")
        );
        assert_eq!(
            payload
                .get("resource_type_filter")
                .and_then(|v| v.as_array())
                .and_then(|filters| filters.first())
                .and_then(|filter| filter.get("top_k"))
                .and_then(|v| v.as_u64()),
            Some(3)
        );
    }

    #[test]
    fn parse_sofya_results_falls_back_to_description_for_empty_content() {
        let body = json!({
            "results": [
                {
                    "title": "Full content",
                    "url": "https://example.com/full",
                    "content": "full extracted page content",
                    "description": "unused description"
                },
                {
                    "title": "Null content",
                    "url": "https://example.com/null",
                    "content": null,
                    "description": "description for null content"
                },
                {
                    "title": "Empty content",
                    "url": "https://example.com/empty",
                    "content": "",
                    "description": "description for empty content"
                },
                {
                    "title": "Whitespace content",
                    "url": "https://example.com/blank",
                    "content": "   ",
                    "description": "description for blank content"
                },
                {
                    "title": "No snippet",
                    "url": "https://example.com/no-snippet"
                }
            ]
        });

        let results = parse_sofya_results(&body, 10);

        assert_eq!(results.len(), 5);
        assert_eq!(
            results[0].snippet.as_deref(),
            Some("full extracted page content")
        );
        assert_eq!(
            results[1].snippet.as_deref(),
            Some("description for null content")
        );
        assert_eq!(
            results[2].snippet.as_deref(),
            Some("description for empty content")
        );
        assert_eq!(
            results[3].snippet.as_deref(),
            Some("description for blank content")
        );
        assert_eq!(results[4].snippet, None);
    }

    #[test]
    fn volcengine_extract_text_skips_non_text_content_blocks() {
        let body = json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {"type": "reasoning", "summary": "thinking first"},
                        {"type": "output_text", "text": "{\"results\":[]}"}
                    ]
                }
            ]
        });

        assert_eq!(
            volcengine_extract_text(&body).as_deref(),
            Some("{\"results\":[]}")
        );
    }

    #[tokio::test]
    async fn tavily_provider_without_api_key_surfaces_clear_error_not_silent_fallback() {
        // 信任边界固定：如果用户已选择 Tavily 但忘记了 api_key，
        // 该工具绝不能静默回退到 DuckDuckGo（这会将查询暴露给用户授权之外的不同提供商）。
        // 相反，它返回一个明确命名缺失密钥的 ToolError。
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Tavily;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");
        let msg = err.to_string();
        assert!(
            msg.contains("Tavily") && msg.contains("API key"),
            "error must name the provider and missing key; got `{msg}`"
        );
    }

    #[tokio::test]
    async fn bocha_provider_without_api_key_surfaces_clear_error_not_silent_fallback() {
        // 与 Bocha 相同的信任边界固定。
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Bocha;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");
        let msg = err.to_string();
        assert!(
            msg.contains("Bocha") && msg.contains("API key"),
            "error must name the provider and missing key; got `{msg}`"
        );
    }

    #[tokio::test]
    async fn baidu_provider_without_api_key_surfaces_clear_error_not_silent_fallback() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let prev = std::env::var_os("BAIDU_SEARCH_API_KEY");
        unsafe { std::env::remove_var("BAIDU_SEARCH_API_KEY") };

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Baidu;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");

        match prev {
            Some(value) => unsafe { std::env::set_var("BAIDU_SEARCH_API_KEY", value) },
            None => unsafe { std::env::remove_var("BAIDU_SEARCH_API_KEY") },
        }

        let msg = err.to_string();
        assert!(
            msg.contains("Baidu") && msg.contains("API key"),
            "error must name the provider and missing key; got `{msg}`"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sofya_provider_without_api_key_surfaces_clear_error_not_silent_fallback() {
        // 与 Tavily/Bocha 相同的信任边界固定：选择 Sofya 但没有密钥
        // 必须返回一个命名提供商的 ToolError，而不是静默回退到 DuckDuckGo。
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        // 此测试在等待工具执行期间持有进程环境锁，
        // 因为工具在该调用期间读取 SOFYA_API_KEY。
        let _guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("SOFYA_API_KEY");
        unsafe { std::env::remove_var("SOFYA_API_KEY") };

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Sofya;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");

        match prev {
            Some(value) => unsafe { std::env::set_var("SOFYA_API_KEY", value) },
            None => unsafe { std::env::remove_var("SOFYA_API_KEY") },
        }

        let msg = err.to_string();
        assert!(
            msg.contains("Sofya") && msg.contains("API key"),
            "error must name the provider and missing key; got `{msg}`"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn volcengine_provider_without_api_key_lists_supported_env_fallbacks() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        // 此测试有意在等待工具执行期间持有进程环境锁，
        // 因为工具在该调用期间读取环境变量回退。
        // 在 await 之前释放锁会重新引入与其他修改环境变量的测试的竞态条件。
        let _guard = crate::test_support::lock_test_env();
        let prev_volc = std::env::var_os("VOLCENGINE_API_KEY");
        let prev_volc_ark = std::env::var_os("VOLCENGINE_ARK_API_KEY");
        let prev_ark = std::env::var_os("ARK_API_KEY");
        unsafe {
            std::env::remove_var("VOLCENGINE_API_KEY");
            std::env::remove_var("VOLCENGINE_ARK_API_KEY");
            std::env::remove_var("ARK_API_KEY");
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Volcengine;
        ctx.search_api_key = None;
        let err = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await
            .expect_err("missing api_key must surface as ToolError");

        match prev_volc {
            Some(value) => unsafe { std::env::set_var("VOLCENGINE_API_KEY", value) },
            None => unsafe { std::env::remove_var("VOLCENGINE_API_KEY") },
        }
        match prev_volc_ark {
            Some(value) => unsafe { std::env::set_var("VOLCENGINE_ARK_API_KEY", value) },
            None => unsafe { std::env::remove_var("VOLCENGINE_ARK_API_KEY") },
        }
        match prev_ark {
            Some(value) => unsafe { std::env::set_var("ARK_API_KEY", value) },
            None => unsafe { std::env::remove_var("ARK_API_KEY") },
        }

        let msg = err.to_string();
        assert!(msg.contains("Volcengine") && msg.contains("API key"));
        assert!(msg.contains("VOLCENGINE_API_KEY"));
        assert!(msg.contains("VOLCENGINE_ARK_API_KEY"));
        assert!(msg.contains("ARK_API_KEY"));
        assert!(!msg.contains("DEEPSEEK_SEARCH_API_KEY"));
    }

    #[tokio::test]
    async fn metaso_provider_uses_built_in_key_when_no_config_key_set() {
        // 与 Tavily/Bocha 不同，Metaso 回退到内置默认值，
        // 因此调用不应该返回与 API 密钥相关的错误——
        // 它应该成功或出现网络级错误，但绝不会是缺失密钥错误。
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Metaso;
        ctx.search_api_key = None;
        let result = WebSearchTool
            .execute(json!({"query": "anything"}), &ctx)
            .await;
        let msg = match &result {
            Ok(res) => format!("{res:?}"),
            Err(e) => e.to_string(),
        };
        assert!(
            !msg.contains("API key"),
            "should not complain about missing API key (built-in default); got `{msg}`"
        );
    }

    #[test]
    fn duckduckgo_compatible_url_uses_custom_base_url_and_preserves_query() {
        let (url, host) = duckduckgo_search_url(
            Some("https://search.internal.example/html/?region=us"),
            "rust async",
        )
        .expect("custom duckduckgo-compatible url");

        assert_eq!(host, "search.internal.example");
        assert_eq!(
            url,
            "https://search.internal.example/html/?region=us&q=rust+async"
        );
    }

    #[test]
    fn custom_duckduckgo_endpoint_disables_public_bing_fallback() {
        assert!(super::duckduckgo_allows_bing_fallback(None));
        assert!(super::duckduckgo_allows_bing_fallback(Some("   ")));
        assert!(!super::duckduckgo_allows_bing_fallback(Some(
            "https://search.internal.example/html/"
        )));
    }

    #[test]
    fn searxng_url_uses_search_path_and_json_format() {
        let (url, host) =
            searxng_search_url(Some("https://search.example/"), "rust async").expect("searxng url");
        let parsed = reqwest::Url::parse(&url).expect("valid url");
        assert_eq!(host, "search.example");
        assert_eq!(parsed.path(), "/search");
        assert_eq!(
            parsed.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            "rust async"
        );
        assert_eq!(
            parsed
                .query_pairs()
                .find(|(key, _)| key == "format")
                .unwrap()
                .1,
            "json"
        );

        let (subpath_url, _) = searxng_search_url(
            Some("https://search.example/searxng?language=en"),
            "codewhale",
        )
        .expect("searxng subpath url");
        let parsed = reqwest::Url::parse(&subpath_url).expect("valid subpath url");
        assert_eq!(parsed.path(), "/searxng/search");
        assert_eq!(
            parsed
                .query_pairs()
                .find(|(key, _)| key == "language")
                .unwrap()
                .1,
            "en"
        );

        let (search_url, _) =
            searxng_search_url(Some("https://search.example/searxng/search"), "codewhale")
                .expect("searxng search endpoint");
        assert_eq!(
            reqwest::Url::parse(&search_url)
                .expect("valid search url")
                .path(),
            "/searxng/search"
        );
    }

    #[test]
    fn searxng_parser_normalizes_results() {
        let parsed = json!({
            "results": [
                {
                    "title": " Rust async ",
                    "url": " https://example.com/rust ",
                    "content": " Result content "
                },
                {
                    "title": "Empty snippet",
                    "url": "https://example.com/empty",
                    "content": "   ",
                    "snippet": " Fallback snippet "
                },
                {
                    "title": "",
                    "url": "https://example.com/missing-title",
                    "content": "ignored"
                },
                {
                    "title": "Missing URL",
                    "content": "ignored"
                }
            ]
        });

        let results = parse_searxng_results(&parsed, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust async");
        assert_eq!(results[0].url, "https://example.com/rust");
        assert_eq!(results[0].snippet.as_deref(), Some("Result content"));
        assert_eq!(results[1].snippet.as_deref(), Some("Fallback snippet"));
    }

    #[tokio::test]
    async fn searxng_provider_requires_base_url() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Searxng;
        ctx.search_base_url = None;

        let err = WebSearchTool
            .execute(json!({"query": "rust async"}), &ctx)
            .await
            .expect_err("searxng requires explicit base_url");
        let msg = err.to_string();
        assert!(
            msg.contains("SearXNG")
                && msg.contains("base_url")
                && msg.contains("no public instance"),
            "got `{msg}`"
        );
    }

    #[tokio::test]
    async fn searxng_search_returns_json_results() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust async"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {
                        "title": "Rust async",
                        "url": "https://example.com/rust",
                        "content": "Async Rust result"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Searxng;
        ctx.search_base_url = Some(server.uri());

        let result = WebSearchTool
            .execute(json!({"query": "rust async"}), &ctx)
            .await
            .expect("searxng endpoint should return results");
        let value: serde_json::Value =
            serde_json::from_str(&result.content).expect("web search json response");

        assert_eq!(value["source"].as_str(), Some("searxng"));
        assert_eq!(value["count"].as_u64(), Some(1));
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("Backend: searxng at")
        );
    }

    #[tokio::test]
    async fn searxng_empty_results_report_backend() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "empty"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Searxng;
        ctx.search_base_url = Some(server.uri());

        let result = WebSearchTool
            .execute(json!({"query": "empty"}), &ctx)
            .await
            .expect("empty searxng response should still be structured");
        let value: serde_json::Value =
            serde_json::from_str(&result.content).expect("web search json response");

        assert_eq!(value["count"].as_u64(), Some(0));
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("Backend: searxng at")
        );
    }

    #[tokio::test]
    async fn searxng_http_errors_are_actionable() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "blocked"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(403).set_body_string("json disabled"))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Searxng;
        ctx.search_base_url = Some(server.uri());

        let err = WebSearchTool
            .execute(json!({"query": "blocked"}), &ctx)
            .await
            .expect_err("403 should be actionable");
        let msg = err.to_string();
        assert!(
            msg.contains("HTTP 403")
                && msg.contains("JSON output")
                && msg.contains("permits API access"),
            "got `{msg}`"
        );
    }

    #[tokio::test]
    async fn searxng_rate_limit_error_mentions_configured_instance() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "later"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(429).set_body_string("too many requests"))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Searxng;
        ctx.search_base_url = Some(server.uri());

        let err = WebSearchTool
            .execute(json!({"query": "later"}), &ctx)
            .await
            .expect_err("429 should be actionable");
        let msg = err.to_string();
        assert!(
            msg.contains("HTTP 429")
                && msg.contains("rate-limiting")
                && msg.contains("trusted/self-hosted instance"),
            "got `{msg}`"
        );
    }

    #[tokio::test]
    async fn searxng_invalid_json_is_actionable() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "html"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Searxng;
        ctx.search_base_url = Some(server.uri());

        let err = WebSearchTool
            .execute(json!({"query": "html"}), &ctx)
            .await
            .expect_err("invalid JSON should be actionable");
        let msg = err.to_string();
        assert!(
            msg.contains("Failed to parse SearXNG JSON response")
                && msg.contains("format=json")
                && msg.contains("JSON output"),
            "got `{msg}`"
        );
    }

    #[tokio::test]
    async fn custom_duckduckgo_results_report_custom_host_source() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/html/"))
            .and(query_param("q", "rust async"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"
                <html><body>
                  <a class="result__a" href="https://example.com/rust">Rust async</a>
                  <div class="result__snippet">Async Rust result</div>
                </body></html>
                "#,
            ))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::DuckDuckGo;
        let base_url = format!("{}/html/", server.uri());
        let expected_host = reqwest::Url::parse(&base_url)
            .expect("mock server url")
            .host_str()
            .expect("mock server host")
            .to_string();
        ctx.search_base_url = Some(base_url);

        let result = WebSearchTool
            .execute(json!({"query": "rust async"}), &ctx)
            .await
            .expect("custom endpoint should return results");
        let value: serde_json::Value =
            serde_json::from_str(&result.content).expect("web search json response");

        assert_eq!(value["source"].as_str(), Some(expected_host.as_str()));
        assert_eq!(value["count"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn custom_duckduckgo_challenge_returns_actionable_error() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/html/"))
            .and(query_param("q", "rust async"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"<html><body><div class="anomaly-modal">Unfortunately, bots use DuckDuckGo too</div></body></html>"#,
            ))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::DuckDuckGo;
        ctx.search_base_url = Some(format!("{}/html/", server.uri()));

        let err = WebSearchTool
            .execute(json!({"query": "rust async"}), &ctx)
            .await
            .expect_err("custom endpoint challenge should error");
        let msg = err.to_string();
        assert!(
            msg.contains("DuckDuckGo-compatible search endpoint")
                && msg.contains("bot challenge")
                && msg.contains("private search service"),
            "got `{msg}`"
        );
    }

    #[tokio::test]
    async fn search_base_url_with_non_duckduckgo_provider_is_explicit_error() {
        use crate::config::SearchProvider;
        use crate::tools::spec::{ToolContext, ToolSpec};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut ctx = ToolContext::new(tmp.path().to_path_buf());
        ctx.search_provider = SearchProvider::Tavily;
        ctx.search_base_url = Some("https://search.internal.example/html/".to_string());

        let err = WebSearchTool
            .execute(json!({"query": "rust async"}), &ctx)
            .await
            .expect_err("non-duckduckgo provider with base_url should error");
        let msg = err.to_string();
        assert!(
            msg.contains("[search].base_url")
                && msg.contains("provider = \"duckduckgo\" or \"searxng\"")
                && msg.contains("tavily"),
            "got `{msg}`"
        );
    }
}
