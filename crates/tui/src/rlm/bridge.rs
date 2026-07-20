//! RPC 桥接，服务 RLM 回合期间从长生命周期 Python REPL 返回的
//! `llm_query` / `rlm_query` 调用。
//!
//! 这是早期版本 HTTP 侧车进程的精神继承者——
//! 只不过不再绑定本地主机端口并通过 `urllib` 路由，
//! 请求通过标准输入/标准输出传入，我们直接在这里的 Rust 中调用 LLM 客户端。
//!
//! 桥接跟踪累积令牌使用量和递归预算。对于
//! `Rlm` / `RlmBatch` 请求，它递归调用深度为 depth-1 的
//! `run_rlm_turn_inner`；未来类型循环（桥接 → run_rlm_turn_inner →
//! 桥接）通过 `run_rlm_turn_inner` 返回盒装 dyn future 来打破。

use std::sync::Arc;
use std::time::Duration;
use std::{future::Future, pin::Pin};

use anyhow::Result;
use futures_util::future::join_all;
use tokio::sync::Mutex;

use crate::llm_client::LlmClient;
use crate::models::{ContentBlock, Message, MessageRequest, MessageResponse, SystemPrompt, Usage};
use crate::repl::runtime::{BatchResp, RpcDispatcher, RpcRequest, RpcResponse, SingleResp};
use crate::utils::spawn_supervised;

/// 每个子完成的超时时间——与之前侧车默认值相同。
const CHILD_TIMEOUT_SECS: u64 = 120;
/// 一次性子完成的默认 `max_tokens`。
const DEFAULT_CHILD_MAX_TOKENS: u32 = 4096;
/// 每个批处理 RPC 的提示上限。
pub const MAX_BATCH: usize = 16;

/// RLM 桥接需要的 LLM 客户端接口的对象安全切片。
///
/// `LlmClient` 本身使用原生异步 trait 方法，这些方法不是 dyn 安全的。
/// 桥接只需要非流式完成，因此这个盒装未来适配器
/// 为测试提供了一个干净的 mock 接缝，无需改变更广泛的提供商 trait。
pub(crate) trait RlmLlmClient: Send + Sync {
    fn create_message_boxed(
        &self,
        request: MessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse>> + Send + '_>>;
}

impl<T> RlmLlmClient for T
where
    T: LlmClient + Send + Sync,
{
    fn create_message_boxed(
        &self,
        request: MessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse>> + Send + '_>> {
        Box::pin(self.create_message(request))
    }
}

/// 与桥接共享的状态，跨越一个回合中的所有 RPC 调用。
pub struct RlmBridge {
    client: Arc<dyn RlmLlmClient>,
    child_model: String,
    /// `Rlm` / `RlmBatch` 请求剩余递归预算。当
    /// 为零时，这些请求回退到普通 `Llm` 完成。
    depth_remaining: u32,
    usage: Arc<Mutex<Usage>>,
}

impl RlmBridge {
    pub(crate) fn new(
        client: Arc<dyn RlmLlmClient>,
        child_model: String,
        depth_remaining: u32,
    ) -> Self {
        Self {
            client,
            child_model,
            depth_remaining,
            usage: Arc::new(Mutex::new(Usage::default())),
        }
    }

    pub fn usage_handle(&self) -> Arc<Mutex<Usage>> {
        Arc::clone(&self.usage)
    }

    async fn dispatch_llm(
        &self,
        prompt: String,
        _model: Option<String>,
        max_tokens: Option<u32>,
        system: Option<String>,
    ) -> SingleResp {
        let request = MessageRequest {
            // Python 助手接受旧代码片段的 `model=`，但有意
            // 不将其视为权威。RLM 子调用固定到工具的配置子模型，
            // 因此模型生成的 Python 无法静默地将廉价扇出工作升级到昂贵模型。
            model: self.child_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
            max_tokens: max_tokens.unwrap_or(DEFAULT_CHILD_MAX_TOKENS),
            system: system.map(SystemPrompt::Text),
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: Some(false),
            temperature: Some(0.4_f32),
            top_p: Some(0.9_f32),
        };

        let fut = self.client.create_message_boxed(request);
        let response =
            match tokio::time::timeout(Duration::from_secs(CHILD_TIMEOUT_SECS), fut).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    return SingleResp {
                        text: String::new(),
                        error: Some(format!("llm_query 失败: {e}")),
                    };
                }
                Err(_) => {
                    return SingleResp {
                        text: String::new(),
                        error: Some(format!("llm_query 在 {CHILD_TIMEOUT_SECS} 秒后超时")),
                    };
                }
            };

        let text = response
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        {
            let mut u = self.usage.lock().await;
            super::add_usage_with_prompt_cache(&mut u, &response.usage);
        }

