//! 代理和活动元数据单元格的紧凑对话渲染。

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::palette;

use super::{
    GenericToolCell, render_tool_header_with_family_and_summary, tool_status_label, truncate_text,
};

pub(super) fn render_agent_compact(cell: &GenericToolCell, low_motion: bool) -> Vec<Line<'static>> {
    let family = crate::tui::widgets::tool_card::ToolFamily::Delegate;
    let agent_id = cell
        .output
        .as_deref()
        .and_then(extract_agent_id)
        .map(str::to_string)
        .unwrap_or_else(|| delegate_identity_fallback(cell));
    // 检查和 join 不能绘制与 spawn 相同的"delegate done"行——
    // 在扇出会话期间，每次 peek/status/wait 否则会被解读为又一个已完成的委托
    // (#4112, dogfood A5)。该操作由 tool_routing 标记在 args summary 的开头。
    let state_label = match agent_inspection_action(cell) {
        Some(AgentCompactAction::Check) => match cell.status {
            super::ToolStatus::Running => "checking",
            _ => "checked",
        },
        Some(AgentCompactAction::Wait) => match cell.status {
            super::ToolStatus::Running => "waiting",
            _ => "waited",
        },
        None => tool_status_label(cell.status),
    };
    vec![render_tool_header_with_family_and_summary(
        family,
        Some(agent_id.as_str()),
        state_label,
        cell.status,
        None,
        low_motion,
    )]
}

enum AgentCompactAction {
    /// 只读检查：peek / status / progress / list / inspect。
    Check,
    /// 阻塞式 join：wait / join / await / block。
    Wait,
}

/// 判断此 `agent` 单元格是只读检查或 join（而非 spawn）——
/// 即使在 Transcript 模式下它们也保持紧凑。
pub(super) fn is_agent_inspection(cell: &GenericToolCell) -> bool {
    agent_inspection_action(cell).is_some()
}

fn agent_inspection_action(cell: &GenericToolCell) -> Option<AgentCompactAction> {
    let summary = cell.input_summary.as_deref()?;
    let action = summary.strip_prefix("action:")?.trim_start();
    let action = action.split_whitespace().next().unwrap_or("");
    match action.trim_end_matches(',') {
        "peek" | "progress" | "status" | "list" | "inspect" => Some(AgentCompactAction::Check),
        "wait" | "join" | "await" | "block" => Some(AgentCompactAction::Wait),
        _ => None,
    }
}

pub(super) fn render_activity_group(cell: &GenericToolCell, width: u16) -> Vec<Line<'static>> {
    let summary = cell.input_summary.as_deref().unwrap_or("Updated metadata");
    let budget = usize::from(width).max(1);
    vec![Line::from(Span::styled(
        truncate_text(summary, budget),
        Style::default().fg(palette::TEXT_MUTED),
    ))]
}

fn delegate_identity_fallback(cell: &GenericToolCell) -> String {
    if let Some(summary) = cell.input_summary.as_deref() {
        let summary = summary.trim();
        if let Some(rest) = summary.strip_prefix("role:") {
            let role = rest.split_whitespace().next().unwrap_or(rest).trim();
            if !role.is_empty() {
                return role.to_string();
            }
        }
        if let Some(rest) = summary.strip_prefix("prompt:") {
            let title = rest.trim();
            if !title.is_empty() {
                let slug: String = title
                    .chars()
                    .take(24)
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() {
                            ch.to_ascii_lowercase()
                        } else {
                            '-'
                        }
                    })
                    .collect();
                let slug = slug.trim_matches('-');
                if !slug.is_empty() {
                    return slug.to_string();
                }
            }
        }
    }
    // #4148: 绝不在默认对话中暴露原始的内部回退令牌（"unknown child"）。
    // 当我们无法解析具体的 role、slug 或 agent id 时，
    // 在"delegate"动词旁边使用友好且不泄露信息的标签读起来效果最好
    //（"delegate running · subagent"）。
    "subagent".to_string()
}

pub(super) fn extract_agent_id(output: &str) -> Option<&str> {
    let key = "\"agent_id\"";
    let key_idx = output.find(key)?;
    let rest = &output[key_idx + key.len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    let after_colon = after_colon.strip_prefix('"')?;
    let end = after_colon.find('"')?;
    let id = &after_colon[..end];
    (!id.is_empty()).then_some(id)
}
