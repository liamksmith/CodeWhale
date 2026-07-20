//! 遗留的文本式工具调用解析器，专门用来处理 DeepSeek 模型以"纯文本"形式输出的工具调用。
//!
//! 现在引擎已经改用结构化的工具调用（即模型原生返回的 JSON 格式），不再调用这个解析器，
//! 保留只是为了参考和调试。
//! 
//! - 核心背景：旧版 DeepSeek 模型不具备原生 JSON 工具调用能力，而是把工具调用作为普通文本输出
//! （比如在聊天内容中夹杂特殊标记）。这个解析器就是用来把那些标记从文本中"抠"出来，转成结构化的
//! 工具调用。现在引擎已经直接使用模型返回的原生 tool_calls JSON 字段，本文件不再被调用。
//!
//! 一些 DeepSeek 模型以各种文本格式输出工具调用：
//! ```text
//! [TOOL_CALL]
//! {tool => "tool_name", args => {...}}
//! [/TOOL_CALL]
//! ```
//!
//! 第二种格式：XML 风格，以自定义标签 `codewhale:tool_call` 包裹，内部用 
//! `invoke` 标签表示调用，`parameter` 标签表示参数。
//! 或者 XML 风格格式：
//! ```text
//! <codewhale:tool_call>
//! <invoke name="tool_name">
//! <parameter name="arg">value</parameter>
//! </invoke>
//! </codewhale:tool_call>
//! ```
//!
//! 此模块把上述文本模式解析成 `ParsedToolCall` 结构体。

use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

/// 从文本内容中解析出的工具调用。
#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    /// 工具的名称，比如 `"read_file"`、`"exec_shell"`。
    pub name: String,
    /// 工具的参数，用 `serde_json::Value` 表示。因为参数可能是对象（`{"path": "test.txt"}`）、
    /// 数组等各种 JSON 形态，用动态的 `Value` 比强类型更灵活——毕竟解析器的任务是从任意文本中"猜"出参数结构。
    pub args: Value,
    /// 解析器自己生成的唯一标识符。为什么需要？因为上游的引擎循环期望每个工具调用都有一个 ID（原生 tool call 
    /// 是由模型提供的），这里模拟了这个约定。
    pub id: String,
}

/// 从文本中解析工具调用的结果。整个解析操作的打包返回值。
#[derive(Debug)]
pub struct ParseResult {
    /// 清洗后的文本——移除原始text中所有工具调用标记（包括 `` 本身）。
    pub clean_text: String,
    /// 在文本中找到的解析后的工具调用。
    pub tool_calls: Vec<ParsedToolCall>,
}

static TOOL_CALL_REGEX: OnceLock<Regex> = OnceLock::new();
static XML_TOOL_CALL_REGEX: OnceLock<Regex> = OnceLock::new();
static INVOKE_REGEX: OnceLock<Regex> = OnceLock::new();
static THINKING_REGEX: OnceLock<Regex> = OnceLock::new();
static FAKE_TOOL_WRAPPER_REGEX: OnceLock<Regex> = OnceLock::new();

const FAKE_TOOL_CALL_MARKERS: &[&str] = &[
    "<function_calls>",
    "<｜DSML｜tool_calls>",
    "<｜DSML｜invoke ",
    "<|DSML|tool_calls>",
    "<|DSML|invoke ",
    "<|dsml|tool_calls>",
    "<|dsml|invoke ",
    "<|tool_calls>",
];

fn get_tool_call_regex() -> &'static Regex {
    TOOL_CALL_REGEX.get_or_init(|| {
        // 匹配 [TOOL_CALL] ... [/TOOL_CALL] 块
        Regex::new(r"(?s)\[TOOL_CALL\]\s*(.*?)\s*\[/TOOL_CALL\]")
            .expect("TOOL_CALL 正则表达式有效")
    })
}

fn get_xml_tool_call_regex() -> &'static Regex {
    XML_TOOL_CALL_REGEX.get_or_init(|| {
        // 匹配 <codewhale:tool_call>...</codewhale:tool_call> 或类似的 XML 模式
        Regex::new(r"(?s)<(?:codewhale:)?tool_call[^>]*>\s*(.*?)\s*</(?:codewhale:)?tool_call>")
            .expect("XML tool_call 正则表达式有效")
    })
}

