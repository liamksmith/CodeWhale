//! 编辑器区域的待处理输入预览组件。
//!
//! 从 `codex-rs/tui/src/bottom_pane/pending_input_preview.rs` 移植，
//! 用于问题 #85。在回合进行中时，在编辑器上方渲染排队/引导的消息，
//! 以便在正在运行的回合期间键入的用户输入不会静默消失。
//! 后备状态仍然区分队列/引导来源，但 UI 渲染一个连贯的待处理输入列表。
//!
//! 空状态渲染零行，以便在没有内容显示时编辑器不会增加无用的高度。
//!
//! 接入 `ui.rs::render` 中聊天区域和编辑器之间的位置；用户
//! 可以查看键入的输入何时已被捕获以供后续交付。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::palette;
use crate::tui::widgets::Renderable;

/// 每项行数上限，超过后将剩余内容折叠为 `…` 溢出行。
const PREVIEW_LINE_LIMIT: usize = 3;
const PENDING_STEER_PREFIX: &str = "  ↳ 实时引导待定: ";
const REJECTED_STEER_PREFIX: &str = "  ↳ 已拒绝的实时引导: ";
const EDITING_QUEUED_PREFIX: &str = "  ↳ 编辑排队的跟进: ";

/// 底部提示行应为"编辑最后一条排队消息"操作显示的快捷键描述。
#[derive(Debug, Clone)]
pub struct EditBinding {
    pub label: &'static str,
}

impl EditBinding {
    pub const UP: EditBinding = EditBinding { label: "↑" };
}

/// 显示回合进行中待处理输入的组件。
#[derive(Debug, Clone)]
pub struct PendingInputPreview {
    pub context_items: Vec<ContextPreviewItem>,
    pub pending_steers: Vec<String>,
    pub rejected_steers: Vec<String>,
    pub queued_messages: Vec<String>,
    pub editing_queued_message: Option<String>,
    pub edit_binding: EditBinding,
}

/// 在编辑器上方显示的紧凑发送前上下文行。`included=false`
/// 标记缺失/跳过的上下文，区别于将发送或内联的文件/媒体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPreviewItem {
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub included: bool,
    pub removable: bool,
    pub selected: bool,
}

impl PendingInputPreview {
    pub fn new() -> Self {
        Self {
            context_items: Vec::new(),
            pending_steers: Vec::new(),
            rejected_steers: Vec::new(),
            queued_messages: Vec::new(),
            editing_queued_message: None,
            edit_binding: EditBinding::UP,
        }
    }

    fn has_pending_inputs(&self) -> bool {
        !self.pending_steers.is_empty()
            || !self.rejected_steers.is_empty()
            || !self.queued_messages.is_empty()
            || self.editing_queued_message.is_some()
    }

    /// 构建此组件在 `width` 处渲染的（可能为空）有序行列表。
    /// 提取出来以便 `desired_height` 可以调用相同的渲染器而不重复换行逻辑。
    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        if (self.context_items.is_empty() && !self.has_pending_inputs()) || width < 4 {
            return Vec::new();
        }

        let dim = Style::default()
            .fg(palette::TEXT_DIM)
            .add_modifier(Modifier::DIM);
        let dim_italic = dim.add_modifier(Modifier::ITALIC);

        let mut lines: Vec<Line<'static>> = Vec::new();

        if !self.context_items.is_empty() {
            push_section_header(
                &mut lines,
                Line::from(vec![Span::raw("• "), Span::raw("下一次发送的上下文")]),
            );
            for item in &self.context_items {
                push_context_item(&mut lines, item, width);
            }
        }

