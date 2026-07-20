//! 键盘快捷键谓词和平台特定标签。
//!
//! 这些辅助函数规范化了 `Ctrl+…`（Linux/Windows）和 `Cmd+…`（macOS）之间的
//! 跨平台差异、历史遗留的 `Ctrl+H` 作为退格键的处理，
//! 以及 macOS Option-拉丁字符转义。
//! 将它们集中管理，可以使 `ui.rs` 中的 composer / transcript 事件循环保持简短，
//! 并且让我们在添加新平台时无需触及调用点。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn has_control_like_modifier(modifiers: KeyModifiers) -> bool {
    has_control_like_modifier_for_platform(modifiers, cfg!(target_os = "macos"))
}

pub(super) fn has_control_like_modifier_for_platform(
    modifiers: KeyModifiers,
    is_macos: bool,
) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        || (is_macos && modifiers.contains(KeyModifiers::SUPER))
}

/// 复制到剪贴板：macOS 上为 `Cmd+C`，其他平台为 `Ctrl+Shift+C`。
pub(super) fn is_copy_shortcut(key: &KeyEvent) -> bool {
    let is_c = matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'));
    if !is_c {
        return false;
    }

    if key.modifiers.contains(KeyModifiers::SUPER) {
        return true;
    }

    key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::SHIFT)
}

/// 切换文件树面板：Linux/Windows 上为 `Ctrl+Shift+E`，macOS 上为 `Cmd+Shift+E`。
pub(super) fn is_file_tree_toggle_shortcut(key: &KeyEvent) -> bool {
    let is_shifted_e = matches!(key.code, KeyCode::Char('E'))
        || (matches!(key.code, KeyCode::Char('e')) && key.modifiers.contains(KeyModifiers::SHIFT));
    if !is_shifted_e {
        return false;
    }

    let has_forbidden_modifier =
        key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SUPER);
    let ctrl_shift_e = key.modifiers.contains(KeyModifiers::CONTROL) && !has_forbidden_modifier;

    let cmd_shift_e = key.modifiers.contains(KeyModifiers::SUPER)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT);

    ctrl_shift_e || cmd_shift_e
}

pub(super) fn tool_details_shortcut_label() -> &'static str {
    "v"
}

pub(super) fn tool_details_shortcut_action_hint(noun: &str) -> String {
    format!("{} opens {noun}", tool_details_shortcut_label())
}

pub(super) fn activity_shortcut_label() -> &'static str {
    "Ctrl+O"
}

/// v0.8.30 系列 `Alt+<key>` 对话导航快捷键（`Alt+G` / `Alt+[` / `Alt+]` / `Alt+?` / `Alt+L`）
/// 的修饰键谓词。需要 `Alt` 并排除 `Ctrl` / `Super`，以便绑定
/// 不与平台剪贴板/窗口管理快捷键冲突。
/// 允许 `Shift`，以便大写字母形式在任何
/// 将其生成为 `Alt+Shift+key` 的键盘布局上都能正常工作。
///
/// 纯 `Char` 事件（无修饰键，或仅 `Shift` 修饰键配合大写形式）会
/// 落入文本插入，这完全是重点——输入"good morning"不再吃掉第一个 `g`。
pub(super) fn alt_nav_modifiers(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::ALT)
        && !modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::SUPER)
}

pub(super) fn is_macos_option_v_legacy_key(key: &KeyEvent) -> bool {
    is_macos_option_v_legacy_key_for_platform(key, cfg!(target_os = "macos"))
}

pub(super) fn is_macos_option_v_legacy_key_for_platform(key: &KeyEvent, is_macos: bool) -> bool {
    is_macos && key.modifiers.is_empty() && matches!(key.code, KeyCode::Char('\u{221A}'))
}

/// 从剪贴板粘贴：`Cmd+V`（macOS）、`Ctrl+V`（Linux/Windows）或
/// 某些终端发出的历史遗留原始 `\u{16}` ETX 字节。
pub(super) fn is_paste_shortcut(key: &KeyEvent) -> bool {
    let is_v = matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'));
    let is_legacy_ctrl_v = matches!(key.code, KeyCode::Char('\u{16}'));
    if !is_v && !is_legacy_ctrl_v {
        return false;
    }

    if is_legacy_ctrl_v {
        return true;
    }

    // macOS 上为 Cmd+V
    if key.modifiers.contains(KeyModifiers::SUPER) {
        return true;
    }

    // Linux/Windows 上为 Ctrl+V
    key.modifiers.contains(KeyModifiers::CONTROL)
}

/// 按键事件是否代表用户在 composer 中输入可打印字符
///（没有会将其变为快捷键的修饰键）。
pub(super) fn is_text_input_key(key: &KeyEvent) -> bool {
    if matches!(key.code, KeyCode::Char(c) if c.is_control()) {
        return false;
    }

    !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
}

/// `Ctrl+H` 是许多终端在用户按下退格键时仍然发出的历史遗留 ASCII 退格键。
/// 排除 Alt/Super 以免与窗口管理组合键冲突。
pub(super) fn is_ctrl_h_backspace(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('h'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
}