fn get_invoke_regex() -> &'static Regex {
    INVOKE_REGEX.get_or_init(|| {
        // 匹配 <invoke name="tool_name">...</invoke> 模式
        Regex::new(r#"(?s)<invoke\s+name\s*=\s*"([^"]+)"[^>]*>(.*?)</invoke>"#)
            .expect("invoke 正则表达式有效")
    })
}

fn get_thinking_regex() -> &'static Regex {
    THINKING_REGEX.get_or_init(|| {
        // 匹配思考块，包括部分关闭标签
        Regex::new(r"(?s)</?(?:think|thinking)[^>]*>").expect("thinking 正则表达式有效")
    })
}

fn get_fake_tool_wrapper_regex() -> &'static Regex {
    FAKE_TOOL_WRAPPER_REGEX.get_or_init(|| {
        Regex::new(
            r#"(?s)<function_calls>.*?</function_calls>|<｜DSML｜tool_calls>.*?</｜DSML｜tool_calls>|<｜DSML｜invoke\b[^>]*>.*?</｜DSML｜invoke>|<\|DSML\|tool_calls>.*?</\|DSML\|tool_calls>|<\|DSML\|invoke\b[^>]*>.*?</\|DSML\|invoke>|<\|dsml\|tool_calls>.*?</\|dsml\|tool_calls>|<\|dsml\|invoke\b[^>]*>.*?</\|dsml\|invoke>|<\|tool_calls>.*?</\|tool_calls>"#,
        )
        .expect("fake tool wrapper 正则表达式有效")
    })
}

/// 从文本内容中解析工具调用。
/// 返回清理后的文本（移除标记后）以及解析出的任何工具调用。
pub fn parse_tool_calls(text: &str) -> ParseResult {
    let mut tool_calls = Vec::new();
    let mut clean_text = text.to_string();
    let mut id_counter = 0;

    // 首先，移除思考标签
    let thinking_regex = get_thinking_regex();
    clean_text = thinking_regex.replace_all(&clean_text, "").to_string();

    // 解析 [TOOL_CALL] 格式
    let regex = get_tool_call_regex();
    for cap in regex.captures_iter(text) {
        let (Some(full_match), Some(inner)) = (cap.get(0), cap.get(1)) else {
            continue;
        };
        let full_match = full_match.as_str();
        let inner = inner.as_str().trim();

        if let Some(parsed) = parse_tool_call_inner(inner, &mut id_counter) {
            tool_calls.push(parsed);
        }

        clean_text = clean_text.replace(full_match, "");
    }

    // 解析 XML 风格 <codewhale:tool_call> 或 <tool_call> 格式
    let xml_regex = get_xml_tool_call_regex();
    for cap in xml_regex.captures_iter(text) {
        let (Some(full_match), Some(inner)) = (cap.get(0), cap.get(1)) else {
            continue;
        };
        let full_match = full_match.as_str();
        let inner = inner.as_str().trim();

        // 解析内部的 invoke 块
        if let Some(parsed) = parse_invoke_block(inner, &mut id_counter) {
            tool_calls.push(parsed);
        } else if let Some(parsed) = parse_tool_call_inner(inner, &mut id_counter) {
            tool_calls.push(parsed);
        }

        clean_text = clean_text.replace(full_match, "");
    }

    // 也解析可能未被包裹的独立 <invoke> 块
    let invoke_regex = get_invoke_regex();
    for cap in invoke_regex.captures_iter(&clean_text.clone()) {
        let (Some(full_match), Some(tool_name), Some(inner)) = (cap.get(0), cap.get(1), cap.get(2))
        else {
            continue;
        };
        let full_match = full_match.as_str();
        let tool_name = tool_name.as_str();
        let inner = inner.as_str();

        let args = parse_xml_parameters(inner);
        id_counter += 1;
        tool_calls.push(ParsedToolCall {
            name: tool_name.to_string(),
            args,
            id: format!("xml_tool_{id_counter}"),
        });

        clean_text = clean_text.replace(full_match, "");
    }

    clean_text = get_fake_tool_wrapper_regex()
        .replace_all(&clean_text, "")
        .to_string();

    // 清理多余空格和空行
    clean_text = clean_text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    ParseResult {
        clean_text,
        tool_calls,
    }
}

