//! 带有实时预览的 `/theme` 选择器。
//!
//! 仿照 `feedback_picker` 设计。区别：
//! - 选项列表来自 `palette::SELECTABLE_THEMES`。
//! - Up/Down 发出 `ConfigUpdated{persist:false}`，使宿主立即交换
//!   `app.ui_theme`，整个 TUI 在模态框下方重新绘制——
//!   用户在提交前就能看到候选主题。
//! - Enter 持久化（`persist:true`）；Esc 再发出一个
//!   `ConfigUpdated{persist:false}` 以恢复选择器打开时
//!   处于活跃状态的原始主题名称。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::palette::{SELECTABLE_THEMES, ThemeId, UiTheme};
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer,
};

pub struct ThemePickerView {
    selected: usize,
    /// 选择器打开时处于活跃状态的主题的设置名称。用于在 Esc 时恢复。
    original_name: String,
    /// `ThemeId::System` 的缓存 UiTheme，在构造时捕获一次，
    /// 这样每帧渲染不会在每个按键时都重新调用 `UiTheme::detect()`（它会读取 `COLORFGBG`）。
    system_ui_theme: UiTheme,
}

impl ThemePickerView {
    #[must_use]
    pub fn new(original_name: String) -> Self {
        // 如果持久化的名称匹配某个条目，则从那里开始；
        // 否则回退到 "System"，以便光标落在有效行上。
        let selected = SELECTABLE_THEMES
            .iter()
            .position(|id| id.name() == original_name.trim().to_ascii_lowercase())
            .unwrap_or(0);
        Self {
            selected,
            original_name,
            system_ui_theme: UiTheme::detect(),
        }
    }

    fn current(&self) -> ThemeId {
        SELECTABLE_THEMES
            .get(self.selected)
            .copied()
            .unwrap_or(ThemeId::System)
    }

    /// 将主题解析为 `UiTheme`，返回缓存的 `System` 解析以避免在 `render` 中重复读取环境变量。
    fn ui_theme_for(&self, id: ThemeId) -> UiTheme {
        if matches!(id, ThemeId::System) {
            self.system_ui_theme
        } else {
            id.ui_theme()
        }
    }

    fn preview_event(&self) -> ViewAction {
        ViewAction::Emit(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.current().name().to_string(),
            persist: false,
        })
    }

    fn commit_event(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.current().name().to_string(),
            persist: true,
        })
    }

    fn revert_event(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.original_name.clone(),
            persist: false,
        })
    }

    fn move_up(&mut self) {
        self.selected = (self.selected + SELECTABLE_THEMES.len() - 1) % SELECTABLE_THEMES.len();
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1) % SELECTABLE_THEMES.len();
    }
}

