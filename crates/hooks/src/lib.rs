use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use codewhale_protocol::EventFrame;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

/// 可通过钩子系统发出的所有事件。
///
/// 每个变体代表一个不同的生命周期或流式事件。该枚举使用
/// `"type"` 鉴别器以 `snake_case` 命名约定进行序列化（例如
/// `"response_start"`、`"tool_lifecycle"`），方便从
/// 基于 JSON 的日志文件或 webhook 接收端消费。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookEvent {
    /// 新的响应流已开始。
    ResponseStart {
        /// 正在流式传输的响应的唯一标识符。
        response_id: String,
    },
    /// 已接收到进行中响应的文本块。
    ResponseDelta {
        /// 正在流式传输的响应的唯一标识符。
        response_id: String,
        /// 此块的增量文本内容。
        delta: String,
    },
    /// 响应流已完成。
    ResponseEnd {
        /// 已完成的响应的唯一标识符。
        response_id: String,
    },
    /// 工具调用已转换到新阶段（例如开始、结束、错误）。
    ToolLifecycle {
        /// 调用该工具所在响应的标识符。
        response_id: String,
        /// 工具名称（例如 `"shell"`、`"read_file"`）。
        tool_name: String,
        /// 工具执行的当前阶段（例如 `"start"`、`"end"`）。
        phase: String,
        /// 与此阶段关联的任意结构化负载。
        payload: Value,
    },
    /// 后台作业已转换到新阶段。
    JobLifecycle {
        /// 作业的唯一标识符。
        job_id: String,
        /// 作业的当前阶段（例如 `"queued"`、`"running"`、`"done"`）。
        phase: String,
        /// 可选的进度百分比（0-100）。
        progress: Option<u8>,
        /// 关于当前阶段的可读详情。
        detail: Option<String>,
    },
    /// 审批请求已转换到新阶段。
    ApprovalLifecycle {
        /// 审批请求的唯一标识符。
        approval_id: String,
        /// 当前阶段（例如 `"requested"`、`"approved"`、`"denied"`）。
        phase: String,
        /// 解释当前阶段的可选原因。
        reason: Option<String>,
    },
    /// 包装任意 [`EventFrame`] 的兜底变体。
    ///
    /// 当你需要转发协议级别的事件帧而无需
    /// 将其映射到更具体的变体时使用。
    GenericEventFrame {
        /// 要转发的原始事件帧。
        frame: Box<EventFrame>,
    },
}

impl HookEvent {
    /// 将此事件序列化为 [`serde_json::Value`]。
    ///
    /// 返回包含 `"type"` 鉴别器和所有变体字段的 JSON 对象。
    /// 如果序列化失败（这应该极为罕见），则返回
    /// 回退值 `{"type":"serialization_error"}` 而非 panic。
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({"type":"serialization_error"}))
    }
}

/// 可以接收 [`HookEvent`] 的目标。
///
/// 实现者处理交付事件的传输特定细节
///（写入 stdout、追加到文件、POST 到 webhook 等）。
/// [`HookDispatcher`] 将每个事件分发到所有注册的接收端，因此
/// 单个进程可以同时记录到多个目标。
///
/// 接收端应是**尽力而为**的：实现应避免
/// panic，并应仅在真正意外的失败时返回 [`anyhow::Error`]。
/// [`HookDispatcher::emit`] 丢弃单个接收端的错误，因此钩子
/// 交付失败不会中止应用程序。
#[async_trait]
pub trait HookSink: Send + Sync {
    /// 将单个事件投递到此接收端。
    ///
    /// 实现应对瞬时故障（例如缺少监听器）具有弹性，
    /// 并且不应长时间阻塞调用者。
    async fn emit(&self, event: &HookEvent) -> Result<()>;
}

/// 一个将每个事件作为单行 JSON 打印到 stdout 的 [`HookSink`]。
///
/// 适用于本地开发和调试。事件通过 [`println!`] 打印，
/// 因此它们会与其他程序输出交错显示。
#[derive(Default)]
pub struct StdoutHookSink;

#[async_trait]
impl HookSink for StdoutHookSink {
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        println!("{}", event.to_json());
        Ok(())
    }
}

/// 一个将每个事件作为 JSON 行追加到文件的 [`HookSink`]。
///
/// 文件（以及任何缺失的父目录）在首次发出事件时创建。
/// 每行是一个 JSON 对象，格式为
/// `{"at": "<ISO 8601 timestamp>", "event": {...}}`。
pub struct JsonlHookSink {
    path: PathBuf,
}

