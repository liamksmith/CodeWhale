//! 引擎的上下文预算和提示词塑形辅助函数。
//!
//! 这些函数由流式轮询循环、容量流程和引擎会话维护代码共享。
//! 将它们放在这里可以防止顶级引擎模块积累不相关的上下文策略细节。

use crate::compaction::estimate_tokens;
use crate::config::ApiProvider;
use crate::context_budget::ContextBudget;
use crate::error_taxonomy::ErrorCategory;
use crate::models::{Message, SystemPrompt, context_window_for_model};
use crate::tools::spec::ToolResult;
use codewhale_config::route::RouteLimits;
use serde_json::Value;

/// 常规代理轮询请求的最大输出 token 数。故意设置得很大：
/// V4 思考模型可以在可见回复之前为困难提示产生数万个推理 token，
/// 而 DeepSeek V4 拥有 1M 上下文窗口。v0.7.5 保持此上限固定，
/// 而不是在压力下静默降低 `max_tokens`；硬循环/预检检查在发送
/// 下一个请求前保留此预算加上安全余量。
pub(super) const TURN_MAX_OUTPUT_TOKENS: u32 = 262_144;

/// API 请求中发送的安全最大输出 token 数。此值必须足够低，
/// 以便与上下文限制小于模型本机窗口的提供商兼容（例如，使用
/// `--max-model-len 131072` 的自托管 vLLM/SGLang）。
/// DeepSeek 的 API 仍将根据需要生成任意数量的 token 供思考；
/// 此上限只是防止来自限制严格的提供商的 HTTP 400。
const API_MAX_OUTPUT_TOKENS: u32 = 65_536;

/// 计算给定模型在 API 请求中发送的有效 `max_tokens`。使用
/// `API_MAX_OUTPUT_TOKENS`（64K），这适合通用提供商限制
///（128K+ 总计）。对于上下文窗口较小的非 V4 模型，上限为
/// 上下文窗口的一半。
///
/// 覆盖：当环境变量 `DEEPSEEK_MAX_OUTPUT_TOKENS` 设置为正整
/// 数时，此函数直接返回该值。用于自托管提供商（vLLM/SGLang），
/// 其 `max-model-len` 很紧张，上述模型表启发式方法会过度分配。
/// 示例：vLLM 以 `--max-model-len 65536` 提供 Qwen3.6 服务时，
/// 应设置 `DEEPSEEK_MAX_OUTPUT_TOKENS=16384`，以使输入 + 输出
/// 远低于提供商的硬限制。
pub(super) fn effective_max_output_tokens(model: &str) -> u32 {
    if let Ok(raw) = std::env::var("DEEPSEEK_MAX_OUTPUT_TOKENS")
        && let Ok(n) = raw.trim().parse::<u32>()
        && n > 0
    {
        return n;
    }
    let window = context_window_for_model(model).unwrap_or(128_000);
    if window >= 500_000 {
        // V4 类模型在大型上下文提供商上：使用 64K，这对大多数部署
        // 都是安全的，同时仍允许大量输出。
        API_MAX_OUTPUT_TOKENS
    } else {
        // 较小的模型：上限为上下文窗口的一半（为输入留出空间）
        let capped = window / 2;
        capped.min(API_MAX_OUTPUT_TOKENS)
    }
}

