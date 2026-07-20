//! `/stash` 斜杠命令 — 列出/提取已暂存的编辑器草稿 (#440)。
//!
//! 磁盘格式和持久化规则参见 `crates/tui/src/composer_stash.rs`。
//! 斜杠命令是用户交互界面；编辑器中的 Ctrl+S 是对应的推入入口。

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::composer_stash;
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "stash",
    aliases: &["park"],
    usage: "/stash [list|pop|clear]",
    description_id: MessageId::CmdStashDescription,
};

pub(in crate::commands) struct StashCmd;

impl RegisterCommand for StashCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        stash(app, arg)
    }
}

/// `/stash` 的顶层调度。子命令：
///
/// * `/stash`        — 等同于 `/stash list`。
/// * `/stash list`   — 显示已暂存的草稿，最早的在前。
/// * `/stash pop`    — 将最近暂存的草稿恢复到编辑器中；弹出的条目从磁盘删除。
/// * `/stash clear`  — 清空整个暂存文件。报告丢弃了多少条目，以便用户知晓删除内容。
pub fn stash(app: &mut App, arg: Option<&str>) -> CommandResult {
    let sub = arg.map(str::trim).unwrap_or("list").to_ascii_lowercase();
    match sub.as_str() {
        "" | "list" | "ls" | "show" => list(),
        "pop" | "restore" => pop(app),
        "clear" | "wipe" | "drop" => clear(),
        other => CommandResult::error(format!(
            "unknown subcommand `{other}`. Try `/stash list`, `/stash pop`, or `/stash clear`."
        )),
    }
}

fn list() -> CommandResult {
    let entries = composer_stash::load_stash();
    if entries.is_empty() {
        return CommandResult::message(
            "Stash empty. Press Ctrl+S in the composer to park the current draft.",
        );
    }
    let mut out = String::new();
    out.push_str(&format!("{} parked draft(s):\n\n", entries.len()));
    for (idx, entry) in entries.iter().enumerate() {
        let preview = preview_first_line(&entry.text, 80);
        let ts = if entry.ts.is_empty() {
            "(no ts)".to_string()
        } else {
            entry.ts.clone()
        };
        out.push_str(&format!("  {idx}. [{ts}] {preview}\n"));
    }
    out.push_str("\nUse `/stash pop` to restore the most recent draft.");
    CommandResult::message(out)
}

fn clear() -> CommandResult {
    match composer_stash::clear_stash() {
        Ok(0) => CommandResult::message("Stash already empty — nothing to clear."),
        Ok(n) => CommandResult::message(format!("Cleared {n} parked draft(s) from the stash.")),
        Err(err) => CommandResult::error(format!("Failed to clear stash: {err}")),
    }
}

fn pop(app: &mut App) -> CommandResult {
    match composer_stash::pop_stash() {
        Some(entry) => {
            // 用弹出的草稿替换当前编辑器内容。我们不合并 — 替换是可预测的行为，
            // 符合"恢复已暂存的草稿"的心智模型。镜像队列编辑模式以实现光标重置。
            app.input = entry.text.clone();
            app.cursor_position = app.input.len();
            let preview = preview_first_line(&entry.text, 60);
            // 告知用户剩余草稿数量，以便他们计划是继续弹出还是继续其他操作。
            // 匹配队列界面使用的确认模式。
            let remaining = composer_stash::load_stash().len();
            let suffix = match remaining {
                0 => " (stash now empty)".to_string(),
                1 => " (1 more parked)".to_string(),
                n => format!(" ({n} more parked)"),
            };
            CommandResult::message(format!("Restored stashed draft: {preview}{suffix}"))
        }
        None => CommandResult::message("Stash empty — nothing to pop."),
    }
}

/// 取 `text` 的单行预览，截断至 `max_chars` 字符。
/// 多行草稿将显示单行摘要，使列表保持可浏览性。
fn preview_first_line(text: &str, max_chars: usize) -> String {
    let head = text.lines().next().unwrap_or("").trim();
    if head.chars().count() <= max_chars {
        return head.to_string();
    }
    let mut out: String = head.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_first_line_truncates_to_cap() {
        let body = "x".repeat(200);
        let p = preview_first_line(&body, 10);
        assert_eq!(p.chars().count(), 10);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn preview_first_line_keeps_short_input_intact() {
        assert_eq!(preview_first_line("short", 50), "short");
    }

    #[test]
    fn preview_first_line_only_uses_first_line_of_multiline() {
        let body = "first line of the draft\nsecond line that's longer\nthird";
        assert_eq!(preview_first_line(body, 80), "first line of the draft");
    }

    #[test]
    fn preview_first_line_handles_empty_input() {
        assert_eq!(preview_first_line("", 50), "");
        assert_eq!(preview_first_line("   ", 50), "");
    }
}
