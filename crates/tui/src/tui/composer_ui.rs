use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::App;

const COMPOSER_ARROW_SCROLL_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscapeAction {
    CloseSlashMenu,
    CancelRequest,
    PauseCommand,
    DiscardQueuedDraft,
    ClearInput,
    Noop,
}

pub(crate) fn next_escape_action(app: &App, slash_menu_open: bool) -> EscapeAction {
    if slash_menu_open {
        EscapeAction::CloseSlashMenu
    } else if app.queued_draft.is_some() {
        EscapeAction::DiscardQueuedDraft
    } else if app.paused || app.paused_quarry.is_some() {
        EscapeAction::CancelRequest
    } else if app.pausable
        && !app.paused
        && (app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")))
    {
        EscapeAction::PauseCommand
    } else if app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
        EscapeAction::CancelRequest
    } else if !app.input.is_empty() {
        EscapeAction::ClearInput
    } else {
        EscapeAction::Noop
    }
}

pub(crate) fn select_previous_slash_menu_entry(app: &mut App, entry_count: usize) {
    if entry_count == 0 {
        return;
    }
    let selected = app.slash_menu_selected.min(entry_count.saturating_sub(1));
    app.slash_menu_selected = (selected + entry_count - 1) % entry_count;
}

pub(crate) fn select_next_slash_menu_entry(app: &mut App, entry_count: usize) {
    if entry_count == 0 {
        return;
    }
    let selected = app.slash_menu_selected.min(entry_count.saturating_sub(1));
    app.slash_menu_selected = (selected + 1) % entry_count;
}

pub(crate) fn handle_composer_history_arrow(
    app: &mut App,
    key: KeyEvent,
    slash_menu_open: bool,
    mention_menu_open: bool,
) -> bool {
    if slash_menu_open || mention_menu_open {
        return false;
    }
    if key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SUPER) {
        return false;
    }

    // 当启用 `composer_arrows_scroll` 时，纯上/下键滚动单行草稿的转录。
    // 多行草稿保持类似编辑器的行导航。如果用户在首行/末行按住上/下键，
    // 不要用提示历史替换他们的当前草稿，除非他们已经在导航历史。
    let scroll_transcript = app.composer_arrows_scroll && !app.input.contains('\n');
    let protect_multiline_draft = app.input.contains('\n') && app.history_index.is_none();

    match key.code {
        KeyCode::Up => {
            if scroll_transcript {
                app.scroll_up(COMPOSER_ARROW_SCROLL_LINES);
            } else if protect_multiline_draft && !cursor_has_previous_logical_line(app) {
                app.needs_redraw = true;
            } else {
                app.vim_move_up();
            }
            true
        }
        KeyCode::Down => {
            if scroll_transcript {
                app.scroll_down(COMPOSER_ARROW_SCROLL_LINES);
            } else if protect_multiline_draft && !cursor_has_next_logical_line(app) {
                app.needs_redraw = true;
            } else {
                app.vim_move_down();
            }
            true
        }
        _ => false,
    }
}

fn cursor_has_previous_logical_line(app: &App) -> bool {
    let cursor_byte = byte_index_at_char(&app.input, app.cursor_position);
    app.input[..cursor_byte].contains('\n')
}

fn cursor_has_next_logical_line(app: &App) -> bool {
    let cursor_byte = byte_index_at_char(&app.input, app.cursor_position);
    app.input[cursor_byte..].contains('\n')
}

fn byte_index_at_char(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

pub(crate) fn is_word_cursor_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT)
}

/// On macOS, map `SUPER` (Cmd ⌘) to `CONTROL` when `CONTROL` is not already
/// set, so that terminal emulators that don't pass Ctrl faithfully still work.
/// On all other platforms this is a no-op.
#[cfg(target_os = "macos")]
pub(crate) fn normalize_macos_modifiers(modifiers: KeyModifiers) -> KeyModifiers {
    // 移除 SUPER 并添加 CONTROL，以便在规范化后精确的修饰符相等性检查
    // （例如 Ctrl+S 存储中的 `modifiers == KeyModifiers::CONTROL`）能正确工作。
    if modifiers.contains(KeyModifiers::SUPER) {
        (modifiers - KeyModifiers::SUPER) | KeyModifiers::CONTROL
    } else {
        modifiers
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn normalize_macos_modifiers(modifiers: KeyModifiers) -> KeyModifiers {
    modifiers
}

pub(crate) fn handle_composer_alt_word_motion_key(app: &mut App, key: KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }

    match key.code {
        KeyCode::Char('f') | KeyCode::Char('F') => {
            app.clear_selection();
            app.move_cursor_word_forward();
            true
        }
        KeyCode::Char('b') | KeyCode::Char('B') => {
            app.clear_selection();
            app.move_cursor_word_backward();
            true
        }
        _ => false,
    }
}

pub(crate) fn is_composer_newline_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('j') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Enter => {
            key.modifiers.contains(KeyModifiers::ALT)
                || (key.modifiers.contains(KeyModifiers::SHIFT)
                    && !key.modifiers.contains(KeyModifiers::CONTROL))
        }
        _ => false,
    }
}

pub(crate) fn is_forced_submit_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Enter => key.modifiers.contains(KeyModifiers::CONTROL),
        // 多个终端将 Ctrl+Enter / Cmd+Enter 编码为 Ctrl+J。空闲时保持
        // Ctrl+J 可用作换行，但让事件循环在轮次已经在运行时使用此辅助函数
        // 强制进行实时干预。
        KeyCode::Char('j') | KeyCode::Char('J') => {
            key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
        }
        _ => false,
    }
}

pub(crate) fn handle_history_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let _ = app.accept_history_search();
        }
        KeyCode::Esc => {
            app.cancel_history_search();
        }
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.cancel_history_search();
        }
        KeyCode::Backspace => {
            app.history_search_backspace();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            while app
                .history_search_query()
                .is_some_and(|query| !query.is_empty())
            {
                app.history_search_backspace();
            }
        }
        KeyCode::Up => {
            app.history_search_select_previous();
        }
        KeyCode::Down => {
            app.history_search_select_next();
        }
        KeyCode::Char(ch)
            if key.modifiers.is_empty()
                || key.modifiers == KeyModifiers::SHIFT
                || key.modifiers == KeyModifiers::NONE =>
        {
            app.history_search_insert_char(ch);
        }
        _ => {}
    }
}
