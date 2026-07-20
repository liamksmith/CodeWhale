//! 工具分发——每回合工具批次的规划/执行辅助函数。
//!
//! 从 `core/engine.rs`（P1.3）提取。高级排序逻辑仍然
//! 位于 `Engine::handle_deepseek_turn` 中；此模块拥有：
//!
//! * 将流式缓冲区解析为最终的 `serde_json::Value` 工具输入
//!   （`final_tool_input`、`parse_tool_input`、围栏/JSON 片段辅助函数）。
//! * `multi_tool_use.parallel` 负载解析器。
//! * 回合循环咨询的策略谓词——何时批处理可以并行运行、
//!   何时 `update_plan` 步骤应停止回合、何时 Plan 提示应强制
//!   先做计划、以及少量安全的只读 MCP 工具可以并行运行。
//! * 批处理驱动程序传递的工具执行计划/结果类型。
//!
//! 所有项仅限于 `pub(super)`：公开的引擎表面（Op/Event、
//! `EngineHandle`、`spawn_engine`）保持在 `core/engine.rs` 中。

use serde_json::json;

use crate::models::{Tool, ToolCaller};
use crate::tools::spec::{ToolError, ToolResult};
use crate::tui::app::AppMode;

use super::ToolUseState;

// === 类型 ============================================================

#[allow(dead_code)] // `index` 镜像批处理顺序，用于诊断的人体工程学。
pub(super) struct ToolExecOutcome {
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) started_at: std::time::Instant,
    pub(super) result: Result<ToolResult, ToolError>,
}

#[derive(Debug, Clone)]
pub(super) struct ToolExecutionPlan {
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) caller: Option<ToolCaller>,
    pub(super) interactive: bool,
    pub(super) approval_required: bool,
    pub(super) approval_description: String,
    pub(super) approval_force_prompt: bool,
    pub(super) supports_parallel: bool,
    pub(super) read_only: bool,
    pub(super) detached_start: bool,
    pub(super) blocked_error: Option<ToolError>,
    pub(super) guard_result: Option<ToolResult>,
}

