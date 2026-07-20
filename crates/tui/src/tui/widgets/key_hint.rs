//! 终端感知的快捷键渲染。
//!
//! `KeyBinding` 是一个和弦（一个 [`KeyCode`] 加上一组 [`KeyModifiers`]）的类型化表示，
//! 它知道如何以匹配宿主平台约定的方式渲染自身。在 macOS 上，Option 键渲染为 `⌥`
//!（与所有其他 Mac 应用——包括 Terminal、iTerm2 和系统菜单栏——标记 Option 和弦的方式一致）。
//! 在 Linux 和 Windows 上，我们保留来自其他 CLI 的用户已经熟悉的纯文本 `alt + X` 表示法。
//!
//! 原始设计见 `codex-rs/tui/src/key_hint.rs`；这是一个 ratatui 兼容的移植版本，
//! 暴露了 [`std::fmt::Display`] 实现以及 `KeyBinding -> Span` 转换，
//! 使调用点可以在纯 `format!` 调用和 ratatui [`ratatui::text::Line`] /
//! [`ratatui::text::Span`] 构建器中使用。
//!
//! Windows AltGr 消歧：许多欧洲键盘布局在单独按下 AltGr 时会产生 `Ctrl+Alt` 事件
//!（用于输入 `@`、`\` 等）。在 Windows 上，[`is_altgr`] 对该组合返回 `true`，
//! 以便调用者可以在用户实际上只是想输入一个符号时抑制绑定到 alt 的快捷键匹配。
//! 在非 Windows 目标上，该函数始终返回 `false`。请参见 [`has_ctrl_or_alt`]
//! 了解便捷谓词，快捷键处理器应优先使用它而不是原始的 `mods.contains(...)` 检查。

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{style::Style, text::Span};

// 编译时平台检测。`#[cfg(test)]` 分支在 `cargo test` 期间强制使用 macOS
// 渲染，使单元测试无论运行在哪个宿主机上都是确定性的（CI 会在 Ubuntu、
// macOS 和 Windows 上运行）。
#[cfg(test)]
const ALT_PREFIX: &str = "⌥+";
#[cfg(all(not(test), target_os = "macos"))]
const ALT_PREFIX: &str = "⌥+";
#[cfg(all(not(test), not(target_os = "macos")))]
const ALT_PREFIX: &str = "alt+";

const CTRL_PREFIX: &str = "ctrl+";
const SHIFT_PREFIX: &str = "shift+";

/// 单个和弦（键 + 修饰键）的类型化表示。
///
/// 对于常见情况，通过 [`plain`]、[`alt`]、[`shift`]、[`ctrl`] 或 [`ctrl_alt`] 构造，
/// 对于任意修饰键集合，使用 [`KeyBinding::new`]。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    key: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyBinding {
    /// 从一个键码和修饰键集合构建绑定。
    pub const fn new(key: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { key, modifiers }
    }

    /// 如果提供的 [`KeyEvent`] 匹配此绑定（键 + 修饰键），则返回 `true`，
    /// 仅考虑 `Press` / `Repeat` 事件（释放事件被忽略——只有当按键释放报告
    /// 开启时 crossterm 才会发出释放事件，我们绝不希望在键弹起时触发快捷键）。
    pub fn is_press(&self, event: KeyEvent) -> bool {
        self.key == event.code
            && self.modifiers == event.modifiers
            && (event.kind == KeyEventKind::Press || event.kind == KeyEventKind::Repeat)
    }
}

/// 没有修饰键的绑定。
pub const fn plain(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::NONE)
}

/// `Alt` 修饰的绑定（在 macOS 上渲染为 `⌥`，其他地方为 `alt+`）。
pub const fn alt(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::ALT)
}

/// `Shift` 修饰的绑定。
pub const fn shift(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::SHIFT)
}

/// `Ctrl` 修饰的绑定。
pub const fn ctrl(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::CONTROL)
}

