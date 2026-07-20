//! Whale/DeepSeek 终端主题令牌。
//!
//! 一个小的、刻意扁平的模块，命名了 TUI 正在使用的颜色、边框和内边距选择。
//! 所有值与之前针对 [`crate::palette`] 硬编码的深色调色板一致；
//! 在这里进行单一事实来源的更改可以稍后切换皮肤。引入此模块不会改变可见输出。
//!
//! 目前唯一的消费者是 [`crate::tui::history`] 中的计划和工具单元格渲染器，
//! 以及 [`crate::tui::ui`] 中的侧边栏部分装饰。
//! 所有其他调用点继续直接使用 [`crate::palette`]，直到后续阶段迁移。

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{BorderType, Borders, Padding};

use crate::palette;
use crate::palette::PaletteMode;
use crate::tui::history::ToolStatus;

/// 主题暴露的视觉变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Dark,
    Light,
    Grayscale,
}

/// 侧边栏、计划和工具渲染的集中化视觉令牌。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub variant: Variant,

    // 侧边栏 / 区域装饰
    pub section_borders: Borders,
    pub section_border_type: BorderType,
    pub section_border_color: Color,
    pub section_bg: Color,
    pub section_title_color: Color,
    pub section_padding: Padding,

    // 工具单元格颜色令牌
    pub tool_title_color: Color,
    pub tool_value_color: Color,
    pub tool_label_color: Color,
    pub tool_running_accent: Color,
    pub tool_success_accent: Color,
    pub tool_failed_accent: Color,

    // 计划单元格颜色令牌
    pub plan_progress_color: Color,
    pub plan_summary_color: Color,
    pub plan_explanation_color: Color,
    pub plan_pending_color: Color,
    pub plan_in_progress_color: Color,
    pub plan_completed_color: Color,
}

impl Theme {
    /// 当前的深色主题。当前可见输出使用这些值。
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            variant: Variant::Dark,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::BORDER_COLOR,
            section_bg: palette::WHALE_BG,
            section_title_color: palette::WHALE_ACCENT_PRIMARY,
            // 仅水平内边距。`Padding::uniform(1)` 占用了每个侧边栏面板的两行——
            // 对于紧凑型终端，Work/Tasks/Agents 通过 25% 布局分割总共获得约 3 行，
            // 这导致内容区域为零行（#63 后续：即使应该显示"无待办"/"无活跃计划"，
            // 面板也渲染为空框）。
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::TEXT_SOFT,
            tool_value_color: palette::TEXT_MUTED,
            tool_label_color: palette::TEXT_DIM,
            tool_running_accent: palette::ACCENT_TOOL_LIVE,
            tool_success_accent: palette::TEXT_DIM,
            tool_failed_accent: palette::ACCENT_TOOL_ISSUE,
            plan_progress_color: palette::STATUS_SUCCESS,
            plan_summary_color: palette::TEXT_MUTED,
            plan_explanation_color: palette::TEXT_DIM,
            plan_pending_color: palette::TEXT_MUTED,
            plan_in_progress_color: palette::STATUS_WARNING,
            plan_completed_color: palette::STATUS_SUCCESS,
        }
    }

    /// 侧边栏和工具装饰的浅色主题令牌。
    #[must_use]
    pub const fn light() -> Self {
        Self {
            variant: Variant::Light,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::LIGHT_BORDER,
            section_bg: palette::LIGHT_PANEL,
            section_title_color: palette::WHALE_ACCENT_PRIMARY,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::LIGHT_TEXT_SOFT,
            tool_value_color: palette::LIGHT_TEXT_MUTED,
            tool_label_color: palette::LIGHT_TEXT_HINT,
            tool_running_accent: palette::WHALE_ACCENT_PRIMARY,
            tool_success_accent: palette::LIGHT_TEXT_HINT,
            tool_failed_accent: palette::WHALE_ERROR,
            plan_progress_color: palette::WHALE_ACCENT_PRIMARY,
            plan_summary_color: palette::LIGHT_TEXT_MUTED,
            plan_explanation_color: palette::LIGHT_TEXT_HINT,
            plan_pending_color: palette::LIGHT_TEXT_MUTED,
            plan_in_progress_color: Color::Rgb(180, 83, 9),
            plan_completed_color: palette::WHALE_ACCENT_PRIMARY,
        }
    }

    /// Solarized Light 主题令牌——温暖的象牙色调，高对比度。
    #[must_use]
    pub const fn solarized_light() -> Self {
        Self {
            variant: Variant::Light,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::SOLARIZED_BORDER,
            section_bg: palette::SOLARIZED_PANEL,
            section_title_color: palette::SOLARIZED_BLUE,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::SOLARIZED_TEXT_SOFT,
            tool_value_color: palette::SOLARIZED_TEXT_MUTED,
            tool_label_color: palette::SOLARIZED_TEXT_DIM,
            tool_running_accent: palette::SOLARIZED_BLUE,
            tool_success_accent: palette::SOLARIZED_CYAN,
            tool_failed_accent: palette::SOLARIZED_RED,
            plan_progress_color: palette::SOLARIZED_BLUE,
            plan_summary_color: palette::SOLARIZED_TEXT_MUTED,
            plan_explanation_color: palette::SOLARIZED_TEXT_DIM,
            plan_pending_color: palette::SOLARIZED_TEXT_MUTED,
            plan_in_progress_color: palette::SOLARIZED_ORANGE,
            plan_completed_color: palette::SOLARIZED_BLUE,
        }
    }

    /// 为想要最小化品牌色彩的用户提供的中性黑白令牌。
    #[must_use]
    pub const fn grayscale() -> Self {
        Self {
            variant: Variant::Grayscale,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::GRAYSCALE_BORDER,
            section_bg: palette::GRAYSCALE_PANEL,
            section_title_color: palette::GRAYSCALE_TEXT_SOFT,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::GRAYSCALE_TEXT_SOFT,
            tool_value_color: palette::GRAYSCALE_TEXT_MUTED,
            tool_label_color: palette::GRAYSCALE_TEXT_HINT,
            tool_running_accent: palette::GRAYSCALE_TEXT_SOFT,
            tool_success_accent: palette::GRAYSCALE_TEXT_HINT,
            tool_failed_accent: palette::GRAYSCALE_TEXT_BODY,
            plan_progress_color: palette::GRAYSCALE_TEXT_SOFT,
            plan_summary_color: palette::GRAYSCALE_TEXT_MUTED,
            plan_explanation_color: palette::GRAYSCALE_TEXT_HINT,
            plan_pending_color: palette::GRAYSCALE_TEXT_MUTED,
            plan_in_progress_color: palette::GRAYSCALE_TEXT_BODY,
            plan_completed_color: palette::GRAYSCALE_TEXT_SOFT,
        }
    }

    #[must_use]
    pub const fn for_palette_mode(mode: PaletteMode) -> Self {
        match mode {
            PaletteMode::Dark => Self::dark(),
            PaletteMode::Light => Self::light(),
            PaletteMode::Grayscale => Self::grayscale(),
            PaletteMode::SolarizedLight => Self::solarized_light(),
        }
    }

    /// 为给定的 [`ToolStatus`] 选择合适的工具强调色。
    #[must_use]
    pub const fn tool_status_color(self, status: ToolStatus) -> Color {
        match status {
            ToolStatus::Running => self.tool_running_accent,
            ToolStatus::Success => self.tool_success_accent,
            ToolStatus::Hydrated => self.tool_running_accent,
            ToolStatus::Failed => self.tool_failed_accent,
        }
    }

    /// 粗体工具标题样式（例如"Plan"、"Shell"）。
    #[must_use]
    pub fn tool_title_style(self) -> Style {
        Style::default()
            .fg(self.tool_title_color)
            .add_modifier(Modifier::BOLD)
    }

    /// 右侧状态文本（"running"、"done"、"issue"）样式。
    #[must_use]
    pub fn tool_status_style(self, status: ToolStatus) -> Style {
        Style::default().fg(self.tool_status_color(status))
    }

    /// 详情标签样式（"command:"、"time:"、步骤标记）。
    #[must_use]
    pub fn tool_label_style(self) -> Style {
        Style::default().fg(self.tool_label_color)
    }

    /// 工具详情行的默认值样式。
    #[must_use]
    pub fn tool_value_style(self) -> Style {
        Style::default().fg(self.tool_value_color)
    }
}

