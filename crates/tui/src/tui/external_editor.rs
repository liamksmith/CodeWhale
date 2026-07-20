//! 编辑器的外部编辑器支持。
//!
//! 在预填充了编辑器当前内容的临时文件上生成 `$VISUAL`/`$EDITOR`（回退 `vi`）。
//! 在编辑期间 TUI 被挂起，并在返回时重新进入。
//! 临时文件在所有路径（成功、编辑器失败、IO 错误）上通过
//! [`tempfile::NamedTempFile`] 清理。
//!
//! 参考：codex-rs 的 `tui/src/external_editor.rs` —— 此处的设计镜像了
//! 该方法，但是同步的（从 TUI 事件循环内联调用），并
//! 处理自己的原始模式切换，而不是依赖调用者。

use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::process::Command;

use crossterm::{
    event::DisableFocusChange,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use tempfile::Builder;

use super::color_compat::ColorCompatBackend;

/// 单次外部编辑器调用的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    /// 编辑器干净退出且文件内容与种子不同。
    Edited(String),
    /// 编辑器干净退出但内容不变（或修剪后为空）。
    /// 编辑器应保持原样。
    Unchanged,
    /// 编辑器非零退出或无法生成。
    /// 编辑器应保持原样并显示状态提示。
    Cancelled,
}

/// 解析编辑器命令，优先使用 `$VISUAL` 而不是 `$EDITOR`，回退到 `vi`。
/// 返回测试路径的原始字符串；`spawn_editor` 通过 `shlex`（Unix）分割它，
/// 以便用户可以设置 `EDITOR="code --wait"`。
fn resolve_editor() -> String {
    env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| env::var("EDITOR").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_string())
}

#[cfg(unix)]
fn split_command(raw: &str) -> Option<Vec<String>> {
    shlex::split(raw)
}

#[cfg(not(unix))]
fn split_command(raw: &str) -> Option<Vec<String>> {
    // 在 Windows 上我们不支持 shell 引用的编辑器命令；将
    // 完整字符串视为程序名称。
    if raw.trim().is_empty() {
        None
    } else {
        Some(vec![raw.to_string()])
    }
}

/// 在不触碰终端状态的情况下运行外部编辑器。为测试公开。
///
/// 返回：
/// - `Ok(EditorOutcome::Edited(new))` 如果编辑器干净退出且内容与 `seed` 不同。
/// - `Ok(EditorOutcome::Unchanged)` 如果编辑器干净退出但内容匹配 `seed`。
/// - `Ok(EditorOutcome::Cancelled)` 如果编辑器非零退出或无法生成。
///
/// 临时文件在所有路径上都被移除，因为 [`tempfile::NamedTempFile`]
/// 在函数结束时被丢弃。
pub fn run_editor_raw(seed: &str) -> io::Result<EditorOutcome> {
    let mut tmp = Builder::new()
        .prefix("deepseek-edit-")
        .suffix(".md")
        .tempfile()?;
    tmp.write_all(seed.as_bytes())?;
    tmp.flush()?;
    let path = tmp.path().to_path_buf();

    let raw = resolve_editor();
    let parts = match split_command(&raw) {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(EditorOutcome::Cancelled),
    };

    let mut cmd = Command::new(&parts[0]);
    if parts.len() > 1 {
        cmd.args(&parts[1..]);
    }
    cmd.arg(&path);

    let status = match cmd.status() {
        Ok(s) => s,
        Err(_) => return Ok(EditorOutcome::Cancelled),
    };
    if !status.success() {
        return Ok(EditorOutcome::Cancelled);
    }

    let new = fs::read_to_string(&path)?;
    // tmp 在此处超出作用域 —— 文件被取消链接。
    if new == seed {
        Ok(EditorOutcome::Unchanged)
    } else {
        Ok(EditorOutcome::Edited(new))
    }
}

