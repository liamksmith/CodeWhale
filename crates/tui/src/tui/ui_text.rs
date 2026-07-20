//! TUI 选择和剪贴板工作流的共享文本辅助函数。

use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::history::HistoryCell;
use crate::tui::osc8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyLineSeparator {
    None,
    Space,
    Newline,
}

impl CopyLineSeparator {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Space => " ",
            Self::Newline => "\n",
        }
    }
}

pub(crate) fn truncate_line_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    // 对于非常小的预算，逐个字符取，直到超过*显示*宽度。
    if max_width <= 3 {
        let mut out = String::new();
        let mut width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > max_width {
                break;
            }
            out.push(ch);
            width += ch_width;
        }
        return out;
    }

    let mut out = String::new();
    let mut width = 0usize;
    let limit = max_width.saturating_sub(3);
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

/// 将 `text` 截断到 `max_width` 显示列，优先保持完整单词。
pub(crate) fn semantic_truncate(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text_display_width(text) <= max_width {
        return text.to_string();
    }

    const ELLIPSIS: char = '…';
    let ellipsis_width = char_display_width(ELLIPSIS);
    let limit = max_width.saturating_sub(ellipsis_width);
    if limit == 0 {
        return ELLIPSIS.to_string();
    }

    let mut width = 0usize;
    let mut cut_byte = 0usize;
    let mut last_word_end = None;
    let mut in_word = false;
    for (byte_idx, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if width + ch_width > limit {
            break;
        }
        width += ch_width;
        cut_byte = byte_idx + ch.len_utf8();
        if ch.is_whitespace() {
            if in_word {
                last_word_end = Some(byte_idx);
                in_word = false;
            }
        } else {
            in_word = true;
        }
    }
    if cut_byte == 0 {
        return ELLIPSIS.to_string();
    }

    let mut body = if let Some(word_end) = last_word_end {
        text[..word_end].trim_end()
    } else {
        text[..cut_byte].trim_end()
    };
    if body.is_empty() {
        body = text[..cut_byte].trim_end();
    }
    let mut out = body.to_string();
    out.push(ELLIPSIS);
    out
}

pub(crate) fn semantic_truncate_with_affixes(
    prefix: &str,
    text: &str,
    suffix: &str,
    max_width: usize,
) -> String {
    let fixed_width = text_display_width(prefix) + text_display_width(suffix);
    if fixed_width > max_width {
        return semantic_truncate(&format!("{prefix}{text}{suffix}"), max_width);
    }
    format!(
        "{prefix}{}{suffix}",
        semantic_truncate_between_affixes(prefix, text, suffix, max_width)
    )
}

pub(crate) fn semantic_truncate_between_affixes(
    prefix: &str,
    text: &str,
    suffix: &str,
    max_width: usize,
) -> String {
    let fixed_width = text_display_width(prefix) + text_display_width(suffix);
    if fixed_width > max_width {
        return String::new();
    }
    semantic_truncate(text, max_width - fixed_width)
}

pub(crate) fn concise_shell_command_label(command: &str, max_width: usize) -> String {
    let normalized = normalize_shell_text(command);
    if let Some(label) = gh_command_label(&normalized) {
        return truncate_line_to_width(&label, max_width);
    }

    let segment = actionable_shell_segment(&normalized).unwrap_or_else(|| normalized.clone());
    truncate_line_to_width(&segment, max_width)
}

fn normalize_shell_text(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    crate::tui::osc8::strip_ansi_into(text, &mut cleaned);
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn actionable_shell_segment(command: &str) -> Option<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace('|', "\n")
        .split(['\n', ';'])
        .map(str::trim)
        .find(|segment| {
            !segment.is_empty()
                && !segment.starts_with("cd ")
                && !segment.starts_with("sleep ")
                && !segment.starts_with("export ")
                && *segment != "true"
                && *segment != ":"
        })
        .map(str::to_string)
}

