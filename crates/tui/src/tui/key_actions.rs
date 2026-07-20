//! 从 `ui.rs` 提取的键盘事件操作处理函数。
//!
//! 每个函数处理键盘输入的一个集中子集，使主事件循环保持精简。

use crossterm::event::{KeyCode, KeyEvent};

use super::app::App;

// ── 文件树按键处理 ───────────────────────────────────────

/// 处理文件树面板可见时的键盘输入。
///
/// 当按键被消费时返回 `true`（调用者应 `continue`）。
pub fn handle_file_tree_key(app: &mut App, key: &KeyEvent) -> bool {
    // 守卫：当文件树面板不可见时不拦截按键。
    if !app.file_tree_visible {
        return false;
    }

    // 即使在条目仍在加载时，Esc 也关闭文件树。
    if key.code == KeyCode::Esc && app.file_tree.is_some() {
        app.file_tree = None;
        app.status_message = Some("File tree closed".to_string());
        app.needs_redraw = true;
        return true;
    }

    let Some(file_tree) = app.file_tree.as_mut() else {
        return false;
    };

    match key.code {
        KeyCode::Up => {
            file_tree.cursor_up();
            app.needs_redraw = true;
            true
        }
        KeyCode::Down => {
            file_tree.cursor_down();
            app.needs_redraw = true;
            true
        }
        KeyCode::Enter => {
            if let Some(rel_path) = file_tree.activate() {
                let path_str = rel_path.to_string_lossy().to_string();
                app.status_message = Some(format!("Attached @{path_str}"));
                app.insert_str(&format!("@{path_str} "));
            } else {
                app.needs_redraw = true;
            }
            true
        }
        _ => false,
    }
}
