//! 编辑器 vim 普通模式按键绑定。

use crate::tui::app::{App, VimMode};

/// 处理编辑器处于 vim 普通模式时的普通字符按键。
///
/// 实现核心的普通模式绑定集：
/// - `h` / `l`  — 按字符左/右移动
/// - `j` / `k`  — 按逻辑行下/上移动（回退到上/下一条历史）
/// - `w` / `b`  — 向前/向后移动一个单词
/// - `0` / `$`  — 行首/行尾
/// - `x`        — 删除光标下的字符
/// - `d`（×2）   — 删除当前行（`dd`）
/// - `i`        — 在光标前进入插入模式
/// - `a`        — 在光标后进入插入模式
/// - `o`        — 在下方新开一行并进入插入模式
/// - `v`        — 进入可视模式
/// - `G`        — 移动到缓冲区末尾
pub(super) fn handle_vim_normal_key(app: &mut App, c: char) {
    // 处理待定的 `d`（等待第二个 `d` 来完成 `dd`）。
    if app.composer.vim_pending_d {
        app.composer.vim_pending_d = false;
        if c == 'd' {
            app.vim_delete_line();
        }
        // 任何其他按键取消待定的操作符。
        return;
    }

    match c {
        'h' => app.move_cursor_left(),
        'l' => app.move_cursor_right(),
        'j' => app.vim_move_down(),
        'k' => app.vim_move_up(),
        'w' => app.vim_move_word_forward(),
        'b' => app.vim_move_word_backward(),
        '0' => app.vim_move_line_start(),
        '$' => app.vim_move_line_end(),
        'x' => app.vim_delete_char_under_cursor(),
        'd' => {
            // 开始 `dd` 操作符序列。
            app.composer.vim_pending_d = true;
        }
        'i' => app.vim_enter_insert(),
        'a' => app.vim_enter_append(),
        'o' => app.vim_open_line_below(),
        'v' => {
            app.composer.vim_mode = VimMode::Visual;
            app.needs_redraw = true;
        }
        'G' => app.move_cursor_end(),
        _ => {
            // 未知的普通模式按键 — 在普通模式下静默忽略。
        }
    }
}
