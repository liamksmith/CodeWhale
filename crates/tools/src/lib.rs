use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use codewhale_protocol::{ToolKind, ToolOutput, ToolPayload};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

tokio::task_local! {
    static TOOL_EXECUTION_LOCK_HELD: ();
}

/// 工具可能拥有或需要的能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCapability {
    /// 工具只读取数据，从不修改状态。
    ReadOnly,
    /// 工具写入文件系统。
    WritesFiles,
    /// 工具执行任意 shell 命令。
    ExecutesCode,
    /// 工具发起网络请求。
    Network,
    /// 工具可以在沙箱中运行。
    Sandboxable,
    /// 工具在执行前需要用户批准。
    RequiresApproval,
}

/// 工具的批准要求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalRequirement {
    /// 从不需要批准：安全的只读操作。
    #[default]
    Auto,
    /// 建议批准但允许用户跳过。
    Suggest,
    /// 始终需要显式用户批准。
    Required,
}

/// 工具执行期间可能发生的错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolError {
    #[error("Failed to validate input: {message}")]
    InvalidInput { message: String },
    #[error("Failed to validate input: missing required field '{field}'")]
    MissingField { field: String },
    #[error("Failed to resolve path '{}': path escapes workspace", path.display())]
    PathEscape { path: PathBuf },
    #[error("Failed to execute tool: {message}")]
    ExecutionFailed { message: String },
    #[error("Failed to execute tool: operation timed out after {seconds}s")]
    Timeout { seconds: u64 },
    #[error("Failed to locate tool: {message}")]
    NotAvailable { message: String },
    #[error("Failed to authorize tool execution: {message}")]
    PermissionDenied { message: String },
}

impl ToolError {
    #[must_use]
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: msg.into(),
        }
    }

    #[must_use]
    pub fn missing_field(field: impl Into<String>) -> Self {
        Self::MissingField {
            field: field.into(),
        }
    }

    #[must_use]
    pub fn execution_failed(msg: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: msg.into(),
        }
    }

    #[must_use]
    pub fn path_escape(path: impl Into<PathBuf>) -> Self {
        Self::PathEscape { path: path.into() }
    }

    #[must_use]
    pub fn not_available(msg: impl Into<String>) -> Self {
        Self::NotAvailable {
            message: msg.into(),
        }
    }

    #[must_use]
    pub fn permission_denied(msg: impl Into<String>) -> Self {
        Self::PermissionDenied {
            message: msg.into(),
        }
    }
}

/// 工具执行的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// 输出内容，可以是 JSON 或纯文本。
    pub content: String,
    /// 执行是否成功。
    pub success: bool,
    /// 可选的结构化元数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    /// 创建成功结果。
    #[must_use]
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: true,
            metadata: None,
        }
    }

    /// 创建错误结果。
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            success: false,
            metadata: None,
        }
    }

    /// 从 JSON 创建成功结果。
    pub fn json<T: Serialize>(value: &T) -> std::result::Result<Self, serde_json::Error> {
        Ok(Self {
            content: serde_json::to_string(value)?,
            success: true,
            metadata: None,
        })
    }

    /// 向结果添加元数据。
    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// 从 JSON 输入中提取必填字符串字段的辅助函数。
pub fn required_str<'a>(input: &'a Value, field: &str) -> std::result::Result<&'a str, ToolError> {
    input.get(field).and_then(Value::as_str).ok_or_else(|| {
        // 当字段缺失时，列出调用方*确实*提供的字段，
        // 以便模型无需重试即可发现不匹配。
        let provided: Vec<&str> = input
            .as_object()
            .map(|obj| obj.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        if provided.is_empty() {
            ToolError::missing_field(field)
        } else {
            let hint = format!(
                "missing required field '{field}'. Input provided: {}",
                provided.join(", ")
            );
            ToolError::invalid_input(hint)
        }
    })
}

/// 从 JSON 输入中提取可选字符串字段的辅助函数。
#[must_use]
pub fn optional_str<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input.get(field).and_then(Value::as_str)
}

/// 从 JSON 输入中提取必填 u64 字段的辅助函数。
pub fn required_u64(input: &Value, field: &str) -> std::result::Result<u64, ToolError> {
    input
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::missing_field(field))
}

