//! `remember` 工具——模型可调用的向用户记忆文件添加条目的功能。
//!
//! 让模型自身注意到值得跨会话保留的持久偏好、约定或事实，
//! 并将其写入用户的 `memory.md`。
//! 该工具自动批准，仅对用户拥有的记忆文件（默认 `~/.deepseek/memory.md`）产生副作用，
//! 因此不需要像 shell 或任意文件写入那样经过审批流程。
//!
//! 仅在 `[memory] enabled = true`（或 `DEEPSEEK_MEMORY=on`）时注册。
//! 禁用时，模型完全看不到此工具，因此提及 `remember` 的提示会直接跳过。

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

/// 向用户记忆文件追加一个条目的工具。
pub struct RememberTool;

#[async_trait]
impl ToolSpec for RememberTool {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn description(&self) -> &'static str {
        "Append a durable note to the user memory file so it surfaces in \
         future sessions. Use this when the user states a preference, a \
         convention they want enforced, or a fact about themselves or \
         their workflow that you should not have to relearn next time. \
         Keep notes terse (one sentence). Don't store secrets, transient \
         tasks, or reasoning scratch — those belong in a checklist or in \
         the conversation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note": {
                    "type": "string",
                    "description": "The single-sentence durable note to remember."
                }
            },
            "required": ["note"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // 记忆写操作仅限于用户自己的记忆文件；将其置于标准 shell/write 审批流程之后
        // 会违背自动记忆的目的。
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let note = required_str(&input, "note")?;
        let path = context.memory_path.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "user memory is disabled — set `[memory] enabled = true` in config.toml or \
                 `DEEPSEEK_MEMORY=on` in the environment to enable",
            )
        })?;

        crate::memory::append_entry(path, note).map_err(|err| {
            ToolError::execution_failed(format!("failed to append to {}: {err}", path.display()))
        })?;

        Ok(ToolResult::success(format!(
            "remembered: {}",
            note.trim_start_matches('#').trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_memory(path: PathBuf) -> ToolContext {
        let mut ctx = ToolContext::new(path.parent().unwrap_or_else(|| std::path::Path::new(".")));
        ctx.memory_path = Some(path);
        ctx
    }

    #[tokio::test]
    async fn returns_error_when_memory_disabled() {
        let tmp = tempdir().unwrap();
        let mut ctx = ToolContext::new(tmp.path());
        ctx.memory_path = None; // 显式禁用

        let tool = RememberTool;
        let err = tool
            .execute(json!({"note": "use 4 spaces for indentation"}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("memory is disabled"), "{err}");
    }

    #[tokio::test]
    async fn appends_bullet_to_memory_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let ctx = ctx_with_memory(path.clone());

        let tool = RememberTool;
        let result = tool
            .execute(json!({"note": "use 4 spaces for indentation"}), &ctx)
            .await
            .expect("ok");
        assert!(result.success);
        assert!(result.content.contains("4 spaces"));

        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("4 spaces"));
        assert!(body.starts_with("- ("), "{body}");
    }

    #[tokio::test]
    async fn rejects_missing_note_field() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let ctx = ctx_with_memory(path);

        let tool = RememberTool;
        let err = tool.execute(json!({}), &ctx).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("note"), "{err}");
    }
}
