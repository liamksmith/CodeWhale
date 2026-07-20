//! 生产环境 `llm_client` 模块接口的仅测试镜像。
//!
//! `tests/integration_mock_llm.rs` 下的集成测试将此文件作为 `mod llm_client` 包含，
//! 并将 `mock.rs` 作为嵌套子模块。这种方式使 `mock.rs` 的 `super::{LlmClient, StreamEventBox}`
//! 路径能够正确解析 — 它们引用的是在此处声明的 trait 和别名。
//!
//! trait 形状必须与 `crates/tui/src/llm_client/mod.rs` 中的真实版本保持 1:1 一致。
//! 如果生产 trait 增加了方法，请在此处镜像它，以便 `mock.rs`（二进制文件中包含的同一源文件）仍然满足它。

use anyhow::Result;
use std::pin::Pin;

use crate::models::{MessageRequest, MessageResponse, StreamEvent};

pub type StreamEventBox =
    Pin<Box<dyn futures_util::Stream<Item = Result<StreamEvent>> + Send + 'static>>;

#[allow(async_fn_in_trait, dead_code)]
pub trait LlmClient: Send + Sync {
    fn provider_name(&self) -> &'static str;
    fn model(&self) -> &str;
    async fn create_message(&self, request: MessageRequest) -> Result<MessageResponse>;
    async fn create_message_stream(&self, request: MessageRequest) -> Result<StreamEventBox>;
    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }
}

#[path = "../../src/llm_client/mock.rs"]
pub mod mock;
