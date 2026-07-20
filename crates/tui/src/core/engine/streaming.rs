//! 流式响应状态和护栏。
//!
//! 此模块拥有解码一个模型流时使用的本地状态：
//! 内容块类型跟踪、流式工具调用缓冲区、透明重试策略
//! 以及对看起来像伪造的工具调用包装器的文本的清理器。

use crate::models::ToolCaller;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContentBlockKind {
    Text,
    Thinking,
    ToolUse,
}

#[derive(Debug, Clone)]
pub(super) struct ToolUseState {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) caller: Option<ToolCaller>,
    pub(super) input_buffer: String,
    pub(super) input_parse_error: Option<String>,
}

/// 终止流之前文本/思考内容的最大总字节数。
pub(super) const STREAM_MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
/// 流总挂钟时间的合理性兜底。**不是**常规的终止开关——流块空闲超时
/// 是主要的卡顿检测器。挂钟上限仅用于限制病态情况
///（例如服务器持续发送心跳但没有任何进展）。
///
/// 历史背景：这曾经是 300 秒（5 分钟），但过于激进了——V4
/// 在硬提示词上的思考轮次完全可能超过 5 分钟挂钟时间，
/// 同时整个过程都在发出 reasoning_content 块。在 v0.6.6 中，
/// 长推理轮次撞到旧上限后调整为 30 分钟。Codex 默认使用
/// 每块 300 秒的空闲超时且没有挂钟上限；我们保留两层但给
/// 挂钟一个宽松的窗口，使其在实践中永远不会触发。
pub(super) const STREAM_MAX_DURATION_SECS: u64 = 1800; // 30 分钟（之前是 300 秒；#103/#1）
/// 在暴露轮次失败之前连续可恢复流错误的硬上限。
/// 在 v0.6.7 中随 HTTP/2 保活默认值一起从 3 调整到 5
///（#103）——保活应该使虚假的解码错误更罕见，因此我们可以在
/// 放弃轮次之前容忍更长的连续错误。
pub(super) const MAX_STREAM_ERRORS_BEFORE_FAIL: u32 = 5;
/// 透明流级别重试的上限——这些只在线路在流式传输任何内容之前
/// 断开时发生，因此 DeepSeek 尚未向我们计费，用户也什么都没看到。
/// 两次尝试足以应对不稳定的边缘节点，同时不会放大真实的故障（#103）。
pub(super) const MAX_TRANSPARENT_STREAM_RETRIES: u32 = 2;

/// 判断流错误是否符合透明重试的条件。
///
/// 仅当所有三个条件都成立时返回 true：
/// 1. 当前尝试中没有收到任何内容——否则 DeepSeek
///    已经就输出 Token 向我们计费，且用户已经看到了部分
///    增量；重新发送会导致重复计费和 UI 不同步。
/// 2. 我们仍然有透明重试预算。
/// 3. 轮次未被取消。
///
/// 提取为纯函数，以便四个 #103 重试情况可以在单元测试中测试，
/// 而无需启动完整的引擎状态机。
pub(super) fn should_transparently_retry_stream(
    any_content_received: bool,
    transparent_attempts: u32,
    cancelled: bool,
) -> bool {
    !any_content_received && transparent_attempts < MAX_TRANSPARENT_STREAM_RETRIES && !cancelled
}

/// 在流断开后重新发出整个请求的预算。由空流外部重试（#103 阶段 3）
/// 和睡眠恢复重试（#2990）共享。
pub(super) const MAX_STREAM_RETRIES: u32 = 3;

/// 挂钟与单调时间之间的偏差阈值，超过此阈值可判断宿主在流中间睡眠（#2990）。
/// `Instant` 在系统睡眠期间暂停（macOS 上的 CLOCK_UPTIME_RAW，
/// Linux 上的 CLOCK_MONOTONIC），而 `SystemTime` 继续推进，因此
/// 大的正偏差只能来自挂起/恢复周期——普通的网络波动永远不会产生此偏差。
/// Windows 的 `Instant` 可能在睡眠期间继续计时，在这种情况下它永远不会触发
///（行为无变化）。
pub(super) const SLEEP_GAP_THRESHOLD: Duration = Duration::from_secs(10);

/// 如果自上次流进度以来的挂钟时间和单调时间之间的偏差
/// 表明宿主被挂起，则返回 true。
pub(super) fn sleep_gap_detected(monotonic_elapsed: Duration, wallclock_elapsed: Duration) -> bool {
    wallclock_elapsed.saturating_sub(monotonic_elapsed) > SLEEP_GAP_THRESHOLD
}