pub(super) fn effective_max_output_tokens_for_route(
    model: &str,
    route_limits: Option<RouteLimits>,
) -> u32 {
    let cap = effective_max_output_tokens(model);
    let cap = crate::route_budget::route_output_limit_tokens(route_limits)
        .map_or(cap, |route_cap| cap.min(route_cap));
    let Some(window) = route_limits
        .and_then(|limits| limits.context_tokens)
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
    else {
        return cap;
    };
    u32::try_from(ContextBudget::new(u64::from(window), 0, u64::from(cap)).output_cap_tokens)
        .unwrap_or(cap)
        .max(1)
}
/// 当需要紧急修剪时保留的最近消息数量。
pub(super) const MIN_RECENT_MESSAGES_TO_KEEP: usize = 4;
/// 在失败轮询前允许的紧急恢复尝试次数。
pub(super) const MAX_CONTEXT_RECOVERY_ATTEMPTS: u8 = 2;
/// 任何插入模型上下文的工具输出的硬上限。
const TOOL_RESULT_CONTEXT_HARD_LIMIT_CHARS: usize = 12_000;
/// 已知噪声工具插入模型上下文的软上限。
const TOOL_RESULT_CONTEXT_SOFT_LIMIT_CHARS: usize = 2_000;
/// 压缩工具输出到模型上下文时保留的片段长度。
const TOOL_RESULT_CONTEXT_SNIPPET_CHARS: usize = 900;
/// 工具输出插入大型上下文模型的硬上限。
const LARGE_CONTEXT_TOOL_RESULT_HARD_LIMIT_CHARS: usize = 48_000;
/// 已知噪声工具插入大型上下文模型的软上限。
const LARGE_CONTEXT_TOOL_RESULT_SOFT_LIMIT_CHARS: usize = 8_000;
/// 压缩大型上下文噪声输出时保留的片段长度。
const LARGE_CONTEXT_TOOL_RESULT_SNIPPET_CHARS: usize = 4_000;
/// 上下文窗口大小，超过该值时可以放宽工具输出限制。
const LARGE_CONTEXT_WINDOW_TOKENS: u32 = 500_000;
/// 从元数据提供的输出摘要中保留的最大字符数。
const TOOL_RESULT_METADATA_SUMMARY_CHARS: usize = 320;

pub(super) const COMPACTION_SUMMARY_MARKER: &str = "Conversation Summary (Auto-Generated)";

#[derive(Debug, Clone, Copy)]
struct ToolResultContextLimits {
    hard_limit_chars: usize,
    noisy_soft_limit_chars: usize,
    snippet_chars: usize,
}

pub(super) fn summarize_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let take = limit.saturating_sub(3);
    let mut out: String = text.chars().take(take).collect();
    out.push_str("...");
    out
}

fn summarize_text_head_tail(text: &str, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    if limit <= 20 {
        return summarize_text(text, limit);
    }

    let marker = "\n\n[... output truncated for context ...]\n\n";
    let marker_len = marker.chars().count();
    if limit <= marker_len + 20 {
        return summarize_text(text, limit);
    }

    let remaining = limit - marker_len;
    let head_len = remaining.saturating_mul(2) / 3;
    let tail_len = remaining.saturating_sub(head_len);
    let head: String = text.chars().take(head_len).collect();
    let tail_vec: Vec<char> = text.chars().rev().take(tail_len).collect();
    let tail: String = tail_vec.into_iter().rev().collect();
    format!("{head}{marker}{tail}")
}

fn tool_result_is_noisy(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_shell_cancel"
            | "task_shell_start"
            | "task_shell_wait"
            | "run_tests"
            | "run_verifiers"
            | "task_gate_run"
            | "multi_tool_use.parallel"
            | "web_search"
    )
}

fn tool_result_metadata_summary(metadata: Option<&serde_json::Value>) -> Option<String> {
    let obj = metadata?.as_object()?;
    for key in ["summary", "stdout_summary", "stderr_summary", "message"] {
        if let Some(text) = obj.get(key).and_then(serde_json::Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(summarize_text(trimmed, TOOL_RESULT_METADATA_SUMMARY_CHARS));
            }
        }
    }
    None
}

fn summarize_subagent_status(status: &serde_json::Value) -> String {
    if let Some(raw) = status.as_str() {
        return raw.to_string();
    }
    if let Some(obj) = status.as_object()
        && let Some((kind, value)) = obj.iter().next()
    {
        if let Some(reason) = value.as_str().filter(|s| !s.trim().is_empty()) {
            return format!("{kind}({})", summarize_text(reason.trim(), 120));
        }
        return kind.to_string();
    }
    status.to_string()
}

