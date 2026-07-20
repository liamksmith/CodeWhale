//! `MockLlmClient` — 用于测试的队列驱动 `LlmClient` 实现。
//!
//! 此客户端通过重放预加载的预设响应队列（每轮一个）来实现
//! [`LlmClient`](super::LlmClient) trait。它捕获运行时分发的每个请求，
//! 以便测试可以断言出站负载——
//! 例如，确认之前的 `reasoning_content` 在 DeepSeek V4
//! 思考模式工具调用轮次中被重放（V4 §5.1.1；该 bug 破坏了
//! v0.4.9-v0.5.1）。
//!
//! # 模拟策略
//!
//! 测试在 **trait 边界**（`LlmClient`）处进行模拟，绝对不会在 `reqwest`
//! HTTP 层进行模拟。Trait 是持久的抽象——内部 HTTP 管道
//! 经常变更，并且不属于公共引擎契约的一部分。
//!
//! # 示例
//!
//! ```ignore
//! use crate::llm_client::mock::{MockLlmClient, canned};
//! use crate::llm_client::LlmClient;
//!
//! // 一个预设轮次，以两个文本 delta 发出 "hello world"，然后
//! // 以 stop_reason = "end_turn" 结束。
//! let turn = vec![
//!     canned::message_start("msg_1"),
//!     canned::text_delta(0, "hello "),
//!     canned::text_delta(0, "world"),
//!     canned::message_stop(),
//! ];
//!
//! let mock = MockLlmClient::new(vec![turn]);
//! let stream = mock.create_message_stream(/* ... */).await.unwrap();
//! // ... 消费流，断言 delta ...
//! assert_eq!(mock.call_count(), 1);
//! assert_eq!(mock.captured_requests().len(), 1);
//! ```

// 此模块提供了集成测试依赖的方法和构建器辅助函数。
// 并非每个辅助函数都会被单元测试调用——这是预期的
//（目标是下游测试的可用模拟表面），因此我们在模块级别
// 静默逐项死代码警告。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, anyhow};
use async_stream::try_stream;
use futures_util::Stream;

use crate::models::{
    ContentBlock, MessageDelta, MessageRequest, MessageResponse, StreamEvent, Usage,
};

use super::{LlmClient, StreamEventBox};

/// mock 将在下一次流式调用中重放的预先录制的"轮次"。
///
/// `MessageStop` *不*需要在最终元素——如果缺失，mock 将自动
/// 发出一个，镜像真实客户端的行为。同样，mock 不要求
/// `MessageStart` 存在。
pub type CannedTurn = Vec<StreamEvent>;

/// 排队的模拟响应步骤。
pub enum FauxStep {
    Canned(CannedTurn),
    /// 从实时的出站请求构建预设轮次。
    ///
    /// 测试可以在此处断言 DeepSeek V4 的思考模式工具调用不变量：
    /// 在产生前一个工具调用的助手轮次上，下一次出站请求
    /// 必须仍然携带 `reasoning_content`（在此模型中表示为
    /// [`ContentBlock::Thinking`] 块）。如果缺失，DeepSeek V4
    /// 在后续轮次上返回 HTTP 400。这守护了
    /// [v0.4.9-v0.5.1 回归范围](https://github.com/Hmbown/CodeWhale/compare/v0.4.9...v0.5.1)
    /// 中该内容被丢弃的问题。
    Factory(Box<dyn Fn(&MessageRequest) -> CannedTurn + Send + Sync>),
}

/// 一个队列驱动的 mock LLM 客户端。
///
/// mock 持有一个 FIFO 预设响应轮次队列。每次调用
/// [`LlmClient::create_message_stream`] 就会出队下一个轮次并将其事件
/// 作为流重放。如果队列耗尽，调用返回错误
/// ——测试应确保它们推送的轮次数量正好等于运行时要消费的
/// 数量。
///
/// mock 还捕获传递给每次调用的 [`MessageRequest`]，以便测试
/// 可以断言出站负载（例如，之前的 `reasoning_content` 是否在
/// 各轮次之间得到保留）。
pub struct MockLlmClient {
    canned: Mutex<VecDeque<FauxStep>>,
    captured_requests: Mutex<Vec<MessageRequest>>,
    calls: AtomicUsize,
    provider_name: &'static str,
    model: String,
    /// 如果设置，[`LlmClient::create_message`] 原样返回此值。否则
    /// 回退到流式 + 收集。对于非流式的压缩风格调用很有用。
    canned_messages: Mutex<VecDeque<MessageResponse>>,
}

