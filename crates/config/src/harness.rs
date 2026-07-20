//! 测试框架姿态 + 配置文件配置类型 (#3311)。
//!
//! *测试框架姿态*是代理塑造策略（子代理上限、工具表面、压缩/缓存策略、
//! 安全立场）；*测试框架配置文件*将姿态绑定到供应商路由 + 模型模式。
//! 从 lib.rs 原样提取，以将此代理姿态领域与其余配置 schema 分离；
//! 在 crate 根重新导出，以便现有路径保持不变。行为相同。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::ProviderKind;

/// 内置测试框架姿态的种类。
///
/// 姿态命名了 CodeWhale 应为某个供应商/模型路由使用的运行时策略：
/// 预加载多少上下文、多积极地依赖子代理，以及如何平衡提示缓存稳定性与快速探索。
/// 运行时选择将在后续 v0.9 版本中接入；此配置模型有意优先使策略数据保持显式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessPostureKind {
    /// 全功能默认：丰富的配置、广泛的工具目录和正常的子代理姿态。
    #[default]
    Standard,
    /// 缓存优先：更深的提示分层和面向前缀缓存的上下文。
    CacheHeavy,
    /// 精简：更小的起始上下文、更快的压缩和更强的探索/委派偏向。
    Lean,
    /// 用户自定义姿态，由下方显式参数组装而成。
    Custom,
}

/// 此姿态应如何处理压缩和提示缓存稳定性。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessCompactionStrategy {
    #[default]
    Default,
    PrefixCache,
    Aggressive,
}

/// 此姿态偏好的工具目录形态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessToolSurface {
    #[default]
    Full,
    ReadOnly,
    Auto,
}

/// 运行时使用测试框架配置文件时应用的安全姿态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessSafetyPosture {
    #[default]
    Standard,
    Strict,
    Permissive,
}

/// 一个具有策略旋钮的具体测试框架姿态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HarnessPosture {
    /// 命名的姿态种类。
    #[serde(default)]
    pub kind: HarnessPostureKind,
    /// 最大并发子代理数（0 = 运行时默认值）。
    #[serde(default)]
    pub max_subagents: usize,
    /// 优先使用基于搜索/按需的上下文，而非常驻文档。
    #[serde(default)]
    pub prefer_codebase_search: bool,
    /// 压缩和提示缓存策略。
    #[serde(default)]
    pub compaction_strategy: HarnessCompactionStrategy,
    /// 偏好的工具目录形态。
    #[serde(default)]
    pub tool_surface: HarnessToolSurface,
    /// 运行时消费者的安全姿态。
    #[serde(default)]
    pub safety_posture: HarnessSafetyPosture,
}

impl Default for HarnessPosture {
    fn default() -> Self {
        Self {
            kind: HarnessPostureKind::Standard,
            max_subagents: 0,
            prefer_codebase_search: false,
            compaction_strategy: HarnessCompactionStrategy::default(),
            tool_surface: HarnessToolSurface::default(),
            safety_posture: HarnessSafetyPosture::default(),
        }
    }
}

impl HarnessPosture {
    /// 针对 DeepSeek V4 / MiMo 风格模型调优的缓存优先姿态。
    #[must_use]
    pub fn cache_heavy() -> Self {
        Self {
            kind: HarnessPostureKind::CacheHeavy,
            max_subagents: 10,
            prefer_codebase_search: false,
            compaction_strategy: HarnessCompactionStrategy::PrefixCache,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    }

    /// 针对较小上下文或工具使用能力较弱模型的精简姿态。
    #[must_use]
    pub fn lean() -> Self {
        Self {
            kind: HarnessPostureKind::Lean,
            max_subagents: 20,
            prefer_codebase_search: true,
            compaction_strategy: HarnessCompactionStrategy::Aggressive,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    }
}

/// 测试框架配置文件将姿态绑定到供应商路由和模型模式。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HarnessProfile {
    /// 此配置文件适用的供应商路由，例如 "deepseek" 或 "xiaomi-mimo"。
    pub provider_route: String,
    /// 模型名称的正则或 glob 模式，例如 "deepseek-v4.*"。
    pub model_pattern: String,
    /// 要应用的姿态。
    #[serde(default)]
    pub posture: HarnessPosture,
}

impl HarnessProfile {
    /// 当此配置文件适用于供应商/模型路由时返回 true。
    ///
    /// 这是一个纯配置辅助函数：匹配配置文件不得改变运行时供应商选择、
    /// 提示、认证、工具、上下文或持久化配置。
    #[must_use]
    pub fn matches_route(&self, provider_route: &str, model: &str) -> bool {
        provider_routes_equal(&self.provider_route, provider_route)
            && wildcard_pattern_matches(&self.model_pattern, model)
    }
}

/// 常见供应商/模型系列的内置配置文件种子。
///
/// 用户配置的配置文件始终优先检查；这些种子仅在配置没有更精确的匹配时
/// 提供稳定的解析结果。
#[must_use]
pub fn built_in_harness_profiles() -> &'static [HarnessProfile] {
    static PROFILES: OnceLock<Vec<HarnessProfile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        vec![
            HarnessProfile {
                provider_route: "deepseek".to_string(),
                model_pattern: "deepseek-v4*".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
            HarnessProfile {
                provider_route: "xiaomi-mimo".to_string(),
                model_pattern: "mimo-v2.5*".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
            HarnessProfile {
                provider_route: "arcee".to_string(),
                model_pattern: "trinity-large-thinking".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
            HarnessProfile {
                provider_route: "huggingface".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "sglang".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "vllm".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "ollama".to_string(),
                model_pattern: "*".to_string(),
                posture: HarnessPosture::lean(),
            },
        ]
    })
}

fn provider_routes_equal(expected: &str, actual: &str) -> bool {
    match (ProviderKind::parse(expected), ProviderKind::parse(actual)) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => expected.trim().eq_ignore_ascii_case(actual.trim()),
    }
}

fn wildcard_pattern_matches(pattern: &str, value: &str) -> bool {
    wildcard_chars_match(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn wildcard_chars_match(pattern: &[char], value: &[char]) -> bool {
    let (mut pattern_idx, mut value_idx) = (0, 0);
    let mut star_idx: Option<usize> = None;
    let mut star_value_idx = 0;

    while value_idx < value.len() {
        if pattern_idx < pattern.len()
            && (pattern[pattern_idx] == '?' || pattern[pattern_idx] == value[value_idx])
        {
            pattern_idx += 1;
            value_idx += 1;
        } else if pattern_idx < pattern.len() && pattern[pattern_idx] == '*' {
            star_idx = Some(pattern_idx);
            pattern_idx += 1;
            star_value_idx = value_idx;
        } else if let Some(star) = star_idx {
            pattern_idx = star + 1;
            star_value_idx += 1;
            value_idx = star_value_idx;
        } else {
            return false;
        }
    }

    pattern[pattern_idx..].iter().all(|ch| *ch == '*')
}