fn summarize_subagent_snapshot(snapshot: &serde_json::Value, index: usize) -> String {
    if let Some(inner) = snapshot.get("snapshot") {
        return summarize_subagent_snapshot(inner, index);
    }

    let Some(obj) = snapshot.as_object() else {
        return format!(
            "- item {index}: {}",
            summarize_text(&snapshot.to_string(), 240)
        );
    };

    let agent_id = obj
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let agent_type = obj
        .get("agent_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("agent");
    let status = obj
        .get("status")
        .map(summarize_subagent_status)
        .unwrap_or_else(|| "unknown".to_string());
    let objective = obj
        .get("assignment")
        .and_then(|assignment| assignment.get("objective"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| summarize_text(s, 220));
    let result = obj
        .get("result")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| summarize_text(s, 1_600));
    let steps = obj.get("steps_taken").and_then(serde_json::Value::as_u64);
    let duration_ms = obj.get("duration_ms").and_then(serde_json::Value::as_u64);

    let mut lines = vec![format!("- {agent_id} ({agent_type}) status={status}")];
    if let Some(objective) = objective {
        lines.push(format!("  objective: {objective}"));
    }
    match result {
        Some(result) => lines.push(format!("  result: {result}")),
        None => lines.push("  result: not available yet".to_string()),
    }
    if steps.is_some() || duration_ms.is_some() {
        let steps = steps
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".to_string());
        let duration_ms = duration_ms
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".to_string());
        lines.push(format!("  stats: steps={steps}, duration_ms={duration_ms}"));
    }
    lines.join("\n")
}

fn compact_subagent_tool_result_for_context(tool_name: &str, raw: &str) -> Option<String> {
    if tool_name != "agent" {
        return None;
    }

    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let snapshots: Vec<&serde_json::Value> = match &parsed {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![&parsed],
        _ => return None,
    };

    let mut out = String::from("[sub-agent result summarized for parent context]\n");
    out.push_str(
        "Child results are self-reports; verify side effects with tools like read_file or list_dir before claiming success.\n",
    );
    out.push_str("Use `handle_read` on `transcript_handle` for bounded transcript slices when the returned summary is not enough.\n");
    for (idx, snapshot) in snapshots.iter().enumerate() {
        if idx >= 8 {
            out.push_str(&format!(
                "- ... {} more sub-agent result(s) omitted from context summary\n",
                snapshots.len().saturating_sub(idx)
            ));
            break;
        }
        out.push_str(&summarize_subagent_snapshot(snapshot, idx + 1));
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

fn json_text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn json_number_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| {
            value
                .as_i64()
                .map(|n| n.to_string())
                .or_else(|| value.as_u64().map(|n| n.to_string()))
        })
        .or_else(|| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        })
}

fn compact_run_tests_result_for_context(raw: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let success = parsed.get("success")?.as_bool()?;
    let exit_code = json_number_text(&parsed, "exit_code").unwrap_or_else(|| "?".to_string());
    let command = json_text(&parsed, "command").unwrap_or("(unknown command)");
    let stdout = json_text(&parsed, "stdout");
    let stderr = json_text(&parsed, "stderr");
    let stream_limit = if success { 500 } else { 1_000 };

    let mut lines = vec![
        "[run_tests result summarized for context]".to_string(),
        format!(
            "status: {}, exit_code: {exit_code}",
            if success { "passed" } else { "failed" }
        ),
        format!("command: {}", summarize_text(command, 300)),
    ];
    if let Some(stderr) = stderr {
        lines.push(format!(
            "stderr: {}",
            summarize_text_head_tail(stderr, stream_limit)
        ));
    }
    if let Some(stdout) = stdout {
        lines.push(format!(
            "stdout: {}",
            summarize_text_head_tail(stdout, stream_limit)
        ));
    }
    Some(lines.join("\n"))
}

fn run_verifier_status_rank(status: Option<&str>) -> u8 {
    match status.unwrap_or_default() {
        "failed" | "timeout" => 0,
        "skipped" => 1,
        "passed" => 2,
        _ => 3,
    }
}

