//! 粘贴突发处理——将快速击键（没有括号粘贴的终端）转换为单个提交的
//! 缓冲区，而不是 N 个单独的字符。
//!
//! 从 `tui/ui.rs`（P1.2）提取。所属状态机位于
//! `App.paste_burst`（`tui::paste_burst`）；这些辅助函数将其连接到
//! 按键事件循环和编辑器的文本缓冲区。

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, looks_like_slash_command_input};
use super::paste_burst::CharDecision;

/// 在粘贴突发检测的上下文中处理按键。当按键被粘贴机制完全处理时
/// 返回 `true`（调用者跳过进一步的输入处理）；当按键仍然需要
/// 正常的编辑器路径时返回 `false`。
pub fn handle_paste_burst_key(app: &mut App, key: &KeyEvent, now: Instant) -> bool {
    if !app.use_paste_burst_detection {
        return false;
    }
    // 一旦我们在本次会话中观察到了真实的 `Event::Paste`，括号粘贴
    // 已被验证工作正常，快速击键启发式方法就不再需要了。跳过它
    // 可以消除在具有可靠括号粘贴功能的终端上（iTerm2 / Ghostty /
    // WezTerm / Windows Terminal 上的主要情况）由快速打字/IME 提交/
    // 自动完成引起的误报。
    if app.bracketed_paste_seen {
        return false;
    }

    let has_ctrl_alt_or_super = key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::ALT)
        || key.modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        KeyCode::Enter => {
            if !in_command_context(app) && app.paste_burst.append_newline_if_active(now) {
                return true;
            }
            if !in_command_context(app)
                && app.paste_burst.newline_should_insert_instead_of_submit(now)
            {
                app.insert_char('\n');
                app.paste_burst.extend_window(now);
                return true;
            }
        }
        KeyCode::Char(c) if !has_ctrl_alt_or_super => {
            if !c.is_ascii() {
                // IME 提交的字符（中文、日文、韩文）以单独的
                // KeyCode::Char 事件到达，每个提交的字符之间
                // 通常有数十毫秒的间隔。当 IME 提交速度慢于
                // 突发启发式方法的定时窗口时，粘贴突发缓冲会丢失字符。
                //
                // 我们仍然调用 note_plain_char + extend_window，以便：
                //   1. 对于没有括号粘贴支持的终端上的非 IME 快速打字，
                //      突发定时计数器仍然推进。
                //   2. 在快速非 ASCII 序列期间，Enter 抑制窗口保持打开，
                //      防止过早提交。
                // 但字符直接插入到编辑器中，而不是放入粘贴突发缓冲区。
                if let Some(pending) = app.paste_burst.flush_before_modified_input() {
                    app.insert_str(&pending);
                }
                app.paste_burst.note_plain_char(now);
                app.paste_burst.extend_window(now);
                app.insert_char(c);
                return true;
            }

            let decision = app.paste_burst.on_plain_char(c, now);
            return handle_paste_burst_decision(app, decision, c, now);
        }
        _ => {}
    }

    false
}

/// 将粘贴突发决策应用到编辑器缓冲区。某些决策会将最后几个字符
/// 从输入中回溯抓取到待处理的粘贴缓冲区（当启发式方法确定
/// 最近的打字实际上是一次粘贴时）。
pub fn handle_paste_burst_decision(
    app: &mut App,
    decision: CharDecision,
    c: char,
    now: Instant,
) -> bool {
    match decision {
        CharDecision::RetainFirstChar => true,
        CharDecision::BeginBufferFromPending | CharDecision::BufferAppend => {
            app.paste_burst.append_char_to_buffer(c, now);
            true
        }
        CharDecision::BeginBuffer { retro_chars } => {
            if apply_paste_burst_retro_capture(app, retro_chars as usize, c, now) {
                return true;
            }
            app.insert_char(c);
            true
        }
    }
}

fn apply_paste_burst_retro_capture(
    app: &mut App,
    retro_chars: usize,
    c: char,
    now: Instant,
) -> bool {
    let cursor_byte = app.cursor_byte_index();
    let before = &app.composer.input[..cursor_byte];
    let Some(grab) = app
        .composer
        .paste_burst
        .decide_begin_buffer(now, before, retro_chars)
    else {
        return false;
    };
    if !grab.grabbed.is_empty() {
        app.input.replace_range(grab.start_byte..cursor_byte, "");
        let removed = grab.grabbed.chars().count();
        app.cursor_position = app.cursor_position.saturating_sub(removed);
    }
    app.paste_burst.append_char_to_buffer(c, now);
    true
}