impl MockLlmClient {
    /// 构造一个将按顺序重放给定预设轮次的 mock。
    #[must_use]
    pub fn new(canned: Vec<CannedTurn>) -> Self {
        Self {
            canned: Mutex::new(canned.into_iter().map(FauxStep::Canned).collect()),
            captured_requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            provider_name: "mock",
            model: "mock-model".to_string(),
            canned_messages: Mutex::new(VecDeque::new()),
        }
    }

    /// 设置 [`LlmClient::provider_name`] 返回的提供者名称字符串。
    #[must_use]
    pub fn with_provider(mut self, name: &'static str) -> Self {
        self.provider_name = name;
        self
    }

    /// 设置 [`LlmClient::model`] 返回的模型标识符。
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 将预设轮次推入队列尾部。
    pub fn push_turn(&self, turn: CannedTurn) {
        self.canned
            .lock()
            .expect("MockLlmClient.canned mutex poisoned")
            .push_back(FauxStep::Canned(turn));
    }

    /// 将工厂步骤推入队列尾部。
    ///
    /// 闭包在构建响应流之前接收实时的出站 [`MessageRequest`]，
    /// 因此断言会直接从客户端调用中 panic，而不是稍后
    /// 在轮询返回的流时 panic。
    pub fn push_factory<F>(&self, factory: F)
    where
        F: Fn(&MessageRequest) -> CannedTurn + Send + Sync + 'static,
    {
        self.canned
            .lock()
            .expect("MockLlmClient.canned mutex poisoned")
            .push_back(FauxStep::Factory(Box::new(factory)));
    }

    /// 推送预设的非流式 `MessageResponse`。由
    /// [`LlmClient::create_message`] 消费（FIFO）。
    pub fn push_message_response(&self, response: MessageResponse) {
        self.canned_messages
            .lock()
            .expect("MockLlmClient.canned_messages mutex poisoned")
            .push_back(response);
    }

    /// 已完成的对 `create_message` 或 `create_message_stream` 的调用次数。
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// 仍在队列中的预设轮次数量。
    #[must_use]
    pub fn remaining_turns(&self) -> usize {
        self.canned
            .lock()
            .expect("MockLlmClient.canned mutex poisoned")
            .len()
    }

    /// mock 已被要求处理的每个请求的快照，按顺序排列。
    #[must_use]
    pub fn captured_requests(&self) -> Vec<MessageRequest> {
        self.captured_requests
            .lock()
            .expect("MockLlmClient.captured_requests mutex poisoned")
            .clone()
    }

    /// 便捷方法：返回最近捕获的请求，如果 mock 尚未被调用则返回 `None`。
    #[must_use]
    pub fn last_request(&self) -> Option<MessageRequest> {
        self.captured_requests
            .lock()
            .expect("MockLlmClient.captured_requests mutex poisoned")
            .last()
            .cloned()
    }

    fn record_request(&self, request: &MessageRequest) {
        self.captured_requests
            .lock()
            .expect("MockLlmClient.captured_requests mutex poisoned")
            .push(request.clone());
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    fn pop_step(&self) -> Option<FauxStep> {
        self.canned
            .lock()
            .expect("MockLlmClient.canned mutex poisoned")
            .pop_front()
    }

    fn turn_from_step(&self, step: FauxStep, request: &MessageRequest) -> CannedTurn {
        match step {
            FauxStep::Canned(turn) => turn,
            FauxStep::Factory(factory) => factory(request),
        }
    }

    fn pop_message(&self) -> Option<MessageResponse> {
        self.canned_messages
            .lock()
            .expect("MockLlmClient.canned_messages mutex poisoned")
            .pop_front()
    }
}

impl LlmClient for MockLlmClient {
    fn provider_name(&self) -> &'static str {
        self.provider_name
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn create_message(&self, request: MessageRequest) -> Result<MessageResponse> {
        self.record_request(&request);

        if let Some(canned) = self.pop_message() {
            return Ok(canned);
        }

        // 回退：从下一个流式轮次合成 MessageResponse。
        let Some(step) = self.pop_step() else {
            return Err(anyhow!(
                "MockLlmClient: create_message called but no canned response queued (request #{})",
                self.calls.load(Ordering::SeqCst)
            ));
        };

        let turn = self.turn_from_step(step, &request);
        Ok(synthesize_message_response(turn, &self.model))
    }