        SingleResp { text, error: None }
    }

    async fn dispatch_llm_batch(
        &self,
        prompts: Vec<String>,
        _model: Option<String>,
        dependency_mode: Option<String>,
    ) -> BatchResp {
        if let Some(resp) = batch_guard(prompts.len(), dependency_mode.as_deref()) {
            return resp;
        }

        let model = Arc::new(self.child_model.clone());

        let futures = prompts.into_iter().map(|prompt| {
            let model = Arc::clone(&model);
            async move {
                self.dispatch_llm((*prompt).to_string(), Some((*model).clone()), None, None)
                    .await
            }
        });

        BatchResp {
            results: join_all(futures).await,
        }
    }

    async fn dispatch_rlm(&self, prompt: String, _model: Option<String>) -> SingleResp {
        if self.depth_remaining == 0 {
            // 预算耗尽——回退到一次性子完成
            // 而不是返回错误。与论文行为一致
            //（"sub_RLM 在 depth=0 时优雅降级为 llm_query"）。
            return self.dispatch_llm(prompt, None, None, None).await;
        }

        // 构建一个 drain 通道来吸收嵌套回合的状态事件
        //（我们不展示它们；此分发对外部代理流不可见）。
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = spawn_supervised(
            "rlm-bridge-drain",
            std::panic::Location::caller(),
            async move { while rx.recv().await.is_some() {} },
        );

        let child_model = self.child_model.clone();

        // 递归调用。`run_rlm_turn_inner` 上的 dyn 擦除打破了
        // `bridge → turn → bridge` 不透明未来循环。
        let result = super::turn::run_rlm_turn_inner(
            Arc::clone(&self.client),
            child_model.clone(),
            prompt,
            None,
            child_model,
            tx,
            self.depth_remaining.saturating_sub(1),
        )
        .await;

        drain.abort();

        {
            let mut u = self.usage.lock().await;
            super::add_usage_with_prompt_cache(&mut u, &result.usage);
        }

        SingleResp {
            text: result.answer,
            error: result.error,
        }
    }

    async fn dispatch_rlm_batch(
        &self,
        prompts: Vec<String>,
        _model: Option<String>,
        dependency_mode: Option<String>,
    ) -> BatchResp {
        if let Some(resp) = batch_guard(prompts.len(), dependency_mode.as_deref()) {
            return resp;
        }

        let futures = prompts
            .into_iter()
            .map(|p| async move { self.dispatch_rlm(p, None).await });
        BatchResp {
            results: join_all(futures).await,
        }
    }
}

fn batch_guard(prompt_count: usize, dependency_mode: Option<&str>) -> Option<BatchResp> {
    if prompt_count == 0 {
        return Some(BatchResp { results: vec![] });
    }
    if prompt_count > MAX_BATCH {
        return Some(BatchResp {
            results: (0..prompt_count)
                .map(|_| SingleResp {
                    text: String::new(),
                    error: Some(format!("批处理过大: {prompt_count} > {MAX_BATCH}")),
                })
                .collect(),
        });
    }
    let mode = dependency_mode
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    if !matches!(
        mode.as_str(),
        "independent" | "parallel_safe" | "map_reduce"
    ) {
        return Some(BatchResp {
            results: (0..prompt_count)
                .map(|_| SingleResp {
                    text: String::new(),
                    error: Some(
                        "批处理需要 dependency_mode='independent'；对于依赖工作请使用 sub_query_sequence 或顺序 sub_query 调用"
                            .to_string(),
                    ),
                })
                .collect(),
        });
    }
    None
}