/// 挂起 TUI，在 `current` 上运行外部编辑器，然后重新进入 TUI。
/// 当用户保存更改时返回新的编辑器文本。
///
/// 在任何错误（原始模式切换、IO、编辑器生成失败）上，
/// 函数在返回前仍然尝试完全恢复终端。
pub(crate) fn spawn_editor_for_input(
    terminal: &mut Terminal<ColorCompatBackend<Stdout>>,
    use_alt_screen: bool,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
    current: &str,
) -> io::Result<EditorOutcome> {
    // 1. 挂起。
    // #443：首先弹出键盘增强标志，以便编辑器
    // 进程不会继承半配置的输入模式。
    // 尽力而为 —— 匹配 main.rs 中的关闭/panic 路径。
    // 使用 Windows 感知辅助方法：原始的 crossterm execute!() 在
    // Windows 上是无操作的，会使编辑器进程处于 Kitty 模式。
    suspend_tui_child_modes(
        terminal.backend_mut(),
        use_mouse_capture,
        use_bracketed_paste,
    );
    let _ = disable_raw_mode();
    if use_alt_screen {
        let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    }

    // 2. 运行编辑器（同步；继承 stdio）。
    let result = run_editor_raw(current);

    // 3. 恢复 —— 无论 `result` 如何，尽力恢复。
    let _ = enable_raw_mode();
    if use_alt_screen {
        let _ = execute!(terminal.backend_mut(), EnterAlternateScreen);
    }
    super::ui::recover_terminal_modes(
        terminal.backend_mut(),
        use_mouse_capture,
        use_bracketed_paste,
    );
    // 强制完全重绘，以便编辑期间的 SIGWINCH 不会留下
    // 过时的视口。
    let _ = terminal.clear();

    result
}

fn suspend_tui_child_modes<W: Write>(
    writer: &mut W,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
) {
    super::ui::pop_keyboard_enhancement_flags(writer);
    super::ui::disable_alternate_scroll_mode(writer);
    let _ = execute!(writer, DisableFocusChange);
    if use_mouse_capture {
        disable_mouse_capture_for_child(writer);
    }
    if use_bracketed_paste {
        super::ui::disable_bracketed_paste_mode(writer);
    }
    let _ = writer.flush();
}

