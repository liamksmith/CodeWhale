use std::collections::HashMap;

use reqwest::header::{ACCEPT, CONTENT_TYPE};

pub(super) const MCP_HTTP_ACCEPT: &str = "application/json, text/event-stream";

pub(super) fn with_default_mcp_http_headers(
    request: reqwest::RequestBuilder,
    json_body: bool,
) -> reqwest::RequestBuilder {
    let request = request.header(ACCEPT, MCP_HTTP_ACCEPT);
    if json_body {
        request.header(CONTENT_TYPE, "application/json")
    } else {
        request
    }
}

/// MCP HTTP 传输使用的自定义标头谓词。
///
/// 我们接受 reqwest 的 `HeaderName::try_from` / `HeaderValue::try_from` 所接受的任何内容，
/// 但有三条额外规则：
///
/// 1. 拒绝空/仅空白字符的键 — 这些会在发送过程中导致请求构建器错误并中止整个连接。
/// 2. 拒绝重复我们已发出框架的键（`Accept`、`Content-Type`）。MCP Streamable HTTP 传输
///    依赖这些精确值进行协议协商；意外的用户覆盖可能静默破坏工具发现。
/// 3. 拒绝包含 ASCII CR 或 LF 的值。reqwest 已经拒绝了这些，但显式检查使失败路径可见
///   （`tracing::warn!` 而非晦涩的构建器错误），并记录了响应拆分防御。
///
/// 返回 `false` 表示"跳过此标头"；请求的其余部分仍会发出。
pub(crate) fn is_safe_custom_header(key: &str, value: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.eq_ignore_ascii_case("accept") || trimmed.eq_ignore_ascii_case("content-type") {
        return false;
    }
    !value.contains('\r') && !value.contains('\n')
}

pub(super) fn apply_safe_custom_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HashMap<String, String>,
) -> reqwest::RequestBuilder {
    for (key, value) in headers {
        if !is_safe_custom_header(key, value) {
            tracing::warn!(
                target: "mcp",
                "skipping unsafe MCP header {:?} (empty/control-char/reserved)",
                key
            );
            continue;
        }
        request = request.header(key.as_str(), value.as_str());
    }
    request
}