/// 将 `<invoke>` 块解析为工具调用。
fn parse_invoke_block(content: &str, id_counter: &mut u32) -> Option<ParsedToolCall> {
    let invoke_regex = get_invoke_regex();
    let cap = invoke_regex.captures(content)?;

    let tool_name = cap.get(1)?.as_str();
    let inner = cap.get(2)?.as_str();

    let args = parse_xml_parameters(inner);

    *id_counter += 1;
    Some(ParsedToolCall {
        name: tool_name.to_string(),
        args,
        id: format!("xml_tool_{id_counter}"),
    })
}

/// 解析 XML 风格参数，如 <parameter name="foo">value</parameter>
fn parse_xml_parameters(content: &str) -> Value {
    let param_regex = Regex::new(
        "<(?:parameter|param)\\s+name\\s*=\\s*\"([^\"]+)\"[^>]*>(.*?)</(?:parameter|param)>",
    )
    .ok();
    let simple_tag_regex =
        Regex::new("<([a-zA-Z_][a-zA-Z0-9_]*)>(.*?)</([a-zA-Z_][a-zA-Z0-9_]*)>").ok();

    let mut map = serde_json::Map::new();

    // 尝试解析 <parameter name="...">value</parameter>
    if let Some(regex) = param_regex {
        for cap in regex.captures_iter(content) {
            if let (Some(name), Some(value)) = (cap.get(1), cap.get(2)) {
                let name_str = name.as_str();
                let value_str = value.as_str().trim();

                // 尝试解析为 JSON，否则用作字符串
                let json_value = serde_json::from_str(value_str)
                    .unwrap_or_else(|_| Value::String(value_str.to_string()));
                map.insert(name_str.to_string(), json_value);
            }
        }
    }

    // 也尝试解析 <tagname>value</tagname> 格式
    if let Some(regex) = simple_tag_regex {
        for cap in regex.captures_iter(content) {
            if let (Some(name), Some(value), Some(close)) = (cap.get(1), cap.get(2), cap.get(3)) {
                if name.as_str() != close.as_str() {
                    continue;
                }
                let name_str = name.as_str();
                // 跳过已知的包装标签
                if ["invoke", "tool_call", "parameter", "param"].contains(&name_str) {
                    continue;
                }
                let value_str = value.as_str().trim();
                if !map.contains_key(name_str) {
                    let json_value = serde_json::from_str(value_str)
                        .unwrap_or_else(|_| Value::String(value_str.to_string()));
                    map.insert(name_str.to_string(), json_value);
                }
            }
        }
    }

    Value::Object(map)
}

/// 解析 `TOOL_CALL` 块的内部内容。
fn parse_tool_call_inner(inner: &str, id_counter: &mut u32) -> Option<ParsedToolCall> {
    // 首先尝试解析为 JSON
    if let Ok(json) = serde_json::from_str::<Value>(inner) {
        return parse_from_json(&json, id_counter);
    }

    // 尝试箭头语法：{tool => "name", args => {...}}
    if let Some(parsed) = parse_arrow_syntax(inner, id_counter) {
        return Some(parsed);
    }

    // 尝试从任意格式提取工具名称和参数
    parse_flexible_format(inner, id_counter)
}

/// 从 JSON 对象解析。
fn parse_from_json(json: &Value, id_counter: &mut u32) -> Option<ParsedToolCall> {
    let obj = json.as_object()?;

    // 尝试工具名称的不同字段名
    let name = obj
        .get("tool")
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("function"))
        .and_then(|v| v.as_str())?
        .to_string();

    // 尝试参数的不同字段名
    let args = obj
        .get("args")
        .or_else(|| obj.get("arguments"))
        .or_else(|| obj.get("input"))
        .or_else(|| obj.get("parameters"))
        .cloned()
        .unwrap_or(json!({}));

    *id_counter += 1;
    Some(ParsedToolCall {
        name,
        args,
        id: format!("text_tool_{id_counter}"),
    })
}