impl JsonlHookSink {
    /// 创建一个写入 `path` 文件的新接收端。
    ///
    /// 父目录在首次 [`HookSink::emit`] 调用时延迟创建。
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl HookSink for JsonlHookSink {
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create hook log directory {}", parent.display())
            })?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("failed to open hook log {}", self.path.display()))?;
        let payload = json!({
            "at": Utc::now().to_rfc3339(),
            "event": event
        });
        let encoded = serde_json::to_string(&payload).context("failed to encode hook event")?;
        file.write_all(encoded.as_bytes())
            .await
            .context("failed to write hook event")?;
        file.write_all(b"\n")
            .await
            .context("failed to write hook event newline")?;
        // 在 drop 之前刷新，以便顺序发出的事件（以及立即读取文件
        // 的测试）能观察到每行完整写入。
        file.flush().await.context("failed to flush hook event")?;
        Ok(())
    }
}

/// 一个将每个事件以 JSON 格式 POST 到远程 HTTP 端点的 [`HookSink`]。
///
/// 请求体为 `{"at": "<ISO 8601 timestamp>", "event": {...}}`。
/// 失败的请求最多重试 2 次，采用指数退避
///（200 毫秒、400 毫秒）。耗尽重试次数后，传播错误。
pub struct WebhookHookSink {
    url: String,
    client: reqwest::Client,
}

impl WebhookHookSink {
    /// 创建一个将事件发送到给定 `url` 的新接收端。
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: codewhale_release::platform_http_client_builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| {
                    codewhale_release::platform_http_client_builder()
                        .build()
                        .expect("build fallback HTTP client")
                }),
        }
    }
}

#[async_trait]
impl HookSink for WebhookHookSink {
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        let mut retries = 0usize;
        loop {
            let resp = self
                .client
                .post(&self.url)
                .json(&json!({
                    "at": Utc::now().to_rfc3339(),
                    "event": event,
                }))
                .send()
                .await;
            match resp {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    if retries >= 2 {
                        anyhow::bail!("webhook returned non-success status {}", response.status());
                    }
                }
                Err(err) => {
                    if retries >= 2 {
                        return Err(err).context("webhook request failed");
                    }
                }
            }
            retries += 1;
            tokio::time::sleep(std::time::Duration::from_millis(200 * retries as u64)).await;
        }
    }
}

/// 一个通过 Unix 域套接字发送事件的 [`HookSink`]。
///
/// 每个事件被序列化为单行 JSON（`{"at": "...", "event": {...}}\n`）
/// 并写入套接字。如果套接字不可用（监听器未运行），
/// 事件被静默丢弃——钩子接收端是尽力而为的可观测性，而非
/// 控制流。
///
/// 在非 Unix 平台上，此结构体存在，但其 [`HookSink::emit`] 为空操作。
#[derive(Debug, Clone)]
pub struct UnixSocketHookSink {
    #[cfg(unix)]
    path: PathBuf,
}

impl UnixSocketHookSink {
    /// 创建一个连接到 `path` 处 Unix 域套接字的接收端。
    pub fn new(path: PathBuf) -> Self {
        #[cfg(unix)]
        {
            Self { path }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Self {}
        }
    }
}

#[async_trait]
impl HookSink for UnixSocketHookSink {
    #[cfg(unix)]
    async fn emit(&self, event: &HookEvent) -> Result<()> {
        let mut stream = match tokio::net::UnixStream::connect(&self.path).await {
            Ok(s) => s,
            Err(_) => return Ok(()), // 监听器未运行，静默跳过
        };
        let payload = json!({
            "at": Utc::now().to_rfc3339(),
            "event": event
        });
        let mut line = serde_json::to_string(&payload).context("failed to encode hook event")?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .await
            .context("failed to write to unix socket")?;
        Ok(())
    }

    #[cfg(not(unix))]
    async fn emit(&self, _event: &HookEvent) -> Result<()> {
        // 此平台上不支持 Unix 套接字。
        Ok(())
    }
}

/// 将 [`HookEvent`] 分发到一组 [`HookSink`]。
///
/// 通过 [`add_sink`](HookDispatcher::add_sink) 注册一个或多个接收端，
/// 然后调用 [`emit`](HookDispatcher::emit) 向所有接收端广播事件。
/// 如果某个接收端返回错误，该错误会被静默忽略，这样
/// 一个失败的接收端不会阻止其余接收端接收事件。
#[derive(Default, Clone)]
pub struct HookDispatcher {
    sinks: Vec<Arc<dyn HookSink>>,
}

impl HookDispatcher {
    /// 注册一个新的接收端，它将接收之后发出的所有事件。
    pub fn add_sink(&mut self, sink: Arc<dyn HookSink>) {
        self.sinks.push(sink);
    }

