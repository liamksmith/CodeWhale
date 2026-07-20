//! 推理/思考记录单元的渲染。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;
use crate::tui::markdown_render;
use crate::tui::ui_text::truncate_line_to_width;

/// 推理头部打开标记。替换思考单元格上的旋转器字形 —
/// 推理是缓慢的呼气，而不是工具旋转。
pub(super) const REASONING_OPENER: &str = "\u{2026}"; // …
/// 推理主体左侧导轨。使用虚线（`╎`）代替实心 `▏` 块，以
/// 在视觉上将推理与消息主体和工具输出分开。
pub(super) const REASONING_RAIL: &str = "\u{254E} "; // ╎ + 空格
/// 流式推理的尾行光标。锚定到实时颜色，
/// 以便用户看到新 token 落下的位置。
pub(super) const REASONING_CURSOR: &str = "\u{258E}"; // ▎

const THINKING_SUMMARY_LINE_LIMIT: usize = 4;
const THINKING_COMPLETED_PREVIEW_LINE_LIMIT: usize = 6;
const THINKING_STREAMING_PREVIEW_LINE_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingVisualState {
    Live,
    Done,
    Idle,
}

#[allow(dead_code)] // 为兼容性/测试保留；实时视图仅使用显式摘要。
#[must_use]
pub fn extract_reasoning_summary(text: &str) -> Option<String> {
    extract_explicit_reasoning_summary(text).or_else(|| {
        let fallback = text.trim();
        if fallback.is_empty() {
            None
        } else {
            Some(fallback.to_string())
        }
    })
}

fn extract_explicit_reasoning_summary(text: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with("summary") {
            let mut summary = String::new();
            if let Some((_, rest)) = trimmed.split_once(':')
                && !rest.trim().is_empty()
            {
                summary.push_str(rest.trim());
                summary.push('\n');
            }
            while let Some(next) = lines.peek() {
                let next_trimmed = next.trim();
                if next_trimmed.is_empty() {
                    break;
                }
                if next_trimmed.starts_with('#') || next_trimmed.starts_with("**") {
                    break;
                }
                summary.push_str(next_trimmed);
                summary.push('\n');
                lines.next();
            }
            let summary = summary.trim().to_string();
            return if summary.is_empty() {
                None
            } else {
                Some(summary)
            };
        }
    }
    None
}

/// 从折叠的推理预览中编辑内部代码标识符，以便
/// 实现细节不会泄漏到默认记录中
/// (#4146/#4148). 每个 `snake_case` 标记（例如 `refresh_catalog_cache`、
/// `agent_id`、`DEEPSEEK_API_KEY`）都会折叠为一个 `…`，以便
/// 周围的散文仍然可读；完整的、未编辑的主体仍然
/// 可在展开（Space / Ctrl+O）以及分页器/剪贴板记录中查看。
fn redact_internal_identifiers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
            continue;
        }
        push_identifier_token(&mut out, &mut token);
        out.push(ch);
    }
    push_identifier_token(&mut out, &mut token);
    out
}

/// 将扫描的单词标记刷新到 `out` 中，当它读起来像内部代码标识符时
/// 将其替换为 `…`。对空标记无操作。
fn push_identifier_token(out: &mut String, token: &mut String) {
    if token.is_empty() {
        return;
    }
    if looks_like_internal_identifier(token) {
        out.push('\u{2026}');
    } else {
        out.push_str(token);
    }
    token.clear();
}

