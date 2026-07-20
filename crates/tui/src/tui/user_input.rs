//! request_user_input 工具提示的模态框。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

use crate::palette;
use crate::tools::user_input::{
    UserInputAnswer, UserInputQuestion, UserInputRequest, UserInputResponse,
};
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent, render_modal_surface};

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            title.to_string(),
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .style(Style::default().bg(palette::WHALE_BG))
        .padding(Padding::uniform(1))
}

fn render_modal_chrome(area: Rect, popup_area: Rect, buf: &mut Buffer) {
    render_modal_surface(area, popup_area, buf);
}

fn push_option_lines(
    lines: &mut Vec<Line<'static>>,
    selected: bool,
    number: usize,
    label: String,
    description: String,
    ticked: bool,
) {
    let row_style = if selected {
        Style::default()
            .fg(palette::SELECTION_TEXT)
            .bg(palette::SELECTION_BG)
            .bold()
    } else {
        Style::default().fg(palette::TEXT_PRIMARY)
    };
    let detail_style = if selected {
        row_style
    } else {
        Style::default().fg(palette::TEXT_MUTED)
    };
    let prefix = if selected { ">" } else { " " };
    // 多选行在被切换到待处理集时显示复选标记槽，
    // 镜像其他多选项选择器中使用的互动元素。
    let mark = if ticked { "✔ " } else { "  " };

    lines.push(Line::from(Span::styled(
        format!("{prefix}{mark}{number}) {label}"),
        row_style,
    )));
    lines.push(Line::from(Span::styled(
        format!("      {description}"),
        detail_style,
    )));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Selecting,
    OtherInput,
}

#[derive(Debug, Clone)]
pub struct UserInputView {
    tool_id: String,
    request: UserInputRequest,
    question_index: usize,
    selected: usize,
    mode: InputMode,
    other_input: String,
    answers: Vec<UserInputAnswer>,
    /// 已切换到当前问题待处理多选集的索引。
    /// 仅当 `question.multi_select` 为 true 时使用。
    multi_pending: Vec<usize>,
}

impl UserInputView {
    pub fn new(tool_id: impl Into<String>, request: UserInputRequest) -> Self {
        Self {
            tool_id: tool_id.into(),
            request,
            question_index: 0,
            selected: 0,
            mode: InputMode::Selecting,
            other_input: String::new(),
            answers: Vec::new(),
            multi_pending: Vec::new(),
        }
    }

    fn current_question(&self) -> &UserInputQuestion {
        &self.request.questions[self.question_index]
    }

    /// 当前问题是否提供"其他"自由文本行。
    fn offers_other(&self) -> bool {
        self.current_question().allow_free_text
    }

    fn option_count(&self) -> usize {
        // 选项 + 条件"Other"行 + 条件"Confirm"行。
        let mut count = self.current_question().options.len();
        count += usize::from(self.offers_other());
        count += usize::from(self.is_multi_select());
        count
    }

    fn is_other_selected(&self) -> bool {
        // 当两者都存在时，"Other"位于确认行之前一位，否则在末尾。
        let other_last = !self.is_multi_select();
        if other_last {
            self.offers_other() && self.selected + 1 == self.option_count()
        } else {
            self.offers_other() && self.selected + 2 == self.option_count()
        }
    }

    /// 当多选"确认选择"行被高亮时返回 true。
    fn is_confirm_selected(&self) -> bool {
        self.is_multi_select() && self.selected + 1 == self.option_count()
    }

    fn is_multi_select(&self) -> bool {
        self.current_question().multi_select
    }

    fn toggle_pending(&mut self, index: usize) {
        if let Some(pos) = self.multi_pending.iter().position(|i| *i == index) {
            self.multi_pending.remove(pos);
        } else {
            self.multi_pending.push(index);
        }
    }

    /// 从单个选中的选项索引为当前问题构建答案
    ///（单选和多选的确认步骤）。
    fn answers_for_selection(&self, index: usize) -> Vec<UserInputAnswer> {
        let question = self.current_question();
        let option = &question.options[index];
        vec![UserInputAnswer {
            id: question.id.clone(),
            label: option.label.clone(),
            value: option.label.clone(),
        }]
    }

    fn advance_question(&mut self, new_answers: Vec<UserInputAnswer>) -> ViewAction {
        self.answers.extend(new_answers);
        if self.question_index + 1 >= self.request.questions.len() {
            let response = UserInputResponse {
                answers: self.answers.clone(),
            };
            return ViewAction::EmitAndClose(ViewEvent::UserInputSubmitted {
                tool_id: self.tool_id.clone(),
                response,
            });
        }
        self.question_index += 1;
        self.selected = 0;
        self.mode = InputMode::Selecting;
        self.other_input.clear();
        self.multi_pending.clear();
        ViewAction::None
    }

