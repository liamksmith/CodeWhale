//! Act / Plan / Operate 的 `/mode` 选择器。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::Locale;
use crate::palette;
use crate::tui::app::AppMode;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer, render_modal_surface,
};

pub struct ModePickerView {
    cursor: usize,
    locale: Locale,
}

impl ModePickerView {
    #[must_use]
    pub fn new(current: AppMode, locale: Locale) -> Self {
        let cursor = AppMode::CHOICES
            .iter()
            .position(|mode| *mode == current)
            .unwrap_or(0);
        Self { cursor, locale }
    }

    fn selected_mode(&self) -> AppMode {
        AppMode::CHOICES
            .get(self.cursor)
            .copied()
            .unwrap_or(AppMode::Agent)
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        let max = AppMode::CHOICES.len().saturating_sub(1);
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    fn select_by_number(&mut self, number: char) -> Option<ViewAction> {
        let idx = AppMode::CHOICES
            .iter()
            .position(|mode| mode.number() == number)?;
        self.cursor = idx;
        Some(ViewAction::EmitAndClose(ViewEvent::ModeSelected {
            mode: self.selected_mode(),
        }))
    }
}

impl ModalView for ModePickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ModePicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Enter => ViewAction::EmitAndClose(ViewEvent::ModeSelected {
                mode: self.selected_mode(),
            }),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ViewAction::None
            }
            KeyCode::Char(number) => self.select_by_number(number).unwrap_or(ViewAction::None),
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_height = u16::try_from(AppMode::CHOICES.len()).unwrap_or(3) + 7;
        let popup_area = centered_modal_area(area, 68, popup_height, 44, 8);

        render_modal_surface(area, popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Mode ",
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("↑/↓", "move"),
                ActionHint::new("Enter", "select"),
                ActionHint::new("Esc", "cancel"),
            ],
        );

        let mut lines = Vec::with_capacity(AppMode::CHOICES.len());

        for (idx, mode) in AppMode::CHOICES.iter().copied().enumerate() {
            let is_cursor = idx == self.cursor;
            let row_style = if is_cursor {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_cursor {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            let pointer = if is_cursor { ">" } else { " " };
            let name = mode.display_name_localized(self.locale);
            // 按终端列宽填充，而不是字符数，这样宽字符（CJK）模式名称能保持提示列对齐。
            let pad = " ".repeat(7usize.saturating_sub(UnicodeWidthStr::width(&*name)));

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{pointer} {}. {name}{pad}", mode.number()),
                    row_style,
                ),
                Span::styled(mode.picker_hint_localized(self.locale), hint_style),
            ]));
        }

        Paragraph::new(lines).render(content, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn opens_on_current_mode() {
        let view = ModePickerView::new(AppMode::Plan, Locale::En);
        assert_eq!(view.selected_mode(), AppMode::Plan);
    }

    #[test]
    fn enter_emits_selected_mode() {
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Plan);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }
    }

    /// v0.8.66 模态拦截器 (#3732) 要求每个覆盖层在这些终端尺寸下保持可读和完全可操作的四个尺寸。
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    fn render_at(width: u16, height: u16) -> (Buffer, Rect) {
        use crate::tui::views::ViewStack;
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        // 预填充一个标记，这样任何组合模态未能绘制到的单元格（透漏）可通过残留的 'X' 检测出来。
        for y in 0..height {
            for x in 0..width {
                buf[(x, y)].set_symbol("X");
            }
        }
        // 通过 ViewStack 渲染，这样共享的不透明背景与生产环境中的绘制方式完全一致。
        let mut stack = ViewStack::new();
        stack.push(ModePickerView::new(AppMode::Agent, Locale::En));
        stack.render(area, &mut buf);
        (buf, area)
    }

    fn rows(buf: &Buffer, area: Rect) -> Vec<String> {
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn mode_picker_is_usable_and_opaque_at_blocker_sizes() {
        for (w, h) in BLOCKER_SIZES {
            let (buf, area) = render_at(w, h);
            let text = rows(&buf, area).join("\n");

            // 操作标签存在（页脚从不丢弃操作）。
            assert!(text.contains("move"), "{w}x{h}: missing 'move' hint");
            assert!(text.contains("select"), "{w}x{h}: missing 'select' hint");
            assert!(text.contains("cancel"), "{w}x{h}: missing 'cancel' hint");

            // 组合框架完全不透明：没有残留的标记，每个单元格都带有模态/背景的墨水背景。
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            let center = &buf[(w / 2, h / 2)];
            assert_eq!(
                center.bg,
                palette::WHALE_BG,
                "{w}x{h}: modal interior must be opaque"
            );

            // 没有行超出框架宽度（无水平溢出）。
            for (y, row) in rows(&buf, area).iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }

    #[test]
    fn number_keys_select_modes() {
        // 可见的名册：1 Act、2 Plan、3 Operate。没有 Multitask / YOLO / 间隔。
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Operate);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }

        // 选择器不提供旧版 YOLO 快捷方式 (4)。
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));

        // 旧版 Operate 编号 (5) 已消失——没有数字间隔。
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
    }
}
