//! 结构化用户输入的选择卡组件。
//!
//! 当 Brother Whale 需要输入时，它会展示一个选择卡：一个带标签的问题，
//! 后跟编号的选项，默认选项高亮显示。用户使用 1-9 键（或 j/k / 上/下）
//! 进行导航，按 Enter 确认。每个决策都会被记录，以便用户稍后检查选择。
//!
//! 这取代了模糊的"我该做什么？"提示，提供了一个结构化的选择界面——
//! 来自 v0.8.43 truth-surface 跟踪器的验收标准。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Widget},
};

use super::renderable::Renderable;

/// 选择卡中的单个选项。
#[derive(Debug, Clone)]
pub struct DecisionOption {
    /// 选项的简短标签（例如"应用补丁"）。
    pub label: String,
    /// 标签下方显示的可选详细描述。
    pub description: Option<String>,
}

/// 向用户展示结构化选择的选择卡。
#[derive(Debug, Clone)]
pub struct DecisionCard {
    /// 用户正在回答的问题或提示。
    pub question: String,
    /// 可用选项列表。每个选项编号为 1..N。
    pub options: Vec<DecisionOption>,
    /// `options` 中默认（高亮）选项的索引。
    pub default_index: usize,
    /// 当前选中选项的索引。
    pub selected_index: usize,
    /// 选择卡是否已提交（按下了 Enter）。
    pub confirmed: bool,
    /// 已确认的选项索引（如果有）。
    pub confirmed_index: Option<usize>,
}

impl DecisionCard {
    pub fn new(question: String, options: Vec<DecisionOption>, default_index: usize) -> Self {
        let default = default_index.min(options.len().saturating_sub(1));
        Self {
            question,
            options,
            default_index: default,
            selected_index: default,
            confirmed: false,
            confirmed_index: None,
        }
    }

    /// 选项数量。
    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    /// 向上移动选择（循环）。
    pub fn select_prev(&mut self) {
        if self.option_count() == 0 {
            return;
        }
        self.selected_index = self
            .selected_index
            .checked_sub(1)
            .unwrap_or(self.option_count() - 1);
    }

    /// 向下移动选择（循环）。
    pub fn select_next(&mut self) {
        if self.option_count() == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.option_count();
    }

    /// 通过数字键选择（从 1 开始）。
    pub fn select_number(&mut self, n: usize) {
        if n > 0 && n <= self.option_count() {
            self.selected_index = n - 1;
        }
    }

    /// 确认当前选择。
    pub fn confirm(&mut self) {
        self.confirmed = true;
        self.confirmed_index = Some(self.selected_index);
    }

    /// 获取已确认选项的标签（如果有）。
    pub fn confirmed_label(&self) -> Option<&str> {
        self.confirmed_index
            .and_then(|i| self.options.get(i))
            .map(|opt| opt.label.as_str())
    }
}

impl Default for DecisionCard {
    fn default() -> Self {
        Self::new(String::new(), Vec::new(), 0)
    }
}

impl Renderable for DecisionCard {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        let border_style = Style::default().fg(Color::Rgb(100, 160, 220));
        let question_style = Style::default()
            .fg(Color::Rgb(220, 220, 240))
            .add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(Color::Rgb(140, 140, 160));
        let selected_style = Style::default()
            .fg(Color::Rgb(80, 200, 255))
            .add_modifier(Modifier::BOLD);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Decision Required ")
            .title_style(question_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 2 || inner.height < 2 {
            return;
        }

        let mut y = inner.y;

        // 问题行
        let question = truncate_to_width(&self.question, inner.width as usize);
        buf.set_string(inner.x, y, &question, question_style);
        y += 1;

        if y >= inner.y + inner.height {
            return;
        }

        // 分隔线
        let sep = "─".repeat(inner.width as usize);
        buf.set_string(inner.x, y, &sep, dim_style);
        y += 1;

        // 选项
        let max_options = (inner.y + inner.height).saturating_sub(y) as usize;
        for (i, option) in self.options.iter().enumerate().take(max_options) {
            if y >= inner.y + inner.height {
                break;
            }

            let num = format!("{}.", i + 1);
            let is_selected = i == self.selected_index;
            let style = if is_selected {
                selected_style
            } else {
                dim_style
            };

            // "1. 标签 (默认)" 或 "1. 标签"
            let mut label = format!("{} {}", num, option.label);
            if i == self.default_index {
                label.push_str(" (default)");
            }
            label = truncate_to_width(&label, inner.width.saturating_sub(1) as usize);

            let prefix = if is_selected { "▸ " } else { "  " };
            let full_label = format!("{prefix}{label}");
            buf.set_string(inner.x, y, &full_label, style);
            y += 1;

            // 描述行（如果有）
            if let Some(ref desc) = option.description
                && y < inner.y + inner.height
            {
                let desc = format!(
                    "    {}",
                    truncate_to_width(desc, inner.width.saturating_sub(5) as usize)
                );
                buf.set_string(inner.x, y, &desc, dim_style);
                y += 1;
            }
        }

        // 底部提示
        if y < inner.y + inner.height {
            let hint = "1-9 select  ·  j/k navigate  ·  Enter confirm";
            let hint = truncate_to_width(hint, inner.width as usize);
            buf.set_string(inner.x, y, &hint, dim_style);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // 问题 + 分隔线 + 选项 + 底部
        let option_lines: u16 = self
            .options
            .iter()
            .map(|o| if o.description.is_some() { 2 } else { 1 })
            .sum();
        // 2 行边框，1 行问题，1 行分隔线，选项，1 行底部
        2 + 1 + 1 + option_lines + 1
    }
}

fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let truncated: String = chars.into_iter().take(max_width - 1).collect();
    format!("{truncated}…")
}
