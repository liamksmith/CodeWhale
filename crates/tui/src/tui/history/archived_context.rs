//! 解析和渲染归档上下文对话单元格。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;

use super::{HistoryCell, TRANSCRIPT_RAIL};

/// 从助手 Text 块中解析 `<archived_context>` 块。
///
/// 当文本包含格式良好的 `<archived_context>...</archived_context>` 块时返回 `Some(HistoryCell::ArchivedContext)`，
/// 如果文本是普通的助手内容则返回 `None`。
pub(super) fn parse_archived_context(text: &str) -> Option<HistoryCell> {
    let text = text.trim();
    if !text.starts_with("<archived_context") || !text.ends_with("</archived_context>") {
        return None;
    }

    let tag_end = text.find('>')?;
    let tag = &text[..tag_end];

    let level = archived_context_attr(tag, "level")
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(0);

    let range = archived_context_attr(tag, "range").unwrap_or_default();

    let tokens = archived_context_attr(tag, "tokens").unwrap_or_default();

    let density = archived_context_attr(tag, "density").unwrap_or_default();

    let model = archived_context_attr(tag, "model").unwrap_or_default();

    let timestamp = archived_context_attr(tag, "timestamp").unwrap_or_default();

    let close_tag = text.rfind("</archived_context>")?;
    let summary_start = tag_end + 1;
    let summary = text[summary_start..close_tag].trim().to_string();

    Some(HistoryCell::ArchivedContext {
        level,
        range,
        tokens,
        density,
        model,
        timestamp,
        summary,
    })
}

fn archived_context_attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 使用暗色/斜体样式渲染 `<archived_context>` 块。
pub(super) fn render_archived_context(
    cell: &HistoryCell,
    width: u16,
    _low_motion: bool,
) -> Vec<Line<'static>> {
    let HistoryCell::ArchivedContext {
        level,
        range,
        tokens,
        density,
        model,
        timestamp,
        summary,
    } = cell
    else {
        return Vec::new();
    };

    let body = if summary.is_empty() {
        "(no summary)".to_string()
    } else {
        summary.clone()
    };

    let label = format!("Context L{level}");
    let label_style = Style::default()
        .fg(palette::TEXT_DIM)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(palette::TEXT_DIM).italic();

    let content_width = width.saturating_sub(4).max(1);

    let mut lines = Vec::new();

    let range_display = if range.is_empty() {
        String::new()
    } else {
        range.to_string()
    };
    let mut header = format!("{label}  {range_display}");
    if !tokens.is_empty() {
        header.push_str(&format!("  {tokens}"));
    }
    if !density.is_empty() && density != tokens {
        header.push_str(&format!("  {density}"));
    }
    lines.push(Line::from(Span::styled(header, label_style)));

    let model_display = if model.is_empty() {
        String::new()
    } else {
        format!("via {model}")
    };
    let ts_display = if timestamp.is_empty() {
        String::new()
    } else {
        timestamp.clone()
    };
    let mut sub = String::new();
    if !model_display.is_empty() {
        sub.push_str(&model_display);
    }
    if !ts_display.is_empty() {
        if !sub.is_empty() {
            sub.push_str(" · ");
        }
        sub.push_str(&ts_display);
    }
    if !sub.is_empty() {
        lines.push(Line::from(Span::styled(
            sub,
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    let rendered = crate::tui::markdown_render::render_markdown(&body, content_width, body_style);
    for (idx, line) in rendered.into_iter().enumerate() {
        if idx == 0 {
            let mut spans = vec![Span::styled(
                TRANSCRIPT_RAIL.to_string(),
                Style::default().fg(palette::TEXT_DIM),
            )];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        } else {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    }

    lines.push(Line::from(""));

    lines
}
