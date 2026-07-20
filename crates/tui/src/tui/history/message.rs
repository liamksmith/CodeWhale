//! 用户、助手和系统消息的对话记录渲染。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::palette;
use crate::tui::markdown_render;
use crate::tui::ui_text::CopyLineSeparator;

use super::{ASSISTANT_GLYPH, USER_GLYPH};

pub(crate) struct RenderedTranscriptLine {
    pub line: Line<'static>,
    pub copy_prefix_width: usize,
    pub copy_separator_after: CopyLineSeparator,
}

pub(super) fn render_message(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_message_with_copy_metadata(prefix, label_style, body_style, content, width)
        .into_iter()
        .map(|rendered| rendered.line)
        .collect()
}

pub(super) fn render_message_with_copy_metadata(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<RenderedTranscriptLine> {
    // 一个内容完全是空白字符的助手消息单元（例如在推理和工具调用之间
    // 流传的杂散换行符）否则会渲染为孤立的角色字形单独浮动一行——
    // 即"后面没有任何内容的蓝色圆点"伪像。不渲染任何内容，
    // 这样对话记录就不会累积空白标记。真正的散文内容，
    // 包括仅以空行开始的消息，仍正常渲染。
    if prefix == ASSISTANT_GLYPH && content.trim().is_empty() {
        return Vec::new();
    }
    let prefix_width = UnicodeWidthStr::width(prefix);
    let prefix_width_u16 = u16::try_from(prefix_width.saturating_add(2)).unwrap_or(u16::MAX);
    let content_width = usize::from(width.saturating_sub(prefix_width_u16).max(1));
    let mut lines = Vec::new();
    let rendered =
        markdown_render::render_markdown_tagged(content, content_width as u16, body_style);
    for (idx, rendered_line) in rendered.into_iter().enumerate() {
        let line = if idx == 0 {
            let mut spans = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(
                    prefix.to_string(),
                    label_style.add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
            }
            spans.extend(rendered_line.line.spans);
            Line::from(spans)
        } else {
            let indent = if prefix.is_empty() {
                String::new()
            } else if rendered_line.is_code {
                " ".repeat(prefix_width + 1)
            } else {
                let mut s = String::with_capacity(prefix_width + 1);
                s.push('\u{258F}');
                s.extend(std::iter::repeat_n(' ', prefix_width));
                s
            };
            let rail_style = Style::default().fg(palette::TEXT_DIM);
            let mut spans = vec![Span::styled(indent, rail_style)];
            spans.extend(rendered_line.line.spans);
            Line::from(spans)
        };
        lines.push(RenderedTranscriptLine {
            line,
            copy_prefix_width: rendered_line.copy_prefix_width
                + history_copy_prefix_width(prefix, prefix_width, rendered_line.is_code, idx),
            copy_separator_after: rendered_line.copy_separator_after,
        });
    }
    if lines.is_empty() {
        lines.push(RenderedTranscriptLine {
            line: Line::from(""),
            copy_prefix_width: 0,
            copy_separator_after: CopyLineSeparator::Newline,
        });
    }
    lines
}

fn history_copy_prefix_width(
    prefix: &str,
    prefix_width: usize,
    is_code: bool,
    line_index: usize,
) -> usize {
    if line_index > 0 && is_code && !prefix.is_empty() {
        prefix_width + 1
    } else {
        0
    }
}

pub(super) fn hard_break_copy_lines(lines: Vec<Line<'static>>) -> Vec<RenderedTranscriptLine> {
    lines
        .into_iter()
        .map(|line| RenderedTranscriptLine {
            line,
            copy_prefix_width: 0,
            copy_separator_after: CopyLineSeparator::Newline,
        })
        .collect()
}

/// 渲染纯文本用户消息：按换行符分割，每行自动换行，保留前导空白。
/// 不进行 Markdown 解释（标题、列表、代码块等按字面文本渲染）。
pub(super) fn render_plain_message(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let prefix_width = UnicodeWidthStr::width(prefix);
    let prefix_width_u16 = u16::try_from(prefix_width.saturating_add(2)).unwrap_or(u16::MAX);
    let content_width = width.saturating_sub(prefix_width_u16).max(1);
    let rendered = markdown_render::render_plain_text(content, content_width, body_style);
    let mut lines = Vec::with_capacity(rendered.len());

    for (idx, line) in rendered.into_iter().enumerate() {
        if idx == 0 {
            let mut spans = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(
                    prefix.to_string(),
                    label_style.add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
            }
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        } else {
            let indent = if prefix.is_empty() {
                String::new()
            } else {
                let mut s = String::with_capacity(prefix_width + 1);
                s.push('\u{258F}');
                s.extend(std::iter::repeat_n(' ', prefix_width));
                s
            };
            let rail_style = Style::default().fg(palette::TEXT_DIM);
            let mut spans = vec![Span::styled(indent, rail_style)];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

pub(super) fn render_user_message(content: &str, width: u16) -> Vec<Line<'static>> {
    render_plain_message(
        USER_GLYPH,
        user_label_style(),
        user_body_style(),
        content,
        width,
    )
    .into_iter()
    .map(|line| apply_user_message_highlight(line, width))
    .collect()
}

fn apply_user_message_highlight(mut line: Line<'static>, width: u16) -> Line<'static> {
    let bg = palette::SURFACE_ELEVATED;
    line.style = line.style.bg(bg);

    let target_width = usize::from(width);
    let line_width = line.width();
    if line_width < target_width {
        line.spans.push(Span::styled(
            " ".repeat(target_width - line_width),
            Style::default().bg(bg),
        ));
    }

    line
}

pub(super) fn user_label_style() -> Style {
    Style::default().fg(palette::USER_BODY)
}

pub(super) fn user_body_style() -> Style {
    Style::default().fg(palette::USER_BODY)
}

/// 助手字形（`●`）的样式。当消息单元正在流式输出且允许动画时，
/// 前景色以 2 秒周期在 30% 到 100% 亮度之间脉动——这是平静对话记录中
/// 唯一刻意动画的元素。当空闲时（或 low_motion 开启时），
/// 它保持完整的 DeepSeek 天蓝色，使完成的轮次看起来饱满而非暗淡。
pub(super) fn assistant_label_style_for(streaming: bool, low_motion: bool) -> Style {
    let color = if streaming && !low_motion {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        palette::pulse_brightness(palette::WHALE_INFO, now_ms)
    } else {
        palette::WHALE_INFO
    };
    Style::default().fg(color)
}

pub(super) fn system_label_style() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

pub(super) fn message_body_style() -> Style {
    Style::default().fg(palette::TEXT_PRIMARY)
}

pub(super) fn system_body_style() -> Style {
    Style::default().fg(palette::TEXT_MUTED).italic()
}
