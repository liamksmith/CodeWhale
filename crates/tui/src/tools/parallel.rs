//! 用于并行执行多个工具调用的工具包装器。
//!
//! 注意：此元工具已故意不再向 agent 注册（参见 `ToolRegistryBuilder::with_parallel_tool`）。
//! DeepSeek-V4 支持在单个 assistant 轮次中的原生并行 `tool_calls`，
//! 而暴露 OpenAI 内部名称 `multi_tool_use.parallel` 导致模型幻觉式地生成 ChatGPT 风格的 XML 包装器。
//! 保留该结构体以使引擎兼容性分发器和历史会话仍能正常解析。

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};
use async_trait::async_trait;
use serde_json::{Value, json};

#[allow(dead_code)]
pub struct MultiToolUseParallelTool;

#[async_trait]
impl ToolSpec for MultiToolUseParallelTool {
    fn name(&self) -> &'static str {
        "multi_tool_use.parallel"
    }

    fn description(&self) -> &'static str {
        "Execute multiple tool calls in parallel and return their results."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_uses": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "recipient_name": { "type": "string" },
                            "parameters": { "type": "object" }
                        },
                        "required": ["recipient_name", "parameters"]
                    }
                }
            },
            "required": ["tool_uses"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "multi_tool_use.parallel must be handled by the engine",
        ))
    }
}