    async fn create_message_stream(&self, request: MessageRequest) -> Result<StreamEventBox> {
        self.record_request(&request);

        let Some(step) = self.pop_step() else {
            return Err(anyhow!(
                "MockLlmClient: create_message_stream called but no canned turn queued (call #{})",
                self.calls.load(Ordering::SeqCst)
            ));
        };

        let turn = self.turn_from_step(step, &request);
        Ok(stream_from_canned(turn))
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }
}

/// 将预设事件向量包装为按顺序产生每个事件并自动追加 `MessageStop`
/// 的流（如果尾部事件还不是 `MessageStop`）。
fn stream_from_canned(turn: CannedTurn) -> StreamEventBox {
    let s = try_stream! {
        let has_stop = matches!(turn.last(), Some(StreamEvent::MessageStop));
        for ev in turn {
            yield ev;
        }
        if !has_stop {
            yield StreamEvent::MessageStop;
        }
    };
    Box::pin(s) as Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send + 'static>>
}

/// 尽力而为：通过连接文本 delta 将流式轮次折叠为非流式
/// `MessageResponse`。仅在调用方在未排队 `MessageResponse` 时
/// 调用 `create_message` 时作为回退使用。
fn synthesize_message_response(turn: CannedTurn, model: &str) -> MessageResponse {
    use crate::models::Delta;

    let mut text = String::new();
    let mut stop_reason: Option<String> = None;

    for ev in turn {
        match ev {
            StreamEvent::ContentBlockDelta {
                delta: Delta::TextDelta { text: t },
                ..
            } => text.push_str(&t),
            StreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: sr, ..
                },
                ..
            } => stop_reason = sr,
            _ => {}
        }
    }

    MessageResponse {
        id: "mock_msg".to_string(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
        model: model.to_string(),
        stop_reason: stop_reason.or_else(|| Some("end_turn".to_string())),
        stop_sequence: None,
        container: None,
        usage: Usage::default(),
    }
}

/// 常见预设事件模式的构建器。重新导出，以便测试可以构建
/// 真实流而无需手动编写 `StreamEvent` 结构。
pub mod canned {
    use serde_json::Value;

    use crate::models::{
        ContentBlockStart, Delta, MessageDelta, MessageResponse, StreamEvent, Usage,
    };

    /// `MessageStart` 事件，带有合成消息信封。
    #[must_use]
    pub fn message_start(id: &str) -> StreamEvent {
        StreamEvent::MessageStart {
            message: MessageResponse {
                id: id.to_string(),
                r#type: "message".to_string(),
                role: "assistant".to_string(),
                content: vec![],
                model: "mock-model".to_string(),
                stop_reason: None,
                stop_sequence: None,
                container: None,
                usage: Usage::default(),
            },
        }
    }