/// 从 JSON 输入中提取可选 u64 字段并带默认值的辅助函数。
#[must_use]
pub fn optional_u64(input: &Value, field: &str, default: u64) -> u64 {
    input.get(field).and_then(Value::as_u64).unwrap_or(default)
}

/// 从 JSON 输入中提取可选布尔字段并带默认值的辅助函数。
#[must_use]
pub fn optional_bool(input: &Value, field: &str, default: bool) -> bool {
    input.get(field).and_then(Value::as_bool).unwrap_or(default)
}

/// 描述注册表中可用工具的描述符。
///
/// 包含工具的名称、JSON 输入/输出模式以及
/// 执行约束，如超时和并行性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// 用于查找工具的唯一名称。
    pub name: String,
    /// 描述工具预期输入参数的 JSON 模式。
    pub input_schema: Value,
    /// 描述工具输出格式的 JSON 模式。
    pub output_schema: Value,
    /// 此工具的多次调用是否可以并发运行。
    pub supports_parallel_tool_calls: bool,
    /// 每次调用的可选超时（毫秒）；`None` 表示无超时。
    pub timeout_ms: Option<u64>,
}

/// [`ToolDescriptor`] 及其运行时配置。
///
/// 包装 `ToolDescriptor` 并直接暴露并行标志，以便
/// 调度器无需深入内部规范即可检查它。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredToolDescriptor {
    /// 底层工具描述符。
    pub spec: ToolDescriptor,
    /// 此工具是否支持并发调用。
    pub supports_parallel_tool_calls: bool,
}

/// 标识工具调用的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallSource {
    /// 直接来自模型或用户的调用。
    Direct,
    /// 通过 JavaScript REPL 环境的调用。
    JsRepl,
}

/// 已验证和分发之前的工具调用请求。
///
/// 包含工具名称、输入载荷以及有关调用来源的元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 要调用的工具的名称。
    pub name: String,
    /// 工具的输入载荷。
    pub payload: ToolPayload,
    /// 此调用的来源（直接或 REPL）。
    pub source: ToolCallSource,
    /// 来自上游提供者的可选原始工具调用标识符。
    pub raw_tool_call_id: Option<String>,
}

impl ToolCall {
    /// 推导此调用的执行主题。
    ///
    /// 对于本地 shell 载荷，返回 shell 命令及其工作目录；
    /// 对于所有其他载荷，返回工具名称和提供的 `fallback_cwd`。
    /// 元组的第三个元素是人类可读的类型标签（`"shell"` 或 `"tool"`）。
    pub fn execution_subject(&self, fallback_cwd: &str) -> (String, String, &'static str) {
        match &self.payload {
            ToolPayload::LocalShell { params } => (
                params.command.clone(),
                params
                    .cwd
                    .clone()
                    .unwrap_or_else(|| fallback_cwd.to_string()),
                "shell",
            ),
            _ => (self.name.clone(), fallback_cwd.to_string(), "tool"),
        }
    }
}

/// 准备好处理的已验证工具调用。
///
/// 在 [`ToolCall`] 通过验证后由注册表创建，这携带了
/// [`ToolHandler`] 执行工具所需的所有上下文。
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// 此调用的唯一标识符（生成或来自提供者）。
    pub call_id: String,
    /// 正在调用的工具的名称。
    pub tool_name: String,
    /// 工具的输入载荷。
    pub payload: ToolPayload,
    /// 此调用的来源。
    pub source: ToolCallSource,
}

/// 工具分发和执行期间可能发生的错误。
///
/// 与表示工具内部输入验证失败的 [`ToolError`] 不同，
/// `FunctionCallError` 涵盖分发层的问题：未找到工具、
/// 类型不匹配、由于工具是可变的而被拒绝、超时、
/// 取消或处理程序返回错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FunctionCallError {
    /// 没有以该名称注册的工具。
    ToolNotFound { name: String },
    /// 载荷类型与处理程序期望的类型不匹配。
    KindMismatch { expected: ToolKind, got: ToolKind },
    /// 工具是可变的但 `allow_mutating` 为 `false`。
    MutatingToolRejected { name: String },
    /// 工具执行超过其配置的超时。
    TimedOut { name: String, timeout_ms: u64 },
    /// 工具执行被取消。
    Cancelled { name: String },
    /// 工具处理程序返回错误。
    ExecutionFailed { name: String, error: String },
}

