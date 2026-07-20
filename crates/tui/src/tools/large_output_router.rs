//! 工具结果的大输出路由（issue #548）。
//!
//! 任何估计 Token 数超过配置阈值的工具结果在到达父上下文之前
//! 会被此处拦截。一个轻量级的 V4-Flash 合成子代理会浓缩原始输出；
//! 只有合成结果返回给父代理。原始内容存储在 workshop 变量
//! `last_tool_result` 中，以便父代理后续如果需要完整文本可以调用
//! `promote_to_context`。
//!
//! 按工具设置的阈值可以覆盖全局默认值。单个工具调用可以传递
//! `raw=true` 以完全绕过路由。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::tools::spec::ToolResult;

// ── 常量 ──────────────────────────────────────────────────────────────────

/// 工具结果通过 workshop 路由的默认 Token 阈值。
/// 匹配 issue 规范中的 4 096 个 Token。
pub const DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS: usize = 4_096;

/// 启发式估算使用的近似字符/Token 比例。
/// 我们有意选择保守值（3 字符/Token），以便在路由时偏向
/// 主动路由而不是将原始数据倾倒入父上下文。
const CHARS_PER_TOKEN_ESTIMATE: usize = 3;

/// 存储原始工具输出的 Workshop 变量名。
pub const WORKSHOP_LAST_TOOL_RESULT_VAR: &str = "last_tool_result";

// ── 配置 ─────────────────────────────────────────────────────────────

/// `config.toml` 中的 `[workshop]` 部分。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkshopConfig {
    /// 工具结果通过 workshop 合成子代理路由的 Token 阈值。
    /// 默认值：[`DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS`]。
    #[serde(default)]
    pub large_output_threshold_tokens: Option<usize>,

    /// 按工具设置的阈值覆盖（工具名称 → Token 限制）。
    /// 名称出现在此处的工具使用此限制而不是
    /// `large_output_threshold_tokens`。
    #[serde(default)]
    pub per_tool_thresholds: Option<HashMap<String, usize>>,
}

impl WorkshopConfig {
    /// 解析给定工具名称的有效阈值。
    #[must_use]
    pub fn threshold_for(&self, tool_name: &str) -> usize {
        if let Some(per_tool) = self.per_tool_thresholds.as_ref()
            && let Some(&limit) = per_tool.get(tool_name)
        {
            return limit;
        }
        self.large_output_threshold_tokens
            .unwrap_or(DEFAULT_LARGE_OUTPUT_THRESHOLD_TOKENS)
    }
}

// ── Token 估算 ──────────────────────────────────────────────────────────

/// 使用字符计数启发式方法估算 `text` 中的 Token 数。
///
/// 这避免了对真实 Tokenizer 的依赖；该估算有意保守（少算 Token），
/// 以便我们主动路由而不是让 5K Token 的块溜过去。
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    // 向上取整：最后一个不完整的 Token 仍然花费一个 Token。
    chars.div_ceil(CHARS_PER_TOKEN_ESTIMATE)
}

// ── 路由器 ────────────────────────────────────────────────────────────────────

/// [`LargeOutputRouter::route`] 返回的决策。
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// 输出足够小；直接通过，不做修改。
    PassThrough,
    /// 输出超过阈值，已（或应）被合成。
    Synthesise {
        /// 原始输出的估计 Token 数。
        estimated_tokens: usize,
        /// 被突破的阈值。
        threshold: usize,
    },
}

/// 拦截工具结果并将大的结果通过 workshop 路由。
///
/// 该类型有意为 `Clone` 和 `Default`，以便可以廉价地嵌入
/// [`ToolContext`](crate::tools::spec::ToolContext) 中，
/// 而无需 `Arc` 包装。
#[derive(Debug, Clone, Default)]
pub struct LargeOutputRouter {
    config: WorkshopConfig,
}

impl LargeOutputRouter {
    /// 从解析后的 workshop 配置构造路由器。
    #[must_use]
    pub fn new(config: WorkshopConfig) -> Self {
        Self { config }
    }

    /// 判断 `tool_name` 的 `result` 是否应被合成。
    ///
    /// 当工具调用包含 `raw = true` 时传递 `raw_bypass = true`。
    #[must_use]
    pub fn route(&self, tool_name: &str, result: &ToolResult, raw_bypass: bool) -> RouteDecision {
        if raw_bypass || !result.success {
            return RouteDecision::PassThrough;
        }
        let threshold = self.config.threshold_for(tool_name);
        let estimated_tokens = estimate_tokens(&result.content);
        if estimated_tokens > threshold {
            RouteDecision::Synthesise {
                estimated_tokens,
                threshold,
            }
        } else {
            RouteDecision::PassThrough
        }
    }

    /// 构建发送给 V4-Flash workshop 子代理的合成提示词。
    ///
    /// 提示词有意简洁——Flash 是快速模型，我们只需要
    /// 忠实的摘要，而不是深度推理。
    ///
    /// 这是后续（当异步 Flash 客户端从注册表层调用安全后）
    /// 接入的实时 LLM 合成调用的构建块。该方法为公开，
    /// 以便此 crate 外部的调用者可以对提示词形状进行单元测试。
    #[must_use]
    #[allow(dead_code)] // 由未来的 Flash 合成调用使用；保留以保持 API 稳定性
    pub fn synthesis_prompt(tool_name: &str, raw_output: &str, estimated_tokens: usize) -> String {
        format!(
            "You are a synthesis assistant. The tool `{tool_name}` produced {estimated_tokens} tokens \
             of output that is too large to include directly in the parent context.\n\n\
             Summarise the output below into a concise, faithful synthesis of ≤ 800 words. \
             Preserve key facts, numbers, file paths, error messages, and any actionable \
             information. Do NOT add commentary or interpretation beyond what is in the source.\n\n\
             <raw_tool_output>\n{raw_output}\n</raw_tool_output>"
        )
    }

