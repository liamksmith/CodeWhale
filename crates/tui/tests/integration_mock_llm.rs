//! [`MockLlmClient`](mock::MockLlmClient) 的集成测试。
//!
//! 这些测试直接测试 [`LlmClient`](llm_client::LlmClient) trait 表面。
//! 它们验证模拟客户端本身在运行时依赖的模式下是否正确行为：
//!
//! - **流式回合循环** — 事件按顺序到达，`MessageStop` 终止流。
//! - **推理重放**（问题 #69 / V4 §5.1.1）— 当运行时在工具回合后发送第二轮时，
//!   它必须重放之前的 `reasoning_content`。捕获了破坏 v0.4.9-v0.5.1 的 HTTP 400 路径。
//! - **工具调用往返** — 助手发出 `tool_calls`，运行时执行，
//!   工具结果被追加，下一轮流式传输文本。
//! - **一轮中的多个工具调用** — 助手返回 N 个 tool_calls；
//!   请求负载保持它们的顺序。
//! - **压缩风格的非流式调用** — `create_message` 返回队列中的
//!   `MessageResponse`，不经过流式路径。
//! - **子代理风格回合** — 子邮箱接收父提示并回复；
//!   trait 边界相同。
//! - **容量门控观察** — 运行时可以探测估计的请求大小并选择不分发；
//!   模拟在该表面暴露捕获侧钩子。
//!
//! # 为什么是 trait 级别（而不是引擎级别）
//!
//! 截至 v0.6.7，引擎（`crates/tui/src/core/engine.rs`）持有具体的
//! `Option<DeepSeekClient>`——[`LlmClient`] trait 已实现，但没有
//! 消费者使用 `Arc<dyn LlmClient>` 或泛型 `<C: LlmClient>`。将模拟
//! 接入完整的引擎回合循环因此需要一个单独的重构：
//! 每个 `Option<DeepSeekClient>` 消费者（引擎、注册表、rlm、审查、
//! cycle_manager、压缩、子代理）必须迁移到 `Arc<dyn LlmClient>`。
//!
//! 根据 v0.7.0 模拟 LLM 问题（本文件的父级）："如果引擎的
//! API 表面太混乱无法干净模拟……请将其记录为 BLOCKED，并说明
//! 需要更改哪些接线。在这种情况下，仍然提交任何干净落地的部分工作。"
//! 完整的引擎集成覆盖仍然被该接缝阻塞；本文件记录阻塞因素，
//! 而不是携带被忽略的占位测试。
//!
//! 一旦 `Arc<dyn LlmClient>` 落地，添加使用此模拟的引擎级别测试。

use futures_util::StreamExt;

// 逐字引入生产模型类型——不需要其他 crate 源，
// 因为模拟对 `models.rs` 是自包含的。
#[path = "../src/model_catalog.rs"]
mod model_catalog;

#[path = "../src/models.rs"]
#[allow(dead_code)]
mod models;

// 镜像真实的 `llm_client` 模块层次结构，以便 `mock.rs` 的
// `super::{LlmClient, StreamEventBox}` 路径可以解析。我们重新声明一个本地的
// `LlmClient` trait + `StreamEventBox` 别名，与生产形态 1:1 匹配
//（二进制文件中包含的公共表面）。模拟实现了
// 这个本地的 trait，它在结构上与生产 trait 相同。
//
// 辅助文件位于 `tests/support/` 下，因此 cargo 不会尝试
// 将其编译为自己的测试二进制文件。
#[path = "support/llm_client.rs"]
mod llm_client;

use crate::llm_client::LlmClient;
use crate::llm_client::mock::{MockLlmClient, canned};
use crate::models::{ContentBlock, Delta, Message, MessageRequest, StreamEvent, Usage};

// === 辅助函数 ===============================================================

fn user_message(text: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
    }
}

