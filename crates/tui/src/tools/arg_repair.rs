//! 针对格式错误的工具调用输入的确定性 JSON 参数修复。
//!
//! DeepSeek 将 `tool_calls.function.arguments` 以增量形式流式传输。常见的两种失败
//! 形态：(a) SSE 块边界在 JSON 字符串中间切断，重组后留下尾部逗号或未闭合的括号；
//! (b) 部分本地后端在 JSON 字符串值中发出字面控制字符。
//!
//! 修复阶梯在报告不可恢复的输入前依次运行五个阶段：
//!
//!  1. 严格解析——如果能解析则完成。
//!  2. 去除字符串值中的字面控制字符。
//!  3. 去除 `}` 或 `]` 前的尾部逗号。
//!  4. 平衡花括号/方括号（追加闭合符号）。
//!  5. 如果增量为负，则去除多余的闭合符号。

use serde_json::Value;

/// 我们会尝试修复的原始参数最大长度（1 MiB）。
const MAX_ARG_LEN: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArgRepairError {
    #[error("argument exceeded {0} chars; refusing to repair")]
    TooLarge(usize),
    #[error("argument could not be repaired into valid JSON")]
    Unrepairable,
}

/// 将原始 JSON 参数字符串修复为有效的 `serde_json::Value`。
///
/// 运行确定性阶梯；成功时返回解析后的值。
pub fn repair(raw: &str) -> Result<Value, ArgRepairError> {
    if raw.len() > MAX_ARG_LEN {
        return Err(ArgRepairError::TooLarge(raw.len()));
    }
    // 阶段 1：严格解析
    if let Ok(v) = serde_json::from_str(raw) {
        return Ok(v);
    }
    // 阶段 2：去除字符串中的控制字符
    let mut s = strip_control_chars_in_strings(raw);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // 阶段 3：去除尾部逗号
    s = strip_trailing_commas(&s);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // 阶段 4：平衡括号
    s = balance_braces(&s, 50);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    // 阶段 5：去除多余的闭合符号
    s = strip_excess_closers(&s);
    if let Ok(v) = serde_json::from_str(&s) {
        return Ok(v);
    }
    Err(ArgRepairError::Unrepairable)
}

/// 去除 JSON 字符串值中出现的 ASCII 控制字符（0x00–0x1F 除 \t、\n、\r 外）。
/// 我们逐字符遍历，跟踪当前是否在字符串内（在两个未转义的双引号之间）。
fn strip_control_chars_in_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            continue;
        }
        if in_string && (ch as u32) < 0x20 && ch != '\t' && ch != '\n' && ch != '\r' {
            // 丢弃字符串内的控制字符
            continue;
        }
        out.push(ch);
    }
    out
}

/// 去除 `}` 或 `]` 前的尾部逗号。
fn strip_trailing_commas(s: &str) -> String {
    // 反复替换 ",}" 和 ",]" 直到稳定（处理嵌套情况）。
    let mut out = s.to_string();
    loop {
        let prev = out.clone();
        out = out.replace(",}", "}").replace(",]", "]");
        // 处理字符串末尾的尾部逗号
        out = out.trim_end_matches(',').to_string();
        if out == prev {
            break;
        }
    }
    out
}

/// 平衡花括号和方括号：统计 `{`/`}` 和 `[`/`]` 的数量，如果增量
/// 为正（开比闭多），则追加闭合符号。限制迭代次数，避免严重损坏的输入无限循环。
fn balance_braces(s: &str, max_iter: usize) -> String {
    let mut out = s.to_string();
    for _ in 0..max_iter {
        let brace_delta: i32 = out
            .chars()
            .map(|ch| match ch {
                '{' => 1,
                '}' => -1,
                _ => 0,
            })
            .sum();
        let bracket_delta: i32 = out
            .chars()
            .map(|ch| match ch {
                '[' => 1,
                ']' => -1,
                _ => 0,
            })
            .sum();
        if brace_delta <= 0 && bracket_delta <= 0 {
            break;
        }
        // 以相反顺序追加需要的闭合符号（两者都不平衡时，方括号在花括号前以确保正确嵌套）。
        for _ in 0..bracket_delta.max(0) {
            out.push(']');
        }
        for _ in 0..brace_delta.max(0) {
            out.push('}');
        }
    }
    out
}

/// 当增量为负（闭比开多）时去除多余的闭合符号。
fn strip_excess_closers(s: &str) -> String {
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '}' => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                    out.push(ch);
                }
                // 否则丢弃多余的闭合符号
            }
            ']' => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                    out.push(ch);
                }
            }
            '{' => {
                brace_depth += 1;
                out.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_parse_passes_through() {
        let v = repair(r#"{"path": "hello.txt"}"#).unwrap();
        assert_eq!(v, json!({"path": "hello.txt"}));
    }

    #[test]
    fn repairs_trailing_comma() {
        let v = repair(r#"{"path": "hello.txt",}"#).unwrap();
        assert_eq!(v, json!({"path": "hello.txt"}));
    }

    #[test]
    fn repairs_trailing_comma_in_array() {
        let v = repair(r#"["a", "b",]"#).unwrap();
        assert_eq!(v, json!(["a", "b"]));
    }

    #[test]
    fn repairs_missing_close_brace() {
        let v = repair(r#"{"path": "hello.txt""#).unwrap();
        assert_eq!(v, json!({"path": "hello.txt"}));
    }

    #[test]
    fn repairs_missing_close_bracket() {
        let v = repair(r#"["a", "b""#).unwrap();
        assert_eq!(v, json!(["a", "b"]));
    }

    #[test]
    fn strips_embedded_control_chars() {
        // 字符串值中的原始 \x0B（垂直制表符）
        let raw = "{\"key\": \"val\x0Bue\"}";
        let v = repair(raw).unwrap();
        assert_eq!(v, json!({"key": "value"}));
    }

    #[test]
    fn rejects_empty_string() {
        assert!(matches!(repair(""), Err(ArgRepairError::Unrepairable)));
    }

    #[test]
    fn rejects_gibberish() {
        assert!(matches!(
            repair("not json at all"),
            Err(ArgRepairError::Unrepairable)
        ));
    }

    #[test]
    fn balances_nested_braces() {
        let v = repair(r#"{"outer": {"inner": "val""#).unwrap();
        assert_eq!(v, json!({"outer": {"inner": "val"}}));
    }

    #[test]
    fn strips_excess_closers() {
        let v = repair(r#"{"key": "val"}}"#).unwrap();
        assert_eq!(v, json!({"key": "val"}));
    }

    #[test]
    fn handles_double_encoded_json() {
        // 这是一个包含 JSON 对象字面量的有效 JSON 字符串。
        // repair 将其解析为字符串；引擎现有的回退机制
        // （parse_tool_input）会解开字符串并重新解析。
        let v = repair(r#""{\"path\": \"hello.txt\"}""#).unwrap();
        assert_eq!(v, Value::String(r#"{"path": "hello.txt"}"#.to_string()));
    }

    #[test]
    fn oversize_input_rejected() {
        let big = "x".repeat(MAX_ARG_LEN + 1);
        assert!(repair(&big).is_err());
    }

    #[test]
    fn repairs_brace_balance_with_trailing_comma() {
        let v = repair(r#"{"a": 1,"#).unwrap();
        assert_eq!(v, json!({"a": 1}));
    }
}