/// 具体工具处理程序实现的 trait。
///
/// 每个注册的工具由处理程序支持，该处理程序报告其类型、
/// 是否可变，并执行实际操作。
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// 此处理程序期望的 [`ToolKind`]（例如 `Function` 或 `Mcp`）。
    fn kind(&self) -> ToolKind;

    /// 如果 `kind` 与此处理程序期望的类型匹配，则返回 `true`。
    ///
    /// 默认实现与 [`kind()`](ToolHandler::kind) 进行比较。
    fn matches_kind(&self, kind: ToolKind) -> bool {
        self.kind() == kind
    }

    /// 此工具是否执行需要用户批准的副作用。
    ///
    /// 默认为 `false`（只读/安全）。
    fn is_mutating(&self) -> bool {
        false
    }

    /// 使用给定的调用上下文执行工具。
    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, FunctionCallError>;
}

/// 通过读/写锁管理并发工具执行。
///
/// 并行安全的工具获取读锁（允许重叠），而
/// 串行工具获取写锁（独占访问）。重入调用
///（例如工具调用另一个工具）跳过锁定以避免死锁。
#[derive(Debug)]
pub struct ToolCallRuntime {
    execution_lock: Arc<RwLock<()>>,
}

impl Default for ToolCallRuntime {
    fn default() -> Self {
        Self {
            execution_lock: Arc::new(RwLock::new(())),
        }
    }
}

#[derive(Debug)]
enum ToolExecutionGuard {
    Parallel(#[allow(dead_code)] OwnedRwLockReadGuard<()>),
    Serial(#[allow(dead_code)] OwnedRwLockWriteGuard<()>),
    Reentrant,
}

impl ToolCallRuntime {
    async fn acquire(&self, supports_parallel: bool) -> ToolExecutionGuard {
        if TOOL_EXECUTION_LOCK_HELD.try_with(|_| ()).is_ok() {
            return ToolExecutionGuard::Reentrant;
        }

        if supports_parallel {
            ToolExecutionGuard::Parallel(self.execution_lock.clone().read_owned().await)
        } else {
            ToolExecutionGuard::Serial(self.execution_lock.clone().write_owned().await)
        }
    }
}

/// 将工具名称映射到其规范和处理程序的中央注册表。
///
/// 使用 [`register()`](ToolRegistry::register) 添加工具，然后
/// 使用 [`dispatch()`](ToolRegistry::dispatch) 调用它们。注册表
/// 拥有管理并发执行的 [`ToolCallRuntime`]。
#[derive(Default)]
pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    specs: HashMap<String, ConfiguredToolDescriptor>,
    runtime: ToolCallRuntime,
}

impl ToolRegistry {
    /// 使用其规范和处理程序注册工具。
    ///
    /// 工具名称取自 `spec.name`。如果注册失败则返回错误
    ///（目前不会失败，但保留 `Result` 用于将来的验证）。
    pub fn register(&mut self, spec: ToolDescriptor, handler: Arc<dyn ToolHandler>) -> Result<()> {
        let name = spec.name.clone();
        self.specs.insert(
            name.clone(),
            ConfiguredToolDescriptor {
                supports_parallel_tool_calls: spec.supports_parallel_tool_calls,
                spec,
            },
        );
        self.handlers.insert(name, handler);
        Ok(())
    }

    /// 返回每个注册工具的配置规范。
    pub fn list_specs(&self) -> Vec<ConfiguredToolDescriptor> {
        self.specs.values().cloned().collect()
    }