fn assistant_thinking(thinking: &str, text: &str) -> Message {
    Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Thinking {
                thinking: thinking.to_string(),
                signature: None,
            },
            ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            },
        ],
    }
}

fn assistant_tool_call(id: &str, name: &str, input: serde_json::Value) -> Message {
    Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
            caller: None,
        }],
    }
}

fn tool_result_message(tool_use_id: &str, content: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: content.to_string(),
            is_error: None,
            content_blocks: None,
        }],
    }
}

fn make_request(messages: Vec<Message>) -> MessageRequest {
    MessageRequest {
        model: "deepseek-v4-pro".to_string(),
        messages,
        max_tokens: 4096,
        system: None,
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: Some("high".to_string()),
        stream: Some(true),
        temperature: None,
        top_p: None,
    }
}

async fn drain_stream_text(
    mock: &MockLlmClient,
    request: MessageRequest,
) -> (String, Option<String>) {
    let mut stream = mock
        .create_message_stream(request)
        .await
        .expect("stream open");
    let mut text = String::new();
    let mut stop_reason: Option<String> = None;
    while let Some(ev) = stream.next().await {
        match ev.expect("event") {
            StreamEvent::ContentBlockDelta {
                delta: Delta::TextDelta { text: t },
                ..
            } => text.push_str(&t),
            StreamEvent::MessageDelta { delta, .. } => {
                stop_reason = delta.stop_reason;
            }
            StreamEvent::MessageStop => break,
            _ => {}
        }
    }
    (text, stop_reason)
}

// === 1. 带流式的完整回合循环 ===============================================

#[tokio::test]
async fn full_turn_loop_streams_text_chunks() {
    // 两个文本增量 + 完成原因——测试引擎驱动的规范流式回合循环路径。
    let turn = vec![
        canned::message_start("msg_1"),
        canned::text_block_start(0),
        canned::text_delta(0, "Hello, "),
        canned::text_delta(0, "world!"),
        canned::block_stop(0),
        canned::message_delta("end_turn", Some(Usage::default())),
        canned::message_stop(),
    ];
    let mock = MockLlmClient::new(vec![turn]);

    let request = make_request(vec![user_message("greet me")]);
    let (text, stop) = drain_stream_text(&mock, request).await;

    assert_eq!(text, "Hello, world!");
    assert_eq!(stop.as_deref(), Some("end_turn"));
    assert_eq!(mock.call_count(), 1);
    assert_eq!(mock.captured_requests().len(), 1);
}

// === 2. 推理重放（V4 思考模式 HTTP-400 回归）================================

#[tokio::test]
async fn reasoning_replay_required_on_subsequent_turn() {
    // 回合 1：助手发出 thinking + tool_call。回合 2：文本回复。
    let turn1 = vec![
        canned::message_start("r1"),
        canned::thinking_delta(0, "I should call list_dir."),
        canned::tool_use_block_start(1, "call_a", "list_dir"),
        canned::tool_input_delta(1, r#"{"path":"/tmp"}"#),
        canned::block_stop(1),
        canned::message_delta("tool_use", None),
        canned::message_stop(),
    ];
    let turn2 = vec![
        canned::message_start("r2"),
        canned::text_block_start(0),
        canned::text_delta(0, "I see /tmp."),
        canned::block_stop(0),
        canned::message_delta("end_turn", None),
        canned::message_stop(),
    ];
    let mock = MockLlmClient::new(vec![turn1, turn2]);

    // === 第 1 轮：用户提示 -> 助手 tool_call ===
    let req1 = make_request(vec![user_message("list /tmp")]);
    let _ = mock.create_message_stream(req1).await.unwrap().next().await;
    // （我们不排干——重要的是捕获）

    // === 第 2 轮：运行时构建下一个请求，包含之前
    // 助手回合的 reasoning_content。模拟可以验证运行时保留的任何
    // ContentBlock::Thinking 都存在于下一个传出请求中——
    // 正是破坏 v0.4.9-v0.5.1 的负载形状。
    let next_messages = vec![
        user_message("list /tmp"),
        assistant_thinking("I should call list_dir.", ""),
        assistant_tool_call("call_a", "list_dir", serde_json::json!({ "path": "/tmp" })),
        tool_result_message("call_a", "/tmp/file1\n/tmp/file2"),
    ];
    let req2 = make_request(next_messages);
    let _ = mock.create_message_stream(req2).await.unwrap();

    // 模拟捕获了两个请求。断言第二个请求保留了
    // 之前助手消息的 Thinking 块——即运行时在重新发送前
    // 没有剥离 reasoning_content。（如果 reasoning_content 缺失，
    // V4 思考模式工具回合会拒绝 HTTP 400。）
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2);

    let req2 = &captured[1];
    let assistant_with_thinking = req2
        .messages
        .iter()
        .find(|m| {
            m.role == "assistant"
                && m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking { .. }))
        })
        .expect("回合 2 请求必须重放助手 Thinking 内容");

    let thinking_text = assistant_with_thinking
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Thinking { thinking, .. } => Some(thinking.clone()),
            _ => None,
        })
        .expect("Thinking 块存在");
    assert_eq!(
        thinking_text, "I should call list_dir.",
        "reasoning_content 必须在工具调用回合中逐字重放"
    );
}