impl RpcDispatcher for RlmBridge {
    fn dispatch<'a>(
        &'a self,
        req: RpcRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RpcResponse> + Send + 'a>> {
        Box::pin(async move {
            match req {
                RpcRequest::Llm {
                    prompt,
                    model,
                    max_tokens,
                    system,
                } => {
                    RpcResponse::Single(self.dispatch_llm(prompt, model, max_tokens, system).await)
                }
                RpcRequest::LlmBatch {
                    prompts,
                    model,
                    dependency_mode,
                    safety_note: _,
                } => RpcResponse::Batch(
                    self.dispatch_llm_batch(prompts, model, dependency_mode)
                        .await,
                ),
                RpcRequest::Rlm { prompt, model } => {
                    RpcResponse::Single(self.dispatch_rlm(prompt, model).await)
                }
                RpcRequest::RlmBatch {
                    prompts,
                    model,
                    dependency_mode,
                    safety_note: _,
                } => RpcResponse::Batch(
                    self.dispatch_rlm_batch(prompts, model, dependency_mode)
                        .await,
                ),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::mock::MockLlmClient;

    fn mock_response_with_usage(text: &str, usage: Usage) -> MessageResponse {
        MessageResponse {
            id: "mock_msg".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            model: "mock-model".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            container: None,
            usage,
        }
    }

    fn mock_response(text: &str, input_tokens: u32, output_tokens: u32) -> MessageResponse {
        mock_response_with_usage(
            text,
            Usage {
                input_tokens,
                output_tokens,
                ..Usage::default()
            },
        )
    }

    fn bridge_for(mock: Arc<MockLlmClient>, depth_remaining: u32) -> RlmBridge {
        let client: Arc<dyn RlmLlmClient> = mock;
        RlmBridge::new(client, "child-model".to_string(), depth_remaining)
    }

    #[test]
    fn batch_guard_allows_non_empty_batches_at_the_cap() {
        assert!(batch_guard(MAX_BATCH, Some("independent")).is_none());
    }

    #[test]
    fn batch_guard_returns_empty_response_for_empty_batches() {
        let response = batch_guard(0, None).expect("空批处理应被处理");
        assert!(response.results.is_empty());
    }

    #[test]
    fn batch_guard_returns_one_error_per_oversized_prompt() {
        let response = batch_guard(MAX_BATCH + 2, Some("independent"))
            .expect("过大批处理应被处理");
        assert_eq!(response.results.len(), MAX_BATCH + 2);
        assert!(response.results.iter().all(|result| {
            result.text.is_empty()
                && result
                    .error
                    .as_deref()
                    .is_some_and(|err| err.contains("batch too large"))
        }));
    }

    #[test]
    fn batch_guard_requires_explicit_independence_for_parallel_work() {
        let response = batch_guard(2, None).expect("缺少依赖模式应被处理");
        assert_eq!(response.results.len(), 2);
        assert!(response.results.iter().all(|result| {
            result.text.is_empty()
                && result
                    .error
                    .as_deref()
                    .is_some_and(|err| err.contains("dependency_mode='independent'"))
        }));

        let response = batch_guard(2, Some("sequential"))
            .expect("依赖的依赖模式应被处理");
        assert!(response.results.iter().all(|result| {
            result
                .error
                .as_deref()
                .is_some_and(|err| err.contains("sub_query_sequence"))
        }));
    }

    #[tokio::test]
    async fn llm_dispatch_pins_configured_child_model() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response("child answer", 7, 11));
        let bridge = bridge_for(Arc::clone(&mock), 1);

        let response = bridge
            .dispatch(RpcRequest::Llm {
                prompt: "child prompt".to_string(),
                model: Some("override-model".to_string()),
                max_tokens: Some(123),
                system: Some("child system".to_string()),
            })
            .await;

        match response {
            RpcResponse::Single(single) => {
                assert_eq!(single.text, "child answer");
                assert!(single.error.is_none());
            }
            other => panic!("预期单个响应，得到 {other:?}"),
        }

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "child-model");
        assert_eq!(captured[0].max_tokens, 123);
        assert_eq!(
            captured[0].system,
            Some(SystemPrompt::Text("child system".to_string()))
        );

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 11);
    }

    #[tokio::test]
    async fn llm_dispatch_preserves_prompt_cache_usage() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response_with_usage(
            "cached child answer",
            Usage {
                input_tokens: 1000,
                output_tokens: 100,
                prompt_cache_hit_tokens: Some(800),
                prompt_cache_miss_tokens: Some(200),
                ..Usage::default()
            },
        ));
        let bridge = bridge_for(Arc::clone(&mock), 1);

        let response = bridge
            .dispatch(RpcRequest::Llm {
                prompt: "child prompt".to_string(),
                model: None,
                max_tokens: None,
                system: None,
            })
            .await;

        match response {
            RpcResponse::Single(single) => {
                assert_eq!(single.text, "cached child answer");
                assert!(single.error.is_none());
            }
            other => panic!("预期单个响应，得到 {other:?}"),
        }

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.prompt_cache_hit_tokens, Some(800));
        assert_eq!(usage.prompt_cache_miss_tokens, Some(200));
    }

    #[tokio::test]
    async fn llm_batch_dispatch_pins_configured_child_model() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response("one", 1, 2));
        mock.push_message_response(mock_response("two", 3, 4));
        mock.push_message_response(mock_response("three", 5, 6));
        let bridge = bridge_for(Arc::clone(&mock), 1);

        let response = bridge
            .dispatch(RpcRequest::LlmBatch {
                prompts: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                model: Some("batch-model".to_string()),
                dependency_mode: Some("independent".to_string()),
                safety_note: Some("test prompts are independent".to_string()),
            })
            .await;

        match response {
            RpcResponse::Batch(batch) => {
                let texts: Vec<_> = batch
                    .results
                    .iter()
                    .map(|result| result.text.as_str())
                    .collect();
                assert_eq!(texts, ["one", "two", "three"]);
                assert!(batch.results.iter().all(|result| result.error.is_none()));
            }
            other => panic!("预期批处理响应，得到 {other:?}"),
        }

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 3);
        assert!(
            captured
                .iter()
                .all(|request| request.model == "child-model")
        );

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 9);
        assert_eq!(usage.output_tokens, 12);
    }

    #[tokio::test]
    async fn rlm_dispatch_at_depth_zero_pins_configured_child_model() {
        let mock = Arc::new(MockLlmClient::new(Vec::new()));
        mock.push_message_response(mock_response("fallback answer", 3, 5));
        let bridge = bridge_for(Arc::clone(&mock), 0);

        let response = bridge
            .dispatch(RpcRequest::Rlm {
                prompt: "nested prompt".to_string(),
                model: Some("override-model".to_string()),
            })
            .await;

        match response {
            RpcResponse::Single(single) => {
                assert_eq!(single.text, "fallback answer");
                assert!(single.error.is_none());
            }
            other => panic!("预期单个响应，得到 {other:?}"),
        }

        let usage = bridge.usage.lock().await;
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);

        let captured = mock.captured_requests();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "child-model");
    }
}
