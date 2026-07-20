//! 幽灵文本形式的后续提示建议。
//!
//! 每完成一个回合后，一个轻量级 API 调用会生成一个用户可能想继续追问的简短问题。
//! 当输入为空时，该建议以暗淡的幽灵文本形式显示在 composer 中。

use std::sync::OnceLock;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use tracing::debug;

/// 可复用的静态客户端——避免每次请求都创建新的连接池。
fn suggestion_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(crate::tls::reqwest_client)
}

/// 基于最近的消息生成一个后续提示建议。
///
/// 使用一条系统提示将对话摘要发送到 API，要求生成一个简短的后续问题。
/// 失败或结果为空时返回 `None`——调用方将其视为尽力而为的尝试。
pub async fn generate_suggestion(
    api_key: &str,
    base_url: &str,
    model: &str,
    recent_messages: &str,
) -> Option<String> {
    let client = suggestion_client();
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "\
    You are a helpful assistant. Based on the recent conversation context, generate \
    ONE short follow-up question (under 60 characters) the user might want to ask \
    next. Reply with ONLY the question text, nothing else — no quotes, no explanations, \
    no prefixes."
            },
            {
                "role": "user",
                "content": format!(
                    "Recent conversation:\n{recent_messages}\n\n\
                     Generate ONE short follow-up question the user might ask next:"
                )
            }
        ],
        "max_tokens": 64,
        "temperature": 0.3,
        "stream": false
    });

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    debug!(%url, %model, "generating prompt suggestion");
    let response = match client
        .post(&url)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .header(CONTENT_TYPE, "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return None,
    };

    let value: Value = match response.json().await {
        Ok(v) => v,
        Err(_) => return None,
    };

    let suggestion = value["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty() && s.len() <= 200)?;

    debug!(text = %suggestion, "prompt suggestion generated");
    Some(suggestion)
}

/// 从单条消息中提取第一行文本。
fn message_summary(m: &crate::models::Message) -> Option<String> {
    let role = match m.role.as_str() {
        "user" => "User",
        "assistant" => "Assistant",
        _ => return None,
    };
    let text = m
        .content
        .iter()
        .filter_map(|block| match block {
            crate::models::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let first_line = text.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }
    let truncated: String = first_line
        .chars()
        .take(120)
        .chain(if first_line.chars().count() > 120 {
            Some('…')
        } else {
            None
        })
        .collect();
    Some(format!("{role}: {truncated}"))
}

/// 构建最近对话上下文的每行一条消息的摘要。
/// 取最后 N 条消息，跳过纯工具消息。
pub fn summarize_recent_messages(messages: &[crate::models::Message], limit: usize) -> String {
    let start = messages.len().saturating_sub(limit);
    messages[start..]
        .iter()
        .filter_map(message_summary)
        .collect::<Vec<_>>()
        .join("\n")
}
