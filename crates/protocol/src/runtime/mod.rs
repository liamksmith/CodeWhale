use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION: u32 = 1;
pub const RUNTIME_API_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEventEnvelope {
    #[serde(default = "default_runtime_event_envelope_schema_version")]
    pub schema_version: u32,
    pub seq: u64,
    pub event: String,
    pub kind: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    pub payload: Value,
    #[serde(default)]
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn default_runtime_event_envelope_schema_version() -> u32 {
    RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION
}

// ---------------------------------------------------------------------------
// 能力声明
// ---------------------------------------------------------------------------

/// `GET /v1/runtime/info` 广告的固定能力映射。
///
/// 所有字段在序列化时都是必需的，以便客户端可以依赖该形状。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub threads: bool,
    pub turns: bool,
    pub turn_steer: bool,
    pub turn_interrupt: bool,
    pub event_replay: bool,
    pub external_tools: bool,
    pub environments: bool,
    pub worker_runtime: bool,
}

/// `GET /v1/runtime/info` 广告的实验性选择加入标志。
///
/// 字段是增量的，当被较旧的服务器省略时默认值为 `false`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeExperimentalCapabilities {
    #[serde(default)]
    pub environments: bool,
}

// ---------------------------------------------------------------------------
// 外部工具桥接协议类型
// ---------------------------------------------------------------------------

/// 由运行时客户端注册的动态外部工具规范。
///
/// 规范中的 JSON 示例：
///
/// ```json
/// {
///   "namespace": "tau_bench",
///   "name": "get_reservation",
///   "description": "Look up an airline reservation.",
///   "input_schema": {
///     "type": "object",
///     "properties": {
///       "reservation_id": { "type": "string" }
///     },
///     "required": ["reservation_id"],
///     "additionalProperties": false
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DynamicToolSpec {
    /// 可选的命名空间，用于对相关工具进行分组（例如 `"tau_bench"`）。
    /// 如果存在，运行时可以将工具以 `<namespace>::<name>`
    /// 的形式暴露给模型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// 简短的工具名称。与 `namespace` 结合形成唯一的工具 ID。
    pub name: String,

    /// 暴露给模型的人类可读描述。
    pub description: String,

    /// 描述工具输入参数的 JSON Schema。
    pub input_schema: Value,

    /// 如果为 true，运行时可以延迟模式验证 / 工具加载，直到
    /// 模型实际调用该工具。
    ///
    /// 默认为 `false`，以便省略此字段的较旧客户端行为保持一致。
    #[serde(default)]
    pub defer_loading: bool,
}

/// 在线程详情和事件负载中显示的动态工具项生命周期状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DynamicToolItemStatus {
    InProgress,
    Completed,
    Failed,
}

/// 标识由运行时发出的动态工具调用请求的参数。
///
/// 这是 `tool_call.requested` 事件的类型化负载，也是运行时查找
/// 挂起调用时使用的自然标识符。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DynamicToolCallParams {
    pub thread_id: String,
    pub turn_id: String,
    pub call_id: String,

    /// 注册工具时可选的命名空间。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// 模型调用的工具名称。
    pub tool: String,

    /// 模型提供的参数，针对 `input_schema` 进行验证。
    pub arguments: Value,
}

/// 运行时客户端执行动态工具后提交的结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DynamicToolCallResult {
    /// 客户端工具执行是否成功。
    pub success: bool,

    /// 工具返回的内容片段。
    ///
    /// 省略时默认为空向量，以便客户端可以发送最小的
    /// `{ "success": false }` 负载。
    #[serde(default)]
    pub content: Vec<DynamicToolCallContent>,
}

/// [`DynamicToolCallResult`] 内部的单个内容片段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DynamicToolCallContent {
    InputText { text: String },
    InputImage { image_url: String },
}

// ---------------------------------------------------------------------------
// 环境目标协议类型
// ---------------------------------------------------------------------------

