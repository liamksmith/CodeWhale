//! 用户级记忆文件（已弃用——请参见 Moraine）。
//!
//! ## 弃用
//!
//! 已弃用 (v0.8.66–v0.8.71)：已被 Moraine MCP 召回取代。
//! 旧的推送/注入路径由 `MemoryConfig.moraine_fallback` 控制。
//! 当 Moraine 落地后（v0.8.66/67），此模块可完全删除。
//!
//! 迁移指南：使用 Moraine MCP 工具（`search_sessions`、`open`、
//! `list_sessions`、`file_attention`）代替 `<user_memory>` 注入。
//!
//! 参考：https://github.com/Hmbown/CodeWhale/issues/3495 (Moraine 采用)
//! 参考：https://github.com/Hmbown/CodeWhale/issues/3490 (v0.8.71 死代码盘点)
//!
//! ### 迁移步骤
//!
//! 1. 安装 Moraine：`uv tool install moraine-cli && moraine setup && moraine up`
//! 2. 在 `~/.codewhale/mcp.json` 中启用 `moraine-mcp`（将 `disabled` 设为 `false`）
//! 3. 在 `config.toml` 中设置 `[memory] moraine_fallback = true` 以跳过旧的
//!    `<user_memory>` 块、`remember` 工具和 `# foo` 快速添加。
//!
//! ## 旧版文档（Moraine 之前）
//!
//! v0.8.8 发布了一个 MVP，允许用户保留一个持久的个人
//! 笔记文件，模型在每轮对话中都会看到它：
//!
//! - **加载** `~/.codewhale/memory.md`（路径可通过
//!   `config.toml` 中的 `memory_path` 和 `DEEPSEEK_MEMORY_PATH` 环境变量配置），
//!   将其包装在 `<user_memory>` 块中，并将其预先添加到系统提示词中，
//!   与现有的 `<project_instructions>` 块并列。
//! - **`# foo`** 在 composer 中键入会将 `foo` 作为带时间戳的项目符号追加到记忆文件中
//!   ——无需离开 TUI 即可快速捕获。
//! - **`/memory`** 显示解析后的文件路径和当前内容，**`/memory edit`** 打印一个可复制粘贴的
//!   `$VISUAL` / `$EDITOR` 命令，用于自行打开文件。
//! - **`remember` 工具**允许模型在注意到值得跨会话保留的持久偏好或约定时自行追加项目符号。
//!
//! 默认行为是 **选择加入**：仅在 `config.toml` 中设置 `[memory] enabled = true`
//! 或设置 `DEEPSEEK_MEMORY=on` 时加载和使用记忆文件。
//! 这使现有用户保持零开销行为，并使功能明确。

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use chrono::Utc;

/// 用户记忆文件的最大大小。较大的文件仍会加载，但
/// `<user_memory>` 块会带有 `<truncated bytes=N source="...">` 标记，
/// 以便用户知道模型只看到了一部分。与 `project_context::MAX_CONTEXT_SIZE` 一致。
const MAX_MEMORY_SIZE: usize = 100 * 1024;

/// 读取 `path` 处的用户记忆文件，当文件不存在或修剪后为空时返回 `None`。
#[must_use]
pub fn load(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    Some(content)
}

/// 将记忆内容包装在 `<user_memory>` 块中，准备预先添加到系统提示词。
/// `source` 值原样渲染到 `source="…"` 属性中——传入路径以便模型知道记忆的来源。
/// 内容为空时返回 `None`。
#[must_use]
pub fn as_system_block(content: &str, source: &Path) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    let display = source.display().to_string();
    let payload = if content.len() > MAX_MEMORY_SIZE {
        let cutoff = truncation_cutoff(content, &display);
        let omitted_bytes = content.len() - cutoff;
        let mut head = content[..cutoff].to_string();
        head.push_str(&truncation_marker(omitted_bytes, &display));
        head
    } else {
        trimmed.to_string()
    };

    Some(format!(
        "<user_memory source=\"{display}\">\n{payload}\n</user_memory>"
    ))
}

fn truncation_cutoff(content: &str, source: &str) -> usize {
    let mut cutoff = previous_char_boundary(content, MAX_MEMORY_SIZE);
    loop {
        let omitted_bytes = content.len() - cutoff;
        let max_head_len =
            MAX_MEMORY_SIZE.saturating_sub(truncation_marker(omitted_bytes, source).len());
        let next_cutoff = previous_char_boundary(content, cutoff.min(max_head_len));
        if next_cutoff == cutoff {
            return cutoff;
        }
        cutoff = next_cutoff;
    }
}

fn truncation_marker(omitted_bytes: usize, source: &str) -> String {
    format!("\n<truncated bytes={omitted_bytes} source=\"{source}\">")
}