fn compact_run_verifiers_result_for_context(raw: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let gates = parsed.get("gates")?.as_array()?;
    let summary = json_text(&parsed, "summary")
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            let passed = json_number_text(&parsed, "passed").unwrap_or_else(|| "?".to_string());
            let failed = json_number_text(&parsed, "failed").unwrap_or_else(|| "?".to_string());
            let skipped = json_number_text(&parsed, "skipped").unwrap_or_else(|| "?".to_string());
            format!("{passed} passed, {failed} failed, {skipped} skipped")
        });

    let mut ordered: Vec<&Value> = gates.iter().collect();
    ordered.sort_by(|a, b| {
        run_verifier_status_rank(json_text(a, "status"))
            .cmp(&run_verifier_status_rank(json_text(b, "status")))
            .then_with(|| json_text(a, "name").cmp(&json_text(b, "name")))
    });

    let mut lines = vec![
        "[run_verifiers result summarized for context]".to_string(),
        format!("summary: {summary}"),
    ];
    let profile = json_text(&parsed, "profile");
    let level = json_text(&parsed, "level");
    if profile.is_some() || level.is_some() {
        lines.push(format!(
            "selection: profile={profile}, level={level}",
            profile = profile.unwrap_or("?"),
            level = level.unwrap_or("?")
        ));
    }

    for (idx, gate) in ordered.iter().enumerate() {
        if idx >= 12 {
            lines.push(format!(
                "- ... {} more gate(s) omitted from context summary",
                ordered.len().saturating_sub(idx)
            ));
            break;
        }

        let name = json_text(gate, "name").unwrap_or("gate");
        let ecosystem = json_text(gate, "ecosystem").unwrap_or("unknown");
        let status = json_text(gate, "status").unwrap_or("unknown");
        let exit = json_number_text(gate, "exit_code")
            .map(|code| format!(" exit={code}"))
            .unwrap_or_default();
        lines.push(format!("- {name} ({ecosystem}): {status}{exit}"));

        if status != "passed" {
            if let Some(command) = json_text(gate, "command") {
                lines.push(format!("  command: {}", summarize_text(command, 240)));
            }
            if let Some(detail) = json_text(gate, "skipped_reason")
                .or_else(|| json_text(gate, "stderr"))
                .or_else(|| json_text(gate, "stdout"))
            {
                lines.push(format!(
                    "  detail: {}",
                    summarize_text_head_tail(detail, 600)
                ));
            }
        }
    }

    Some(lines.join("\n"))
}

fn compact_task_gate_run_result_for_context(raw: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let gate = parsed.get("gate")?;
    let gate_name = json_text(gate, "gate").unwrap_or("gate");
    let status = json_text(gate, "status").unwrap_or("unknown");
    let command = json_text(gate, "command").unwrap_or("(unknown command)");
    let summary = json_text(gate, "summary")
        .or_else(|| json_text(&parsed, "stderr_summary"))
        .or_else(|| json_text(&parsed, "stdout_summary"));
    let exit = json_number_text(gate, "exit_code")
        .map(|code| format!(", exit_code: {code}"))
        .unwrap_or_default();

    let mut lines = vec![
        "[task_gate_run result summarized for context]".to_string(),
        format!("gate: {gate_name}, status: {status}{exit}"),
        format!("command: {}", summarize_text(command, 300)),
    ];
    if let Some(summary) = summary {
        lines.push(format!("summary: {}", summarize_text(summary, 800)));
    }
    if let Some(log_path) = json_text(gate, "log_path") {
        lines.push(format!("log_path: {log_path}"));
    }
    Some(lines.join("\n"))
}

fn compact_structured_tool_result_for_context(tool_name: &str, raw: &str) -> Option<String> {
    match tool_name {
        "run_tests" => compact_run_tests_result_for_context(raw),
        "run_verifiers" => compact_run_verifiers_result_for_context(raw),
        "task_gate_run" => compact_task_gate_run_result_for_context(raw),
        _ => None,
    }
}

