//! 撤销、重试、编辑和差异命令。

use crate::dependencies::{ExternalTool, Git};
use crate::models::ContentBlock;
use crate::tui::app::{App, AppAction};
use crate::tui::history::HistoryCell;

use super::CommandResult;

/// 移除最后一对消息（用户 + 助手）。
///
/// 这是旧的 `/undo` 行为——它从历史和 API 消息中移除最新的
/// 用户+助手对话对。新的 `/undo` 首先尝试通过 [`patch_undo`]
/// 恢复工作区文件；如果没有快照可用，则回退到此函数。
pub fn undo_conversation(app: &mut App) -> CommandResult {
    // 从显示历史中移除（直到最后一条用户消息）
    let mut removed_count = 0;
    while !app.history.is_empty() {
        let last_is_user = matches!(app.history.last(), Some(HistoryCell::User { .. }));
        app.pop_history();
        removed_count += 1;
        if last_is_user {
            break;
        }
    }

    // 从 API 消息中移除
    while let Some(last) = app.api_messages.last() {
        if last.role == "user" {
            app.api_messages.pop();
            break;
        }
        app.api_messages.pop();
    }

    if removed_count > 0 {
        // 截断后保持工具/索引映射一致。
        app.tool_cells.clear();
        app.tool_details_by_cell.clear();
        app.exploring_entries.clear();
        app.ignored_tool_calls.clear();
        app.mark_history_updated();
        CommandResult::message(format!("Removed {removed_count} message(s)"))
    } else {
        CommandResult::message("Nothing to undo")
    }
}

pub(crate) fn prune_undone_tool_context(app: &mut App, tool_id: &str) {
    if let Some(history_idx) = app.tool_cells.get(tool_id).copied() {
        app.truncate_history_to(history_idx);
    }

    let Some((msg_idx, block_idx)) =
        app.api_messages
            .iter()
            .enumerate()
            .find_map(|(msg_idx, msg)| {
                msg.content
                    .iter()
                    .position(
                        |block| matches!(block, ContentBlock::ToolUse { id, .. } if id == tool_id),
                    )
                    .map(|block_idx| (msg_idx, block_idx))
            })
    else {
        return;
    };

    let kept_blocks = app.api_messages[msg_idx].content[..block_idx].to_vec();
    let kept_tool_ids: std::collections::HashSet<String> = kept_blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    if kept_blocks.is_empty() {
        app.api_messages.truncate(msg_idx);
        return;
    }
    let preserved_tool_results: Vec<_> =
        app.api_messages
            .iter()
            .skip(msg_idx + 1)
            .take_while(|msg| {
                msg.role == "user"
                    && !msg.content.is_empty()
                    && msg
                        .content
                        .iter()
                        .all(|block| tool_result_id(block).is_some())
            })
            .filter(|msg| {
                msg.role == "user"
                    && !msg.content.is_empty()
                    && msg.content.iter().all(|block| {
                        tool_result_id(block).is_some_and(|id| kept_tool_ids.contains(id))
                    })
            })
            .cloned()
            .collect();
    app.api_messages.truncate(msg_idx + 1);
    app.api_messages[msg_idx].content = kept_blocks;
    app.api_messages.extend(preserved_tool_results);
}

fn prune_undone_turn_context(app: &mut App) {
    if let Some(history_idx) = app
        .history
        .iter()
        .rposition(|cell| matches!(cell, HistoryCell::User { .. }))
    {
        app.truncate_history_to(history_idx);
    }

    if let Some(api_idx) = app.api_messages.iter().rposition(|msg| msg.role == "user") {
        app.api_messages.truncate(api_idx);
    }
}

fn tool_result_id(block: &ContentBlock) -> Option<&String> {
    match block {
        ContentBlock::ToolResult { tool_use_id, .. }
        | ContentBlock::ToolSearchToolResult { tool_use_id, .. }
        | ContentBlock::CodeExecutionToolResult { tool_use_id, .. } => Some(tool_use_id),
        _ => None,
    }
}