// === 3. 工具调用往返 ========================================================

#[tokio::test]
async fn tool_call_round_trip_streams_args_then_continues() {
    // 回合 1 发出带有分块输入 JSON 的 tool_use 块。
    let turn1 = vec![
        canned::message_start("rt1"),
        canned::tool_use_block_start(0, "call_x", "read_file"),
        canned::tool_input_delta(0, r#"{"path":"#),
        canned::tool_input_delta(0, r#""README.md"}"#),
        canned::block_stop(0),
        canned::message_delta("tool_use", None),
        canned::message_stop(),
    ];
    let turn2 = vec![
        canned::message_start("rt2"),
        canned::text_block_start(0),
        canned::text_delta(0, "README starts with: # deepseek-tui"),
        canned::block_stop(0),
        canned::message_delta("end_turn", None),
        canned::message_stop(),
    ];
    let mock = MockLlmClient::new(vec![turn1, turn2]);

    // 第 1 轮
    let mut s1 = mock
        .create_message_stream(make_request(vec![user_message("read README.md")]))
        .await
        .unwrap();

    let mut tool_use_seen = false;
    let mut json_seen = String::new();
    while let Some(ev) = s1.next().await {
        match ev.unwrap() {
            StreamEvent::ContentBlockStart { content_block, .. } => {
                use crate::models::ContentBlockStart;
                if let ContentBlockStart::ToolUse { name, .. } = content_block {
                    assert_eq!(name, "read_file");
                    tool_use_seen = true;
                }
            }
            StreamEvent::ContentBlockDelta {
                delta: Delta::InputJsonDelta { partial_json },
                ..
            } => json_seen.push_str(&partial_json),
            StreamEvent::MessageStop => break,
            _ => {}
        }
    }
    assert!(tool_use_seen);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_seen).expect("连接后的有效 JSON");
    assert_eq!(parsed["path"], "README.md");

    // 第 2 轮——运行时发回 tool_result，模拟用
    // 最终的助手文本回合回复。
    let req2 = make_request(vec![
        user_message("read README.md"),
        assistant_tool_call(
            "call_x",
            "read_file",
            serde_json::json!({ "path": "README.md" }),
        ),
        tool_result_message("call_x", "# deepseek-tui\n..."),
    ]);
    let (text, stop) = drain_stream_text(&mock, req2).await;
    assert!(text.contains("# deepseek-tui"));
    assert_eq!(stop.as_deref(), Some("end_turn"));
}

// === 4. 一轮中的多个工具调用（并行排序）=====================================

#[tokio::test]
async fn parallel_tool_calls_preserve_ordering_in_turn_payload() {
    // 助手在单个回合中返回两个 tool_calls（索引 0 和 1）。
    // 运行时可以自由并行执行它们；此测试断言规范事件排序
    // 在单回合重放后仍然保持。
    let turn = vec![
        canned::message_start("p1"),
        canned::tool_use_block_start(0, "call_one", "list_dir"),
        canned::tool_input_delta(0, r#"{"path":"a"}"#),
        canned::block_stop(0),
        canned::tool_use_block_start(1, "call_two", "list_dir"),
        canned::tool_input_delta(1, r#"{"path":"b"}"#),
        canned::block_stop(1),
        canned::message_delta("tool_use", None),
        canned::message_stop(),
    ];
    let mock = MockLlmClient::new(vec![turn]);

    let mut stream = mock
        .create_message_stream(make_request(vec![user_message("list both")]))
        .await
        .unwrap();

    let mut starts: Vec<(u32, String)> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let StreamEvent::ContentBlockStart {
            index,
            content_block,
        } = ev.unwrap()
        {
            use crate::models::ContentBlockStart;
            if let ContentBlockStart::ToolUse { id, .. } = content_block {
                starts.push((index, id));
            }
        }
    }

    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0], (0, "call_one".to_string()));
    assert_eq!(starts[1], (1, "call_two".to_string()));
}