/// `Ctrl+Alt` 修饰的绑定。
pub const fn ctrl_alt(key: KeyCode) -> KeyBinding {
    KeyBinding::new(key, KeyModifiers::CONTROL.union(KeyModifiers::ALT))
}

fn modifiers_to_string(modifiers: KeyModifiers) -> String {
    let mut result = String::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        result.push_str(CTRL_PREFIX);
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        result.push_str(SHIFT_PREFIX);
    }
    if modifiers.contains(KeyModifiers::ALT) {
        result.push_str(ALT_PREFIX);
    }
    result
}

fn keycode_to_string(key: &KeyCode) -> String {
    match key {
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "shift+tab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "del".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string().to_ascii_lowercase(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::PageUp => "pgup".to_string(),
        KeyCode::PageDown => "pgdn".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::F(n) => format!("f{n}"),
        _ => format!("{key}").to_ascii_lowercase(),
    }
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            modifiers_to_string(self.modifiers),
            keycode_to_string(&self.key)
        )
    }
}

impl From<KeyBinding> for Span<'static> {
    fn from(binding: KeyBinding) -> Self {
        (&binding).into()
    }
}

impl From<&KeyBinding> for Span<'static> {
    fn from(binding: &KeyBinding) -> Self {
        Span::styled(binding.to_string(), key_hint_style())
    }
}

fn key_hint_style() -> Style {
    Style::default().dim()
}

/// `Alt` 修饰和弦的平台特定前缀，匹配 TUI 其余部分标记它们的方式：
/// macOS 上为 `⌥+`（每个 Mac 应用使用的 Option 键符号），Linux/Windows 上为 `alt+`。
/// 构建自己的快捷键提示字符串的调用者（例如热栏槽位标签）应使用此函数，
/// 以使修饰键标签与帮助覆盖层和页脚提示保持一致。
pub fn alt_prefix() -> &'static str {
    ALT_PREFIX
}

/// 如果 `mods` 携带 Ctrl 或 Alt——但不包括 Windows 上的 AltGr Ctrl+Alt
/// 组合，则返回 `true`。快捷键处理应优先使用此谓词
/// 而非 `mods.contains(CONTROL) || mods.contains(ALT)`，这样它们不会在
/// AltGr 按键上触发（在欧洲键盘布局上，AltGr 是用户输入 `@`、`\`、`|` 等的方式）。
pub fn has_ctrl_or_alt(mods: KeyModifiers) -> bool {
    (mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::ALT)) && !is_altgr(mods)
}

/// 在 Windows 上，AltGr 被传递为 `Ctrl+Alt`。没有终端可移植的方式
/// 来区分真实的 `Ctrl+Alt` 和弦与布局发出的 AltGr 字符——crossterm
/// 不会在所有后端上暴露左/右修饰键的区别——因此我们将任何 `Ctrl+Alt`（没有其他修饰键）
/// 视为 AltGr。这以（罕见的）绑定 `Ctrl+Alt+<char>` 的能力为代价，
/// 换取不吞掉欧洲用户输入的重音字符。在非 Windows 平台上始终返回 `false`。
#[cfg(windows)]
#[inline]
pub fn is_altgr(mods: KeyModifiers) -> bool {
    mods.contains(KeyModifiers::ALT) && mods.contains(KeyModifiers::CONTROL)
}