/// 当标记是 `snake_case` 运行时，它读起来像内部代码标识符：
/// 它包含下划线，至少有一个字母，并且除此之外
/// 仅由 ASCII 字母数字/下划线组成。普通散文单词从不匹配。
fn looks_like_internal_identifier(token: &str) -> bool {
    token.contains('_')
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub(super) fn render_thinking(
    content: &str,
    width: u16,
    streaming: bool,
    duration_secs: Option<f32>,
    collapsed: bool,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let state = thinking_visual_state(streaming, duration_secs);
    let style = thinking_style();
    // 在应用墨水上的 12% 推理表面色调 — 记录中唯一有意的
    // 暖色元素。在 Ansi-16 终端上放弃，因为
    // 色调会扭曲命名调色板。
    let depth = cached_color_depth();
    let body_bg = palette::reasoning_surface_tint(depth);
    let body_style = match body_bg {
        Some(bg) => style.italic().bg(bg),
        None => style.italic(),
    };
    let mut lines = Vec::new();

    // 头部：`…` 打开标记（替换旋转器；推理不是工具，而是
    // 缓慢的呼气），后跟推理标签和实时状态。
    let mut header_spans = vec![
        Span::styled(
            format!("{REASONING_OPENER} "),
            Style::default().fg(thinking_state_accent(state)),
        ),
        Span::styled("reasoning", thinking_title_style()),
    ];
    header_spans.push(Span::styled(" ", Style::default()));
    header_spans.push(Span::styled(
        thinking_status_label(state),
        thinking_status_style(state),
    ));
    if let Some(dur) = duration_secs {
        header_spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        header_spans.push(Span::styled(format!("{dur:.1}s"), thinking_meta_style()));
    }
    lines.push(Line::from(header_spans));

    let content_width = width.saturating_sub(3).max(1);
    let mut collapsed_without_explicit_summary = false;
    let body_text = if collapsed {
        if streaming {
            // #861 RC4 / #1324：流式传输期间我们尚未获得
            // 完整的推理块，因此 `extract_reasoning_summary`
            // 没有意义。显示原始内容并让
            // 下面的截断逻辑保留 *最后* `LIMIT` 行，以便
            // 用户看到模型最新的思考，而不是
            // 盯着空占位符。
            content.to_string()
        } else {
            match extract_explicit_reasoning_summary(content) {
                Some(summary) => summary,
                None => {
                    collapsed_without_explicit_summary = true;
                    content.to_string()
                }
            }
        }
    } else {
        content.to_string()
    };
    // #4146/#4148：完成的推理在默认记录中折叠为安静的收据 —
    // 擦除内部代码标识符（函数名如 `refresh_catalog_cache`、
    // 原始 agent id），以便实现细节
    // 不会泄漏。流式推理保持原样（用户正在观看
    // 思考过程），展开/分页器/剪贴板记录保留完整的、
    // 未编辑的主体。编辑会更改 `body_text`，进而触发
    // 下面的提示，以便用户仍然看到
    // "展开查看完整推理"的提示。
    let body_text = if collapsed && !streaming {
        redact_internal_identifiers(&body_text)
    } else {
        body_text
    };
    let mut rendered = if body_text.trim().is_empty() {
        Vec::new()
    } else {
        markdown_render::render_markdown(&body_text, content_width, body_style)
    };
    let mut truncated = false;
    let line_limit = if streaming {
        THINKING_STREAMING_PREVIEW_LINE_LIMIT
    } else if collapsed_without_explicit_summary {
        THINKING_COMPLETED_PREVIEW_LINE_LIMIT
    } else {
        THINKING_SUMMARY_LINE_LIMIT
    };
    if collapsed && rendered.len() > line_limit {
        if streaming {
            // 流式传输期间丢弃 *头部*，以便可见窗口
            // 跟踪底部的实时光标。
            let drop = rendered.len() - line_limit;
            rendered.drain(0..drop);
        } else {
            rendered.truncate(line_limit);
        }
        truncated = true;
    }

    let rail_style = Style::default().fg(thinking_state_accent(state));
    let cursor_style = Style::default().fg(palette::ACCENT_REASONING_LIVE);

    if rendered.is_empty() && streaming {
        let mut spans = vec![Span::styled(REASONING_RAIL.to_string(), rail_style)];
        spans.push(Span::styled("reasoning...", body_style.italic()));
        if !low_motion {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    let last_idx = rendered.len().saturating_sub(1);
    for (idx, line) in rendered.into_iter().enumerate() {
        let mut spans = vec![Span::styled(REASONING_RAIL.to_string(), rail_style)];
        spans.extend(line.spans);
        // 流式传输时在最后一行主体上放置尾行光标 —
        // 表示"仍在生成"而不刷新每一行。
        if streaming && !low_motion && idx == last_idx {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    let needs_affordance = collapsed
        && if streaming {
            // #861 RC4 / #1324：流式传输期间，只要有任何
            // 头部行被裁剪，就显示提示，以便用户
            // 知道上方还有更多内容以及如何访问它。
            truncated
        } else {
            truncated || body_text.trim() != content.trim()
        };
    if needs_affordance {
        let label = if streaming {
            "更多推理内容在 Ctrl+O 中"
        } else {
            "Space 展开 · 完整推理在 Ctrl+O 中"
        };
        lines.push(Line::from(vec![
            Span::styled(REASONING_RAIL.to_string(), rail_style),
            Span::styled(label, Style::default().fg(palette::TEXT_MUTED).italic()),
        ]));
    }

    lines
}

pub(super) fn render_hidden_thinking_activity(
    width: u16,
    duration_secs: Option<f32>,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let state = ThinkingVisualState::Live;
    let rail_style = Style::default().fg(thinking_state_accent(state));
    let body_style = thinking_style().italic();
    let content_width = width.saturating_sub(3).max(1) as usize;

    let mut header_spans = vec![
        Span::styled(
            format!("{REASONING_OPENER} "),
            Style::default().fg(thinking_state_accent(state)),
        ),
        Span::styled("reasoning", thinking_title_style()),
        Span::styled(" ", Style::default()),
        Span::styled(thinking_status_label(state), thinking_status_style(state)),
    ];
    if let Some(dur) = duration_secs {
        header_spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        header_spans.push(Span::styled(format!("{dur:.1}s"), thinking_meta_style()));
    }

    let mut body =
        truncate_line_to_width("推理已隐藏；模型仍在工作", content_width);
    if !low_motion {
        body.push(' ');
        body.push_str(REASONING_CURSOR);
    }

    vec![
        Line::from(header_spans),
        Line::from(vec![
            Span::styled(REASONING_RAIL.to_string(), rail_style),
            Span::styled(body, body_style),
        ]),
    ]
}

fn thinking_style() -> Style {
    Style::default().fg(palette::TEXT_REASONING)
}

fn thinking_visual_state(streaming: bool, duration_secs: Option<f32>) -> ThinkingVisualState {
    if streaming {
        ThinkingVisualState::Live
    } else if duration_secs.is_some() {
        ThinkingVisualState::Done
    } else {
        ThinkingVisualState::Idle
    }
}

fn thinking_status_label(state: ThinkingVisualState) -> &'static str {
    match state {
        ThinkingVisualState::Live => "live",
        ThinkingVisualState::Done => "done",
        ThinkingVisualState::Idle => "idle",
    }
}

fn thinking_title_style() -> Style {
    Style::default()
        .fg(palette::TEXT_SOFT)
        .add_modifier(Modifier::BOLD)
}

fn thinking_status_style(state: ThinkingVisualState) -> Style {
    Style::default().fg(match state {
        ThinkingVisualState::Live => palette::ACCENT_REASONING_LIVE,
        ThinkingVisualState::Done => palette::TEXT_DIM,
        ThinkingVisualState::Idle => palette::TEXT_DIM,
    })
}

fn thinking_meta_style() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

fn thinking_state_accent(state: ThinkingVisualState) -> Color {
    match state {
        ThinkingVisualState::Live => palette::ACCENT_REASONING_LIVE,
        ThinkingVisualState::Done => palette::TEXT_DIM,
        ThinkingVisualState::Idle => palette::TEXT_DIM,
    }
}

/// 终端会话的一次初始化颜色深度。避免在每一帧都重新读取
/// `COLORTERM` / `TERM` 环境变量。
static COLOR_DEPTH: std::sync::OnceLock<palette::ColorDepth> = std::sync::OnceLock::new();

fn cached_color_depth() -> palette::ColorDepth {
    *COLOR_DEPTH.get_or_init(palette::ColorDepth::detect)
}