fn in_command_context(app: &App) -> bool {
    looks_like_slash_command_input(&app.input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.use_paste_burst_detection = true;
        app
    }

    fn plain(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    #[test]
    fn raw_short_cjk_multiline_paste_buffers_enter_instead_of_submitting() {
        // #1302：粘贴短 CJK 内容如"请联网搜索：\nSTM32 …"过去会
        // 静默提交第一行，因为启发式方法认为它不像粘贴（无空格且
        // 少于 16 个字符）。非 ASCII 绕过现在将其分类为粘贴，因此
        // Enter 被吸收到突发缓冲区中。
        let mut app = test_app();
        let t0 = Instant::now();

        let pasted = "请联网搜索：\nSTM32 商业应用案例";
        for (i, ch) in pasted.chars().enumerate() {
            let key = if ch == '\n' {
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            } else {
                plain(ch)
            };
            let handled =
                handle_paste_burst_key(&mut app, &key, t0 + Duration::from_millis(i as u64));
            assert!(
                handled,
                "原始粘贴字符 {ch:?} 必须由粘贴突发检测处理"
            );
        }

        // 非 ASCII 字符现在直接插入到编辑器中，而不是由粘贴突发缓冲。
        // Enter 抑制窗口阻止了换行符过早提交。
        assert_eq!(app.input, pasted);
    }

    #[test]
    fn raw_multiline_paste_buffers_enter_instead_of_submitting() {
        let mut app = test_app();
        let t0 = Instant::now();

        assert!(handle_paste_burst_key(&mut app, &plain('a'), t0));
        assert!(handle_paste_burst_key(
            &mut app,
            &plain('b'),
            t0 + Duration::from_millis(1)
        ));
        assert!(handle_paste_burst_key(
            &mut app,
            &plain('c'),
            t0 + Duration::from_millis(2)
        ));
        assert!(handle_paste_burst_key(
            &mut app,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            t0 + Duration::from_millis(3)
        ));

        assert!(app.input.is_empty(), "粘贴保持缓冲直到空闲");
        assert!(app.flush_paste_burst_if_due(
            t0 + Duration::from_millis(3)
                + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay()
        ));
        assert_eq!(app.input, "abc\n");
    }

    #[test]
    fn paste_buffered_question_mark_does_not_fall_through_to_help_shortcut() {
        let mut app = test_app();
        let t0 = Instant::now();

        assert!(handle_paste_burst_key(&mut app, &plain('?'), t0));

        assert!(app.input.is_empty(), "快捷键字符先保持缓冲");
        assert!(app.view_stack.is_empty(), "帮助弹窗不得打开");
        assert!(app.flush_paste_burst_if_due(
            t0 + crate::tui::paste_burst::PasteBurst::recommended_flush_delay()
        ));
        assert_eq!(app.input, "?");
    }

    /// 固定 IME 输入契约：macOS/Windows 输入法在候选项弹窗关闭后
    /// 将每个中文字符提交为单个 `KeyCode::Char(c)` 事件。每个码点
    /// 适合一个 `char`（BMP 字符无需担心代理对），因此简单的纯字符
    /// 事件序列必须逐字到达 `app.input`——无 ASCII 过滤、无字节与
    /// 字符索引漂移、无无限缓冲字符的粘贴突发误报。
    #[test]
    fn ime_chinese_chars_route_through_to_composer() {
        let mut app = test_app();
        let t0 = Instant::now();

        // 逐个事件输入四个中文字符"你好世界"，每个事件之间有
        // 大约 50ms 的间隔，使粘贴突发启发式方法不会将其分类
        // 为粘贴突发。
        for (i, ch) in "你好世界".chars().enumerate() {
            let now = t0 + Duration::from_millis(50 * i as u64);
            let _ = handle_paste_burst_key(&mut app, &plain(ch), now);
        }

        // 超过活跃刷新延迟，使任何缓冲的突发提交。
        let after = t0
            + Duration::from_millis(50 * 4)
            + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay();
        let _ = app.flush_paste_burst_if_due(after);

        assert_eq!(
            app.input, "你好世界",
            "IME 输入的中文字符必须逐字到达编辑器"
        );
        assert_eq!(
            app.cursor_position, 4,
            "游标按每个码点前进，而不是按每个 UTF-8 字节"
        );
    }

    /// 固定 CJK 内容的括号粘贴契约：粘贴的中文文本（例如用户从
    /// 中文网站复制问题并粘贴到编辑器时）必须保留每个码点，并且
    /// 不在游标位置中重复计算多字节字符。
    #[test]
    fn bracketed_paste_preserves_chinese_and_mixed_text() {
        let mut app = test_app();
        app.insert_paste_text("你好世界 hello 世界 café");
        assert_eq!(app.input, "你好世界 hello 世界 café");
        // 4 + 1 + 5 + 1 + 2 + 1 + 4 = 18 个码点（将 é 计为一个）。
        assert_eq!(app.cursor_position, 18);
    }

    #[test]
    fn paste_burst_detection_can_be_disabled_without_disabling_bracketed_paste() {
        let mut app = test_app();
        app.use_paste_burst_detection = false;

        assert!(!handle_paste_burst_key(
            &mut app,
            &plain('a'),
            Instant::now()
        ));
        assert!(app.input.is_empty());

        app.insert_paste_text("line 1\r\nline 2");
        assert_eq!(app.input, "line 1\nline 2");
        assert!(app.use_bracketed_paste);
    }

    /// 一旦会话观察到了真实的 `Event::Paste`，快速击键启发式方法
    /// 必须短路。这固定了新的"验证括号粘贴后自动禁用粘贴突发"
    /// 行为，使功能完备的终端上的快速打字/IME 提交/自动完成不会
    /// 被误分类为粘贴突发。
    #[test]
    fn paste_burst_short_circuits_after_bracketed_paste_observed() {
        let mut app = test_app();
        app.use_paste_burst_detection = true;
        app.bracketed_paste_seen = true;

        let t0 = Instant::now();
        for (i, ch) in "abcdefgh".chars().enumerate() {
            // 打字速度足够快，通常会让粘贴突发触发。
            let now = t0 + Duration::from_millis(i as u64);
            assert!(
                !handle_paste_burst_key(&mut app, &plain(ch), now),
                "一旦括号粘贴被验证，粘贴突发不得消耗按键"
            );
        }
        // 没有缓冲——每个字符都落入了正常的编辑器路径
        //（当突发处理器返回 false 时，测试工具不会插入字符；
        // 我们在这里只断言短路契约）。
        assert!(app.input.is_empty());
    }
}