/// 解析箭头语法：{tool => "name", args => {...}}
fn parse_arrow_syntax(inner: &str, id_counter: &mut u32) -> Option<ParsedToolCall> {
    // 提取工具名称
    let tool_regex = Regex::new(r#"tool\s*=>\s*"([^"]+)""#).ok()?;
    let name = tool_regex.captures(inner)?.get(1)?.as_str().to_string();

    // 提取参数——尝试在 "args =>" 之后找到 JSON 对象
    let args = if let Some(args_start) = inner.find("args =>") {
        let args_str = inner[args_start + 7..].trim();
        // 首先尝试解析为 JSON
        if let Ok(args_json) = serde_json::from_str::<Value>(args_str) {
            args_json
        } else if let Some(brace_start) = args_str.find('{') {
            // 尝试提取花括号之间的内容
            let mut brace_count = 0;
            let mut end_idx = brace_start;
            for (i, c) in args_str[brace_start..].chars().enumerate() {
                match c {
                    '{' => brace_count += 1,
                    '}' => {
                        brace_count -= 1;
                        if brace_count == 0 {
                            end_idx = brace_start + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let content = &args_str[brace_start + 1..end_idx - 1];

            // 尝试解析为 JSON
            if let Ok(json) = serde_json::from_str::<Value>(&format!("{{{content}}}")) {
                json
            } else {
                // 尝试 CLI 风格参数：--arg_name "value" 或 --arg_name value
                parse_cli_style_args(content)
            }
        } else {
            json!({})
        }
    } else {
        json!({})
    };

    *id_counter += 1;
    Some(ParsedToolCall {
        name,
        args,
        id: format!("text_tool_{id_counter}"),
    })
}

/// 解析 CLI 风格参数：--`arg_name` "value" 或 --`arg_name` value
fn parse_cli_style_args(content: &str) -> Value {
    let mut map = serde_json::Map::new();

    // 模式：--arg_name "value" 或 --arg_name 'value' 或 --arg_name value
    let arg_regex =
        Regex::new(r#"--([a-zA-Z_][a-zA-Z0-9_]*)\s+(?:"([^"]*)"|'([^']*)'|(\S+))"#).ok();

    if let Some(regex) = arg_regex {
        for cap in regex.captures_iter(content) {
            if let Some(arg_name) = cap.get(1) {
                let arg_name = arg_name.as_str();
                // 从任意匹配的捕获组获取值
                let value = cap
                    .get(2)
                    .or_else(|| cap.get(3))
                    .or_else(|| cap.get(4))
                    .map_or("", |m| m.as_str());

                // 尝试解析为 JSON 值，否则用作字符串
                let json_value = serde_json::from_str(value)
                    .unwrap_or_else(|_| Value::String(value.to_string()));
                map.insert(arg_name.to_string(), json_value);
            }
        }
    }

    // 也尝试简单的 key=value 格式
    let kv_regex =
        Regex::new(r#"([a-zA-Z_][a-zA-Z0-9_]*)\s*[:=]\s*(?:"([^"]*)"|'([^']*)'|(\S+))"#).ok();
    if let Some(regex) = kv_regex {
        for cap in regex.captures_iter(content) {
            if let Some(key) = cap.get(1) {
                let key = key.as_str();
                if !map.contains_key(key) {
                    let value = cap
                        .get(2)
                        .or_else(|| cap.get(3))
                        .or_else(|| cap.get(4))
                        .map_or("", |m| m.as_str());
                    let json_value = serde_json::from_str(value)
                        .unwrap_or_else(|_| Value::String(value.to_string()));
                    map.insert(key.to_string(), json_value);
                }
            }
        }
    }

    Value::Object(map)
}

/// 尝试解析灵活格式。
fn parse_flexible_format(inner: &str, id_counter: &mut u32) -> Option<ParsedToolCall> {
    // 查找常见模式，如：
    // tool: list_dir
    // name: "list_dir"
    // function: list_dir

    let patterns = [(
        r#"(?:tool|name|function)\s*[:=]\s*"?([a-zA-Z_][a-zA-Z0-9_]*)"?"#,
        1,
    )];

    for (pattern, group) in patterns {
        if let Ok(regex) = Regex::new(pattern)
            && let Some(cap) = regex.captures(inner)
            && let Some(name_match) = cap.get(group)
        {
            let name = name_match.as_str().to_string();

            // 尝试提取 args/input 作为 JSON
            let args = extract_json_object(inner).unwrap_or(json!({}));

            *id_counter += 1;
            return Some(ParsedToolCall {
                name,
                args,
                id: format!("text_tool_{id_counter}"),
            });
        }
    }

    None
}

/// 从字符串中提取第一个 JSON 对象。
fn extract_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let mut brace_count = 0;
    let mut end_idx = start;

    for (i, c) in text[start..].chars().enumerate() {
        match c {
            '{' => brace_count += 1,
            '}' => {
                brace_count -= 1;
                if brace_count == 0 {
                    end_idx = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    let json_str = &text[start..end_idx];
    serde_json::from_str(json_str).ok()
}

/// 检查文本是否包含工具调用标记（任一格式）。
pub fn has_tool_call_markers(text: &str) -> bool {
    text.contains("[TOOL_CALL]")
        || text.contains("<codewhale:tool_call")
        || text.contains("<tool_call")
        || text.contains("<invoke ")
        || FAKE_TOOL_CALL_MARKERS
            .iter()
            .any(|marker| text.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arrow_syntax() {
        let text = r#"I'll list the directory.
[TOOL_CALL]
{tool => "list_dir", args => {}}
[/TOOL_CALL]"#;

        let result = parse_tool_calls(text);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "list_dir");
        assert_eq!(result.clean_text, "I'll list the directory.");
    }

    #[test]
    fn test_parse_json_syntax() {
        let text = r#"Let me check.
[TOOL_CALL]
{"tool": "read_file", "args": {"path": "test.txt"}}
[/TOOL_CALL]"#;

        let result = parse_tool_calls(text);
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "read_file");
        assert_eq!(result.tool_calls[0].args["path"], "test.txt");
    }

    #[test]
    fn test_parse_multiple_tool_calls() {
        let text = r#"First I'll list, then read.
[TOOL_CALL]
{tool => "list_dir", args => {}}
[/TOOL_CALL]
[TOOL_CALL]
{tool => "read_file", args => {"path": "file.txt"}}
[/TOOL_CALL]"#;

        let result = parse_tool_calls(text);
        assert_eq!(result.tool_calls.len(), 2);
        assert_eq!(result.tool_calls[0].name, "list_dir");
        assert_eq!(result.tool_calls[1].name, "read_file");
    }

    #[test]
    fn test_no_tool_calls() {
        let text = "Just some regular text without any tool calls.";
        let result = parse_tool_calls(text);
        assert!(result.tool_calls.is_empty());
        assert_eq!(result.clean_text, text);
    }

    #[test]
    fn test_dsml_wrappers_are_stripped_without_execution() {
        let text = "before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"path\" string=\"true\">secret.txt</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nafter";

        assert!(has_tool_call_markers(text));
        let result = parse_tool_calls(text);

        assert!(result.tool_calls.is_empty());
        assert!(result.clean_text.contains("before"));
        assert!(result.clean_text.contains("after"));
        assert!(!result.clean_text.contains("DSML"));
        assert!(!result.clean_text.contains("read_file"));
        assert!(!result.clean_text.contains("secret.txt"));
    }

    #[test]
    fn test_ascii_dsml_wrappers_are_stripped_without_execution() {
        let text = "before <|DSML|invoke name=\"grep_files\"><|DSML|parameter name=\"pattern\">SECRET</|DSML|parameter></|DSML|invoke> after";

        assert!(has_tool_call_markers(text));
        let result = parse_tool_calls(text);

        assert!(result.tool_calls.is_empty());
        assert!(result.clean_text.contains("before"));
        assert!(result.clean_text.contains("after"));
        assert!(!result.clean_text.contains("DSML"));
        assert!(!result.clean_text.contains("grep_files"));
        assert!(!result.clean_text.contains("SECRET"));
    }

    #[test]
    fn test_has_markers() {
        assert!(has_tool_call_markers("[TOOL_CALL]test[/TOOL_CALL]"));
        assert!(has_tool_call_markers(
            "<｜DSML｜tool_calls>...</｜DSML｜tool_calls>"
        ));
        assert!(!has_tool_call_markers("no markers here"));
    }
}