pub(super) enum ToolExecutionBatch {
    Parallel(Vec<ToolExecutionPlan>),
    Serial(Box<ToolExecutionPlan>),
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ParallelToolResultEntry {
    pub(super) tool_name: String,
    pub(super) success: bool,
    pub(super) content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ParallelToolResult {
    pub(super) results: Vec<ParallelToolResultEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolApprovalStamp {
    ApprovedByUser,
    ApprovedWithPolicy,
}

impl ToolApprovalStamp {
    fn decision(self) -> &'static str {
        match self {
            Self::ApprovedByUser => "approved_by_user",
            Self::ApprovedWithPolicy => "approved_with_policy",
        }
    }

    fn model_visible_note(self) -> &'static str {
        match self {
            Self::ApprovedByUser => {
                "[approval] 此工具调用需要审批并已在执行前由用户批准。"
            }
            Self::ApprovedWithPolicy => {
                "[approval] 此工具调用需要审批并已在执行前由用户使用调整后的执行策略批准。"
            }
        }
    }
}

pub(super) fn stamp_tool_result_approval(result: &mut ToolResult, approval: ToolApprovalStamp) {
    let approval_metadata = json!({
        "required": true,
        "decision": approval.decision(),
        "model_visible": true,
    });
    let metadata = result.metadata.get_or_insert_with(|| json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert("approval".to_string(), approval_metadata);
    } else {
        let prior = std::mem::replace(metadata, json!({}));
        if let Some(object) = metadata.as_object_mut() {
            object.insert("_prior".to_string(), prior);
            object.insert("approval".to_string(), approval_metadata);
        }
    }

    let note = approval.model_visible_note();
    if result.content.starts_with("[approval] ") {
        return;
    }
    if result.content.is_empty() {
        result.content = note.to_string();
    } else {
        result.content = format!("{note}\n\n{}", result.content);
    }
}

// 在工具执行期间持有锁保护。内部守卫为 RAII 目的而持有（守卫被丢弃时释放）。
pub(super) enum ToolExecGuard<'a> {
    Read(#[allow(dead_code)] tokio::sync::RwLockReadGuard<'a, ()>),
    Write(#[allow(dead_code)] tokio::sync::RwLockWriteGuard<'a, ()>),
}

// === 调用者策略和错误 ==========================================

pub(super) fn caller_type_for_tool_use(caller: Option<&ToolCaller>) -> &str {
    caller.map_or("direct", |c| c.caller_type.as_str())
}

pub(super) fn caller_allowed_for_tool(
    caller: Option<&ToolCaller>,
    tool_def: Option<&Tool>,
) -> bool {
    let requested = caller_type_for_tool_use(caller);
    if let Some(def) = tool_def
        && let Some(allowed) = &def.allowed_callers
    {
        if allowed.is_empty() {
            return requested == "direct";
        }
        return allowed.iter().any(|item| item == requested);
    }
    requested == "direct"
}

/// "mode"/"modes" 的完整词检查——一个简单的 `contains("mode")` 也会
/// 匹配 "model"，使提供商模型错误跳过可操作提示后缀（#3020）。
fn mentions_mode_word(lower: &str) -> bool {
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == "mode" || word == "modes")
}

pub(super) fn format_tool_error(err: &ToolError, tool_name: &str) -> String {
    let message = match err {
        ToolError::InvalidInput { message } => {
            format!("工具 '{tool_name}' 的输入无效: {message}")
        }
        ToolError::MissingField { field } => {
            format!("工具 '{tool_name}' 缺少必需字段 '{field}'")
        }
        ToolError::PathEscape { path } => format!(
            "路径逃逸工作空间: {}。请使用工作空间相对路径或启用信任模式。",
            path.display()
        ),
        ToolError::ExecutionFailed { message } => message.clone(),
        ToolError::Timeout { seconds } => format!(
            "工具 '{tool_name}' 在 {seconds} 秒后超时。尝试更窄的范围或更长的超时时间。"
        ),
        ToolError::NotAvailable { message } => {
            let lower = message.to_ascii_lowercase();
            // #3020：透传已包含原因（模式切换、allow_shell、特性标志）的自解释消息。
            // 避免在已给出恢复路径的 "switch to Act mode" 之上
            // 追加冲突的 "Check mode, feature flags" 后缀。
            if lower.contains("current tool catalog")
                || lower.contains("did you mean:")
                || mentions_mode_word(&lower)
                || lower.contains("allow_shell")
                || lower.contains("feature flag")
            {
                message.clone()
            } else {
                format!(
                    "工具 '{tool_name}' 不可用: {message}。检查模式、特性标志或工具名称。"
                )
            }
        }
        ToolError::PermissionDenied { message } => {
            let lower = message.to_ascii_lowercase();
            // #3020：透传已命名拒绝原因的消息。
            if mentions_mode_word(&lower)
                || lower.contains("allow_shell")
                || lower.contains("denied by user")
            {
                message.clone()
            } else {
                format!(
                    "工具 '{tool_name}' 被拒绝: {message}。调整审批模式或请求权限。"
                )
            }
        }
    };

    with_transient_tool_fallback_hint(message, err, tool_name)
}

fn with_transient_tool_fallback_hint(message: String, err: &ToolError, tool_name: &str) -> String {
    if message_already_has_recovery_hint(&message) {
        return message;
    }

    let Some(hint) = transient_tool_fallback_hint(err, tool_name, &message) else {
        return message;
    };

    format!("{message} 回退方案: {hint}")
}

fn message_already_has_recovery_hint(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("recovery:") || lower.contains("fallback:")
}

fn transient_tool_fallback_hint(
    err: &ToolError,
    tool_name: &str,
    formatted_message: &str,
) -> Option<&'static str> {
    if !is_transient_tool_failure(err, formatted_message) {
        return None;
    }

    let lower_tool = tool_name.to_ascii_lowercase();
    if lower_tool.contains("web_search")
        || lower_tool.contains("web_run")
        || lower_tool == "web.run"
    {
        return Some(
            "重试一次后，切换到直接的 URL/open/fetch 路径或缓存上下文，而不是重复相同的搜索。",
        );
    }

    if lower_tool.contains("fetch_url") {
        return Some(
            "重试一次后，尝试更窄的 URL/来源、使用搜索结果或缓存上下文，或说明访问限制，而不是重复相同的请求。",
        );
    }

    if lower_tool.contains("file_search") || lower_tool.contains("grep") {
        return Some(
            "重试一次后，缩小查询/路径或直接检查可能文件，而不是不变地重复相同的搜索。",
        );
    }