/// 回滚最近的写入工具（apply_patch/edit_file/write_file）或轮次。
///
/// 打开侧边 git 快照仓库并查找最近的快照，优先选择
/// 每个工具的快照（`tool:*`）而不是轮次前快照（`pre-turn:*`）。
/// 从该快照恢复文件并显示差异摘要。当没有快照存在时
/// 回退到对话撤销。
///
/// 发布一个 `HistoryCell::System` 条目，以便用户可以在对话记录中
/// 看到回滚了什么。
pub fn patch_undo(app: &mut App) -> CommandResult {
    let workspace = app.workspace.clone();

    let repo = match crate::snapshot::SnapshotRepo::open_or_init(&workspace) {
        Ok(r) => r,
        Err(e) => {
            return CommandResult::error(format!(
                "Snapshot repo unavailable for {}: {e}",
                workspace.display(),
            ));
        }
    };

    let snapshots = match repo.list(20) {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!("Failed to list snapshots: {e}"));
        }
    };

    if snapshots.is_empty() {
        return CommandResult::message("No snapshots found to undo — nothing to revert.");
    }

    // 优先选择最新的可回滚 `tool:` / `pre-turn:` 快照，其
    // 跟踪内容与当前工作区不同。这允许重复 `/undo` 向后
    // 遍历更旧的快照，而不是永远恢复同一个无变化的目标。
    let target = snapshots
        .iter()
        .filter(|s| s.label.starts_with("tool:") || s.label.starts_with("pre-turn:"))
        .find(|s| match repo.work_tree_matches_snapshot(&s.id) {
            Ok(matches) => !matches,
            Err(_) => true,
        });

    let Some(target) = target else {
        return CommandResult::message(
            "No older tool or pre-turn snapshots differ from the current workspace — nothing to revert.",
        );
    };

    if let Err(e) = repo.restore(&target.id) {
        return CommandResult::error(format!("Restore failed: {e}"));
    }

    if let Some(tool_id) = target.label.strip_prefix("tool:") {
        prune_undone_tool_context(app, tool_id);
    } else if target.label.starts_with("pre-turn:") {
        prune_undone_turn_context(app);
    }

    // 显示差异统计，让用户知道发生了什么变化。
    let diff_stat = Git::command()
        .map(|mut git| {
            git.args(["diff", "--stat"])
                .current_dir(&workspace)
                .output()
                .ok()
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
        })
        .unwrap_or(None);

    let short = &target.id.as_str()[..target.id.as_str().len().min(8)];
    let summary = match diff_stat {
        Some(ref stat) => {
            format!(
                "Restored snapshot '{}' ({}). Files affected:\n{stat}",
                target.label, short
            )
        }
        None => {
            format!(
                "Restored snapshot '{}' ({}). No diff changes detected.",
                target.label, short
            )
        }
    };

    // 发布系统单元格，使回滚状态在对话记录中可见。
    app.push_history_cell(HistoryCell::System {
        content: format!(
            "/undo reverted workspace to snapshot '{}' ({})",
            target.label, short
        ),
    });

    CommandResult::with_message_and_action(
        summary,
        AppAction::SyncSession {
            session_id: app.current_session_id.clone(),
            messages: app.api_messages.clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        },
    )
}

/// 将最后一条用户消息加载回编辑器以供编辑。
///
/// 搜索 `app.history` 中最近的 `HistoryCell::User`，将其内容
/// 复制到 `app.input`，并将游标定位在末尾，以便用户可以编辑
/// 并按 Enter 重新提交。原始的交互保留在对话记录中可见。
pub fn edit(app: &mut App) -> CommandResult {
    let last_user = app.history.iter().rev().find_map(|cell| match cell {
        HistoryCell::User { content } => Some(content.clone()),
        _ => None,
    });

    match last_user {
        Some(content) => {
            app.input = content;
            app.cursor_position = app.input.chars().count();
            app.edit_in_progress = true;
            CommandResult::message(
                "Last message loaded into composer — edit and press Enter to resubmit",
            )
        }
        None => CommandResult::message("No previous message to edit"),
    }
}

/// 显示自会话开始以来的 git 差异输出。
///
/// 在工作区目录中运行 `git diff --stat` 和 `git diff --name-only`。
/// 显示哪些文件已更改以及统计摘要。如果没有更改或 git 失败，
/// 返回适当的消息。
pub fn diff(app: &mut App) -> CommandResult {
    let workspace = app.workspace.clone();

    let Some(mut name_only_cmd) = Git::command() else {
        return CommandResult::error("git not found on PATH");
    };
    let Some(mut stat_cmd) = Git::command() else {
        return CommandResult::error("git not found on PATH");
    };
    let name_only_output = name_only_cmd
        .args(["diff", "--name-only"])
        .current_dir(&workspace)
        .output();
    let stat_output = stat_cmd
        .args(["diff", "--stat"])
        .current_dir(&workspace)
        .output();

    match (name_only_output, stat_output) {
        (Ok(name_only), Ok(stat)) => {
            let name_stdout = String::from_utf8_lossy(&name_only.stdout);
            let stat_stdout = String::from_utf8_lossy(&stat.stdout);

            if name_stdout.trim().is_empty() {
                return CommandResult::message("No changes since session start");
            }

            let files: Vec<&str> = name_stdout.lines().filter(|l| !l.is_empty()).collect();
            let file_count = files.len();
            let file_list = files.join("\n");

            // 检测重命名条目（例如 "foo -> bar"）并将其从
            // 文件计数标题中排除，以便用户只看到实际修改。
            let renamed_count = files.iter().filter(|f| f.contains(" -> ")).count();
            let summary = if renamed_count > 0 {
                format!("Changed files ({file_count}, {renamed_count} renamed):\n{file_list}")
            } else {
                format!("Changed files ({file_count}):\n{file_list}")
            };

            let stat_str = stat_stdout.trim();
            let mut message = summary;
            if !stat_str.is_empty() {
                message.push_str("\n\n── Stat ──\n");
                message.push_str(stat_str);
            }
            CommandResult::message(message)
        }
        (Err(e), _) | (_, Err(e)) => {
            CommandResult::message(format!("Git diff failed — is this a git repository?\n{e}"))
        }
    }
}

/// 重试最后一条请求——移除最后一次交换并重新发送用户的消息
pub fn retry(app: &mut App) -> CommandResult {
    let last_user_input = app.history.iter().rev().find_map(|cell| match cell {
        HistoryCell::User { content } => Some(content.clone()),
        _ => None,
    });

    match last_user_input {
        Some(input) => {
            undo_conversation(app);
            let display_input = if input.len() > 50 {
                let truncate_at = input
                    .char_indices()
                    .take_while(|(i, _)| *i <= 50)
                    .last()
                    .map_or(0, |(i, _)| i);
                format!("{}...", &input[..truncate_at])
            } else {
                input.clone()
            };
            CommandResult::with_message_and_action(
                format!("Retrying: {display_input}"),
                AppAction::SendMessage(input),
            )
        }
        None => CommandResult::error("No previous request to retry"),
    }
}