fn previous_char_boundary(value: &str, mut index: usize) -> usize {
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// 为系统提示词组合 `<user_memory>` 块，遵循选择加入开关。
/// 当功能被禁用、`moraine_fallback` 激活或文件缺失/为空时返回 `None`，
/// 这样调用者无需检查两个条件。
///
/// 持有 `&Config` 的调用者应直接传递 `config.memory_enabled() &&
/// !config.moraine_fallback()` 和 `config.memory_path()`。
/// 这种拆分使此模块与 `Config` 无关，从而可以在高级 `Config` 不可用的
/// 子代理/引擎边界处重用。
#[must_use]
pub fn compose_block(enabled: bool, path: &Path) -> Option<String> {
    if !enabled {
        return None;
    }
    let content = load(path)?;
    as_system_block(&content, path)
}

/// 将 `entry` 追加到 `path` 处的记忆文件，如有需要则创建文件（及其父目录）。
/// 条目带有时间戳，以便用户以后查看每条笔记的添加时间。
/// 从 `# foo` 快速添加中去除前导 `#`，使文件保持为可读的 Markdown。
pub fn append_entry(path: &Path, entry: &str) -> io::Result<()> {
    let trimmed = entry.trim_start_matches('#').trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory entry is empty after stripping `#` prefix",
        ));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let timestamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "- ({timestamp}) {trimmed}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_returns_none_for_missing_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("never-existed.md");
        assert!(load(&path).is_none());
    }

    #[test]
    fn load_returns_none_for_whitespace_only_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        fs::write(&path, "   \n   \n").unwrap();
        assert!(load(&path).is_none());
    }

    #[test]
    fn load_returns_content_for_real_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        fs::write(&path, "remember the milk").unwrap();
        assert_eq!(load(&path).as_deref(), Some("remember the milk"));
    }

    #[test]
    fn as_system_block_produces_xml_wrapper() {
        let block = as_system_block("note 1", Path::new("/tmp/m.md")).unwrap();
        assert!(block.contains("<user_memory source=\"/tmp/m.md\">"));
        assert!(block.contains("note 1"));
        assert!(block.ends_with("</user_memory>"));
    }

    #[test]
    fn as_system_block_returns_none_for_empty_content() {
        assert!(as_system_block("   ", Path::new("/tmp/m.md")).is_none());
    }

    #[test]
    fn as_system_block_truncates_oversize_input() {
        let big = "x".repeat(MAX_MEMORY_SIZE + 100);
        let block = as_system_block(&big, Path::new("/tmp/m.md")).unwrap();
        let payload = user_memory_payload(&block);
        assert_eq!(payload.len(), MAX_MEMORY_SIZE);
        assert!(payload.ends_with("<truncated bytes=141 source=\"/tmp/m.md\">"));
    }

    #[test]
    fn as_system_block_truncates_non_ascii_at_char_boundary() {
        let mut content = "x".repeat(MAX_MEMORY_SIZE - 1);
        content.push('é');
        content.push_str("tail");

        let block = as_system_block(&content, Path::new("/tmp/m.md")).unwrap();
        let payload = block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap();
        let (head, marker) = payload
            .split_once("\n<truncated bytes=45 source=\"/tmp/m.md\">")
            .unwrap();

        assert_eq!(payload.len(), MAX_MEMORY_SIZE);
        assert_eq!(head.len(), MAX_MEMORY_SIZE - 40);
        assert!(head.bytes().all(|byte| byte == b'x'));
        assert_eq!(marker, "");
    }

    #[test]
    fn as_system_block_truncates_emoji_at_char_boundary() {
        let mut content = "x".repeat(MAX_MEMORY_SIZE - 1);
        content.push('😀');
        content.push_str("tail");

        let block = as_system_block(&content, Path::new("/tmp/m.md")).unwrap();
        assert!(block.contains("<truncated bytes=47 source=\"/tmp/m.md\">"));

        let payload = block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap();
        let head = payload
            .strip_suffix("\n<truncated bytes=47 source=\"/tmp/m.md\">")
            .unwrap();

        assert_eq!(payload.len(), MAX_MEMORY_SIZE);
        assert!(head.len() <= MAX_MEMORY_SIZE);
        assert_eq!(head.len(), MAX_MEMORY_SIZE - 40);
        assert!(head.bytes().all(|byte| byte == b'x'));
    }

    fn user_memory_payload(block: &str) -> &str {
        block
            .strip_prefix("<user_memory source=\"/tmp/m.md\">\n")
            .unwrap()
            .strip_suffix("\n</user_memory>")
            .unwrap()
    }

    #[test]
    fn append_entry_creates_file_and_writes_one_bullet() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# remember the milk").unwrap();

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("remember the milk"), "{body}");
        assert!(
            body.starts_with("- ("),
            "should start with bullet + date: {body}"
        );
        assert!(body.trim_end().ends_with("remember the milk"));
    }

    #[test]
    fn append_entry_appends_subsequent_lines() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        append_entry(&path, "# first").unwrap();
        append_entry(&path, "second").unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("first"));
        assert!(body.contains("second"));
        // Two bullets means two lines of `- (date) entry`.
        assert_eq!(body.matches("- (").count(), 2);
    }

    #[test]
    fn append_entry_rejects_empty_after_strip() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let err = append_entry(&path, "###").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