fn tool_result_context_limits_for_model(model: &str) -> ToolResultContextLimits {
    let is_large_context =
        context_window_for_model(model).is_some_and(|window| window >= LARGE_CONTEXT_WINDOW_TOKENS);

    if is_large_context {
        ToolResultContextLimits {
            hard_limit_chars: LARGE_CONTEXT_TOOL_RESULT_HARD_LIMIT_CHARS,
            noisy_soft_limit_chars: LARGE_CONTEXT_TOOL_RESULT_SOFT_LIMIT_CHARS,
            snippet_chars: LARGE_CONTEXT_TOOL_RESULT_SNIPPET_CHARS,
        }
    } else {
        ToolResultContextLimits {
            hard_limit_chars: TOOL_RESULT_CONTEXT_HARD_LIMIT_CHARS,
            noisy_soft_limit_chars: TOOL_RESULT_CONTEXT_SOFT_LIMIT_CHARS,
            snippet_chars: TOOL_RESULT_CONTEXT_SNIPPET_CHARS,
        }
    }
}

pub(crate) fn compact_tool_result_for_context(
    model: &str,
    tool_name: &str,
    output: &ToolResult,
) -> String {
    let raw = output.content.trim();
    if raw.is_empty() {
        return String::new();
    }

    if let Some(summary) = compact_subagent_tool_result_for_context(tool_name, raw) {
        return summary;
    }

    if let Some(summary) = compact_structured_tool_result_for_context(tool_name, raw) {
        return summary;
    }

    let limits = tool_result_context_limits_for_model(model);
    let raw_chars = raw.chars().count();
    let should_compact = raw_chars > limits.hard_limit_chars
        || (tool_result_is_noisy(tool_name) && raw_chars > limits.noisy_soft_limit_chars);
    if !should_compact {
        return raw.to_string();
    }

    let snippet = summarize_text_head_tail(raw, limits.snippet_chars);
    let omitted = raw_chars.saturating_sub(snippet.chars().count());
    let summary = tool_result_metadata_summary(output.metadata.as_ref());

    if let Some(summary) = summary {
        format!(
            "[{tool_name} output compacted to protect context]\nSummary: {summary}\nSnippet: {snippet}\n(Original: {raw_chars} chars, omitted: {omitted} chars.)"
        )
    } else {
        format!(
            "[{tool_name} output compacted to protect context]\nSnippet: {snippet}\n(Original: {raw_chars} chars, omitted: {omitted} chars.)"
        )
    }
}

pub(super) fn extract_compaction_summary_prompt(
    prompt: Option<SystemPrompt>,
) -> Option<SystemPrompt> {
    match prompt {
        Some(SystemPrompt::Blocks(blocks)) => {
            let summary_blocks: Vec<_> = blocks
                .into_iter()
                .filter(|block| block.text.contains(COMPACTION_SUMMARY_MARKER))
                .collect();
            if summary_blocks.is_empty() {
                None
            } else {
                Some(SystemPrompt::Blocks(summary_blocks))
            }
        }
        Some(SystemPrompt::Text(text)) => {
            if text.contains(COMPACTION_SUMMARY_MARKER) {
                Some(SystemPrompt::Text(text))
            } else {
                None
            }
        }
        None => None,
    }
}

#[allow(dead_code)] // 为将来的引擎侧调用方暴露；当前调用路径通过 compaction::estimate_input_tokens_conservative 经过 token_estimate_cache。
fn estimate_text_tokens_conservative(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

#[allow(dead_code)] // 见上方的 estimate_text_tokens_conservative
fn estimate_system_tokens_conservative(system: Option<&SystemPrompt>) -> usize {
    match system {
        Some(SystemPrompt::Text(text)) => estimate_text_tokens_conservative(text),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| estimate_text_tokens_conservative(&block.text))
            .sum(),
        None => 0,
    }
}

#[allow(dead_code)] // 见上方的 estimate_text_tokens_conservative
pub(super) fn estimate_input_tokens_conservative(
    messages: &[Message],
    system: Option<&SystemPrompt>,
) -> usize {
    let message_tokens = estimate_tokens(messages).saturating_mul(3).div_ceil(2);
    let system_tokens = estimate_system_tokens_conservative(system);
    let framing_overhead = messages.len().saturating_mul(12).saturating_add(48);
    message_tokens
        .saturating_add(system_tokens)
        .saturating_add(framing_overhead)
}