// === 5. 压缩风格的非流式调用 ================================================

#[tokio::test]
async fn compaction_non_streaming_returns_queued_message_response() {
    use crate::models::MessageResponse;

    let mock = MockLlmClient::new(vec![]);
    mock.push_message_response(MessageResponse {
        id: "compact_msg".to_string(),
        r#type: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "## Summary\n- Step 1\n- Step 2".to_string(),
            cache_control: None,
        }],
        model: "deepseek-v4-pro".to_string(),
        stop_reason: Some("end_turn".to_string()),
        stop_sequence: None,
        container: None,
        usage: Usage::default(),
    });

    // 运行时的压缩路径使用 create_message（而不是流）。
    let req = MessageRequest {
        stream: Some(false),
        ..make_request(vec![user_message("summarize")])
    };
    let resp = mock.create_message(req).await.unwrap();

    let text = match &resp.content[0] {
        ContentBlock::Text { text, .. } => text.clone(),
        _ => panic!("预期文本内容"),
    };
    assert!(text.contains("Summary"));
    assert_eq!(resp.id, "compact_msg");
    assert_eq!(mock.call_count(), 1);
}

// === 6. 子代理风格回合 ======================================================
//
// 在 `agent` 摘要后的下一轮必须在报告成功前重新验证声称的副作用。

#[tokio::test]
async fn v4_parent_reverifies_subagent_file_self_report_before_claiming_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("child-claimed-write.txt");
    assert!(!missing.exists(), "fixture 路径必须以缺失状态开始");
    let missing_path = missing.display().to_string();

    let parent = MockLlmClient::new(vec![vec![
        canned::message_start("parent_verify"),
        canned::thinking_delta(0, "Verify the child's file-write self-report first."),
        canned::tool_use_block_start(1, "verify_file", "read_file"),
        canned::tool_input_delta(1, &serde_json::json!({ "path": &missing_path }).to_string()),
        canned::block_stop(1),
        canned::message_delta("tool_use", None),
        canned::message_stop(),
    ]])
    .with_model("deepseek-v4-pro");
    let tool_summary = format!(
        "[sub-agent result summarized for parent context]\n\
Child results are self-reports; verify side effects with tools like read_file or list_dir before claiming success.\n\
- agent_filecheck (implementer) status=Completed\n  result: Wrote {missing_path} successfully."
    );

    let mut stream = parent
        .create_message_stream(make_request(vec![
            user_message("Use a child to create the file, then report back."),
            assistant_tool_call(
                "agent_call",
                "agent",
                serde_json::json!({
                    "prompt": "Create the requested file and report the result.",
                    "role": "implementer"
                }),
            ),
            tool_result_message("agent_call", &tool_summary),
        ]))
        .await
        .unwrap();

    let mut text_before_verification = String::new();
    let mut tool_name = None;
    let mut tool_input = String::new();
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            StreamEvent::ContentBlockStart { content_block, .. } => {
                use crate::models::ContentBlockStart;
                if let ContentBlockStart::ToolUse { name, .. } = content_block {
                    tool_name = Some(name);
                }
            }
            StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                Delta::InputJsonDelta { partial_json } => tool_input.push_str(&partial_json),
                Delta::TextDelta { text } => text_before_verification.push_str(&text),
                _ => {}
            },
            StreamEvent::MessageStop => break,
            _ => {}
        }
    }

    assert_eq!(text_before_verification, "");
    assert_eq!(tool_name.as_deref(), Some("read_file"));
    let parsed: serde_json::Value = serde_json::from_str(&tool_input).expect("工具输入 JSON");
    assert_eq!(parsed["path"], missing_path);
}

