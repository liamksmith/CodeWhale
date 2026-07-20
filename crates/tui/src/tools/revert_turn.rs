//! `revert_turn`——可被代理调用的工具，用于将工作区回滚到之前的轮次前快照。
//!
//! 当用户说"撤销上次编辑"或"回滚"时，模型调用此工具。它类似于 `/restore`，
//! 但使用 JSON 通信并接受轮次偏移量（默认 1 = 上一个轮次）而不是列表索引，
//! 这样模型不必计数条目。
//!
//! 审批要求为 `Required`，因为这会修改工作区。

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, optional_u64,
};
use crate::snapshot::SnapshotRepo;

/// 默认偏移量：回滚最近的轮次（即历史中最后一个 `pre-turn:*` 快照）。
const DEFAULT_OFFSET: u64 = 1;
/// 硬上限，防止模型要求回滚到最初状态。
const MAX_OFFSET: u64 = 50;

pub struct RevertTurnTool;

#[async_trait]
impl ToolSpec for RevertTurnTool {
    fn name(&self) -> &str {
        "revert_turn"
    }

    fn description(&self) -> &str {
        "Roll back the workspace files to the snapshot taken before a recent turn. \
         Use when the user explicitly asks to undo, revert, or roll back the most recent edits. \
         `turn_offset` is 1-based: 1 reverts the most recent turn, 2 reverts the previous one, \
         and so on (max 50). Conversation history is NOT modified — only working-tree files are \
         restored from the side-git snapshot repo."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "turn_offset": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_OFFSET,
                    "description": "How many turns back to revert (default 1)."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let offset = optional_u64(&input, "turn_offset", DEFAULT_OFFSET);
        if offset == 0 || offset > MAX_OFFSET {
            return Err(ToolError::invalid_input(format!(
                "turn_offset must be between 1 and {MAX_OFFSET}; got {offset}",
            )));
        }

        let workspace = context.workspace.clone();
        let label = format!("revert_turn(offset={offset})");
        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let repo = SnapshotRepo::open_or_init(&workspace)
                .map_err(|e| format!("Snapshot repo init failed: {e}"))?;
            // 只查找 pre-turn:* 快照——它们标记每个轮次的开始，
            // 是合适的回滚目标。我们拉取一个宽松的列表并过滤，
            // 这样模型的 `turn_offset` 按轮次计数，而非原始快照。
            let snapshots = repo
                .list((MAX_OFFSET as usize).saturating_mul(2) + 16)
                .map_err(|e| format!("Snapshot list failed: {e}"))?;
            let pre_turns: Vec<_> = snapshots
                .into_iter()
                .filter(|s| s.label.starts_with("pre-turn:"))
                .collect();
            let target = pre_turns
                .get((offset - 1) as usize)
                .ok_or_else(|| {
                    format!(
                        "Only {} pre-turn snapshot(s) exist; turn_offset={offset} is out of range.",
                        pre_turns.len(),
                    )
                })?
                .clone();
            if repo
                .work_tree_matches_snapshot(&target.id)
                .map_err(|e| format!("Snapshot comparison failed: {e}"))?
            {
                return Err(format!(
                    "NoSnapshotForTurn: target '{}' ({}) already matches the current workspace. \
                     Revert operates at completed turn boundaries; there is no distinct later snapshot to restore.",
                    target.label,
                    short_sha(target.id.as_str()),
                ));
            }
            repo.restore(&target.id)
                .map_err(|e| format!("Restore failed: {e}"))?;
            Ok(format!(
                "{label}: restored '{}' ({}). Workspace files reverted; conversation unchanged.",
                target.label,
                short_sha(target.id.as_str()),
            ))
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("revert_turn join failed: {e}")))?;

        match result {
            Ok(msg) => Ok(ToolResult::success(msg)),
            Err(e) => Ok(ToolResult::error(e)),
        }
    }
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_test_env;
    use std::sync::MutexGuard;
    use tempfile::tempdir;

    /// 在测试期间将 HOME 固定到临时目录，处于进程级环境互斥锁
    /// （`crate::test_support::lock_test_env`）的保护下。
    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        _lock: MutexGuard<'static, ()>,
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY：进程级锁仍在持有中。
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }
    fn scoped_home(home: &std::path::Path) -> HomeGuard {
        let lock = lock_test_env();
        let prev = std::env::var_os("HOME");
        // SAFETY：由全局环境锁序列化。
        unsafe {
            std::env::set_var("HOME", home);
        }
        HomeGuard { prev, _lock: lock }
    }

    #[tokio::test]
    async fn revert_turn_default_offset_restores_pre_turn_one() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        // 设置：创建 pre-turn:1、post-turn:1 并附带文件修改。
        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), b"original").unwrap();
        repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(workspace.join("a.txt"), b"modified").unwrap();
        repo.snapshot("post-turn:1").unwrap();

        let tool = RevertTurnTool;
        let ctx = ToolContext::new(workspace.clone());
        let r = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(r.success, "expected success: {r:?}");

        let content = std::fs::read_to_string(workspace.join("a.txt")).unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn revert_turn_invalid_offset_rejected() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let tool = RevertTurnTool;
        let ctx = ToolContext::new(workspace);
        let r = tool.execute(json!({"turn_offset": 0}), &ctx).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn revert_turn_rejects_snapshot_matching_current_workspace() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), b"unchanged").unwrap();
        repo.snapshot("pre-turn:1").unwrap();

        let tool = RevertTurnTool;
        let ctx = ToolContext::new(workspace);
        let r = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(!r.success);
        assert!(r.content.contains("NoSnapshotForTurn"), "{}", r.content);
    }

    #[tokio::test]
    async fn revert_turn_no_snapshots_returns_error_result() {
        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let _guard = scoped_home(tmp.path());

        let tool = RevertTurnTool;
        let ctx = ToolContext::new(workspace);
        let r = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(!r.success);
        assert!(r.content.contains("out of range"));
    }
}