fn gh_command_label(command: &str) -> Option<String> {
    let tokens: Vec<String> = command
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch: char| matches!(ch, '\'' | '"' | '(' | ')' | ';' | ','))
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .collect();

    for index in 0..tokens.len() {
        let token = tokens[index].as_str();
        if token != "gh" && !token.ends_with("/gh") {
            continue;
        }
        let Some(area) = tokens.get(index + 1).map(String::as_str) else {
            continue;
        };
        let Some(action) = tokens.get(index + 2).map(String::as_str) else {
            continue;
        };
        if !matches!(area, "pr" | "run") {
            continue;
        }
        if !matches!(
            action,
            "checks" | "view" | "status" | "list" | "watch" | "rerun"
        ) {
            continue;
        }

        let mut label = format!("gh {area} {action}");
        if let Some(target) = tokens
            .iter()
            .skip(index + 3)
            .map(String::as_str)
            .find(|token| !token.starts_with('-') && *token != "&&" && *token != ";")
        {
            label.push(' ');
            label.push_str(target);
        }
        return Some(label);
    }
    None
}

pub(super) fn history_cell_to_text(cell: &HistoryCell, width: u16) -> String {
    cell.transcript_lines(width)
        .into_iter()
        .map(line_to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_to_string(line: Line<'static>) -> String {
    let mut out = String::new();
    append_spans_plain(line.spans.iter(), &mut out);
    out
}

/// 将渲染的会话记录行转换为纯文本，剥离 OSC-8 链接
/// 转义序列。调用者负责调整选择列以考虑任何仅视觉的轨前缀
///（参见 `TranscriptViewCache::rail_prefix_width`）。
pub(super) fn line_to_plain(line: &Line<'static>) -> String {
    let mut out = String::new();
    append_spans_plain(line.spans.iter(), &mut out);
    out
}

fn append_spans_plain<'a, I>(spans: I, out: &mut String)
where
    I: Iterator<Item = &'a Span<'a>>,
{
    for span in spans {
        if span.content.contains('\x1b') {
            osc8::strip_into(&span.content, out);
        } else {
            out.push_str(span.content.as_ref());
        }
    }
}

pub(crate) fn text_display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

pub(super) fn slice_text(text: &str, start: usize, end: usize) -> String {
    if end <= start {
        return String::new();
    }

    let mut out = String::new();
    let mut col = 0usize;
    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        let ch_start = col;
        let ch_end = col.saturating_add(ch_width);
        if ch_end > start && ch_start < end {
            out.push(ch);
        }
        col = ch_end;
        if col >= end {
            break;
        }
    }
    out
}

pub(super) fn char_display_width(ch: char) -> usize {
    if ch == '\t' {
        4
    } else {
        // `width()` 对控制/未分配字符返回 `None`（默认为一列
        // 以避免布局坍塌），对真正的零宽字符——组合标记、ZWJ、
        // 零宽空格——返回 `Some(0)`，必须保持 0 以便显示宽度计算
        //（截断、切片、溢出、复制）与终端实际渲染一致。
        UnicodeWidthChar::width(ch).unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    #[test]
    fn line_to_plain_strips_osc_8_wrapper() {
        let wrapped = format!(
            "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            "https://example.com", "https://example.com"
        );
        let line = Line::from(vec![
            Span::raw("see "),
            Span::raw(wrapped),
            Span::raw(" for details"),
        ]);
        let text = line_to_plain(&line);
        assert_eq!(text, "see https://example.com for details");
    }

    #[test]
    fn line_to_plain_passes_through_plain_spans() {
        let line = Line::from(vec![Span::raw("plain "), Span::raw("text")]);
        let text = line_to_plain(&line);
        assert_eq!(text, "plain text");
    }

    #[test]
    fn line_to_plain_includes_all_spans() {
        // 仅视觉的轨前缀由调用者使用
        // TranscriptViewCache::rail_prefix_width 剥离——line_to_plain 本身
        // 是忠实的 span 到字符串传递。
        let line = Line::from(vec![Span::raw("\u{2502} "), Span::raw("tool output")]);
        let text = line_to_plain(&line);
        assert_eq!(text, "\u{2502} tool output");
    }

    #[test]
    fn slice_text_respects_column_bounds() {
        let text = "hello world";
        assert_eq!(slice_text(text, 0, 5), "hello");
        assert_eq!(slice_text(text, 6, 11), "world");
        assert_eq!(slice_text(text, 0, 0), "");
        assert_eq!(slice_text(text, 0, 100), text);
    }

    #[test]
    fn slice_text_handles_multibyte_characters() {
        let text = "a─b"; // U+2500 在支持的终端上为 1 显示列
        assert_eq!(slice_text(text, 1, 2), "─");
        assert_eq!(slice_text(text, 0, 3), text);
    }

    #[test]
    fn slice_text_truncates_at_end() {
        let text = "ab";
        assert_eq!(slice_text(text, 1, 5), "b");
    }

    // --- Unicode / CJK / 终端宽度质量保证（问题 #3488）-----------------------
    // 这些直接使用生产宽度辅助函数，因此断言跟踪渲染器使用的相同代码路径。

    #[test]
    fn text_display_width_counts_cjk_as_two_columns() {
        assert_eq!(text_display_width("中文"), 4); // 两个宽字形
        assert_eq!(text_display_width("Hello世界"), 9); // 5 ASCII + 2×2
        // 全宽（歧义→宽）标点符号每字符两列。
        assert_eq!(text_display_width("，。！？"), 8);
    }

    #[test]
    fn text_display_width_treats_zero_width_marks_as_zero() {
        // 组合标记不增加列："e" + U+0301 渲染为一个单元格。
        //（回归防范：旧的 `.max(1)` 将其计为 1，高估宽度
        // 导致含组合标记或 ZWJ 表情符号序列的文本过早截断/边框漂移。）
        assert_eq!(text_display_width("e\u{0301}"), 1);
        assert_eq!(text_display_width("cafe\u{0301}"), 4);
        // ZWJ 连接器本身是零宽；两个表情符号各 2 列。
        assert_eq!(text_display_width("\u{1F469}\u{200D}\u{1F4BB}"), 4);
    }

    #[test]
    fn text_display_width_keeps_control_and_tab_widths() {
        // 控制字符仍占用一列（避免布局坍塌）；tab = 4。
        assert_eq!(text_display_width("a\u{0007}b"), 3);
        assert_eq!(text_display_width("\t"), 4);
        assert_eq!(text_display_width("\ta"), 5);
    }

    #[test]
    fn truncate_line_to_width_respects_display_width_not_byte_len() {
        // 字符串在显示宽度上已适合时无需截断。
        assert_eq!(truncate_line_to_width("中文", 10), "中文");
        // 超出：为省略号保留 3 列，其余按宽度填充。
        let out = truncate_line_to_width("中文测试", 7);
        assert_eq!(out, "中文...");
        assert_eq!(text_display_width(&out), 7);
        // 永远不跨边界分割宽字形，也从不发出 U+FFFD。
        let clipped = truncate_line_to_width("界界界界界", 5);
        assert!(text_display_width(&clipped) <= 5);
        assert!(!clipped.contains('\u{FFFD}'));
    }

    #[test]
    fn semantic_truncate_prefers_word_boundaries() {
        let out = semantic_truncate("hello world foo bar", 14);
        assert_eq!(out, "hello world…");
        assert!(text_display_width(&out) <= 14);
    }

    #[test]
    fn semantic_truncate_falls_back_with_long_words_and_wide_glyphs() {
        let long_word = semantic_truncate("supercalifragilistic", 8);
        assert_eq!(long_word, "superca…");
        assert!(text_display_width(&long_word) <= 8);

        let cjk = semantic_truncate("中文测试文本", 7);
        assert_eq!(cjk, "中文测…");
        assert!(text_display_width(&cjk) <= 7);
    }

    #[test]
    fn semantic_truncate_handles_empty_and_tiny_budgets() {
        assert_eq!(semantic_truncate("", 10), "");
        assert_eq!(semantic_truncate("hello", 0), "");
        assert_eq!(semantic_truncate("hello", 1), "…");
    }

    #[test]
    fn semantic_truncate_between_affixes_reserves_fixed_columns() {
        let hint = semantic_truncate_between_affixes(
            " > [ ] Prefix stability  (",
            "whether system/tools stayed cacheable",
            ")",
            49,
        );
        let row = format!(" > [ ] Prefix stability  ({hint})");
        assert_eq!(hint, "whether system/tools…");
        assert!(text_display_width(&row) <= 49);
    }

    #[test]
    fn slice_text_slices_cjk_by_display_column() {
        // 列：中=[0,2) 文=[2,4) a=[4,5) b=[5,6)
        let text = "中文ab";
        assert_eq!(slice_text(text, 0, 2), "中");
        assert_eq!(slice_text(text, 2, 4), "文");
        assert_eq!(slice_text(text, 4, 6), "ab");
    }

    #[test]
    fn concise_shell_command_label_prefers_gh_pr_checks_over_wrappers() {
        let label = concise_shell_command_label(
            "cd /tmp/repo && sleep 15 && gh pr checks 1611 --repo Hmbown/CodeWhale",
            80,
        );
        assert_eq!(label, "gh pr checks 1611");
    }

    #[test]
    fn concise_shell_command_label_falls_back_to_actionable_segment() {
        let label = concise_shell_command_label("cd /tmp/repo && cargo test --workspace", 80);
        assert_eq!(label, "cargo test --workspace");
    }

    #[test]
    fn concise_shell_command_label_strips_ansi_before_collapsing_text() {
        let label = concise_shell_command_label(
            "cd /repo && \x1b[38;2;6;174;242mcargo test\x1b[0m --workspace",
            80,
        );
        assert_eq!(label, "cargo test --workspace");
        assert!(!label.contains("38;2"));
    }

    // --- 新的 #3488 测试数据：选择器样式行的 CJK/宽字形截断。
    // truncate_line_to_width 是侧边栏（file_tree）、状态栏（footer_ui）、
    // 热栏和选择器（mouse_ui）行渲染的生产辅助函数，
    // 因此这些测试执行与那些表面相同的截断路径。

    #[test]
    fn truncate_line_to_width_full_width_cjk_lands_on_glyph_boundary() {
        // 每个汉字字形为两列。使用奇数预算时，截断必须落在完整字形边界上
        //（为省略号保留三列），从不留下半渲染的宽单元格或发出 U+FFFD。
        let title = "项目报告结果"; // 6 个字形，12 列
        let out = truncate_line_to_width(title, 7);
        // 预算 7 -> 限制 4 列 -> 两个字形适合，然后是省略号。
        assert_eq!(out, "项目...");
        assert_eq!(text_display_width(&out), 7);
        // 保留的前缀仅由完整的宽字形组成（每个 2 列），
        // 证明边界字形被完整丢弃，而非分割。
        let prefix = out.strip_suffix("...").expect("省略号存在");
        assert!(prefix.chars().all(|c| char_display_width(c) == 2));
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn truncate_line_to_width_mixed_ascii_cjk_row_keeps_ellipsis_within_budget() {
        // 侧边栏/选择器行混合 ASCII 标签和 CJK 标题，宽度超过列预算，
        // 必须截断并带尾随省略号，仍适合显示宽度，且不得分割宽字形。
        let row = "Task: 数据库迁移任务 done"; // ASCII 标签 + 7 个汉字字形
        let budget = 12;
        let out = truncate_line_to_width(row, budget);
        assert!(out.ends_with("..."), "预期省略号，得到 {out:?}");
        // 省略号和内容在*显示*宽度内适应预算。
        assert!(text_display_width(&out) <= budget);
        // 非省略号前缀保持在预算减省略号的范围内，因此
        // 边界上的宽字形被完整丢弃而非半绘制。
        let prefix = out.strip_suffix("...").expect("省略号存在");
        assert!(text_display_width(prefix) <= budget - 3);
        assert!(!out.contains('\u{FFFD}'));
        // 语义的 ASCII 前缀在截断后存活。
        assert!(out.starts_with("Task:"));
    }

    #[test]
    fn truncate_line_to_width_dense_cjk_selector_row_survives_narrow_widths() {
        // 选择器/选择行在终端狭窄时通过 truncate_line_to_width 回退。
        // 带有前导标记字形和 CJK 内容的密集行在极小宽度下
        // 必须保持在预算内，不 panic 或从字形中间字节分割发出替换字符。
        let row = "▸ 中文项目 · main"; // 标记 + CJK + 分隔符 + 分支
        for width in [1usize, 2, 3, 4, 6, 8] {
            let out = truncate_line_to_width(row, width);
            assert!(
                text_display_width(&out) <= width,
                "width={width}: {out:?} 超出预算"
            );
            assert!(
                !out.contains('\u{FFFD}'),
                "width={width}: 截断分割了一个宽字形"
            );
        }
    }
}