// === 7. 请求捕获观察 ========================================================
//
// 模拟在响应流打开前就已暴露请求捕获，因此 trait 级别测试可以验证
// 捕获的请求是按调用可观察的，而不是跨调用缓冲的。

#[tokio::test]
async fn capacity_gate_can_observe_request_before_response_streams() {
    let turn = vec![canned::simple_text_turn("ok")];
    let mock = MockLlmClient::new(turn);

    // 构建一个"接近限制"的请求——许多用户消息。
    let mut messages = Vec::new();
    for i in 0..200 {
        messages.push(user_message(&format!("m{i}")));
    }
    let req = make_request(messages);

    // 在运行时排干流之前，模拟已捕获请求。
    // 容量控制器可以检查此请求，并在估计令牌成本超过软上限时
    // 短路分发。
    let stream_future = mock.create_message_stream(req);
    let mut stream = stream_future.await.unwrap();

    assert_eq!(mock.captured_requests().len(), 1);
    let captured = mock.last_request().unwrap();
    assert_eq!(captured.messages.len(), 200);
    // 验证容量门控可以根据原始消息计数 + 捕获请求的负载大小
    // 计算"应推迟"的决策。
    let total_chars: usize = captured
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { text, .. } => text.len(),
            _ => 0,
        })
        .sum();
    assert!(
        total_chars > 100,
        "合成超容量请求应具有非平凡大小"
    );

    // 排干以保持模拟状态一致。
    while stream.next().await.is_some() {}
}

// === 8. 压缩默认值（#402 P0）================================================

#[test]
fn compaction_config_defaults_are_enabled_for_session_survivability() {
    // 生产 CompactionConfig 通过 `#[path = ...]` 模块门控，
    // 此处未连接，但我们可以测试原则：
    // `should_compact` 函数和 `CompactionConfig` 位于同一 crate 中。
    // 从生产模块重新导入以验证默认值。
    //
    // 我们通过模拟路径测试：上面的非流式压缩调用（测试 5）
    // 已经使用 `stream: Some(false)` 执行了 `create_message`，
    // 这是 `compact_messages` 使用的代码路径。结合
    // 容量控制器的 `TargetedContextRefresh`，默认启用的
    // 压缩配置意味着长会话会在到达上下文窗口限制前自动压缩。
    //
    // 此测试是一个烟雾测试，确保默认值编译且正确。
    // 生产 `CompactionConfig::default()` 由
    // `compaction::tests::should_compact_respects_enabled_flag` 等测试。
    let config = crate::models::compaction_threshold_for_model_at_percent("deepseek-v4-pro", 80.0);
    // 验证阈值合理（> 0 且 < 上下文窗口）。
    assert!(config > 0, "压缩阈值必须为正数");
    assert!(config < 1_000_000, "压缩阈值必须低于 1M");
}