/// 返回 TUI 当前使用的活动主题。
#[must_use]
pub const fn active_theme() -> Theme {
    Theme::dark()
}

#[cfg(test)]
mod tests {
    use super::{Theme, Variant, active_theme};
    use crate::palette;
    use crate::tui::history::ToolStatus;

    #[test]
    fn active_theme_returns_dark() {
        assert_eq!(active_theme(), Theme::dark());
    }

    #[test]
    fn dark_theme_matches_existing_palette_choices() {
        let theme = Theme::dark();
        assert_eq!(theme.variant, Variant::Dark);
        assert_eq!(theme.section_border_color, palette::BORDER_COLOR);
        assert_eq!(theme.section_bg, palette::WHALE_BG);
        assert_eq!(theme.section_title_color, palette::WHALE_ACCENT_PRIMARY);
        assert_eq!(theme.tool_title_color, palette::TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::TEXT_MUTED);
        assert_eq!(theme.tool_label_color, palette::TEXT_DIM);
        assert_eq!(theme.tool_running_accent, palette::ACCENT_TOOL_LIVE);
        assert_eq!(theme.tool_success_accent, palette::TEXT_DIM);
        assert_eq!(theme.tool_failed_accent, palette::ACCENT_TOOL_ISSUE);
    }

    #[test]
    fn light_theme_uses_light_panel_tokens() {
        let theme = Theme::for_palette_mode(crate::palette::PaletteMode::Light);
        assert_eq!(theme.variant, Variant::Light);
        assert_eq!(theme.section_bg, palette::LIGHT_PANEL);
        assert_eq!(theme.section_border_color, palette::LIGHT_BORDER);
        assert_eq!(theme.tool_title_color, palette::LIGHT_TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::LIGHT_TEXT_MUTED);
        assert_eq!(theme.plan_summary_color, palette::LIGHT_TEXT_MUTED);
    }

    #[test]
    fn grayscale_theme_uses_neutral_tokens() {
        let theme = Theme::for_palette_mode(crate::palette::PaletteMode::Grayscale);
        assert_eq!(theme.variant, Variant::Grayscale);
        assert_eq!(theme.section_bg, palette::GRAYSCALE_PANEL);
        assert_eq!(theme.section_border_color, palette::GRAYSCALE_BORDER);
        assert_eq!(theme.tool_running_accent, palette::GRAYSCALE_TEXT_SOFT);
        assert_eq!(theme.tool_failed_accent, palette::GRAYSCALE_TEXT_BODY);
        assert_eq!(theme.plan_summary_color, palette::GRAYSCALE_TEXT_MUTED);
    }

    #[test]
    fn tool_status_color_maps_each_status() {
        let theme = Theme::dark();
        assert_eq!(
            theme.tool_status_color(ToolStatus::Running),
            theme.tool_running_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Success),
            theme.tool_success_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Hydrated),
            theme.tool_running_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Failed),
            theme.tool_failed_accent
        );
    }
}