    /// 向每个注册的接收端广播事件。
    ///
    /// 来自单个接收端的错误会被静默丢弃，这样
    /// 一个失败的接收端不会阻塞其他接收端。
    pub async fn emit(&self, event: HookEvent) {
        for sink in &self.sinks {
            let _ = sink.emit(&event).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn hook_event_serializes_with_snake_case_type_and_payload() {
        let event = HookEvent::ToolLifecycle {
            response_id: "resp-1".to_string(),
            tool_name: "shell".to_string(),
            phase: "end".to_string(),
            payload: json!({ "exit_code": 0 }),
        };

        let encoded = event.to_json();

        assert_eq!(encoded["type"], "tool_lifecycle");
        assert_eq!(encoded["response_id"], "resp-1");
        assert_eq!(encoded["tool_name"], "shell");
        assert_eq!(encoded["phase"], "end");
        assert_eq!(encoded["payload"]["exit_code"], 0);
    }

    #[test]
    fn generic_event_frame_serialization_is_unchanged_by_boxing() {
        let event = HookEvent::GenericEventFrame {
            frame: Box::new(EventFrame::ResponseStart {
                response_id: "resp-1".to_string(),
            }),
        };

        let encoded = event.to_json();

        assert_eq!(encoded["type"], "generic_event_frame");
        assert_eq!(encoded["frame"]["event"], "response_start");
        assert_eq!(encoded["frame"]["response_id"], "resp-1");
    }

    #[tokio::test]
    async fn jsonl_sink_creates_parent_dir_and_appends_events() {
        let root = unique_temp_dir("jsonl_sink");
        let path = root.join("nested").join("hooks.jsonl");
        let sink = JsonlHookSink::new(path.clone());

        sink.emit(&HookEvent::ResponseStart {
            response_id: "resp-1".to_string(),
        })
        .await
        .unwrap();
        sink.emit(&HookEvent::ResponseEnd {
            response_id: "resp-1".to_string(),
        })
        .await
        .unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);

        let first: Value = serde_json::from_str(lines[0]).unwrap();
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert!(first["at"].as_str().is_some());
        assert_eq!(first["event"]["type"], "response_start");
        assert_eq!(first["event"]["response_id"], "resp-1");
        assert_eq!(second["event"]["type"], "response_end");
        assert_eq!(second["event"]["response_id"], "resp-1");

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dispatcher_continues_after_sink_error() {
        let mut dispatcher = HookDispatcher::default();
        let first = Arc::new(RecordingSink::default());
        let second = Arc::new(RecordingSink::default());

        dispatcher.add_sink(first.clone());
        dispatcher.add_sink(Arc::new(FailingSink));
        dispatcher.add_sink(second.clone());

        dispatcher
            .emit(HookEvent::ApprovalLifecycle {
                approval_id: "approval-1".to_string(),
                phase: "requested".to_string(),
                reason: Some("needs review".to_string()),
            })
            .await;

        assert_eq!(
            first.events(),
            vec![json!({
                "type": "approval_lifecycle",
                "approval_id": "approval-1",
                "phase": "requested",
                "reason": "needs review",
            })]
        );
        assert_eq!(second.events(), first.events());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_sink_skips_when_listener_absent() {
        let (root, socket_path) = unique_short_socket_path("missing");
        let sink = UnixSocketHookSink::new(socket_path);
        let result = sink
            .emit(&HookEvent::ResponseStart {
                response_id: "resp-1".to_string(),
            })
            .await;
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_sink_sends_event_to_listener() {
        use tokio::io::AsyncBufReadExt;
        use tokio::net::UnixListener;

        let (root, socket_path) = unique_short_socket_path("send");
        std::fs::create_dir_all(&root).expect("mkdir");
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path).expect("bind");
        let sink = UnixSocketHookSink::new(socket_path.clone());

        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut reader = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read_line");
            line
        });

        sink.emit(&HookEvent::ResponseStart {
            response_id: "resp-42".to_string(),
        })
        .await
        .expect("emit");

        let received = handle.await.expect("join");
        let parsed: Value = serde_json::from_str(&received).expect("parse");
        assert_eq!(parsed["event"]["type"], "response_start");
        assert_eq!(parsed["event"]["response_id"], "resp-42");
        assert!(parsed["at"].as_str().is_some());

        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<Value>>,
    }

    impl RecordingSink {
        fn events(&self) -> Vec<Value> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HookSink for RecordingSink {
        async fn emit(&self, event: &HookEvent) -> Result<()> {
            self.events.lock().unwrap().push(event.to_json());
            Ok(())
        }
    }

    struct FailingSink;

    #[async_trait::async_trait]
    impl HookSink for FailingSink {
        async fn emit(&self, _event: &HookEvent) -> Result<()> {
            anyhow::bail!("sink failed")
        }
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "deepseek-hooks-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    fn unique_short_socket_path(label: &str) -> (PathBuf, PathBuf) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PathBuf::from("/tmp").join(format!("cw-hk-{}-{nanos}", std::process::id()));
        let path = root.join(format!("{label}.sock"));
        (root, path)
    }
}