    /// 验证并执行工具调用。
    ///
    /// 按名称查找工具，验证载荷类型与处理程序匹配，
    /// 应用 `allow_mutating` 守卫，获取适当的执行锁，
    /// 并将调用转发给处理程序。如果任何验证步骤失败或
    /// 处理程序返回错误，则返回 [`FunctionCallError`]。
    pub async fn dispatch(
        &self,
        call: ToolCall,
        allow_mutating: bool,
    ) -> std::result::Result<ToolOutput, FunctionCallError> {
        let handler = self.handlers.get(&call.name).cloned().ok_or_else(|| {
            FunctionCallError::ToolNotFound {
                name: call.name.clone(),
            }
        })?;
        let configured =
            self.specs
                .get(&call.name)
                .cloned()
                .ok_or_else(|| FunctionCallError::ToolNotFound {
                    name: call.name.clone(),
                })?;

        let payload_kind = tool_payload_kind(&call.payload);
        let expected = handler.kind();
        if !handler.matches_kind(payload_kind) {
            return Err(FunctionCallError::KindMismatch {
                expected,
                got: payload_kind,
            });
        }
        if handler.is_mutating() && !allow_mutating {
            return Err(FunctionCallError::MutatingToolRejected { name: call.name });
        }

        let invocation = ToolInvocation {
            call_id: call
                .raw_tool_call_id
                .clone()
                .unwrap_or_else(|| format!("tool-call-{}", uuid::Uuid::new_v4())),
            tool_name: call.name.clone(),
            payload: call.payload,
            source: call.source,
        };

        let _guard = self
            .runtime
            .acquire(configured.supports_parallel_tool_calls)
            .await;

        TOOL_EXECUTION_LOCK_HELD
            .scope(
                (),
                self.execute_with_timeout(handler, configured.spec.timeout_ms, invocation),
            )
            .await
    }