/// 为轮次的 shell/文件系统工作选择的环境目标。
///
/// JSON 示例：
///
/// ```json
/// {
///   "environment_id": "local",
///   "cwd": "/workspace"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnEnvironmentParams {
    pub environment_id: String,
    pub cwd: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dynamic_tool_spec_roundtrip() {
        let spec = DynamicToolSpec {
            namespace: Some("tau_bench".into()),
            name: "get_reservation".into(),
            description: "Look up an airline reservation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "reservation_id": { "type": "string" }
                },
                "required": ["reservation_id"],
                "additionalProperties": false
            }),
            defer_loading: false,
        };

        let serialized = serde_json::to_string(&spec).unwrap();
        let deserialized: DynamicToolSpec = serde_json::from_str(&serialized).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn dynamic_tool_spec_omits_defer_loading_defaults_false() {
        let json = r#"{
            "namespace": "tau_bench",
            "name": "get_reservation",
            "description": "Look up an airline reservation.",
            "input_schema": { "type": "object" }
        }"#;

        let spec: DynamicToolSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.namespace, Some("tau_bench".into()));
        assert_eq!(spec.name, "get_reservation");
        assert!(!spec.defer_loading);
    }

    #[test]
    fn dynamic_tool_item_status_snake_case() {
        assert_eq!(
            serde_json::to_string(&DynamicToolItemStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::from_str::<DynamicToolItemStatus>("\"completed\"").unwrap(),
            DynamicToolItemStatus::Completed
        );
        assert_eq!(
            serde_json::from_str::<DynamicToolItemStatus>("\"failed\"").unwrap(),
            DynamicToolItemStatus::Failed
        );
    }

    #[test]
    fn dynamic_tool_call_params_roundtrip() {
        let params = DynamicToolCallParams {
            thread_id: "thr_123".into(),
            turn_id: "turn_456".into(),
            call_id: "call_abc".into(),
            namespace: Some("tau_bench".into()),
            tool: "get_reservation".into(),
            arguments: json!({ "reservation_id": "ABC123" }),
        };

        let serialized = serde_json::to_string(&params).unwrap();
        let deserialized: DynamicToolCallParams = serde_json::from_str(&serialized).unwrap();
        assert_eq!(params, deserialized);
    }

    #[test]
    fn dynamic_tool_call_content_roundtrip() {
        let content = vec![
            DynamicToolCallContent::InputText {
                text: "{\"status\":\"confirmed\"}".into(),
            },
            DynamicToolCallContent::InputImage {
                image_url: "http://example.com/receipt.png".into(),
            },
        ];

        let value = serde_json::to_value(&content).unwrap();
        let deserialized: Vec<DynamicToolCallContent> = serde_json::from_value(value).unwrap();
        assert_eq!(content, deserialized);

        // 验证规范期望的确切 JSON 标签名称。
        assert_eq!(
            serde_json::to_string(&DynamicToolCallContent::InputText { text: "x".into() }).unwrap(),
            r#"{"type":"input_text","text":"x"}"#
        );
        assert_eq!(
            serde_json::to_string(&DynamicToolCallContent::InputImage {
                image_url: "y".into()
            })
            .unwrap(),
            r#"{"type":"input_image","image_url":"y"}"#
        );
    }

    #[test]
    fn dynamic_tool_call_result_defaults_empty_content() {
        let json = r#"{ "success": false }"#;
        let result: DynamicToolCallResult = serde_json::from_str(json).unwrap();
        assert!(!result.success);
        assert!(result.content.is_empty());
    }

    #[test]
    fn dynamic_tool_call_result_roundtrip_with_content() {
        let result = DynamicToolCallResult {
            success: true,
            content: vec![DynamicToolCallContent::InputText {
                text: "done".into(),
            }],
        };

        let serialized = serde_json::to_string(&result).unwrap();
        let deserialized: DynamicToolCallResult = serde_json::from_str(&serialized).unwrap();
        assert_eq!(result, deserialized);
    }

    #[test]
    fn turn_environment_params_roundtrip() {
        let env = TurnEnvironmentParams {
            environment_id: "local".into(),
            cwd: PathBuf::from("/workspace"),
        };

        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: TurnEnvironmentParams = serde_json::from_str(&serialized).unwrap();
        assert_eq!(env, deserialized);

        // 验证来自规范的 JSON 能否直接反序列化。
        let from_spec = r#"{
            "environment_id": "local",
            "cwd": "/workspace"
        }"#;
        let parsed: TurnEnvironmentParams = serde_json::from_str(from_spec).unwrap();
        assert_eq!(parsed.environment_id, "local");
        assert_eq!(parsed.cwd, PathBuf::from("/workspace"));
    }

    #[test]
    fn runtime_capabilities_serializes_expected_shape() {
        let caps = RuntimeCapabilities {
            threads: true,
            turns: true,
            turn_steer: true,
            turn_interrupt: true,
            event_replay: true,
            external_tools: false,
            environments: false,
            worker_runtime: false,
        };
        let value = serde_json::to_value(&caps).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.get("threads").unwrap(), &json!(true));
        assert_eq!(obj.get("external_tools").unwrap(), &json!(false));
        assert!(obj.contains_key("worker_runtime"));
    }

    #[test]
    fn runtime_event_envelope_schema_version_default() {
        let json = r#"{
            "seq": 1,
            "event": "test",
            "kind": "test",
            "thread_id": "thr_1",
            "timestamp": "2026-06-12T00:00:00Z",
            "payload": {}
        }"#;
        let envelope: RuntimeEventEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(
            envelope.schema_version,
            RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION
        );
    }
}