impl ModalView for ThemePickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ThemePicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_up(),
            MouseEventKind::ScrollDown => self.move_down(),
            _ => {}
        }
        ViewAction::None
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => self.revert_event(),
            KeyCode::Enter => self.commit_event(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                self.preview_event()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                self.preview_event()
            }
            KeyCode::Home => {
                self.selected = 0;
                self.preview_event()
            }
            KeyCode::End => {
                self.selected = SELECTABLE_THEMES.len().saturating_sub(1);
                self.preview_event()
            }
            // Number shortcuts: '1'..='9' jump to that row (1-indexed).
            // '0' is rejected explicitly — saturating_sub would otherwise
            // collapse it onto row 0, which is unintuitive.
            KeyCode::Char(c)
                if matches!(c, '1'..='9')
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let idx = (c as usize) - ('1' as usize);
                if idx < SELECTABLE_THEMES.len() {
                    self.selected = idx;
                    self.preview_event()
                } else {
                    ViewAction::None
                }
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // 1 个标题 + 1 个间距 + N 行 + 间距 + 正文内操作页脚。
        // centered_modal_area 严格限制在 `area` 内，因此模态框即使在
        // 微小或分屏终端上也始终适用。
        let needed_height = (SELECTABLE_THEMES.len() as u16).saturating_add(9);
        let popup_area = centered_modal_area(area, 78, needed_height, 44, 8);

        // 实时主题已通过 ConfigUpdated 在我们下方交换，
        // 因此我们从光标行提取*当前*预览的 UiTheme 来设置模态框外观。
        // 这样弹出框本身随光标移动而变色，匹配 Enter 后背景的样子。
        // 我们保留实时 `surface_bg`（而非共享墨水）和裸 `Clear`，
        // 以便预览背景按预期显示。
        let live = self.ui_theme_for(self.current());

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Theme ",
                Style::default()
                    .fg(live.status_working)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(live.border))
            .style(Style::default().bg(live.surface_bg))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("↑/↓", "preview"),
                ActionHint::new("Enter", "save"),
                ActionHint::new("Esc", "revert"),
            ],
        );

        let mut lines: Vec<Line> = Vec::with_capacity(SELECTABLE_THEMES.len() + 5);
        lines.push(Line::from(Span::styled(
            "Pick a theme — preview is live; Enter saves to settings.toml.",
            Style::default().fg(live.text_muted),
        )));
        lines.push(Line::from(""));

        for (idx, id) in SELECTABLE_THEMES.iter().enumerate() {
            let id = *id;
            let is_selected = idx == self.selected;
            let row_style = if is_selected {
                Style::default()
                    .fg(live.text_body)
                    .bg(live.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(live.text_body)
            };
            let tagline_style = if is_selected {
                Style::default().fg(live.text_muted).bg(live.selection_bg)
            } else {
                Style::default().fg(live.text_dim)
            };
            let number_style = if is_selected {
                Style::default()
                    .fg(live.status_working)
                    .bg(live.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(live.text_hint)
            };
            let pointer = if is_selected { "▶" } else { " " };

            // 每行 3 格色样，使用候选主题自身的强调色 + 面板 + 边框颜色，
            // 使选择器兼作图例。使用缓存解析器，以便 `System` 不重复 `UiTheme::detect()`。
            let row_theme = self.ui_theme_for(id);
            let swatch = vec![
                Span::styled("  ", Style::default().bg(row_theme.surface_bg)),
                Span::styled("  ", Style::default().bg(row_theme.panel_bg)),
                Span::styled("  ", Style::default().bg(row_theme.status_working)),
                Span::styled("  ", Style::default().bg(row_theme.mode_yolo)),
                Span::styled("  ", Style::default().bg(row_theme.mode_plan)),
            ];

            let mut spans: Vec<Span> = Vec::with_capacity(8);
            spans.push(Span::styled(format!(" {pointer} "), row_style));
            spans.push(Span::styled(format!("{}. ", idx + 1), number_style));
            spans.push(Span::styled(
                format!("{:<22}", id.display_name()),
                row_style,
            ));
            spans.extend(swatch);
            spans.push(Span::raw("  "));

            let prefix_width = Line::from(spans.clone()).width();
            let tagline = crate::tui::ui_text::semantic_truncate(
                id.tagline(),
                usize::from(content.width).saturating_sub(prefix_width),
            );
            spans.push(Span::styled(tagline, tagline_style));

            lines.push(Line::from(spans));
        }

        Paragraph::new(lines).render(content, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn selected_name(action: &ViewAction) -> Option<&str> {
        match action {
            ViewAction::Emit(ViewEvent::ConfigUpdated { key, value, .. })
            | ViewAction::EmitAndClose(ViewEvent::ConfigUpdated { key, value, .. })
                if key == "theme" =>
            {
                Some(value.as_str())
            }
            _ => None,
        }
    }

    #[test]
    fn opens_at_persisted_theme() {
        let v = ThemePickerView::new("tokyo-night".to_string());
        assert_eq!(v.current(), ThemeId::TokyoNight);
    }

    #[test]
    fn unknown_persisted_name_falls_back_to_first_row() {
        let v = ThemePickerView::new("not-a-real-theme".to_string());
        assert_eq!(v.selected, 0);
        assert_eq!(v.current(), ThemeId::System);
    }

    #[test]
    fn arrow_down_previews_next_theme() {
        let mut v = ThemePickerView::new("system".to_string());
        let action = v.handle_key(key(KeyCode::Down));
        assert!(matches!(action, ViewAction::Emit(_)));
        assert_eq!(selected_name(&action), Some(ThemeId::Terminal.name()));
    }

    #[test]
    fn arrow_navigation_wraps_at_picker_edges() {
        let mut v = ThemePickerView::new("system".to_string());
        let last = SELECTABLE_THEMES.last().unwrap();

        let action = v.handle_key(key(KeyCode::Up));
        assert_eq!(selected_name(&action), Some(last.name()));

        let action = v.handle_key(key(KeyCode::Down));
        assert_eq!(selected_name(&action), Some(SELECTABLE_THEMES[0].name()));
    }

    #[test]
    fn enter_commits_with_persist_true() {
        let mut v = ThemePickerView::new("system".to_string());
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down)); // -> CatppuccinMocha
        let action = v.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            }) => {
                assert_eq!(key, "theme");
                assert_eq!(value, ThemeId::CatppuccinMocha.name());
                assert!(persist);
            }
            other => panic!("expected commit, got {other:?}"),
        }
    }

    #[test]
    fn esc_reverts_to_original() {
        let mut v = ThemePickerView::new("dracula".to_string());
        v.handle_key(key(KeyCode::Up));
        v.handle_key(key(KeyCode::Up));
        let action = v.handle_key(key(KeyCode::Esc));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            }) => {
                assert_eq!(key, "theme");
                assert_eq!(value, "dracula");
                assert!(!persist);
            }
            other => panic!("expected revert, got {other:?}"),
        }
    }

    #[test]
    fn digit_jumps_to_row() {
        let mut v = ThemePickerView::new("system".to_string());
        let action = v.handle_key(key(KeyCode::Char('6')));
        // Row 6 (1-indexed) -> index 5 -> CatppuccinMocha
        assert_eq!(
            selected_name(&action),
            Some(ThemeId::CatppuccinMocha.name())
        );
    }

    #[test]
    fn digit_zero_is_rejected_not_remapped_to_row_zero() {
        let mut v = ThemePickerView::new("dracula".to_string());
        let before = v.selected;
        let action = v.handle_key(key(KeyCode::Char('0')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(v.selected, before, "'0' should not move the cursor");
    }

    #[test]
    fn render_does_not_panic_on_zero_sized_area() {
        // The picker historically panicked here via .max(W).max(H) floors
        // that produced dimensions larger than the available area, then
        // underflowed the centering arithmetic.
        let v = ThemePickerView::new("system".to_string());
        let outer = ratatui::layout::Rect::new(0, 0, 10, 10);
        let area = ratatui::layout::Rect::new(0, 0, 0, 0);
        let mut buf = ratatui::buffer::Buffer::empty(outer);
        v.render(area, &mut buf);
    }

    #[test]
    fn render_does_not_panic_on_tiny_area() {
        // 20×6 is smaller than every soft floor the picker prefers.
        let v = ThemePickerView::new("system".to_string());
        let area = ratatui::layout::Rect::new(0, 0, 20, 6);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        v.render(area, &mut buf);
    }

    #[test]
    fn render_semantically_truncates_taglines_at_narrow_width() {
        let v = ThemePickerView::new("system".to_string());
        let area = ratatui::layout::Rect::new(0, 0, 56, 12);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        v.render(area, &mut buf);
        let rows = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let text = rows.join("\n");

        assert!(text.contains('…'), "{text}");
        for (idx, row) in rows.iter().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(row) <= usize::from(area.width),
                "line {idx} overflows: {row:?}"
            );
        }
    }

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires
    /// every overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn theme_picker_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        use ratatui::{buffer::Buffer, layout::Rect};
        use unicode_width::UnicodeWidthStr;

        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(ThemePickerView::new("system".to_string()));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            for label in ["preview", "save", "revert"] {
                assert!(text.contains(label), "{w}x{h}: missing footer '{label}'");
            }
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            // The theme picker paints the *live* theme surface (not the shared
            // ink), so assert the center cell is painted (no surviving
            // sentinel) rather than checking a fixed background color.
            assert_ne!(
                buf[(w / 2, h / 2)].symbol(),
                "X",
                "{w}x{h}: modal interior must be painted"
            );
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }
}
