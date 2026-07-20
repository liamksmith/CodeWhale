//! 自动路由辅助函数：决定何时咨询自动路由闪念模型，
//! 以及构建它看到的小上下文窗口。
//!
//! 当 `app.auto_model` 设置时，TUI 在每个用户轮次调用一次
//! `resolve_auto_model_selection`。这个异步函数从 `api_messages`
//! 构建一个近期上下文摘要（最多六行，每行最多 900 字符），
//! 通过 `model_routing::resolve_auto_route_with_inventory` 传递它，
//! 并返回选择结果（模型 + 推理力度）。其余辅助函数是用于构建
//! 该摘要的纯转换函数。

use anyhow::Result;

use crate::config::Config;
use crate::model_routing;
use crate::models::{ContentBlock, Message};
use crate::tui::app::{App, QueuedMessage, ReasoningEffort};

/// 下一个轮次是否应咨询自动路由闪念模型。
pub(super) fn should_resolve_auto_model_selection(app: &App) -> bool {
    app.auto_model
}

/// 使用用户的草稿 + 短近期上下文窗口调用自动路由闪念模型。
/// 返回选中的模型和推理力度。
pub(super) async fn resolve_auto_model_selection(
    app: &App,
    config: &Config,
    message: &QueuedMessage,
    latest_content: &str,
) -> Result<model_routing::AutoRouteSelection> {
    let latest_request = if latest_content.trim().is_empty() {
        message.display.as_str()
    } else {
        latest_content
    };
    model_routing::resolve_auto_route_with_inventory_for_session(
        config,
        latest_request,
        &recent_auto_router_context(&app.api_messages),
        app.mode.as_setting(),
        if app.auto_model { "auto" } else { "fixed" },
        app.reasoning_effort
            .as_setting_for_provider(app.api_provider),
    )
    .await
}

/// 将启发式推理力度归一化为规范的自动路由推理力度。
pub(super) fn normalize_auto_routed_effort(effort: ReasoningEffort) -> ReasoningEffort {
    model_routing::normalize_auto_route_effort(effort)
}

/// 为自动路由提示词构建紧凑的近期上下文摘要。
///
/// 从最近的一个轮次开始反向遍历 `api_messages`，跳过
/// 最终的草稿（它是路由器被要求分类的对象），
/// 收集最多六行非空行，然后反转使提示词按从旧到新
/// 的顺序阅读。每行的格式为 `<role>: <截断的内容>`，
/// 上限为 900 字符。
pub(super) fn recent_auto_router_context(messages: &[Message]) -> String {
    let mut rows = Vec::new();
    for message in messages.iter().rev().skip(1) {
        if rows.len() >= 6 {
            break;
        }
        let text = content_blocks_text(&message.content);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        rows.push(format!(
            "{}: {}",
            message.role,
            truncate_for_auto_router(text, 900)
        ));
    }
    rows.reverse();
    if rows.is_empty() {
        "无先前上下文。".to_string()
    } else {
        rows.join("\n")
    }
}

fn content_blocks_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                append_router_text(&mut out, text);
            }
            ContentBlock::Thinking { .. } => {}
            ContentBlock::ToolUse { name, .. } => {
                append_router_text(&mut out, &format!("[工具调用：{name}]"));
            }
            ContentBlock::ToolResult { content, .. } => {
                append_router_text(&mut out, &format!("[工具结果] {content}"));
            }
            _ => {}
        }
    }
    out
}

fn append_router_text(out: &mut String, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

fn truncate_for_auto_router(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContentBlock;

    fn make_msg(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn truncate_for_auto_router_honors_char_budget() {
        let s = "abcdefghij";
        assert_eq!(truncate_for_auto_router(s, 4), "abcd...");
        assert_eq!(truncate_for_auto_router(s, 10), "abcdefghij");
        assert_eq!(truncate_for_auto_router(s, 100), "abcdefghij");
    }

    #[test]
    fn recent_auto_router_context_skips_final_message_and_caps_rows() {
        // 八条消息；最后一条（正在路由的草稿）被跳过，
        // 因此我们期望最多从剩余的七条中获取六条。
        let msgs: Vec<Message> = (0..8)
            .map(|i| {
                make_msg(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &format!("turn {i}"),
                )
            })
            .collect();
        let context = recent_auto_router_context(&msgs);
        assert!(!context.contains("turn 7"), "最终草稿必须被跳过");
        let row_count = context.lines().count();
        assert_eq!(row_count, 6);
        // 输出按从旧到新的顺序。
        let first = context.lines().next().unwrap();
        assert!(first.contains("turn 1"), "实际：{context}");
    }

    #[test]
    fn recent_auto_router_context_handles_empty_history() {
        assert_eq!(recent_auto_router_context(&[]), "无先前上下文。");
    }

    #[test]
    fn recent_auto_router_context_excludes_hidden_thinking() {
        let msgs = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        signature: None,
                        thinking: "用户似乎在让我对自己进行分类。".to_string(),
                    },
                    ContentBlock::Text {
                        text: "可见的助手回答。".to_string(),
                        cache_control: None,
                    },
                ],
            },
            make_msg("user", "最新草稿"),
        ];

        let context = recent_auto_router_context(&msgs);

        assert!(context.contains("可见的助手回答。"));
        assert!(!context.contains("用户似乎在"));
        assert!(!context.contains("最新草稿"));
    }
}