        if self.has_pending_inputs() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            push_section_header(
                &mut lines,
                Line::from(vec![Span::raw("• "), Span::raw("待处理输入")]),
            );
            let pending_steer_indent = continuation_indent(PENDING_STEER_PREFIX);
            for steer in &self.pending_steers {
                push_truncated_item(
                    &mut lines,
                    steer,
                    width,
                    dim,
                    PENDING_STEER_PREFIX,
                    &pending_steer_indent,
                );
            }
            let rejected_steer_indent = continuation_indent(REJECTED_STEER_PREFIX);
            for steer in &self.rejected_steers {
                push_truncated_item(
                    &mut lines,
                    steer,
                    width,
                    dim,
                    REJECTED_STEER_PREFIX,
                    &rejected_steer_indent,
                );
            }
            if let Some(draft) = self.editing_queued_message.as_deref() {
                let editing_indent = continuation_indent(EDITING_QUEUED_PREFIX);
                push_truncated_item(
                    &mut lines,
                    draft,
                    width,
                    dim_italic,
                    EDITING_QUEUED_PREFIX,
                    &editing_indent,
                );
                lines.push(Line::from(vec![Span::styled(
                    "    Esc 恢复排队的跟进".to_string(),
                    dim,
                )]));
            }
            for (idx, message) in self.queued_messages.iter().enumerate() {
                let row_number = idx + 1;
                let queued_prefix = format!("  ↳ 排队的跟进 #{row_number}: ");
                let queued_message_indent = continuation_indent(&queued_prefix);
                push_truncated_item(
                    &mut lines,
                    message,
                    width,
                    dim_italic,
                    &queued_prefix,
                    &queued_message_indent,
                );
                lines.push(Line::from(vec![Span::styled(
                    format!("    /queue send {row_number} · drop {row_number} · clear"),
                    dim,
                )]));
            }
            if !self.queued_messages.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "    Ctrl+S 立即发送 · {} 编辑最后一条排队",
                        self.edit_binding.label
                    ),
                    dim,
                )]));
            }
        }

        lines
    }
}

impl Default for PendingInputPreview {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderable for PendingInputPreview {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let lines = self.lines(area.width);
        if lines.is_empty() {
            return;
        }
        Paragraph::new(lines).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let lines = self.lines(width);
        u16::try_from(lines.len()).unwrap_or(u16::MAX)
    }
}

fn continuation_indent(prefix: &str) -> String {
    " ".repeat(display_width(prefix))
}

fn push_section_header(lines: &mut Vec<Line<'static>>, header: Line<'static>) {
    lines.push(header);
}

fn push_context_item(lines: &mut Vec<Line<'static>>, item: &ContextPreviewItem, width: u16) {
    let status_style = if item.selected {
        Style::default()
            .fg(palette::SELECTION_TEXT)
            .bg(palette::SELECTION_BG)
            .add_modifier(Modifier::BOLD)
    } else if item.included {
        Style::default().fg(palette::TEXT_MUTED)
    } else {
        Style::default().fg(palette::STATUS_WARNING)
    };
    let label_style = if item.selected {
        Style::default()
            .fg(palette::SELECTION_TEXT)
            .bg(palette::SELECTION_BG)
    } else if item.included {
        Style::default().fg(palette::TEXT_PRIMARY)
    } else {
        Style::default().fg(palette::TEXT_MUTED)
    };
    let detail = item
        .detail
        .as_deref()
        .filter(|detail| !detail.trim().is_empty())
        .map(|detail| format!(" · {detail}"))
        .unwrap_or_default();
    let action = if item.selected {
        " · Backspace/Delete 移除"
    } else if item.removable {
        " · 可移除"
    } else {
        ""
    };
    let body = format!("[{}] {}{}{}", item.kind, item.label, detail, action);
    let body_width = width.saturating_sub(4).max(1) as usize;
    for (idx, segment) in wrap_to_width(&body, body_width).into_iter().enumerate() {
        let prefix = if idx == 0 {
            if item.selected { "  ▸ " } else { "  ↳ " }
        } else {
            "    "
        };
        lines.push(Line::from(vec![
            Span::styled(prefix.to_string(), status_style),
            Span::styled(segment, label_style),
        ]));
    }
}

/// 使用 `↳` 前缀渲染单个桶项，截断到 [`PREVIEW_LINE_LIMIT`] 可见行。
/// 多行输入在给定的列预算处换行，续行获得 `subsequent_indent`，
/// 以便前缀和正文保持列对齐。
fn push_truncated_item(
    lines: &mut Vec<Line<'static>>,
    raw: &str,
    width: u16,
    style: Style,
    prefix: &str,
    subsequent_indent: &str,
) {
    let body_width = width.saturating_sub(display_width(prefix) as u16) as usize;
    let body_width = body_width.max(1);

    let mut produced: Vec<String> = Vec::new();
    for (idx, paragraph) in raw.split('\n').enumerate() {
        let wrapped = wrap_to_width(paragraph, body_width);
        for (j, segment) in wrapped.into_iter().enumerate() {
            let row = if idx == 0 && j == 0 {
                format!("{prefix}{segment}")
            } else {
                format!("{subsequent_indent}{segment}")
            };
            produced.push(row);
            if produced.len() > PREVIEW_LINE_LIMIT {
                break;
            }
        }
        if produced.len() > PREVIEW_LINE_LIMIT {
            break;
        }
    }

    let truncated = produced.len() > PREVIEW_LINE_LIMIT;
    for (i, row) in produced.into_iter().enumerate() {
        if i >= PREVIEW_LINE_LIMIT {
            break;
        }
        lines.push(Line::from(Span::styled(row, style)));
    }
    if truncated {
        lines.push(Line::from(Span::styled(
            format!("{subsequent_indent}…"),
            style,
        )));
    }
}

