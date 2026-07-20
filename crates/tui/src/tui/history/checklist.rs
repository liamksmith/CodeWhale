//! 清单和待办事项记录渲染辅助函数。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::palette;

use super::{
    RenderMode, TRANSCRIPT_RAIL, ToolStatus, render_card_detail_line_single, render_compact_kv,
    render_tool_header_with_family_and_summary, tool_status_label, tool_value_style, truncate_text,
    wrap_text,
};

pub(super) fn is_checklist_tool_name(name: &str) -> bool {
    matches!(
        name,
        "work_update"
            | "checklist_write"
            | "checklist_add"
            | "checklist_update"
            | "todo_write"
            | "todo_add"
            | "todo_update"
    )
}

#[derive(Debug, Clone)]
pub(super) struct ChecklistItemSnapshot {
    pub(super) content: String,
    pub(super) status: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ChecklistSnapshot {
    pub(super) items: Vec<ChecklistItemSnapshot>,
    pub(super) completion_pct: u8,
    pub(super) completed: usize,
    pub(super) total: usize,
}

/// 从工具的文本输出中提取结构化的清单快照。
/// 工具会先发出一个人可读的行，然后是 JSON，因此我们扫描第一个 `{` 并从那里开始解析。
/// 如果负载缺少预期的 `items` 数组，则返回 `None`。
pub(super) fn parse_checklist_snapshot(output: &str) -> Option<ChecklistSnapshot> {
    let json_start = output.find('{')?;
    let parsed: Value = serde_json::from_str(&output[json_start..]).ok()?;
    let items_value = parsed.get("items")?.as_array()?;

    let items: Vec<ChecklistItemSnapshot> = items_value
        .iter()
        .map(|item| ChecklistItemSnapshot {
            content: item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    let completed = items
        .iter()
        .filter(|item| item.status.eq_ignore_ascii_case("completed"))
        .count();
    let total = items.len();
    let completion_pct = parsed
        .get("completion_pct")
        .and_then(Value::as_u64)
        .map(|pct| u8::try_from(pct.min(100)).unwrap_or(100))
        .unwrap_or_else(|| {
            (completed * 100)
                .checked_div(total)
                .and_then(|pct| u8::try_from(pct).ok())
                .unwrap_or(0)
        });

    Some(ChecklistSnapshot {
        items,
        completion_pct,
        completed,
        total,
    })
}

/// 由 `todo_update` / `checklist_update` 发出的一个已解析的 "Updated todo #N to STATUS" 前缀行。
/// 由 [`render_checklist_change_card`] 用于显示紧凑的状态变更行，而不是完整项目列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChecklistChange {
    pub(super) id: u32,
    pub(super) status: String,
}

/// 解析清单更新工具输出的首行。对于非更新输出（例如 `todo_write` 快照、错误
/// 或意外格式）返回 `None`，以便调用者回退到完整列表渲染器。
pub(super) fn parse_update_prefix(output: &str) -> Option<ChecklistChange> {
    // 工具输出格式为 `Updated todo #3 to in_progress\n{ ... }`。
    // 我们接受 `checklist` 或 `todo` 作为名词，以及任何合理的状态词
    //（渲染器中的快照查找是标题的真实来源——我们只需要 id+status 对）。
    let first = output.lines().next()?.trim();
    let rest = first
        .strip_prefix("Updated todo #")
        .or_else(|| first.strip_prefix("Updated checklist #"))?;
    let (id_str, after) = rest.split_once(' ')?;
    let id: u32 = id_str.parse().ok()?;
    let status = after.strip_prefix("to ")?.trim().to_string();
    if status.is_empty() {
        return None;
    }
    Some(ChecklistChange { id, status })
}

/// 为 `todo_update` / `checklist_update` 调用渲染紧凑的单行状态变更卡片 (#403)。
/// 显示已变更项目的标记、标题和旧 -> 新状态，头部带有 `M/N · pct%` 进度摘要。
/// 完整列表仍然可以通过工具详情记录查看。
pub(super) fn render_checklist_change_card(
    name: &str,
    status: ToolStatus,
    snapshot: &ChecklistSnapshot,
    change: &ChecklistChange,
    width: u16,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header_summary = format!(
        "{}/{} \u{00B7} {}%",
        snapshot.completed, snapshot.total, snapshot.completion_pct
    );
    let family = crate::tui::widgets::tool_card::tool_family_for_name(name);
    lines.push(render_tool_header_with_family_and_summary(
        family,
        Some(&header_summary),
        tool_status_label(status),
        status,
        None,
        low_motion,
    ));

    // 从快照中查找标题。工具输入中的 `id` 从 1 开始；`items` 从 0 开始。
    let item = (change.id as usize)
        .checked_sub(1)
        .and_then(|idx| snapshot.items.get(idx));
    let title = item
        .map(|i| i.content.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(missing title)".to_string());

    let (marker, marker_color) = checklist_status_marker(&change.status);
    let prefix = format!("{marker} ");
    let prefix_width =
        UnicodeWidthStr::width(TRANSCRIPT_RAIL) + UnicodeWidthStr::width(prefix.as_str());
    let id_label = format!("Todo #{}", change.id);
    let arrow = " \u{2192} ";
    let status_label = change.status.clone();
    let title_budget = usize::from(width)
        .saturating_sub(prefix_width)
        .saturating_sub(UnicodeWidthStr::width(id_label.as_str()))
        .saturating_sub(UnicodeWidthStr::width(arrow))
        .saturating_sub(UnicodeWidthStr::width(status_label.as_str()))
        .saturating_sub(2)
        .max(8);
    let title_truncated = truncate_text(title.as_str(), title_budget);

    let spans = vec![
        Span::styled(
            "\u{258F} ".to_string(),
            Style::default().fg(palette::TEXT_DIM),
        ),
        Span::styled(prefix, Style::default().fg(marker_color)),
        Span::styled(id_label, Style::default().fg(palette::TEXT_DIM)),
        Span::styled(": ".to_string(), Style::default().fg(palette::TEXT_DIM)),
        Span::styled(title_truncated, tool_value_style()),
        Span::styled(arrow.to_string(), Style::default().fg(palette::TEXT_DIM)),
        Span::styled(status_label, Style::default().fg(marker_color)),
    ];
    lines.push(Line::from(spans));

    // 提示完整列表仍然可用，无需离开记录文本。与其他工具单元格使用的功能类似。
    lines.push(render_card_detail_line_single(
        None,
        &format!(
            "{} item{}; {}",
            snapshot.total,
            if snapshot.total == 1 { "" } else { "s" },
            crate::tui::key_shortcuts::tool_details_shortcut_action_hint("full list")
        ),
        Style::default().fg(palette::TEXT_MUTED),
    ));
    lines
}

fn checklist_status_marker(status: &str) -> (&'static str, Color) {
    match status.to_ascii_lowercase().as_str() {
        "completed" | "done" => ("\u{2611}", palette::STATUS_SUCCESS), // ☑
        "in_progress" | "inprogress" | "running" => ("\u{25D0}", palette::WHALE_INFO), // ◐
        "blocked" | "failed" => ("\u{2717}", palette::STATUS_ERROR),   // ✗
        "cancelled" | "canceled" | "skipped" => ("\u{2298}", palette::TEXT_MUTED), // ⊘
        _ => ("\u{2610}", palette::TEXT_MUTED),                        // ☐ pending
    }
}

const CHECKLIST_LIVE_ITEM_LIMIT: usize = 8;

pub(super) fn render_checklist_card(
    name: &str,
    status: ToolStatus,
    snapshot: &ChecklistSnapshot,
    width: u16,
    low_motion: bool,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header_summary = format!(
        "{}/{} \u{00B7} {}%",
        snapshot.completed, snapshot.total, snapshot.completion_pct
    );
    let family = crate::tui::widgets::tool_card::tool_family_for_name(name);
    lines.push(render_tool_header_with_family_and_summary(
        family,
        Some(&header_summary),
        tool_status_label(status),
        status,
        None,
        low_motion,
    ));
    lines.extend(render_compact_kv(
        "checklist",
        name,
        tool_value_style(),
        width,
    ));

    let cap = match mode {
        RenderMode::Live => CHECKLIST_LIVE_ITEM_LIMIT,
        RenderMode::Transcript => snapshot.items.len(),
    };
    let visible: Vec<&ChecklistItemSnapshot> = snapshot.items.iter().take(cap).collect();
    let omitted = snapshot.items.len().saturating_sub(visible.len());

    for item in visible {
        let (marker, color) = checklist_status_marker(&item.status);
        let prefix = format!("{marker} ");
        // 在换行内容时为轨道 + 标记前缀预留空间。
        let prefix_width =
            UnicodeWidthStr::width(TRANSCRIPT_RAIL) + UnicodeWidthStr::width(prefix.as_str());
        let content_width = usize::from(width).saturating_sub(prefix_width).max(1);
        for (idx, part) in wrap_text(item.content.trim(), content_width)
            .into_iter()
            .enumerate()
        {
            let mut spans = vec![Span::styled(
                "\u{258F} ".to_string(),
                Style::default().fg(palette::TEXT_DIM),
            )];
            if idx == 0 {
                spans.push(Span::styled(prefix.clone(), Style::default().fg(color)));
            } else {
                spans.push(Span::raw(
                    " ".repeat(UnicodeWidthStr::width(prefix.as_str())),
                ));
            }
            spans.push(Span::styled(part, tool_value_style()));
            lines.push(Line::from(spans));
        }
    }

    if omitted > 0 {
        lines.push(render_card_detail_line_single(
            None,
            &format!(
                "+{omitted} more; {}",
                crate::tui::key_shortcuts::tool_details_shortcut_action_hint("full list")
            ),
            Style::default().fg(palette::TEXT_DIM),
        ));
    }

    lines
}