    fn handle_selecting_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.option_count().saturating_sub(1));
                ViewAction::None
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let Some(number) = ch.to_digit(10) else {
                    return ViewAction::None;
                };
                if number == 0 {
                    return ViewAction::None;
                }
                let index = usize::try_from(number - 1).unwrap_or(usize::MAX);
                if index >= self.option_count() {
                    return ViewAction::None;
                }
                self.selected = index;
                self.activate_or_confirm_selection()
            }
            KeyCode::Char(' ') if self.is_multi_select() => {
                // 空格切换待处理集中的高亮选项，
                // 而不离开选择器（标准多选互动元素）。
                if !self.is_other_selected() {
                    self.toggle_pending(self.selected);
                }
                ViewAction::None
            }
            KeyCode::Enter => self.activate_or_confirm_selection(),
            KeyCode::Esc => ViewAction::EmitAndClose(ViewEvent::UserInputCancelled {
                tool_id: self.tool_id.clone(),
            }),
            _ => ViewAction::None,
        }
    }

    /// 解析当前高亮行的数字/Enter 激活。
    ///
    /// - "Other"行 → 进入自由文本输入模式。
    /// - 多选选项 → 切换到待处理集（Enter 在专用的"确认"步骤上确认；
    ///   这里它只是切换，如 Space）。
    /// - 单选选项 → 立即提交（遗留行为）。
    fn activate_or_confirm_selection(&mut self) -> ViewAction {
        if self.is_other_selected() {
            self.mode = InputMode::OtherInput;
            self.other_input.clear();
            return ViewAction::None;
        }
        if self.is_multi_select() {
            if self.is_confirm_selected() {
                // 将待处理集刷新为此问题的答案。空集
                // 是允许的（类似跳过）——模型应提供
                // 合理的默认值，但我们不会死锁。
                let question = self.current_question();
                let answers: Vec<UserInputAnswer> = self
                    .multi_pending
                    .iter()
                    .filter_map(|i| question.options.get(*i))
                    .map(|opt| UserInputAnswer {
                        id: question.id.clone(),
                        label: opt.label.clone(),
                        value: opt.label.clone(),
                    })
                    .collect();
                return self.advance_question(answers);
            }
            // 在真实选项上按 Enter/Space 将其切换到待处理集。
            self.toggle_pending(self.selected);
            return ViewAction::None;
        }
        // 单选：立即提交。
        let answers = self.answers_for_selection(self.selected);
        self.advance_question(answers)
    }

    fn handle_other_input_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Selecting;
                self.other_input.clear();
                ViewAction::None
            }
            KeyCode::Enter => {
                let question = self.current_question();
                let answer = UserInputAnswer {
                    id: question.id.clone(),
                    label: "Other".to_string(),
                    value: self.other_input.trim().to_string(),
                };
                // 在多选模式下，自由文本"Other"仍是单个答案，
                // 附加到已切换的任何选项后。
                let mut answers: Vec<UserInputAnswer> = self
                    .multi_pending
                    .iter()
                    .filter_map(|i| question.options.get(*i))
                    .map(|opt| UserInputAnswer {
                        id: question.id.clone(),
                        label: opt.label.clone(),
                        value: opt.label.clone(),
                    })
                    .collect();
                answers.push(answer);
                self.advance_question(answers)
            }
            KeyCode::Backspace => {
                self.other_input.pop();
                ViewAction::None
            }
            KeyCode::Char('h')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.other_input.pop();
                ViewAction::None
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.other_input.push(ch);
                }
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }
}

