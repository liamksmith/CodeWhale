//! `responseSchema` 解码：将子代理的回复解析为 JSON 并针对调用者提供的 schema 进行验证。
//!
//! 重试语义位于驱动器侧（它拥有子代理及其提示词）；VM 仅进行解析和验证——
//! 不是有效 JSON 或未通过 schema 验证的回复会在等待的 `task()` 调用上抛出异常。

/// 编译调用者的 schema。在生成之前调用，以便格式错误的 schema 快速失败，而不是浪费一个子代理。
pub(crate) fn compile_schema(schema: &serde_json::Value) -> Result<jsonschema::Validator, String> {
    jsonschema::validator_for(schema)
        .map_err(|err| format!("task(): invalid responseSchema: {err}"))
}

/// 将 `text` 解析为 JSON（允许有效负载周围有一个 Markdown 代码围栏）并针对 `validator` 进行验证。
pub(crate) fn decode_reply(
    text: &str,
    validator: &jsonschema::Validator,
) -> Result<serde_json::Value, String> {
    let candidate = strip_code_fence(text);
    let parsed: serde_json::Value = serde_json::from_str(candidate).map_err(|err| {
        format!("task(): responseSchema was set but the reply is not valid JSON: {err}")
    })?;
    let errors = validator
        .iter_errors(&parsed)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!(
            "task(): reply failed responseSchema validation: {}",
            errors.join("; ")
        ));
    }
    Ok(parsed)
}

/// 如果整个回复包裹在一个 Markdown 代码围栏（``` 或 ```json）中，
/// 则返回围栏内的内容；否则原样返回修整后的回复。
fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(body) = rest.strip_suffix("```") else {
        return trimmed;
    };
    // 丢弃开头围栏行上的可选语言标签。
    match body.split_once('\n') {
        Some((first_line, tail)) if !first_line.trim().is_empty() => tail.trim(),
        _ => body.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validator() -> jsonschema::Validator {
        compile_schema(&json!({
            "type": "object",
            "properties": { "refuted": { "type": "boolean" } },
            "required": ["refuted"],
        }))
        .expect("schema compiles")
    }

    #[test]
    fn decodes_plain_json() {
        let value = decode_reply(r#"{"refuted": true}"#, &validator()).unwrap();
        assert_eq!(value, json!({"refuted": true}));
    }

    #[test]
    fn decodes_fenced_json() {
        let text = "```json\n{\"refuted\": false}\n```";
        let value = decode_reply(text, &validator()).unwrap();
        assert_eq!(value, json!({"refuted": false}));
    }

    #[test]
    fn rejects_non_json() {
        let err = decode_reply("definitely not json", &validator()).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
    }

    #[test]
    fn rejects_schema_violation() {
        let err = decode_reply(r#"{"refuted": "yes"}"#, &validator()).unwrap_err();
        assert!(err.contains("responseSchema validation"), "{err}");
    }

    #[test]
    fn rejects_invalid_schema_before_spawn() {
        let err = compile_schema(&json!({"type": "not-a-type"})).unwrap_err();
        assert!(err.contains("invalid responseSchema"), "{err}");
    }
}