    if lower_tool.contains("exec_shell")
        || lower_tool.contains("run_tests")
        || lower_tool.contains("run_verifiers")
    {
        return Some(
            "重试一次后，缩小命令/范围、仅为预期长时间运行增加超时，或切换到文件级别证据。",
        );
    }

    if lower_tool.contains("agent") {
        return Some(
            "重试一次后，减少委派范围或在父上下文继续，而不是重复生成相同代理。",
        );
    }

    Some(
        "重试一次后，选择不同工具或更窄策略，而不是不变地重复相同调用。",
    )
}

fn is_transient_tool_failure(err: &ToolError, formatted_message: &str) -> bool {
    if matches!(err, ToolError::Timeout { .. }) {
        return true;
    }

    if !matches!(err, ToolError::ExecutionFailed { .. }) {
        return false;
    }

    let lower = formatted_message.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "request failed",
        "connection",
        "network",
        "http 429",
        "rate limit",
        "http 5",
        "anti-bot",
        "captcha",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

// === 流式缓冲区解析 =========================================

/// 将流式 `ToolUseState` 提升为最终 JSON 输入。
///
/// 优先级顺序：
///
///   1. `input_buffer`（原始流式增量拼接）——解析为 JSON。
///      这是最权威的，因为它是模型实际发出的内容。
///   2. `input`（每个增量的尽力而为解析镜像）——当缓冲区为空时使用
///      （预流式工具调用走此路径）。
///   3. `input_buffer` 非空但无法解析→回退到 `input`
///      （每增量解析器已将最新的有效部分解析镜像到 `tool_state.input` 中）。
pub(super) fn final_tool_input(state: &ToolUseState) -> serde_json::Value {
    if state.input_parse_error.is_some() {
        return malformed_tool_arguments_input(&state.input_buffer);
    }
    if !state.input_buffer.trim().is_empty()
        && let Some(parsed) = parse_tool_input(&state.input_buffer)
    {
        return parsed;
    }
    state.input.clone()
}

pub(super) fn parse_tool_input(buffer: &str) -> Option<serde_json::Value> {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 首先尝试确定性参数修复阶梯（处理尾随逗号、未闭合花括号、嵌入控制字符等）。
    if let Ok(value) = crate::tools::arg_repair::repair(trimmed) {
        return Some(value);
    }
    // 回退到针对代码围栏、双重编码和修复阶梯未覆盖的片段提取模式的现有策略。
    if let Some(stripped) = strip_code_fences(trimmed)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&stripped)
    {
        return Some(value);
    }
    if let Ok(serde_json::Value::String(inner)) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&inner)
    {
        return Some(value);
    }
    extract_json_segment(trimmed)
        .and_then(|segment| serde_json::from_str::<serde_json::Value>(&segment).ok())
}

pub(super) fn malformed_tool_arguments_input(buffer: &str) -> serde_json::Value {
    json!({ "raw_arguments": buffer })
}

pub(super) fn malformed_tool_arguments_error(buffer: &str) -> String {
    format!("模型的工具参数格式错误：预期有效 JSON，收到 {buffer:?}")
}

fn strip_code_fences(text: &str) -> Option<String> {
    if !text.contains("```") {
        return None;
    }
    let line_count = text.lines().count();
    let mut lines = Vec::with_capacity(line_count);
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            continue;
        }
        lines.push(line);
    }
    let stripped = lines.join("\n");
    let stripped = stripped.trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn extract_json_segment(text: &str) -> Option<String> {
    extract_balanced_segment(text, '{', '}').or_else(|| extract_balanced_segment(text, '[', ']'))
}

fn extract_balanced_segment(text: &str, open: char, close: char) -> Option<String> {
    let start = text.find(open)?;
    let mut depth = 0i32;
    let mut end = None;
    for (offset, ch) in text[start..].char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                end = Some(start + offset + ch.len_utf8());
                break;
            }
        }
    }
    end.map(|end_idx| text[start..end_idx].to_string())
}

fn normalize_parallel_tool_name(raw: &str) -> String {
    let mut name = raw.trim();
    for prefix in ["functions.", "tools.", "tool."] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped;
            break;
        }
    }
    name.to_string()
}