    /// 在 `index` 处打开一个文本内容块。
    #[must_use]
    pub fn text_block_start(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockStart::Text {
                text: String::new(),
            },
        }
    }

    /// 将 `text` 追加到 `index` 处的内容块。
    #[must_use]
    pub fn text_delta(index: u32, text: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index,
            delta: Delta::TextDelta {
                text: text.to_string(),
            },
        }
    }

    /// 在 `index` 处追加一个思考内容 delta。
    #[must_use]
    pub fn thinking_delta(index: u32, thinking: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index,
            delta: Delta::ThinkingDelta {
                thinking: thinking.to_string(),
            },
        }
    }

    /// 在 `index` 处打开一个 tool_use 内容块。
    #[must_use]
    pub fn tool_use_block_start(index: u32, id: &str, name: &str) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index,
            content_block: ContentBlockStart::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: Value::Null,
                caller: None,
            },
        }
    }

    /// 流式传输工具输入参数的 JSON 片段。
    #[must_use]
    pub fn tool_input_delta(index: u32, partial_json: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta {
            index,
            delta: Delta::InputJsonDelta {
                partial_json: partial_json.to_string(),
            },
        }
    }

    /// 关闭 `index` 处的内容块。
    #[must_use]
    pub fn block_stop(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStop { index }
    }

    /// 发出携带 `stop_reason` 和可选 `usage` 的 `message_delta`。
    #[must_use]
    pub fn message_delta(stop_reason: &str, usage: Option<Usage>) -> StreamEvent {
        StreamEvent::MessageDelta {
            delta: MessageDelta {
                stop_reason: Some(stop_reason.to_string()),
                stop_sequence: None,
            },
            usage,
        }
    }

    /// 最终的 `message_stop` 哨兵。
    #[must_use]
    pub fn message_stop() -> StreamEvent {
        StreamEvent::MessageStop
    }

    /// 便捷方法：一个完整的"助手发出此文本"轮次，以
    /// `stop_reason = "end_turn"` 结束。
    #[must_use]
    pub fn simple_text_turn(text: &str) -> Vec<StreamEvent> {
        vec![
            message_start("mock_msg_1"),
            text_block_start(0),
            text_delta(0, text),
            block_stop(0),
            message_delta("end_turn", None),
            message_stop(),
        ]
    }

    /// 便捷方法：一个助手发出一次 tool_call 然后停止的轮次。
    #[must_use]
    pub fn tool_call_turn(call_id: &str, tool_name: &str, args_json: &str) -> Vec<StreamEvent> {
        vec![
            message_start("mock_msg_tool"),
            tool_use_block_start(0, call_id, tool_name),
            tool_input_delta(0, args_json),
            block_stop(0),
            message_delta("tool_use", None),
            message_stop(),
        ]
    }
}