    /// 用 workshop 来源头信息和对已存储原始输出的提示包装合成结果。
    #[must_use]
    pub fn wrap_synthesis(
        tool_name: &str,
        synthesis: &str,
        estimated_tokens: usize,
        threshold: usize,
    ) -> String {
        format!(
            "[workshop-synthesis: tool={tool_name}, raw_tokens≈{estimated_tokens}, \
             threshold={threshold}, raw_stored_in={WORKSHOP_LAST_TOOL_RESULT_VAR}]\n\n{synthesis}"
        )
    }
}

// ── Workshop 变量存储 ───────────────────────────────────────────────────

/// 进程内存储，用于在会话内跨工具调用持久化的 workshop 变量。
/// 今天暴露的唯一变量是 `last_tool_result`，它保存最近一次
/// 为 `promote_to_context` 路由的原始大工具输出。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkshopVariables {
    /// 最近一次通过 workshop 路由的大型工具输出的原始内容。
    /// 当未发生路由时为空字符串。
    #[serde(default)]
    pub last_tool_result: String,

    /// 产生 `last_tool_result` 的工具名称。
    #[serde(default)]
    pub last_tool_name: String,
}

impl WorkshopVariables {
    /// 存储来自大型工具路由事件的原始输出。
    pub fn store_raw(&mut self, tool_name: &str, raw: &str) {
        self.last_tool_result = raw.to_string();
        self.last_tool_name = tool_name.to_string();
    }

    /// 检索并清除存储的原始输出（消费语义，防止变量被意外提升两次）。
    ///
    /// 由 `promote_to_context` 工具调用（此 PR 中尚未接入）。
    #[must_use]
    #[allow(dead_code)] // 由后续的 promote_to_context 工具使用
    pub fn take_raw(&mut self) -> Option<(String, String)> {
        if self.last_tool_result.is_empty() {
            return None;
        }
        let content = std::mem::take(&mut self.last_tool_result);
        let name = std::mem::take(&mut self.last_tool_name);
        Some((name, content))
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(content: &str) -> ToolResult {
        ToolResult::success(content.to_string())
    }

    #[test]
    fn pass_through_below_threshold() {
        let router = LargeOutputRouter::default();
        let small = "x".repeat(100);
        let result = make_result(&small);
        assert_eq!(
            router.route("read_file", &result, false),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn synthesise_above_threshold() {
        let router = LargeOutputRouter::default();
        // 默认阈值 = 4096 tokens；3 字符/token → 4096*3 = 12288 字符
        let big = "a".repeat(13_000);
        let result = make_result(&big);
        assert!(matches!(
            router.route("read_file", &result, false),
            RouteDecision::Synthesise { .. }
        ));
    }

    #[test]
    fn raw_bypass_skips_routing() {
        let router = LargeOutputRouter::default();
        let big = "a".repeat(13_000);
        let result = make_result(&big);
        // raw=true → 始终直通，无论大小
        assert_eq!(
            router.route("exec_shell", &result, true),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn error_results_always_pass_through() {
        let router = LargeOutputRouter::default();
        let big = "error: ".repeat(2_000);
        let result = ToolResult::error(big);
        assert_eq!(
            router.route("exec_shell", &result, false),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn per_tool_threshold_override() {
        let mut per_tool = HashMap::new();
        per_tool.insert("grep_files".to_string(), 100); // 非常低
        let config = WorkshopConfig {
            large_output_threshold_tokens: Some(4096),
            per_tool_thresholds: Some(per_tool),
        };
        let router = LargeOutputRouter::new(config);
        // 100 tokens * 3 = 300 字符 → 400 字符即可触发
        let medium = "b".repeat(400);
        let result = make_result(&medium);
        assert!(matches!(
            router.route("grep_files", &result, false),
            RouteDecision::Synthesise { .. }
        ));
        // 其他工具仍使用全局阈值
        assert_eq!(
            router.route("read_file", &result, false),
            RouteDecision::PassThrough
        );
    }

    #[test]
    fn estimate_tokens_conservative() {
        // 9 字符 → ceil(9/3) = 3 tokens
        assert_eq!(estimate_tokens("123456789"), 3);
        // 10 字符 → ceil(10/3) = 4 tokens
        assert_eq!(estimate_tokens("1234567890"), 4);
        // 空字符串
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn workshop_variables_store_and_take() {
        let mut vars = WorkshopVariables::default();
        assert!(vars.take_raw().is_none());

        vars.store_raw("read_file", "raw content here");
        let taken = vars.take_raw().expect("should have content");
        assert_eq!(taken.0, "read_file");
        assert_eq!(taken.1, "raw content here");

        // 第二次取为空——消费语义
        assert!(vars.take_raw().is_none());
    }

    #[test]
    fn wrap_synthesis_includes_provenance_header() {
        let wrapped = LargeOutputRouter::wrap_synthesis("web_search", "key facts here", 5000, 4096);
        assert!(wrapped.contains("workshop-synthesis"));
        assert!(wrapped.contains("web_search"));
        assert!(wrapped.contains("5000"));
        assert!(wrapped.contains("key facts here"));
    }
}