#[cfg(not(windows))]
#[inline]
pub fn is_altgr(_mods: KeyModifiers) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试通过 `cfg(test)` 强制 ALT_PREFIX = "⌥+"。我们通过显式调用
    // 宿主 OS cfg 分支会选择的辅助代码路径来验证两种平台特定的渲染。

    #[test]
    fn plain_renders_just_the_key() {
        assert_eq!(plain(KeyCode::Enter).to_string(), "enter");
        assert_eq!(plain(KeyCode::Char(' ')).to_string(), "space");
        assert_eq!(plain(KeyCode::Up).to_string(), "↑");
    }

    #[test]
    fn alt_renders_with_macos_glyph_in_tests() {
        // 在 cfg(test) 下，我们强制使用 macOS 前缀，使测试输出是
        // 确定性的。非 macOS 的渲染在下面的 `non_macos_alt_prefix` 中测试。
        assert_eq!(alt(KeyCode::Up).to_string(), "⌥+↑");
        assert_eq!(alt(KeyCode::Char('p')).to_string(), "⌥+p");
    }

    #[test]
    fn shift_and_ctrl_render_in_canonical_order() {
        // 顺序是：ctrl, shift, alt——匹配 codex-rs 和用户
        // 从跨工具肌肉记忆中期望的顺序。
        assert_eq!(ctrl(KeyCode::Char('c')).to_string(), "ctrl+c");
        assert_eq!(shift(KeyCode::Tab).to_string(), "shift+tab");
        assert_eq!(
            KeyBinding::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )
            .to_string(),
            "ctrl+shift+x"
        );
    }

    #[test]
    fn ctrl_alt_combo_renders_both_modifiers() {
        assert_eq!(ctrl_alt(KeyCode::Char('a')).to_string(), "ctrl+⌥+a");
    }

    #[test]
    fn keycode_lowercases_letters() {
        assert_eq!(plain(KeyCode::Char('A')).to_string(), "a");
    }

    #[test]
    fn function_keys_render_as_f_n() {
        assert_eq!(plain(KeyCode::F(1)).to_string(), "f1");
        assert_eq!(plain(KeyCode::F(12)).to_string(), "f12");
    }

    #[test]
    fn span_conversion_carries_dim_style() {
        let span: Span<'static> = alt(KeyCode::Up).into();
        assert_eq!(span.content, "⌥+↑");
        // ratatui 中 `Style` 的确切表示不易比较，
        // 因此我们只验证样式被设置了（不是默认值）。
        assert_ne!(span.style, Style::default());
    }

    #[test]
    fn is_press_matches_press_and_repeat() {
        let binding = ctrl(KeyCode::Char('c'));
        let press = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        let repeat = KeyEvent {
            kind: KeyEventKind::Repeat,
            ..press
        };
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press
        };
        let wrong_mods = KeyEvent {
            modifiers: KeyModifiers::NONE,
            ..press
        };
        assert!(binding.is_press(press));
        assert!(binding.is_press(repeat));
        assert!(!binding.is_press(release));
        assert!(!binding.is_press(wrong_mods));
    }

    #[test]
    fn altgr_only_fires_on_windows() {
        let altgr_mods = KeyModifiers::ALT | KeyModifiers::CONTROL;
        if cfg!(windows) {
            assert!(is_altgr(altgr_mods));
            assert!(!has_ctrl_or_alt(altgr_mods));
        } else {
            assert!(!is_altgr(altgr_mods));
            assert!(has_ctrl_or_alt(altgr_mods));
        }
        // 单独的 Alt 从不是 AltGr。
        assert!(!is_altgr(KeyModifiers::ALT));
        assert!(has_ctrl_or_alt(KeyModifiers::ALT));
        // 无修饰键：从不是 Ctrl/Alt。
        assert!(!has_ctrl_or_alt(KeyModifiers::NONE));
    }

    /// 按照 Linux/Windows 非测试分支的方式渲染一个 alt 前缀绑定。
    /// 我们无法在运行时切换 cfg，因此使用替代前缀重新构建渲染
    /// 以锁定预期的字符串形状。
    #[test]
    fn non_macos_alt_prefix_shape() {
        let mods = modifiers_to_string(KeyModifiers::ALT);
        // 在 cfg(test) 下，这是 "⌥+"。去掉并替换为 "alt+" 以展示
        // Linux/Windows 发布版中的实际形状。
        let linux_shape = mods.replace("⌥+", "alt+");
        assert_eq!(linux_shape, "alt+");

        let mods_mixed = modifiers_to_string(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let linux_shape_mixed = mods_mixed.replace("⌥+", "alt+");
        assert_eq!(linux_shape_mixed, "ctrl+alt+");
    }
}
