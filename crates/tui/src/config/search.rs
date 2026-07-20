//! 网络搜索供应商配置类型。
//!
//! 从 `config.rs` 中逐字提取的自包含 `[search]` 表类型。
//! 通过 `pub use search::*;` 从 `crate::config` 重新导出，以便现有的
//! `crate::config::SearchProvider`（及同类）路径无需更改即可解析 (#3311)。

use serde::{Deserialize, Serialize};

/// 搜索供应商枚举——选择 `web_search` 使用的后端。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    /// Bing HTML 抓取。无需 API 密钥。
    Bing,
    /// DuckDuckGo HTML 抓取，带 Bing 回退。无需 API 密钥。
    #[default]
    #[serde(alias = "duckduckgo")]
    DuckDuckGo,
    /// Tavily AI 搜索 API（<https://tavily.com>）。需要 api_key。
    Tavily,
    /// Bocha AI 搜索 API（<https://bochaai.com>）。需要 api_key。
    Bocha,
    /// Metaso AI 搜索 API（<https://metaso.cn>）。使用内置默认密钥
    /// 或 `METASO_API_KEY` 环境变量；可通过 `[search] api_key` 配置。
    #[serde(alias = "metaso")]
    Metaso,
    /// SearXNG JSON 搜索 API。需要可信/自托管的 `base_url`。
    #[serde(alias = "searx", alias = "searx-ng", alias = "searx_ng")]
    Searxng,
    /// 百度 AI 搜索 API（<https://qianfan.baidubce.com>）。需要 api_key。
    #[serde(
        alias = "baidu-search",
        alias = "baidu_ai_search",
        alias = "baidu_search",
        alias = "baidu-ai-search"
    )]
    Baidu,
    /// 火山引擎 Ark 通过 Responses API 提供的 web_search。需要 api_key。
    /// 免费层：每个 API 密钥每月 20K 次查询。当 `[search] api_key` 未设置时，
    /// 回退到 `VOLCENGINE_API_KEY` / `VOLCENGINE_ARK_API_KEY` / `ARK_API_KEY`
    /// 环境变量。
    #[serde(
        alias = "volcengine",
        alias = "ark",
        alias = "volc",
        alias = "volcengine-ark",
        alias = "volcengine_ark",
        alias = "volc-ark"
    )]
    Volcengine,
    /// Sofya 网络搜索 API（<https://sofya.co>）。需要 api_key
    ///（`ay_live_...`）。返回完整的提取页面内容而不是
    /// 摘要；当 `[search] api_key` 未设置时，回退到 `SOFYA_API_KEY` 环境变量。
    Sofya,
}

impl SearchProvider {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bing" => Some(Self::Bing),
            "duckduckgo" | "duck-duck-go" | "duck_duck_go" | "ddg" => Some(Self::DuckDuckGo),
            "tavily" => Some(Self::Tavily),
            "bocha" => Some(Self::Bocha),
            "metaso" => Some(Self::Metaso),
            "searxng" | "searx" | "searx-ng" | "searx_ng" => Some(Self::Searxng),
            "baidu" | "baidu-search" | "baidu_search" | "baidu-ai-search" | "baidu_ai_search" => {
                Some(Self::Baidu)
            }
            "volcengine" | "ark" | "volc" | "volcengine-ark" => Some(Self::Volcengine),
            "sofya" => Some(Self::Sofya),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bing => "bing",
            Self::DuckDuckGo => "duckduckgo",
            Self::Tavily => "tavily",
            Self::Bocha => "bocha",
            Self::Metaso => "metaso",
            Self::Searxng => "searxng",
            Self::Baidu => "baidu",
            Self::Volcengine => "volcengine",
            Self::Sofya => "sofya",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderSource {
    Default,
    Config,
    EnvOverride,
}

impl SearchProviderSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Config => "config",
            Self::EnvOverride => "env override",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProviderResolution {
    pub provider: SearchProvider,
    pub source: SearchProviderSource,
}

/// 网络搜索供应商配置（config.toml 中的 `[search]` 表）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchConfig {
    /// 搜索供应商：`bing` | `duckduckgo` | `tavily` | `bocha` | `metaso` | `searxng` | `baidu` | `volcengine`。默认值：`duckduckgo`。
    #[serde(default)]
    pub provider: Option<SearchProvider>,
    /// 可选的搜索端点。使用 `duckduckgo` 时，这是一个兼容 DuckDuckGo 的 HTML 端点。
    /// 使用 `searxng` 时，这是可信的 SearXNG 实例根地址或 `/search` 端点。
    #[serde(default)]
    pub base_url: Option<String>,
    /// Tavily、Bocha、Metaso、Baidu 或 Volcengine 的 API 密钥。Bing、DuckDuckGo 或 SearXNG 不需要。
    /// Metaso 还会回退到 `METASO_API_KEY` 环境变量，然后是内置默认值。
    /// Baidu 还会回退到 `BAIDU_SEARCH_API_KEY` 环境变量。
    /// Volcengine 还会回退到 `VOLCENGINE_API_KEY` / `VOLCENGINE_ARK_API_KEY` / `ARK_API_KEY` 环境变量。
    #[serde(default)]
    pub api_key: Option<String>,
}