fn disable_mouse_capture_for_child<W: Write>(writer: &mut W) {
    // Crossterm 的鼠标捕获命令在 Windows 上采用 WinAPI 路径，
    // 并且不会向 mintty 等 PTY 风格终端发送字节。外部
    // 编辑器继承 PTY 状态，因此直接在此处发送 xterm 重置序列。
    const DISABLE_MOUSE_CAPTURE: &[u8] = b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
    if let Err(err) = writer.write_all(DISABLE_MOUSE_CAPTURE) {
        tracing::debug!(?err, "DisableMouseCapture direct reset ignored");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// 序列化改变进程全局环境变量的测试。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        keys: Vec<(&'static str, Option<OsString>)>,
    }
    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let saved: Vec<_> = keys.iter().map(|k| (*k, env::var_os(k))).collect();
            Self { keys: saved }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.keys {
                match v {
                    Some(val) => unsafe { env::set_var(k, val) },
                    None => unsafe { env::remove_var(k) },
                }
            }
        }
    }

    #[test]
    fn resolve_editor_prefers_visual_over_editor() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        unsafe {
            env::set_var("VISUAL", "vis-cmd");
            env::set_var("EDITOR", "ed-cmd");
        }
        assert_eq!(resolve_editor(), "vis-cmd");
    }

    #[test]
    fn resolve_editor_falls_back_to_vi() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        unsafe {
            env::remove_var("VISUAL");
            env::remove_var("EDITOR");
        }
        assert_eq!(resolve_editor(), "vi");
    }

    /// 立即退出 0 而不触碰文件的编辑器 ⇒ Unchanged。
    #[test]
    #[cfg(unix)]
    fn run_editor_unchanged_when_editor_is_noop() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        unsafe {
            env::remove_var("VISUAL");
            env::set_var("EDITOR", "true");
        }
        let out = run_editor_raw("seed text").expect("editor ok");
        assert_eq!(out, EditorOutcome::Unchanged);
    }

    /// 非零退出的编辑器 ⇒ Cancelled。
    #[test]
    #[cfg(unix)]
    fn run_editor_cancelled_on_nonzero_exit() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        unsafe {
            env::remove_var("VISUAL");
            env::set_var("EDITOR", "false");
        }
        let out = run_editor_raw("seed").expect("call ok");
        assert_eq!(out, EditorOutcome::Cancelled);
    }

    /// 生成不存在的编辑器二进制文件 ⇒ Cancelled（优雅处理）。
    #[test]
    #[cfg(unix)]
    fn run_editor_cancelled_when_editor_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        unsafe {
            env::remove_var("VISUAL");
            env::set_var("EDITOR", "/nonexistent/codewhale-test-editor");
        }
        let out = run_editor_raw("seed").expect("call ok");
        assert_eq!(out, EditorOutcome::Cancelled);
    }

    /// 重写文件的编辑器 ⇒ Edited(new)。
    #[test]
    #[cfg(unix)]
    fn run_editor_returns_edited_contents() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ed.sh");
        fs::write(&script, "#!/bin/sh\nprintf 'edited body' > \"$1\"\n").unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        unsafe {
            env::remove_var("VISUAL");
            env::set_var("EDITOR", script.to_string_lossy().to_string());
        }
        let out = run_editor_raw("seed body").expect("editor ok");
        assert_eq!(out, EditorOutcome::Edited("edited body".to_string()));
    }

    /// 验证 `run_editor_raw` 返回后临时文件被取消链接，
    /// 无论结果如何。我们通过一个脚本测试成功路径，
    /// 该脚本在退出前将文件路径回显到侧信道。
    #[test]
    #[cfg(unix)]
    fn run_editor_cleans_up_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::new(&["VISUAL", "EDITOR"]);
        let dir = tempfile::tempdir().unwrap();
        let path_capture = dir.path().join("capture.txt");
        let script = dir.path().join("ed.sh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$1\" > \"{}\"\nprintf 'x' > \"$1\"\n",
                path_capture.display()
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        unsafe {
            env::remove_var("VISUAL");
            env::set_var("EDITOR", script.to_string_lossy().to_string());
        }
        let _ = run_editor_raw("seed").expect("editor ok");

        let captured = fs::read_to_string(&path_capture).expect("captured path");
        assert!(!captured.is_empty(), "editor should have received a path");
        assert!(
            !std::path::Path::new(&captured).exists(),
            "temp file {captured:?} should be cleaned up after run_editor_raw returns"
        );
    }

    #[test]
    fn suspend_tui_child_modes_disables_every_inherited_mode() {
        let mut out = Vec::new();

        suspend_tui_child_modes(&mut out, true, true);

        let seq = String::from_utf8_lossy(&out);
        assert!(
            seq.contains("\x1b[?1007l"),
            "external editor suspend must disable alternate-scroll mode: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1004l"),
            "external editor suspend must disable focus events: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?2004l"),
            "external editor suspend must disable bracketed paste: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1000l"),
            "external editor suspend must disable mouse capture when active: {seq:?}"
        );
    }

    #[test]
    fn suspend_tui_child_modes_leaves_mouse_capture_alone_when_inactive() {
        let mut out = Vec::new();

        suspend_tui_child_modes(&mut out, false, true);

        let seq = String::from_utf8_lossy(&out);
        assert!(
            !seq.contains("\x1b[?1000l"),
            "external editor suspend must not emit mouse-capture reset when inactive: {seq:?}"
        );
    }

    #[test]
    fn resume_tui_child_modes_reenables_shared_terminal_modes() {
        let mut out = Vec::new();

        crate::tui::ui::recover_terminal_modes(&mut out, true, true);

        let seq = String::from_utf8_lossy(&out);
        assert!(
            seq.contains("\x1b[?1007h"),
            "external editor resume must restore alternate-scroll mode: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1004h"),
            "external editor resume must restore focus events: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?2004h"),
            "external editor resume must restore bracketed paste: {seq:?}"
        );
    }

    #[test]
    fn resume_tui_child_modes_leaves_alternate_scroll_off_when_mouse_capture_inactive() {
        let mut out = Vec::new();

        crate::tui::ui::recover_terminal_modes(&mut out, false, true);

        let seq = String::from_utf8_lossy(&out);
        assert!(
            !seq.contains("\x1b[?1007h"),
            "external editor resume must not enable alternate-scroll without mouse capture: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1007l"),
            "external editor resume must reset alternate-scroll without mouse capture: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?1004h"),
            "external editor resume must still restore focus events: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[?2004h"),
            "external editor resume must still restore bracketed paste: {seq:?}"
        );
    }
}