/// 朴素且考虑单词的换行，尊重 unicode 显示宽度。匹配 codex 源中
/// 快照测试预期的行为——超过 `width` 的长 URL 类令牌在自己的行上发出，
/// 而不是在字符中间硬断。
fn wrap_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_inclusive(' ') {
        let word_width = display_width(word);
        if current_width + word_width > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width > width {
            // 令牌比预算长：刷新当前，将单词作为自己的行发出即使溢出。
            // 避免长 URL 扩散为 N 个垃圾省略号行的 codex 问题。
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_width = 0;
            }
            out.push(word.trim_end().to_string());
            continue;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

// 委托给规范的宽度约定（`ui_text::text_display_width`）：
// 制表符为 4 列，控制字符占一列，与渲染器绘制的内容一致。
// 旧的本地副本使用 `unwrap_or(0)` 并忽略制表符，因此预览
// 换行在那些输入上与实际布局不一致（#3924）。
fn display_width(s: &str) -> usize {
    crate::tui::ui_text::text_display_width(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(widget: &PendingInputPreview, width: u16) -> Vec<String> {
        let height = widget.desired_height(width);
        if height == 0 {
            return Vec::new();
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        widget.render(Rect::new(0, 0, width, height), &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn empty_widget_has_zero_height() {
        let preview = PendingInputPreview::new();
        assert_eq!(preview.desired_height(40), 0);
    }

    #[test]
    fn single_queued_message_renders_header_item_and_hint() {
        let mut preview = PendingInputPreview::new();
        preview.queued_messages.push("Hello, world!".to_string());
        let rows = render_to_string(&preview, 40);
        // 预期：标题行、消息行、操作行、提示行。
        assert_eq!(rows.len(), 4, "得到行: {rows:?}");
        assert!(rows[0].contains("Pending inputs"));
        assert!(rows[1].contains("Hello, world!"));
        assert!(rows[2].contains("/queue send 1"));
        assert!(rows[2].contains("drop 1"));
        assert!(rows[2].contains("clear"));
        assert!(rows[3].contains("Ctrl+S send now"));
        assert!(rows[3].contains("edit last queued"));
    }

    #[test]
    fn editing_queued_message_renders_explicit_state_and_restore_hint() {
        let mut preview = PendingInputPreview::new();
        preview.editing_queued_message = Some("revise before sending".to_string());

        let rows = render_to_string(&preview, 80);

        assert!(rows[0].contains("Pending inputs"));
        assert!(
            rows.iter()
                .any(|row| row.contains("编辑排队的跟进: revise before sending")),
            "缺少编辑标签: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Esc 恢复排队的跟进")),
            "缺少恢复提示: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("edit last queued")),
            "编辑模式不应同时宣传打开排队编辑: {rows:?}"
        );
    }

    #[test]
    fn context_items_render_before_queue_buckets() {
        let mut preview = PendingInputPreview::new();
        preview.context_items.push(ContextPreviewItem {
            kind: "file".to_string(),
            label: "src/main.rs".to_string(),
            detail: Some("included".to_string()),
            included: true,
            removable: false,
            selected: false,
        });
        preview.context_items.push(ContextPreviewItem {
            kind: "missing".to_string(),
            label: "nope.txt".to_string(),
            detail: Some("not found".to_string()),
            included: false,
            removable: false,
            selected: false,
        });
        let rows = render_to_string(&preview, 64);
        assert!(rows[0].contains("Context for next send"));
        assert!(rows[1].contains("[file] src/main.rs"));
        assert!(rows[2].contains("[missing] nope.txt"));
    }

    #[test]
    fn selected_removable_attachment_renders_delete_hint() {
        let mut preview = PendingInputPreview::new();
        preview.context_items.push(ContextPreviewItem {
            kind: "image".to_string(),
            label: "/tmp/pasted.png".to_string(),
            detail: Some("attached media".to_string()),
            included: true,
            removable: true,
            selected: true,
        });

        let rows = render_to_string(&preview, 96);

        assert!(
            rows.iter()
                .any(|row| row.contains("Backspace/Delete 移除"))
        );
        assert!(rows.iter().any(|row| row.contains("▸")));
    }

    #[test]
    fn pending_steer_renders_without_queue_edit_hint() {
        let mut preview = PendingInputPreview::new();
        preview.pending_steers.push("Please continue.".to_string());
        let rows = render_to_string(&preview, 80);
        assert!(
            rows.iter().any(|r| r.contains("Pending inputs")),
            "缺少待处理输入标题: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("Esc")),
            "意外的 Esc 提示: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("edit last queued")),
            "在仅实时引导的视图中意外的编辑提示: {rows:?}"
        );
    }

    #[test]
    fn all_pending_inputs_render_as_one_list() {
        let mut preview = PendingInputPreview::new();
        preview.pending_steers.push("steer".to_string());
        preview.rejected_steers.push("rejected".to_string());
        preview.queued_messages.push("queued".to_string());
        let rows = render_to_string(&preview, 60);
        assert!(rows[0].contains("Pending inputs"));
        assert_eq!(
            rows.iter().filter(|r| r.contains("Pending inputs")).count(),
            1
        );
        assert!(rows.iter().any(|r| r.contains("steer")));
        assert!(rows.iter().any(|r| r.contains("rejected")));
        assert!(rows.iter().any(|r| r.contains("queued")));
        assert!(rows.iter().any(|r| r.contains("↑")));
        assert!(rows.iter().any(|r| r.contains("Ctrl+S")));
    }

    #[test]
    fn pending_input_rows_label_each_delivery_mode() {
        let mut preview = PendingInputPreview::new();
        preview.pending_steers.push("steer".to_string());
        preview.rejected_steers.push("rejected".to_string());
        preview.queued_messages.push("queued".to_string());
        preview.editing_queued_message = Some("editing".to_string());

        let rows = render_to_string(&preview, 80);

        assert!(
            rows.iter()
                .any(|row| row.contains("实时引导待定: steer")),
            "缺少实时引导待定标签: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("已拒绝的实时引导: rejected")),
            "缺少已拒绝引导标签: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("排队的跟进 #1: queued")),
            "缺少排队跟进标签: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("编辑排队的跟进: editing")),
            "缺少排队编辑标签: {rows:?}"
        );
    }

    #[test]
    fn wrapped_pending_input_aligns_continuation_under_label() {
        let mut preview = PendingInputPreview::new();
        preview
            .queued_messages
            .push("alpha beta gamma delta epsilon zeta".to_string());

        let rows = render_to_string(&preview, 34);

        assert!(rows[1].contains("Queued follow-up #1: alpha"));
        assert!(
            rows[2].starts_with(&continuation_indent("  ↳ 排队的跟进 #1: ")),
            "续行应对齐在标签下: {rows:?}"
        );
        assert!(
            !rows[2].trim().is_empty(),
            "续行应保留换行文本: {rows:?}"
        );
    }

    #[test]
    fn message_truncates_to_three_visible_lines() {
        let mut preview = PendingInputPreview::new();
        preview
            .queued_messages
            .push("line1\nline2\nline3\nline4\nline5".to_string());
        let rows = render_to_string(&preview, 40);
        // 标题 + 3 可见行 + 省略号行 + 操作 + 提示 = 7 行。
        assert_eq!(rows.len(), 7, "得到行: {rows:?}");
        assert!(rows[0].contains("Pending inputs"));
        assert!(rows[1].contains("line1"));
        assert!(rows[2].contains("line2"));
        assert!(rows[3].contains("line3"));
        assert!(rows[4].contains("…"));
        assert!(rows[5].contains("/queue send 1"));
        assert!(rows[6].contains("Ctrl+S send now"));
        assert!(rows[6].contains("edit last queued"));
    }

    #[test]
    fn long_url_does_not_explode_into_ellipsis_rows() {
        let mut preview = PendingInputPreview::new();
        preview.queued_messages.push(
            "example.test/api/v1/projects/alpha/releases/2026-02-17/build/1234567890/artifacts/x"
                .to_string(),
        );
        let rows = render_to_string(&preview, 36);
        // 标题 + URL 行 + 操作行 + 提示 = 4 行；URL 不得
        // 导致一连串的换行省略号行。
        assert_eq!(rows.len(), 4, "得到行: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains("…")));
    }

    #[test]
    fn narrow_width_renders_nothing() {
        let mut preview = PendingInputPreview::new();
        preview.queued_messages.push("hi".to_string());
        assert_eq!(preview.desired_height(2), 0);
    }
}