/// 判断是否应在宿主在轮次中间睡眠后静默地重新发出失败的流（#2990）。
///
/// 与透明重试（#103）不同，即使内容已经流式传输后也会触发：
/// 部分输出在睡眠之前产生，用户没有看到，重新运行相同的请求
/// 是正确的用户可见行为。阻止普通内容后重试的重复计费担忧
/// 在此处被接受，因为否则轮次将死亡，用户无论如何都必须重新提示
///（并再次付费）。
pub(super) fn should_resume_after_sleep(
    sleep_detected: bool,
    retry_attempts: u32,
    cancelled: bool,
) -> bool {
    sleep_detected && retry_attempts < MAX_STREAM_RETRIES && !cancelled
}

/// 将底层 reqwest/hyper 流读取错误转换为面向操作员的消息。
/// 原始提供商错误仍然附加，但首句解释了为什么 CodeWhale
/// 可能在任何输出之前重试，以及为什么一旦部分输出已经流式传输
/// 就必须展示警告。
pub(super) fn stream_read_error_user_message(message: &str, any_content_received: bool) -> String {
    let lower = message.to_ascii_lowercase();
    let is_stream_read = lower.contains("stream read error")
        || lower.contains("error decoding response body")
        || lower.contains("chunk decode error")
        || lower.contains("body decode");
    if !is_stream_read {
        return message.to_string();
    }

    let retry_note = if any_content_received {
        "Some output had already streamed, so CodeWhale is surfacing the warning instead of replaying the request and risking duplicated output."
    } else {
        "No output had streamed yet, so CodeWhale will retry automatically while retry budget remains."
    };
    format!(
        "Provider stream connection dropped while reading the response body. {retry_note} Details: {message}"
    )
}

pub(crate) const TOOL_CALL_START_MARKERS: [&str; 12] = [
    "[TOOL_CALL]",
    "<codewhale:tool_call",
    "<tool_call",
    "<invoke ",
    "<function_calls>",
    "<｜DSML｜tool_calls>",
    "<｜DSML｜invoke ",
    "<|DSML|tool_calls>",
    "<|DSML|invoke ",
    "<|dsml|tool_calls>",
    "<|dsml|invoke ",
    "<|tool_calls>",
];

pub(crate) const TOOL_CALL_END_MARKERS: [&str; 12] = [
    "[/TOOL_CALL]",
    "</codewhale:tool_call>",
    "</tool_call>",
    "</invoke>",
    "</function_calls>",
    "</｜DSML｜tool_calls>",
    "</｜DSML｜invoke>",
    "</|DSML|tool_calls>",
    "</|DSML|invoke>",
    "</|dsml|tool_calls>",
    "</|dsml|invoke>",
    "</|tool_calls>",
];

const TOOL_CALL_MARKER_PAIRS: [(&str, &str); 12] = [
    ("[TOOL_CALL]", "[/TOOL_CALL]"),
    ("<codewhale:tool_call", "</codewhale:tool_call>"),
    ("<tool_call", "</tool_call>"),
    ("<invoke ", "</invoke>"),
    ("<function_calls>", "</function_calls>"),
    ("<｜DSML｜tool_calls>", "</｜DSML｜tool_calls>"),
    ("<｜DSML｜invoke ", "</｜DSML｜invoke>"),
    ("<|DSML|tool_calls>", "</|DSML|tool_calls>"),
    ("<|DSML|invoke ", "</|DSML|invoke>"),
    ("<|dsml|tool_calls>", "</|dsml|tool_calls>"),
    ("<|dsml|invoke ", "</|dsml|invoke>"),
    ("<|tool_calls>", "</|tool_calls>"),
];

#[derive(Debug, Default)]
pub(crate) struct ToolCallDeltaFilterState {
    in_tool_call: bool,
    marker_carry: String,
    active_end_marker: Option<&'static str>,
}

/// 当模型尝试在纯文本中伪造工具调用包装器而不是使用 API 工具通道时，
/// 发出的一次性紧凑通知。可见内容仍然被清理；此通知的存在让用户
/// 可以看到为什么他们的文本变短了。
pub(crate) const FAKE_WRAPPER_NOTICE: &str =
    "Stripped non-API tool-call wrapper from model output (use the API tool channel)";