pub(super) fn parse_parallel_tool_calls(
    input: &serde_json::Value,
) -> Result<Vec<(String, serde_json::Value)>, ToolError> {
    let tool_uses = input
        .get("tool_uses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::missing_field("tool_uses"))?;
    if tool_uses.is_empty() {
        return Err(ToolError::invalid_input(
            "multi_tool_use.parallel 需要至少一个工具调用",
        ));
    }

    let mut calls = Vec::with_capacity(tool_uses.len());
    for item in tool_uses {
        let name = item
            .get("recipient_name")
            .or_else(|| item.get("tool_name"))
            .or_else(|| item.get("name"))
            .or_else(|| item.get("tool"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("recipient_name"))?;
        let params = item
            .get("parameters")
            .or_else(|| item.get("input"))
            .or_else(|| item.get("args"))
            .or_else(|| item.get("arguments"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        calls.push((normalize_parallel_tool_name(name), params));
    }

    Ok(calls)
}

// === 分发策略 ==================================================

#[cfg(test)]
pub(super) fn should_parallelize_tool_batch(plans: &[ToolExecutionPlan]) -> bool {
    !plans.is_empty() && plans.iter().all(tool_plan_can_join_parallel_batch)
}

pub(super) fn tool_plan_is_parallel_safe(plan: &ToolExecutionPlan) -> bool {
    plan.read_only && plan.supports_parallel && !plan.approval_required && !plan.interactive
}

pub(super) fn tool_plan_can_join_parallel_batch(plan: &ToolExecutionPlan) -> bool {
    plan.blocked_error.is_none()
        && (tool_plan_is_parallel_safe(plan)
            || (plan.detached_start && !plan.approval_required && !plan.interactive))
}

pub(super) fn plan_tool_execution_batches(
    plans: Vec<ToolExecutionPlan>,
) -> Vec<ToolExecutionBatch> {
    let mut batches = Vec::new();
    let mut parallel_chunk = Vec::new();

    for plan in plans {
        if tool_plan_can_join_parallel_batch(&plan) {
            parallel_chunk.push(plan);
            continue;
        }

        if !parallel_chunk.is_empty() {
            batches.push(ToolExecutionBatch::Parallel(std::mem::take(
                &mut parallel_chunk,
            )));
        }
        batches.push(ToolExecutionBatch::Serial(Box::new(plan)));
    }

    if !parallel_chunk.is_empty() {
        batches.push(ToolExecutionBatch::Parallel(parallel_chunk));
    }

    batches
}

pub(super) fn should_stop_after_plan_tool(
    mode: AppMode,
    tool_name: &str,
    result: &Result<ToolResult, ToolError>,
) -> bool {
    mode == AppMode::Plan && tool_name == "update_plan" && result.is_ok()
}

pub(super) fn should_force_update_plan_first(mode: AppMode, content: &str) -> bool {
    if mode != AppMode::Plan {
        return false;
    }

    let lower = content.to_ascii_lowercase();
    // 仅快捷处理真正轻量级的计划请求。裸的"make a plan"措辞
    // 通常用于仓库/版本/构建工作，其中 Plan 模式在发布交接工件前
    // 仍需要检查可用上下文。
    let asks_for_direct_plan = [
        "quick plan",
        "short plan",
        "simple plan",
        "3-step plan",
        "3 step plan",
        "three-step plan",
        "three step plan",
        "high-level plan",
        "high level plan",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if !asks_for_direct_plan {
        return false;
    }

    let asks_for_repo_exploration = [
        "inspect the repo",
        "inspect the code",
        "explore the repo",
        "search the repo",
        "read the code",
        "review the code",
        "analyze the code",
        "investigate",
        "figure out",
        "figuring out",
        "look through",
        "understand the current",
        "current state",
        "ground it in the codebase",
        "based on the codebase",
        "repo",
        "codebase",
        "version",
        "ver ",
        "release",
        "build",
        "benchmark",
        "api server",
        "github.com",
        "http://",
        "https://",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    !asks_for_repo_exploration
}

pub(super) fn mcp_tool_is_parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
}

pub(super) fn mcp_tool_is_read_only(name: &str) -> bool {
    matches!(
        name,
        "list_mcp_resources"
            | "list_mcp_resource_templates"
            | "mcp_read_resource"
            | "read_mcp_resource"
            | "mcp_get_prompt"
    )
}

pub(super) fn mcp_tool_approval_description(name: &str) -> String {
    if mcp_tool_is_read_only(name) {
        format!("只读 MCP 工具 '{name}'")
    } else {
        format!("MCP 工具 '{name}' 可能有副作用")
    }
}