/// 上下文窗口在此大小或以上时，在计算内部输入预算时保留完整的
/// [`TURN_MAX_OUTPUT_TOKENS`]（262K），为 V4 类交织思考留出空间。
/// 低于此值时，回退到 [`effective_max_output_tokens`]，以便较小的
/// 自托管窗口不会下溢为负预算。
const INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD: u32 = 500_000;

/// 提供商/模型路由的内部输入侧 token 预算：
/// `window - reserved_output - headroom`。由预检检查、
/// 紧急恢复和容量修剪使用，以决定何时进行压缩。
/// 未知模型 ID 回退到提供商的保守默认值，而不是禁用预检；
/// 自定义长上下文部署仍可以使用 `-256k`/`-1024k` 模型后缀
/// 来广告其窗口。
///
/// 保留的输出项取决于窗口大小：
///   * `window >= 500K`（V4 类大型上下文）→ [`TURN_MAX_OUTPUT_TOKENS`]
///     （262K）。保留"为交织思考留出空间"的约定。
///   * `window < 500K`（较小/自托管，例如 256K vLLM Qwen 窗口）
///     → [`effective_max_output_tokens`]，即 API 实际上限的输出。
///     如果在此处保留完整的 262K，将计算出
///     `256K - 262K - 1K`，这会使 `checked_sub` 下溢为 `None`，
///     并*静默禁用每个预检和紧急恢复路径*——然后会话将一直运行
///     直到提供商因上下文长度硬拒绝。
#[cfg(test)]
pub(super) fn context_input_budget_for_provider(
    provider: ApiProvider,
    model: &str,
) -> Option<usize> {
    context_input_budget_for_route(provider, model, None, 0)
}

/// 公开以便外部调用方（例如，派生自己的压缩触发线的宿主/桥接）可以
/// 重用*完全相同的*内部输入预算计算——窗口减去与窗口相关的输出保留
///（`route_output_reservation_for_window`，它编码了 ≥500K→262K 与
/// 较小窗口的区分）再减去余量——而不是重新派生这些常量并静默偏离引擎。
/// 传递 `input_tokens = 0` 以获取路由的完整紧急输入预算。
pub fn context_input_budget_for_route(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    input_tokens: usize,
) -> Option<usize> {
    route_context_budget_for_route(provider, model, route_limits, input_tokens)
        .and_then(|budget| usize::try_from(budget.available_input_tokens).ok())
}

#[cfg(test)]
pub(super) fn route_context_budget_for_provider(
    provider: ApiProvider,
    model: &str,
    input_tokens: usize,
) -> Option<ContextBudget> {
    route_context_budget_for_route(provider, model, None, input_tokens)
}

pub(super) fn route_context_budget_for_route(
    provider: ApiProvider,
    model: &str,
    route_limits: Option<RouteLimits>,
    input_tokens: usize,
) -> Option<ContextBudget> {
    let window = crate::route_budget::route_context_window_tokens(provider, model, route_limits);
    let output_cap = route_output_reservation_for_window(model, window, route_limits);
    crate::route_budget::route_context_budget(
        provider,
        model,
        route_limits,
        input_tokens,
        output_cap,
    )
}

fn route_output_reservation_for_window(
    model: &str,
    window_tokens: u32,
    route_limits: Option<RouteLimits>,
) -> u32 {
    if let Some(route_cap) = crate::route_budget::route_output_limit_tokens(route_limits) {
        return route_cap.min(TURN_MAX_OUTPUT_TOKENS);
    }
    if window_tokens >= INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD {
        TURN_MAX_OUTPUT_TOKENS
    } else {
        effective_max_output_tokens(model)
    }
}

pub(super) fn is_context_length_error_message(message: &str) -> bool {
    crate::error_taxonomy::classify_error_message(message) == ErrorCategory::InvalidInput
}
