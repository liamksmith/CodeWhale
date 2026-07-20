//! JSON Schema 的字节级规范化，确保前缀缓存稳定性。
//!
//! 当 MCP 服务器返回工具 schema 时，每个 schema 对象内的字段顺序
//! 以及 `required` / `dependentRequired` 数组中的条目顺序
//! 可能在不同重连之间发生变化。此模块将那些顺序标准化，
//! 使得两个逻辑上等价的 schema 在序列化后始终产生相同的字节。
//!
//! 方法与 `reasonix/internal/provider/schema_canonicalize.go` 类似：
//!
//! 1. 按字母顺序对每个 `"required"` 数组排序。
//! 2. 按字母顺序对每个 `"dependentRequired"` 子数组排序。
//! 3. 递归进入所有嵌套的对象和数组。
//!
//! 当启用 `preserve_order` 时（此 crate 确实启用），
//! `serde_json::Value::Object` 使用 `IndexMap`。
//! 因此我们使用排序后的键重建映射，以保证确定的键顺序。

use serde_json::Value;

/// 原地递归规范化 JSON Schema 值。
///
/// 规范化后，两个语义等价的 schema
///（相同的键、相同的 `required` 集合、相同的 `dependentRequired` 集合）
/// 无论原始字段或数组顺序如何，都将序列化为字节级相同的 JSON。
pub fn canonicalize_schema(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // 对 `required` 数组排序（根据 JSON Schema 规范，它们是集合）。
            if let Some(Value::Array(req)) = map.get_mut("required") {
                sort_string_array(req);
            }
            // 对 `dependentRequired` 子数组排序。
            if let Some(Value::Object(deps)) = map.get_mut("dependentRequired") {
                for dep_value in deps.values_mut() {
                    if let Value::Array(arr) = dep_value {
                        sort_string_array(arr);
                    }
                }
            }
            // 递归进入每个子值。
            for v in map.values_mut() {
                canonicalize_schema(v);
            }
            // 使用排序后的键重建映射，确保序列化结果是确定的。
            // serde_json::Map 由 IndexMap 支持（preserve_order），
            // 没有 drain()，因此我们交换到临时映射并重建。
            let old = std::mem::take(map);
            let mut entries: Vec<(String, Value)> = old.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in entries {
                map.insert(k, v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                canonicalize_schema(v);
            }
        }
        _ => {}
    }
}

/// 原地按字母顺序排序 JSON 字符串值数组。
///
/// 非字符串条目会保留在末尾，保持原始的相对顺序。
fn sort_string_array(arr: &mut [Value]) {
    arr.sort_by(|a, b| match (a.as_str(), b.as_str()) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_required_array() {
        let mut schema = json!({
            "type": "object",
            "required": ["z", "a", "m"],
            "properties": {}
        });
        canonicalize_schema(&mut schema);
        assert_eq!(schema["required"], json!(["a", "m", "z"]));
    }

    #[test]
    fn equivalent_ordering_matches() {
        // 仅在字段顺序和 required 顺序上不同的两个 schema
        // 必须序列化为相同的字节。
        let mut a = json!({
            "required": ["b", "a"],
            "properties": {"x": {}, "y": {}},
            "type": "object"
        });
        let mut b = json!({
            "type": "object",
            "properties": {"y": {}, "x": {}},
            "required": ["a", "b"]
        });
        canonicalize_schema(&mut a);
        canonicalize_schema(&mut b);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "逻辑等价的 schema 必须产生相同的字节"
        );
    }

    #[test]
    fn sorts_dependent_required() {
        let mut schema = json!({
            "type": "object",
            "dependentRequired": {
                "x": ["z", "a"],
                "y": ["m", "b"]
            }
        });
        canonicalize_schema(&mut schema);
        assert_eq!(schema["dependentRequired"]["x"], json!(["a", "z"]));
        assert_eq!(schema["dependentRequired"]["y"], json!(["b", "m"]));
    }

    #[test]
    fn recursive_into_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "required": ["z", "a"],
                    "properties": {}
                }
            }
        });
        canonicalize_schema(&mut schema);
        assert_eq!(
            schema["properties"]["nested"]["required"],
            json!(["a", "z"])
        );
    }

    #[test]
    fn preserves_non_required_array_order() {
        // 非 `required` 或 `dependentRequired` 的数组应
        // 保持其语义顺序（例如 enum 值、oneOf 项）。
        let mut schema = json!({
            "type": "string",
            "enum": ["z", "a", "m"]
        });
        canonicalize_schema(&mut schema);
        assert_eq!(schema["enum"], json!(["z", "a", "m"]));
    }

    #[test]
    fn handles_empty_schema() {
        let mut schema = json!({});
        canonicalize_schema(&mut schema);
        assert_eq!(schema, json!({}));
    }

    #[test]
    fn handles_deeply_nested() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "level1": {
                    "type": "object",
                    "properties": {
                        "level2": {
                            "type": "object",
                            "required": ["z", "a"]
                        }
                    }
                }
            }
        });
        canonicalize_schema(&mut schema);
        assert_eq!(
            schema["properties"]["level1"]["properties"]["level2"]["required"],
            json!(["a", "z"])
        );
    }

    #[test]
    fn key_order_is_alphabetical_after_canonicalize() {
        let mut schema = json!({
            "z_field": 1,
            "a_field": 2,
            "m_field": 3
        });
        canonicalize_schema(&mut schema);
        let keys: Vec<&str> = schema
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(keys, vec!["a_field", "m_field", "z_field"]);
    }
}