impl ModalView for UserInputView {
    fn kind(&self) -> ModalKind {
        ModalKind::UserInput
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.mode {
            InputMode::Selecting => self.handle_selecting_key(key),
            InputMode::OtherInput => self.handle_other_input_key(key),
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let question = self.current_question();
        let total = self.request.questions.len();
        let header = format!(
            " {} ({}/{}) ",
            question.header,
            self.question_index + 1,
            total
        );

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "需要操作",
            Style::default().fg(palette::WHALE_INFO).bold(),
        )]));
        lines.push(Line::from(vec![
            Span::styled(
                question.header.clone(),
                Style::default().fg(palette::TEXT_PRIMARY).bold(),
            ),
            Span::styled(
                format!("  问题 {}/{}", self.question_index + 1, total),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            question.question.clone(),
            Style::default().fg(palette::TEXT_PRIMARY).bold(),
        )]));
        lines.push(Line::from(""));

        for (idx, option) in question.options.iter().enumerate() {
            let number = idx + 1;
            let ticked = self.is_multi_select() && self.multi_pending.contains(&idx);
            push_option_lines(
                &mut lines,
                self.selected == idx,
                number,
                option.label.clone(),
                option.description.clone(),
                ticked,
            );
        }

        // 自由文本"Other"行现在取决于 allow_free_text。
        if self.offers_other() {
            let other_index = question.options.len();
            let other_number = other_index + 1;
            push_option_lines(
                &mut lines,
                self.selected == other_index,
                other_number,
                "其他".to_string(),
                "输入自定义回复".to_string(),
                false,
            );
        }

        // 多选在选项（以及存在时的"Other"）后有一个专用的"确认选择"行。
        // 选中并在其上按 Enter 将待处理集刷新为问题的答案。
        if self.is_multi_select() {
            let confirm_index = self.option_count();
            let confirm_number = confirm_index + 1;
            push_option_lines(
                &mut lines,
                self.selected == confirm_index,
                confirm_number,
                "确认选择".to_string(),
                format!("提交 {} 个已选择", self.multi_pending.len()),
                false,
            );
        }

        if self.mode == InputMode::OtherInput {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "> 自定义回复:",
                    Style::default().fg(palette::TEXT_PRIMARY).bold(),
                ),
                Span::raw(" "),
                Span::styled(
                    if self.other_input.is_empty() {
                        "（输入您的回复）".to_string()
                    } else {
                        self.other_input.clone()
                    },
                    Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
                ),
            ]));
        }

        lines.push(Line::from(""));
        if self.mode == InputMode::OtherInput {
            lines.push(Line::from(vec![
                Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled(" 提交", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("  "),
                Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled(" 返回", Style::default().fg(palette::TEXT_MUTED)),
            ]));
        } else {
            let opt_count = self.option_count();
            let quick_pick_label = if opt_count <= 9 {
                format!("1-{opt_count}")
            } else {
                "数字".to_string()
            };
            if self.is_multi_select() {
                lines.push(Line::from(vec![
                    Span::styled(
                        quick_pick_label,
                        Style::default().fg(palette::WHALE_INFO).bold(),
                    ),
                    Span::styled(" 移动", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Space", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" 切换", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" 切换/确认", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" 取消", Style::default().fg(palette::TEXT_MUTED)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        quick_pick_label,
                        Style::default().fg(palette::WHALE_INFO).bold(),
                    ),
                    Span::styled(" 快速选择", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Up/Down", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" 移动", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" 确认", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" 取消", Style::default().fg(palette::TEXT_MUTED)),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true })
            .block(modal_block(&header));

        let popup_area = centered_rect(82, 68, area);
        render_modal_chrome(area, popup_area, buf);
        paragraph.render(popup_area, buf);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1]);
    horizontal[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::user_input::{UserInputOption, UserInputQuestion, UserInputRequest};

    fn render_view(view: &UserInputView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample_view() -> UserInputView {
        UserInputView::new(
            "tool-1",
            UserInputRequest {
                questions: vec![UserInputQuestion {
                    header: "确认".to_string(),
                    id: "confirm".to_string(),
                    question: "接下来应该做什么？".to_string(),
                    options: vec![
                        UserInputOption {
                            label: "提交".to_string(),
                            description: "继续当前更改集".to_string(),
                        },
                        UserInputOption {
                            label: "修改".to_string(),
                            description: "在继续前返回编辑".to_string(),
                        },
                    ],
                    allow_free_text: true,
                    multi_select: false,
                }],
            },
        )
    }

    #[test]
    fn user_input_modal_calls_out_required_action_and_controls() {
        let rendered = render_view(&sample_view(), 110, 36);

        assert!(rendered.contains("需要操作"));
        assert!(rendered.contains("问题 1/1"));
        assert!(rendered.contains("快速选择"));
        // allow_free_text=true 会显示"其他"行。
        assert!(rendered.contains("其他"));
    }

    #[test]
    fn user_input_modal_renders_custom_response_state() {
        let mut view = sample_view();
        view.selected = 2;
        view.mode = InputMode::OtherInput;
        view.other_input = "需要再次检查".to_string();

        let rendered = render_view(&view, 110, 36);

        assert!(rendered.contains("自定义回复"));
        assert!(rendered.contains("需要再次检查"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("提交"));
    }

    #[test]
    fn user_input_modal_hides_other_row_when_free_text_disabled() {
        // 问题 #3102：allow_free_text=false 绝不能渲染硬编码的
        // "Other" 伪选项。以前"Other"总是被追加。
        let mut view = sample_view();
        view.request.questions[0].allow_free_text = false;
        // 将选择重置为有效选项索引（没有"Other"行可定位）。
        view.selected = 0;

        let rendered = render_view(&view, 110, 36);
        assert!(
            !rendered.contains("输入自定义回复"),
            "当 allow_free_text 为 false 时，其他行应隐藏"
        );
        assert!(!rendered.contains("\n其他\n"));
    }

    #[test]
    fn user_input_modal_renders_multi_select_ticks_and_confirm() {
        // 问题 #3102：multi_select=true 在切换的选项上渲染复选标记槽，
        // 以及尾随的"确认选择"行，控制提示提示 Space/Enter 切换语义。
        let mut view = sample_view();
        view.request.questions[0].multi_select = true;
        view.request.questions[0].allow_free_text = false;
        // 将第一个选项切换到待处理集。
        view.multi_pending.push(0);
        // 高亮确认行（最后一个可选择行）。
        view.selected = view.option_count() - 1;

        let rendered = render_view(&view, 120, 40);
        assert!(rendered.contains("✔"), "切换的选项显示复选标记");
        assert!(
            rendered.contains("确认选择"),
            "多选渲染确认行"
        );
        assert!(rendered.contains("提交 1 个已选择"));
        assert!(rendered.contains("切换"));
    }
}