    async fn execute_with_timeout(
        &self,
        handler: Arc<dyn ToolHandler>,
        timeout_ms: Option<u64>,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, FunctionCallError> {
        if let Some(timeout_ms) = timeout_ms {
            let name = invocation.tool_name.clone();
            match tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                handler.handle(invocation),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(FunctionCallError::TimedOut { name, timeout_ms }),
            }
        } else {
            handler.handle(invocation).await
        }
    }
}

fn tool_payload_kind(payload: &ToolPayload) -> ToolKind {
    match payload {
        ToolPayload::Mcp { .. } => ToolKind::Mcp,
        ToolPayload::Function { .. }
        | ToolPayload::Custom { .. }
        | ToolPayload::LocalShell { .. } => ToolKind::Function,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_result_success_sets_plain_content() {
        let content = "operation completed successfully";
        let result = ToolResult::success(content);

        assert!(result.success);
        assert_eq!(result.content, content);
        assert!(result.metadata.is_none());
    }

    #[test]
    fn tool_result_json_round_trips_content() {
        let result = ToolResult::json(&json!({"ok": true})).expect("json");
        assert!(result.success);
        let content: serde_json::Value =
            serde_json::from_str(&result.content).expect("content is valid json");
        assert_eq!(content, json!({"ok": true}));
    }

    #[test]
    fn helper_extractors_validate_shape() {
        let input = json!({"name": "demo", "count": 7, "enabled": true});
        assert_eq!(required_str(&input, "name").expect("name"), "demo");
        assert_eq!(optional_str(&input, "name"), Some("demo"));
        assert_eq!(optional_str(&input, "missing"), None);
        assert_eq!(optional_str(&input, "count"), None);
        assert_eq!(optional_str(&json!({"name": null}), "name"), None);
        assert_eq!(optional_u64(&input, "count", 0), 7);
        assert!(optional_bool(&input, "enabled", false));
        assert!(matches!(
            required_u64(&input, "name"),
            Err(ToolError::MissingField { .. })
        ));
    }

    #[test]
    fn required_u64_rejects_missing_or_non_integer_values() {
        assert!(matches!(
            required_u64(&json!({}), "count"),
            Err(ToolError::MissingField { .. })
        ));
        assert_eq!(required_u64(&json!({"count": 42}), "count").unwrap(), 42);
        assert_eq!(
            required_u64(&json!({"count": u64::MAX}), "count").unwrap(),
            u64::MAX
        );

        for value in [json!(-1), json!(2.5), json!("42")] {
            assert!(matches!(
                required_u64(&json!({"count": value}), "count"),
                Err(ToolError::MissingField { .. })
            ));
        }
    }

    #[test]
    fn required_str_reports_provided_fields_on_missing_required_field() {
        let input = json!({"path": "src/lib.rs", "content": "new body"});
        let err = required_str(&input, "replace").expect_err("replace is missing");
        let message = err.to_string();
        assert!(message.contains("missing required field 'replace'"));
        assert!(message.contains("Input provided:"));
        assert!(message.contains("path"));
        assert!(message.contains("content"));
    }

    #[test]
    fn tool_error_display_matches_legacy_text() {
        let err = ToolError::missing_field("path");
        assert_eq!(
            err.to_string(),
            "Failed to validate input: missing required field 'path'"
        );
    }

    #[test]
    fn tool_error_missing_field_constructor() {
        let err = ToolError::missing_field("my_field");
        assert!(matches!(err, ToolError::MissingField { field } if field == "my_field"));
    }

    #[test]
    fn tool_error_not_available_displays_reason() {
        let err = ToolError::not_available("custom tool not found");

        assert!(matches!(err, ToolError::NotAvailable { .. }));
        assert_eq!(
            err.to_string(),
            "Failed to locate tool: custom tool not found"
        );
    }

    #[test]
    fn tool_error_permission_denied_displays_reason() {
        let err = ToolError::permission_denied("unauthorized user");

        assert!(matches!(err, ToolError::PermissionDenied { .. }));
        assert_eq!(
            err.to_string(),
            "Failed to authorize tool execution: unauthorized user"
        );
    }

    #[test]
    fn tool_error_execution_failed_displays_reason() {
        let err = ToolError::execution_failed("process crashed");

        assert!(
            matches!(err, ToolError::ExecutionFailed { ref message } if message == "process crashed")
        );
        assert_eq!(err.to_string(), "Failed to execute tool: process crashed");
    }

    #[test]
    fn tool_error_invalid_input_creates_correct_variant() {
        let err = ToolError::invalid_input("test invalid message");
        match err {
            ToolError::InvalidInput { message } => {
                assert_eq!(message, "test invalid message");
            }
            _ => panic!("Expected ToolError::InvalidInput, got {err:?}"),
        }
    }

    #[test]
    fn tool_error_path_escape_display() {
        let path = std::path::PathBuf::from("../outside");
        let err = ToolError::path_escape(path);
        assert_eq!(
            err.to_string(),
            "Failed to resolve path '../outside': path escapes workspace"
        );
    }

    #[test]
    fn tool_call_execution_subject_uses_local_shell_command_and_cwd() {
        let call = ToolCall {
            name: "shell".to_string(),
            payload: ToolPayload::LocalShell {
                params: codewhale_protocol::LocalShellParams {
                    command: "ls -l".to_string(),
                    cwd: Some("/custom/dir".to_string()),
                    timeout_ms: None,
                },
            },
            source: ToolCallSource::Direct,
            raw_tool_call_id: None,
        };

        assert_eq!(
            call.execution_subject("/fallback/dir"),
            ("ls -l".to_string(), "/custom/dir".to_string(), "shell")
        );
    }

    #[test]
    fn tool_call_execution_subject_falls_back_for_shell_without_cwd() {
        let call = ToolCall {
            name: "shell".to_string(),
            payload: ToolPayload::LocalShell {
                params: codewhale_protocol::LocalShellParams {
                    command: "echo hello".to_string(),
                    cwd: None,
                    timeout_ms: None,
                },
            },
            source: ToolCallSource::Direct,
            raw_tool_call_id: None,
        };

        assert_eq!(
            call.execution_subject("/fallback/dir"),
            (
                "echo hello".to_string(),
                "/fallback/dir".to_string(),
                "shell"
            )
        );
    }

    #[test]
    fn tool_call_execution_subject_uses_tool_name_for_non_shell_payloads() {
        let call = ToolCall {
            name: "my_tool".to_string(),
            payload: ToolPayload::Function {
                arguments: "{}".to_string(),
            },
            source: ToolCallSource::Direct,
            raw_tool_call_id: None,
        };

        assert_eq!(
            call.execution_subject("/fallback/dir"),
            ("my_tool".to_string(), "/fallback/dir".to_string(), "tool")
        );
    }
}
