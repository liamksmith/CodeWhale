//! CLI 的轻量级详细日志辅助函数。

use std::sync::atomic::{AtomicBool, Ordering};

use colored::Colorize;

use crate::palette;
static VERBOSE: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static VERBOSE_SNAPSHOT: AtomicBool = AtomicBool::new(false);

/// 启用或禁用详细日志输出。
pub fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::SeqCst);
}

/// 捕获当前的详细状态，以便 TUI 在暂时抑制 Windows 备用屏幕输出后恢复它。
#[cfg(windows)]
pub fn snapshot_verbose_state() {
    VERBOSE_SNAPSHOT.store(is_verbose(), Ordering::SeqCst);
}

/// 恢复最后捕获的详细状态。
#[cfg(windows)]
pub fn restore_verbose_state() {
    set_verbose(VERBOSE_SNAPSHOT.load(Ordering::SeqCst));
}

/// 当 `DEEPSEEK_LOG_LEVEL` 请求详细输出时返回 true。
///
/// 注意：此处有意不检查 `RUST_LOG`——它控制 `runtime_log.rs`（文件日志）中的
/// `tracing` 订阅者过滤器，不应控制 CLI 详细输出。
/// 在 Windows 上，stderr 不会被重定向到日志文件，将两者耦合会导致
/// 追踪日志消息泄漏到 TUI 备用屏幕中。
#[must_use]
pub fn env_requests_verbose_logging() -> bool {
    std::env::var("DEEPSEEK_LOG_LEVEL")
        .ok()
        .is_some_and(|value| log_value_enables_verbose(&value))
}

fn log_value_enables_verbose(value: &str) -> bool {
    value.split(',').any(|directive| {
        let level = directive
            .rsplit('=')
            .next()
            .unwrap_or(directive)
            .trim()
            .to_ascii_lowercase();
        matches!(level.as_str(), "trace" | "debug" | "info")
    })
}

/// 检查详细日志是否已启用。
#[must_use]
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

/// 发出详细信息消息（当详细输出禁用时无操作）。
pub fn info(message: impl AsRef<str>) {
    if is_verbose() {
        let (r, g, b) = palette::WHALE_INFO_RGB;
        eprintln!("{} {}", "info".truecolor(r, g, b).bold(), message.as_ref());
    }
}

/// 发出详细警告消息（当详细输出禁用时无操作）。
pub fn warn(message: impl AsRef<str>) {
    if is_verbose() {
        let (r, g, b) = palette::WHALE_INFO_RGB;
        eprintln!("{} {}", "warn".truecolor(r, g, b).bold(), message.as_ref());
    }
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn log_value_parser_accepts_common_rust_log_directives() {
        assert!(log_value_enables_verbose("debug"));
        assert!(log_value_enables_verbose("codewhale_cli=debug"));
        assert!(log_value_enables_verbose(
            "warn,codewhale_tui::client=trace"
        ));
        assert!(!log_value_enables_verbose("warn"));
        assert!(!log_value_enables_verbose("codewhale_tui=off"));
    }

    #[test]
    fn snapshot_and_restore_verbose_state_round_trip() {
        let _guard = TEST_GUARD.lock().unwrap_or_else(|err| err.into_inner());

        set_verbose(false);
        snapshot_verbose_state();
        set_verbose(true);
        restore_verbose_state();
        assert!(!is_verbose());

        set_verbose(true);
        snapshot_verbose_state();
        set_verbose(false);
        restore_verbose_state();
        assert!(is_verbose());

        set_verbose(false);
    }

    #[test]
    fn restore_keeps_cli_verbose_state_even_when_env_is_not_verbose() {
        let _guard = TEST_GUARD.lock().unwrap_or_else(|err| err.into_inner());

        set_verbose(true);
        snapshot_verbose_state();

        // 模拟 Windows 备用屏幕抑制路径。恢复必须
        // 在不依赖环境的情况下带回抑制前的 CLI 状态。
        set_verbose(false);
        restore_verbose_state();

        assert!(is_verbose());
        set_verbose(false);
    }
}
