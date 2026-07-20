//! `notify` 工具——模型可调用的桌面通知（#1322）。
//!
//! 通过现有的 `tui::notifications` 基础设施路由（对已知支持的终端使用
//! OSC 9，macOS/Linux 上回退 BEL，Windows 上显示使用 `MessageBeep`）。
//! 模型决定何时触发——此工具用于"长任务完成，请回来"的提示和
//! 子代理完成的通知，而非闲聊。
//!
//! 当 `[notifications].method = "off"` 时自动静默。输出消息
//! 有长度限制，防止失控的模型在终端标题栏中写入大段文字。

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, required_str,
};
use crate::tui::notifications::{Method, notify_done};

/// 传递给标题的最大字符数——确保 OSC 9 转义码
/// 在那些不擅长处理长标题的终端上保持合理。
const NOTIFY_TITLE_CAP: usize = 80;
/// 传递给正文的最大字符数。大多数接收端会在 ~120 字符处截断，
/// 因此 200 在留有回旋余地的同时仍有限界。
const NOTIFY_BODY_CAP: usize = 200;

/// 触发单次桌面通知的工具。
pub struct NotifyTool;

#[async_trait]
impl ToolSpec for NotifyTool {
    fn name(&self) -> &'static str {
        "notify"
    }

    fn description(&self) -> &'static str {
        "触发单次桌面通知（OSC 9 / 终端铃声）。请谨慎使用——\
         仅当长时间运行的任务完成、某个轮次正在等待的远程操作刚结束、\
         或用户确实需要回到终端时使用。传入一个简短 \
         的 `title` 和一个可选的 `body`。不要将其用于 \
         常规进度更新、对话确认或确认模型仍在运行——那都是噪音。用户 \
         可以通过 `~/.deepseek/config.toml` 中的 \
         `[notifications].method = \"off\"` 完全禁用通知；\
         禁用时此工具为静默空操作。"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "简短的通知标题（截断后 ≤ 80 字符）。必填。"
                },
                "body": {
                    "type": "string",
                    "description": "可选的长正文（截断后 ≤ 200 字符）。"
                }
            },
            "required": ["title"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // 没有文件系统或 shell 副作用；唯一的输出是一次终端转义码写入 stdout。
        // 标记为 ReadOnly，这样审批要求的默认值为 `Auto`，工具
        // 无需提示即可直接路由。
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let title_raw = required_str(&input, "title")?;
        let body_raw = optional_str(&input, "body").unwrap_or("");

        // 按字符（而非字节）截断，这样不会从多字节序列中间
        // 切断，向终端输出无效的 UTF-8。
        let title: String = title_raw.chars().take(NOTIFY_TITLE_CAP).collect();
        let body: String = body_raw.chars().take(NOTIFY_BODY_CAP).collect();
        let title = title.trim();
        let body = body.trim();

        if title.is_empty() {
            return Err(ToolError::execution_failed("title 不能为空"));
        }

        let msg = if body.is_empty() {
            title.to_string()
        } else {
            format!("{title}: {body}")
        };

        let in_tmux = std::env::var("TMUX")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        // 阈值为 0，这样通知始终触发；模型已经决定这是合适的时机。
        notify_done(
            Method::Auto,
            in_tmux,
            &msg,
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
        );

        Ok(ToolResult::success(format!("已通知：{title}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ctx() -> ToolContext {
        ToolContext::new(Path::new("."))
    }

    #[tokio::test]
    async fn rejects_missing_title() {
        let err = NotifyTool.execute(json!({}), &ctx()).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("title"), "{err}");
    }

    #[tokio::test]
    async fn rejects_empty_title_after_trim() {
        let err = NotifyTool
            .execute(json!({"title": "   "}), &ctx())
            .await
            .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("must not be empty"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn truncates_title_to_cap() {
        let long = "x".repeat(500);
        let result = NotifyTool
            .execute(json!({"title": long}), &ctx())
            .await
            .expect("ok");
        // 确认消息回显的是*截断后*的标题。
        let echo_x_count = result.content.matches('x').count();
        assert_eq!(echo_x_count, NOTIFY_TITLE_CAP);
    }

    #[tokio::test]
    async fn accepts_body_optional() {
        let result = NotifyTool
            .execute(json!({"title": "done", "body": "tests pass"}), &ctx())
            .await
            .expect("ok");
        assert!(result.success);
        assert!(result.content.contains("done"));
    }

    #[tokio::test]
    async fn safe_against_multibyte_truncation() {
        // 构造一个字符数低于上限但字节数会超过朴素字节上限的标题；
        // 断言不会 panic 且成功内容完整保留了标题。
        let title: String = "我".repeat(30); // 30 字符 × 3 字节 = 90 字节，< 80 字符上限（实际 == 30 字符）
        let result = NotifyTool
            .execute(json!({"title": title.clone()}), &ctx())
            .await
            .expect("ok");
        assert!(result.content.contains(&title));
    }

    #[test]
    fn schema_exposes_title_and_body_fields() {
        let schema = NotifyTool.input_schema();
        let props = schema.get("properties").unwrap();
        assert!(props.get("title").is_some());
        assert!(props.get("body").is_some());
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("title")));
        assert!(!required.iter().any(|v| v.as_str() == Some("body")));
    }
}
