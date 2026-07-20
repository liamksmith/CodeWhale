use std::collections::VecDeque;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;

use super::headers::{apply_safe_custom_headers, with_default_mcp_http_headers};
use super::{
    ERROR_BODY_PREVIEW_BYTES, McpHttpAuth, bounded_body_excerpt, mask_url_secrets,
    parse_sse_message_data,
};

pub(super) struct StreamableHttpTransport {
    pub(super) client: reqwest::Client,
    pub(super) url: String,
    /// 出站 POST 的请求时身份验证和自定义头部解析器。
    pub(super) auth: McpHttpAuth,
    pending_messages: VecDeque<Vec<u8>>,
    /// 服务器在第一次响应（通常是 `initialize` 响应）中返回的
    /// 规范的 MCP 会话标识符。附加为每个后续出站请求的
    /// `Mcp-Session-Id` 头部，以便服务器可以关联同一
    /// 会话内的消息。
    pub(super) session_id: Option<String>,
}

#[derive(Debug)]
pub(super) enum StreamableSendError {
    Incompatible(String),
    StaleSession(String),
    Other(anyhow::Error),
}

impl StreamableHttpTransport {
    pub(super) fn new(client: reqwest::Client, url: String, auth: McpHttpAuth) -> Self {
        Self {
            client,
            url,
            auth,
            pending_messages: VecDeque::new(),
            session_id: None,
        }
    }

    pub(super) async fn send(
        &mut self,
        msg: Vec<u8>,
    ) -> std::result::Result<(), StreamableSendError> {
        // 在协议框架后应用用户配置的自定义头部，这样保留的
        // Accept / Content-Type 覆写可以被过滤掉。
        let headers = self
            .auth
            .resolved_headers()
            .await
            .map_err(StreamableSendError::Other)?;
        let mut request = apply_safe_custom_headers(
            with_default_mcp_http_headers(self.client.post(&self.url), true),
            &headers,
        );
        // 根据 Streamable HTTP 规范附加任何先前捕获的会话 ID，
        // 以便服务器可以将此请求关联到现有会话。
        if let Some(ref sid) = self.session_id {
            request = request.header("Mcp-Session-Id", sid.as_str());
        }
        let response = request
            .body(msg)
            .send()
            .await
            .map_err(|err| StreamableSendError::Other(err.into()))?;

        let status = response.status();

        // 从任何响应（2xx、202、4xx……）中捕获会话 ID。
        // 服务器可能会在 `initialize` 响应或下面的
        // 尽力 GET 预检中返回它。
        if let Some(sid) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
            && self.session_id.as_deref() != Some(sid)
        {
            let session_ref = crate::utils::redacted_identifier_for_log(sid);
            tracing::debug!(target: "mcp", session = %session_ref, "捕获到 MCP 会话 ID");
            self.session_id = Some(sid.to_string());
        }
        if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
            return Ok(());
        }

        if !status.is_success() {
            let body_excerpt = bounded_body_excerpt(response, ERROR_BODY_PREVIEW_BYTES).await;
            if self.session_id.is_some()
                && is_streamable_http_stale_session_status(status, &body_excerpt)
            {
                return Err(StreamableSendError::StaleSession(format!(
                    "status={status} body={body_excerpt}"
                )));
            }
            if is_streamable_http_incompatible_status(status) {
                return Err(StreamableSendError::Incompatible(format!(
                    "status={status} body={body_excerpt}"
                )));
            }
            return Err(StreamableSendError::Other(anyhow::anyhow!(
                "MCP Streamable HTTP 被拒绝（transport=http url={} status={}）：{}",
                mask_url_secrets(&self.url),
                status,
                body_excerpt,
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        // 在读取任何内容之前拒绝过大的声明体（快速路径），
        // 然后限制读取本身，这样分块/无长度的响应也无法导致 OOM——
        // 仅靠 Content-Length 无法防止不声明长度就进行流式传输的服务器。
        if let Some(len) = response.content_length()
            && len > super::MAX_MCP_RESPONSE_BYTES as u64
        {
            return Err(StreamableSendError::Other(anyhow::anyhow!(
                "MCP 响应 Content-Length {len} 超过了 {} 字节——正在中止",
                super::MAX_MCP_RESPONSE_BYTES
            )));
        }
        let body = read_body_capped(response, super::MAX_MCP_RESPONSE_BYTES)
            .await
            .map_err(StreamableSendError::Other)?;
        self.store_response_body(content_type.as_deref(), &body)
            .map_err(StreamableSendError::Other)
    }

    pub(super) async fn recv(&mut self) -> Result<Vec<u8>> {
        self.pending_messages
            .pop_front()
            .context("MCP Streamable HTTP 响应队列为空")
    }

    fn store_response_body(&mut self, content_type: Option<&str>, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Ok(());
        }

        let is_event_stream = content_type
            .map(|value| value.to_ascii_lowercase().contains("text/event-stream"))
            .unwrap_or(false)
            || body.trim_start().starts_with("event:")
            || body.trim_start().starts_with("data:");

        if is_event_stream {
            for msg in parse_sse_message_data(body) {
                self.pending_messages.push_back(msg);
            }
            return Ok(());
        }

        self.pending_messages.push_back(body.as_bytes().to_vec());
        Ok(())
    }
}

/// 通过字节流读取响应体，一旦超出 `max_bytes` 就失败。
/// 这像限制声明的长度一样限制分块和缺少 Content-Length 的响应
///（`send` 中的声明长度快速路径仅覆盖诚实声明其大小的服务器）。
/// MCP 体是 JSON 或 SSE，因此 lossy UTF-8 与 `.text()` 行为匹配。
pub(super) async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<String> {
    use futures_util::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("读取 MCP 响应体失败")?;
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("MCP 响应体超过 {max_bytes} 字节——正在中止");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn is_streamable_http_incompatible_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_ACCEPTABLE
            | StatusCode::UNSUPPORTED_MEDIA_TYPE
            | StatusCode::NOT_IMPLEMENTED
    )
}

fn is_streamable_http_stale_session_status(status: StatusCode, body_excerpt: &str) -> bool {
    if status == StatusCode::NOT_FOUND {
        return true;
    }
    if status != StatusCode::BAD_REQUEST && status != StatusCode::UNAUTHORIZED {
        return false;
    }
    let body = body_excerpt.to_ascii_lowercase();
    body.contains("session") && (body.contains("expired") || body.contains("invalid"))
}