/// 如果 `text` 包含任何已知的伪造包装器起始标记，则返回 true。由
/// 流式循环用于决定是否发出 `FAKE_WRAPPER_NOTICE`。
pub(crate) fn contains_fake_tool_wrapper(text: &str) -> bool {
    TOOL_CALL_START_MARKERS.iter().any(|m| text.contains(m))
}

fn find_first_marker(text: &str, markers: &[&str]) -> Option<(usize, usize)> {
    markers
        .iter()
        .filter_map(|marker| text.find(marker).map(|idx| (idx, marker.len())))
        .min_by_key(|(idx, _)| *idx)
}

fn find_first_start_marker(text: &str) -> Option<(usize, usize, &'static str)> {
    TOOL_CALL_MARKER_PAIRS
        .iter()
        .filter_map(|(start, end)| text.find(start).map(|idx| (idx, start.len(), *end)))
        .min_by_key(|(idx, _, _)| *idx)
}

fn trailing_marker_prefix_len(text: &str, markers: &[&str]) -> usize {
    markers
        .iter()
        .flat_map(|marker| {
            marker
                .char_indices()
                .map(|(idx, _)| idx)
                .filter(|idx| *idx > 0)
                .chain(std::iter::once(marker.len()))
                .filter(|idx| *idx < marker.len())
                .filter(|idx| {
                    let prefix = &marker[..*idx];
                    text.ends_with(prefix)
                })
        })
        .max()
        .unwrap_or(0)
}

fn trailing_start_marker_prefix_len(text: &str) -> usize {
    TOOL_CALL_MARKER_PAIRS
        .iter()
        .flat_map(|(marker, _)| {
            marker
                .char_indices()
                .map(|(idx, _)| idx)
                .filter(|idx| *idx > 0)
                .chain(std::iter::once(marker.len()))
                .filter(|idx| *idx < marker.len())
                .filter(|idx| {
                    let prefix = &marker[..*idx];
                    text.ends_with(prefix)
                })
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn filter_tool_call_delta(delta: &str, in_tool_call: &mut bool) -> String {
    let mut state = ToolCallDeltaFilterState {
        in_tool_call: *in_tool_call,
        ..ToolCallDeltaFilterState::default()
    };
    let output = filter_tool_call_delta_with_state(delta, &mut state);
    *in_tool_call = state.in_tool_call;
    output
}

pub(crate) fn filter_tool_call_delta_with_state(
    delta: &str,
    state: &mut ToolCallDeltaFilterState,
) -> String {
    if delta.is_empty() {
        return String::new();
    }

    let chunk;
    let mut rest = if state.marker_carry.is_empty() {
        delta
    } else {
        chunk = format!("{}{delta}", state.marker_carry);
        state.marker_carry.clear();
        &chunk
    };
    let mut output = String::new();

    loop {
        if state.in_tool_call {
            let active_end_marker = state.active_end_marker;
            let found = active_end_marker
                .and_then(|marker| rest.find(marker).map(|idx| (idx, marker.len())))
                .or_else(|| find_first_marker(rest, &TOOL_CALL_END_MARKERS));
            let Some((idx, len)) = found else {
                let keep = active_end_marker.map_or_else(
                    || trailing_marker_prefix_len(rest, &TOOL_CALL_END_MARKERS),
                    |marker| trailing_marker_prefix_len(rest, &[marker]),
                );
                if keep > 0 {
                    state.marker_carry.push_str(&rest[rest.len() - keep..]);
                }
                break;
            };
            rest = &rest[idx + len..];
            state.in_tool_call = false;
            state.active_end_marker = None;
        } else {
            let Some((idx, len, end_marker)) = find_first_start_marker(rest) else {
                let keep = trailing_start_marker_prefix_len(rest);
                if keep > 0 {
                    let split = rest.len() - keep;
                    output.push_str(&rest[..split]);
                    state.marker_carry.push_str(&rest[split..]);
                } else {
                    output.push_str(rest);
                }
                break;
            };
            output.push_str(&rest[..idx]);
            rest = &rest[idx + len..];
            state.in_tool_call = true;
            state.active_end_marker = Some(end_marker);
        }
    }

    output
}

pub(crate) fn flush_tool_call_delta_state(state: &mut ToolCallDeltaFilterState) -> String {
    if state.in_tool_call {
        state.marker_carry.clear();
        return String::new();
    }
    std::mem::take(&mut state.marker_carry)
}
