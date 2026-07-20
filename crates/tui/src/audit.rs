//! 敏感操作的轻量级审计日志记录。

use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde_json::{Value, json};

use crate::utils::{flush_and_sync, open_append};

/// 将审计事件追加到 `~/.codewhale/audit.log`。
///
/// 此辅助函数设计为尽力而为：如果审计持久化失败，调用者不应使关键流程失败。
pub fn log_sensitive_event(event: &str, details: Value) {
    if let Err(err) = append_event(event, details) {
        crate::logging::warn(format!("audit log write failed: {err}"));
    }
}

fn append_event(event: &str, details: Value) -> anyhow::Result<()> {
    let path = default_audit_path()?;
    let parent = path.parent().map(|p| p.to_path_buf());
    if let Some(ref parent) = parent {
        fs::create_dir_all(parent)?;
    }
    // 使用 BufWriter 以追加模式打开，进行缓冲 I/O，然后在每个事件后刷新 + fsync，
    // 确保记录持久地写入磁盘。
    let mut writer = open_append(&path)?;
    let record = json!({
        "ts": Utc::now().to_rfc3339(),
        "event": event,
        "details": details,
    });
    let line = serde_json::to_string(&record)?;
    use std::io::Write;
    writeln!(writer, "{line}")?;
    flush_and_sync(&mut writer)?;
    Ok(())
}

fn default_audit_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home directory not found"))?;
    Ok(home.join(".codewhale").join("audit.log"))
}