// === 测试 ===

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;
    use crate::llm_client::LlmClient;
    use crate::models::{Delta, Message, MessageRequest, StreamEvent};

    fn empty_request() -> MessageRequest {
        MessageRequest {
            model: "mock-model".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![],
            }],
            max_tokens: 1024,
            system: None,
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(true),
            temperature: None,
            top_p: None,
        }
    }

    #[tokio::test]
    async fn replays_canned_turn_via_stream() {
        let mock = MockLlmClient::new(vec![canned::simple_text_turn("hello world")]);

        let mut stream = mock
            .create_message_stream(empty_request())
            .await
            .expect("stream should open");

        let mut text = String::new();
        let mut saw_stop = false;
        while let Some(ev) = stream.next().await {
            match ev.expect("event") {
                StreamEvent::ContentBlockDelta {
                    delta: Delta::TextDelta { text: t },
                    ..
                } => text.push_str(&t),
                StreamEvent::MessageStop => {
                    saw_stop = true;
                    break;
                }
                _ => {}
            }
        }

        assert_eq!(text, "hello world");
        assert!(saw_stop);
        assert_eq!(mock.call_count(), 1);
        assert_eq!(mock.captured_requests().len(), 1);
        assert_eq!(mock.remaining_turns(), 0);
    }

    #[tokio::test]
    async fn errors_when_queue_exhausted() {
        let mock = MockLlmClient::new(Vec::new());
        let result = mock.create_message_stream(empty_request()).await;
        match result {
            Ok(_) => panic!("should error on empty queue"),
            Err(err) => assert!(format!("{err}").contains("no canned")),
        }
    }

    #[tokio::test]
    async fn captures_request_payload_for_assertions() {
        let mock = MockLlmClient::new(vec![canned::simple_text_turn("ok")]);
        let mut req = empty_request();
        req.temperature = Some(0.42);
        let _ = mock.create_message_stream(req).await.unwrap();

        let captured = mock.last_request().expect("should have captured");
        assert_eq!(captured.temperature, Some(0.42));
    }

    #[tokio::test]
    async fn stream_auto_appends_message_stop() {
        // 排队一个缺少 MessageStop 的轮次——mock 应追加一个。
        let turn = vec![canned::text_block_start(0), canned::text_delta(0, "x")];
        let mock = MockLlmClient::new(vec![turn]);

        let mut stream = mock.create_message_stream(empty_request()).await.unwrap();
        let mut saw_stop = false;
        while let Some(ev) = stream.next().await {
            if matches!(ev.expect("event"), StreamEvent::MessageStop) {
                saw_stop = true;
            }
        }
        assert!(saw_stop, "auto MessageStop missing");
    }

    #[tokio::test]
    async fn create_message_uses_canned_message_response_first() {
        let mock = MockLlmClient::new(vec![canned::simple_text_turn("from stream")]);
        mock.push_message_response(MessageResponse {
            id: "preset".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "from preset".to_string(),
                cache_control: None,
            }],
            model: "mock-model".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            container: None,
            usage: Usage::default(),
        });

        let resp = mock.create_message(empty_request()).await.unwrap();
        assert_eq!(resp.id, "preset");
    }

    #[tokio::test]
    async fn create_message_synthesizes_from_streaming_turn_when_no_message_queued() {
        let mock = MockLlmClient::new(vec![canned::simple_text_turn("synthesized")]);
        let resp = mock.create_message(empty_request()).await.unwrap();
        let text = match &resp.content[0] {
            ContentBlock::Text { text, .. } => text.clone(),
            _ => panic!("expected text"),
        };
        assert_eq!(text, "synthesized");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
    }

    #[tokio::test]
    async fn create_message_synthesizes_from_factory_turn() {
        let mock = MockLlmClient::new(Vec::new());
        mock.push_factory(|request| {
            assert_eq!(request.model, "mock-model");
            canned::simple_text_turn("from factory")
        });

        let resp = mock.create_message(empty_request()).await.unwrap();
        let text = match &resp.content[0] {
            ContentBlock::Text { text, .. } => text.clone(),
            _ => panic!("expected text"),
        };
        assert_eq!(text, "from factory");
    }

    #[tokio::test]
    async fn provider_and_model_are_overridable() {
        let mock = MockLlmClient::new(vec![canned::simple_text_turn("x")])
            .with_provider("test-provider")
            .with_model("test-model");
        assert_eq!(mock.provider_name(), "test-provider");
        assert_eq!(mock.model(), "test-model");
    }

    #[tokio::test]
    async fn tool_call_turn_serializes_correctly() {
        let mock = MockLlmClient::new(vec![canned::tool_call_turn(
            "call_1",
            "list_dir",
            r#"{"path":"/tmp"}"#,
        )]);
        let mut stream = mock.create_message_stream(empty_request()).await.unwrap();

        let mut saw_tool_use = false;
        let mut json_seen = String::new();
        while let Some(ev) = stream.next().await {
            match ev.unwrap() {
                StreamEvent::ContentBlockStart { content_block, .. } => {
                    use crate::models::ContentBlockStart;
                    if let ContentBlockStart::ToolUse { name, .. } = content_block {
                        assert_eq!(name, "list_dir");
                        saw_tool_use = true;
                    }
                }
                StreamEvent::ContentBlockDelta {
                    delta: Delta::InputJsonDelta { partial_json },
                    ..
                } => json_seen.push_str(&partial_json),
                _ => {}
            }
        }
        assert!(saw_tool_use, "expected tool_use start event");
        assert!(json_seen.contains("/tmp"));
    }

    #[tokio::test]
    async fn multiple_turns_consumed_in_order() {
        let mock = MockLlmClient::new(vec![
            canned::simple_text_turn("turn-one"),
            canned::simple_text_turn("turn-two"),
        ]);
        for expected in ["turn-one", "turn-two"] {
            let mut stream = mock.create_message_stream(empty_request()).await.unwrap();
            let mut text = String::new();
            while let Some(ev) = stream.next().await {
                if let StreamEvent::ContentBlockDelta {
                    delta: Delta::TextDelta { text: t },
                    ..
                } = ev.unwrap()
                {
                    text.push_str(&t);
                }
            }
            assert_eq!(text, expected);
        }
        assert_eq!(mock.call_count(), 2);
        assert_eq!(mock.remaining_turns(), 0);
    }
}
